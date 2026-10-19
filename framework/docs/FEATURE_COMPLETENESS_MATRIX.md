# Aether Feature-Completeness Matrix

**Audit date:** 2026-10-18
**Aether version:** workspace `0.1.0` (Rust 1.98.1, edition 2024)
**Scope:** the implementation in `framework/` is the source of truth. Nothing below is marked
implemented because a type, config field, or dependency exists; each `VERIFIED_IMPLEMENTED`
row was exercised by tests or a live server check.

**Status codes**

| Code | Meaning |
|---|---|
| `VERIFIED_IMPLEMENTED` | Behavior exists and was exercised by a test or live request. |
| `PARTIALLY_IMPLEMENTED` | Works, but a documented or important part of the behavior is missing. |
| `NOT_IMPLEMENTED` | No implementation. |
| `BROKEN` | Implemented but produces incorrect behavior. |
| `UNVERIFIED` | Cannot be verified in this environment. |
| `NOT_APPLICABLE` | Not a framework responsibility, or provided by the ecosystem for all frameworks. |

**Evidence used**

- `cargo test --workspace` — **258 tests, 0 failed** (2026-10-18), including 13 checked-in
  raw-socket end-to-end tests in `framework/tests/tests/http_e2e.rs`, 27 scheduler tests in
  `framework/scheduler/tests/`, and 9 scheduling wire tests in
  `framework/tests/tests/scheduling.rs`.
- An earlier raw-socket end-to-end probe against a release build: 86/86 checks covering framing,
  HEAD, routing, 405/Allow, Arabic percent-decoding, JSON errors, compression, CORS,
  chunked uploads, pipelining, panic isolation, static files, conditional GET, oversized and
  malformed requests, and slow-client timeout. The probe lives outside the repository; the
  checked-in suite supersedes it for regression evidence.
- Direct code inspection of every crate in the workspace.
- Reference-framework columns describe **documented built-in behavior** of Axum 0.8 and
  Actix Web 4.x as of the audit date. They were not independently re-tested here, and
  first-party external crates are called out rather than counted as built-in.

**Aether claims no HTTP/2 or HTTP/3.** The server implements HTTP/1.1 only.

---

## HTTP/1.1 and framing

| Feature | Aether | Axum | Actix Web | Evidence |
|---|---|---|---|---|
| HTTP/1.1 request parsing | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `http/src/parser.rs`; 53 crate tests; fuzz target |
| Strict header/request-target validation | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `httparse` + hostile-input tests |
| Content-Length framing | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Exact consumed-offset and partial-read tests |
| Chunked request decoding | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Single/multi chunk, extensions, trailers, pipelining tests; live upload |
| Chunked size/trailer bounds | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | 1024-byte line limit, body budget, trailer count/byte budgets |
| Reject dual `Content-Length` | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Parser tests and live smuggling probes |
| Reject `Content-Length` + `Transfer-Encoding` | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Parser tests; unsupported codings rejected |
| Keep-alive | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Connection loop, idle timeout; live multi-request connection |
| Pipelining (exact consumed-byte accounting) | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Two-request pipeline test; live pipelined GETs |
| HTTP/1.0 request handling | PARTIALLY_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | 1.0 requests parse, but every response is written as `HTTP/1.1` and 1.0 close-by-default semantics are not applied |
| Response body streaming / chunked responses | NOT_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Every response is buffered and sent with `Content-Length` |
| HTTP/2 | NOT_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | No `h2`/hyper integration |
| HTTP/3 | NOT_IMPLEMENTED | PARTIALLY_IMPLEMENTED | NOT_IMPLEMENTED | Axum only via ecosystem; no QUIC anywhere in Aether |

## Routing

