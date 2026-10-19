# Aether Production-Readiness Report

**Date:** 2026-10-18
**Workspace:** `aether-web-framework/framework` (no VCS metadata in this snapshot)
**Toolchain:** rustc 1.98.1 (48a229cea 2026-09-01), cargo 1.98.1, edition 2024

This report records what was actually changed, tested, and measured. Where a claim could not
be verified in this environment it is marked as such.

---

## 1. Environment

| Item | Value |
|---|---|
| OS | Windows 10.0.26200 (x64) |
| CPU | Intel Core i7-8565U, 4 cores / 8 logical threads, 1.80 GHz |
| RAM | 16,976,576,512 bytes (~15.8 GiB) |
| Runtime | Tokio multi-threaded work-stealing; not thread-per-core |
| Load generator | Python 3.14.7 raw sockets, loopback |
| Server build | `cargo build --release`, default profile for the external probe; workspace profile `lto="fat"`, `codegen-units=1`, `strip=true`, `panic="unwind"` for framework binaries |

The machine is a laptop, not a bare-metal benchmark host. Absolute throughput numbers are not
comparable with published framework benchmarks.

---

## 2. Executive summary

- **Correctness/security work is verified.** `cargo test --workspace` passes **182 tests, 0
  failures**, including **13 checked-in raw-socket end-to-end tests** in
  `framework/tests/tests/http_e2e.rs` (routing, pipelining, chunked bodies, duplicate headers,
  multipart, static ranges, CORS, limits, panic isolation, total deadline, graceful shutdown,
  and WebSocket upgrade/echo/close). An earlier external probe passed **86/86** checks against
  a release build, including smuggling attempts, oversized/malformed requests, and slow-client
  timeout.
- **Newly completed capabilities in this round:** RFC 7578 multipart parsing, RFC 9110
  `Range`/`If-Range` static-file serving, duplicate request-header preservation, testable
  server lifecycle with programmatic shutdown + SIGTERM, and full RFC 6455 WebSocket support
  (handshake validation, bounded connection driver, `app.ws` integration).
- **Highest-impact missing behavior implemented in earlier rounds:** chunked request decoding
  (with trailers and limits), dynamic static-directory serving (strong ETags, 304, traversal
  protection), JWT claim/algorithm validation, a real 405 `Allow` union, and release panic
  isolation (`panic = "unwind"`).
- **Performance (release build, hello-world route, measured before the multipart/WebSocket
  additions):** median **8.5k req/s at concurrency 1**, **22.5k req/s at concurrency 8**,
  **20.2k req/s at concurrency 64**; server CPU **0.09–0.13 ms per request**; idle RSS
  **6.9 MiB**, peak RSS **8.8 MiB**; zero failed requests. Multipart and WebSocket were not
  benchmarked, and no new optimization is claimed.
- **Earlier three-framework comparison is inconclusive:** Aether ~23.9k/20.8k, Axum
  ~25.3k/17.8k, Actix ~24.8k/25.6k req/s (plaintext/JSON), with 33–59 % run-to-run noise.
  **No framework is claimed to be universally faster.**
- **Binary size (fresh release build after WebSocket integration):** examples are
  **2,129,408 B** and **2,164,736 B** (~2.03/2.06 MiB), +92,160/+93,696 B over the
  pre-WebSocket build. Switching from `panic = "unwind"` to `"abort"` saved ~266–333 KB but
  was rejected because it turns a handler panic into a process-wide abort. The 2.8 GB
  `target/` directory is build artifacts, not executable size.
- **Aether remains HTTP/1.1 only.** HTTP/2 and HTTP/3 are not implemented and are not claimed.

---

## 3. Feature completeness

Full row-by-row matrix: [FEATURE_COMPLETENESS_MATRIX.md](FEATURE_COMPLETENESS_MATRIX.md).

Status counts (122 audited rows):

| Status | Rows |
|---|---|
| VERIFIED_IMPLEMENTED | 78 |
| PARTIALLY_IMPLEMENTED | 12 |
| NOT_IMPLEMENTED | 30 |
| BROKEN | 0 |
| UNVERIFIED | 0 |
| NOT_APPLICABLE | 2 |

