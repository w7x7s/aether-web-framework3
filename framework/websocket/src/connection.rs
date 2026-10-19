//! # Bounded RFC 6455 connection driver
//!
//! Reads masked client frames, assembles fragmented messages, answers pings, and performs
//! the closing handshake. Every read is bounded before the payload is buffered, so a frame
//! header declaring a terabyte cannot make the server allocate it.

use aether_core::error::AetherError;
use std::fmt;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{parse_client_frame, WsOpcode};

/// A complete application message. Control frames are handled by the driver and never
/// surfaced here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Text(String),
    Binary(Vec<u8>),
}

/// Resource bounds for a connection.
#[derive(Debug, Clone, Copy)]
pub struct WebSocketLimits {
    /// Maximum size of a reassembled message.
    pub max_message_bytes: usize,
    /// Maximum size of a single frame payload. A frame larger than this is rejected as soon
    /// as its header arrives, before the payload is read.
    pub max_frame_bytes: usize,
    /// Inactivity policy for `recv`: the maximum gap between raw bytes arriving from the
    /// peer before the connection is failed with [`WsError::IdleTimeout`]. Any frame counts
    /// (including pings), so a heartbeat satisfies it. `None` disables the timeout.
    pub read_timeout: Option<std::time::Duration>,
}

impl Default for WebSocketLimits {
    fn default() -> Self {
        Self {
            max_message_bytes: 16 * 1024 * 1024,
            max_frame_bytes: 16 * 1024 * 1024,
            // A silent peer for two minutes is treated as gone: idle sockets otherwise
            // pin a task, a connection slot, and their read buffer indefinitely.
            read_timeout: Some(std::time::Duration::from_secs(120)),
        }
    }
}

#[derive(Debug)]
pub enum WsError {
    Io(std::io::Error),
    Protocol(&'static str),
    MessageTooLarge,
    InvalidUtf8,
    /// A send was attempted after a close frame went out.
    Closed,
    /// The peer closed the TCP stream without a WebSocket close frame.
    UnexpectedEof,
    /// No bytes arrived from the peer within `WebSocketLimits::read_timeout`.
    IdleTimeout,
}

impl fmt::Display for WsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "WebSocket I/O error: {err}"),
            Self::Protocol(msg) => write!(f, "WebSocket protocol error: {msg}"),
            Self::MessageTooLarge => f.write_str("WebSocket message exceeds the configured limit"),
            Self::InvalidUtf8 => f.write_str("WebSocket text message is not valid UTF-8"),
            Self::Closed => f.write_str("WebSocket close frame already sent"),
            Self::UnexpectedEof => f.write_str("WebSocket stream ended without a close frame"),
            Self::IdleTimeout => f.write_str("WebSocket peer was idle past the configured read timeout"),
        }
    }
}

impl std::error::Error for WsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for WsError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

/// Server side of an established WebSocket connection.
///
/// The generic parameter lets the driver run over a `TcpStream`, a TLS stream, or an
/// in-memory transport in tests. Outgoing frames are never masked, as required of a server
/// by RFC 6455 §5.1.
pub struct WebSocket<S> {
    io: S,
    buffer: Vec<u8>,
    limits: WebSocketLimits,
    fragments: Vec<u8>,
    fragment_opcode: Option<WsOpcode>,
    close_sent: bool,
}

