//! # Aether WebSocket (RFC 6455)
//!
//! Opening handshake validation, strict structural frame parsing, and a bounded connection
//! driver with fragmentation assembly, automatic pong replies, and the closing handshake.
//!
//! Extensions (permessage-deflate) and subprotocol negotiation are not implemented.

use aether_core::error::AetherError;

pub mod connection;
pub mod handshake;

pub use connection::{Message, WebSocket, WebSocketLimits, WsError};
pub use handshake::{accept_for_key, validate_handshake, HandshakeError, WS_GUID};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum WsOpcode {
    Continuation = 0x0,
    Text = 0x1,
    Binary = 0x2,
    Close = 0x8,
    Ping = 0x9,
    Pong = 0xA,
}

pub struct WsFrameView<'a> {
    pub fin: bool,
    pub opcode: WsOpcode,
    /// Payload exactly as it appeared on the wire. Client frames remain masked; use
    /// [`parse_client_frame`] for a validated, unmasked copy.
    pub payload: &'a [u8],
}

/// A validated client-to-server frame with an unmasked payload.
pub struct ClientFrame {
    pub fin: bool,
    pub opcode: WsOpcode,
    pub payload: Vec<u8>,
}

/// Parses one frame without imposing masking direction.
///
/// Applies the structural rules of RFC 6455 §5.2: RSV bits must be clear (no negotiated
/// extension), opcodes must be defined, control frames must be final and carry at most 125
/// bytes, and length encodings must be minimal. Returns `Ok(None)` when more bytes are
/// needed.
pub fn parse_frame(buf: &[u8]) -> Result<Option<(WsFrameView<'_>, usize)>, AetherError> {
    if buf.len() < 2 {
        return Ok(None);
    }
    let b0 = buf[0];
    let b1 = buf[1];

    if b0 & 0x70 != 0 {
        return Err(AetherError::MalformedRequest);
    }

    let fin = (b0 & 0x80) != 0;
    let opcode = match b0 & 0x0F {
        0x0 => WsOpcode::Continuation,
        0x1 => WsOpcode::Text,
        0x2 => WsOpcode::Binary,
        0x8 => WsOpcode::Close,
        0x9 => WsOpcode::Ping,
        0xA => WsOpcode::Pong,
        // 0x3-0x7 and 0xB-0xF are reserved and must fail the connection.
        _ => return Err(AetherError::MalformedRequest),
    };

    let masked = (b1 & 0x80) != 0;
    let len_code = b1 & 0x7F;
    let mut payload_len = len_code as u64;
    let mut offset = 2usize;

    if len_code == 126 {
        if buf.len() < 4 {
            return Ok(None);
        }
        payload_len = u16::from_be_bytes([buf[2], buf[3]]) as u64;
        if payload_len < 126 {
            return Err(AetherError::MalformedRequest);
        }
        offset = 4;
    } else if len_code == 127 {
        if buf.len() < 10 {
            return Ok(None);
        }
        payload_len = u64::from_be_bytes(buf[2..10].try_into().unwrap());
        // The most significant bit must be zero and the encoding must be minimal.
        if payload_len >> 63 != 0 || payload_len < 65_536 {
            return Err(AetherError::MalformedRequest);
        }
        offset = 10;
    }

    let is_control = (b0 & 0x08) != 0;
    if is_control && (!fin || payload_len > 125) {
        return Err(AetherError::MalformedRequest);
    }

    if masked {
        offset += 4;
    }

    let payload_len = usize::try_from(payload_len).map_err(|_| AetherError::BodyLimitExceeded)?;
    let total_len = offset
        .checked_add(payload_len)
        .ok_or(AetherError::BodyLimitExceeded)?;
    if buf.len() < total_len {
        return Ok(None);
    }

    let payload = &buf[offset..total_len];
    Ok(Some((
        WsFrameView {
            fin,
            opcode,
            payload,
        },
        total_len,
    )))
}

