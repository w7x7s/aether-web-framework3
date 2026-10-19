# Aether vs Actix Web — fair-comparison re-benchmark and robustness audit (2026-10-18)

This document supersedes the earlier snapshot-based comparison. It re-runs everything
under unified conditions after fixing the methodology gaps, and records the serve-path
robustness audit with confirmed/possible classification per finding. Raw repeat numbers
are kept verbatim from the measured runs; nothing was averaged in advance.

## 1. Unified conditions (Release vs Release)

- `threekway` (user site on Aether): `cargo build --release` (workspace release profile,
  fat LTO), run via `AETHER_PORT=3002` on `0.0.0.0:3002`.
- `actix_cmp` (Actix Web 4.17): `cargo build --release` (`lto = "fat"`, `codegen-units = 1`),
  bound `0.0.0.0:3001`, default worker count.
- Load client: `loadgen.py` (Python, one keep-alive `HTTPConnection` per thread), identical
  for both servers; 15 s per cell, 3 repeats; response bytes integrity checked per response
  (`mismatch=0` everywhere).

## 2. Route-equivalence finding

`/learning` in `threekway` contains **no SQLite query** — it is a Tera render of
`learning.html` + `base.html` with a single context variable (`active_page = "learning"`),
Tera built once at startup, autoescape default, no custom filters, no session check,
no compression (disabled on the site). The Actix replica renders the same template
directory with the same Tera version and returns the same header set.

Verification: `GET /learning` bodies from `:3002` and `:3001` are SHA-256 identical
(`e9e2e87b5fddb39f0ad7f747818bc3c9ed389daaa599a50bc9203f87a16de8d4`, 205,267 bytes);
CSS is `include_bytes!` of the same 45,832-byte file, also hash-identical.

## 3. Repeated matrix (median of 3, range in parentheses)

| Path | conns | Aether rps | Actix rps | Aether p50/p95/p99 (µs) | Actix p50/p95/p99 (µs) |
|---|---|---|---|---|---|
| css | 1 | 2282 (2208–3236) | 2658 (2490–2746) | 400/708/1119 | 337/682/1002 |
| css | 8 | 3456 (3129–3561) | 3249 (2872–3496) | 2085/4355/6390 | 2219/4658/6634 |
| css | 16 | 3496 (3425–3507) | 2744 (2674–3221) | 4086/8969/11865 | 4504/9400/13127 |
| /learning | 1 | 1388 (1367–1397) | 1412 (1345–1426) | 637/1314/2219 | 579/1397/2186 |
| /learning | 8 | 1984 (1849–2190) | 1949 (1837–1972) | 3706/6934/8616 | 3708/6986/8763 |
| /learning | 16 | 1667 (1625–1813) | 1817 (1447–1920) | 8235/15105/20216 | 8099/16577/25271 |

Errors: `err=0` and HTTP 200 only, in every one of the 36 cells.
CPU (cores consumed by the loaded server, /learning @8): Aether ≈1.35, Actix ≈1.26.
RAM (working set under /learning @16 load): Aether ≈20.1 MB avg / 21.2 max; Actix ≈16.6–17.1 avg / 17.8 max. Idle: Aether 13.6 MB, Actix 12.9 MB.

**Conclusion:** with identical release builds, identical route work and identical client,
the two servers are statistically equivalent (±5–10 % run-to-run noise dominates the
difference). The earlier "Actix +40 %" and "Aether wins @16" claims were both artifacts
of mixed build profiles and of the snapshot-vs-render shortcut; they are retracted.

## 4. Step-load ladder (/learning, 10 s/step, rps)

| conns | 1 | 2 | 4 | 8 | 12 | 16 | 24 | 32 |
|---|---|---|---|---|---|---|---|---|
| Aether | 1604 | 2295 | 2147 | 2064 | 1992 | 1989 | 1863 | 1901 |
| Actix | 1310 | 2160 | 1987 | 1847 | 1937 | 1816 | 1843 | 1893 |
| Aether p50 (µs) | 468 | 717 | 1635 | 3620 | 5658 | 7694 | 12372 | 16062 |
| Aether CPU (cores) | 0.54 | 0.85 | 1.22 | 1.28 | 1.30 | 1.45 | 1.41 | 1.28 |

Both saturate at ~1.2–1.45 cores at ~1.9–2.0 k rps for a 205 KB Tera-rendered page
(~0.65 ms server CPU per request); beyond saturation only latency grows, throughput is
flat and failures stay zero up to 32 keep-alive connections. The cap is the workload +
the shared laptop (load generator runs on the same 4 cores), not the Aether connection
engine: static 45 KB files reach 3.5 k rps at ~0.57 cores.

## 5. Soak (180 s @12 connections, /learning)

