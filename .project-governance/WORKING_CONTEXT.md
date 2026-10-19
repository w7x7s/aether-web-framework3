# Working Context

## Current Goal
Execute REQ-0001: full Aether re-engineering mission (10 phases per vision doc attached 2026-10-18).

## Active Track
Adaptive Tree Scheduling phase (user-requested 2026-10-18). Delivered and verified:
- `aether-scheduler` crate: protocol-independent engine (admission, bounded queues,
  three-class tree, baseline from measurements excluding large jobs, quota slicing,
  starvation guard, adaptive per-kind capacity; FIFO/RR/Deficit/AdaptiveTree policies)
  — 17 engine + 8 simulator-invariant + 2 stress tests
- Opt-in serve-path integration (`AetherApp::with_scheduling`): connection/handler
  admission, 503 + Retry-After shedding, RAII release on panic/abort/disconnect,
  `run_slices` divisible driver — 9 wire tests; default path unchanged
- Windows RST fix on the scheduling reject path (graceful `shutdown()` before close)
- Design doc `framework/docs/ADAPTIVE_TREE_SCHEDULING_DESIGN.md`; measured four-policy
  comparison `framework/docs/SCHEDULING_COMPARISON.md` (real simulator output, including
  the large-heavy scenario where RoundRobin beats AdaptiveTree); example
  `framework/examples/scheduling_demo.rs` run live (20 000-unit job sliced to completion
  while HTTP traffic kept flowing)

Workspace: 258 tests, 0 failures (was 222). Release build clean.

Previous track (Developer Experience, delivered 2026-10-18): `TestClient`, typed
extractors, `use_middleware`.

## Known Breaking Change (documented)
`get_async`/`route_async`/group `*_async` now take `Handler<Args>`: closures taking
`RequestContext` directly must annotate: `|req: RequestContext| async { .. }`. Sync
`get`/`post`/... unchanged.

## Next Candidate Tracks (user priority order TBD)
- Security: rate limiting, CSRF, Host validation
- Streaming request/response bodies
- HTTP/2 (via h2), TLS termination
- Remaining DX: request-scoped extensions, dedicated per-route middleware API

## Last Update
2026-10-18
