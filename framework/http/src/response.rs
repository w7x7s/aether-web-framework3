//! # Aether Response Builder
//!
//! Pre-formats HTTP/1.1 response status lines and headers directly into
//! output bytes slices to eliminate temporary allocations.

use bytes::{Bytes, BytesMut};

pub struct Response {
    pub status: u16,
    pub status_text: &'static str,
    pub headers: Vec<(&'static str, String)>,
    pub body: Bytes,
}

/// Percent-encodes a Set-Cookie component. `is_value=false` applies the RFC 7230
/// token set (cookie names must be tokens); `is_value=true` applies the RFC 6265
/// cookie-value octet set, which keeps `=` but forbids `;`, `,`, `"`/`\`, space
/// and control characters. Bytes outside the allowed set (including every byte of
/// a non-ASCII sequence) become `%XX` with uppercase hex.
fn encode_cookie_component(s: &str, is_value: bool) -> String {
    fn allowed(b: u8, is_value: bool) -> bool {
        if is_value {
            b == 0x21
                || (0x23..=0x2B).contains(&b)
                || (0x2D..=0x3A).contains(&b)
                || (0x3C..=0x5B).contains(&b)
                || (0x5D..=0x7E).contains(&b)
        } else {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-'
                        | b'^' | b'_' | b'`' | b'|' | b'~'
                )
        }
    }
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        if allowed(*b, is_value) {
            out.push(*b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

impl Response {
    pub fn ok() -> Self {
        Self {
            status: 200,
            status_text: "OK",
            headers: Vec::with_capacity(6),
            body: Bytes::new(),
        }
    }

    pub fn json(body: Bytes) -> Self {
        let mut r = Self::ok();
        r.headers.push(("Content-Type", "application/json; charset=utf-8".to_string()));
        r.headers.push(("Content-Length", body.len().to_string()));
        r.body = body;
        r
    }

    /// Creates a JSON response with a custom HTTP status code and status text
    pub fn json_status(status: u16, status_text: &'static str, body: impl Into<Bytes>) -> Self {
        let b = body.into();
        let mut r = Self::status(status, status_text);
        r.headers.push(("Content-Type", "application/json; charset=utf-8".to_string()));
        r.headers.push(("Content-Length", b.len().to_string()));
        r.body = b;
        r
    }

    pub fn text(body: &'static str) -> Self {
        let mut r = Self::ok();
        r.headers.push(("Content-Type", "text/plain; charset=utf-8".to_string()));
        r.headers.push(("Content-Length", body.len().to_string()));
        r.body = Bytes::from_static(body.as_bytes());
        r
    }

    pub fn html(body: &'static str) -> Self {
        let mut r = Self::ok();
        r.headers.push(("Content-Type", "text/html; charset=utf-8".to_string()));
        r.headers.push(("Content-Length", body.len().to_string()));
        r.body = Bytes::from_static(body.as_bytes());
        r
    }

    /// Creates an HTML response from a dynamic owned String
    pub fn html_dynamic(body: impl Into<String>) -> Self {
        let b = body.into();
        let bytes = Bytes::from(b.into_bytes());
        Self::bytes(bytes, "text/html; charset=utf-8")
    }

    /// Creates a Plain Text response from a dynamic owned String
    pub fn text_dynamic(body: impl Into<String>) -> Self {
        let b = body.into();
        let bytes = Bytes::from(b.into_bytes());
        Self::bytes(bytes, "text/plain; charset=utf-8")
    }

    /// Reads and serves a file directly from the filesystem with automatic MIME type detection.
    ///
    /// Blocking: performs `std::fs::read` synchronously. Call it outside runtime workers
    /// (startup, build, tests); async routes use `App::static_file`/`static_dir` instead.
    pub fn file(path: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        let path = path.as_ref();
        let content = std::fs::read(path)?;
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let mime = crate::static_files::mime_from_extension(ext);
        Ok(Self::bytes(Bytes::from(content), mime))
    }

    pub fn bytes(body: Bytes, content_type: &'static str) -> Self {
        let mut r = Self::ok();
        r.headers.push(("Content-Type", content_type.to_string()));
        r.headers.push(("Content-Length", body.len().to_string()));
        r.body = body;
        r
    }

    pub fn status(status: u16, msg: &'static str) -> Self {
        Self {
            status,
            status_text: msg,
            headers: Vec::with_capacity(4),
            body: Bytes::new(),
        }
    }

    /// Creates an HTTP redirect response (301 Moved Permanently, 302 Found, 307 Temporary Redirect, or 308 Permanent Redirect)
    pub fn redirect(location: impl Into<String>, status: u16) -> Self {
        let mut r = Self::status(status, canonical_reason(status));
        r.headers.push(("Location", location.into()));
        r
    }

    /// Attaches a secure Cookie to the response (`Set-Cookie` header)
    ///
    /// Name and value are percent-encoded wherever RFC 6265 forbids the byte
    /// (`;`, `,`, whitespace, `"`/`\` in the value; any non-token byte in the
    /// name), so a caller-supplied string can never inject additional cookie
    /// attributes. Legitimate values round-trip byte-identically. `SameSite`
    /// accepts only the three registered spellings; anything else falls back
    /// to `Lax`.
    pub fn set_cookie(
        mut self,
        name: &str,
        value: &str,
        http_only: bool,
        secure: bool,
        same_site: &str,
        max_age_secs: Option<u64>,
    ) -> Self {
        let same_site = if same_site.eq_ignore_ascii_case("strict") {
            "Strict"
        } else if same_site.eq_ignore_ascii_case("none") {
            "None"
        } else {
            "Lax"
        };
        let mut cookie_str = format!(
            "{}={}; SameSite={}; Path=/",
            encode_cookie_component(name, false),
            encode_cookie_component(value, true),
            same_site
        );
        if http_only {
            cookie_str.push_str("; HttpOnly");
        }
        if secure {
            cookie_str.push_str("; Secure");
        }
        if let Some(age) = max_age_secs {
            cookie_str.push_str(&format!("; Max-Age={}", age));
        }
        self.headers.push(("Set-Cookie", cookie_str));
        self
    }

    /// Adds a custom header (builder pattern)
    pub fn header(mut self, key: &'static str, val: impl Into<String>) -> Self {
        self.headers.push((key, val.into()));
        self
    }

    /// Adds a custom header in-place
    pub fn add_header(&mut self, key: &'static str, val: impl Into<String>) {
        self.headers.push((key, val.into()));
    }

    /// Formats the complete HTTP/1.1 payload into a reusable buffer with zero intermediate String allocations.
    ///
    /// This function is the sole authority for message framing: any `Content-Length` present in
    /// `self.headers` is ignored and exactly one is derived from `self.body`. Emitting a second
    /// `Content-Length` from a constructor or middleware would be a response-splitting vector, so
    /// the invariant is enforced here rather than trusted at every call site.
    ///
    /// Header values are sanitised for the same reason. `Location` and `Set-Cookie` routinely
    /// carry caller- or client-derived data, and RFC 9110 §5.5 forbids CR, LF and NUL inside a
    /// field value — a value containing them would let an attacker inject whole response headers.
    pub fn write_http1_to(&self, out: &mut BytesMut) {
        self.write_http1_to_with_body(out, true);
    }

    /// As [`Self::write_http1_to`], but optionally omits the body bytes.
    ///
    /// A `HEAD` response must carry the header fields — including the `Content-Length` — that
    /// the equivalent `GET` would have produced, while sending no content (RFC 9110 §9.3.2).
    /// Passing `false` preserves the framing headers and drops only the payload.
    pub fn write_http1_to_with_body(&self, out: &mut BytesMut, include_body: bool) {
        use std::io::Write;
        {
            let mut writer = bytes::BufMut::writer(&mut *out);
            let _ = write!(writer, "HTTP/1.1 {} {}\r\n", self.status, self.status_text);

            let mut has_date = false;
            for (k, v) in &self.headers {
                if k.eq_ignore_ascii_case("content-length") {
                    continue;
                }
                // The body is always Content-Length framed here; emitting a
                // caller-set `Transfer-Encoding` alongside it would put the
                // client in TE.CL response-desync territory.
                if k.eq_ignore_ascii_case("transfer-encoding") {
                    continue;
                }
                if k.eq_ignore_ascii_case("date") {
                    has_date = true;
                }
                let _ = write!(writer, "{}: ", k);
                let _ = write_sanitized_field_value(&mut writer, v);
                let _ = write!(writer, "\r\n");
            }

            if !has_date {
                write_http_date_now(&mut writer);
            }

            // RFC 9110 §15.3.5 (204) and §15.4.5 (304): these responses carry no body, so a
            // Content-Length must not be emitted. Same for 1xx informational responses.
            if !status_forbids_body(self.status) {
                let _ = write!(writer, "Content-Length: {}\r\n", self.body.len());
            }

            let _ = write!(writer, "\r\n");
        }
        // A status that forbids a body must drop the payload too, not just the
        // Content-Length header: stray bytes after the head would be parsed by the
        // client as the start of the next pipelined response, desynchronising the
        // connection for every request that follows on the same socket.
        if include_body && !status_forbids_body(self.status) {
            out.extend_from_slice(&self.body);
        }
    }
}

/// Writes a header field value, dropping the bytes that RFC 9110 §5.5 forbids inside one.
/// The common case (a clean value) is a single `write_str` with no per-byte work.
fn write_sanitized_field_value<W: std::io::Write>(w: &mut W, value: &str) -> std::io::Result<()> {
    if !value.bytes().any(|b| b == b'\r' || b == b'\n' || b == b'\0') {
        return w.write_all(value.as_bytes());
    }
    for &b in value.as_bytes() {
        if b != b'\r' && b != b'\n' && b != b'\0' {
            w.write_all(&[b])?;
        }
    }
    Ok(())
}

/// True for status codes that must not carry a message body.
pub fn status_forbids_body(status: u16) -> bool {
    matches!(status, 204 | 304) || (100..200).contains(&status)
}

/// The IANA-registered reason phrase for a status code (RFC 9110 §15).
///
/// Callers that build a response from a bare numeric status must not invent a phrase — a
/// `404 Custom` status line confuses clients, intermediaries and log analysis alike.
pub fn canonical_reason(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        101 => "Switching Protocols",
        102 => "Processing",
        103 => "Early Hints",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        203 => "Non-Authoritative Information",
        204 => "No Content",
        205 => "Reset Content",
        206 => "Partial Content",
        207 => "Multi-Status",
        208 => "Already Reported",
        226 => "IM Used",
        300 => "Multiple Choices",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        305 => "Use Proxy",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        407 => "Proxy Authentication Required",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        411 => "Length Required",
        412 => "Precondition Failed",
        413 => "Content Too Large",
        414 => "URI Too Long",
        415 => "Unsupported Media Type",
        416 => "Range Not Satisfiable",
        417 => "Expectation Failed",
        418 => "I'm a Teapot",
        421 => "Misdirected Request",
        422 => "Unprocessable Content",
        423 => "Locked",
        424 => "Failed Dependency",
        425 => "Too Early",
        426 => "Upgrade Required",
        428 => "Precondition Required",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        451 => "Unavailable For Legal Reasons",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        505 => "HTTP Version Not Supported",
        506 => "Variant Also Negotiates",
        507 => "Insufficient Storage",
        508 => "Loop Detected",
        510 => "Not Extended",
        511 => "Network Authentication Required",
        // RFC 9110 §15 permits an empty reason phrase for unregistered codes; clients key off
        // the numeric status, so a blank phrase is correct where a guessed one would be wrong.
        _ => "",
    }
}

/// Howard Hinnant's civil_from_days: converts days since 1970-01-01 into (year, month, day).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (y + i64::from(m <= 2), m, d)
}

/// Formats a UNIX timestamp as an IMF-fixdate string (RFC 9110 §5.6.7), allocation-light.
pub fn format_http_date(unix_secs: u64) -> String {
    const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];

