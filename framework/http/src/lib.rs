//! # Aether HTTP Core Crate

pub mod header;
pub mod limits;
pub mod method;
pub mod multipart;
pub mod parser;
pub mod response;
pub mod static_files;
pub mod url;

pub use header::{HeaderMap, HeaderView, standard_headers};
pub use limits::ServerLimits;
pub use method::Method;
pub use multipart::{MultipartError, MultipartForm, Part};
pub use parser::{parse_request, RequestView};
pub use response::{canonical_reason, status_forbids_body, Response};
pub use static_files::{
    generate_etag, if_range_matches, mime_from_extension, parse_range_header, serve_static,
    serve_static_with_range, RangeDecision,
};
pub use url::{form_decode, percent_decode, percent_decode_cow};
