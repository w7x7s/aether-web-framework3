//! Deterministic discrete-event simulator used by tests and the policy-comparison
//! benchmark (design §9.2). All policies run identical seeded workloads on
//! identical resources; the only variable is the scheduling logic.
//!
//! The simulated work is `Divisible` jobs with safe yield points (per-slice wall
//! time derived from a per-job hidden rate). Indivisible HTTP traffic is
//! deliberately NOT simulated here — the scheduler cannot rotate it by design,
//! so policy differences in the serve path are covered by the wire tests instead.

use std::collections::{BinaryHeap, HashMap};
use std::cmp::Reverse;
use std::time::Duration;

use crate::config::{PolicyKind, ResourceConfig, SchedulingConfig};
use crate::engine::SchedulerEngine;
use crate::job::{JobId, NewJob, Work};

/// One seeded job: arrival time, true total work, hidden execution rate
/// (work/ms), and whether the driver declares a size hint.
#[derive(Debug, Clone, Copy)]
pub struct JobSpec {
    pub arrival_ms: u64,
    pub work: Work,
    pub rate: f64,
    pub known: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    MixedSteady,
    Burst,
    LargeHeavy,
    UnknownHeavy,
}

impl Scenario {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::MixedSteady => "mixed-steady",
            Self::Burst => "burst",
            Self::LargeHeavy => "large-heavy",
            Self::UnknownHeavy => "unknown-heavy",
        }
    }
}

/// Jobs at or above this total work count as "large" for result reporting
/// (matches the generator's large band floor).
pub const LARGE_WORK: Work = 800;

/// SplitMix64 — deterministic across platforms and Rust versions.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed ^ 0x1234_5677_89AB_CDEF)
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn below(&mut self, n: Work) -> Work {
        if n == 0 { 0 } else { self.next_u64() % n }
    }
    fn exponential(&mut self, mean: f64) -> f64 {
        -mean * (1.0 - self.f64()).ln()
    }
}

/// Build one workload: small (5-95), medium (100-480), and a scenario-sized
/// share of large jobs (800-4800); `unknown_share` of the jobs hide their size
/// hint. Burst scenario arrives 20× faster inside a 300 ms window. Same seed ⇒
/// same job list, byte for byte, for every policy.
#[must_use]
pub fn workload(seed: u64, scenario: Scenario, count: usize) -> Vec<JobSpec> {
    let mut rng = Rng::new(seed ^ ((scenario as u64) << 32));
    let (large_share, unknown_share) = match scenario {
        Scenario::MixedSteady => (0.10, 0.15),
        Scenario::Burst => (0.12, 0.15),
        Scenario::LargeHeavy => (0.30, 0.10),
        Scenario::UnknownHeavy => (0.12, 0.60),
    };
    let base_gap_ms = 6.0;
    let mut jobs = Vec::with_capacity(count);
    let mut t = 0u64;
    for _ in 0..count {
        let mut gap = rng.exponential(base_gap_ms).max(1.0);
        if scenario == Scenario::Burst && (10_000..10_300).contains(&t) {
            gap *= 0.05;
        }
        t = t.saturating_add(gap.round() as u64);
        let r = rng.f64();
        let work = if r < large_share {
            LARGE_WORK + rng.below(4_000)
        } else if r < large_share + 0.20 {
            100 + rng.below(380)
        } else {
            5 + rng.below(90)
        };
        let rate = 8.0 + rng.f64() * 22.0;
        let known = rng.f64() >= unknown_share;
        jobs.push(JobSpec { arrival_ms: t, work, rate, known });
    }
    jobs
}

#[derive(Debug, Clone)]
pub struct SimResult {
    pub policy: &'static str,
    pub scenario: &'static str,
    pub completed: u64,
    pub rejected: u64,
    pub tat_mean_ms: f64,
    pub tat_p50_ms: u64,
    pub tat_p95_ms: u64,
    pub tat_p99_ms: u64,
    pub small_p99_ms: u64,
    pub large_p99_ms: u64,
    pub max_start_wait_ms: u64,
    pub starvation_promotions: u64,
    pub makespan_ms: u64,
    pub throughput_work_per_s: f64,
    pub capacity_changes: u64,
    pub class_promotions: u64,
    pub peak_queue: usize,
    pub avg_slices_per_large: f64,
}

struct SimJobState {
    rate: f64,
    work_total: Work,
    executed: Work,
    arrival_ms: u64,
    busy_until: Option<u64>,
    slices: u64,
}

