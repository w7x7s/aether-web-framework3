# Adaptive Tree Scheduling — Executive Design

Status: DESIGN + IMPLEMENTATION (REQ-0001, scheduling track). 2026-10-18.
Scope: `aether-scheduler` crate (protocol-independent) + opt-in integration in
`framework/aether/src/scheduling.rs`. This document is the contract for what the
code claims and what it does NOT claim.

## 1. Problem and placement (from the architecture audit)

Current serve path (all in `framework/aether/src/app.rs`):

- One `tokio::sync::Semaphore(max_connections)` is acquired **blocking** before
  `listener.accept()` (app.rs:847, :873). Over-limit connections wait forever and
  the listen loop stalls accepting; there is no bounded queue and no rejection.
- Handler execution is spawned per request (app.rs:1157) with **no independent
  limit**: handler concurrency ≤ `max_connections` only implicitly.
- `aether-runtime::handle_connection` is dead code; the scheduler does not touch it.
- Shutdown is a `tokio::sync::watch<bool>` (app.rs:848); buffers are per-connection
  8 KiB reads + `conn_buf` capped at `max_header_bytes + max_body_bytes`.

Integration points chosen:

1. **Connection admission** replaces the blocking semaphore when scheduling is
   enabled (opt-in `AetherApp::with_scheduling`). Bounded wait queue; overflow is
   rejected with a protocol-valid `503 + Connection: close` before any handler runs.
2. **Handler execution gating**: a permit is taken before `tokio::spawn(handler)`;
   full queue ⇒ immediate `503` response (no headers sent yet, so semantics are safe).
3. **Divisible-work driver** (generic API, not HTTP-specific): long CPU work that
   yields at safe points runs slices under a per-round quota.

When scheduling is disabled (the default), the serve loop is byte-for-byte the
current behavior. No existing public API changes; additions only.

## 2. Architecture

```
aether-scheduler (NO tokio, NO HTTP, NO protocol knowledge)
 ├─ job.rs        JobId, Work, JobKind, JobState, JobRecord
 ├─ baseline.rs   robust baseline (median ring), large classification with hysteresis
 ├─ quota.rs      large-job per-round quota math (all formulas below)
 ├─ tree.rs       scheduling tree: root(class index) → class nodes → job leaves
 ├─ policy.rs     SchedulingPolicy trait + Fifo / RoundRobin / Deficit / AdaptiveTree
 ├─ resources.rs  ResourceManager: measured vs configured budgets, hysteresis adapt
 ├─ engine.rs     SchedulerEngine<P>: admit/select/report/complete/cancel + metrics
 └─ error.rs      RejectReason, Decision (with reason), SchedulerError

aether (integration layer, tokio-aware)
 ├─ scheduling.rs SchedulingLayer: admission guards, handler gate, 503 overflow,
 │                RAII release (Drop), cancellation & timeout wiring
 └─ app.rs        with_scheduling(cfg) opt-in; serve loop consults Option<&Layer>
```

Invariants:

- The engine is synchronous and pure: every policy call receives an explicit
  `now: Duration` and returns deterministic results. Ties are broken by
  `arrival_seq` (monotonic u64). This is what makes the algorithm testable
  without wall-clock flakiness.
- The scheduler never assumes work↔time equivalence. Drivers report measured
  `(work, elapsed)` per slice; quotas are in abstract work units.
- Nothing is claimed to be preempted unless it is `JobKind::Divisible` and yields
  at a safe point. Indivisible HTTP handler/connection jobs occupy their slot to
  completion — rotation between them comes from admission ordering, not fakery.

## 3. Job model

States:

```
Queued ──admit──▶ Active ──(Divisible: slice ⇄ Yielded)──▶ Completed
   │                 │
   │                 ├──cancel (disconnect/timeout/shutdown drain)──▶ Cancelled
   └──reject (queue full)──▶ Rejected
```

Every non-terminal→terminal transition releases capacity and unregisters memory.
`complete`/`cancel` are idempotent (double-release is impossible). The tokio layer
wraps admission in RAII guards so a panic, abort, or future-drop cannot leak a slot.

`Work` is `u64` work units. `JobKind`: `Connection`, `Handler`, `CpuHeavy`,
`Divisible`. `work_hint: Option<Work>` lets callers declare expected work when the
protocol knows it (e.g. static file size, declared body length).

