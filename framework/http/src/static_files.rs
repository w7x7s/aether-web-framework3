//! # Aether Static Files Engine
//!
//! Zero-Copy static asset resolution with automatic MIME type detection,
//! strong ETag hashing, Cache-Control headers, conditional HTTP 304 responses,
//! and hardened path traversal protection.

use crate::response::Response;
use bytes::Bytes;
use std::path::{Component, Path, PathBuf};

/// Determines MIME type from file extension without external runtime dependencies.
pub fn mime_from_extension(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "wasm" => "application/wasm",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "application/xml; charset=utf-8",
        "pdf" => "application/pdf",
        "mp4" => "video/mp4",
        _ => "application/octet-stream",
    }
}

/// Computes a strong ETag from a content hash and length.
///
/// The result is a syntactically valid entity-tag per RFC 9110 §8.8.3: `"<len>-<hash>"`.
pub fn generate_etag(data: &[u8]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in data {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("\"{:x}-{:x}\"", data.len(), hash)
}

/// Weak comparison of an `If-None-Match` field against a generated ETag (RFC 9110 §13.1.2).
/// The list form and `*` are supported; `W/` prefixes are ignored on both sides.
fn etag_matches(if_none_match: &str, etag: &str) -> bool {
    let target = etag.strip_prefix("W/").unwrap_or(etag);
    if_none_match.split(',').any(|candidate| {
        let candidate = candidate.trim();
        if candidate == "*" {
            return true;
        }
        candidate.strip_prefix("W/").unwrap_or(candidate) == target
    })
}

/// Securely resolves a relative file path against a root directory, strictly preventing
/// path traversal attacks (`../`, `%2e%2e/`, Windows backslash escapes, null bytes, symlink escapes).
pub fn safe_resolve_path(root_dir: &Path, request_path: &str) -> Result<PathBuf, ()> {
    // Reject null bytes immediately
    if request_path.contains('\0') {
        return Err(());
    }

    if request_path.contains('\\') {
        return Err(());
    }
    let lower = request_path.to_ascii_lowercase();
    if lower.contains("%2e") || lower.contains("%2f") || lower.contains("%5c") {
        return Err(());
    }

    // Reject absolute paths (starting with / on Unix or drive letters on Windows)
    if request_path.starts_with('/') || (request_path.len() >= 2 && request_path.as_bytes()[1] == b':') {
        return Err(());
    }

    let normalized = request_path.trim_start_matches('/');
    let target = Path::new(normalized);

    // Ensure no ParentDir (`..`) or Prefix/RootDir components exist
    for comp in target.components() {
        match comp {
            Component::Normal(_) | Component::CurDir => continue,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return Err(()),
        }
    }

    // Windows interprets `name:stream` as an alternate data stream and silently maps
    // reserved device names (CON, NUL, COM1, ...) to devices even under a directory.
    // Neither may be reachable through a static-file route.
    #[cfg(windows)]
    for comp in target.components() {
        if let Component::Normal(name) = comp {
            let name = name.to_string_lossy();
            if name.contains(':') {
                return Err(());
            }
            if name.ends_with('.') || name.ends_with(' ') {
                return Err(());
            }
            let stem = name.split('.').next().unwrap_or("");
            let upper = stem.to_ascii_uppercase();
            let reserved = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                || (upper.len() == 4
                    && (upper.starts_with("COM") || upper.starts_with("LPT"))
                    && upper.as_bytes()[3].is_ascii_digit()
                    && upper.as_bytes()[3] != b'0');
            if reserved {
                return Err(());
            }
        }
    }

    let full_path = root_dir.join(target);

    // Canonicalize if file exists to verify it stays strictly inside root_dir, and
    // return the *canonical* path rather than the lexical join: the caller then opens
    // exactly the path that passed the containment test. Discarding the canonicalised
    // value (the previous behaviour) meant a symlink re-targeted between resolution and
    // open could redirect the validated-but-different path outside the root.
    let serve_path = match (root_dir.canonicalize(), full_path.canonicalize()) {
        (Ok(canonical_root), Ok(canonical_target)) => {
            if !canonical_target.starts_with(&canonical_root) {
                return Err(());
            }
            canonical_target
        }
        // Not on disk yet: containment cannot be checked against a real inode, and the
        // caller's own open will fail with NotFound.
        _ => full_path,
    };

    Ok(serve_path)
}

/// Decision for a `Range` request field (RFC 9110 §14.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeDecision {
    /// No usable range: serve the full representation.
    Ignore,
    /// A single satisfiable byte range, inclusive.
    Satisfiable { start: u64, end: u64 },
    /// Syntactically valid but outside the representation: answer 416.
    Unsatisfiable,
}

