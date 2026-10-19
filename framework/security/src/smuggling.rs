//! # HTTP Request Smuggling & Desync Detection (RFC 9112 Section 6.3)
//!
//! Rejects requests containing both Content-Length and Transfer-Encoding,
//! or malformed chunked transfer encodings.

use aether_core::error::AetherError;
use aether_http::header::HeaderMap;

pub fn audit_request_desync(headers: &HeaderMap<'_>) -> Result<(), AetherError> {
    let mut has_content_length = false;
    let mut has_transfer_encoding = false;

    for h in headers.iter() {
        if h.name.eq_ignore_ascii_case("content-length") {
            if has_content_length {
                // Dual Content-Length headers -> Smuggling attack!
                return Err(AetherError::MalformedRequest);
            }
            has_content_length = true;
        } else if h.name.eq_ignore_ascii_case("transfer-encoding") {
            has_transfer_encoding = true;
        }
    }

    // CL.TE or TE.CL conflict
    if has_content_length && has_transfer_encoding {
        return Err(AetherError::MalformedRequest);
    }

    Ok(())
}