## 4. Adaptive Tree algorithm

The "tree": root node indexes three class nodes — `SMALL`, `LARGE`, `UNKNOWN` —
and job leaves hang under their class. A **round** is one deterministic traversal:

1. Purge terminal leaves; refresh gauges.
2. `ResourceManager::refresh(now)` — capacity may change, but at most once per
   `adapt_interval` (hysteresis, §6).
3. **Starvation guard**: queued jobs with `wait ≥ starvation_guard` are promoted to
   the head of the selection order, oldest first. This overrides *ordering*, never
   *capacity*: promotion cannot exceed resource limits.
4. Admission (arrival precedence): walk queued jobs by `arrival_seq`; admit while
   an execution slot and registered memory budget fit. Overflow: when the queue is
   at `max_queue`, new arrivals are `Rejected` (503). No silent dropping.
5. Execution order for Active divisible jobs: round-robin by arrival sequence;
   each due job runs one slice with quota `q_j(§5)`, then yields at its next safe
   point and re-queues at the tail of the active rotation. Indivisible jobs hold
   their slot until completion.
6. Round budget: `Σ q_j ≤ round_work_budget = active_slots × baseline × round_factor`;
   jobs that would exceed it wait for the next round (bounded, they keep their
   arrival precedence, so this cannot starve them past the guard).

### 4.1 Stable baseline

- `B` = median of the last `N = 32` completed **non-large** jobs' work.
- Completed LARGE jobs never enter the sample, so one huge job cannot destabilize
  the baseline; the median additionally discards outliers inside the sample.
- Before `min_samples = 4` samples, `B` falls back to configured `default_baseline`.
- Unknown-size jobs (no hint and no history) are admitted as `UNKNOWN`, capped to
  one baseline of work per slice; their cumulative executed work is re-classified
  after every slice, so unknowns converge to a class without any protocol guess.
- Sudden load shift: because the sample is a recency window, `B` re-anchors within
  ≤ N small completions; the dual-threshold hysteresis below prevents the class
  boundary itself from oscillating.

### 4.2 Large classification (hysteresis, no flapping)

Let `W_j` be `work_hint` if present, else cumulative executed work so far.

- Becomes LARGE when `W_j > large_factor_hi × B` (default `hi = 4.0`).
- Becomes non-large only when `W_j < large_factor_lo × B` (default `lo = 1.5`).
- Inside the dead band `[lo×B, hi×B]` the previous classification is kept.
- HTTP body size alone is NOT the classifier: hints are advisory and any job-type
  driver may supply its own measure of work; classification always re-checks
  against *executed* work.

### 4.3 Large-job work quota per round

Per active large job `j`:

- Reference share: `q0 = quota_alpha × B` (default `quota_alpha = 1.0` — one
  baseline chunk per round).
- Observed rate (work/ms) `r_j` = EWMA over measured slices:
  `r_j ← (1−λ)·r_j + λ·(w_slice / max(t_slice,1ms))`, `λ = 0.3`; until the first
  measurement assume `r_j = B / target_slice_ms`.
- Time fit: `q_time = target_slice_ms × r_j` (default `target_slice_ms = 10`).
- Base quota: `q = clamp(q0, min(q_time, quota_max × q0), min(quota_min × q0, remaining), remaining)`
  with defaults `quota_min = 0.25`, `quota_max = 4.0`.
- Wait boost (anti-starvation of big jobs): `q ×= 1 + wait_boost × min(wait/starvation_guard, 1)`,
  default `wait_boost = 0.5`.
- Adaptation, applied on the previous slice outcome:
  - slice duration `> 2 × target` ⇒ `q ×= 0.5` (fast decrease);
  - slice duration `< 0.5 × target` and job behind its fair share ⇒ `q ×= 1.25`
    (slow ramp); at most one ramp step per round.
- Hard caps: `q ≤ remaining_work`, and the whole round obeys `round_work_budget`
  (step 6 above), so catch-up never breaches resource limits.

Documented limits of the design: the baseline is not a size ceiling for requests
and quota is not a completion-time guarantee; large jobs finish in ≥ remaining/B'
rounds where B' depends on measured throughput. Progress of every admitted large
job is guaranteed ≥ `quota_min × q0` per round while capacity exists (tested).

## 5. Compared policies (same engine, `SchedulingPolicy` trait)