    let days = (unix_secs / 86_400) as i64;
    let secs_of_day = (unix_secs % 86_400) as u32;
    let (h, mi, s) = (
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    );
    let (y, m, d) = civil_from_days(days);
    // 1970-01-01 was a Thursday, which is index 4 in a Sunday-first week.
    let weekday = WEEKDAYS[(((days % 7) + 7 + 4) % 7) as usize];

    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
        weekday,
        d,
        MONTHS[(m - 1) as usize],
        y,
        h,
        mi,
        s
    )
}

/// Current time as an IMF-fixdate string.
pub fn http_date_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_http_date(secs)
}

/// Writes the `Date` field, reusing one formatted value per UNIX second.
///
/// IMF-fixdate has one-second granularity (RFC 9110 §5.6.7), so every response in the same
/// second is byte-identical in this field. Caching it removes a calendar computation, a
/// `format!`, and a heap allocation from each response. The lock is held only across the
/// memcpy-fast write path — never across I/O or `await`. Poisoning cannot leave this cache
/// inconsistent (the value is a plain `(secs, string)` pair), so the data is kept on recovery.
/// A clock that steps back (NTP) regenerates the string for the new, earlier second.
fn write_http_date_now<W: std::io::Write>(writer: &mut W) {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<Option<(u64, String)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));

    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    if guard.as_ref().map_or(true, |(s, _)| *s != secs) {
        *guard = Some((secs, format_http_date(secs)));
    }
    // Unset immediately after the assignment above; unwrap cannot fail here.
    let date = guard.as_ref().expect("date cache populated above").1.as_str();
    let _ = write!(writer, "Date: {date}\r\n");
}