| Feature | Aether | Axum | Actix Web | Evidence |
|---|---|---|---|---|
| Static routes | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Radix trie tests |
| Path parameters (`{id}`) | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Param nodes, percent-decoded, tested |
| Catch-all (`/*filepath`) | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Full-path capture via pointer offset; tests |
| Precedence: static > param > catch-all | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Backtracking tests |
| 404 handling | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Unit + live checks |
| 405 with complete `Allow` header | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Accumulated method bitmask across all path-matching branches; regression test + live check |
| HEAD via GET fallback | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Live HEAD checks; body suppressed while length preserved |
| Route groups (prefix) | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `RouteGroup` in `aether/src/app.rs` |
| Duplicate-route detection | VERIFIED_IMPLEMENTED | PARTIALLY_IMPLEMENTED | PARTIALLY_IMPLEMENTED | Registration panics with a diagnostic instead of silently shadowing |
| Nested routers / independent sub-apps | NOT_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Prefix grouping only; no nested state or middleware |
| Per-route middleware/guards | PARTIALLY_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | App-wide middleware plus group-scoped middleware (`AetherApp::use_middleware`, `RouteGroup::use_middleware`); a single route uses an empty-prefix group; no dedicated per-route API |
| Automatic OPTIONS / CORS preflight | PARTIALLY_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Automatic only when CORS is enabled; arbitrary OPTIONS can be registered manually; no generic OPTIONS responder |

## Request extraction

| Feature | Aether | Axum | Actix Web | Evidence |
|---|---|---|---|---|
| Path parameters | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `param`, `param_parse` |
| Query parameters (raw) | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `query`, `queries` |
| Query parameters (percent/form-decoded) | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `query_decoded`, Arabic `%D9%85` live check |
| Headers, case-insensitive | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `header()` |
| Duplicate / multi-value headers | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Repeated field lines are retained per name in wire order and exposed via `header_values()`; `header()` returns the first line and the legacy `headers` map remains last-value-wins. E2E duplicate-cookie test |
| Cookies | VERIFIED_IMPLEMENTED | PARTIALLY_IMPLEMENTED | VERIFIED_IMPLEMENTED | `cookie()` parser; no signed/encrypted cookies |
| JSON body parsing | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `json()` via serde; size bounded by parser; no `Content-Type` enforcement |
| `application/x-www-form-urlencoded` | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `form_data()` |
| Raw body bytes | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `RequestContext::body: Bytes` |
| Multipart / file uploads | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | RFC 7578 parser (`http/src/multipart.rs`): borrowed parts, strict boundary/header grammar, quoted parameters, part/header/size bounds; 11 unit tests + E2E |
| Typed extractors (`FromRequest`) | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `aether/src/extract.rs` + `aether/src/handler.rs`: `Path<T>`, `Query<T>`, `Json<T>`, `Form<T>`, `State<T>` on the `*_async` methods (0–4 args); 17 wire tests in `tests/tests/extractors.rs` |
| Streaming request bodies | NOT_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Entire body buffered before the handler runs |
| Request-scoped extensions/state | NOT_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | No per-request typed extension bag |

## State and dependency injection

| Feature | Aether | Axum | Actix Web | Evidence |
|---|---|---|---|---|
| App-wide typed state | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `manage`/`state`; TypeId container test |
| Request-scoped state | NOT_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | No extensions |
| Database connection pool | NOT_APPLICABLE | NOT_APPLICABLE | NOT_APPLICABLE | External crates for all three |
| Redis / cache client | NOT_APPLICABLE | NOT_APPLICABLE | NOT_APPLICABLE | External crates for all three |

## Error handling

| Feature | Aether | Axum | Actix Web | Evidence |
|---|---|---|---|---|
| Structured error type | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `AetherError` with diagnostics |
| JSON error responses, escaped | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `serde_json` escaping; live quote/newline check |
| Canonical reason phrases | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | 418 "I'm a Teapot" live check |
| Panic isolation → 500 | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Spawned handler + `JoinError`; `panic = "unwind"`; live panic check, server survives |
| Typed error → status mapping | PARTIALLY_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Any `Result<T, E: Display>` maps to 400 (`E5002`), including internal failures; no `IntoResponse for AetherError` |
| Request diagnostics | VERIFIED_IMPLEMENTED | PARTIALLY_IMPLEMENTED | PARTIALLY_IMPLEMENTED | `Diagnostic` with hint/suggestions; startup banner |
| Access logging | PARTIALLY_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `LoggerMiddleware` exists and is wired, but defaults to disabled and `AetherApp` exposes no enable method |