/// Parses a client-to-server frame, which RFC 6455 §5.1 requires to be masked, and returns
/// an unmasked payload. The caller must already know the frame is complete; partial frames
/// return `Ok(None)`.
pub fn parse_client_frame(buf: &[u8]) -> Result<Option<(ClientFrame, usize)>, AetherError> {
    let Some((frame, consumed)) = parse_frame(buf)? else {
        return Ok(None);
    };
    // `parse_frame` leaves the direction unconstrained; clients must mask (§5.1).
    let masked = buf[1] & 0x80 != 0;
    if !masked {
        return Err(AetherError::MalformedRequest);
    }

    let mask_offset = consumed - frame.payload.len() - 4;
    let mask = &buf[mask_offset..mask_offset + 4];
    let mut payload = frame.payload.to_vec();
    for (i, byte) in payload.iter_mut().enumerate() {
        *byte ^= mask[i & 3];
    }

    Ok(Some((
        ClientFrame {
            fin: frame.fin,
            opcode: frame.opcode,
            payload,
        },
        consumed,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(fin: bool, opcode: u8, payload: &[u8], mask: Option<[u8; 4]>) -> Vec<u8> {
        let mut out = vec![(if fin { 0x80 } else { 0 }) | opcode];
        let masked_bit = if mask.is_some() { 0x80 } else { 0 };
        out.push(masked_bit | payload.len() as u8);
        if let Some(key) = mask {
            out.extend_from_slice(&key);
            out.extend(payload.iter().enumerate().map(|(i, b)| b ^ key[i & 3]));
        } else {
            out.extend_from_slice(payload);
        }
        out
    }

    #[test]
    fn test_unmasked_text_frame() {
        let raw = frame(true, 0x1, b"hello", None);
        let (f, consumed) = parse_frame(&raw).unwrap().unwrap();
        assert!(f.fin);
        assert_eq!(f.opcode, WsOpcode::Text);
        assert_eq!(f.payload, b"hello");
        assert_eq!(consumed, raw.len());
    }

    #[test]
    fn test_client_frame_is_unmasked() {
        let raw = frame(true, 0x1, b"hello", Some([1, 2, 3, 4]));
        let (f, consumed) = parse_client_frame(&raw).unwrap().unwrap();
        assert_eq!(f.opcode, WsOpcode::Text);
        assert_eq!(f.payload, b"hello");
        assert_eq!(consumed, raw.len());
    }

    #[test]
    fn test_unmasked_client_frame_rejected() {
        let raw = frame(true, 0x1, b"hello", None);
        assert!(matches!(
            parse_client_frame(&raw),
            Err(AetherError::MalformedRequest)
        ));
    }

    #[test]
    fn test_reserved_bits_and_opcodes_rejected() {
        let mut rsv = frame(true, 0x1, b"", None);
        rsv[0] |= 0x40;
        assert!(parse_frame(&rsv).is_err());

        for opcode in [0x3u8, 0x7, 0xB, 0xF] {
            let raw = frame(true, opcode, b"", None);
            assert!(parse_frame(&raw).is_err(), "opcode {opcode:#x} must be rejected");
        }
    }

    #[test]
    fn test_control_frame_rules() {
        // Fragmented control frame.
        let raw = frame(false, 0x9, b"", None);
        assert!(parse_frame(&raw).is_err());

        // Oversized control frame.
        let raw = frame(true, 0x9, &[0u8; 126], None);
        assert!(parse_frame(&raw).is_err());

        // Valid ping.
        let raw = frame(true, 0x9, b"ping", None);
        assert_eq!(parse_frame(&raw).unwrap().unwrap().0.opcode, WsOpcode::Ping);
    }

    #[test]
    fn test_partial_frames_request_more_data() {
        let raw = frame(true, 0x2, b"abcdef", Some([9, 8, 7, 6]));
        for cut in 0..raw.len() {
            assert!(parse_frame(&raw[..cut]).unwrap().is_none(), "cut={cut}");
        }
    }

    #[test]
    fn test_length_encodings_are_minimal_and_bounded() {
        // 126 encoded in the 7-bit form (not minimal) when it fits.
        let mut raw = vec![0x82, 126, 0, 100];
        raw.extend(std::iter::repeat_n(0u8, 100));
        assert!(parse_frame(&raw).is_err(), "non-minimal 16-bit length must fail");

        // 64-bit length below 65536 is non-minimal.
        let mut raw = vec![0x82, 127];
        raw.extend_from_slice(&1000u64.to_be_bytes());
        raw.extend(std::iter::repeat_n(0u8, 1000));
        assert!(parse_frame(&raw).is_err());

        // MSB set in a 64-bit length is illegal.
        let mut raw = vec![0x82, 127];
        raw.extend_from_slice(&(1u64 << 63 | 65_536).to_be_bytes());
        assert!(parse_frame(&raw).is_err());
    }

    #[test]
    fn test_hostile_inputs_do_not_panic() {
        for len in 0..64 {
            let raw = vec![0xFFu8; len];
            let _ = parse_frame(&raw);
            let _ = parse_client_frame(&raw);
        }
    }
}
