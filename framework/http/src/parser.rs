//! # Ultra-fast Zero-Copy HTTP Request Parser
//!
//! Borrows all slices (&'req str, &'req [u8]) directly from the socket buffer.
//! Zero string copies, zero heap allocations for standard requests.
//! Hardened with RFC 9112 HTTP/1.1 Request Smuggling (CL.TE, TE.CL, Dual-CL) protection.

use crate::header::{HeaderMap, HeaderView};
use crate::limits::ServerLimits;
use crate::method::Method;
use aether_core::error::AetherError;

/// تمثيل مباشر وسريع لطلب HTTP يستعير شرائح الذاكرة (`&'req str`, `&'req [u8]`) مباشرة من بافر المقبس بدون أي نسخ (Zero-Copy).
#[derive(Debug)]
pub struct RequestView<'req> {
    pub method: Method,
    pub raw_uri: &'req str,
    pub path: &'req str,
    pub query_str: Option<&'req str>,
    pub version: u8,
    /// Minor version from the request line (`HTTP/1.0` -> 0, `HTTP/1.1` -> 1). Defaults
    /// to 1 for any other shape so legacy keep-alive semantics stay unchanged; the
    /// connection loop uses this because HTTP/1.0 closes by default while 1.1 persists.
    pub version_minor: u8,
    pub headers: HeaderMap<'req>,
    pub body: &'req [u8],
}

/// Number of header slots kept on the stack. Requests within this budget never touch the
/// heap; larger configured limits fall back to a one-shot `Vec`.
const STACK_HEADER_SLOTS: usize = 64;

/// RFC 9112 OWS is SP/HTAB only. `str::trim` also strips Unicode whitespace such as
/// U+00A0, and a framing value padded that way would be re-read differently by a
/// strict frontend — a CL.TE/TE.CL smuggling seam. Framing decisions must use this.
fn ows_trim(s: &str) -> &str {
    s.trim_matches(|c: char| c == ' ' || c == '\t')
}

impl<'req> RequestView<'req> {
    /// Parses a request whose body is framed by `Content-Length` only.
    ///
    /// `Transfer-Encoding` is deliberately rejected here; use [`parse_request`] for the
    /// runtime path that also supports chunked decoding.
    pub fn parse(buf: &'req [u8], limits: &ServerLimits) -> Result<Option<(Self, usize)>, AetherError> {
        let Some((head, header_len)) = Self::parse_head(buf, limits, false)? else {
            return Ok(None);
        };
        let cl = head.content_length.unwrap_or(0);
        let total_needed = header_len
            .checked_add(cl)
            .ok_or(AetherError::BodyLimitExceeded)?;
        if buf.len() < total_needed {
            return Ok(None);
        }
        Ok(Some((
            Self::from_head(head, &buf[header_len..total_needed]),
            total_needed,
        )))
    }

    fn from_head(head: RequestHead<'req>, body: &'req [u8]) -> Self {
        Self {
            method: head.method,
            raw_uri: head.raw_uri,
            path: head.path,
            query_str: head.query_str,
            version: head.version,
            version_minor: head.version_minor,
            headers: head.headers,
            body,
        }
    }

