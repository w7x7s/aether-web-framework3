#[cfg(test)]
mod tests {
    use aether::prelude::*;
    use aether_core::arena::RequestArena;
    use aether_core::bitset::Bitset64;
    use aether_core::diagnostic::{Diagnostic, ErrorCode, ServerBanner};
    use aether_http::limits::ServerLimits;
    use aether_http::parser::RequestView;
    use aether_http::static_files::{generate_etag, mime_from_extension};
    use bytes::BytesMut;

    use aether_http::method::Method;
    use aether_router::trie::{MatchResult, RadixRouter};
    use aether_validation::validator::Email;


    #[test]
    fn test_error_codes_and_diagnostics() {
        let diag = Diagnostic::new(ErrorCode::E1001ServerStartupFailed, "Address already in use")
            .with_context("Address", "0.0.0.0:3000")
            .with_hint("Port is occupied.")
            .with_suggestion("Change port via AETHER_PORT=3001");

        let rendered = diag.format_terminal(false);
        assert!(rendered.contains("ERROR [E1001]"));
        assert!(rendered.contains("Address: 0.0.0.0:3000"));
        assert!(rendered.contains("Change port via AETHER_PORT=3001"));
    }

    #[test]
    fn test_server_banner() {
        let banner = ServerBanner {
            env: "development",
            address: "http://127.0.0.1:3000".to_string(),
            routes_count: 12,
            workers: 8,
        };
        let text = banner.render();
        assert!(text.contains("AETHER WEB FRAMEWORK"));
        assert!(text.contains("http://127.0.0.1:3000"));
    }

    #[test]
    fn test_static_mime_and_etag() {
        assert_eq!(mime_from_extension("html"), "text/html; charset=utf-8");
        assert_eq!(mime_from_extension("css"), "text/css; charset=utf-8");
        assert_eq!(mime_from_extension("js"), "application/javascript; charset=utf-8");
        assert_eq!(mime_from_extension("png"), "image/png");

        let data = b"console.log('hello aether');";
        let etag1 = generate_etag(data);
        let etag2 = generate_etag(data);
        assert_eq!(etag1, etag2);
        // Strong ETag: quoted, no W/ prefix, and the quoted-pair syntax is valid.
        assert!(etag1.starts_with('"') && etag1.ends_with('"'));
        assert!(!etag1.starts_with("W/"));
        assert!(etag1.len() > 2);
    }

    #[test]
    fn test_request_arena_zero_copy() {
        let mut arena = RequestArena::with_capacity(4096);
        let s = arena.alloc_slice(b"Zero-Copy Slice").unwrap();
        assert_eq!(s, b"Zero-Copy Slice");
        arena.reset();
        assert_eq!(arena.bytes_used(), 0);
    }

    #[test]
    fn test_64bit_bitset() {
        let mut bitset = Bitset64::new(0);
        bitset = bitset.set(0); // GET
        bitset = bitset.set(1); // POST
        assert!(bitset.contains(0));
        assert!(bitset.contains(1));
        assert!(!bitset.contains(2)); // PUT
    }

    #[test]
    fn test_app_state_container() {
        struct DbConfig {
            pool_size: u32,
        }

        let mut container = AppStateContainer::new();
        container.insert(DbConfig { pool_size: 50 });

        let retrieved = container.get::<DbConfig>().expect("DbConfig must exist");
        assert_eq!(retrieved.pool_size, 50);
    }

    #[test]
    fn test_http_request_parsing_and_query_split() {
        let raw_req = b"GET /api/v1/search?q=rust&limit=10 HTTP/1.1\r\nHost: localhost:3000\r\nCookie: session_id=xyz987\r\n\r\n";
        let limits = ServerLimits::default();
        let parsed = RequestView::parse(raw_req, &limits).unwrap();
        assert!(parsed.is_some());

        let (req, consumed) = parsed.unwrap();
        assert_eq!(consumed, raw_req.len());
        assert_eq!(req.path, "/api/v1/search");
        assert_eq!(req.query_str, Some("q=rust&limit=10"));
        assert_eq!(req.method, Method::Get);
    }

