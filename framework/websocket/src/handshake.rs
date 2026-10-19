//! # RFC 6455 opening handshake
//!
//! Validates the client's upgrade request and computes `Sec-WebSocket-Accept`. The SHA-1
//! digest is used exactly where RFC 6455 requires it (the accept nonce); it is not a
//! security primitive here, and `ring`'s legacy SHA-1 implementation is used to avoid a
//! hand-written hash.

use ring::digest::{digest, SHA1_FOR_LEGACY_USE_ONLY};
use std::fmt;

/// The fixed GUID from RFC 6455 §1.3.
pub const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Why an opening handshake was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeError {
    MethodNotGet,
    MissingUpgrade,
    MissingConnection,
    MissingVersion,
    UnsupportedVersion,
    MissingKey,
    InvalidKey,
}

impl fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::MethodNotGet => "WebSocket handshakes must use GET",
            Self::MissingUpgrade => "missing or invalid Upgrade: websocket header",
            Self::MissingConnection => "missing or invalid Connection: Upgrade header",
            Self::MissingVersion => "missing Sec-WebSocket-Version header",
            Self::UnsupportedVersion => "only WebSocket version 13 is supported",
            Self::MissingKey => "missing Sec-WebSocket-Key header",
            Self::InvalidKey => "Sec-WebSocket-Key is not canonical base64 for 16 bytes",
        };
        f.write_str(message)
    }
}

impl std::error::Error for HandshakeError {}

/// Validate the handshake fields and return the `Sec-WebSocket-Accept` value.
///
/// `connection` may be a comma-separated list (`keep-alive, Upgrade`); the tokens are
/// matched case-insensitively. `key` must be canonical base64 encoding exactly 16 bytes.
pub fn validate_handshake(
    method: &str,
    upgrade: Option<&str>,
    connection: Option<&str>,
    version: Option<&str>,
    key: Option<&str>,
) -> Result<String, HandshakeError> {
    if !method.eq_ignore_ascii_case("GET") {
        return Err(HandshakeError::MethodNotGet);
    }

    let upgrade = upgrade.ok_or(HandshakeError::MissingUpgrade)?;
    if !upgrade
        .split(',')
        .any(|token| token.trim().eq_ignore_ascii_case("websocket"))
    {
        return Err(HandshakeError::MissingUpgrade);
    }

    let connection = connection.ok_or(HandshakeError::MissingConnection)?;
    if !connection
        .split(',')
        .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
    {
        return Err(HandshakeError::MissingConnection);
    }

    let version = version.ok_or(HandshakeError::MissingVersion)?;
    if version.trim() != "13" {
        return Err(HandshakeError::UnsupportedVersion);
    }

    // HTTP OWS is SP/HTAB only. `str::trim` would also silently accept embedded CR/LF,
    // which the header parser must never produce but which has no business being valid here.
    let key = key
        .ok_or(HandshakeError::MissingKey)?
        .trim_matches([' ', '\t']);
    let decoded = base64_decode(key).ok_or(HandshakeError::InvalidKey)?;
    if decoded.len() != 16 {
        return Err(HandshakeError::InvalidKey);
    }

    Ok(accept_for_key(key))
}

