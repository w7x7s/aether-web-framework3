# Aether Web Framework

A production-oriented Rust web framework built on Tokio. It ships its own HTTP/1.1
connection engine, a segment radix router, security limits, and a small set of built-in
middleware.

**Status:** HTTP/1.1 only. HTTP/2 and HTTP/3 are **not** implemented. See
[docs/FEATURE_COMPLETENESS_MATRIX.md](docs/FEATURE_COMPLETENESS_MATRIX.md) for a row-by-row
audit against Axum and Actix Web, including everything that is missing.

## What is verified today

- HTTP/1.1 parsing and framing: Content-Length and chunked request bodies, pipelining,
  keep-alive, exact consumed-byte accounting.
- RFC 9112 smuggling defenses: dual `Content-Length`, `Content-Length` + `Transfer-Encoding`,
  unsupported transfer codings, and HTTP/1.0 chunked requests are rejected.
- Security limits: 8 KiB header budget, 64 headers, 2 MiB body, 10 000 connections,
  5 s per-read idle timeout, a total receive deadline (default 30 s) that returns 408 to
  trickling clients, 10 s write timeout, 30 s handler timeout.
- Radix routing with static > parameter > catch-all precedence, percent-decoded parameters,
  correct 404, and 405 with a complete `Allow` header.
- Per-request panic isolation: a panicking handler returns 500 and the server keeps running
  (release profile uses `panic = "unwind"`).
- CORS (allow-list, credentials, preflight, `Vary: Origin`), security headers, and
  gzip/Brotli compression with q-value negotiation.
- Duplicate request header fields are preserved (`header_values`) without changing the legacy
  `header()`/`headers` behavior.
- Multipart/form-data parsing (RFC 7578) with borrowed parts and strict size/boundary limits.
- Dynamic static-file serving (`static_dir`) with strong ETags, conditional GET, byte ranges
  (`206`/`416`, strong `If-Range`), Cache-Control, async disk I/O, and traversal/Windows
  device-name protection.
- Graceful shutdown on Ctrl+C and Unix SIGTERM, plus a programmatic
  `AetherServer::serve_with_shutdown` API for tests and embedders.
- JWT (HS256) validation with algorithm pinning, `exp`/`nbf`/`iat`/`iss`/`aud` checks, and
  constant-time signature verification.
- WebSocket RFC 6455: validated opening handshake, fragmented message assembly, ping/pong,
  close handshake, bounded frame/message sizes, and `app.ws()` routing. Compression and
  subprotocol negotiation are not implemented.
- Typed extractors on the `*_async` route methods: `Path<T>`, `Query<T>`, `Json<T>`,
  `Form<T>`, and `State<T>` for zero to four handler arguments, with typed rejections
  (400/415/500). A handler taking the context directly must now annotate it:
  `|req: RequestContext| async { .. }`.
- User-defined middleware: `app.use_middleware(...)` app-wide and
  `RouteGroup::use_middleware(...)` per group, composed around the handler with a `Next`
  continuation; middleware runs in registration order and may short-circuit.
- An ergonomic `TestClient` for integration tests: starts the app on an ephemeral loopback
  port, sends real HTTP/1.1 requests, and offers request-builder, JSON/form, and graceful
  shutdown helpers.

The regression suite includes 13 checked-in raw-socket end-to-end tests
(`tests/tests/http_e2e.rs`), so these behaviors are exercised by `cargo test --workspace`
rather than only by an external harness.

## Not implemented (selected)

HTTP/2, HTTP/3, streaming request/response bodies, static-file `sendfile`, request-scoped
extensions, a dedicated single-route middleware API, rate limiting, authentication
middleware, TLS termination, and directory listing. This list is not exhaustive; the matrix is.

## Typed extractors

```rust
use aether::prelude::*;
use serde::Deserialize;

#[derive(Deserialize)]
struct CreateUser { name: String }

app.post_async("/users/{id}", |Path(id): Path<u32>, Json(body): Json<CreateUser>| async move {
    format!("created user {id} named {}", body.name)
});
```

Extraction failures return a typed rejection (400 for malformed input, 415 for a wrong
`Content-Type`, 500 for missing state) and the handler never runs.

## User middleware

```rust
app.use_middleware(|req: RequestContext, next: Next| async move {
    if req.header("x-token").is_none() {
        return Response::status(401, "Unauthorized");
    }
    next(req).await
});
```

Middleware applies to routes registered after the `use_middleware` call. Group-scoped
middleware composes with the app-wide stack (app-wide runs first on the request path).

## Testing utilities

```rust
let mut app = AetherApp::new();
app.get("/hello", |_| "hello");

let client = TestClient::start(app).await;
let res = client.get("/hello").send().await;
assert_eq!(res.status, 200);
assert_eq!(res.text(), "hello");
client.shutdown().await;
```

## Quick start