/// Parse a single-range `Range` header field.
///
/// Multiple ranges are deliberately ignored rather than answered with a
/// `multipart/byteranges` body: RFC 9110 permits ignoring `Range`, and a bounded single-range
/// implementation avoids a second, subtly different body framer inside static serving.
pub fn parse_range_header(value: &str, content_len: u64) -> RangeDecision {
    let value = value.trim();
    let Some((unit, spec)) = value.split_once('=') else {
        return RangeDecision::Ignore;
    };
    if !unit.trim().eq_ignore_ascii_case("bytes") {
        return RangeDecision::Ignore;
    }
    let spec = spec.trim();
    if spec.is_empty() || spec.contains(',') {
        return RangeDecision::Ignore;
    }
    let Some((first, last)) = spec.split_once('-') else {
        return RangeDecision::Ignore;
    };
    if last.contains('-') {
        return RangeDecision::Ignore;
    }

    if first.trim().is_empty() {
        // Suffix range: `bytes=-N` means the final N bytes.
        let Some(suffix) = parse_digits(last) else {
            return RangeDecision::Ignore;
        };
        if suffix == 0 || content_len == 0 {
            return RangeDecision::Unsatisfiable;
        }
        let suffix = suffix.min(content_len);
        return RangeDecision::Satisfiable {
            start: content_len - suffix,
            end: content_len - 1,
        };
    }

    let Some(start) = parse_digits(first) else {
        return RangeDecision::Ignore;
    };
    if start >= content_len {
        return RangeDecision::Unsatisfiable;
    }
    let end = if last.trim().is_empty() {
        content_len - 1
    } else {
        let Some(end) = parse_digits(last) else {
            return RangeDecision::Ignore;
        };
        if end < start {
            return RangeDecision::Ignore;
        }
        end.min(content_len - 1)
    };

    RangeDecision::Satisfiable { start, end }
}

fn parse_digits(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse::<u64>().ok()
}

/// Strong `If-Range` comparison (RFC 9110 §13.1.5).
///
/// Only a strong entity-tag can match. Date validators are not supported because static
/// responses currently expose no `Last-Modified`; an unmatched value simply disables the
/// range, which is always a correct fallback (the client receives the full representation).
pub fn if_range_matches(value: &str, etag: &str) -> bool {
    let value = value.trim();
    !value.starts_with("W/") && value == etag
}

/// Builds an optimized Response for static files with caching.
pub fn serve_static(
    filename: &str,
    content: Bytes,
    if_none_match: Option<&str>,
    max_age_secs: u32,
) -> Response {
    serve_static_with_range(filename, content, if_none_match, None, None, max_age_secs)
}