    /// Parses and validates the request head, applying the framing rules that protect
    /// against request smuggling. `allow_transfer_encoding` is false for the legacy
    /// `Content-Length`-only entry point.
    fn parse_head(
        buf: &'req [u8],
        limits: &ServerLimits,
        allow_transfer_encoding: bool,
    ) -> Result<Option<(RequestHead<'req>, usize)>, AetherError> {
        let header_budget = limits.max_headers_count.max(1);

        // The slot array must be sized from the configured limit, otherwise
        // `max_headers_count` is unenforceable and oversized header sections surface as a
        // generic 400 instead of the RFC-correct 431.
        let mut stack_headers = [httparse::EMPTY_HEADER; STACK_HEADER_SLOTS];
        let mut heap_headers: Vec<httparse::Header<'_>> = Vec::new();
        if header_budget > STACK_HEADER_SLOTS {
            heap_headers.resize(header_budget, httparse::EMPTY_HEADER);
        }
        let slots: &mut [httparse::Header<'_>] = if header_budget > STACK_HEADER_SLOTS {
            &mut heap_headers
        } else {
            &mut stack_headers[..header_budget]
        };

        let mut req = httparse::Request::new(slots);

        let status = match req.parse(buf) {
            Ok(status) => status,
            // httparse reports an exhausted slot array distinctly; map it to the limit error
            // so the server answers 431 rather than 400.
            Err(httparse::Error::TooManyHeaders) => return Err(AetherError::HeaderTooLarge),
            Err(_) => return Err(AetherError::MalformedRequest),
        };

        match status {
            httparse::Status::Complete(header_len) => {
                if header_len > limits.max_header_bytes {
                    return Err(AetherError::HeaderTooLarge);
                }

                let method_str = req.method.ok_or(AetherError::MalformedRequest)?;
                let method = Method::from_bytes(method_str.as_bytes())
                    .ok_or(AetherError::MalformedRequest)?;

                let raw_uri = req.path.ok_or(AetherError::MalformedRequest)?;
                let version = req.version.unwrap_or(1);
                let version_minor = request_line_version_minor(&buf[..header_len]);

                // فصل المسار عن معلمات الاستعلام بدون تخصيص: /users?page=1 -> path="/users", query="page=1"
                let (path, query_str) = if let Some(idx) = raw_uri.find('?') {
                    (&raw_uri[..idx], Some(&raw_uri[idx + 1..]))
                } else {
                    (raw_uri, None)
                };

                let mut headers = HeaderMap::new();
                let mut content_length: Option<usize> = None;
                let mut te_count: usize = 0;
                let mut te_chunked: usize = 0;
                let mut te_other = false;

                for h in req.headers.iter() {
                    headers.push(HeaderView {
                        name: h.name,
                        value: h.value,
                    });

                    if h.name.eq_ignore_ascii_case("content-length") {
                        if content_length.is_some() {
                            // الحماية من هجمات Request Smuggling: تم رصد تكرار Content-Length
                            return Err(AetherError::MalformedRequest);
                        }

                        let val_str = std::str::from_utf8(h.value)
                            .map_err(|_| AetherError::MalformedRequest)?;
                        let trimmed = ows_trim(val_str);
                        if trimmed.is_empty() || !trimmed.bytes().all(|b| b.is_ascii_digit()) {
                            return Err(AetherError::MalformedRequest);
                        }
                        let parsed_len = trimmed.parse::<usize>()
                            .map_err(|_| AetherError::MalformedRequest)?;
                        content_length = Some(parsed_len);
                    } else if h.name.eq_ignore_ascii_case("transfer-encoding") {
                        te_count += 1;
                        // Only an exact `chunked` coding is supported. Anything else
                        // (`gzip`, `identity`, lists) cannot be framed and must be refused.
                        let value = std::str::from_utf8(h.value)
                            .map_err(|_| AetherError::MalformedRequest)?;
                        if ows_trim(value).eq_ignore_ascii_case("chunked") {
                            te_chunked += 1;
                        } else {
                            te_other = true;
                        }
                    }
                }

                // RFC 9112 بند 6.3: حظر الجمع بين Transfer-Encoding و Content-Length (منع ثغرات CL.TE و TE.CL)
                if headers.len() > limits.max_headers_count {
                    return Err(AetherError::HeaderTooLarge);
                }
                if te_count > 0 && content_length.is_some() {
                    return Err(AetherError::MalformedRequest);
                }
                if te_count > 0 && !allow_transfer_encoding {
                    return Err(AetherError::MalformedRequest);
                }
                if let Some(cl) = content_length {
                    if cl > limits.max_body_bytes {
                        return Err(AetherError::BodyLimitExceeded);
                    }
                }

                Ok(Some((
                    RequestHead {
                        method,
                        raw_uri,
                        path,
                        query_str,
                        version,
                        version_minor,
                        headers,
                        content_length,
                        te_count,
                        te_chunked,
                        te_other,
                    },
                    header_len,
                )))
            }
            httparse::Status::Partial => {
                if buf.len() > limits.max_header_bytes {
                    return Err(AetherError::HeaderTooLarge);
                }
                Ok(None)
            }
        }
    }
}

/// Parsed head plus the framing metadata needed to decide how to read the body.
struct RequestHead<'req> {
    method: Method,
    raw_uri: &'req str,
    path: &'req str,
    query_str: Option<&'req str>,
    version: u8,
    version_minor: u8,
    headers: HeaderMap<'req>,
    content_length: Option<usize>,
    te_count: usize,
    te_chunked: usize,
    te_other: bool,
}

/// Parses a request, decoding a `Transfer-Encoding: chunked` body into an owned buffer.
///
/// Returns the request view, an owned body when the request was chunked (`None` means the
/// body is borrowed from `buf` and available as `view.body`), and the exact number of
/// consumed bytes so pipelined requests stay framed.
pub fn parse_request<'req>(
    buf: &'req [u8],
    limits: &ServerLimits,
) -> Result<Option<(RequestView<'req>, Option<Vec<u8>>, usize)>, AetherError> {
    let Some((head, header_len)) = RequestView::parse_head(buf, limits, true)? else {
        return Ok(None);
    };

    // RFC 9112 §3.2: every HTTP/1.1 request must carry exactly one Host field. A missing
    // Host leaves virtual-host routing undefined; a duplicated one is a smuggling vector,
    // because an upstream that keeps the first value and a downstream that keeps the last
    // disagree on the target. HTTP/1.0 requests predate the rule and stay accepted.
    if head.version == 1 && head.version_minor >= 1 {
        let host_values: Vec<&str> = head
            .headers
            .iter()
            .filter(|h| h.name.eq_ignore_ascii_case("host"))
            .map(|h| std::str::from_utf8(h.value).unwrap_or(""))
            .collect();
        // Exactly one, and non-empty after OWS: `Host:` with a bare value leaves
        // origin-form routing undefined and is refused by strict frontends, so
        // accepting it here would re-open a CL.TE-style disagreement.
        if host_values.len() != 1 || ows_trim(host_values[0]).is_empty() {
            return Err(AetherError::MalformedRequest);
        }
    }

    if head.te_count == 0 {
        let cl = head.content_length.unwrap_or(0);
        let total = header_len
            .checked_add(cl)
            .ok_or(AetherError::BodyLimitExceeded)?;
        if buf.len() < total {
            return Ok(None);
        }
        return Ok(Some((
            RequestView::from_head(head, &buf[header_len..total]),
            None,
            total,
        )));
    }

    // RFC 9112 §6.1: `chunked` must be the sole coding and only HTTP/1.1 supports it.
    // Duplicate Transfer-Encoding fields are refused rather than merged.
    if head.version != 1
        || head.version_minor < 1
        || head.te_count != 1
        || head.te_chunked != 1
        || head.te_other
    {
        return Err(AetherError::MalformedRequest);
    }

    let Some((decoded, chunk_bytes)) = decode_chunked(&buf[header_len..], limits)? else {
        return Ok(None);
    };

    Ok(Some((
        RequestView::from_head(head, &[]),
        Some(decoded),
        header_len + chunk_bytes,
    )))
}