/// Run one policy over one workload. Fixed resources for every policy:
/// 3 divisible execution slots, queue bound 64, virtual millisecond ticks.
#[must_use]
pub fn run(policy: PolicyKind, jobs: &[JobSpec]) -> SimResult {
    let scfg = SchedulingConfig {
        policy,
        max_queue: 64,
        starvation_guard: Duration::from_millis(1_500),
        ..SchedulingConfig::default()
    };
    let rcfg = ResourceConfig {
        max_active_handlers: Some(3),
        min_slots: 3,
        max_active_divisible: Some(3),
        memory_budget_bytes: Work::MAX / 2,
        adapt_interval: Duration::from_millis(100),
        ..ResourceConfig::default()
    };
    let mut e = SchedulerEngine::new(scfg, rcfg);
    let mut states: HashMap<JobId, SimJobState> = HashMap::new();
    // (finish_ms, start_ms, job, work) min-heap on finish time.
    let mut heap: BinaryHeap<Reverse<(u64, u64, u64, Work)>> = BinaryHeap::new();
    let mut next_arrival = 0usize;
    let mut now_ms: u64 = 0;
    let mut tats: Vec<(u64, Work)> = Vec::new();
    let mut completed = 0u64;
    let mut rejected = 0u64;
    let mut max_start_wait = 0u64;
    let mut large_slices = 0u64;
    let mut large_completed = 0u64;

    loop {
        let mut candidates = vec![now_ms + 25];
        if let Some(j) = jobs.get(next_arrival) {
            candidates.push(j.arrival_ms);
        }
        if let Some(&Reverse((t, _, _, _))) = heap.peek() {
            candidates.push(t);
        }
        now_ms = *candidates.iter().min().unwrap();
        if now_ms > 20_000_000 {
            break; // safety bound against simulator livelock
        }
        let now = Duration::from_millis(now_ms);

        while next_arrival < jobs.len() && jobs[next_arrival].arrival_ms <= now_ms {
            let spec = jobs[next_arrival];
            next_arrival += 1;
            let hint = if spec.known { Some(spec.work) } else { None };
            if let Ok(id) = e.admit(NewJob::divisible(hint, 0), now) {
                states.insert(id, SimJobState {
                    rate: spec.rate,
                    work_total: spec.work,
                    executed: 0,
                    arrival_ms: spec.arrival_ms,
                    busy_until: None,
                    slices: 0,
                });
            } else {
                rejected += 1;
            }
        }

        while let Some(&Reverse((finish, start, raw_id, work))) = heap.peek() {
            if finish > now_ms {
                break;
            }
            heap.pop();
            let id = JobId(raw_id);
            let Some(st) = states.get_mut(&id) else { continue };
            if st.busy_until != Some(finish) {
                continue; // stale bookkeeping entry
            }
            st.busy_until = None;
            st.executed = st.executed.saturating_add(work);
            st.slices += 1;
            if st.work_total >= LARGE_WORK {
                large_slices += 1;
            }
            let elapsed = Duration::from_millis(finish.saturating_sub(start).max(1));
            e.report_slice(id, work, elapsed, now);
            if st.executed >= st.work_total {
                let arrival = st.arrival_ms;
                let total = st.work_total;
                e.complete(id, now);
                states.remove(&id);
                tats.push((now_ms.saturating_sub(arrival), total));
                completed += 1;
                if total >= LARGE_WORK {
                    large_completed += 1;
                }
            }
        }

        let out = e.pump(now);
        for id in &out.admitted {
            if let Some(st) = states.get_mut(id) {
                let _ = st;
                let w = now_ms.saturating_sub(states[id].arrival_ms);
                max_start_wait = max_start_wait.max(w);
            }
        }
        for g in &out.grants {
            let Some(st) = states.get_mut(&g.job) else { continue };
            if st.busy_until.is_some() {
                continue; // a slice is already in flight
            }
            let work = g.quota.min(st.work_total.saturating_sub(st.executed));
            if work == 0 {
                continue;
            }
            let dur = ((work as f64 / st.rate.max(0.01)).ceil() as u64).max(1);
            let finish = now_ms + dur;
            heap.push(Reverse((finish, now_ms, g.job.0, work)));
            st.busy_until = Some(finish);
        }

        if next_arrival >= jobs.len() && heap.is_empty() && states.is_empty() {
            break;
        }
    }

    let mut t: Vec<u64> = tats.iter().map(|(x, _)| *x).collect();
    t.sort_unstable();
    let mean = if t.is_empty() { 0.0 } else { t.iter().sum::<u64>() as f64 / t.len() as f64 };
    let mut small: Vec<u64> = tats.iter().filter(|(_, w)| *w < LARGE_WORK).map(|(x, _)| *x).collect();
    small.sort_unstable();
    let mut large: Vec<u64> = tats.iter().filter(|(_, w)| *w >= LARGE_WORK).map(|(x, _)| *x).collect();
    large.sort_unstable();
    let total_work: Work = jobs.iter().map(|j| j.work).sum();
    let makespan = now_ms.max(1);
    let res = e.resources();
    let m = e.metrics();
    SimResult {
        policy: e.policy_name(),
        scenario: "",
        completed,
        rejected,
        tat_mean_ms: mean,
        tat_p50_ms: percentile(&t, 0.50),
        tat_p95_ms: percentile(&t, 0.95),
        tat_p99_ms: percentile(&t, 0.99),
        small_p99_ms: percentile(&small, 0.99),
        large_p99_ms: percentile(&large, 0.99),
        max_start_wait_ms: max_start_wait,
        starvation_promotions: m.starvation_promotions,
        makespan_ms: makespan,
        throughput_work_per_s: total_work as f64 * 1000.0 / makespan as f64,
        capacity_changes: res.capacity_changes as u64,
        class_promotions: m.class_promotions,
        peak_queue: m.peak_queue,
        avg_slices_per_large: if large_completed > 0 {
            large_slices as f64 / large_completed as f64
        } else {
            0.0
        },
    }
}

#[must_use]
pub fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx]
}
