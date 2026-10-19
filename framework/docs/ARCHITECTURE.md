# Aether Architecture & Design Specification

> **DESIGN DOCUMENT — 2026-10-18.** Parts of this document describe intended or aspirational
> architecture, not the verified implementation. In particular, the server runs on Tokio's
> multi-threaded work-stealing runtime (not thread-per-core); request bodies and headers are
> copied into `RequestContext` before handlers run; and `RequestArena` is not used by the
> server path. HTTP/2 and HTTP/3 are not implemented. See
> [FEATURE_COMPLETENESS_MATRIX.md](FEATURE_COMPLETENESS_MATRIX.md) for verified status.

## 1. Architectural Philosophy
Aether is an ultra-high-throughput, zero-copy native web framework engineered to combine the developer ergonomics of FastAPI with the zero-allocation performance of Drogon (C++) and the fearless concurrency and memory safety of Rust.

```
                      ┌────────────────────────────┐
                      │    FastAPI-Style DX API    │
                      └─────────────┬──────────────┘
                                    │
                                    ▼
                      ┌────────────────────────────┐
                      │    Precompiled Request     │
                      │       Execution Plan       │
                      └─────────────┬──────────────┘
                                    │
          ┌─────────────────────────┼────────────────────────┐
          ▼                         ▼                        ▼
┌──────────────────┐      ┌──────────────────┐     ┌──────────────────┐
│ Radix Trie       │      │ Compile-Time     │     │ Constant-Time    │
│ Router (Bitsets) │      │ Monomorphic DTO  │     │ Security & RFC   │
└─────────┬────────┘      └─────────┬────────┘     └─────────┬────────┘
          │                         │                        │
          └─────────────────────────┼────────────────────────┘
                                    │
                                    ▼
                      ┌────────────────────────────┐
                      │   Zero-Copy HTTP Engine    │
                      │  (SmallVec + httparse)     │
                      └─────────────┬──────────────┘
                                    │
                                    ▼
                      ┌────────────────────────────┐
                      │ Thread-per-Core Async Core │
                      │  (io_uring / epoll / tokio)│
                      └────────────────────────────┘
```

## 2. Zero-Copy Borrowing Model
Standard frameworks allocate strings and heap vectors for request methods, path tokens, query maps, and headers.
Aether borrows directly from the socket read ring buffer:
- `RequestView<'req>`: Borrows `&'req str` for method, path, headers, and body.
- `SmallVec<[HeaderView<'req>; 16]>`: 16 headers allocated on the stack.
- `RequestArena`: Bump-pointer 4KB arena per worker connection. Resetting on Keep-Alive requests is an $O(1)$ offset zeroing operation.

## 3. Bitset-Accelerated Radix Router
- Routes are organized into a Radix compressed trie.
- Allowed methods are packed into a 64-bit scalar (`u64 bitmask`).
- Checking whether an incoming method is permitted is a single machine instruction:
  `if (node.allowed_methods & (1 << method.bit())) != 0`
- Extracted URL path parameters are placed into a `SmallVec<[Param<'req>; 4]>` directly borrowing slices from the URI string.

## 4. Precompiled Dependency Injection Graph
- Unlike Python FastAPI's dynamic runtime reflection on every request, Aether analyzes all handlers, singletons, database pools, and request-scoped services at application boot.
- Performs topological cycle validation (Kahn's algorithm).
- Generates an inlined DAG execution vector, resolving dependencies in deterministic $O(1)$ index accesses.
