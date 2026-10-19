//! # HTTP Security Limits (RFC9112 & DoS Protection)

use std::time::Duration;

#[derive(Clone, Debug)]
pub struct ServerLimits {
    pub max_header_bytes: usize,
    pub max_body_bytes: usize,
    pub max_headers_count: usize,
    pub max_connections: usize,
    pub read_timeout: Duration,
    pub write_timeout: Duration,
    pub keep_alive_timeout: Duration,
    pub request_timeout: Duration,
}

impl Default for ServerLimits {
    fn default() -> Self {
        Self {
            max_header_bytes: 8 * 1024,          // 8 KB max header budget
            max_body_bytes: 2 * 1024 * 1024,     // 2 MB max body
            max_headers_count: 64,               // 64 max headers
            max_connections: 10_000,             // 10k concurrent connections limit
            read_timeout: Duration::from_secs(5), // Slowloris mitigation
            write_timeout: Duration::from_secs(10),
            keep_alive_timeout: Duration::from_secs(60),
            request_timeout: Duration::from_secs(30),
        }
    }
}
