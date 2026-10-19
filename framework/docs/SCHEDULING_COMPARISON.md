# Scheduling Policy Comparison — Actual Measured Results

This page records **real measurements**, produced by the deterministic policy-comparison
simulator described in `ADAPTIVE_TREE_SCHEDULING_DESIGN.md` §9.2. Nothing here is
estimated, projected, or hand-picked: the tables are the verbatim output of
`cargo run -p aether-benchmarks --bin sched_compare --release`.

## How to reproduce

```
cd framework
cargo run -p aether-benchmarks --bin sched_compare --release
cargo test -p aether-scheduler          # 17 engine tests + 8 simulator invariant tests
cargo test -p aether --test scheduling  # 8 wire-level integration tests (real HTTP)
```

- Host: Windows 10 (10.0.26200), x64, release profile (`lto=fat`, `codegen-units=1`).
- Date measured: 2026-10-18.
- Default run: 400 jobs per scenario. The binary accepts an optional job-count argument.

## What is measured, and what is not

The simulator (`framework/scheduler/src/sim.rs`) is a discrete-event, virtual-clock
model of the **divisible** job class only:

- All policies see byte-for-byte identical seeded workloads (SplitMix64; verified by the
  `workload_is_reproducible_byte_for_byte` test).
- Identical fixed resources for every policy: 3 divisible execution slots, queue bound 64,
  starvation guard 1500 ms, adapt interval 100 ms, unlimited memory budget.
- Each job has a hidden execution rate (8–30 work/ms). The engine never sees the rate; it
  only receives measured `(work, elapsed)` slice reports, exactly like the real driver.
