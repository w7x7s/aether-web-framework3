# Aether Web Framework — Complete Framework Specification (English)

Welcome to the comprehensive, production-grade technical specification and functional documentation for the **Aether Web Framework** in Rust.

Aether is an ultra-high-throughput, zero-copy, memory-safe asynchronous web framework engineered with strict RFC 9112 conformance, deterministic routing, and zero lock-contention hot paths.

---

## 📑 Complete Functional Table of Contents

1. [Architecture & Request Execution Pipeline](#1-architecture--request-execution-pipeline)
2. [Socket Layer & Adaptive Two-Tier TCP Framing](#2-socket-layer--adaptive-two-tier-tcp-framing)
3. [Zero-Copy HTTP/1.1 Parser & RFC 9112 Security Engine](#3-zero-copy-http11-parser--rfc-9112-security-engine)
4. [Deterministic Radix Trie Router & Method Bitmask Matching](#4-deterministic-radix-trie-router--method-bitmask-matching)
5. [Request Context & Memory Architecture (`RequestContext`)](#5-request-context--memory-architecture-requestcontext)
6. [Response Construction & Zero-Allocation Serialization (`Response`)](#6-response-construction--zero-allocation-serialization-response)
7. [Dependency Injection & Shared Application State (`AppStateContainer`)](#7-dependency-injection--shared-application-state-appstatecontainer)
8. [Middleware Subsystems (CORS, Security Headers, Tracing, Logger)](#8-middleware-subsystems-cors-security-headers-tracing-logger)
9. [Resource Limits, Concurrency Semaphores & Slowloris Mitigation (`ServerLimits`)](#9-resource-limits-concurrency-semaphores--slowloris-mitigation-serverlimits)
10. [Error Handling Subsystem (`AetherError`)](#10-error-handling-subsystem-aethererror)
11. [End-to-End Developer Examples](#11-end-to-end-developer-examples)

---

## 1. Architecture & Request Execution Pipeline

Aether executes requests along an explicit, non-blocking asynchronous pipeline designed to minimize cache-misses and eliminate global locking:

```text
 Client TCP Connection
          │
          ▼
┌─────────────────────────────────────────┐
│ 1. Concurrency Semaphore (acquire_owned)│  -> Limits concurrent sockets (Default: 10,000)
└─────────────────┬───────────────────────┘
                  ▼
┌─────────────────────────────────────────┐
│ 2. Two-Tier Persistent Read Buffer      │  -> BytesMut accumulation with Read Timeout
└─────────────────┬───────────────────────┘
                  ▼
┌─────────────────────────────────────────┐
│ 3. Zero-Copy RequestView Parser         │  -> RFC 9112 validation & Smuggling defense
└─────────────────┬───────────────────────┘
                  ▼
┌─────────────────────────────────────────┐
│ 4. Radix Trie Router ($O(K)$ Depth)     │  -> 64-bit Bitmask method matching + Path Params
└─────────────────┬───────────────────────┘
                  ▼
┌─────────────────────────────────────────┐
│ 5. Middleware Pipeline Execution        │  -> CORS / Security Headers / Zero-overhead Logger
└─────────────────┬───────────────────────┘
                  ▼
┌─────────────────────────────────────────┐
│ 6. Async Route Handler Invocation       │  -> User application code with RequestContext
└─────────────────┬───────────────────────┘
                  ▼
┌─────────────────────────────────────────┐
│ 7. Direct Buffer Serialization          │  -> `write_http1_to` direct to BytesMut writer
└─────────────────┬───────────────────────┘
                  ▼
┌─────────────────────────────────────────┐
│ 8. Non-blocking Socket Write & Flush    │  -> Write Timeout applied; RAII buffer cleanup
└─────────────────────────────────────────┘
```

---

## 2. Socket Layer & Adaptive Two-Tier TCP Framing

### The Problem:
TCP is a byte-stream protocol. A single HTTP request may arrive across multiple fragmented TCP packets, or multiple HTTP requests may arrive clustered together in a single read (HTTP Pipelining / Keep-Alive).

### The Aether Solution:
- **Fast-Path (Stack-Read):** Reads into a local 8 KB stack buffer.
- **Accumulator Buffer (`BytesMut`):** If `RequestView::parse` returns `Ok(None)` (partial request), the bytes are safely preserved in `conn_buf`, and the next socket chunk is awaited with `read_timeout`.
- **Zero-Copy Slicing (`split_to`):** Once a request of length $N$ is consumed, `conn_buf.split_to(consumed)` advances the buffer in $O(1)$ time without memory reallocation, immediately evaluating any remaining pipelined bytes.

---

## 3. Zero-Copy HTTP/1.1 Parser & RFC 9112 Security Engine

The parser constructs a borrowed `RequestView<'req>` directly over socket bytes:

### Security Hardening (RFC 9112 Conformance):
1. **Dual Content-Length Attack Prevention:** If multiple `Content-Length` headers or comma-separated lists are received, the request is rejected with `400 Bad Request`.
2. **CL.TE / TE.CL Desync Prevention:** Combining `Transfer-Encoding` with `Content-Length` is strictly forbidden and rejected immediately.
3. **Integer Overflow & Negative Range Protection:** All payload lengths are safely validated against `usize::MAX` and `ServerLimits::max_body_bytes`.
4. **Header Limits:** Header block size is capped by `max_header_bytes` (returns `431 Request Header Fields Too Large` on violation).

---

## 4. Deterministic Radix Trie Router & Method Bitmask Matching

### Algorithm:
- **Radix Trie with Segment Sorting:** Routes are indexed as hierarchical segment trees.
- **Deterministic Precedence:**
  1. **Static Segments** (`/users/me`) -> Checked first.
  2. **Parameterized Segments** (`/users/:id` or `/users/{id}`) -> Checked second.
  3. **Catch-All Wildcards** (`/static/*filepath`) -> Checked last.
- **Zero-Allocation Stack Traversal:** Path splitting uses `SmallVec<[&str; 8]>` on the CPU stack.
- **64-bit Method Bitmask:** Checking whether a route supports `GET`, `POST`, `PUT`, `DELETE`, etc., is executed in a **single CPU instruction**:
  ```rust
  if (node.allowed_methods_mask & (1 << method_id)) != 0 { /* Matched! */ }
  ```

---

## 5. Request Context & Memory Architecture (`RequestContext`)

`RequestContext` is passed to every route handler with strictly scoped lifetimes, preventing memory leaks (eliminated all `Box::leak` calls):

### Key Functions & APIs:

| Function | Signature | Description |
| :--- | :--- | :--- |
| `param` | `pub fn param(&self, key: &str) -> Option<&str>` | Retrieves route parameter value (e.g. `:id`). |
| `param_parse` | `pub fn param_parse<T: FromStr>(&self, key: &str) -> Option<T>` | Parses a route parameter into a typed primitive (`u32`, `i64`, `bool`). |
| `query` | `pub fn query(&self, key: &str) -> Option<&str>` | Retrieves query string parameter (`?page=2`). |
| `header` | `pub fn header(&self, name: &str) -> Option<&str>` | Case-insensitive header lookup. |
| `cookie` | `pub fn cookie(&self, name: &str) -> Option<&str>` | Extracts cookie value by name from `Cookie:` header. |
| `body_str` | `pub fn body_str(&self) -> Result<&str, Utf8Error>` | Zero-copy UTF-8 view of request body. |
| `json` | `pub fn json<T: DeserializeOwned>(&self) -> Result<T, Error>` | Direct JSON deserialization from body bytes. |
| `state` | `pub fn state<T: Send + Sync + 'static>(&self) -> Option<Arc<T>>` | Retrieves typed shared state from the DI container. |
| `request_id` | `pub fn request_id(&self) -> &str` | Accesses the unique trace identifier for the request. |

---

## 6. Response Construction & Zero-Allocation Serialization (`Response`)

### Response Constructors:
- `Response::ok(body)`: Returns status `200 OK`.
- `Response::json(&data)`: Serializes `data` to JSON and attaches `Content-Type: application/json`.
- `Response::status(code, text)`: Creates a custom status response (e.g. `201 Created`, `404 Not Found`).

### Direct Buffer Serialization (`write_http1_to`):
Writes headers and body directly into `BytesMut` using `bytes::BufMut::writer`, completely bypassing intermediate `String` or `format!()` heap allocations.

---

## 7. Dependency Injection & Shared Application State (`AppStateContainer`)

Aether includes a thread-safe DI container keyed by `TypeId`:

```rust
let mut app = AetherApp::new();
app.state_container.insert(DatabasePool::new("postgres://..."));
app.state_container.insert(RedisCache::new("redis://..."));

app.get("/data", |ctx| async move {
    let db = ctx.state::<DatabasePool>().expect("DatabasePool registered");
    Response::ok("Data fetched")
});
```

---

## 8. Middleware Subsystems

- **`CorsMiddleware`**: Configurable CORS policies (Origins, Methods, Headers, Max Age).
- **`SecurityHeadersMiddleware`**: Injects HSTS, X-Frame-Options (`DENY`), X-Content-Type-Options (`nosniff`), and CSP.
- **`LoggerMiddleware`**: Zero-cost abstraction (`#[inline(always)]`), disabled on hot-paths by default to prevent `stdout` global mutex contention.

---

## 9. Resource Limits, Concurrency Semaphores & Slowloris Mitigation (`ServerLimits`)

```rust
pub struct ServerLimits {
    pub max_header_bytes: usize,     // 8 KB default
    pub max_body_bytes: usize,       // 2 MB default
    pub max_headers_count: usize,    // 64 headers default
    pub max_connections: usize,      // 10,000 concurrent sockets
    pub read_timeout: Duration,      // 5s Slowloris mitigation
    pub write_timeout: Duration,     // 10s socket flush timeout
    pub keep_alive_timeout: Duration,// 60s idle timeout
}
```

---

## 10. Error Handling Subsystem (`AetherError`)

- `AetherError::MalformedRequest` -> Returns `400 Bad Request`.
- `AetherError::HeaderTooLarge` -> Returns `431 Request Header Fields Too Large`.
- `AetherError::PayloadTooLarge` -> Returns `413 Payload Too Large`.
- `AetherError::Timeout` -> Closes stream safely.

---

## 11. End-to-End Developer Examples

```rust
use aether::app::AetherApp;
use aether::context::RequestContext;
use http::response::Response;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct UserProfile {
    id: u64,
    username: String,
}

#[derive(Deserialize)]
struct CreateUserDto {
    username: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = AetherApp::new();

    app.get("/", |_ctx| async move {
        Response::ok("Aether Core Engine Running ⚡")
    });

    app.get("/users/:id", |ctx: RequestContext| async move {
        let user_id = ctx.param_parse::<u64>("id").unwrap_or(0);
        Response::json(&UserProfile {
            id: user_id,
            username: format!("user_{}", user_id),
        })
    });

    app.post("/users", |ctx: RequestContext| async move {
        match ctx.json::<CreateUserDto>() {
            Ok(dto) => Response::status(201, "Created")
                .with_json(&UserProfile { id: 101, username: dto.username }),
            Err(_) => Response::status(400, "Bad Request"),
        }
    });

    app.listen("0.0.0.0:3000").await?;
    Ok(())
}
```
