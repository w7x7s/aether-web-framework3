//! # Compression Middleware (Gzip & Brotli)
//!
//! Automatically compresses response bodies based on Accept-Encoding header.

use aether_http::response::Response;
use bytes::Bytes;

#[derive(Clone, Debug)]
pub struct CompressionConfig {
    pub enable_gzip: bool,
    pub enable_brotli: bool,
    pub min_compression_size: usize,
    pub compression_level: u32,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            enable_gzip: true,
            enable_brotli: false, // Brotli is slower, disable by default
            min_compression_size: 1024, // Only compress responses >= 1KB
            compression_level: 6, // Default balance between speed and compression
        }
    }
}

pub struct CompressionMiddleware {
    config: CompressionConfig,
}

impl CompressionMiddleware {
    pub fn new(config: CompressionConfig) -> Self {
        Self { config }
    }

    pub fn default() -> Self {
        Self::new(CompressionConfig::default())
    }

    /// Apply compression to response based on Accept-Encoding header
    pub fn apply(&self, response: &mut Response, accept_encoding: Option<&str>) {
        if aether_http::response::status_forbids_body(response.status) {
            return;
        }
        if response.body.len() < self.config.min_compression_size {
            return;
        }
        if response
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("content-encoding"))
        {
            return;
        }
        if is_precompressed_content_type(response) {
            return;
        }
        // RFC 9111 §5.6.5: a representation carrying `Cache-Control: no-transform` must
        // not have its body altered, and compression is a transform. Handlers use the
        // directive deliberately (byte-exact payloads, streaming ranges, signatures), so
        // it must be honoured before any Vary bookkeeping.
        if response.headers.iter().any(|(k, v)| {
            k.eq_ignore_ascii_case("cache-control")
                && v.split(',')
                    .map(|t| t.trim())
                    .any(|t| t.eq_ignore_ascii_case("no-transform"))
        }) {
            return;
        }

        // The response varies on Accept-Encoding whether or not an encoding is ultimately
        // selected; without Vary a shared cache can hand a gzip body to a client that never
        // asked for one (RFC 9111 §4.1).
        add_vary(response);

        let Some(encoding) = select_encoding(
            accept_encoding.unwrap_or(""),
            self.config.enable_gzip,
            self.config.enable_brotli,
        ) else {
            return;
        };

        let original_len = response.body.len();
        let compressed = match encoding {
            "br" => self.compress_brotli(&response.body),
            _ => self.compress_gzip(&response.body),
        };

        // Incompressible payloads can come out larger than they went in; keep the original
        // rather than paying bytes for nothing.
        if let Ok(body) = compressed {
            if body.len() < original_len {
                response.body = body;
                response.add_header("Content-Encoding", encoding);
                self.update_content_length(response);
            }
        }
    }

    fn compress_gzip(&self, data: &[u8]) -> Result<Bytes, std::io::Error> {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;

        let mut encoder = GzEncoder::new(Vec::new(), Compression::new(self.config.compression_level));
        encoder.write_all(data)?;
        Ok(Bytes::from(encoder.finish()?))
    }

    fn compress_brotli(&self, data: &[u8]) -> Result<Bytes, std::io::Error> {
        use brotli::enc::writer::CompressorWriter;
        use std::io::Write;

        let mut writer = CompressorWriter::new(
            Vec::new(),
            4096, // buffer size
            self.config.compression_level,
            22,   // lg_window_size
        );
        writer.write_all(data)?;
        Ok(Bytes::from(writer.into_inner()))
    }

    fn update_content_length(&self, response: &mut Response) {
        // Remove old Content-Length and add new one
        response.headers.retain(|(k, _)| !k.eq_ignore_ascii_case("content-length"));
        response.add_header("Content-Length", response.body.len().to_string());
    }
}

/// Media types whose payload is already entropy-coded. Recompressing them burns CPU and
/// usually produces a *larger* body.
const PRECOMPRESSED_MIME: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/webp",
    "image/avif",
    "image/x-icon",
    "image/vnd.microsoft.icon",
    "font/woff",
    "font/woff2",
    "application/vnd.ms-fontobject",
    "application/zip",
    "application/gzip",
    "application/x-gzip",
    "application/x-bzip2",
    "application/x-7z-compressed",
    "application/x-rar-compressed",
    "application/x-xz",
];

/// Media-type prefixes treated as already compressed.
const PRECOMPRESSED_PREFIX: &[&str] = &["video/", "audio/"];

