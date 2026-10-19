# Aether Web Framework

Aether is a Rust web framework built on Tokio with its own HTTP/1.1 connection engine,
a segment radix router, request-scoped security limits, and built-in CORS, security-header,
and compression middleware.

The Rust workspace lives in [`framework/`](framework/). The audit documents are in
[`framework/docs/`](framework/docs/), including the
[feature-completeness matrix](framework/docs/FEATURE_COMPLETENESS_MATRIX.md) and the
[production-readiness report](framework/docs/PRODUCTION_READINESS_REPORT.md).

## Status

HTTP/1.1 only. HTTP/2 and HTTP/3 are **not** implemented. The matrix records verified
behavior against Axum and Actix Web and lists every missing feature; do not rely on older
reports that predate the current audit.

Verified in the current tree (2026-10-18): **258 workspace tests pass**, including 13 checked-in
raw-socket end-to-end tests. Chunked request framing, pipelining, smuggling defenses, 405
`Allow` generation, panic isolation, CORS, compression, duplicate header retention, multipart
uploads, dynamic static files with conditional and range requests, JWT validation, graceful
shutdown, RFC 6455 WebSocket upgrade/echo/close, typed extractors (`Path`/`Query`/`Json`/
`Form`/`State`), user-defined and group-scoped middleware, the `TestClient` integration
harness, and opt-in adaptive scheduling (bounded admission queues, 503 load shedding,
per-kind capacity, baseline-derived work quotas — `aether-scheduler`, design and measured
policy comparison in `framework/docs/`) were exercised with unit tests, negative tests, and
wire-level tests. WebSocket compression and HTTP/2/HTTP/3 are not implemented.

## Quick start

```bash
cd framework
cargo test --workspace
cargo run --release --bin basic_api     # http://127.0.0.1:3000
```

The verified toolchain is Rust 1.98.1 (edition 2024). See
[`framework/README.md`](framework/README.md) for application examples, configuration, and
security notes.

## Repository layout

| Path | Contents |
|---|---|
| `framework/` | Cargo workspace: 18 crates (`aether`, `aether-http`, `aether-router`, `aether-scheduler`, …) |
| `framework/docs/` | Architecture, API, deployment, audit, and benchmark documents |
| `docs/` | Specification and design documents (Arabic and English) |
| `src/`, `index.html`, `package.json` | A separate Vite/React site that presents the framework. It is **not** part of the Rust build and requires Node.js (`npm install`, `npm run dev`). |

## License

MIT OR Apache-2.0.