    #[test]
    fn test_response_headers_and_cookies() {
        let res = Response::ok()
            .set_cookie("auth_token", "secret123", true, true, "Strict", Some(3600))
            .header("X-Custom", "AetherEngine");

        let mut out = BytesMut::new();
        res.write_http1_to(&mut out);
        let out_str = std::str::from_utf8(&out).unwrap();

        assert!(out_str.contains("HTTP/1.1 200 OK"));
        assert!(out_str.contains("Set-Cookie: auth_token=secret123; SameSite=Strict; Path=/; HttpOnly; Secure; Max-Age=3600"));
        assert!(out_str.contains("X-Custom: AetherEngine"));
    }

    #[test]
    fn test_full_request_lifecycle() {
        let raw_http = b"POST /api/v1/users/42/activate HTTP/1.1\r\n\
                         Host: api.example.com\r\n\
                         Content-Length: 26\r\n\
                         Content-Type: application/json\r\n\
                         \r\n\
                         {\"email\":\"user@domain.com\"}";

        let limits = ServerLimits::default();
        let (req, consumed) = RequestView::parse(raw_http, &limits)
            .expect("Valid request")
            .expect("Complete request");

        assert!(consumed > 0);
        assert_eq!(req.method, Method::Post);
        assert_eq!(req.path, "/api/v1/users/42/activate");

        // Radix Routing
        let mut router = RadixRouter::new();
        router.add_route(Method::Post, "/api/v1/users/{id}/activate", 999);

        match router.find(req.method, req.path) {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 999);
                assert_eq!(params.len(), 1);
                assert_eq!(params[0].key, "id");
                assert_eq!(params[0].value, "42");
            }
            _ => panic!("Route matching failed"),
        }

        // Validation
        let email = Email::try_new("user@domain.com".to_string()).expect("Email valid");
        assert_eq!(email.as_str(), "user@domain.com");
    }

    #[test]
    fn test_pipelined_requests_framing() {
        let limits = ServerLimits::default();
        let req1 = b"GET /first HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let req2 = b"POST /second HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\n\r\nworld";

        let mut combined = Vec::new();
        combined.extend_from_slice(req1);
        combined.extend_from_slice(req2);

        // Parse Request 1
        let (r1, consumed1) = RequestView::parse(&combined, &limits).unwrap().unwrap();
        assert_eq!(r1.path, "/first");
        assert_eq!(consumed1, req1.len());

        // Parse Request 2 from remaining slice
        let remaining = &combined[consumed1..];
        let (r2, consumed2) = RequestView::parse(remaining, &limits).unwrap().unwrap();
        assert_eq!(r2.path, "/second");
        assert_eq!(r2.body, b"world");
        assert_eq!(consumed2, req2.len());
    }

    #[test]
    fn test_fragmented_tcp_read_handling() {
        let limits = ServerLimits::default();
        let full = b"POST /submit HTTP/1.1\r\nHost: example.com\r\nContent-Length: 10\r\n\r\n0123456789";

        // Partial header
        let part1 = &full[..15];
        assert!(matches!(RequestView::parse(part1, &limits).unwrap(), None));

        // Header complete, partial body
        let part2 = &full[..full.len() - 3];
        assert!(matches!(RequestView::parse(part2, &limits).unwrap(), None));

        // Complete request
        let (req, consumed) = RequestView::parse(full, &limits).unwrap().unwrap();
        assert_eq!(consumed, full.len());
        assert_eq!(req.body, b"0123456789");
    }

    #[test]
    fn test_405_method_not_allowed_with_allow_header() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/api/users", 1);
        router.add_route(Method::Post, "/api/users", 2);

        // Try DELETE on a route that only supports GET and POST
        match router.find(Method::Delete, "/api/users") {
            MatchResult::MethodNotAllowed { allowed_bitmask } => {
                let allow_header = RadixRouter::bitmask_to_allow_header(allowed_bitmask);
                assert!(allow_header.contains("GET"));
                assert!(allow_header.contains("POST"));
                assert!(!allow_header.contains("DELETE"));
            }
            _ => panic!("Expected 405 Method Not Allowed"),
        }
    }
}