- `Fifo` — strict arrival order, run-to-completion, no preemption, no quota.
- `RoundRobin` — active jobs rotate one fixed share (`B`), arrival order preserved
  between equal-status jobs; indivisible jobs still run-to-completion.
- `Deficit` (fair comparator) — deficit round-robin: each job gets `quantum +
  accrued deficit`; unused quantum accrues, surplus is carried.
- `AdaptiveTree` — §4.

All four share admission, queue bounds, rejection and resource caps, so the only
variable in a comparison is the selection/quota logic.

## 6. ResourceManager

Separates, in types, what is **measured** from what is **configured**:

- Measured by the platform: `available_parallelism()`; framework-tracked gauges —
  open connections, running handlers, cpu jobs, registered in-flight bytes
  (the framework knows its own buffer sizes exactly).
- Configured (never presented as measurements): `max_connections`,
  `max_active_handlers` (default `4 × cores`), `max_cpu_heavy` (default `cores`),
  `max_active_divisible` (default 4), `memory_budget_bytes` (default 256 MiB — the
  OS gives us no reliable free-memory figure, so this is a registered-bytes
  ceiling, not a claim about RAM), `max_queue` (default 1024).
- **Divisible-slot trade-off (decided during integration):** a `Divisible` job
  reserves one slot from its own static budget for its entire lifetime and is
  never re-admitted between slices. Re-admission would expose a long background
  job to queue-full rejection mid-flight; slot reservation would let it starve
  HTTP handler capacity if the budgets were shared. Separate budgets keep both
  honest: rotation (quota) governs *how fast* divisible jobs progress, slots
  govern *how many* run at once, and handler capacity is unaffected.
- Adaptation with hysteresis (anti-oscillation):
  - `pressure = clamp(queue_len/max_queue + reject_rate_window, 0, 2)`
  - pressure high for ≥ `sustain_interval` ⇒ `capacity ×= 0.5` (floor `min_slots = 2`);
  - pressure low for ≥ `sustain_interval` ⇒ `capacity += 1` (ceiling configured max);
  - at most one change per `adapt_interval` (default 1 s).
- Overload protection: burst admission is bounded by `max_queue`; rejection is the
  only overflow behavior (no shed-with-priority guessing), and `503` carries
  `Retry-After: 1`.

## 7. Protocol integration honesty

- HTTP/1.1 keep-alive, pipelining, byte ordering and framing are untouched: the
  scheduler gates *whole requests* and *connections*, never mid-parse states.
- A handler is never preempted: HTTP handlers are indivisible; rotation comes from
  admission ordering + bounded slots. `Divisible` jobs are a separate, explicit
  integration interface (`SchedulerDriver`) for caller code that has safe yield
  points; nothing in the HTTP parser or WebSocket driver claims preemption.
- Cancellation: client disconnect, request timeout (504 path) and shutdown drain
  all map to `cancel`, releasing slots and memory exactly once.
- WebSocket sessions: counted against connection capacity, not handler capacity,
  for their (long) lifetime — documented and tested.

## 8. Failure policy

- No unbounded retries, ever; rejected work is reported, not requeued by the server.
- A cancelled job is never rescheduled as if healthy (terminal states are sticky).
- Engine bugs cannot translate into lost requests: admission is fail-open on
  `SchedulerError` (a scheduler that errors internally degrades to the plain
  semaphore behavior and increments `engine_errors` — visible in metrics).

## 9. Verification plan (what actually gets measured)

1. Deterministic unit tests of §4/§5/§6 with a virtual clock (no sleeps).
2. Seeded simulator (`tests/sim.rs`, same LCG seeds for all four policies):
   small/medium/large/unknown/burst mixes, closed arrival rate, limited capacity.
   Metrics: mean & p95 & p99 completion time, max wait, starvation count
   (wait > guard), throughput, rejections, peak queue, capacity-change count
   (adaptation stability). Numbers land in `docs/SCHEDULING_COMPARISON.md` —
   including results where AdaptiveTree is worse.
3. Tokio integration tests: admission bounds, 503 overflow, no slot/memory leaks
   after load, cancellation on disconnect/timeout, graceful shutdown under load.
4. Wire tests against a real bound port with scheduling enabled and default-off —
   responses/byte-order identical to today's engine; the pre-existing full test
   suite must pass unchanged.