## Middleware

| Feature | Aether | Axum | Actix Web | Evidence |
|---|---|---|---|---|
| CORS allow-list + preflight | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Origin allow-list, credentials, `Vary: Origin`, preflight validation; live checks |
| Security headers | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Opt-in; HSTS, `X-Frame-Options`, `nosniff`, Referrer-Policy, `X-XSS-Protection: 0` |
| Compression (gzip/brotli) | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Opt-in; q-value negotiation, MIME skip, `Vary: Accept-Encoding`; live checks |
| Request logging | PARTIALLY_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | See above |
| Deterministic pipeline order | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Fixed order: security → CORS → compression → log |
| User-defined middleware | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `AetherApp::use_middleware` / `RouteGroup::use_middleware`: `Fn(RequestContext, Next) -> Future<Response>`; registration-order execution, short-circuit support; 8 wire tests in `tests/tests/user_middleware.rs` |
| Per-route middleware | PARTIALLY_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Group-scoped only; no dedicated single-route API (empty-prefix group is the workaround) |
| Rate limiting | NOT_IMPLEMENTED | NOT_IMPLEMENTED (ecosystem) | NOT_IMPLEMENTED (ecosystem) | No built-in limiter in any of the three; external crates required |
| Authentication middleware | NOT_IMPLEMENTED | NOT_IMPLEMENTED (ecosystem) | NOT_IMPLEMENTED (ecosystem) | JWT **validator** exists, but no middleware integration |

## Security

| Feature | Aether | Axum | Actix Web | Evidence |
|---|---|---|---|---|
| Request-smuggling defenses | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Dual CL, CL+TE, unsupported TE codings, HTTP/1.0 chunked rejected; tests + live probes |
| Header count/byte budgets | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | 431 before parse; stack/heap header slots; tests |
| Body budget | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Declared, chunked cumulative, and exact-limit tests |
| Slow-client protection | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | 5 s per-read idle timeout **and** a total receive deadline set by `max_request_time`; a trickle client receives 408 and is closed (verified live at an 8 s configured deadline). The same deadline covers body upload, so very slow large uploads are refused |
| Concurrent-connection cap | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | 10 000 default via semaphore |
| Path-traversal protection | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `safe_resolve_path`; Windows ADS/device-name/trailing-dot rejection; live traversal checks |
| CSRF protection | NOT_IMPLEMENTED | NOT_IMPLEMENTED (ecosystem) | NOT_IMPLEMENTED (ecosystem) | No token middleware |
| CSP | PARTIALLY_IMPLEMENTED | PARTIALLY_IMPLEMENTED | PARTIALLY_IMPLEMENTED | Static opt-in header only; no nonce/hash generation |
| HSTS | VERIFIED_IMPLEMENTED | PARTIALLY_IMPLEMENTED | PARTIALLY_IMPLEMENTED | Opt-in; default `max-age=31536000` |
| TLS termination | NOT_IMPLEMENTED | NOT_IMPLEMENTED | VERIFIED_IMPLEMENTED | Aether serves plaintext only; run behind a TLS proxy or terminate in-process |
| `Host` header validation | NOT_IMPLEMENTED | NOT_IMPLEMENTED | NOT_IMPLEMENTED | No host allow-list; relevant when absolute-form targets or vhosts are used |
| JWT validation (HS256) | VERIFIED_IMPLEMENTED | NOT_APPLICABLE | NOT_APPLICABLE | `aether-security`: alg pinning, exp/nbf/iat, iss/aud, canonical base64url, constant-time tag check; 7 tests |
| Constant-time comparison | VERIFIED_IMPLEMENTED | NOT_APPLICABLE | NOT_APPLICABLE | `ring::hmac::verify` and `constant_time_eq` |
| Secret management | NOT_IMPLEMENTED | NOT_IMPLEMENTED | NOT_IMPLEMENTED | Environment variables only |