fn is_precompressed_content_type(response: &Response) -> bool {
    let Some((_, ct)) = response
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
    else {
        return false;
    };
    let mime = ct.split(';').next().unwrap_or("").trim();
    PRECOMPRESSED_MIME
        .iter()
        .any(|p| mime.eq_ignore_ascii_case(p))
        || PRECOMPRESSED_PREFIX.iter().any(|p| {
            mime.len() >= p.len() && mime[..p.len()].eq_ignore_ascii_case(p)
        })
}

fn add_vary(response: &mut Response) {
    if let Some((_, existing)) = response
        .headers
        .iter_mut()
        .find(|(k, _)| k.eq_ignore_ascii_case("vary"))
    {
        let already = existing
            .split(',')
            .any(|t| t.trim().eq_ignore_ascii_case("accept-encoding"));
        if !already {
            existing.push_str(", Accept-Encoding");
        }
    } else {
        response.add_header("Vary", "Accept-Encoding");
    }
}

/// Chooses a response encoding from an `Accept-Encoding` header, honouring q-values.
///
/// A substring test is not sufficient here: `Accept-Encoding: gzbrip` contains `br`, and
/// `gzip;q=0` explicitly forbids gzip even though the token `gzip` is present. Per RFC 9110
/// §12.5.1 a q-value of 0 means "not acceptable", and an unlisted coding is only acceptable
/// when the wildcard `*` is present. Ties prefer Brotli, which has the better ratio.
pub fn select_encoding(accept: &str, gzip_enabled: bool, brotli_enabled: bool) -> Option<&'static str> {
    let mut gzip_q: Option<f32> = None;
    let mut br_q: Option<f32> = None;
    let mut wildcard_q: Option<f32> = None;

    for item in accept.split(',') {
        let mut parts = item.split(';');
        let Some(raw) = parts.next() else { continue };
        let coding = raw.trim();
        if coding.is_empty() {
            continue;
        }

        let mut q: f32 = 1.0;
        for param in parts {
            if let Some(v) = param.trim().strip_prefix("q=") {
                // An unparsable q-value is treated as "not acceptable" rather than trusted.
                q = v.trim().parse::<f32>().unwrap_or(0.0).clamp(0.0, 1.0);
            }
        }

        if coding.eq_ignore_ascii_case("*") {
            wildcard_q = Some(wildcard_q.map_or(q, |old| old.max(q)));
        } else if coding.eq_ignore_ascii_case("gzip") {
            gzip_q = Some(gzip_q.map_or(q, |old| old.max(q)));
        } else if coding.eq_ignore_ascii_case("br") {
            br_q = Some(br_q.map_or(q, |old| old.max(q)));
        }
    }

    // An explicit q (including q=0) overrides the wildcard.
    let resolve = |explicit: Option<f32>| explicit.unwrap_or_else(|| wildcard_q.unwrap_or(0.0));
    let gzip_score = if gzip_enabled { resolve(gzip_q) } else { 0.0 };
    let br_score = if brotli_enabled { resolve(br_q) } else { 0.0 };

    if br_score > 0.0 && br_score >= gzip_score {
        Some("br")
    } else if gzip_score > 0.0 {
        Some("gzip")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compression_config_default() {
        let config = CompressionConfig::default();
        assert!(config.enable_gzip);
        assert!(!config.enable_brotli);
        assert_eq!(config.min_compression_size, 1024);
        assert_eq!(config.compression_level, 6);
    }

    #[test]
    fn test_skip_small_responses() {
        let middleware = CompressionMiddleware::default();
        let mut response = Response::text("small");
        assert_eq!(response.body.len(), 5);
        middleware.apply(&mut response, Some("gzip"));
        // Should not be compressed (too small)
        assert!(response.headers.iter().all(|(k, _)| !k.eq_ignore_ascii_case("content-encoding")));
    }

    #[test]
    fn test_gzip_compression() {
        let middleware = CompressionMiddleware::default();
        let large_body = "a".repeat(2000);
        let mut response = Response::text_dynamic(large_body.clone());
        let original_len = response.body.len();

        middleware.apply(&mut response, Some("gzip"));

        // Should be compressed
        assert!(response.headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("content-encoding") && v == "gzip"));
        // Compressed data should be smaller
        assert!(response.body.len() < original_len);
    }

    #[test]
    fn test_no_compression_without_accept() {
        let middleware = CompressionMiddleware::default();
        let large_body = "a".repeat(2000);
        let mut response = Response::text_dynamic(large_body.clone());

        middleware.apply(&mut response, None);

        // Should not be compressed
        assert!(response.headers.iter().all(|(k, _)| !k.eq_ignore_ascii_case("content-encoding")));
    }

    /// A cache must be told the body depends on Accept-Encoding, or it can serve a gzip body
    /// to a client that never advertised gzip support.
    #[test]
    fn test_vary_accept_encoding_is_added() {
        let middleware = CompressionMiddleware::default();
        let mut response = Response::text_dynamic("a".repeat(2000));
        middleware.apply(&mut response, Some("gzip"));

        let vary = response
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("vary"))
            .map(|(_, v)| v.as_str());
        assert_eq!(vary, Some("Accept-Encoding"));
    }

    #[test]
    fn test_vary_is_added_even_when_client_declines_compression() {
        let middleware = CompressionMiddleware::default();
        let mut response = Response::text_dynamic("a".repeat(2000));
        middleware.apply(&mut response, Some("identity"));

        assert!(response
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("vary") && v.contains("Accept-Encoding")));
        assert!(response
            .headers
            .iter()
            .all(|(k, _)| !k.eq_ignore_ascii_case("content-encoding")));
    }

    #[test]
    fn test_vary_is_not_duplicated() {
        let middleware = CompressionMiddleware::default();
        let mut response = Response::text_dynamic("a".repeat(2000));
        response.add_header("Vary", "Accept-Language");

        middleware.apply(&mut response, Some("gzip"));
        middleware.apply(&mut response, Some("gzip"));

        let count = response
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("vary"))
            .count();
        assert_eq!(count, 1);
        let v = response
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("vary"))
            .unwrap()
            .1
            .clone();
        assert!(v.contains("Accept-Language"), "existing Vary lost: {}", v);
        assert_eq!(
            v.to_ascii_lowercase().matches("accept-encoding").count(),
            1,
            "Accept-Encoding duplicated: {}",
            v
        );
    }

    #[test]
    fn test_no_vary_on_bodyless_status_codes() {
        let middleware = CompressionMiddleware::default();
        for status in [204u16, 304] {
            let mut response = Response::status(status, "x");
            response.body = Bytes::from("a".repeat(2000));
            middleware.apply(&mut response, Some("gzip"));
            assert!(
                response.headers.iter().all(|(k, _)| !k.eq_ignore_ascii_case("vary")),
                "{} must not gain Vary",
                status
            );
        }
    }

    /// Regression: `accept.contains("br")` matched any header containing those two bytes in
    /// sequence, and ignored q-values entirely.
    #[test]
    fn test_select_encoding_honours_q_values() {
        // q=0 means "not acceptable" (RFC 9110 §12.5.1).
        assert_eq!(select_encoding("gzip;q=0", true, false), None);
        assert_eq!(select_encoding("gzip;q=0.0", true, false), None);
        assert_eq!(select_encoding("br;q=0, gzip;q=0.5", true, true), Some("gzip"));

        // An explicit q=0 beats a wildcard.
        assert_eq!(select_encoding("*;q=1, gzip;q=0", true, false), None);

        // Highest q wins, regardless of header order.
        assert_eq!(select_encoding("gzip;q=0.5, br;q=0.9", true, true), Some("br"));
        assert_eq!(select_encoding("br;q=0.3, gzip;q=0.8", true, true), Some("gzip"));

        // Ties prefer Brotli (better ratio).
        assert_eq!(select_encoding("gzip, br", true, true), Some("br"));
        assert_eq!(select_encoding("br, gzip", true, true), Some("br"));
    }

    #[test]
    fn test_select_encoding_rejects_substring_false_positives() {
        // "gzbrip" contains the bytes "br" — the old substring test served Brotli here.
        assert_eq!(select_encoding("gzbrip", true, true), None);
        assert_eq!(select_encoding("x-gzip", true, false), None);
        assert_eq!(select_encoding("compress", true, true), None);
        assert_eq!(select_encoding("", true, true), None);
        assert_eq!(select_encoding("identity", true, true), None);
    }

    #[test]
    fn test_select_encoding_wildcard_and_case() {
        assert_eq!(select_encoding("*", true, false), Some("gzip"));
        assert_eq!(select_encoding("*", false, true), Some("br"));
        assert_eq!(select_encoding("*", false, false), None);
        assert_eq!(select_encoding("GZIP", true, false), Some("gzip"));
        assert_eq!(select_encoding("  gzip  ", true, false), Some("gzip"));
        assert_eq!(select_encoding("gzip;q=0.000", true, false), None);
        // Whitespace and empty entries must not confuse the parser.
        assert_eq!(select_encoding(", ,gzip,, ", true, false), Some("gzip"));
    }

    #[test]
    fn test_select_encoding_respects_disabled_codecs() {
        assert_eq!(select_encoding("gzip, br", false, false), None);
        assert_eq!(select_encoding("gzip, br", false, true), Some("br"));
        assert_eq!(select_encoding("gzip, br", true, false), Some("gzip"));
    }

    #[test]
    fn test_select_encoding_malformed_q_does_not_panic() {
        for accept in [
            "gzip;q=",
            "gzip;q",
            "gzip;q=abc",
            "gzip;q=-1",
            "gzip;q=1e999",
            "gzip;q=NaN",
            "gzip;",
            ";",
            "gzip;q=0.5;level=1",
        ] {
            let _ = select_encoding(accept, true, true);
        }
        // A malformed q must not be treated as full-quality acceptance.
        assert_eq!(select_encoding("gzip;q=abc", true, false), None);
        assert_eq!(select_encoding("gzip;q=-1", true, false), None);
    }

    /// Already-compressed media must not be recompressed.
    #[test]
    fn test_precompressed_content_types_are_skipped() {
        let middleware = CompressionMiddleware::default();
        for mime in [
            "image/jpeg",
            "image/png",
            "image/webp",
            "video/mp4",
            "audio/mpeg",
            "font/woff2",
            "application/zip",
            "IMAGE/JPEG",
            "image/jpeg; charset=binary",
        ] {
            let mut response = Response::bytes(Bytes::from("a".repeat(2000)), "application/octet-stream");
            response.headers.retain(|(k, _)| !k.eq_ignore_ascii_case("content-type"));
            response.add_header("Content-Type", mime);

            middleware.apply(&mut response, Some("gzip"));
            assert!(
                response.headers.iter().all(|(k, _)| !k.eq_ignore_ascii_case("content-encoding")),
                "{} should not be recompressed",
                mime
            );
        }

        // Text-like types still compress.
        let mut response = Response::text_dynamic("a".repeat(2000));
        middleware.apply(&mut response, Some("gzip"));
        assert!(response
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("content-encoding") && v == "gzip"));
    }

    /// An incompressible body must not be replaced by a larger "compressed" one.
    #[test]
    fn test_incompressible_body_is_left_alone() {
        let middleware = CompressionMiddleware::default();

        // Pseudorandom bytes do not compress; gzip framing would make them bigger.
        let noise: Vec<u8> = (0..4096u32).map(|i| ((i.wrapping_mul(2654435761) >> 13) & 0xFF) as u8).collect();
        let mut response = Response::bytes(Bytes::from(noise.clone()), "application/octet-stream");

        middleware.apply(&mut response, Some("gzip"));
        assert!(
            response.body.len() <= noise.len(),
            "body grew from {} to {}",
            noise.len(),
            response.body.len()
        );
        if response.body.len() == noise.len() {
            assert!(response
                .headers
                .iter()
                .all(|(k, _)| !k.eq_ignore_ascii_case("content-encoding")));
        }
    }

    /// Brotli must round-trip when enabled.
    #[test]
    fn test_brotli_round_trip() {
        let config = CompressionConfig {
            enable_brotli: true,
            ..CompressionConfig::default()
        };
        let middleware = CompressionMiddleware::new(config);
        let body = "hello aether ".repeat(400);
        let mut response = Response::text_dynamic(body.clone());

        middleware.apply(&mut response, Some("br"));

        assert!(response
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("content-encoding") && v == "br"));
        assert!(response.body.len() < body.len());

        let mut decoded = Vec::new();
        brotli::BrotliDecompress(&mut &response.body[..], &mut decoded).unwrap();
        assert_eq!(decoded, body.as_bytes());
    }

    /// Gzip must round-trip, and Content-Length must match the compressed body.
    #[test]
    fn test_gzip_round_trip_and_content_length() {
        use flate2::read::GzDecoder;
        use std::io::Read;

        let middleware = CompressionMiddleware::default();
        let body = "hello aether ".repeat(400);
        let mut response = Response::text_dynamic(body.clone());
        middleware.apply(&mut response, Some("gzip"));

        let mut decoded = String::new();
        GzDecoder::new(&response.body[..]).read_to_string(&mut decoded).unwrap();
        assert_eq!(decoded, body);

        let cl = response
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .map(|(_, v)| v.clone())
            .collect::<Vec<_>>();
        assert_eq!(cl.len(), 1, "Content-Length duplicated: {:?}", cl);
        assert_eq!(cl[0].parse::<usize>().unwrap(), response.body.len());
    }
}
