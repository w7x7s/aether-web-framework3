//! # Dynamic static-directory serving
//!
//! Maps a catch-all route to files under a developer-supplied root. Every request path is
//! resolved through [`safe_resolve_path`], so traversal, alternate data streams, and
//! Windows device names cannot escape the root. Files are read with `tokio::fs` to keep
//! blocking I/O off the reactor.

use aether_http::static_files::{safe_resolve_path, serve_static_with_range};
use aether_http::response::Response;
use bytes::Bytes;
use std::path::Path;

/// Serve one file from a static root.
///
/// The file is read in full before the response is built, so the ETag remains a content
/// hash and a range request can be answered from the same bytes. Streaming large static
/// files without buffering them is tracked as remaining work.
pub(crate) async fn serve_from_dir(
    root: &Path,
    relative: &str,
    if_none_match: Option<&str>,
    if_range: Option<&str>,
    range: Option<&str>,
    max_age_secs: u32,
) -> Response {
    let Ok(path) = safe_resolve_path(root, relative) else {
        return Response::status(404, "Not Found");
    };

    let Ok(metadata) = tokio::fs::metadata(&path).await else {
        return Response::status(404, "Not Found");
    };
    if !metadata.is_file() {
        // No directory listings: they leak filenames and are a common misconfiguration.
        return Response::status(404, "Not Found");
    }

    let Ok(content) = tokio::fs::read(&path).await else {
        return Response::status(500, "Internal Server Error");
    };

    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(relative);
    serve_static_with_range(
        filename,
        Bytes::from(content),
        if_none_match,
        if_range,
        range,
        max_age_secs,
    )
}