## Timeouts, resources, lifecycle

| Feature | Aether | Axum | Actix Web | Evidence |
|---|---|---|---|---|
| Read timeout | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | 5 s default; live slowloris check |
| Write timeout | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | 10 s default around response write |
| Keep-alive timeout | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | 60 s default |
| Per-handler timeout | VERIFIED_IMPLEMENTED | PARTIALLY_IMPLEMENTED (ecosystem) | PARTIALLY_IMPLEMENTED (ecosystem) | `tokio::time::timeout` around handler task; 504 on expiry |
| Concurrent-connection limit | VERIFIED_IMPLEMENTED | PARTIALLY_IMPLEMENTED (ecosystem) | VERIFIED_IMPLEMENTED | Semaphore, RAII permit |
| Graceful shutdown | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `AetherServer::{bind, local_addr, serve_with_shutdown}`; Ctrl+C plus SIGTERM on Unix; stops accepting immediately, drains HTTP connections with a bounded deadline, then aborts. E2E test proves idle keep-alive sockets close and the listener is released. Upgraded WebSocket sessions are aborted when shutdown starts |
| Client-disconnect cancellation | PARTIALLY_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Handler is not cancelled on disconnect; the failure is observed at write time |
| Bounded response memory | NOT_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Responses, including whole static files, are fully buffered |
| Backpressure / load shedding | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Connection semaphore on the default path; opt-in adaptive scheduling (`AetherApp::with_scheduling`) adds bounded admission queues, explicit 503 + `Retry-After` shedding, per-kind capacity, and queue/rejection metrics. 9 wire tests in `framework/tests/tests/scheduling.rs` |
| Adaptive work scheduling (opt-in) | VERIFIED_IMPLEMENTED | NOT_IMPLEMENTED (ecosystem) | NOT_IMPLEMENTED (ecosystem) | `aether-scheduler` crate (protocol-independent engine; FIFO/RoundRobin/Deficit/AdaptiveTree policies, baseline learned from measurements, quota slicing of divisible work, starvation guard, adaptive resource budgets). 27 crate tests (17 engine, 8 simulator invariants, 2 stress) + 9 wire tests; design in `docs/ADAPTIVE_TREE_SCHEDULING_DESIGN.md`, measured policy comparison in `docs/SCHEDULING_COMPARISON.md` |

## Static files

| Feature | Aether | Axum | Actix Web | Evidence |
|---|---|---|---|---|
| Serve a byte buffer with MIME type | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `static_file`/`static_asset` |
| Serve from a directory at runtime | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `static_dir`/`static_dir_with_cache`; live checks |
| MIME detection | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Extension table; tests |
| Strong ETag | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `"len-hash"` syntax test; no embedded `W/` |
| Conditional GET (304) | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Weak, list, and `*` matching per RFC 9110; live checks |
| Cache-Control | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Configurable max-age |
| Path-traversal protection | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Tests + live checks |
| Directory listing | NOT_IMPLEMENTED | NOT_IMPLEMENTED (ecosystem) | PARTIALLY_IMPLEMENTED (ecosystem) | Directories return 404 |
| Range requests | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Single and suffix ranges, strong `If-Range`, `206`/`416`, `Content-Range`, `Accept-Ranges`; malformed/multi-range ignored; 6 tests + E2E |
| Pre-compressed `.gz`/`.br` assets | NOT_IMPLEMENTED | NOT_IMPLEMENTED (ecosystem) | NOT_IMPLEMENTED (ecosystem) | No variant selection |
| Streaming / `sendfile` | NOT_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Whole file read into memory; HEAD also reads it |
| Async disk I/O | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `tokio::fs` in `static_dir`; in-memory helpers are caller-supplied bytes |