The 30 missing rows include HTTP/2, HTTP/3, response streaming, streaming request bodies,
typed extractors, custom/per-route middleware, rate limiting, authentication middleware, TLS
termination, WebSocket compression/subprotocol negotiation/graceful-close-on-shutdown,
directory listing, a test client, host validation, and CSRF.

---

## 4. Changes made

### 4.1 HTTP framing and parser (`http/src/parser.rs`)

- Split head parsing from body framing (`RequestHead` internal type). `RequestView::parse`
  retains its public signature and semantics (Content-Length framing).
- Added public `parse_request`, which returns the request view, an optional owned chunked
  body, and the exact consumed byte count.
- Implemented strict chunked decoding: size line before extensions, ≤16 hex digits, CRLF after
  each chunk, cumulative body budget, last-chunk trailers validated as header tokens, trailer
  count/byte budgets, HTTP/1.0 chunked rejection, exactly one `Transfer-Encoding` header, and
  only the `chunked` coding.
- Preserved and regression-tested smuggling defenses: dual `Content-Length`, `CL`+`TE`,
  malformed/overflowing lengths, unsupported codings, header count (including heap fallback
  above 64 slots) and header-byte budgets.
- 53 crate tests, including every-prefix partial input, pipelining offsets, exact-limit
  behavior, and hostile input.

### 4.2 Server (`aether/src/app.rs`, new `aether/src/static_dir.rs`)

- Integrates `parse_request`; chunked bodies are delivered as owned `Bytes`, framed bodies are
  copied out of the reusable connection buffer before handler execution.
- Removed a per-request header-map clone (map is moved into `RequestContext`).
- Fixed duplicate `Connection: close` on handler timeout when the client also requested close.
- Added a total receive deadline: each request receive phase is bounded by
  `max_request_time`; incomplete requests receive `408` and the connection closes. The 5 s
  per-read idle timeout remains.
- Added `static_dir` and `static_dir_with_cache`: async file I/O, MIME detection, strong ETag,
  `Cache-Control`, conditional GET, no directory listing, traversal protection.
- Retained earlier fixes: HEAD→GET fallback with body suppression, CORS preflight only when no
  user OPTIONS route exists, preflight requires a routable requested method, percent-decoded
  path parameters, IPv6-aware config, JSON-escaped error bodies, graceful Ctrl+C drain with
  `JoinSet` and a 10 s deadline.
- Added `bind`/`bind_with_config` returning `AetherServer` with `local_addr()` and
  `serve_with_shutdown(future)`, so tests and embedders can bind port 0, discover the assigned
  port, and stop the server deterministically. Unix installs a SIGTERM handler alongside
  Ctrl+C. The banner reports the actual bound port.
- Fixed limits desynchronization when a config was supplied at listen time, and the banner's
  route count now includes WebSocket routes.

### 4.3 Router (`router/src/trie.rs`)

- Fixed 405 `Allow` generation: the method bitmask now accumulates across **all**
  path-matching branches (static, param, catch-all), not just the first.
- Retained duplicate-route detection (panic with diagnostic), catch-all full-path capture, and
  static > param > catch-all backtracking. 16 tests.

### 4.4 Static files (`http/src/static_files.rs`)

- Strong, syntactically valid ETag (`"len-hash"`); removed the invalid embedded `W/`.
- `If-None-Match` supports lists, `*`, and weak comparison per RFC 9110 §13.1.2.
- Hardened `safe_resolve_path`: traversal, backslashes, drive letters, percent/backslash
  escapes; Windows alternate data streams (`:`), reserved device names (CON/PRN/AUX/NUL/
  COM1-9/LPT1-9), and trailing dots/spaces are rejected.
- Added negative and conditional-GET tests.

### 4.5 Panic isolation (`framework/Cargo.toml`)

- Release profile changed from `panic = "abort"` to `"unwind"`, with a comment explaining why:
  the spawned handler's `JoinError` becomes a 500 for that request; abort would kill every
  connection in the process. Verified on the release probe: `/panic` returns 500 and the
  server keeps serving.

### 4.6 WebSocket (`websocket/src/{lib,handshake,connection}.rs`, `aether/src/app.rs`)

- Strict frame validation: RSV bits must be zero; only defined opcodes; control frames must be
  FIN and ≤125 bytes; minimal length encodings enforced; checked length arithmetic.
