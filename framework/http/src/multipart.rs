//! # RFC 7578 `multipart/form-data` parser
//!
//! Parses a complete request body into borrowed parts. The body is already bounded by the
//! server's configured body limit, so parsing never allocates more than the number of parts
//! and header fields. Part payloads are slices of the request body (binary-safe, no copies).
//!
//! Security properties:
//! - boundary length and charset are validated before scanning;
//! - a `\r\n--boundary` sequence inside a payload is only treated as a delimiter when it is
//!   followed by `--` or transport padding plus CRLF, so binary payloads cannot desynchronise
//!   the parse;
//! - obsolete line folding, bare CR/LF in part headers, oversized part headers, and more than
//!   [`MAX_PARTS`] parts are rejected;
//! - filenames are returned as metadata only; this parser never touches the filesystem.

use std::fmt;

/// Maximum number of parts accepted in one form.
pub const MAX_PARTS: usize = 1024;
/// Maximum size of one part's header block, excluding the terminating CRLFCRLF.
pub const MAX_PART_HEADER_BYTES: usize = 16 * 1024;
/// Maximum number of header fields in one part.
pub const MAX_PART_HEADERS: usize = 64;
/// Maximum boundary length allowed by RFC 2046.
pub const MAX_BOUNDARY_LEN: usize = 70;

/// One part of a multipart form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part<'a> {
    /// `name` parameter of `Content-Disposition` (`None` when omitted).
    pub name: Option<String>,
    /// `filename` parameter of `Content-Disposition` (client-supplied, untrusted).
    pub filename: Option<String>,
    /// `Content-Type` of the part (`None` when omitted).
    pub content_type: Option<String>,
    /// Every header of the part, with original casing.
    pub headers: Vec<(String, String)>,
    /// Raw payload bytes, borrowed from the request body.
    pub data: &'a [u8],
}

impl Part<'_> {
    /// The payload interpreted as UTF-8.
    pub fn text(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(self.data)
    }

    /// First header matching `name` case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A parsed multipart form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultipartForm<'a> {
    parts: Vec<Part<'a>>,
}

impl<'a> MultipartForm<'a> {
    /// Parse `body` according to the supplied `Content-Type` field value.
    pub fn parse(content_type: &str, body: &'a [u8]) -> Result<Self, MultipartError> {
        let (media_type, params) =
            parse_value_with_parameters(content_type).map_err(|_| MultipartError::InvalidContentType)?;
        if !media_type.eq_ignore_ascii_case("multipart/form-data") {
            return Err(MultipartError::UnsupportedContentType);
        }
        let boundary = params
            .iter()
            .find(|(name, _)| name == "boundary")
            .map(|(_, value)| value.as_str())
            .ok_or(MultipartError::MissingBoundary)?;
        validate_boundary(boundary)?;
        Self::parse_with_boundary(boundary.as_bytes(), body)
    }

    /// Parse using an already extracted boundary (no `Content-Type` parsing).
    pub fn parse_with_boundary(boundary: &[u8], body: &'a [u8]) -> Result<Self, MultipartError> {
        if boundary.is_empty() || boundary.len() > MAX_BOUNDARY_LEN {
            return Err(MultipartError::InvalidBoundary);
        }
        if boundary
            .iter()
            .any(|&b| b == b'\r' || b == b'\n' || b == b'\0' || b > 0x7f)
        {
            return Err(MultipartError::InvalidBoundary);
        }

        let mut delimiter = Vec::with_capacity(boundary.len() + 2);
        delimiter.extend_from_slice(b"--");
        delimiter.extend_from_slice(boundary);

        let first = find_delimiter(body, &delimiter, 0).ok_or(MultipartError::MalformedBody)?;
        if first.closing {
            // A form with only a closing delimiter is empty but well formed.
            return Ok(Self { parts: Vec::new() });
        }

        let mut parts = Vec::new();
        let mut cursor = first.after_line;
        loop {
            if parts.len() == MAX_PARTS {
                return Err(MultipartError::TooManyParts);
            }

            let next = find_delimiter(body, &delimiter, cursor).ok_or(MultipartError::MalformedBody)?;
            // The CRLF immediately preceding the next delimiter belongs to that delimiter.
            parts.push(parse_part(&body[cursor..next.delimiter_start])?);

            if next.closing {
                // Epilogue after the closing delimiter is ignored (RFC 2046 §5.1.1).
                return Ok(Self { parts });
            }
            cursor = next.after_line;
        }
    }