```rust
use aether::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut app = AetherApp::new();
    app.enable_security_headers();

    app.get("/", |_req| Response::text("hello"));
    app.get("/users/{id}", |req| {
        let id = req.param("id").unwrap_or("unknown");
        let body = serde_json::json!({ "id": id }).to_string();
        Response::json(bytes::Bytes::from(body))
    });

    app.listen("0.0.0.0:3000").await
}
```

A `Cargo.toml` for an application:

```toml
[dependencies]
aether = { path = "path/to/aether/framework/aether" }
tokio = { version = "1", features = ["full"] }
bytes = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

The workspace is Rust 2024 edition; the verified toolchain is rustc 1.98.1.

## Configuration

`AetherConfig::from_env()` reads:

| Variable | Default | Meaning |
|---|---|---|
| `AETHER_HOST` | `0.0.0.0` | Bind host (IPv6 literals are bracketed) |
| `AETHER_PORT` | `3000` | Bind port |
| `AETHER_ENV` | `development` | Environment name |
| `AETHER_MAX_BODY_BYTES` | `2097152` | Request body budget |
| `AETHER_MAX_HEADER_BYTES` | `8192` | Request head budget |
| `AETHER_WORKERS` | CPU count | **Parsed and displayed, but not applied** to the Tokio runtime |
| `AETHER_MAX_REQUEST_TIME_SECS` | `30` | Per-handler timeout |
| `AETHER_MAX_RESPONSE_TIME_SECS` | `10` | Response write timeout |

`AetherConfig::listen_addr()` formats IPv6 hosts correctly.

## Static files

```rust
// Serves ./public/** at /files/** with a one-hour Cache-Control.
app.static_dir_with_cache("/files", "./public", 3600);
```

Directories return 404 (no listing). Files are read into memory; there is no `sendfile` or
pre-compressed variant selection. Single and suffix byte ranges are supported with `206`,
`416`, `Content-Range`, and strong `If-Range` handling. `.`/`..` traversal, percent-encoded
traversal, Windows alternate data streams, reserved device names, and trailing dots/spaces are
rejected.

## WebSocket

```rust
app.ws("/ws", |_req, mut socket| async move {
    while let Ok(Some(message)) = socket.recv().await {
        if let WsMessage::Text(text) = message {
            if socket.send_text(&text).await.is_err() {
                break;
            }
        }
    }
});
```

The framework validates the opening handshake and writes `101 Switching Protocols` before the
handler runs; invalid handshakes receive 400, unsupported versions 426 (advertising 13), and
non-GET methods 405. `WebSocket` exposes `recv`, `send`/`send_text`/`send_binary`, `ping`, and
`close`, with default 16 MiB frame/message limits. WebSocket sessions are aborted when the
server shuts down; no close frame is sent during shutdown.

## Security notes

- `enable_security_headers()` is **opt-in** and adds HSTS, `X-Frame-Options: DENY`,
  `X-Content-Type-Options: nosniff`, `Referrer-Policy`, and `X-XSS-Protection: 0`.
  CSP is configurable but off by default because a wrong policy breaks applications.
- TLS is not built in. Terminate TLS at a reverse proxy or in a separate listener.
- The total receive deadline reuses `AETHER_MAX_REQUEST_TIME_SECS`, which is also the handler
  timeout. A legitimately slow large upload therefore shares its budget with handler
  execution; a dedicated header/body timeout setting is still missing.
- Cookies are parsed but not signed or encrypted.
- The server writes every response as HTTP/1.1, including responses to HTTP/1.0 requests.

## Build, test, run

```bash
cd framework
cargo test --workspace          # 258 tests, 0 failures (2026-10-18)
cargo build --release --workspace
cargo run --release --bin basic_api
cargo run --release --bin scheduling_demo   # adaptive scheduling demo (see docs/SCHEDULING_COMPARISON.md)
```

The 258 tests include 13 checked-in raw-socket end-to-end tests in
`tests/tests/http_e2e.rs`, 27 tests for the protocol-independent `aether-scheduler` crate,
and 9 scheduling wire tests in `tests/tests/scheduling.rs`; no external harness is
required to reproduce the server-level checks.

## Documentation

- [Feature-completeness matrix](docs/FEATURE_COMPLETENESS_MATRIX.md)
- [Adaptive Tree Scheduling design](docs/ADAPTIVE_TREE_SCHEDULING_DESIGN.md) and
  [measured policy comparison](docs/SCHEDULING_COMPARISON.md) — opt-in admission control,
  baseline-derived work quotas, and per-kind capacity via `AetherApp::with_scheduling`
- [Production-readiness report](docs/PRODUCTION_READINESS_REPORT.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Deployment](docs/DEPLOYMENT.md)
- Older reports (`FINAL_REPORT.md`, `CONSOLIDATED_FINAL_REPORT.md`) predate the current audit
  and may contain stale claims.

## License

MIT OR Apache-2.0.