impl<S> WebSocket<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    /// Wraps an already-upgraded stream. `initial` is any bytes that were read past the
    /// opening HTTP request (the first WebSocket frames can arrive in the same TCP segment).
    pub fn from_parts(io: S, initial: Vec<u8>, limits: WebSocketLimits) -> Self {
        Self {
            io,
            buffer: initial,
            limits,
            fragments: Vec::new(),
            fragment_opcode: None,
            close_sent: false,
        }
    }

    pub fn limits(&self) -> WebSocketLimits {
        self.limits
    }

    /// Receives the next application message. Returns `Ok(None)` after the closing
    /// handshake; the caller should then drop the connection.
    ///
    /// Pings are answered and pongs are consumed internally, so a caller blocked in `recv`
    /// is still responsive to control traffic.
    pub async fn recv(&mut self) -> Result<Option<Message>, WsError> {
        loop {
            let parsed = match parse_client_frame(&self.buffer) {
                Ok(result) => result,
                Err(err) => {
                    let mapped = map_parse_error(err);
                    return Err(self.fail(mapped).await);
                }
            };

            let Some((frame, consumed)) = parsed else {
                if let Err(err) = self.check_buffered_header() {
                    return Err(self.fail(err).await);
                }
                // The inactivity policy bounds each network wait; buffered frame bytes
                // already in `self.buffer` make progress without touching the socket.
                let read_result = match self.limits.read_timeout {
                    Some(limit) => match tokio::time::timeout(limit, self.read_more()).await {
                        Ok(inner) => inner,
                        Err(_) => return Err(WsError::IdleTimeout),
                    },
                    None => self.read_more().await,
                };
                match read_result {
                    Ok(()) => continue,
                    Err(err) => return Err(err),
                }
            };

            self.buffer.drain(..consumed);

            // `check_buffered_header` enforces `max_frame_bytes` before each socket read,
            // but a complete frame that was already sitting in the bytes read past the HTTP
            // upgrade never passes through that gate. Re-check the decoded frame so the
            // per-frame limit cannot be skipped by pipelining the first frame with the
            // handshake.
            if frame.payload.len() > self.limits.max_frame_bytes {
                return Err(self.fail(WsError::MessageTooLarge).await);
            }

            match frame.opcode {
                WsOpcode::Ping => {
                    self.write_frame(WsOpcode::Pong, &frame.payload, true).await?;
                }
                WsOpcode::Pong => {}
                WsOpcode::Close => {
                    if let Err(err) = validate_close_payload(&frame.payload) {
                        return Err(self.fail(err).await);
                    }
                    if !self.close_sent {
                        self.write_frame(WsOpcode::Close, &frame.payload, true)
                            .await?;
                        self.close_sent = true;
                    }
                    return Ok(None);
                }
                WsOpcode::Text | WsOpcode::Binary | WsOpcode::Continuation => {
                    match self.accept_data_frame(frame) {
                        Ok(Some(message)) => return Ok(Some(message)),
                        Ok(None) => {}
                        Err(err) => return Err(self.fail(err).await),
                    }
                }
            }
        }
    }

    pub async fn send(&mut self, message: Message) -> Result<(), WsError> {
        match message {
            Message::Text(text) => self.send_text(&text).await,
            Message::Binary(bytes) => self.send_binary(&bytes).await,
        }
    }

    pub async fn send_text(&mut self, text: &str) -> Result<(), WsError> {
        self.ensure_open(text.len())?;
        self.write_frame(WsOpcode::Text, text.as_bytes(), true).await
    }

    pub async fn send_binary(&mut self, payload: &[u8]) -> Result<(), WsError> {
        self.ensure_open(payload.len())?;
        self.write_frame(WsOpcode::Binary, payload, true).await
    }

    /// Sends an unsolicited ping. The peer's pong is consumed by [`Self::recv`].
    pub async fn ping(&mut self, payload: &[u8]) -> Result<(), WsError> {
        if payload.len() > 125 {
            return Err(WsError::Protocol("control frame payload exceeds 125 bytes"));
        }
        self.write_frame(WsOpcode::Ping, payload, true).await
    }

    /// Starts the closing handshake. Idempotent: a second call does nothing.
    ///
    /// The caller still needs to call [`Self::recv`] (or simply drop the connection) to
    /// receive the peer's close frame; servers may close the TCP stream after a short wait.
    pub async fn close(&mut self, code: u16, reason: &str) -> Result<(), WsError> {
        if self.close_sent {
            return Ok(());
        }
        if !valid_close_code(code) {
            return Err(WsError::Protocol("invalid close status code"));
        }
        if reason.len() > 123 {
            return Err(WsError::Protocol("close reason exceeds 123 bytes"));
        }
        let mut payload = Vec::with_capacity(2 + reason.len());
        payload.extend_from_slice(&code.to_be_bytes());
        payload.extend_from_slice(reason.as_bytes());
        self.write_frame(WsOpcode::Close, &payload, true).await?;
        self.close_sent = true;
        Ok(())
    }

    /// Returns the underlying transport after the session ends.
    pub fn into_inner(self) -> S {
        self.io
    }

    fn ensure_open(&self, payload_len: usize) -> Result<(), WsError> {
        if self.close_sent {
            return Err(WsError::Closed);
        }
        if payload_len > self.limits.max_message_bytes {
            return Err(WsError::MessageTooLarge);
        }
        Ok(())
    }

    fn accept_data_frame(
        &mut self,
        frame: crate::ClientFrame,
    ) -> Result<Option<Message>, WsError> {
        match frame.opcode {
            WsOpcode::Text | WsOpcode::Binary => {
                if self.fragment_opcode.is_some() {
                    return Err(WsError::Protocol(
                        "new data frame while a fragmented message is in progress",
                    ));
                }
                if frame.fin {
                    return self.complete_message(frame.opcode, frame.payload);
                }
                if frame.payload.len() > self.limits.max_message_bytes {
                    return Err(WsError::MessageTooLarge);
                }
                self.fragment_opcode = Some(frame.opcode);
                self.fragments = frame.payload;
                Ok(None)
            }
            WsOpcode::Continuation => {
                let Some(opcode) = self.fragment_opcode else {
                    return Err(WsError::Protocol("continuation frame without a start frame"));
                };
                if self.fragments.len() + frame.payload.len() > self.limits.max_message_bytes {
                    return Err(WsError::MessageTooLarge);
                }
                self.fragments.extend_from_slice(&frame.payload);
                if frame.fin {
                    self.fragment_opcode = None;
                    let payload = std::mem::take(&mut self.fragments);
                    self.complete_message(opcode, payload)
                } else {
                    Ok(None)
                }
            }
            _ => Err(WsError::Protocol("unexpected frame opcode")),
        }
    }

    fn complete_message(
        &mut self,
        opcode: WsOpcode,
        payload: Vec<u8>,
    ) -> Result<Option<Message>, WsError> {
        if payload.len() > self.limits.max_message_bytes {
            return Err(WsError::MessageTooLarge);
        }
        match opcode {
            WsOpcode::Text => match String::from_utf8(payload) {
                Ok(text) => Ok(Some(Message::Text(text))),
                Err(_) => Err(WsError::InvalidUtf8),
            },
            WsOpcode::Binary => Ok(Some(Message::Binary(payload))),
            _ => Err(WsError::Protocol("unexpected message opcode")),
        }
    }

    /// Rejects a frame whose declared size is over the limit before its payload is read,
    /// and refuses unmasked client frames as soon as the second byte arrives.
    fn check_buffered_header(&self) -> Result<(), WsError> {
        if self.buffer.len() >= 2 && self.buffer[1] & 0x80 == 0 {
            return Err(WsError::Protocol("client frames must be masked"));
        }
        let Some(payload_len) = declared_payload_len(&self.buffer) else {
            return Ok(());
        };
        if payload_len > self.limits.max_frame_bytes {
            return Err(WsError::MessageTooLarge);
        }
        Ok(())
    }

    async fn read_more(&mut self) -> Result<(), WsError> {
        let mut chunk = [0u8; 8192];
        let read = self.io.read(&mut chunk).await?;
        if read == 0 {
            return Err(WsError::UnexpectedEof);
        }
        self.buffer.extend_from_slice(&chunk[..read]);
        Ok(())
    }

    async fn write_frame(
        &mut self,
        opcode: WsOpcode,
        payload: &[u8],
        fin: bool,
    ) -> Result<(), WsError> {
        let is_control = matches!(opcode, WsOpcode::Close | WsOpcode::Ping | WsOpcode::Pong);
        if is_control && payload.len() > 125 {
            return Err(WsError::Protocol("control frame payload exceeds 125 bytes"));
        }

        let mut header = [0u8; 10];
        header[0] = (if fin { 0x80 } else { 0 }) | opcode as u8;
        let header_len = if payload.len() <= 125 {
            header[1] = payload.len() as u8;
            2
        } else if payload.len() <= u16::MAX as usize {
            header[1] = 126;
            header[2..4].copy_from_slice(&(payload.len() as u16).to_be_bytes());
            4
        } else {
            header[1] = 127;
            header[2..10].copy_from_slice(&(payload.len() as u64).to_be_bytes());
            10
        };

        self.io.write_all(&header[..header_len]).await?;
        self.io.write_all(payload).await?;
        self.io.flush().await?;
        Ok(())
    }

    /// Best-effort protocol failure notification. The connection is unusable afterwards
    /// regardless, so a failed close write does not replace the original error.
    async fn fail(&mut self, err: WsError) -> WsError {
        if !self.close_sent {
            let (code, reason) = match &err {
                WsError::MessageTooLarge => (1009u16, "message too large"),
                WsError::InvalidUtf8 => (1007, "invalid UTF-8"),
                WsError::Protocol(_) => (1002, "protocol error"),
                _ => (1011, "internal error"),
            };
            let _ = self.close(code, reason).await;
        }
        err
    }
}