    /// All parts in wire order.
    pub fn parts(&self) -> &[Part<'a>] {
        &self.parts
    }

    /// First part whose `name` matches exactly.
    pub fn get(&self, name: &str) -> Option<&Part<'a>> {
        self.parts.iter().find(|part| part.name.as_deref() == Some(name))
    }

    /// Every part with the given name (HTML forms may submit duplicate fields).
    pub fn get_all(&self, name: &str) -> Vec<&Part<'a>> {
        self.parts
            .iter()
            .filter(|part| part.name.as_deref() == Some(name))
            .collect()
    }

    /// Consume the form and return its parts.
    pub fn into_parts(self) -> Vec<Part<'a>> {
        self.parts
    }
}

/// Errors produced while parsing a multipart body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultipartError {
    UnsupportedContentType,
    InvalidContentType,
    MissingBoundary,
    InvalidBoundary,
    MalformedBody,
    TooManyParts,
    MissingPartHeaders,
    PartHeaderTooLarge,
    TooManyPartHeaders,
    InvalidPartHeader,
    MissingContentDisposition,
    InvalidContentDisposition,
}

impl fmt::Display for MultipartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::UnsupportedContentType => "request is not multipart/form-data",
            Self::InvalidContentType => "multipart Content-Type is malformed",
            Self::MissingBoundary => "multipart Content-Type is missing a boundary parameter",
            Self::InvalidBoundary => "multipart boundary is empty, too long, or contains forbidden bytes",
            Self::MalformedBody => "multipart body is truncated or delimiters are malformed",
            Self::TooManyParts => "multipart form contains too many parts",
            Self::MissingPartHeaders => "multipart part is missing its header block",
            Self::PartHeaderTooLarge => "multipart part header block exceeds the limit",
            Self::TooManyPartHeaders => "multipart part has too many header fields",
            Self::InvalidPartHeader => "multipart part contains an invalid header field",
            Self::MissingContentDisposition => "multipart part is missing Content-Disposition",
            Self::InvalidContentDisposition => "multipart part Content-Disposition is invalid",
        };
        f.write_str(message)
    }
}

impl std::error::Error for MultipartError {}

struct Delimiter {
    /// Byte index of the CRLF (or body start) preceding `--boundary`.
    delimiter_start: usize,
    /// Byte index just past the delimiter line's CRLF (or closing `--`).
    after_line: usize,
    closing: bool,
}

/// Finds the next valid boundary delimiter at or after `from`.
///
/// A candidate is only a delimiter when it starts a line and is followed by `--` or by
/// optional transport padding and CRLF. Embedded `\r\n--boundary` byte sequences in binary
/// payloads that do not satisfy that grammar are skipped.
fn find_delimiter(body: &[u8], delimiter: &[u8], from: usize) -> Option<Delimiter> {
    let mut search = from;
    while search <= body.len() {
        let relative = find_subslice(&body[search..], delimiter)?;
        let index = search + relative;

        let starts_line = index == 0 || (index >= 2 && &body[index - 2..index] == b"\r\n");
        if !starts_line {
            search = index + 1;
            continue;
        }
        let delimiter_start = if index >= 2 { index - 2 } else { 0 };

        let mut pos = index + delimiter.len();
        let closing = body.get(pos..pos + 2) == Some(b"--");
        if closing {
            pos += 2;
        } else {
            // RFC 2046 permits transport padding (spaces/tabs) between the delimiter and CRLF.
            while matches!(body.get(pos), Some(b' ' | b'\t')) {
                pos += 1;
            }
            if body.get(pos..pos + 2) != Some(b"\r\n") {
                search = index + 1;
                continue;
            }
            pos += 2;
        }

        return Some(Delimiter {
            delimiter_start,
            after_line: pos,
            closing,
        });
    }
    None
}