/// Static-file response with conditional and single-range support.
pub fn serve_static_with_range(
    filename: &str,
    content: Bytes,
    if_none_match: Option<&str>,
    if_range: Option<&str>,
    range: Option<&str>,
    max_age_secs: u32,
) -> Response {
    let ext = filename.rsplit('.').next().unwrap_or("");
    let mime = mime_from_extension(ext);
    let etag = generate_etag(&content);
    let cache_control = format!("public, max-age={}", max_age_secs);

    // Conditional GET: 304 Not Modified check (weak comparison).
    if let Some(req_etag) = if_none_match {
        if etag_matches(req_etag, &etag) {
            let mut not_modified = Response::status(304, "Not Modified");
            not_modified.headers.push(("ETag", etag));
            not_modified.headers.push(("Cache-Control", cache_control));
            not_modified.headers.push(("Accept-Ranges", "bytes".to_string()));
            return not_modified;
        }
    }

    let range_decision = match range {
        Some(value) if if_range.is_none_or(|candidate| if_range_matches(candidate, &etag)) => {
            parse_range_header(value, content.len() as u64)
        }
        _ => RangeDecision::Ignore,
    };

    match range_decision {
        RangeDecision::Unsatisfiable => {
            let mut res = Response::status(416, "Range Not Satisfiable");
            res.headers.push(("Content-Range", format!("bytes */{}", content.len())));
            res.headers.push(("ETag", etag));
            res.headers.push(("Cache-Control", cache_control));
            res.headers.push(("Accept-Ranges", "bytes".to_string()));
            res
        }
        RangeDecision::Satisfiable { start, end } => {
            let len = content.len() as u64;
            let body = content.slice(start as usize..=end as usize);
            let mut res = Response::status(206, "Partial Content");
            res.headers.push(("Content-Type", mime.to_string()));
            res.headers.push(("Content-Range", format!("bytes {}-{}/{}", start, end, len)));
            res.headers.push(("ETag", etag));
            res.headers.push(("Cache-Control", cache_control));
            res.headers.push(("Accept-Ranges", "bytes".to_string()));
            res.body = body;
            res
        }
        RangeDecision::Ignore => {
            let mut res = Response::ok();
            res.headers.push(("Content-Type", mime.to_string()));
            res.headers.push(("Content-Length", content.len().to_string()));
            res.headers.push(("ETag", etag));
            res.headers.push(("Cache-Control", cache_control));
            res.headers.push(("Accept-Ranges", "bytes".to_string()));
            res.body = content;
            res
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_traversal_prevention() {
        let root = Path::new("public");
        assert!(safe_resolve_path(root, "app.js").is_ok());
        assert!(safe_resolve_path(root, "css/style.css").is_ok());

        // Attacks
        assert!(safe_resolve_path(root, "../etc/passwd").is_err());
        assert!(safe_resolve_path(root, "sub/../../secret").is_err());
        assert!(safe_resolve_path(root, "/etc/shadow").is_err());
        assert!(safe_resolve_path(root, "foo\0bar.txt").is_err());
        assert!(safe_resolve_path(root, "css\\..\\secret").is_err());
        assert!(safe_resolve_path(root, "C:\\Windows\\win.ini").is_err());
        assert!(safe_resolve_path(root, "a/%2e%2e/b").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn test_windows_device_and_stream_paths_are_rejected() {
        let root = Path::new("public");
        for hostile in ["CON", "nul", "COM1", "lpt9.txt", "app.js:hidden", "name.", "name "] {
            assert!(
                safe_resolve_path(root, hostile).is_err(),
                "{hostile} must not resolve under the static root"
            );
        }
        // "COM0" is not a device; a normal file must remain legal.
        assert!(safe_resolve_path(root, "com0.txt").is_ok());
    }

    #[test]
    fn test_etag_is_a_valid_quoted_entity_tag() {
        let etag = generate_etag(b"hello");
        assert!(etag.starts_with('"') && etag.ends_with('"'), "bad etag {etag}");
        assert!(!etag.contains("W/"), "strong ETag must not embed a weak prefix: {etag}");
        assert_eq!(etag, generate_etag(b"hello"));
        assert_ne!(etag, generate_etag(b"hellO"));
    }

    #[test]
    fn test_range_header_decisions() {
        use RangeDecision::*;
        let len = 10;
        assert_eq!(parse_range_header("bytes=0-4", len), Satisfiable { start: 0, end: 4 });
        assert_eq!(parse_range_header("bytes=5-", len), Satisfiable { start: 5, end: 9 });
        assert_eq!(parse_range_header("bytes=-3", len), Satisfiable { start: 7, end: 9 });
        assert_eq!(parse_range_header("bytes=0-999", len), Satisfiable { start: 0, end: 9 });
        assert_eq!(parse_range_header("Bytes=0-0", len), Satisfiable { start: 0, end: 0 });
        assert_eq!(parse_range_header("bytes=-99", len), Satisfiable { start: 0, end: 9 });
        assert_eq!(parse_range_header("bytes=10-", len), Unsatisfiable);
        assert_eq!(parse_range_header("bytes=20-30", len), Unsatisfiable);
        assert_eq!(parse_range_header("bytes=-0", len), Unsatisfiable);
        assert_eq!(parse_range_header("bytes=0-", 0), Unsatisfiable);
        assert_eq!(parse_range_header("bytes=0-4", 0), Unsatisfiable);

        // Malformed or unsupported fields are ignored, not treated as 416.
        for header in [
            "",
            "items=0-1",
            "bytes=",
            "bytes=abc-def",
            "bytes=+1-2",
            "bytes=5-3",
            "bytes=0-1,3-4",
            "bytes=0-1-2",
            "bytes=18446744073709551616-",
        ] {
            assert_eq!(parse_range_header(header, len), Ignore, "header {header}");
        }
    }

    #[test]
    fn test_range_response_headers_and_body() {
        let content = Bytes::from_static(b"0123456789");
        let res = serve_static_with_range("a.txt", content.clone(), None, None, Some("bytes=2-5"), 60);
        assert_eq!(res.status, 206);
        assert_eq!(res.body, Bytes::from_static(b"2345"));
        let header = |name: &str| {
            res.headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(header("Content-Range"), Some("bytes 2-5/10"));
        assert_eq!(header("Accept-Ranges"), Some("bytes"));
        assert_eq!(header("Content-Type"), Some("text/plain; charset=utf-8"));
    }

    #[test]
    fn test_unsatisfiable_range_is_416_without_body() {
        let content = Bytes::from_static(b"0123456789");
        let res = serve_static_with_range("a.txt", content, None, None, Some("bytes=99-"), 60);
        assert_eq!(res.status, 416);
        assert!(res.body.is_empty());
        assert_eq!(
            res.headers.iter().find(|(k, _)| *k == "Content-Range").map(|(_, v)| v.as_str()),
            Some("bytes */10")
        );
    }

    #[test]
    fn test_if_range_mismatch_serves_full_representation() {
        let content = Bytes::from_static(b"0123456789");
        let etag = generate_etag(&content);

        let full = serve_static_with_range(
            "a.txt",
            content.clone(),
            None,
            Some("\"stale\""),
            Some("bytes=0-1"),
            60,
        );
        assert_eq!(full.status, 200);
        assert_eq!(full.body.len(), 10);

        let partial = serve_static_with_range(
            "a.txt",
            content.clone(),
            None,
            Some(&etag),
            Some("bytes=0-1"),
            60,
        );
        assert_eq!(partial.status, 206);
        assert_eq!(partial.body, Bytes::from_static(b"01"));

        // Weak validators must never satisfy If-Range.
        let weak = serve_static_with_range(
            "a.txt",
            content,
            None,
            Some(&format!("W/{etag}")),
            Some("bytes=0-1"),
            60,
        );
        assert_eq!(weak.status, 200);
    }

    #[test]
    fn test_conditional_get_304_with_weak_and_list_matches() {
        let content = Bytes::from_static(b"body");
        let etag = generate_etag(&content);

        for header in [
            etag.clone(),
            format!("W/{etag}"),
            format!("\"other\", {etag}"),
            "*".to_string(),
        ] {
            let res = serve_static("a.css", content.clone(), Some(&header), 60);
            assert_eq!(res.status, 304, "header {header} should match");
            assert_eq!(res.body.len(), 0);
            assert_eq!(
                res.headers.iter().find(|(k, _)| *k == "ETag").map(|(_, v)| v.as_str()),
                Some(etag.as_str())
            );
        }

        let res = serve_static("a.css", content.clone(), Some("\"nope\""), 60);
        assert_eq!(res.status, 200);
        assert_eq!(res.body, content);
        assert_eq!(
            res.headers.iter().find(|(k, _)| *k == "Cache-Control").map(|(_, v)| v.as_str()),
            Some("public, max-age=60")
        );
    }
}