fn map_parse_error(err: AetherError) -> WsError {
    match err {
        AetherError::BodyLimitExceeded => WsError::MessageTooLarge,
        _ => WsError::Protocol("malformed WebSocket frame"),
    }
}

fn declared_payload_len(buf: &[u8]) -> Option<usize> {
    if buf.len() < 2 {
        return None;
    }
    match buf[1] & 0x7f {
        len @ 0..=125 => Some(len as usize),
        126 => {
            if buf.len() < 4 {
                None
            } else {
                Some(u16::from_be_bytes([buf[2], buf[3]]) as usize)
            }
        }
        _ => {
            if buf.len() < 10 {
                return None;
            }
            let len = u64::from_be_bytes(buf[2..10].try_into().expect("10 bytes"));
            Some(usize::try_from(len).unwrap_or(usize::MAX))
        }
    }
}

fn validate_close_payload(payload: &[u8]) -> Result<(), WsError> {
    match payload.len() {
        0 => Ok(()),
        1 => Err(WsError::Protocol("close frame with a 1-byte payload")),
        _ => {
            let code = u16::from_be_bytes([payload[0], payload[1]]);
            if !valid_close_code(code) {
                return Err(WsError::Protocol("invalid close status code"));
            }
            std::str::from_utf8(&payload[2..])
                .map(|_| ())
                .map_err(|_| WsError::InvalidUtf8)
        }
    }
}