fn parse_part(bytes: &[u8]) -> Result<Part<'_>, MultipartError> {
    // Header block ends at the first CRLFCRLF. A part without one is malformed, not empty.
    let header_end = find_subslice(bytes, b"\r\n\r\n").ok_or(MultipartError::MissingPartHeaders)?;
    if header_end > MAX_PART_HEADER_BYTES {
        return Err(MultipartError::PartHeaderTooLarge);
    }
    let header_block = &bytes[..header_end];
    let data = &bytes[header_end + 4..];

    let mut headers = Vec::new();
    if !header_block.is_empty() {
        let lines: Vec<&[u8]> = header_block.split(|&b| b == b'\n').collect();
        for (index, raw) in lines.iter().enumerate() {
            // Every boundary between header lines must be a full CRLF; a bare LF would be
            // interpreted differently by a proxy that normalises line endings.
            let line = if index + 1 == lines.len() {
                raw.strip_suffix(b"\r").unwrap_or(raw)
            } else {
                raw.strip_suffix(b"\r")
                    .ok_or(MultipartError::InvalidPartHeader)?
            };
            if line.is_empty() {
                return Err(MultipartError::InvalidPartHeader);
            }
            // Obsolete folding and bare CR are refused: accepting them invites parser
            // differentials between Aether and an upstream proxy.
            if line.starts_with(b" ") || line.starts_with(b"\t") || line.contains(&b'\r') {
                return Err(MultipartError::InvalidPartHeader);
            }
            let colon = line
                .iter()
                .position(|&b| b == b':')
                .ok_or(MultipartError::InvalidPartHeader)?;
            let name = &line[..colon];
            if name.is_empty() || !name.iter().all(|&b| is_token_byte(b)) {
                return Err(MultipartError::InvalidPartHeader);
            }
            let value = trim_ows(&line[colon + 1..]);
            let name = std::str::from_utf8(name).map_err(|_| MultipartError::InvalidPartHeader)?;
            let value = std::str::from_utf8(value).map_err(|_| MultipartError::InvalidPartHeader)?;
            headers.push((name.to_string(), value.to_string()));
            if headers.len() > MAX_PART_HEADERS {
                return Err(MultipartError::TooManyPartHeaders);
            }
        }
    }

    let disposition = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-disposition"))
        .map(|(_, value)| value.as_str())
        .ok_or(MultipartError::MissingContentDisposition)?;
    let (disposition_type, params) =
        parse_value_with_parameters(disposition).map_err(|_| MultipartError::InvalidContentDisposition)?;
    if !disposition_type.eq_ignore_ascii_case("form-data") {
        return Err(MultipartError::InvalidContentDisposition);
    }

    let mut name = None;
    let mut filename = None;
    for (key, value) in params {
        match key.as_str() {
            "name" if name.is_none() && !value.is_empty() => name = Some(value),
            // `filename*` (RFC 5987) is deliberately not decoded into `filename`: silently
            // preferring it over `filename` is a documented source of filename confusion.
            "filename" if filename.is_none() => filename = Some(value),
            "name" | "filename" => return Err(MultipartError::InvalidContentDisposition),
            _ => {}
        }
    }

    let content_type = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.clone());

    Ok(Part {
        name,
        filename,
        content_type,
        headers,
        data,
    })
}