- Added `ClientFrame`/`parse_client_frame` requiring the client mask and unmasking correctly.
  Public `WsFrameView` layout preserved. 8 parser tests.
- Added RFC 6455 opening handshake validation: GET only, `Upgrade: websocket` and
  `Connection: Upgrade` token matching (case-insensitive, comma lists), version 13 only, and a
  canonical base64 key that decodes to exactly 16 bytes. The accept value uses `ring`'s
  legacy SHA-1; the RFC sample vector is tested.
- Added a bounded connection driver: fragmentation assembly, interleaved control frames,
  automatic pong, close echo/status validation, UTF-8 validation, 16 MiB default message and
  frame caps, and header-first rejection of oversized frames before buffering their payloads.
- Added `AetherApp::ws(path, handler)` and `RouteGroup::ws`; the server writes the 101
  response, passes any pipelined bytes into the driver, and maps failures to 400/426/405 with
  `Sec-WebSocket-Version: 13` or `Allow: GET` where applicable.
- 22 crate tests (8 parser, 4 handshake, 10 driver) plus two raw-socket E2E tests.

### 4.7 JWT (`security/src/jwt.rs`)

- Signature check now uses `ring::hmac::verify` (constant time).
- Added `JwtValidationOptions` and `verify`: algorithm pinning (`HS256`), `exp`/`nbf`/`iat`
  with leeway and saturating arithmetic, exact `iss`, and `aud` as string or array.
- `decode_base64url` rejects impossible lengths, non-zero trailing bits, and the standard
  alphabet. Original `verify_signature` retained. 7 tests.
- Added `serde_json` to `aether-security` (already present in the workspace lockfile).

### 4.8 Diagnostics and docs

- Startup banner now states the real engine/allocator: Tokio work-stealing, system allocator,
  configured worker count (not applied).
- Crate docs state HTTP/1.1-only, body copies, and unused arena.
- Root README replaced (it was an unrelated AI Studio template); `framework/README.md` created
  (the manifest referenced a missing README).
- Feature matrix rewritten. Old reports/marketing docs now carry explicit
  SUPERSEDED/NOT-VERIFIED banners: `FINAL_REPORT.md`, `CONSOLIDATED_FINAL_REPORT.md`,
  `BENCHMARKS.md`, `BASELINE_MEASUREMENTS.md`, `ARCHITECTURE.md`.
- Examples no longer interpolate user input into JSON; they build values with
  `serde_json::json!`.

### 4.9 Multipart form data (`http/src/multipart.rs`)

- New RFC 7578 parser exposed as `RequestContext::multipart()`.
- Parts borrow the request body (no file-content copy) and expose name, filename,
  content type, part headers, and bytes.
- Strict grammar: line-start delimiters, closing `--`, transport padding, preamble/epilogue,
  quoted-string parameter unescaping, folded/bare-LF header rejection, and `form-data`
  disposition validation. Filenames are metadata only and never touch the filesystem.
- Bounds: 1024 parts, 16 KiB of headers per part, 64 headers per part, 70-byte boundary (the
  RFC limit). 11 unit tests plus an end-to-end upload test.

### 4.10 Range requests (`http/src/static_files.rs`, `aether/src/static_dir.rs`)

- Added `parse_range_header`, strong-only `if_range_matches`, and
  `serve_static_with_range`.
- Supports one byte range (closed, open-ended, and suffix forms), `206 Partial Content`,
  `416 Range Not Satisfiable` with `Content-Range: bytes */len`, and `Accept-Ranges: bytes`.
- Malformed or multiple ranges are ignored (full 200 response); weak `If-Range` validators
  never match, so a changed representation is never sliced under a stale validator.
- Files are still read fully into memory; streaming remains missing. 4 range-specific unit
  tests plus E2E checks for 206/304/416 and If-Range.

### 4.11 Duplicate headers and checked-in end-to-end tests

- `RequestContext` retains all values of repeated field lines in wire order
  (`header_values()`), while `header()` returns the first line and the public `headers` map
  keeps its last-value-wins compatibility behavior.