fn valid_close_code(code: u16) -> bool {
    matches!(code, 1000..=1003 | 1007..=1014 | 3000..=4999)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    fn client_frame(fin: bool, opcode: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
        let mut out = vec![(if fin { 0x80 } else { 0 }) | opcode];
        let len = payload.len();
        if len <= 125 {
            out.push(0x80 | len as u8);
        } else if len <= u16::MAX as usize {
            out.push(0x80 | 126);
            out.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            out.push(0x80 | 127);
            out.extend_from_slice(&(len as u64).to_be_bytes());
        }
        out.extend_from_slice(&mask);
        out.extend(
            payload
                .iter()
                .enumerate()
                .map(|(i, byte)| byte ^ mask[i & 3]),
        );
        out
    }

    async fn read_frame(peer: &mut tokio::io::DuplexStream) -> (u8, Vec<u8>) {
        let mut head = [0u8; 2];
        peer.read_exact(&mut head).await.unwrap();
        let opcode = head[0] & 0x0f;
        let masked = head[1] & 0x80 != 0;
        assert!(!masked, "server frames must not be masked");
        let len = match head[1] & 0x7f {
            126 => {
                let mut ext = [0u8; 2];
                peer.read_exact(&mut ext).await.unwrap();
                u16::from_be_bytes(ext) as usize
            }
            127 => {
                let mut ext = [0u8; 8];
                peer.read_exact(&mut ext).await.unwrap();
                u64::from_be_bytes(ext) as usize
            }
            n => n as usize,
        };
        let mut payload = vec![0u8; len];
        peer.read_exact(&mut payload).await.unwrap();
        (opcode, payload)
    }

    #[tokio::test]
    async fn reassembles_fragments_and_answers_ping_in_between() {
        let (server, mut peer) = tokio::io::duplex(4096);
        let mut ws = WebSocket::from_parts(server, Vec::new(), WebSocketLimits::default());

        peer.write_all(&client_frame(false, 0x1, b"hel", [1, 2, 3, 4]))
            .await
            .unwrap();
        peer.write_all(&client_frame(true, 0x9, b"p", [5, 6, 7, 8]))
            .await
            .unwrap();
        peer.write_all(&client_frame(true, 0x0, b"lo", [9, 1, 2, 3]))
            .await
            .unwrap();

        let message = ws.recv().await.unwrap();
        assert_eq!(message, Some(Message::Text("hello".to_string())));

        let (opcode, payload) = read_frame(&mut peer).await;
        assert_eq!(opcode, 0xA, "ping must be answered with pong");
        assert_eq!(payload, b"p");
    }

    #[tokio::test]
    async fn rejects_unmasked_client_frame_with_protocol_close() {
        let (server, mut peer) = tokio::io::duplex(4096);
        let mut ws = WebSocket::from_parts(server, Vec::new(), WebSocketLimits::default());

        peer.write_all(&[0x81, 0x05]).await.unwrap();
        peer.write_all(b"hello").await.unwrap();

        let err = ws.recv().await.unwrap_err();
        assert!(matches!(err, WsError::Protocol(_)));

        let (opcode, payload) = read_frame(&mut peer).await;
        assert_eq!(opcode, 0x8);
        assert_eq!(u16::from_be_bytes([payload[0], payload[1]]), 1002);
    }

    #[tokio::test]
    async fn close_is_echoed_and_ends_the_stream() {
        let (server, mut peer) = tokio::io::duplex(4096);
        let mut ws = WebSocket::from_parts(server, Vec::new(), WebSocketLimits::default());

        let mut payload = 1000u16.to_be_bytes().to_vec();
        payload.extend_from_slice(b"bye");
        peer.write_all(&client_frame(true, 0x8, &payload, [4, 3, 2, 1]))
            .await
            .unwrap();

        assert_eq!(ws.recv().await.unwrap(), None);
        let (opcode, echoed) = read_frame(&mut peer).await;
        assert_eq!(opcode, 0x8);
        assert_eq!(echoed, payload);
    }

    #[tokio::test]
    async fn rejects_text_with_invalid_utf8() {
        let (server, mut peer) = tokio::io::duplex(4096);
        let mut ws = WebSocket::from_parts(server, Vec::new(), WebSocketLimits::default());

        peer.write_all(&client_frame(true, 0x1, &[0xff, 0xfe], [7, 7, 7, 7]))
            .await
            .unwrap();

        assert!(matches!(ws.recv().await, Err(WsError::InvalidUtf8)));
        let (opcode, payload) = read_frame(&mut peer).await;
        assert_eq!(opcode, 0x8);
        assert_eq!(u16::from_be_bytes([payload[0], payload[1]]), 1007);
    }

    #[tokio::test]
    async fn continuation_without_start_is_rejected() {
        let (server, mut peer) = tokio::io::duplex(4096);
        let mut ws = WebSocket::from_parts(server, Vec::new(), WebSocketLimits::default());

        peer.write_all(&client_frame(true, 0x0, b"orphan", [1, 1, 1, 1]))
            .await
            .unwrap();

        assert!(matches!(ws.recv().await, Err(WsError::Protocol(_))));
    }

    #[tokio::test]
    async fn declared_oversized_frame_is_rejected_before_payload_arrives() {
        let (server, mut peer) = tokio::io::duplex(4096);
        let limits = WebSocketLimits {
            max_message_bytes: 64,
            max_frame_bytes: 64,
            ..WebSocketLimits::default()
        };
        let mut ws = WebSocket::from_parts(server, Vec::new(), limits);

        // Header alone declares a 70-byte masked payload; the payload is never sent.
        let mut header = vec![0x82, 0x80 | 70];
        header.extend_from_slice(&[1, 2, 3, 4]);
        peer.write_all(&header).await.unwrap();

        let err = tokio::time::timeout(std::time::Duration::from_secs(1), ws.recv())
            .await
            .expect("must not wait for the payload");
        assert!(matches!(err, Err(WsError::MessageTooLarge)));
    }

    #[tokio::test]
    async fn reassembled_message_limit_is_enforced() {
        let (server, mut peer) = tokio::io::duplex(8192);
        let limits = WebSocketLimits {
            max_message_bytes: 8,
            max_frame_bytes: 64,
            ..WebSocketLimits::default()
        };
        let mut ws = WebSocket::from_parts(server, Vec::new(), limits);

        peer.write_all(&client_frame(false, 0x2, b"12345", [1, 2, 3, 4]))
            .await
            .unwrap();
        peer.write_all(&client_frame(true, 0x0, b"6789", [5, 6, 7, 8]))
            .await
            .unwrap();

        assert!(matches!(ws.recv().await, Err(WsError::MessageTooLarge)));
    }

    #[tokio::test]
    async fn server_output_is_unmasked_and_round_trips() {
        let (server, mut peer) = tokio::io::duplex(4096);
        let mut ws = WebSocket::from_parts(server, Vec::new(), WebSocketLimits::default());

        ws.send_text("hello").await.unwrap();
        let (opcode, payload) = read_frame(&mut peer).await;
        assert_eq!(opcode, 0x1);
        assert_eq!(payload, b"hello");

        peer.write_all(&client_frame(true, 0x1, b"world", [9, 9, 9, 9]))
            .await
            .unwrap();
        assert_eq!(
            ws.recv().await.unwrap(),
            Some(Message::Text("world".to_string()))
        );
    }

    #[tokio::test]
    async fn initial_bytes_from_the_upgrade_request_are_not_lost() {
        let (server, mut peer) = tokio::io::duplex(4096);
        let initial = client_frame(true, 0x1, b"early", [4, 4, 4, 4]);
        let mut ws = WebSocket::from_parts(server, initial, WebSocketLimits::default());

        assert_eq!(
            ws.recv().await.unwrap(),
            Some(Message::Text("early".to_string()))
        );
        let _ = &mut peer;
    }

    #[tokio::test]
    async fn close_code_and_payload_rules() {
        let (server, mut peer) = tokio::io::duplex(4096);
        let mut ws = WebSocket::from_parts(server, Vec::new(), WebSocketLimits::default());

        assert!(matches!(
            ws.close(1005, "").await,
            Err(WsError::Protocol(_))
        ));
        ws.close(1000, "done").await.unwrap();
        assert!(matches!(ws.send_text("late").await, Err(WsError::Closed)));

        let (opcode, payload) = read_frame(&mut peer).await;
        assert_eq!(opcode, 0x8);
        assert_eq!(u16::from_be_bytes([payload[0], payload[1]]), 1000);
        assert_eq!(&payload[2..], b"done");
    }
}