#[cfg(test)]
fn write_http_date_to(out: &mut BytesMut) {
    write_http_date_now(&mut bytes::BufMut::writer(out));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_date_matches_known_fixtures() {
        // RFC 9110 §5.6.7 example
        assert_eq!(format_http_date(784111777), "Sun, 06 Nov 1994 08:49:37 GMT");
        assert_eq!(format_http_date(0), "Thu, 01 Jan 1970 00:00:00 GMT");
        assert_eq!(format_http_date(86399), "Thu, 01 Jan 1970 23:59:59 GMT");
        assert_eq!(format_http_date(86400), "Fri, 02 Jan 1970 00:00:00 GMT");
        // Leap-day and month/year boundary handling
        assert_eq!(format_http_date(1709164800), "Thu, 29 Feb 2024 00:00:00 GMT");
        assert_eq!(format_http_date(1709251200), "Fri, 01 Mar 2024 00:00:00 GMT");
        assert_eq!(format_http_date(1740787200), "Sat, 01 Mar 2025 00:00:00 GMT");
    }

    #[test]
    fn test_cached_date_write_matches_uncached_format() {
        // The cached writer must emit exactly what format_http_date produces for the
        // current second. Sampling before/after tolerates the tick crossing the second
        // boundary mid-write; any other value is a cache/format divergence.
        let now_secs = || {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        };
        let before = now_secs();
        let mut out = BytesMut::new();
        write_http_date_to(&mut out);
        let after = now_secs();

        let line = std::str::from_utf8(&out).unwrap();
        let value = line
            .strip_prefix("Date: ")
            .and_then(|v| v.strip_suffix("\r\n"))
            .expect("output must be a single Date field line");
        let allowed = [
            format_http_date(before),
            format_http_date(after),
            format_http_date(before + 1),
        ];
        assert!(
            allowed.iter().any(|a| a == value),
            "cached date {value:?} not a valid current fixdate (window {before}..{after})"
        );

        // Repeated writes stay byte-identical inside one second (the cache working),
        // and always parse back as a valid fixdate.
        let mut again = BytesMut::new();
        write_http_date_to(&mut again);
        assert_eq!(out, again, "Date width must be stable");
    }

    #[test]
    fn test_cached_date_single_emission_per_response() {        let mut out = BytesMut::new();
        Response::text("hi").write_http1_to(&mut out);
        let text = std::str::from_utf8(&out).unwrap();
        let head = text.split("\r\n\r\n").next().unwrap();
        let count = head
            .lines()
            .filter(|l| l.to_ascii_lowercase().starts_with("date:"))
            .count();
        assert_eq!(count, 1);
        assert!(
            head.lines().last().unwrap().starts_with("Content-Length:"),
            "Date must slot between the fields and Content-Length, {head:?}"
        );
    }

    #[test]
    #[ignore = "perf microbench; run with cargo test --release -p aether-http -- --ignored --nocapture"]
    fn bench_date_write_paths() {
        use std::io::Write;
        use std::time::Instant;

        let n = 200_000usize;
        let mut buf = BytesMut::with_capacity(128);

        let uncached = |buf: &mut BytesMut| {
            buf.clear();
            let mut w = bytes::BufMut::writer(&mut *buf);
            let _ = write!(w, "Date: {}\r\n", http_date_now());
        };
        let cached = |buf: &mut BytesMut| {
            buf.clear();
            let mut w = bytes::BufMut::writer(&mut *buf);
            write_http_date_now(&mut w);
        };

        // Warm caches (time-of-day page, allocator) before the first measurement.
        for _ in 0..1000 {
            uncached(&mut buf);
            cached(&mut buf);
        }

        for round in 0..3 {
            let t0 = Instant::now();
            for _ in 0..n {
                uncached(&mut buf);
            }
            let t_unc = t0.elapsed();
            let t1 = Instant::now();
            for _ in 0..n {
                cached(&mut buf);
            }
            let t_cch = t1.elapsed();
            println!(
                "round{round}: uncached {:.3}µs/op  cached {:.3}µs/op  ratio x{:.2}",
                t_unc.as_secs_f64() * 1e6 / n as f64,
                t_cch.as_secs_f64() * 1e6 / n as f64,
                t_unc.as_secs_f64() / t_cch.as_secs_f64()
            );
        }
        assert!(buf.starts_with(b"Date: "));
    }

    #[test]
    fn test_write_http1_emits_exactly_one_content_length() {
        // A constructor or middleware that pushed its own stale Content-Length must not
        // be able to produce a second one: that is a response-splitting vector.
        let mut res = Response::json(Bytes::from_static(b"{\"ok\":true}"));
        res.headers.push(("Content-Length", "9999".to_string()));
        res.headers.push(("Content-Length", "0".to_string()));

        let mut out = BytesMut::new();
        res.write_http1_to(&mut out);
        let text = std::str::from_utf8(&out).unwrap();

        let count = text
            .lines()
            .filter(|l| l.to_ascii_lowercase().starts_with("content-length:"))
            .count();
        assert_eq!(count, 1, "must emit exactly one Content-Length, got: {}", text);
        assert!(text.contains(&format!("Content-Length: {}", res.body.len())));
        assert!(!text.contains("9999"));
    }

    #[test]
    fn test_write_http1_includes_date_once() {
        let res = Response::text("hi");
        let mut out = BytesMut::new();
        res.write_http1_to(&mut out);
        let text = std::str::from_utf8(&out).unwrap();

        let count = text
            .lines()
            .filter(|l| l.to_ascii_lowercase().starts_with("date:"))
            .count();
        assert_eq!(count, 1);
        assert!(text.contains("GMT\r\n"), "date must be IMF-fixdate: {}", text);
    }

    #[test]
    fn test_user_supplied_date_is_preserved_not_duplicated() {
        let res = Response::text("hi").header("Date", "Thu, 01 Jan 1970 00:00:00 GMT");
        let mut out = BytesMut::new();
        res.write_http1_to(&mut out);
        let text = std::str::from_utf8(&out).unwrap();

        let count = text
            .lines()
            .filter(|l| l.to_ascii_lowercase().starts_with("date:"))
            .count();
        assert_eq!(count, 1);
        assert!(text.contains("Date: Thu, 01 Jan 1970 00:00:00 GMT"));
    }

    #[test]
    fn test_204_and_304_carry_no_content_length() {
        for status in [204u16, 304] {
            let res = Response::status(status, "x");
            let mut out = BytesMut::new();
            res.write_http1_to(&mut out);
            let text = std::str::from_utf8(&out).unwrap();
            assert!(
                !text.to_ascii_lowercase().contains("content-length:"),
                "{} must not carry Content-Length, got: {}",
                status,
                text
            );
        }
        assert!(status_forbids_body(204));
        assert!(status_forbids_body(304));
        assert!(status_forbids_body(100));
        assert!(!status_forbids_body(200));
        assert!(!status_forbids_body(404));
    }

    /// Regression: suppressing only the `Content-Length` while still writing `self.body`
    /// leaked stray bytes onto the wire for 204/304/1xx. With no framing header, the next
    /// request's response parser reads them as the start of a new message, desynchronising
    /// the whole keep-alive connection.
    #[test]
    fn test_bodyless_status_drops_stray_payload() {
        for status in [204u16, 304, 100, 199] {
            let mut res = Response::status(status, "x");
            res.body = Bytes::from_static(b"LEAKED");
            let mut out = BytesMut::new();
            res.write_http1_to(&mut out);
            let text = std::str::from_utf8(&out).unwrap();
            assert!(
                !text.contains("LEAKED"),
                "{status} must not emit any body bytes, got: {text}"
            );

            // Same via the HEAD-suppression path: headers kept, payload dropped.
            let mut out2 = BytesMut::new();
            res.write_http1_to_with_body(&mut out2, false);
            assert!(!std::str::from_utf8(&out2).unwrap().contains("LEAKED"));
        }

        // Control: a body-permitting status must still emit its payload.
        let mut res = Response::status(200, "OK");
        res.body = Bytes::from_static(b"KEEP");
        let mut out = BytesMut::new();
        res.write_http1_to(&mut out);
        assert!(std::str::from_utf8(&out).unwrap().contains("KEEP"));
    }

    #[test]
    fn test_json_status_has_single_correct_content_length() {
        // Regression: json_status used to inherit "Content-Length: 0" from status()
        // and then push a second, real one.
        let res = Response::json_status(404, "Not Found", Bytes::from_static(b"{\"e\":1}"));
        let mut out = BytesMut::new();
        res.write_http1_to(&mut out);
        let text = std::str::from_utf8(&out).unwrap();

        let count = text
            .lines()
            .filter(|l| l.to_ascii_lowercase().starts_with("content-length:"))
            .count();
        assert_eq!(count, 1, "got: {}", text);
        assert!(text.contains("Content-Length: 7"), "got: {}", text);
        assert!(text.starts_with("HTTP/1.1 404 Not Found\r\n"));
    }

    /// A caller- or client-derived header value must not be able to inject extra headers.
    #[test]
    fn test_header_value_crlf_injection_is_neutralised() {
        let malicious = "/ok\r\nSet-Cookie: session=attacker\r\nX-Injected: yes";
        let res = Response::redirect(malicious, 302);

        let mut out = BytesMut::new();
        res.write_http1_to(&mut out);
        let text = std::str::from_utf8(&out).unwrap();

        let header_section = text.split("\r\n\r\n").next().unwrap();
        // Stripping CR/LF leaves the attacker's text inline inside the Location value, so the
        // assertion has to be about line structure, not substring absence.
        let lines: Vec<&str> = header_section.split("\r\n").collect();
        assert_eq!(
            lines.len(),
            4,
            "expected status + Location + Date + Content-Length only, got: {:#?}",
            lines
        );
        assert!(lines[0].starts_with("HTTP/1.1 302 Found"));
        assert!(lines[1].starts_with("Location: /ok"), "got: {}", lines[1]);
        assert!(
            !lines.iter().skip(1).any(|l| l.starts_with("Set-Cookie:")),
            "injected Set-Cookie became a real header: {:#?}",
            lines
        );
        assert!(
            !lines.iter().skip(1).any(|l| l.starts_with("X-Injected:")),
            "injected X-Injected became a real header: {:#?}",
            lines
        );
        assert!(!header_section.contains("\n\n"));
    }

    #[test]
    fn test_header_value_nul_and_lone_lf_are_stripped() {
        let mut res = Response::ok();
        res.add_header("X-A", "before\0after");
        res.add_header("X-B", "line1\nline2");
        res.add_header("X-C", "line1\rline2");

        let mut out = BytesMut::new();
        res.write_http1_to(&mut out);
        let text = std::str::from_utf8(&out).unwrap();
        let header_section = text.split("\r\n\r\n").next().unwrap();

        assert!(!header_section.contains('\0'));
        // status + X-A + X-B + X-C + Date + Content-Length, and nothing more.
        let lines: Vec<&str> = header_section.split("\r\n").collect();
        assert_eq!(lines.len(), 6, "got: {:#?}", lines);
        assert!(lines.contains(&"X-A: beforeafter"), "{:#?}", lines);
        assert!(lines.contains(&"X-B: line1line2"), "{:#?}", lines);
        assert!(lines.contains(&"X-C: line1line2"), "{:#?}", lines);
    }

    /// Clean values must pass through untouched, including UTF-8.
    #[test]
    fn test_clean_header_values_are_untouched() {
        let mut res = Response::ok();
        res.add_header("X-Arabic", "مرحبا بالعالم");
        res.add_header("X-Token", "abc.def-ghi_jkl~mno+pqr/stu=vwx");

        let mut out = BytesMut::new();
        res.write_http1_to(&mut out);
        let text = std::str::from_utf8(&out).unwrap();

        assert!(text.contains("X-Arabic: مرحبا بالعالم"));
        assert!(text.contains("X-Token: abc.def-ghi_jkl~mno+pqr/stu=vwx"));
    }

    #[test]
    fn test_canonical_reason_phrases() {
        assert_eq!(canonical_reason(200), "OK");
        assert_eq!(canonical_reason(204), "No Content");
        assert_eq!(canonical_reason(301), "Moved Permanently");
        assert_eq!(canonical_reason(404), "Not Found");
        assert_eq!(canonical_reason(405), "Method Not Allowed");
        assert_eq!(canonical_reason(413), "Content Too Large");
        assert_eq!(canonical_reason(431), "Request Header Fields Too Large");
        assert_eq!(canonical_reason(500), "Internal Server Error");
        assert_eq!(canonical_reason(504), "Gateway Timeout");
        // Unregistered codes get an empty phrase rather than a fabricated one.
        assert_eq!(canonical_reason(599), "");
        assert_eq!(canonical_reason(0), "");
    }

    #[test]
    fn test_redirect_uses_canonical_reason() {
        let res = Response::redirect("/new", 308);
        let mut out = BytesMut::new();
        res.write_http1_to(&mut out);
        let text = std::str::from_utf8(&out).unwrap();
        assert!(text.starts_with("HTTP/1.1 308 Permanent Redirect\r\n"), "got: {}", text);

        // Previously any non-redirect status fell through to "See Other".
        let res = Response::redirect("/x", 200);
        assert_eq!(res.status_text, "OK");
    }

    /// T1-b: the writer is the sole framing authority (Content-Length derived from
    /// the body). A caller-set `Transfer-Encoding` would co-exist with that
    /// Content-Length on the wire — a response-desync vector — so it must not be
    /// emitted.
    #[test]
    fn test_caller_transfer_encoding_never_emitted() {
        let res = Response::text_dynamic("hi")
            .header("Transfer-Encoding", "chunked");
        let mut out = BytesMut::new();
        res.write_http1_to(&mut out);
        let text = String::from_utf8_lossy(&out).to_lowercase();
        assert!(
            !text.contains("transfer-encoding"),
            "TE leaked into a Content-Length-framed response: {text}"
        );
        assert!(text.contains("content-length: 2"), "got: {text}");
    }

    /// T1-c: RFC 6265 forbids `;`, `,`, whitespace, `"` and `\` inside the
    /// cookie-value and requires a token name/attribute — unfiltered callers
    /// could inject extra Set-Cookie attributes (e.g. turn a session cookie into
    /// `sid=x; Admin=1`). They must be percent-encoded; legitimate values stay
    /// byte-identical.
    #[test]
    fn test_set_cookie_encodes_injection_characters() {
        let res = Response::ok().set_cookie("sid", "a=b; Admin=1", true, true, "Lax", None);
        let (_, value) = res
            .headers
            .iter()
            .find(|(k, _)| *k == "Set-Cookie")
            .expect("Set-Cookie present");
        assert_eq!(
            value.as_str(),
            "sid=a=b%3B%20Admin=1; SameSite=Lax; Path=/; HttpOnly; Secure"
        );

        // benign value unchanged
        let res = Response::ok().set_cookie("sid", "abc-123~x", false, false, "Strict", Some(60));
        let (_, value) = res
            .headers
            .iter()
            .find(|(k, _)| *k == "Set-Cookie")
            .unwrap();
        assert_eq!(
            value.as_str(),
            "sid=abc-123~x; SameSite=Strict; Path=/; Max-Age=60"
        );

        // attribute-position injection through same_site is neutralised
        let res = Response::ok().set_cookie("sid", "v", false, false, "Lax; Admin=1", None);
        let (_, value) = res
            .headers
            .iter()
            .find(|(k, _)| *k == "Set-Cookie")
            .unwrap();
        assert!(
            !value.contains("Admin"),
            "same_site injected an attribute: {value}"
        );

        // name-position injection is neutralised
        let res = Response::ok().set_cookie("sid=evil; x", "v", false, false, "Lax", None);
        let (_, value) = res
            .headers
            .iter()
            .find(|(k, _)| *k == "Set-Cookie")
            .unwrap();
        assert!(
            !value.contains("; x=") && !value.starts_with("sid=evil;"),
            "name injected attributes: {value}"
        );
    }
}
