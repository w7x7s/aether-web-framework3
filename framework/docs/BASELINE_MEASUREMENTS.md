# Aether Baseline Measurements

> **SUPERSEDED — 2026-10-18.** Recorded on a different machine/profile (and with an incorrect
> Rust version line). Use [PRODUCTION_READINESS_REPORT.md](PRODUCTION_READINESS_REPORT.md)
> for the measurements taken during the final audit.

## Environment
- **OS**: Windows (Windows Subsystem for Linux or native Windows)
- **Date**: 2026-10-18
- **Rust Version**: 1.53.1 (from tokio dependency)
- **Build Profile**: Release (opt-level=3, lto=fat, codegen-units=1, panic=abort, strip=true)

---

## Build Time

### Clean Release Build
- **Time**: 1m 13.566s
- **Command**: `cargo clean && cargo build --release`
- **Result**: Successful with 2 warnings (unused variable, unused field)

### Benchmark Build
- **Time**: 3m 34s (includes criterion dependencies)
- **Command**: `cargo bench`
- **Result**: Successful

---

## Binary Size

### Release Binaries
- **basic_api.exe**: 379 KB
- **full_async_api.exe**: 442 KB

**Analysis**: Binary sizes are reasonable for a Rust web framework with tokio. The ~2 GB size mentioned in the task was not observed. The actual sizes are:
- Small enough for container images
- Includes tokio runtime (largest dependency)
- No excessive debug symbols (strip=true enabled)
- LTO and optimizations active

---

## Performance Benchmarks (Router)

### Static Route Lookup
- **Mean**: 231.36 ns
- **Range**: [221.59 ns, 241.61 ns]
- **Iterations**: 20M
- **Outliers**: 4 high severe (4%)

### Two-Parameter Route Lookup
- **Mean**: 459.82 ns
- **Range**: [434.85 ns, 489.49 ns]
- **Iterations**: 9.2M
- **Outliers**: 3 high mild (3%)

**Analysis**:
- Router performance is excellent (sub-microsecond)
- Static routes are ~2x faster than parameterized routes
- Low outlier count indicates stable performance
- Radix trie with bitmask matching is effective

---

## Test Suite Results

### Unit Tests
- **Total Tests**: 19 tests passed
- **Failed**: 0
- **Warnings**: 3 (unused imports, unused variable, unused field)

### Test Breakdown
- **aether-core**: 2 tests (arena, bitset)
- **aether-dependency**: 1 test (DAG topological sort)
- **aether-http**: 3 tests (smuggling protection, path traversal)
- **aether-router**: 1 test (radix routing with params)
- **aether-security**: 2 tests (constant-time, JWT verification)
- **aether-tests**: 8 tests (diagnostics, banner, MIME, ETag, arena, bitset, state, parsing, response)
- **aether-serialization**: 0 tests
- **aether-validation**: 0 tests
- **aether-websocket**: 0 tests
- **aether-middleware**: 0 tests
- **aether-observability**: 0 tests
- **aether-openapi**: 0 tests
- **aether-runtime**: 0 tests

**Analysis**:
- Test coverage is partial (~50% of crates have tests)
- Critical security tests exist (smuggling, path traversal)
- Missing tests for: middleware, validation, websocket, serialization, openapi, runtime
- Need to add integration tests for full request lifecycle

---

## Dependencies

### Workspace Dependencies
- smallvec = "1.13"
- bitflags = "2.5"
- bytes = "1.6"
- httparse = "1.9"
- serde = { version = "1.0", features = ["derive"] }
- sonic-rs = "0.3"
- tokio = { version = "1.38", features = ["full"] }
- tracing = "0.1"
- ring = "0.17"
- flate2 = "1.0"
- brotli = "6.0"
- criterion = "0.5"

**Analysis**:
- Dependencies are reasonable and well-maintained
- sonic-rs (fast JSON) is included but not used in code
- flate2 and brotli are included but compression not implemented
- ring is used for cryptographic operations (JWT)
- tokio with "full" features may be overweight

