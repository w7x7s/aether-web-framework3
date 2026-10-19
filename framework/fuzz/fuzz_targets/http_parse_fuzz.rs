//! # AFL / libFuzzer Target for HTTP Request Parser
//!
//! Validates that no malformed network input can cause panics, out-of-bounds reads, or memory corruption.

use aether_http::limits::ServerLimits;
use aether_http::parser::RequestView;

pub fn fuzz_target(data: &[u8]) {
    let limits = ServerLimits::default();
    // Must never panic on any arbitrary byte sequence
    let _ = RequestView::parse(data, &limits);
}

fn main() {
    let sample = b"GET /api/test HTTP/1.1\r\nHost: example.com\r\n\r\n";
    fuzz_target(sample);
}