/// Maximum bytes permitted for a chunk-size line (including extensions) or a trailer line.
const MAX_CHUNK_LINE: usize = 1024;

/// Extracts the HTTP minor version from the request line (`HTTP/1.0` -> 0). httparse only
/// reports the major version, so the raw first line is scanned; anything else defaults to 1,
/// preserving the pre-1.1 semantics the server already implements.
///
/// The scan is restricted to the FIRST line, terminated by LF (RFC 9110 §3.1.1 tolerates
/// bare-LF request lines, which httparse accepts). Splitting on `\r` instead would make a
/// LF-only request line swallow the entire header block, letting a later header value that
/// contains `HTTP/1.` — e.g. a Referer URL — decide the version and flip keep-alive
/// semantics. Within the line the LAST `HTTP/1.` token wins, because a URI could contain
/// an earlier look-alike.
fn request_line_version_minor(head_bytes: &[u8]) -> u8 {
    let line = head_bytes
        .split(|b| *b == b'\n')
        .next()
        .unwrap_or(head_bytes);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    line.windows(b"HTTP/1.".len())
        .rposition(|w| w == b"HTTP/1.")
        .and_then(|i| line.get(i + b"HTTP/1.".len()))
        .filter(|c| c.is_ascii_digit())
        .map_or(1, |c| c - b'0')
}

fn find_crlf(data: &[u8]) -> Option<usize> {
    data.windows(2).position(|w| w == b"\r\n")
}

fn is_token_char(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_'
                | b'`' | b'|' | b'~'
        )
}