/// Computes `Sec-WebSocket-Accept` for a client key, without validating it.
pub fn accept_for_key(key: &str) -> String {
    let mut input = Vec::with_capacity(key.len() + WS_GUID.len());
    input.extend_from_slice(key.as_bytes());
    input.extend_from_slice(WS_GUID.as_bytes());
    let digest = digest(&SHA1_FOR_LEGACY_USE_ONLY, &input);
    base64_encode(digest.as_ref())
}

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        out.push(BASE64_ALPHABET[(b0 >> 2) as usize] as char);
        out.push(BASE64_ALPHABET[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(BASE64_ALPHABET[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(BASE64_ALPHABET[(b2 & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Strict, canonical base64 decoding (standard alphabet, required padding, zero trailing
/// bits). Rejecting non-canonical keys prevents two spellings of the same key from producing
/// different accepts.
pub(crate) fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let bytes = input.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let chunks = bytes.len() / 4;
    for (index, chunk) in bytes.chunks(4).enumerate() {
        let is_last = index + 1 == chunks;
        let mut values = [0u8; 4];
        let mut padding = 0usize;
        for (position, &byte) in chunk.iter().enumerate() {
            if byte == b'=' {
                if !is_last || position < 2 {
                    return None;
                }
                padding += 1;
            } else {
                if padding > 0 {
                    return None;
                }
                values[position] = decode_base64_byte(byte)?;
            }
        }
        if padding == 2 && values[1] & 0x0f != 0 {
            return None;
        }
        if padding == 1 && values[2] & 0x03 != 0 {
            return None;
        }
        out.push(((values[0] & 0x3f) << 2) | (values[1] >> 4));
        if padding < 2 {
            out.push(((values[1] & 0x0f) << 4) | (values[2] >> 2));
        }
        if padding < 1 {
            out.push(((values[2] & 0x03) << 6) | values[3]);
        }
    }
    Some(out)
}

fn decode_base64_byte(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc6455_example_accept_key() {
        // RFC 6455 §1.3.
        assert_eq!(
            accept_for_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn accepts_valid_handshakes() {
        let accept = validate_handshake(
            "GET",
            Some("websocket"),
            Some("keep-alive, Upgrade"),
            Some("13"),
            Some("dGhlIHNhbXBsZSBub25jZQ=="),
        )
        .unwrap();
        assert_eq!(accept, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");

        // Header token casing must not matter.
        validate_handshake(
            "get",
            Some("WebSocket"),
            Some("UPGRADE"),
            Some("13"),
            Some("dGhlIHNhbXBsZSBub25jZQ=="),
        )
        .unwrap();
    }

    #[test]
    fn rejects_missing_or_invalid_fields() {
        let valid = "dGhlIHNhbXBsZSBub25jZQ==";
        assert_eq!(
            validate_handshake("POST", Some("websocket"), Some("Upgrade"), Some("13"), Some(valid)),
            Err(HandshakeError::MethodNotGet)
        );
        assert_eq!(
            validate_handshake("GET", None, Some("Upgrade"), Some("13"), Some(valid)),
            Err(HandshakeError::MissingUpgrade)
        );
        assert_eq!(
            validate_handshake("GET", Some("h2c"), Some("Upgrade"), Some("13"), Some(valid)),
            Err(HandshakeError::MissingUpgrade)
        );
        assert_eq!(
            validate_handshake("GET", Some("websocket"), Some("keep-alive"), Some("13"), Some(valid)),
            Err(HandshakeError::MissingConnection)
        );
        assert_eq!(
            validate_handshake("GET", Some("websocket"), Some("Upgrade"), None, Some(valid)),
            Err(HandshakeError::MissingVersion)
        );
        assert_eq!(
            validate_handshake("GET", Some("websocket"), Some("Upgrade"), Some("8"), Some(valid)),
            Err(HandshakeError::UnsupportedVersion)
        );
        assert_eq!(
            validate_handshake("GET", Some("websocket"), Some("Upgrade"), Some("13"), None),
            Err(HandshakeError::MissingKey)
        );
        for key in [
            "not base64!!",
            "AAAA",
            "dGhlIHNhbXBsZSBub25jZQ",
            "dGhlIHNhbXBsZSBub25jZR==",
            "dGhlIHNhbXBsZSBub25jZQ==x",
            "dGhlIHNhbXBsZSBub25jZQ==\r\n",
        ] {
            assert_eq!(
                validate_handshake("GET", Some("websocket"), Some("Upgrade"), Some("13"), Some(key)),
                Err(HandshakeError::InvalidKey),
                "key {key:?} must be rejected"
            );
        }
    }

    #[test]
    fn base64_round_trip_and_canonical_rules() {
        for input in [b"".as_slice(), b"f", b"fo", b"foo", b"foobar", &[0u8; 32]] {
            let encoded = base64_encode(input);
            assert_eq!(base64_decode(&encoded).as_deref(), Some(input));
        }
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert_eq!(base64_encode(b"M"), "TQ==");
        assert_eq!(base64_decode("TQ==").unwrap(), b"M");
        assert_eq!(base64_decode("TR=="), None, "non-zero trailing bits");
        assert_eq!(base64_decode("T==="), None);
        assert_eq!(base64_decode("TQ="), None);
        assert_eq!(base64_decode("TQ==="), None);
    }
}