## Testing utilities

| Feature | Aether | Axum | Actix Web | Evidence |
|---|---|---|---|---|
| Unit tests | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | 258 workspace tests |
| Integration tests | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | 12 cross-crate tests plus 13 checked-in raw-socket server tests |
| End-to-end wire tests | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `framework/tests/tests/http_e2e.rs`: routing, pipelining, chunked bodies, duplicate headers, multipart, static ranges, CORS, limits, panic isolation, timeout, shutdown, and WebSocket upgrade/echo/close |
| Fuzz target | VERIFIED_IMPLEMENTED | NOT_APPLICABLE | NOT_APPLICABLE | `fuzz/fuzz_targets/http_parse_fuzz.rs`; compiled, not fuzzed for a sustained run here |
| Test client / request builder | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `aether::test_client::{TestClient, TestRequest, TestResponse}`: real HTTP/1.1 over loopback, request builder, JSON/form helpers, graceful shutdown; 12 wire tests in `tests/tests/test_client.rs` |
| Property-based tests | NOT_IMPLEMENTED | NOT_IMPLEMENTED (ecosystem) | NOT_IMPLEMENTED (ecosystem) | No proptest integration |

## WebSocket

| Feature | Aether | Axum | Actix Web | Evidence |
|---|---|---|---|---|
| Frame parser | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | RSV/opcode/control-frame/minimal-length rules; 8 parser tests |
| Client-frame unmasking | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Mask requirement + unmask tests; unmasked client frame gets close 1002 in E2E |
| HTTP upgrade handshake | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `validate_handshake` (GET, `Upgrade`, `Connection` tokens, version 13, canonical 16-byte key, RFC sample accept vector); `AetherApp::ws` writes 101; 400/426/405 E2E |
| Message loop / connection driver | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Bounded `WebSocket`: fragmentation, interleaved control frames, auto-pong, close echo, UTF-8 and close-code validation, 16 MiB default message/frame caps; 10 driver tests + E2E |
| Message-size and buffer bounds | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Declared frame length rejected before payload is buffered; reassembled message cap; tests |
| Subprotocol negotiation | NOT_IMPLEMENTED | PARTIALLY_IMPLEMENTED (ecosystem) | PARTIALLY_IMPLEMENTED | `Sec-WebSocket-Protocol` is not negotiated |
| Streaming / permessage-deflate | NOT_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | No compression or streaming API |
| Graceful close on server shutdown | NOT_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Upgraded sessions are aborted when `serve_with_shutdown` begins |

## Developer experience

| Feature | Aether | Axum | Actix Web | Evidence |
|---|---|---|---|---|
| Typed route registration | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Method-specific builders; PATCH supported |
| Handler macros | NOT_IMPLEMENTED | NOT_IMPLEMENTED | VERIFIED_IMPLEMENTED | Manual closures |
| OpenAPI generation | PARTIALLY_IMPLEMENTED | NOT_IMPLEMENTED (ecosystem) | NOT_IMPLEMENTED (ecosystem) | Manual `OpenApiSpec` builder; not generated from routes |
| Environment configuration | VERIFIED_IMPLEMENTED | PARTIALLY_IMPLEMENTED | PARTIALLY_IMPLEMENTED | Seven `AETHER_*` variables, strict parsing; `workers` is not applied to the runtime |
| Startup banner / diagnostics | VERIFIED_IMPLEMENTED | NOT_IMPLEMENTED | NOT_IMPLEMENTED | Banner now states actual runtime/allocator |
| Compiling examples | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `basic_api`, `full_async_api` compile in the workspace |
| Hot reload | NOT_IMPLEMENTED | NOT_IMPLEMENTED (ecosystem) | NOT_IMPLEMENTED (ecosystem) | — |
| README / docs accuracy | PARTIALLY_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Root README was an unrelated AI Studio template; corrected during this audit. Legacy reports still contain stale claims |