/// Parses `value; key=value; key="quoted value"` into the leading value and its parameters.
/// Parameter names are lowercased; values are returned with quotes and escapes resolved.
fn parse_value_with_parameters(input: &str) -> Result<(String, Vec<(String, String)>), MultipartError> {
    let bytes = input.as_bytes();
    let mut pos = 0usize;
    while pos < bytes.len() && bytes[pos] != b';' {
        pos += 1;
    }
    let value = input[..pos].trim();
    if value.is_empty() {
        return Err(MultipartError::InvalidContentDisposition);
    }

    let mut params = Vec::new();
    while pos < bytes.len() {
        // Skip ';' and optional whitespace.
        pos += 1;
        while pos < bytes.len() && (bytes[pos] == b' ' || bytes[pos] == b'\t') {
            pos += 1;
        }
        if pos >= bytes.len() {
            break;
        }
        let key_start = pos;
        while pos < bytes.len() && bytes[pos] != b'=' && bytes[pos] != b';' {
            pos += 1;
        }
        if bytes.get(pos) != Some(&b'=') {
            return Err(MultipartError::InvalidContentDisposition);
        }
        let key = input[key_start..pos].trim().to_ascii_lowercase();
        if key.is_empty() || !key.bytes().all(is_token_byte) {
            return Err(MultipartError::InvalidContentDisposition);
        }
        pos += 1;
        while pos < bytes.len() && (bytes[pos] == b' ' || bytes[pos] == b'\t') {
            pos += 1;
        }

        let value = if bytes.get(pos) == Some(&b'"') {
            pos += 1;
            let mut out = String::new();
            let mut closed = false;
            while pos < bytes.len() {
                match bytes[pos] {
                    b'\\' => {
                        pos += 1;
                        let escaped = *bytes.get(pos).ok_or(MultipartError::InvalidContentDisposition)?;
                        if escaped == b'\r' || escaped == b'\n' {
                            return Err(MultipartError::InvalidContentDisposition);
                        }
                        if escaped.is_ascii() {
                            out.push(escaped as char);
                            pos += 1;
                        } else {
                            // A multi-byte escape must copy the whole UTF-8
                            // sequence: pushing one byte left `pos` inside the
                            // sequence and the next `input[pos..]` panicked on a
                            // non-char-boundary slice (remote DoS via
                            // `filename="x\é"`).
                            let ch = input[pos..].chars().next().ok_or(MultipartError::InvalidContentDisposition)?;
                            out.push(ch);
                            pos += ch.len_utf8();
                        }
                    }
                    b'"' => {
                        pos += 1;
                        closed = true;
                        break;
                    }
                    b'\r' | b'\n' => return Err(MultipartError::InvalidContentDisposition),
                    byte if byte.is_ascii() => {
                        out.push(byte as char);
                        pos += 1;
                    }
                    _ => {
                        // The input is already valid UTF-8; copy one full character.
                        let ch = input[pos..].chars().next().ok_or(MultipartError::InvalidContentDisposition)?;
                        out.push(ch);
                        pos += ch.len_utf8();
                    }
                }
            }
            if !closed {
                return Err(MultipartError::InvalidContentDisposition);
            }
            while pos < bytes.len() && (bytes[pos] == b' ' || bytes[pos] == b'\t') {
                pos += 1;
            }
            if pos < bytes.len() && bytes[pos] != b';' {
                return Err(MultipartError::InvalidContentDisposition);
            }
            out
        } else {
            let value_start = pos;
            while pos < bytes.len() && bytes[pos] != b';' {
                if bytes[pos] == b'"' || bytes[pos] == b'\r' || bytes[pos] == b'\n' {
                    return Err(MultipartError::InvalidContentDisposition);
                }
                pos += 1;
            }
            input[value_start..pos].trim().to_string()
        };

        params.push((key, value));
    }

    Ok((value.to_string(), params))
}