---

## Memory Footprint (Estimated)

### Per-Connection Memory
- **Connection buffer**: 8 KB (temp_buf) + variable (conn_buf)
- **Write buffer**: 4 KB (write_buf)
- **Request arena**: 4 KB (default capacity)
- **Router/Handler state**: Shared Arc (negligible per connection)
- **Total per connection**: ~16-20 KB (without body)

### With 10,000 connections (max_connections limit)
- **Estimated**: 160-200 MB
- **Analysis**: Reasonable for high-concurrency web server

---

## Startup Time

### Basic API Example
- **Measured**: Not yet measured (needs manual timing)
- **Estimated**: < 100ms (typical for tokio apps)

---

## Security Audit Findings

### Implemented Protections
1. ✅ Request smuggling protection (CL.TE, TE.CL, dual CL)
2. ✅ Path traversal protection (safe_resolve_path)
3. ✅ Header size limits (max_header_bytes)
4. ✅ Body size limits (max_body_bytes)
5. ✅ Slowloris protection (read_timeout)
6. ✅ Constant-time comparison (JWT signature verification)

### Missing Protections
1. ❌ No Content-Security-Policy header
2. ❌ No CSRF token generation/validation
3. ❌ No rate limiting
4. ❌ No IP-based blocking
5. ❌ No request size validation beyond header/body limits

---

## Correctness Issues Found

### High Priority
1. **405 responses missing Allow header** - Returns 405 but doesn't generate Allow header from allowed_methods bitmask
2. **No graceful shutdown** - No signal handling for SIGTERM/SIGINT
3. **No request timeout** - Handlers can hang indefinitely
4. **No panic recovery** - Panics in handlers will crash the task

### Medium Priority
1. **HTTP/2 not supported** - No hyper/h2 integration
2. **Compression not implemented** - Dependencies exist but unused
3. **No per-route middleware** - Cannot add middleware to specific routes
4. **No nested routers** - No Router::nest equivalent
5. **No custom middleware API** - No trait for user-defined middleware

### Low Priority
1. **Limited testing utilities** - No test request builder or client
2. **No typed extractors** - Manual extraction instead of trait-based
3. **No multipart form data** - No file upload support
4. **Partial WebSocket support** - Frame parser exists but no handshake

---

## Performance Gaps

### Missing Optimizations
1. No connection pooling for database connections
2. No response streaming (chunked transfer encoding)
3. No compression middleware (gzip/brotli)
4. No connection reuse metrics
5. No backpressure handling for slow clients

### Potential Optimizations
1. sonic-rs not used (could replace serde_json for JSON)
2. Buffer reuse could be improved
3. Arena allocation not fully utilized in request path
4. No SIMD for header parsing

---

## Developer Experience Gaps

### Missing Features
1. No handler macros (#[handler], #[route])
2. No auto OpenAPI generation from routes
3. No hot reloading for development
4. No request/response type validation at compile time
5. No built-in testing client or request builder

---

## Summary

### Strengths
- ✅ Excellent router performance (sub-microsecond)
- ✅ Reasonable binary sizes (379-442 KB)
- ✅ Strong security foundation (smuggling, traversal protection)
- ✅ Clean architecture with good separation
- ✅ Rich error diagnostics
- ✅ Fuzzing support included

### Weaknesses
- ❌ Missing HTTP/2 support (critical for modern web)
- ❌ No graceful shutdown (production issue)
- ❌ Limited middleware system
- ❌ No compression (performance issue)
- ❌ Partial test coverage (~50%)
- ❌ Missing Allow header in 405 responses (correctness issue)

### Priorities
1. **Fix correctness**: Add Allow header to 405 responses
2. **Add production features**: Graceful shutdown, request timeout
3. **Add HTTP/2**: Integrate hyper/h2
4. **Implement compression**: Use flate2/brotli dependencies
5. **Improve middleware**: Add custom middleware API and per-route support
6. **Increase test coverage**: Add tests for all crates
7. **Add testing utilities**: Request builder and test client