- `Accept-Encoding` is negotiated across repeated field lines per RFC 9110 §5.3.
- Added `framework/tests/tests/http_e2e.rs`: 13 raw-TCP tests that bind port 0, read the real
  address, and exercise the wire protocol, including multipart, ranges, CORS, limits, 408,
  graceful shutdown, and WebSocket. These run in `cargo test --workspace`, so the claims no
  longer depend on an external harness.

---

## 5. Tests and build

### Commands and results

| Command | Result |
|---|---|
| `cargo test --workspace` (final tree) | **182 passed, 0 failed**, exit 0 |
| `cargo check --workspace --all-targets` | clean, exit 0 |
| `cargo build --release --workspace` | success (fresh run in this round; see §7) |
| `python aether_e2e_probe.py` (release probe, earlier round) | **86 checks, 86 passed, 0 failed** |
| Trickle-client probe (8 s `max_request_time`, earlier round) | `HTTP/1.1 408 Request Timeout` at 8.1 s |
| Startup to first accepted connection (3 runs, earlier round) | 80.9 / 76.6 / 83.0 ms |

Test breakdown (final tree): `aether-http` 68, `aether-middleware` 35, `aether-websocket` 22,
`aether-router` 16, `aether-tests` E2E 13, integration 12, `aether-core` 8, `aether-security`
7, `aether-dependency` 1. Doc tests contain no runnable examples.

The checked-in E2E suite covers: routing params, 405/Allow, HEAD fallback, pipelining and
chunked bodies, duplicate headers, multipart uploads, static conditional/range requests, CORS
scoping, 413/431, panic isolation, the total receive deadline, graceful shutdown, WebSocket
handshake/echo/fragmentation/ping/close, and handshake/unmasked-frame rejection. The earlier
external probe additionally covered status reasons, Arabic query decoding, JSON error escaping,
compression negotiation, and traversal probes.

**Not verified:** sustained fuzzing (the fuzz target compiles but was not run), behavior behind
a real TLS proxy, WebSocket compression/subprotocols (not implemented), and any HTTP/2 or
HTTP/3 behavior (not implemented).

---

## 6. Performance measurements

### 6.1 Release build, keep-alive hello-world

These numbers were measured **before** multipart and WebSocket were added. Those features do
not execute on the ordinary GET path (one extra `Arc` clone per connection and a larger route
table are the only costs), but they have not been re-benchmarked, so no claim is made that they
leave throughput unchanged.

Method: raw-socket Python workers, one connection per worker, sequential keep-alive requests,
8 s measured window after a 1 s warm-up, 3 trials per concurrency. Server CPU from
`Get-Process` deltas; memory from `WorkingSet64`/`PeakWorkingSet64`. Zero request failures in
all runs.

| Concurrency | Trials (req/s) | Median | Server CPU/request | Peak RSS |
|---|---|---|---|---|
| 1 | 8,479 / 9,111 / 7,496 | **8,479** | 0.086–0.103 ms | 6.9 MiB |
| 8 | 22,167 / 22,518 / 22,643 | **22,518** | 0.127–0.131 ms | 7.3 MiB |
| 64 | 22,109 / 15,467 / 20,160 | **20,160** | 0.110–0.124 ms | 8.8 MiB |

At concurrency 8 the server used ~2.8 cores; at concurrency 64 the Python client is visibly
the bottleneck (throughput fell and variance rose). Idle RSS was 6.85 MiB; final working set
after load was 7.8 MiB. Startup was ~77–83 ms.

### 6.2 Three-framework comparison (earlier run in this audit)

| Framework | Plaintext medians | JSON medians |
|---|---|---|
| Aether | ~23.9k req/s | ~20.8k req/s |
| Axum | ~25.3k req/s | ~17.8k req/s |
| Actix Web | ~24.8k req/s | ~25.6k req/s |

Run-to-run variance was **33–59 %**, so these medians do not establish a winner in either
direction. The Aether build used for that run predates the final deadline/parser edits; those
edits do not touch the normal hello-world path except for one `Option<Instant>` check per
read. Wire-size observation from that run: Aether responses carried ~+25 % (plaintext) to
~+35 % (JSON) more bytes than the reference apps because of `X-Request-ID` (+32 B) and
`application/json; charset=utf-8`. This is a concrete, measurable disadvantage and a candidate
for optimization.

### 6.3 Interpretation and rejected optimizations