fn validate_boundary(boundary: &str) -> Result<(), MultipartError> {
    if boundary.is_empty() || boundary.len() > MAX_BOUNDARY_LEN {
        return Err(MultipartError::InvalidBoundary);
    }
    if boundary
        .bytes()
        .any(|b| b > 0x7f || b == b'\r' || b == b'\n' || b == b'\0')
    {
        return Err(MultipartError::InvalidBoundary);
    }
    Ok(())
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    if needle.len() > haystack.len() {
        return None;
    }
    let first = needle[0];
    let last_start = haystack.len() - needle.len();
    let mut index = 0usize;
    while index <= last_start {
        if haystack[index] == first && &haystack[index..index + needle.len()] == needle {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn is_token_byte(byte: u8) -> bool {
    matches!(
        byte,
        b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_'
            | b'`' | b'|' | b'~'
    ) || byte.is_ascii_alphanumeric()
}

fn trim_ows(mut value: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = value {
        if *first == b' ' || *first == b'\t' {
            value = rest;
        } else {
            break;
        }
    }
    while let [rest @ .., last] = value {
        if *last == b' ' || *last == b'\t' {
            value = rest;
        } else {
            break;
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(boundary: &str, parts: &[(&str, &str)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, value) in parts {
            out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            out.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
            );
            out.extend_from_slice(value.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        out
    }

    #[test]
    fn parses_multiple_text_fields() {
        let body = form("XyZ", &[("first", "one"), ("second", "two")]);
        let parsed = MultipartForm::parse("multipart/form-data; boundary=XyZ", &body).unwrap();
        assert_eq!(parsed.parts().len(), 2);
        assert_eq!(parsed.get("first").unwrap().text().unwrap(), "one");
        assert_eq!(parsed.get("second").unwrap().text().unwrap(), "two");
        assert_eq!(parsed.get_all("first").len(), 1);
    }

    #[test]
    fn parses_file_metadata_and_binary_payload() {
        let body = b"--b\r\n\
Content-Disposition: form-data; name=\"upload\"; filename=\"report.bin\"\r\n\
Content-Type: application/octet-stream\r\n\
\r\n\
\x00\x01\xffbinary\r\n\
--b--\r\n";
        let parsed = MultipartForm::parse("multipart/form-data; boundary=\"b\"", body).unwrap();
        let part = parsed.get("upload").unwrap();
        assert_eq!(part.filename.as_deref(), Some("report.bin"));
        assert_eq!(part.content_type.as_deref(), Some("application/octet-stream"));
        assert_eq!(part.data, b"\x00\x01\xffbinary");
    }

    #[test]
    fn quoted_boundary_and_escaped_parameter() {
        let boundary = "a b'c";
        let body = b"--a b'c\r\n\
Content-Disposition: form-data; name=\"fi\\\"eld\"\r\n\r\n\
v\r\n\
--a b'c--";
        let parsed = MultipartForm::parse(
            "multipart/form-data; charset=utf-8; boundary=\"a b'c\"",
            body,
        )
        .unwrap();
        assert_eq!(parsed.get("fi\"eld").unwrap().data, b"v");
        assert!(boundary.contains(' '));
    }

    /// Regression: a `\` escape followed by a multi-byte UTF-8 character used to advance
    /// exactly one byte, leaving `pos` mid-sequence so the next `input[pos..]` slicing hit a
    /// non-char-boundary and panicked — a remote panic via any multipart upload filename.
    #[test]
    fn escaped_non_ascii_in_quoted_parameter_decodes_without_panic() {
        // `café` = \xc3\xa9; `😀` = \xf0\x9f\x98\x80, as raw UTF-8 bytes on the wire.
        let body = b"--b\r\nContent-Disposition: form-data; name=\"x\"; filename=\"caf\\\xc3\xa9.png\"\r\n\r\nv\r\n--b--";
        let parsed = MultipartForm::parse("multipart/form-data; boundary=b", body).unwrap();
        assert_eq!(parsed.get("x").unwrap().filename.as_deref(), Some("café.png"));

        // 4-byte sequences must round-trip too.
        let body4 = b"--b\r\nContent-Disposition: form-data; name=\"x\"; filename=\"a\\\xf0\x9f\x98\x80b\"\r\n\r\nv\r\n--b--";
        let parsed4 = MultipartForm::parse("multipart/form-data; boundary=b", body4).unwrap();
        assert_eq!(parsed4.get("x").unwrap().filename.as_deref(), Some("a😀b"));
    }

    #[test]
    fn rejects_escape_of_line_break_and_unterminated_quote() {
        // The header line is split at its CRLF first, so the value ends with a lone
        // backslash: the escape has nothing to consume and must error, not panic.
        let esc_crlf = b"--b\r\nContent-Disposition: form-data; name=\"x\"; filename=\"a\\\r\n\r\nv\r\n--b--";
        assert_eq!(
            MultipartForm::parse("multipart/form-data; boundary=b", esc_crlf).unwrap_err(),
            MultipartError::InvalidContentDisposition
        );
        // Backslash at end of input escapes the closing quote, leaving the string unclosed.
        // The request head is never terminated, so this fails during framing rather than
        // disposition parsing; either way it must be a clean error, not a panic.
        let trunc = b"--b\r\nContent-Disposition: form-data; name=\"x\"; filename=\"a\\";
        assert!(MultipartForm::parse("multipart/form-data; boundary=b", trunc).is_err());
    }

    #[test]
    fn preamble_and_epilogue_are_ignored() {
        let body = b"preamble text\r\n--b\r\n\
Content-Disposition: form-data; name=\"x\"\r\n\r\n\
1\r\n--b--\r\nepilogue ignored";
        let parsed = MultipartForm::parse("multipart/form-data; boundary=b", body).unwrap();
        assert_eq!(parsed.parts().len(), 1);
        assert_eq!(parsed.get("x").unwrap().data, b"1");
    }

    #[test]
    fn embedded_boundary_lookalike_is_data() {
        // `\r\n--bX` does not satisfy the delimiter grammar and must stay in the payload.
        let body = b"--b\r\n\
Content-Disposition: form-data; name=\"x\"\r\n\r\n\
before\r\n--bXafter\r\n\
--b--\r\n";
        let parsed = MultipartForm::parse("multipart/form-data; boundary=b", body).unwrap();
        assert_eq!(parsed.get("x").unwrap().data, b"before\r\n--bXafter");
    }

    #[test]
    fn preserves_single_trailing_crlf_in_data() {
        let body = b"--b\r\n\
Content-Disposition: form-data; name=\"x\"\r\n\r\n\
line\r\n\r\n\
--b--\r\n";
        let parsed = MultipartForm::parse("multipart/form-data; boundary=b", body).unwrap();
        assert_eq!(parsed.get("x").unwrap().data, b"line\r\n");
    }

    #[test]
    fn rejects_malformed_inputs() {
        let cases: &[(&str, &[u8], MultipartError)] = &[
            (
                "multipart/form-data",
                b"--b\r\n\r\nx\r\n--b--",
                MultipartError::MissingBoundary,
            ),
            (
                "multipart/form-data; boundary=",
                b"",
                MultipartError::InvalidBoundary,
            ),
            (
                "multipart/form-data; boundary=b",
                b"--b\r\nContent-Disposition: form-data; name=\"x\"\r\n\r\nunterminated",
                MultipartError::MalformedBody,
            ),
            (
                "multipart/form-data; boundary=b",
                b"--b\r\n\r\nx\r\n--b--",
                MultipartError::MissingPartHeaders,
            ),
            (
                "multipart/form-data; boundary=b",
                b"--b\r\nContent-Disposition: attachment; name=\"x\"\r\n\r\nx\r\n--b--",
                MultipartError::InvalidContentDisposition,
            ),
            (
                "multipart/form-data; boundary=b",
                b"--b\r\nContent-Disposition: form-data; name=\"x\"\r\n\r\nx",
                MultipartError::MalformedBody,
            ),
            (
                "multipart/form-data; boundary=b",
                b"--b\r\nContent-Disposition: form-data; name=\"x\"\r\n\r\nx\r\n--b-",
                MultipartError::MalformedBody,
            ),
        ];
        for (content_type, body, expected) in cases {
            assert_eq!(
                MultipartForm::parse(content_type, body).unwrap_err(),
                *expected,
                "content-type={content_type} body={body:?}"
            );
        }
    }

    #[test]
    fn rejects_oversized_boundary_and_non_form_type() {
        let long = "a".repeat(MAX_BOUNDARY_LEN + 1);
        assert_eq!(
            MultipartForm::parse(&format!("multipart/form-data; boundary={long}"), b"").unwrap_err(),
            MultipartError::InvalidBoundary
        );
        assert_eq!(
            MultipartForm::parse("application/json", b"{}").unwrap_err(),
            MultipartError::UnsupportedContentType
        );
    }

    #[test]
    fn rejects_folded_and_invalid_part_headers() {
        let folded = b"--b\r\nContent-Disposition: form-data; name=\"x\"\r\nX: a\r\n b\r\n\r\nv\r\n--b--";
        assert_eq!(
            MultipartForm::parse("multipart/form-data; boundary=b", folded).unwrap_err(),
            MultipartError::InvalidPartHeader
        );

        let bad_name = b"--b\r\nContent-Disposition: form-data; name=\"x\"\r\nBad Name: v\r\n\r\nv\r\n--b--";
        assert_eq!(
            MultipartForm::parse("multipart/form-data; boundary=b", bad_name).unwrap_err(),
            MultipartError::InvalidPartHeader
        );

        // Bare LF between part headers must not be normalised into a header boundary.
        let bare_lf = b"--b\r\nContent-Disposition: form-data; name=\"x\"\nX: y\r\n\r\nv\r\n--b--";
        assert_eq!(
            MultipartForm::parse("multipart/form-data; boundary=b", bare_lf).unwrap_err(),
            MultipartError::InvalidPartHeader
        );
    }

    #[test]
    fn enforces_part_count_limit() {
        let mut body = Vec::new();
        for _ in 0..=MAX_PARTS {
            body.extend_from_slice(b"--b\r\nContent-Disposition: form-data; name=\"x\"\r\n\r\nv\r\n");
        }
        body.extend_from_slice(b"--b--\r\n");
        assert_eq!(
            MultipartForm::parse("multipart/form-data; boundary=b", &body).unwrap_err(),
            MultipartError::TooManyParts
        );
    }

    #[test]
    fn content_type_is_case_insensitive_and_disposition_parameters_are_typed() {
        let body = b"--b\r\nContent-Disposition: form-data; name=\"x\"; filename=\"a.txt\"\r\nContent-Type: text/plain\r\n\r\nv\r\n--b--";
        let parsed = MultipartForm::parse("Multipart/Form-Data; Boundary=b", body).unwrap();
        let part = parsed.get("x").unwrap();
        assert_eq!(part.filename.as_deref(), Some("a.txt"));
        assert_eq!(part.content_type.as_deref(), Some("text/plain"));
        assert_eq!(part.header("content-type"), Some("text/plain"));
    }
}