## Performance features

| Feature | Aether | Axum | Actix Web | Evidence |
|---|---|---|---|---|
| Zero-copy head parsing | PARTIALLY_IMPLEMENTED | PARTIALLY_IMPLEMENTED | PARTIALLY_IMPLEMENTED | Parser borrows the connection buffer, but the server copies the body and every header into owned `RequestContext` fields before handlers run |
| Request arena | NOT_IMPLEMENTED | NOT_APPLICABLE | NOT_APPLICABLE | `RequestArena` exists in `aether-core` and is tested, but the server path does not use it |
| SIMD-style validation | PARTIALLY_IMPLEMENTED | NOT_APPLICABLE | NOT_APPLICABLE | 64-bit word ASCII scan used by `Email`; not in the HTTP hot path |
| Connection buffer reuse | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | `BytesMut` retained per connection; `split_to` for pipelining |
| Thread-per-core runtime | NOT_IMPLEMENTED | NOT_IMPLEMENTED | PARTIALLY_IMPLEMENTED | Aether uses Tokio's multi-threaded work-stealing runtime; `AETHER_WORKERS` affects only the banner |
| Compression | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | VERIFIED_IMPLEMENTED | Opt-in gzip/brotli middleware |
| Release profile tuned for size | VERIFIED_IMPLEMENTED | NOT_APPLICABLE | NOT_APPLICABLE | `lto = "fat"`, `codegen-units = 1`, `strip = true`, `panic = "unwind"` |

## Status count (Aether only)

| Status | Rows |
|---|---|
| VERIFIED_IMPLEMENTED | 80 |
| PARTIALLY_IMPLEMENTED | 11 |
| NOT_IMPLEMENTED | 30 |
| BROKEN | 0 |
| UNVERIFIED | 0 |
| NOT_APPLICABLE | 2 |

Total: 123 rows.

The totals are the literal row counts in the tables above. They are not a quality score: a
`NOT_APPLICABLE` row (database pool, Redis) is not a deficiency, and several
`NOT_IMPLEMENTED` rows are ecosystem features in Axum/Actix too.

---

## Highest-priority remaining work

### Correctness / security
1. **Typed error mapping.** `Result<T, E>` always produces 400. Add `IntoResponse for
   AetherError` (and let embedders control `E`) so internal failures produce 500.
2. **Separate receive deadlines.** The total receive deadline currently reuses
   `max_request_time` (also the handler timeout), so a legitimately slow large upload and a
   slow handler share one budget. A dedicated header/body timeout setting is preferable.
3. **Client-disconnect cancellation.** A handler is not cancelled when its client goes away.
4. **Host validation, CSRF, rate limiting, authentication middleware, TLS.** All remain
   external/absent; see the security table.

### Feature completeness
5. HTTP/2 and HTTP/3 (a new connection engine, not a patch to the HTTP/1.1 loop).
6. Streaming request bodies, streaming responses, and static-file `sendfile`; everything is
   currently buffered.
7. Custom and per-route middleware API (delivered 2026-10-18: app-wide and group-scoped
   `use_middleware` with `Next`-continuation composition; a dedicated single-route API is
   still open).
8. Typed extractors (delivered 2026-10-18: `Path`/`Query`/`Json`/`Form`/`State`) and
   request-scoped extensions (still open).
9. WebSocket permessage-deflate, subprotocol negotiation, and a graceful close frame on
   server shutdown (sessions are currently aborted).

### Performance (only after measurement)
10. Remove per-request header `HashMap` construction and body copy where handlers do not need
    them; re-measure before/after at several concurrency levels. Do not assume zero-copy wins
    end-to-end.
11. Arena integration, *if* profiling shows allocator time in the request path.
12. Run the fuzz target for a sustained period; it compiles and has unit-level hostile-input
    coverage, but no long fuzzing campaign was run here.