- **The server is CPU-bound before the client at c=8.** ~0.13 ms of server CPU per hello-world
  request is high; per-request `tokio::spawn` + timeout, request-id formatting, header
  `HashMap` allocation, param map, and body copy are the structural costs. They were
  identified but **not** optimized further because no isolated before/after measurement was
  available and a late restructuring would risk correctness.
- **`panic = "abort"` was rejected** despite saving 266–333 KB: it sacrifices per-request
  panic isolation.
- **`http.client`-based benchmarks were discarded** as client-bound and not representative.
- **Automated Ctrl+C shutdown tests were unreliable** on this Windows environment (process
  exit `0xC000013A`, signal occasionally not delivered). The added programmatic
  `serve_with_shutdown` API and E2E test now verify the drain path without depending on
  console signals.
- **Arena adoption and zero-copy handler views were not attempted.** The measurement does not
  prove they would help end-to-end, and both would require public API changes.

---

## 7. Binary and artifact size

| Artifact | Size |
|---|---|
| `basic_api.exe` (final, with WebSocket) | 2,129,408 B (2.03 MiB) |
| `full_async_api.exe` (final, with WebSocket) | 2,164,736 B (2.06 MiB) |
| Same examples before WebSocket integration (earlier round) | 2,037,248 B / 2,071,040 B |
| Delta attributable to WebSocket integration | +92,160 B / +93,696 B (~4.5 %) |
| Same examples built with `panic="abort"` (controlled experiment, earlier code) | 1,697,280 B / 1,713,152 B |
| Delta attributable to unwinding (approximate) | +265,728 B (+15.7 %) / +332,800 B (+19.4 %) |
| External release probe binary | 2,425,856 B (default profile, not stripped/LTO) |
| `framework/target/` total (after the fresh release build) | 2.8 GB |

The previous "1.4–1.5 GB" figure was the build directory, not an executable. No framework
features were removed to reduce size. Growth versus older reports (~1.56–1.62 MB) comes from
`panic = "unwind"` plus new code (chunked decoding, static_dir, JWT claims, deadline,
multipart, ranges, WebSocket) and `serde_json` use in examples. The final incremental release
build took 44.26 s.

---

## 8. Security and compatibility findings

### Security fixes verified

- Request smuggling: dual CL, CL+TE, unsupported TE, HTTP/1.0 chunked, malformed/overflowing
  lengths are rejected (400); header/body budgets return 431/413.
- Trickle slow clients are bounded by a total receive deadline and receive 408.
- Static-file traversal, Windows ADS/device names, and encoded traversal are blocked.
- Panic isolation returns 500 without terminating the process.
- JWT verification is constant-time with algorithm and claim validation.
- WebSocket frame parsing rejects reserved bits/opcodes, fragmented control frames, oversized
  control frames, non-minimal lengths, and unmasked client frames; the handshake enforces GET,
  version 13, upgrade tokens, and a canonical 16-byte key. Frame and message sizes are bounded
  before payload buffering.
- Multipart parsing bounds part count, per-part headers, header bytes, and boundary length;
  filenames are never used as filesystem paths.
- Range handling only slices a representation when a strong `If-Range` validator still matches;
  multiple/malformed ranges fall back to the full response.
- Control characters/JSON injection in framework-generated error bodies are escaped.

### Remaining security gaps

1. **TLS is not built in.** Deploy behind a TLS terminator; verify forwarded-header handling
   there.
2. **No `Host` header validation/allow-list.**
3. **No CSRF protection.**
4. **No rate limiting or authentication middleware** (JWT is a library, not middleware).
5. **Generic handler errors map to 400, not 500** (`E5002` body), masking server faults.
6. **Graceful shutdown aborts WebSocket sessions** rather than sending a close frame; HTTP
   connections drain properly, and `SIGTERM`/programmatic shutdown now exist.
7. **Cookies are parsed but not signed/encrypted.**
8. **CSP is an opt-in static header**, with no nonce/hash support.
9. **The receive deadline covers body upload**, so a legitimately slow large upload can be
    408'd; it is configurable only through the shared request-timeout setting.
10. **WebSocket compression and subprotocol negotiation are not implemented.**
11. **WebSocket sessions have no post-upgrade read/write deadline.** An idle or non-reading
    peer holds a connection permit until it closes or shutdown aborts the session; only the
    101 response itself is covered by the HTTP write timeout.

