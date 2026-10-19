//! Stress tests for the scheduling engine: high-volume mixed-kind workloads and a
//! cancellation storm. These verify the security/health requirements (design §8):
//! no task or slot leaks, capacity returns exactly to baseline once all jobs are
//! terminal, and accounting is conserved (admitted == completed + cancelled).

use std::collections::HashMap;
use std::time::Duration;

use aether_scheduler::{JobId, JobState, NewJob, PolicyKind, ResourceConfig, SchedulingConfig, SchedulerEngine, Work};

/// Small deterministic LCG — tests must not depend on wall-clock randomness.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

#[derive(Clone, Copy)]
struct Local {
    divisible: bool,
    work_left: Work,
    rate: Work, // work units executed per ms by the simulated driver
    finish_at_ms: u64,
}

fn engine() -> SchedulerEngine {
    let scfg = SchedulingConfig { policy: PolicyKind::AdaptiveTree, max_queue: 1_024, ..Default::default() };
    let rcfg = ResourceConfig {
        max_connections: 256,
        max_active_handlers: Some(8),
        max_cpu_heavy: Some(2),
        max_active_divisible: Some(4),
        min_slots: 8,
        memory_budget_bytes: 16 * 1024 * 1024,
        adapt_interval: Duration::from_millis(50),
        ..Default::default()
    };
    SchedulerEngine::new(scfg, rcfg)
}

/// Arrive 5000 mixed-kind jobs, drive the engine on a virtual millisecond clock
/// until every job is terminal, then prove zero leaks and exact conservation.
#[test]
fn stress_mixed_kinds_5000_jobs_no_leaks() {
    let mut e = engine();
    let mut rng = Lcg(0xDEADBEEF);
    let mut states: HashMap<JobId, Local> = HashMap::new();
    let mut to_arrive = 5_000u64;
    let mut rejected = 0u64;
    let mut now_ms = 0u64;

    loop {
        now_ms += 1;
        let now = Duration::from_millis(now_ms);

        while to_arrive > 0 {
            let roll = rng.below(10);
            let mem = rng.below(40_000) * 64; // 0..2.5 MB per job
            let (req, local) = match roll {
                0..=3 => {
                    (NewJob::connection(mem), Local { divisible: false, work_left: 0, rate: 0, finish_at_ms: now_ms + 3 + rng.below(25) })
                }
                4..=6 => {
                    (NewJob::handler(mem), Local { divisible: false, work_left: 0, rate: 0, finish_at_ms: now_ms + 3 + rng.below(25) })
                }
                7 => {
                    (NewJob::cpu_heavy(mem), Local { divisible: false, work_left: 0, rate: 0, finish_at_ms: now_ms + 3 + rng.below(25) })
                }
                _ => {
                    let work = 50 + rng.below(2_000);
                    (NewJob::divisible(Some(work), mem), Local { divisible: true, work_left: work, rate: 10 + rng.below(20), finish_at_ms: u64::MAX })
                }
            };
            if let Ok(id) = e.admit(req, now) {
                states.insert(id, local);
            } else {
                rejected += 1;
            }
            to_arrive -= 1;
            // Admit a bounded burst per tick so the queue actually saturates.
            if rng.below(3) == 0 {
                break;
            }
        }

        let out = e.pump(now);
        for g in &out.grants {
            let Some(st) = states.get_mut(&g.job) else { continue };
            if !st.divisible || st.work_left == 0 || st.rate == 0 {
                continue;
            }
            let work = g.quota.min(st.work_left);
            if work == 0 {
                continue;
            }
            st.work_left -= work;
            let elapsed = Duration::from_millis((work / st.rate).max(1));
            e.report_slice(g.job, work, elapsed, now);
            if st.work_left == 0 {
                e.complete(g.job, now);
                states.remove(&g.job);
            }
        }
        // Complete indivisible jobs whose simulated runtime elapsed, if admitted.
        let to_complete: Vec<JobId> = states
            .iter()
            .filter(|(id, st)| !st.divisible && st.finish_at_ms <= now_ms && e.job_state(**id) == Some(JobState::Active))
            .map(|(id, _)| *id)
            .collect();
        for id in to_complete {
            e.complete(id, now);
            states.remove(&id);
        }

        if to_arrive == 0 && states.is_empty() {
            break;
        }
        assert!(now_ms < 200_000, "engine livelocked with {} jobs pending", states.len());
    }

    let m = e.metrics();
    let res = e.resources();
    assert_eq!(m.admitted, m.completed + m.cancelled, "every admitted job reached a terminal state");
    assert_eq!(m.admitted + m.rejected_queue_full + m.rejected_memory, 5_000);
    assert_eq!(m.rejected_queue_full + m.rejected_memory, rejected);
    assert_eq!(res.active_connections, 0);
    assert_eq!(res.active_handlers, 0);
    assert_eq!(res.active_cpu, 0);
    assert_eq!(res.active_divisible, 0);
    assert_eq!(res.registered_memory_bytes, 0, "memory registrations leaked");
    assert_eq!(e.queued_len(), 0);
    assert!(m.grants_issued > 0 && m.slice_reports > 0);
    assert!(m.work_executed > 0);
}

/// Admit a large batch, then cancel every live job (queued and active) mid-run:
/// terminal-sticky transitions must release exactly the capacity held.
#[test]
fn stress_cancel_storm_returns_capacity_to_baseline() {
    let mut e = engine();
    let mut rng = Lcg(0x5A17E);
    let mut live: Vec<JobId> = Vec::new();
    let mut now = Duration::ZERO;

    for _ in 0..2_000 {
        now += Duration::from_millis(1);
        let req = if rng.below(2) == 0 {
            NewJob::divisible(Some(1_000 + rng.below(4_000)), rng.below(100_000))
        } else {
            NewJob::handler(rng.below(50_000))
        };
        if let Ok(id) = e.admit(req, now) {
            live.push(id);
        }
    }

    // Run a few rounds so some jobs are Active, then cancel everything.
    for _ in 0..10 {
        now += Duration::from_millis(1);
        let out = e.pump(now);
        for g in &out.grants {
            e.report_slice(g.job, g.quota.min(50), Duration::from_millis(5), now);
        }
    }
    let before = e.resources();
    for id in &live {
        assert!(e.cancel(*id, now) || e.job_state(*id).is_none());
        // Double-cancel and late-complete attempts must be no-ops.
        assert!(!e.cancel(*id, now));
        assert!(!e.complete(*id, now));
    }

    let after = e.resources();
    assert_eq!(after.active_connections, 0);
    assert_eq!(after.active_handlers, 0);
    assert_eq!(after.active_divisible, 0);
    assert_eq!(after.registered_memory_bytes, 0);
    assert!(before.active_handlers + before.active_divisible > 0, "storm must start with live work");
    let m = e.metrics();
    // `admitted` counts Active promotions only; every accepted job — queued or
    // active — ends in exactly one terminal transition.
    assert_eq!(m.completed, 0, "no job was driven to completion in this storm");
    assert_eq!(m.cancelled, live.len() as u64);
    assert!(m.admitted <= m.cancelled, "only cancelled-after-admission jobs were live");
    assert_eq!(e.queued_len(), 0);
}