/// Decodes `chunk-size CRLF chunk-data CRLF ... 0 CRLF trailers CRLF` into a contiguous
/// buffer. Returns `Ok(None)` while the body is incomplete, and the exact consumed byte
/// count once the terminating chunk and trailers have arrived.
fn decode_chunked(
    data: &[u8],
    limits: &ServerLimits,
) -> Result<Option<(Vec<u8>, usize)>, AetherError> {
    let mut pos = 0usize;
    let mut body: Vec<u8> = Vec::new();

    loop {
        let line_end = match find_crlf(&data[pos..]) {
            Some(rel) => pos + rel,
            None => {
                if data.len() - pos > MAX_CHUNK_LINE {
                    return Err(AetherError::MalformedRequest);
                }
                return Ok(None);
            }
        };
        let line = &data[pos..line_end];
        if line.len() > MAX_CHUNK_LINE {
            return Err(AetherError::MalformedRequest);
        }

        let size_part = line.split(|&b| b == b';').next().unwrap_or(b"");
        let size_part = size_part.trim_ascii();
        if size_part.is_empty()
            || size_part.len() > 16
            || !size_part.iter().all(|b| b.is_ascii_hexdigit())
        {
            return Err(AetherError::MalformedRequest);
        }
        let size_str = std::str::from_utf8(size_part)
            .map_err(|_| AetherError::MalformedRequest)?;
        let size = u64::from_str_radix(size_str, 16)
            .map_err(|_| AetherError::MalformedRequest)?;
        if size > limits.max_body_bytes as u64 {
            return Err(AetherError::BodyLimitExceeded);
        }
        let size = size as usize;
        pos = line_end + 2;

        if size == 0 {
            // Last-chunk: consume optional trailer fields up to the terminating empty line.
            let mut trailer_bytes = 0usize;
            let mut trailer_count = 0usize;
            loop {
                let end = match find_crlf(&data[pos..]) {
                    Some(rel) => pos + rel,
                    None => {
                        if data.len() - pos > MAX_CHUNK_LINE {
                            return Err(AetherError::MalformedRequest);
                        }
                        return Ok(None);
                    }
                };
                let trailer = &data[pos..end];
                if trailer.len() > MAX_CHUNK_LINE {
                    return Err(AetherError::MalformedRequest);
                }
                if trailer.is_empty() {
                    return Ok(Some((body, end + 2)));
                }
                trailer_bytes += trailer.len() + 2;
                trailer_count += 1;
                if trailer_bytes > limits.max_header_bytes || trailer_count > limits.max_headers_count
                {
                    return Err(AetherError::HeaderTooLarge);
                }
                // Trailer fields must be well-formed `token ":" OWS value`; they are not
                // merged into the request headers, so they cannot influence routing.
                let Some(colon) = trailer.iter().position(|&b| b == b':') else {
                    return Err(AetherError::MalformedRequest);
                };
                if colon == 0 || !trailer[..colon].iter().all(|&b| is_token_char(b)) {
                    return Err(AetherError::MalformedRequest);
                }
                // RFC 9112 §6.5: a trailer must not carry a field that affects message
                // framing or routing. The head has already been validated without these
                // values, so honouring them here would let a client smuggle a second
                // Content-Length/Transfer-Encoding/Host past the parser.
                let name = &trailer[..colon];
                if name.eq_ignore_ascii_case(b"content-length")
                    || name.eq_ignore_ascii_case(b"transfer-encoding")
                    || name.eq_ignore_ascii_case(b"host")
                {
                    return Err(AetherError::MalformedRequest);
                }
                pos = end + 2;
            }
        }

        let data_end = pos
            .checked_add(size)
            .ok_or(AetherError::BodyLimitExceeded)?;
        let frame_end = data_end
            .checked_add(2)
            .ok_or(AetherError::BodyLimitExceeded)?;
        if data.len() < frame_end {
            return Ok(None);
        }
        if &data[data_end..frame_end] != b"\r\n" {
            return Err(AetherError::MalformedRequest);
        }
        if body.len().checked_add(size).ok_or(AetherError::BodyLimitExceeded)? > limits.max_body_bytes {
            return Err(AetherError::BodyLimitExceeded);
        }
        body.extend_from_slice(&data[pos..data_end]);
        pos = frame_end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_smuggling_dual_content_length_rejected() {
        let limits = ServerLimits::default();
        let raw = b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: 5\r\nContent-Length: 10\r\n\r\nhello";
        let result = RequestView::parse(raw, &limits);
        assert!(matches!(result, Err(AetherError::MalformedRequest)));
    }

    #[test]
    fn test_smuggling_cl_te_conflict_rejected() {
        let limits = ServerLimits::default();
        let raw = b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n";
        let result = RequestView::parse(raw, &limits);
        assert!(matches!(result, Err(AetherError::MalformedRequest)));
    }

    /// `Transfer-Encoding` alone (with no `Content-Length`) must also be rejected while
    /// chunked support is absent — otherwise the body would be silently treated as empty
    /// and the trailing chunk data misparsed as the next pipelined request.
    #[test]
    fn test_smuggling_te_only_rejected() {
        let limits = ServerLimits::default();
        let raw = b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
        assert!(matches!(
            RequestView::parse(raw, &limits),
            Err(AetherError::MalformedRequest)
        ));
    }

    /// Obfuscated `Transfer-Encoding` spellings must not slip past the check.
    #[test]
    fn test_smuggling_obfuscated_transfer_encoding_rejected() {
        let limits = ServerLimits::default();
        for raw in [
            b"POST / HTTP/1.1\r\ntransfer-encoding: chunked\r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding : chunked\r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nTRANSFER-ENCODING: chunked\r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: identity, chunked\r\n\r\n".as_slice(),
        ] {
            assert!(
                matches!(RequestView::parse(raw, &limits), Err(AetherError::MalformedRequest)),
                "expected rejection for {:?}",
                String::from_utf8_lossy(raw)
            );
        }
    }

    /// A non-numeric or negative `Content-Length` must be rejected, not parsed as zero.
    #[test]
    fn test_malformed_content_length_rejected() {
        let limits = ServerLimits::default();
        for raw in [
            b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: abc\r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: -5\r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: 5x\r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: \r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: 5 5\r\n\r\n".as_slice(),
        ] {
            assert!(
                matches!(RequestView::parse(raw, &limits), Err(AetherError::MalformedRequest)),
                "expected rejection for {:?}",
                String::from_utf8_lossy(raw)
            );
        }
    }

    /// An astronomically large `Content-Length` must not overflow `header_len + cl`.
    #[test]
    fn test_content_length_overflow_is_rejected_not_wrapped() {
        let limits = ServerLimits::default();
        let raw = b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: 18446744073709551615\r\n\r\n";
        assert!(matches!(
            RequestView::parse(raw, &limits),
            Err(AetherError::BodyLimitExceeded)
        ));

        // Just past usize::MAX on 64-bit: parse::<usize> fails, still rejected.
        let raw2 = b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: 99999999999999999999999999\r\n\r\n";
        assert!(matches!(
            RequestView::parse(raw2, &limits),
            Err(AetherError::MalformedRequest)
        ));
    }

    /// `max_headers_count` must be enforced and reported as 431, not a generic 400.
    #[test]
    fn test_header_count_limit_is_enforced() {
        let mut limits = ServerLimits::default();
        limits.max_headers_count = 4;
        limits.max_header_bytes = 64 * 1024;

        let build = |n: usize| {
            let mut raw = String::from("GET / HTTP/1.1\r\n");
            for i in 0..n {
                raw.push_str(&format!("X-Header-{i}: value\r\n"));
            }
            raw.push_str("\r\n");
            raw.into_bytes()
        };

        // At the limit: accepted.
        let at_limit = build(4);
        let ok = RequestView::parse(&at_limit, &limits).unwrap();
        assert!(ok.is_some());

        // One past the limit: 431.
        let over_limit = build(5);
        assert!(
            matches!(RequestView::parse(&over_limit, &limits), Err(AetherError::HeaderTooLarge)),
            "configured max_headers_count must be enforced"
        );
    }

    /// The limit must be honoured above the 64-slot stack array too, via the heap fallback.
    #[test]
    fn test_header_count_limit_above_stack_slots() {
        let mut limits = ServerLimits::default();
        limits.max_headers_count = 100;
        limits.max_header_bytes = 64 * 1024;

        let build = |n: usize| {
            let mut raw = String::from("GET / HTTP/1.1\r\n");
            for i in 0..n {
                raw.push_str(&format!("X-Header-{i}: value\r\n"));
            }
            raw.push_str("\r\n");
            raw.into_bytes()
        };

        let at_limit = build(100);
        let ok = RequestView::parse(&at_limit, &limits).unwrap();
        let (req, _) = ok.expect("100 headers should parse under a 100-header budget");
        assert_eq!(req.headers.len(), 100);

        let over_limit = build(101);
        assert!(matches!(
            RequestView::parse(&over_limit, &limits),
            Err(AetherError::HeaderTooLarge)
        ));
    }

    /// Exceeding the header byte budget must yield 431 whether or not the head is complete.
    #[test]
    fn test_header_byte_budget_enforced() {
        let mut limits = ServerLimits::default();
        limits.max_header_bytes = 128;

        // Complete but oversized.
        let big = format!("GET / HTTP/1.1\r\nX-Pad: {}\r\n\r\n", "a".repeat(4096));
        assert!(matches!(
            RequestView::parse(big.as_bytes(), &limits),
            Err(AetherError::HeaderTooLarge)
        ));

        // Incomplete and already oversized (Slowloris shape).
        let partial = format!("GET / HTTP/1.1\r\nX-Pad: {}", "a".repeat(4096));
        assert!(matches!(
            RequestView::parse(partial.as_bytes(), &limits),
            Err(AetherError::HeaderTooLarge)
        ));
    }

    /// A `Content-Length` above the body budget must be refused before any body is read.
    #[test]
    fn test_body_limit_enforced_from_content_length() {
        let mut limits = ServerLimits::default();
        limits.max_body_bytes = 10;

        let raw = b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: 11\r\n\r\n0123456789a";
        assert!(matches!(
            RequestView::parse(raw, &limits),
            Err(AetherError::BodyLimitExceeded)
        ));

        let ok = b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: 10\r\n\r\n0123456789";
        let (req, consumed) = RequestView::parse(ok, &limits).unwrap().unwrap();
        assert_eq!(req.body, b"0123456789");
        assert_eq!(consumed, ok.len());
    }

    /// Partial requests must report "need more data" rather than an error.
    #[test]
    fn test_partial_requests_request_more_data() {
        let limits = ServerLimits::default();

        // Headers not terminated yet.
        assert!(RequestView::parse(b"GET / HTTP/1.1\r\nHost: x\r\n", &limits)
            .unwrap()
            .is_none());

        // Headers complete, body short.
        assert!(RequestView::parse(b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: 10\r\n\r\nabc", &limits)
            .unwrap()
            .is_none());

        // Empty buffer.
        assert!(RequestView::parse(b"", &limits).unwrap().is_none());
    }

    /// The consumed-byte count must be exact, or pipelining desynchronises the stream.
    #[test]
    fn test_consumed_offset_is_exact_for_pipelining() {
        let limits = ServerLimits::default();
        let raw = b"GET /a HTTP/1.1\r\n\r\nGET /b HTTP/1.1\r\n\r\n";

        let (req1, consumed1) = RequestView::parse(raw, &limits).unwrap().unwrap();
        assert_eq!(req1.path, "/a");

        let (req2, consumed2) = RequestView::parse(&raw[consumed1..], &limits).unwrap().unwrap();
        assert_eq!(req2.path, "/b");
        assert_eq!(consumed1 + consumed2, raw.len());
    }

    /// Path and query must be split without allocating, and `+`/`%` left untouched here —
    /// percent-decoding is a routing-layer concern, not a framing one.
    #[test]
    fn test_path_query_split() {
        let limits = ServerLimits::default();

        let (req, _) = RequestView::parse(b"GET /users?page=1&x=2 HTTP/1.1\r\n\r\n", &limits)
            .unwrap()
            .unwrap();
        assert_eq!(req.path, "/users");
        assert_eq!(req.query_str, Some("page=1&x=2"));
        assert_eq!(req.raw_uri, "/users?page=1&x=2");

        let (req, _) = RequestView::parse(b"GET /users? HTTP/1.1\r\n\r\n", &limits)
            .unwrap()
            .unwrap();
        assert_eq!(req.path, "/users");
        assert_eq!(req.query_str, Some(""));

        let (req, _) = RequestView::parse(b"GET /users HTTP/1.1\r\n\r\n", &limits)
            .unwrap()
            .unwrap();
        assert_eq!(req.path, "/users");
        assert_eq!(req.query_str, None);
    }

    /// Garbage on the wire must be rejected cleanly, never panic.
    #[test]
    fn test_malformed_requests_do_not_panic() {
        let limits = ServerLimits::default();
        let hostile: [&[u8]; 12] = [
            b"\x00\x00\x00",
            b"\r\n\r\n",
            b"GET",
            b"GET ",
            b"GET / ",
            b"GET / HTTP/1.1",
            b"NOTAMETHOD / HTTP/1.1\r\n\r\n",
            b"GET /\x00 HTTP/1.1\r\n\r\n",
            b"GET / HTTP/9.9\r\n\r\n",
            b"GET / HTTP/1.1\r\nBad Header No Colon\r\n\r\n",
            b"GET / HTTP/1.1\r\nHost: \xc3\x28\r\n\r\n",
            &[0xFF; 64],
        ];
        for raw in hostile {
            let _ = RequestView::parse(raw, &limits);
        }
    }

    /// Non-standard methods must not be silently coerced into a standard one.
    #[test]
    fn test_unknown_method_rejected() {
        let limits = ServerLimits::default();
        assert!(matches!(
            RequestView::parse(b"PROPFIND / HTTP/1.1\r\n\r\n", &limits),
            Err(AetherError::MalformedRequest)
        ));
    }

    // ------------------------------------------------------------------ chunked bodies

    fn chunked(body: &str) -> Vec<u8> {
        body.as_bytes().to_vec()
    }

    #[test]
    fn test_chunked_single_and_multi_chunk_decoded() {
        let limits = ServerLimits::default();

        let raw = b"POST /upload HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
        let (req, owned, consumed) = parse_request(raw, &limits).unwrap().unwrap();
        assert_eq!(req.path, "/upload");
        assert_eq!(owned.as_deref(), Some(b"hello".as_slice()));
        assert!(req.body.is_empty(), "owned bodies must not alias the connection buffer");
        assert_eq!(consumed, raw.len());

        let raw = b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n";
        let (_, owned, consumed) = parse_request(raw, &limits).unwrap().unwrap();
        assert_eq!(owned.as_deref(), Some(b"abcde".as_slice()));
        assert_eq!(consumed, raw.len());
    }

    /// Chunk extensions are legal and must be ignored, not treated as part of the size.
    #[test]
    fn test_chunk_extensions_ignored() {
        let limits = ServerLimits::default();
        let raw = b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n5;name=value;x=y\r\nhello\r\n0\r\n\r\n";
        let (_, owned, _) = parse_request(raw, &limits).unwrap().unwrap();
        assert_eq!(owned.as_deref(), Some(b"hello".as_slice()));
    }

    /// Every strict prefix of a chunked request must request more data, never error or
    /// report a short body.
    #[test]
    fn test_chunked_partial_at_every_prefix_requests_more() {
        let limits = ServerLimits::default();
        let raw = chunked(
            "POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\nX-T: 1\r\n\r\n",
        );

        for cut in 0..raw.len() {
            assert!(
                parse_request(&raw[..cut], &limits).unwrap().is_none(),
                "prefix of {} bytes must be incomplete",
                cut
            );
        }
        let (req, owned, consumed) = parse_request(&raw, &limits).unwrap().unwrap();
        assert_eq!(req.body.len(), 0);
        assert_eq!(owned.as_deref(), Some(b"hello world".as_slice()));
        assert_eq!(consumed, raw.len());
    }

    /// Consumed bytes must include the terminating chunk and trailers so a pipelined
    /// request behind a chunked body stays framed.
    #[test]
    fn test_chunked_consumed_offset_is_exact_for_pipelining() {
        let limits = ServerLimits::default();
        let raw = b"POST /a HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nbody\r\n0\r\nTrailer: yes\r\n\r\nGET /b HTTP/1.1\r\nHost: t\r\n\r\n";

        let (req1, owned, consumed) = parse_request(raw, &limits).unwrap().unwrap();
        assert_eq!(req1.path, "/a");
        assert_eq!(owned.as_deref(), Some(b"body".as_slice()));

        let (req2, owned2, consumed2) = parse_request(&raw[consumed..], &limits).unwrap().unwrap();
        assert_eq!(req2.path, "/b");
        assert!(owned2.is_none());
        assert_eq!(consumed + consumed2, raw.len());
    }

    /// The legacy entry point must keep rejecting Transfer-Encoding; only `parse_request`
    /// opts into chunked decoding.
    #[test]
    fn test_legacy_parse_still_rejects_transfer_encoding() {
        let limits = ServerLimits::default();
        let raw = b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n";
        assert!(matches!(
            RequestView::parse(raw, &limits),
            Err(AetherError::MalformedRequest)
        ));
    }

    #[test]
    fn test_chunked_with_content_length_rejected() {
        let limits = ServerLimits::default();
        let raw = b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
        assert!(matches!(
            parse_request(raw, &limits),
            Err(AetherError::MalformedRequest)
        ));
    }

    #[test]
    fn test_duplicate_and_unsupported_transfer_encoding_rejected() {
        let limits = ServerLimits::default();
        for raw in [
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: gzip\r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: identity\r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked, gzip\r\n\r\n0\r\n\r\n".as_slice(),
        ] {
            let res = parse_request(raw, &limits);
            assert!(
                matches!(res, Err(AetherError::MalformedRequest)),
                "expected rejection for {:?}, got {:?}",
                String::from_utf8_lossy(raw),
                res.map(|o| o.map(|(_, b, c)| (b, c)))
            );
        }
    }

    #[test]
    fn test_chunked_http10_rejected() {
        let limits = ServerLimits::default();
        let raw = b"POST / HTTP/1.0\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n";
        assert!(matches!(
            parse_request(raw, &limits),
            Err(AetherError::MalformedRequest)
        ));
    }

    #[test]
    fn test_chunked_malformed_framing_rejected() {
        let limits = ServerLimits::default();
        for raw in [
            // Non-hex size.
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n".as_slice(),
            // Signed / 0x-prefixed size.
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n+5\r\nhello\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n0x5\r\nhello\r\n".as_slice(),
            // Chunk data not followed by CRLF.
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhelloXX".as_slice(),
            // Size over 16 hex digits (would overflow a reasonable usize).
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n11111111111111111\r\n".as_slice(),
            // Trailer without a colon.
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nBadTrailer\r\n\r\n".as_slice(),
            // Obsolete line folding / space before the trailer colon.
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nX-T : 1\r\n\r\n".as_slice(),
        ] {
            assert!(
                matches!(parse_request(raw, &limits), Err(AetherError::MalformedRequest)),
                "expected rejection for {:?}",
                String::from_utf8_lossy(raw)
            );
        }
    }

    #[test]
    fn test_chunked_body_limit_enforced() {
        let mut limits = ServerLimits::default();
        limits.max_body_bytes = 10;

        // Declared chunk size alone exceeds the budget: reject before buffering data.
        let raw = b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n11\r\n";
        assert!(matches!(
            parse_request(raw, &limits),
            Err(AetherError::BodyLimitExceeded)
        ));

        // Individually legal chunks that together exceed the budget.
        let raw = b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n6\r\nabcdef\r\n6\r\nghijkl\r\n0\r\n\r\n";
        assert!(matches!(
            parse_request(raw, &limits),
            Err(AetherError::BodyLimitExceeded)
        ));

        // Exactly at the budget is accepted.
        let raw = b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\na\r\n0123456789\r\n0\r\n\r\n";
        let (_, owned, _) = parse_request(raw, &limits).unwrap().unwrap();
        assert_eq!(owned.as_deref(), Some(b"0123456789".as_slice()));
    }

    #[test]
    fn test_chunked_trailers_are_bounded() {
        let mut limits = ServerLimits::default();
        limits.max_header_bytes = 100;
        limits.max_headers_count = 2;

        let raw = b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nA: 1\r\n\r\n";
        let (_, owned, _) = parse_request(raw, &limits).unwrap().unwrap();
        assert_eq!(owned.as_deref(), Some(b"".as_slice()));

        let padding = "a".repeat(200);
        let too_large = format!(
            "POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nX-Padding: {padding}\r\n\r\n"
        );
        assert!(matches!(
            parse_request(too_large.as_bytes(), &limits),
            Err(AetherError::HeaderTooLarge)
        ));

        let too_many = b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nA: 1\r\nB: 2\r\nC: 3\r\n\r\n";
        assert!(matches!(
            parse_request(too_many, &limits),
            Err(AetherError::HeaderTooLarge)
        ));
    }

    /// Mixed case and surrounding OWS are legal for the `chunked` token.
    #[test]
    fn test_chunked_token_is_case_insensitive() {
        let limits = ServerLimits::default();
        let raw = b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: Chunked \r\n\r\n1\r\nx\r\n0\r\n\r\n";
        let (_, owned, _) = parse_request(raw, &limits).unwrap().unwrap();
        assert_eq!(owned.as_deref(), Some(b"x".as_slice()));
    }

    /// Regression: scanning for the version token used to split the head on `\r`, so a
    /// bare-LF request line (which httparse accepts) made the "first line" the whole
    /// header block, and a later header value containing `HTTP/1.` — e.g. a Referer URL —
    /// decided the connection's keep-alive semantics.
    #[test]
    fn test_version_minor_reads_only_the_first_line() {
        assert_eq!(request_line_version_minor(b"GET / HTTP/1.0\nHost: x\n\n"), 0);
        assert_eq!(request_line_version_minor(b"GET / HTTP/1.1\nHost: x\n\n"), 1);
        assert_eq!(request_line_version_minor(b"GET / HTTP/1.0\r\nHost: x\r\n\r\n"), 0);

        let crlf = b"GET /x?y=1 HTTP/1.1\r\nReferer: http://e/HTTP/1.0/page\r\n\r\n";
        assert_eq!(request_line_version_minor(crlf), 1);
        let lf_only = b"GET /x?y=1 HTTP/1.1\nReferer: http://e/HTTP/1.0/page\n\n";
        assert_eq!(request_line_version_minor(lf_only), 1);

        // Missing or non-numeric version tokens fall back to 1.
        assert_eq!(request_line_version_minor(b"GET / weird\r\n\r\n"), 1);
        assert_eq!(request_line_version_minor(b"GET / HTTP/1.x\r\n\r\n"), 1);
    }

    /// RFC 9112 §3.2: an HTTP/1.1 request must carry exactly one Host field. Zero leaves
    /// routing undefined; two is a smuggling vector (first-vs-last divergence). HTTP/1.0
    /// predates the rule and must stay accepted.
    #[test]
    fn test_http11_requires_exactly_one_host() {
        let limits = ServerLimits::default();

        assert!(matches!(
            parse_request(b"GET / HTTP/1.1\r\nUser-Agent: x\r\n\r\n", &limits),
            Err(AetherError::MalformedRequest)
        ));
        assert!(matches!(
            parse_request(b"GET / HTTP/1.1\r\nHost: a\r\nhost: b\r\n\r\n", &limits),
            Err(AetherError::MalformedRequest)
        ));

        let (req, _, _) = parse_request(b"GET / HTTP/1.1\r\nHost: example\r\n\r\n", &limits)
            .unwrap()
            .unwrap();
        assert_eq!(req.path, "/");

        assert!(parse_request(b"GET / HTTP/1.0\r\n\r\n", &limits)
            .unwrap()
            .is_some());
    }

    /// RFC 9112 §6.5: trailers must not carry framing- or routing-critical fields. The
    /// head was already validated without them, so accepting these names would smuggle a
    /// second Content-Length/Transfer-Encoding/Host past the parser. Ordinary trailers
    /// remain legal.
    #[test]
    fn test_chunked_rejects_framing_trailer_names() {
        let limits = ServerLimits::default();
        for raw in [
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nContent-Length: 99\r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nTRANSFER-ENCODING: chunked\r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nHost: evil\r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\ncontent-length: 0\r\n\r\n".as_slice(),
        ] {
            assert!(
                matches!(parse_request(raw, &limits), Err(AetherError::MalformedRequest)),
                "expected rejection for {:?}",
                String::from_utf8_lossy(raw)
            );
        }

        let ok = b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nX-Checksum: abc\r\n\r\n";
        assert!(parse_request(ok, &limits).unwrap().is_some());
    }

    /// T1-a (CL.TE vector): framing values may be padded only by RFC 9112 OWS
    /// (SP/HTAB). `str::trim` additionally strips Unicode whitespace such as
    /// U+00A0, so `Transfer-Encoding: <NBSP>chunked` framed the body as chunked
    /// for Aether while strict frontends see a non-`chunked` token and fall back
    /// to Content-Length. Both spellings must be refused.
    #[test]
    fn test_framing_headers_accept_only_ascii_ows() {
        let limits = ServerLimits::default();
        let cases = [
            // NBSP-prefixed/suffixed chunked -> must NOT be treated as chunked
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: \xc2\xa0chunked\r\n\r\n0\r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\xc2\xa0\r\n\r\n0\r\n\r\n".as_slice(),
            // NBSP-padded Content-Length -> must NOT parse as the number
            b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: \xc2\xa05\r\n\r\nhello".as_slice(),
            b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length: 5\xc2\xa0\r\n\r\nhello".as_slice(),
        ];
        for raw in cases {
            assert!(
                matches!(parse_request(raw, &limits), Err(AetherError::MalformedRequest)),
                "expected rejection for {:?}",
                String::from_utf8_lossy(raw)
            );
        }
        // OWS (SP/HTAB) continues to be accepted.
        let ok =
            b"POST / HTTP/1.1\r\nHost: t\r\nContent-Length:\t5 \r\n\r\nhello";
        assert!(parse_request(ok, &limits).unwrap().is_some());
    }

    /// T1-d: the exactly-one-Host rule must also require a non-empty field value
    /// (RFC 9112 §3.2.2 origin-form); `Host: ` is not a valid authority.
    #[test]
    fn test_http11_rejects_empty_host_value() {
        let limits = ServerLimits::default();
        assert!(matches!(
            parse_request(b"GET / HTTP/1.1\r\nHost: \r\n\r\n", &limits),
            Err(AetherError::MalformedRequest)
        ));
        assert!(matches!(
            parse_request(b"GET / HTTP/1.1\r\nHost:\t\r\n\r\n", &limits),
            Err(AetherError::MalformedRequest)
        ));
        assert!(parse_request(b"GET / HTTP/1.1\r\nHost: a\r\n\r\n", &limits)
            .unwrap()
            .is_some());
    }
}