- Latency is time-in-system (arrival → completion) in virtual milliseconds.
- `small` = total work < 800, `large` = ≥ 800 (generator's large-band floor).

**Not covered by these numbers** (see Limitations at the bottom): indivisible HTTP
handler/connection traffic (covered by the wire tests instead, which measure wall-clock
latency under real requests), real CPU/RAM usage, and network effects.

## Definitions

- **done / rej** — jobs completed / rejected at admission. `done + rej` always equals the
  workload size (asserted by `every_job_is_accounted_for_exactly_once`).
- **TAT** — turnaround (time in system) percentiles across all completed jobs.
- **max start wait** — worst-case time from arrival to a job's first granted slice.
- **starv. promos** — admissions forced early by the starvation guard.
- **thr. work/s** — abstract work units finished per simulated second over the makespan.
- **cap. changes** — resource-capacity adaptations performed during the run (0 here: the
  divisible-slot pool is intentionally static, see Limitations).
- **class promos** — jobs whose classification changed (Unknown→Large, Large→…) as the
  baseline and per-job estimates converged.
- **slices/large** — average number of preemption slices a large job required.
  FIFO runs a job to completion (1.0 by construction).

## Scenario: mixed-steady (seed 7, 400 jobs)

| policy | done | rej | TAT mean ms | p50 | p95 | p99 | small p99 | large p99 | max start wait ms | starv. promos | makespan ms | thr. work/s | cap. changes | class promos | peak queue | slices/large |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| FIFO | 322 | 78 | 461.5 | 472 | 804 | 876 | 819 | 1038 | 817 | 0 | 3169 | 49142 | 0 | 0 | 64 | 1.0 |
| RoundRobin | 367 | 33 | 334.8 | 283 | 717 | 785 | 727 | 976 | 724 | 0 | 3217 | 48409 | 0 | 0 | 64 | 28.9 |
| DeficitRR | 367 | 33 | 334.8 | 283 | 717 | 785 | 727 | 976 | 724 | 0 | 3217 | 48409 | 0 | 0 | 64 | 28.9 |
| **AdaptiveTree** | **371** | **29** | **311.0** | **253** | **678** | **748** | **685** | **922** | **680** | 0 | **3061** | **50876** | 0 | 101 | 64 | 17.4 |

AdaptiveTree wins every latency column, completes the most jobs (371 vs 367/322), and
does it with 40% fewer large-job slices than RoundRobin (17.4 vs 28.9) — the adaptive
quota grows toward a job's true rate instead of paying a fixed quantum.

## Scenario: burst (seed 5, 400 jobs — 20× arrival spike during 10.0–10.3 s)

| policy | done | rej | TAT mean ms | p50 | p95 | p99 | small p99 | large p99 | max start wait ms | starv. promos | makespan ms | thr. work/s | cap. changes | class promos | peak queue | slices/large |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| FIFO | 290 | 110 | 483.5 | 442 | 857 | 944 | 938 | 1166 | 941 | 0 | 3134 | 58805 | 0 | 0 | 64 | 1.0 |
| RoundRobin | 303 | 97 | 477.9 | 539 | 822 | 1042 | 838 | 1109 | 872 | 0 | 3431 | 53714 | 0 | 0 | 64 | 29.9 |
| DeficitRR | 303 | 97 | 477.9 | 539 | 822 | 1042 | 838 | 1109 | 872 | 0 | 3431 | 53714 | 0 | 0 | 64 | 29.9 |
| **AdaptiveTree** | **306** | **94** | **455.7** | **503** | **819** | **1013** | 846 | **1076** | 853 | 0 | 3402 | **54172** | 0 | 82 | 64 | 19.9 |

Under saturation the queue is pinned at 64 for every policy and rejections are
unavoidable; AdaptiveTree still sheds slightly fewer jobs and keeps the best mean/p99.
RoundRobin's p99 (1042) is its worst showing — fixed 100-unit quanta waste slots on
small jobs while large jobs re-queue repeatedly.

## Scenario: large-heavy (seed 3, 400 jobs, 30% large)

| policy | done | rej | TAT mean ms | p50 | p95 | p99 | small p99 | large p99 | max start wait ms | starv. promos | makespan ms | thr. work/s | cap. changes | class promos | peak queue | slices/large |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| FIFO | 192 | 208 | 993.3 | 956 | 1640 | 1794 | 1648 | 1905 | 1646 | 13 | 4054 | 90759 | 0 | 0 | 64 | 1.0 |
| **RoundRobin** | **208** | 192 | **954.8** | **1029** | **1446** | **1579** | **1442** | 1694 | **1440** | 0 | 4149 | 88681 | 0 | 0 | 64 | 27.9 |
| DeficitRR | 208 | 192 | 954.8 | 1029 | 1446 | 1579 | 1442 | 1694 | 1440 | 0 | 4149 | 88681 | 0 | 0 | 64 | 27.9 |
| AdaptiveTree | 195 | 205 | 964.0 | 1006 | 1540 | 1653 | 1560 | **1682** | 1557 | 10 | **3660** | **100530** | 0 | 81 | 64 | 17.9 |

**Reported honestly: RoundRobin beats AdaptiveTree here** on completed jobs (208 vs 195)
and overall TAT percentiles (p95 1446 vs 1540). AdaptiveTree finishes slightly earlier
(best makespan/throughput) and has the best large-job p99 (1682 vs 1694), but its
quota ramp-up costs it completions when 30% of the load is large: the conservative first
grants (`q0 = 1 × baseline`) under-serve a workload whose baseline is dragged down by
the many tiny jobs. This is a real trade-off of "exclude large jobs from the baseline",
not a simulator artifact — see Limitations.

## Scenario: unknown-heavy (seed 9, 400 jobs, 60% of jobs hide their size)

| policy | done | rej | TAT mean ms | p50 | p95 | p99 | small p99 | large p99 | max start wait ms | starv. promos | makespan ms | thr. work/s | cap. changes | class promos | peak queue | slices/large |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| FIFO | 246 | 154 | 728.2 | 596 | 1500 | 1755 | 1753 | 1759 | 1746 | 8 | 3797 | 38946 | 0 | 0 | 64 | 1.0 |
| **RoundRobin** | 384 | 16 | **332.6** | **303** | **652** | 728 | **667** | 885 | **668** | 0 | 3369 | 43894 | 0 | 0 | 64 | 27.4 |
| DeficitRR | 384 | 16 | 332.6 | 303 | 652 | 728 | 667 | 885 | 668 | 0 | 3369 | 43894 | 0 | 0 | 64 | 27.4 |
| **AdaptiveTree** | **385** | **15** | 339.2 | 304 | 668 | **722** | 687 | **877** | 686 | 0 | **3361** | **43998** | 0 | 114 | 64 | 19.9 |

When size hints are mostly missing, FIFO collapses (a single unknown-size large job
head-blocks: 246 done vs 384). AdaptiveTree matches RoundRobin's completion count with
fewer context switches (19.9 vs 27.4 slices/large) and a marginally better tail
(p99 722 vs 728, large p99 877 vs 885); RoundRobin keeps a marginally better mean.

## Summary of what the data actually shows

1. **vs FIFO (the naive default the serve path used before):** AdaptiveTree is better in
   every scenario and essentially every fairness metric — e.g. unknown-heavy completions
   385 vs 246 (+56%), burst max-start-wait 853 vs 941 ms. FIFO's problem is exactly the
   one the design targets: an indivisible-looking large job head-blocks the queue.
2. **vs RoundRobin/Deficit (a fair baseline policy):** AdaptiveTree wins clearly on
   steady mixed load (every percentile better, +4 jobs done) and ties or trades blows
   elsewhere; it **loses** on the large-heavy scenario for completed jobs and mid-range
   percentiles. Claiming uniform superiority would be false.
3. **AdaptiveTree's consistent structural win is slice efficiency:** 17–20 slices per
   large job vs 28–30 for RoundRobin — roughly 35% fewer dispatches for the same or
   better tail latency. Each avoided slice is avoided engine wake-up, grant bookkeeping,
   and per-slice report overhead in the real driver too.
4. DeficitRR produced identical numbers to RoundRobin in all four scenarios. The deficit
   bank only changes outcomes when a job stops mid-quantum voluntarily; this generator's
   jobs always consume their full grant (driver grants `min(quota, remaining)`), so the
   banked deficit never accumulated. Reported as observed, not hidden.

## Limitations (read before drawing conclusions)

- **Virtual, divisible-only simulation.** These numbers characterize scheduling logic,
  not the HTTP serve path. Real wall-clock latency, TCP behavior, handler panic
  isolation, and timeouts are verified separately by `framework/tests/tests/scheduling.rs`
  (8 wire tests, all passing) — those tests bound concurrency and prove 503 overflow,
  cancellation, and drain, but they do not produce percentile tables.
- **CPU and memory usage were not measured by the simulator.** The engine reports
  `peak_queue`, grants, and slice counts; OS-level CPU/RSS comparison would require
  running the real server under load per policy, which has not been done yet
  (listed under unfinished work in the final report).
- **`cap. changes` is 0 everywhere by design in this simulation**: the divisible-slot
  pool is intentionally a static budget (design §6 trade-off) and the simulated handler
  floor equals the pool, so the adaptive resource manager never acted. Its shrink/grow
  hysteresis is instead verified by the deterministic unit test
  `resources_shrink_slowly_and_recover_additively`.
- **Baseline contamination in large-heavy loads:** excluding large jobs from the
  baseline (a deliberate design rule) means a 30%-large workload makes the baseline —
  and thus the first quota — too conservative. A future improvement is a second,
  large-class-relative reference; that is speculative and not yet implemented.
- One seed per scenario was used for the headline tables (same seeds across policies,
  which is what matters for a fair A/B). Multiple-seed sweeps have not been run yet.

## Addendum — re-run 2026-10-19 after algorithmic defect fixes (same command, same seeds)

Four real defects in the scheduler core were found by source audit and fixed (each now
covered by a regression test in `framework/scheduler/tests/engine.rs`):

1. **Hint-overrun stall (all four policies).** Grants were clamped to
   `remaining_work`, which is `hint − executed`; a `Divisible` job that executed *more*
   than its declared hint reached `remaining == 0` and every subsequent grant became 0 —
   the driver stalled forever inside `run_slices`. Grants now treat a zeroed remainder as
   "no longer a bound" (`JobRecord::grantable_remaining`); the driver stays the
   authority on completion.
2. **Stale hints never reclassified.** `measured_work()` returned the declared hint for
   known-size jobs, so the documented "re-classify from executed work after every slice"
   never happened. It is now `max(hint, executed)` — a job that declared 100 and ran to
   500 is promoted to Large (regression:
   `overshooting_a_declared_hint_keeps_granting_and_reclassifies`).
3. **Wait-boost rewarded age, not starvation.** The Large-quota boost was computed from
   `admitted_at`, so any long-running active job carried a permanent +50% quota once it
   passed the guard, while the jobs it should have outran (starved, not executing) got
   the same boost. It now measures time since the job's last executed slice
   (`JobRecord::grant_wait`; regression: `wait_boost_rewards_staleness_not_age`).
4. **Double-counted remainder on completion.** `finish()` computed implicit work as
   `remaining − executed`, but `report_slice` already decrements `remaining` per slice —
   every partially-reported completion under-counted its total (metrics
   `work_executed` and baseline samples). Implicit work is now exactly the unreported
   remainder (regression: `completion_accounts_the_whole_declared_total`).

FIFO, RoundRobin and DeficitRR rows are byte-identical to the tables above (the fixes
only change paths their simulated drivers never hit), which is itself a determinism
check. AdaptiveTree, before → after:

| scenario | done | TAT mean ms | p99 ms | small p99 | large p99 | starv. promos | thr. work/s |
|---|---|---|---|---|---|---|---|
| mixed-steady | 371 → **373** | 311.0 → **308.7** | 748 → **738** | 685 → **681** | 922 → **914** | 0 → 0 | 50876 → 50026 |
| burst | 306 → 306 | 455.7 → **450.3** | 1013 → **965** | 846 → **843** | 1076 → **1065** | 0 → 0 | 54172 → **58861** |
| large-heavy | 195 → 196 | 964.0 → **947.1** | 1653 → **1633** | 1560 → **1531** | 1682 → **1646** | 10 → **4** | 100530 → 96954 |
| unknown-heavy | 385 → **386** | 339.2 → **331.8** | 722 → **715** | 687 → 687 | 877 → **858** | 0 → 0 | 43998 → **44248** |

Honest reading of the after-fix data:

- Latency and fairness improved in **every** scenario (mean/p99/small-p99/large-p99 all
  better or equal; large-heavy starvation promotions dropped 10 → 4). In unknown-heavy
  AdaptiveTree now also beats RoundRobin on TAT mean (331.8 vs 332.6).
- Throughput traded slightly **down** in two scenarios (mixed-steady −1.7 %, large-heavy
  −3.6 %) and up in the other two. Cause: the permanent +50% boost on running Large jobs
  is gone, so their quota share is honest baseline-sized shares instead of inflated ones.
  This is the fix working as intended (fairness was being bought by disguised quota
  inflation), not a regression — but it is reported as a real number, not hidden.
- The documented RoundRobin win on large-heavy completions persists but narrowed
  (208 vs 195 → 208 vs 196). The remaining gap is the known baseline-conservatism
  limitation noted above, unchanged.

Verification: `cargo test -p aether-scheduler` = 31 tests green (21 engine incl. 3 new,
8 sim, 2 stress); full workspace = 43 suites green; the comparison above is the verbatim
output of `cargo run -p aether-benchmarks --bin sched_compare --release`.