### Public API compatibility

No existing public function signatures were removed or changed. Preserved:
`RequestView::parse`, `WsFrameView` layout, `JwtValidator::verify_signature`,
`ServerLimits`, `AetherConfig`, `RequestContext`, `AetherApp` registration methods.

Additions: `http::parse_request`; `aether::AetherApp::{static_dir, static_dir_with_cache,
ws}`; `aether::{AetherServer, bind, bind_with_config}`, `AetherServer::{local_addr, serve,
serve_with_shutdown}`; `RequestContext::{header_values, multipart}`;
`http::multipart::{MultipartForm, Part, MultipartError}`;
`http::static_files::{parse_range_header, if_range_matches, serve_static_with_range,
RangeDecision}`; `websocket::{ClientFrame, parse_client_frame, validate_handshake,
accept_for_key, HandshakeError, WebSocket, WebSocketLimits, WsError, Message}`;
`JwtValidationOptions`, `JwtValidator::verify`, `VerifiedJwt`.

Behavioral changes (not signature breaks) that embedders must know:

- `Transfer-Encoding: chunked` is now decoded instead of rejected.
- WebSocket and JWT parsing are stricter.
- Static ETags are now strong (`"…"`) instead of a malformed weak-looking string.
- A panicking handler now yields 500 under the release profile (previously the process
  aborted).
- CORS preflight now checks that the requested method is routable.
- `AETHER_MAX_HEADER_BYTES`, `AETHER_WORKERS`, and the two time variables are now read by
  `from_env`; `AETHER_WORKERS` is still **not applied** to the Tokio runtime.
- Response `Connection: close` is emitted once, not twice.
- `RequestContext::header()` returns the first field line; the legacy `headers` map remains
  last-value-wins. Use `header_values()` when repeats matter.
- Static handlers now answer satisfiable `Range` requests with 206 and unsatisfiable ones with
  416 instead of always returning 200.
- `framework/middleware/src/timeout.rs` (unreferenced, uncompilable dead file) was deleted
  with owner approval; no module or API referenced it.

Dependency changes: `aether-security` now depends on `serde_json`; `aether-websocket` now
depends on `ring` and `tokio`; `aether` depends on `aether-websocket`. All were already in the
lockfile. No dependency was removed.

---

## 9. Remaining work (priority order)

1. Typed error mapping (`IntoResponse for AetherError`, 500 for internal errors).
2. Separate receive/upload timeout from the handler timeout; client-disconnect cancellation.
3. Streaming request bodies, streaming responses, and static-file `sendfile`.
4. Custom and per-route middleware.
5. Typed extractors and request extensions.
6. HTTP/2 (new connection engine; not a small patch); HTTP/3 after that.
7. WebSocket permessage-deflate, subprotocol negotiation, and graceful close on shutdown.
8. TLS termination (or documented proxy-only deployment), Host validation, CSRF, rate
   limiting, and authentication middleware.
9. Performance work driven by isolated before/after measurements: request-id formatting,
   header map allocation, handler spawn/timeout structure, and wire-size reduction.
10. Run the fuzz target for a sustained period.

---

## 10. Evidence and cleanup

- Checked-in regression evidence: `framework/tests/tests/http_e2e.rs` (13 raw-socket tests run
  by `cargo test --workspace`).
- Final test run, 2026-10-18: `cargo test --workspace` — **182 passed, 0 failed**.
- Historical logs from earlier rounds (may describe the earlier 140-test tree):
  `C:\Users\hamza\AppData\Local\Temp\aether_full_tests_final2.log`,
  `C:\Users\hamza\AppData\Local\Temp\aether_release_final.log`.
- Earlier external E2E harness: `C:\Users\hamza\AppData\Local\Temp\aether_e2e_probe.py`;
  benchmark harness: `C:\Users\hamza\AppData\Local\Temp\aether_bench_final.py`.
- Probe project: `C:\Users\hamza\Desktop\nwfm\aether-probe` (outside the repository).
- All probe servers were stopped after measurement.

The probe project and temporary harnesses are outside the repository and are not part of the
delivered framework. The `target/` directory was retained; it is build output, not source.
