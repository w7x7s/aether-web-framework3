//! # URL Percent-Decoding
//!
//! RFC 3986 §2.1 percent-decoding plus the `+`-as-space rule that belongs to
//! `application/x-www-form-urlencoded`. Decoding operates on bytes and then validates UTF-8,
//! so multi-byte sequences such as Arabic or emoji survive intact.

use std::borrow::Cow;

#[inline]
fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Percent-decodes a URI component, borrowing when there is nothing to decode.
///
/// Invalid escapes (`%zz`, a trailing `%4`) are left verbatim rather than rejected: they are
/// already inert, and dropping the request at this layer would turn a cosmetic encoding bug
/// into an outage. Invalid UTF-8 produced by hostile input is replaced with U+FFFD so callers
/// always receive a usable `str`.
pub fn percent_decode_cow(input: &str) -> Cow<'_, str> {
    let bytes = input.as_bytes();
    if !bytes.contains(&b'%') {
        return Cow::Borrowed(input);
    }

    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }

    match String::from_utf8(out) {
        Ok(s) => Cow::Owned(s),
        Err(e) => Cow::Owned(String::from_utf8_lossy(e.as_bytes()).into_owned()),
    }
}

/// Percent-decodes a URI component into an owned `String`.
pub fn percent_decode(input: &str) -> String {
    match percent_decode_cow(input) {
        Cow::Borrowed(s) => s.to_string(),
        Cow::Owned(s) => s,
    }
}

/// Decodes `application/x-www-form-urlencoded` data: percent-escapes plus `+` for space.
///
/// The `+` substitution happens before percent-decoding so that a literal `%2B` still
/// decodes to `+` rather than being turned into a space.
pub fn form_decode(input: &str) -> String {
    let plus_decoded: Cow<'_, str> = if input.as_bytes().contains(&b'+') {
        Cow::Owned(input.replace('+', " "))
    } else {
        Cow::Borrowed(input)
    };
    match percent_decode_cow(plus_decoded.as_ref()) {
        Cow::Borrowed(s) => s.to_string(),
        Cow::Owned(s) => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_escapes_borrows() {
        assert!(matches!(percent_decode_cow("/users/42"), Cow::Borrowed(_)));
        assert_eq!(percent_decode("/users/42"), "/users/42");
    }

    #[test]
    fn test_ascii_escapes() {
        assert_eq!(percent_decode("hello%20world"), "hello world");
        assert_eq!(percent_decode("%2Fetc%2Fpasswd"), "/etc/passwd");
        assert_eq!(percent_decode("%41%42%43"), "ABC");
        assert_eq!(percent_decode("a%2Bb"), "a+b");
    }

    /// The reason this module exists: byte-wise decoding then UTF-8 validation, so
    /// multi-byte text is not mangled into mojibake.
    #[test]
    fn test_utf8_round_trip() {
        let arabic = "مرحبا";
        let encoded: String = arabic
            .bytes()
            .map(|b| format!("%{:02X}", b))
            .collect();
        assert_eq!(percent_decode(&encoded), arabic);

        let emoji = "😀 café";
        let encoded: String = emoji
            .bytes()
            .map(|b| format!("%{:02X}", b))
            .collect();
        assert_eq!(percent_decode(&encoded), emoji);

        // Raw UTF-8 with a single escape mixed in.
        assert_eq!(percent_decode("مرحبا%20عالم"), "مرحبا عالم");
    }

    /// Regression: the previous implementation pushed each decoded byte as `char`, so
    /// `%D9%85` became "Ù..." instead of "م".
    #[test]
    fn test_arabic_is_not_mojibake() {
        let decoded = percent_decode("%D9%85%D8%B1%D8%AD%D8%A8%D8%A7");
        assert_eq!(decoded, "مرحبا");
        assert_eq!(decoded.chars().count(), 5);
        assert!(!decoded.contains('Ù'));
    }

    #[test]
    fn test_plus_is_literal_for_path_decoding() {
        // `+` only means space in form encoding, not in a URI path segment.
        assert_eq!(percent_decode("a+b"), "a+b");
        assert_eq!(form_decode("a+b"), "a b");
        // A literal plus survives form decoding.
        assert_eq!(form_decode("a%2Bb"), "a+b");
        // Space via percent-escape also works in form data.
        assert_eq!(form_decode("a%20b"), "a b");
    }

    #[test]
    fn test_invalid_escapes_are_left_verbatim() {
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("%4"), "%4");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%%41"), "%A");
        assert_eq!(percent_decode("%G1"), "%G1");
    }

    #[test]
    fn test_lowercase_hex_accepted() {
        assert_eq!(percent_decode("%2f"), "/");
        assert_eq!(percent_decode("%d9%85"), "م");
    }

    /// An escape that produces invalid UTF-8 must not panic.
    #[test]
    fn test_invalid_utf8_yields_replacement_not_panic() {
        let decoded = percent_decode("%FF%FE");
        assert!(!decoded.is_empty());
        assert!(decoded.contains('\u{FFFD}'));
        // Mixed valid and invalid bytes stay usable.
        let decoded = percent_decode("ok%FFdone");
        assert!(decoded.starts_with("ok"));
        assert!(decoded.ends_with("done"));
    }

    /// Path traversal via encoded dots must decode predictably; the *caller* is responsible
    /// for rejecting the result, but it must at least see the real bytes.
    #[test]
    fn test_encoded_traversal_decodes_for_caller_inspection() {
        assert_eq!(percent_decode("%2e%2e%2f"), "../");
        assert_eq!(percent_decode("..%2F..%2Fetc%2Fpasswd"), "../../etc/passwd");
    }

    #[test]
    fn test_null_bytes_are_preserved_for_caller_to_reject() {
        assert_eq!(percent_decode("%00"), "\0");
    }

    #[test]
    fn test_empty_and_edge_inputs() {
        assert_eq!(percent_decode(""), "");
        assert_eq!(form_decode(""), "");
        assert_eq!(percent_decode("%"), "%");
        assert_eq!(percent_decode("%%"), "%%");
        assert_eq!(percent_decode("a%"), "a%");
    }
}