| | requests | rps | errors | CPU cores | RAM avg/max | RAM after idle |
|---|---|---|---|---|---|---|
| Aether | 317,319 | 1763 | 0 | 1.37 | 19.3/20.4 MB | 13.6 MB |
| Actix | 334,983 | 1861 | 0 | 1.28 | 16.9/17.8 MB | 12.9 MB |

No throughput decay, no growth trend, working set returns to baseline after load →
no leak detected in 3 minutes of sustained load (limitation: 3 min is not hours).

## 6. Serve-path robustness audit (findings and status)

| # | Finding | Classification | Evidence | Status / fix |
|---|---|---|---|---|
| F1 | Error-path closes (408/431/400/503) `return` without graceful FIN; Windows RST discards the just-written response when request bytes are still unread in the receive buffer | **Confirmed (live)** | `rst_probe.py` on release :3002: `WinError 10054` for both 431 and 400 cases; Actix delivered its errors. After fix: probe returns `HTTP/1.1 431…` / `HTTP/1.1 400…` | Fixed in `aether/src/app.rs`: `stream.shutdown().await` after 408, parse-error 400/413/431, `Connection: close` keep-alive exit, and WS-handshake write failure. Regression test `error_responses_survive_unread_pipelined_bytes` in `tests/http_e2e.rs`. Full suite green (259 tests) |
| F2 | Error-response writes were un-bounded awaits (no write timeout) — a never-reading client pins the task+permit | **Confirmed at code level** (pre-fix) | old `app.rs:1336/1350`: bare `write_all().await` | Fixed together with F1: writes wrapped in `tokio::time::timeout(write_timeout, …)`. Runtime torture test (stalled client) not executed — preventive fix |
| F3 | Mid-response IO error treated as success: `timeout(...).is_err()` ignores `Ok(Err(io))`, keep-alive loop continues on a dead socket | **Confirmed at code level** | `app.rs:1311` pattern vs correct `map_or(true, …)` at `app.rs:1170` | Fixed to `map_or(true, \|r\| r.is_err())`. Wire-level repro test not executed |
| F4 | `SchedulingLayer::admit` not cancellation-safe: if the awaiting future is dropped at the internal `timeout(...).await` (task abort), the queued job is never cancelled → permanent `queued` inflation | **Possible** (code path traced; not reproduced live) | `aether/src/scheduling.rs:154–175`; serve path does not wrap `admit` in a select/timeout, so exposure is limited to shutdown-abort scenarios | Proposed: RAII `QueuedGuard` that cancels the job if dropped before activation. Required test: saturate queue, abort waiting tasks, assert `queued_len()` returns to baseline |
| F5 | `static_file` route serves via `std::fs::read` on the async worker (blocking syscall) | **Confirmed at code level**; **no impact on the user site** (it uses `static_asset`/`from_static`; `static_dir` uses `tokio::fs`) | `framework/http/src/response.rs:76`, reached from `app.rs:678` | Proposed: `spawn_blocking` or an async `Response::file_async`. Slow/NFS/AV-locked-disk latency impact remains **possible** |
| F6 | Keep-alive conformance: no max-requests-per-connection; HTTP/1.0 requests kept alive without `Connection: keep-alive`; multi-value `Connection: keep-alive, close` missed; `keep_alive_timeout` (60 s) used only for shutdown drain (doc at `app.rs:855`), idle close is actually `read_timeout` 5 s | **Confirmed (live for HTTP/1.0)** | raw-socket probe: HTTP/1.0 request → socket still open 3 s later (RFC 9112 requires close); grep shows `keep_alive_timeout` referenced only at `app.rs:1425` | Proposed: honor `version` in `close_requested`; treat `keep_alive_timeout` as idle deadline or rename it in docs |
| F7 | WebSocket session has no recv timeout; stalled peer pins the connection task until shutdown | **Possible** (code-level, `connection.rs:305–307`) | not exercised live | Proposed: activity deadline or ping/pong keepalive |

Checked and found correct (no action): request-parser smuggling defenses (TE.CL/CL.TE,
duplicate TE, chunk bounds), memory bounds on `conn_buf`/chunked bodies, slow-loris
deadlines (per-read + whole-request), `AdmissionGuard` RAII release on all exits,
fail-open on mutex poisoning, no network-reachable `unwrap/expect`, shutdown drain
task tracking, HEAD semantics, pipelined-buffer accounting.

## 7. Retained previous results (for the record — now invalidated for comparison use)

Debug Aether (:3000) vs Debug Actix snapshot (:3001): css 923/1055, 1564/2191, 2006/1907 rps;
learning 356/620, 964/928, 1082/1060 rps (1/8/16 conns). Release Actix snapshot vs Debug
Aether: css 1597/1692/2059, learning 1073/1288/1447. Flask vs debug Aether: Aether ≈7–10×
Flask throughput (Flask GIL-capped ≈200 rps), 23 MB vs 45 MB RAM. All of these mixed build
profiles and/or compared snapshot-serving against live rendering; §3 replaces them.
