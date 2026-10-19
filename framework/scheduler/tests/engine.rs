//! Deterministic engine tests on a virtual clock (design §9.1). Every policy
//! call passes `now` explicitly, so none of these tests depend on wall time.

use std::time::Duration;

use aether_scheduler::{JobClass, JobKind, NewJob, PolicyKind, ResourceConfig, SchedulingConfig, SchedulerEngine};

fn ms(v: u64) -> Duration {
    Duration::from_millis(v)
}

fn cfg_with(policy: PolicyKind) -> SchedulingConfig {
    SchedulingConfig { policy, ..SchedulingConfig::default() }
}

fn res_with(handlers: usize) -> ResourceConfig {
    ResourceConfig { max_active_handlers: Some(handlers), min_slots: 1, ..ResourceConfig::default() }
}

fn engine(policy: PolicyKind, handlers: usize) -> SchedulerEngine {
    SchedulerEngine::new(cfg_with(policy), res_with(handlers))
}

#[test]
fn admission_respects_slot_bounds() {
    let mut e = engine(PolicyKind::Fifo, 2);
    let ids: Vec<_> = (0..5)
        .map(|_| e.admit(NewJob { kind: JobKind::Handler, work_hint: None, memory_bytes: 0 }, ms(0)).unwrap())
        .collect();
    let out = e.pump(ms(1));
    assert_eq!(out.admitted, vec![ids[0], ids[1]], "two slots admit exactly the two oldest");
    assert_eq!(e.queued_len(), 3);

    assert!(e.complete(ids[0], ms(2)));
    let out = e.pump(ms(2));
    assert_eq!(out.admitted, vec![ids[2]], "freed capacity admits the OLDEST queued job");
    assert_eq!(e.job_state(ids[0]), None, "terminal jobs are purged");
}

#[test]
fn queue_overflow_rejects_oldest_arrival_policy() {
    let mut e = SchedulerEngine::new(
        SchedulingConfig { policy: PolicyKind::AdaptiveTree, max_queue: 2, ..SchedulingConfig::default() },
        res_with(8),
    );
    e.admit(NewJob::handler(0), ms(0)).unwrap();
    e.admit(NewJob::handler(0), ms(0)).unwrap();
    let err = e.admit(NewJob::handler(0), ms(0)).unwrap_err();
    assert!(e.metrics().rejected_queue_full == 1);
    use aether_scheduler::RejectReason;
    assert_eq!(err, RejectReason::QueueFull);
    assert_eq!(e.queued_len(), 2, "rejected work is reported, never silently dropped");
}

#[test]
fn memory_budget_backpressures_without_rejecting() {
    let rcfg = ResourceConfig { memory_budget_bytes: 1_000, ..ResourceConfig::default() };
    let mut e = SchedulerEngine::new(cfg_with(PolicyKind::AdaptiveTree), rcfg);
    let a = e.admit(NewJob::handler(600), ms(0)).unwrap();
    let b = e.admit(NewJob::handler(600), ms(0)).unwrap();
    let out = e.pump(ms(0));
    assert_eq!(out.admitted, vec![a]);
    assert_eq!(e.job_state(b), Some(aether_scheduler::JobState::Queued));
    assert!(e.complete(a, ms(1)));
    let out = e.pump(ms(1));
    assert_eq!(out.admitted, vec![b]);
    // A job larger than the whole budget can never fit.
    assert_eq!(e.admit(NewJob::handler(2_000), ms(2)), Err(aether_scheduler::RejectReason::MemoryBudget));
    assert_eq!(e.metrics().rejected_memory, 1);
}

#[test]
fn terminal_transitions_release_exactly_once() {
    let mut e = engine(PolicyKind::Fifo, 4);
    let a = e.admit(NewJob::handler(100), ms(0)).unwrap();
    let b = e.admit(NewJob::handler(100), ms(0)).unwrap();
    e.pump(ms(0));
    assert_eq!(e.resources().registered_memory_bytes, 200);
    assert_eq!(e.resources().active_handlers, 2);

    assert!(e.complete(a, ms(1)));
    assert!(!e.complete(a, ms(1)), "double complete must be a no-op");
    assert!(!e.cancel(a, ms(1)));
    assert_eq!(e.resources().registered_memory_bytes, 100);
    assert_eq!(e.resources().active_handlers, 1);

    assert!(e.cancel(b, ms(2)));
    assert_eq!(e.resources().registered_memory_bytes, 0);
    assert_eq!(e.resources().active_handlers, 0);
    assert_eq!(e.metrics().completed, 1);
    assert_eq!(e.metrics().cancelled, 1);
}

#[test]
fn cancel_a_queued_job_never_releases_unheld_capacity() {
    let mut e = engine(PolicyKind::Fifo, 1);
    let a = e.admit(NewJob::handler(0), ms(0)).unwrap();
    let b = e.admit(NewJob::handler(0), ms(0)).unwrap();
    e.pump(ms(0)); // a active, b queued
    assert!(e.cancel(b, ms(1))); // cancelling a QUEUED job
    assert_eq!(e.queued_len(), 0);
    assert_eq!(e.resources().active_handlers, 1, "b never held a slot, so nothing was released twice");
    assert!(e.complete(a, ms(2)));
    assert_eq!(e.resources().active_handlers, 0);
}

#[test]
fn indivisible_jobs_are_never_granted_slices() {
    // HTTP handlers/connections are indivisible: the engine must not pretend to
    // preempt them. Only Divisible jobs receive grants.
    let mut e = engine(PolicyKind::AdaptiveTree, 4);
    e.admit(NewJob::handler(0), ms(0)).unwrap();
    e.admit(NewJob::connection(0), ms(0)).unwrap();
    let out = e.pump(ms(0));
    assert_eq!(out.admitted.len(), 2);
    assert!(out.grants.is_empty(), "no slice rotation for non-divisible work");
}

#[test]
fn large_job_quota_starts_at_one_baseline_share() {
    let mut e = engine(PolicyKind::AdaptiveTree, 4);
    let big = e.admit(NewJob::divisible(Some(10_000), 0), ms(0)).unwrap();
    let out = e.pump(ms(0));
    assert!(out.admitted.contains(&big));
    assert_eq!(out.grants.len(), 1);
    // q0 = quota_alpha × default_baseline = 1 × 100; no wait boost, no rate yet.
    assert_eq!(out.grants[0].quota, 100);
}

#[test]
fn quota_shrinks_fast_after_an_overrun_slice() {
    let mut e = engine(PolicyKind::AdaptiveTree, 4);
    let big = e.admit(NewJob::divisible(Some(10_000), 0), ms(0)).unwrap();
    let g1 = e.pump(ms(0)).grants[0].quota;
    // Slice took 3× the 10 ms target → fast decrease (×0.5) and a low measured rate.
    assert!(e.report_slice(big, g1, ms(30), ms(30)));
    let g2 = e.pump(ms(30)).grants[0].quota;
    assert!(g2 < g1, "overrun slices must reduce the next quota: {g2} !< {g1}");
    assert!(g2 >= 25, "quota never drops below quota_min × q0 = 25");
}

#[test]
fn round_budget_caps_total_grants_per_round() {
    let mut e = engine(PolicyKind::AdaptiveTree, 4);
    let ids: Vec<_> = (0..6)
        .map(|i| e.admit(NewJob::divisible(Some(10_000 + i), 0), ms(i)).unwrap())
        .collect();
    let out = e.pump(ms(10));
    // slots(4) × baseline(100) × round_factor(4) = 1600
    let total: u64 = out.grants.iter().map(|g| g.quota).sum();
    assert!(total <= 4 * 100 * 4, "round budget exceeded: {total}");
    assert!(!out.grants.is_empty());
    let per_job: Vec<_> = ids.iter().map(|i| out.grants.iter().filter(|g| &g.job == i).count()).collect();
    assert!(per_job.iter().all(|c| *c <= 1), "no job gets two slices in one round");
}

#[test]
fn unknown_size_jobs_converge_to_large_from_executed_work() {
    let mut e = engine(PolicyKind::AdaptiveTree, 4);
    let anon = e.admit(NewJob::divisible(None, 0), ms(0)).unwrap();
    let out = e.pump(ms(0));
    // Unknown cap: at most one baseline (100) per slice.
    assert!(out.grants[0].quota <= 100);
    // Executed crosses hi × B (400): must promote to Large.
    assert!(e.report_slice(anon, 450, ms(1), ms(1)));
    assert_eq!(e.job(anon).unwrap().class, JobClass::Large);
    assert_eq!(e.metrics().class_promotions, 1);
}

#[test]
fn baseline_samples_exclude_large_completions() {
    let mut e = engine(PolicyKind::AdaptiveTree, 4);
    // Four small jobs (work 50 < lo×B=150) shift B from 100 (default) to 50.
    for _ in 0..4 {
        let s = e.admit(NewJob::divisible(Some(50), 0), ms(0)).unwrap();
        e.pump(ms(0));
        assert!(e.complete(s, ms(1)));
    }
    assert_eq!(e.baseline(), 50, "median of four 50-work completions");
    // A huge completion (hint ≫ 4×B) must not touch the sample.
    let big = e.admit(NewJob::divisible(Some(100_000), 0), ms(2)).unwrap();
    assert_eq!(e.job(big).unwrap().class, JobClass::Large);
    e.pump(ms(2));
    e.complete(big, ms(3));
    assert_eq!(e.baseline(), 50, "large jobs never feed the baseline");
    // And the large share follows the stable baseline: q0 = 1 × 50.
    let big2 = e.admit(NewJob::divisible(Some(100_000), 0), ms(4)).unwrap();
    let out = e.pump(ms(4));
    assert!(out.grants.iter().any(|g| g.job == big2 && g.quota == 50));
}

#[test]
fn starvation_guard_admits_the_oldest_protected_job_first() {
    let mut e = SchedulerEngine::new(
        SchedulingConfig {
            policy: PolicyKind::AdaptiveTree,
            starvation_guard: Duration::from_millis(100),
            ..SchedulingConfig::default()
        },
        res_with(1),
    );
    let busy = e.admit(NewJob::divisible(Some(1_000), 0), ms(0)).unwrap();
    e.pump(ms(0));
    let old = e.admit(NewJob::handler(0), ms(10)).unwrap();
    let young = e.admit(NewJob::handler(0), ms(20)).unwrap();
    assert!(e.complete(busy, ms(200)));
    let out = e.pump(ms(200));
    assert_eq!(out.admitted, vec![old]);
    assert!(e.metrics().starvation_promotions >= 1);
    assert_eq!(e.job_state(young), Some(aether_scheduler::JobState::Queued));
}

#[test]
fn round_robin_rotates_one_grant_per_job_per_round() {
    let mut e = engine(PolicyKind::RoundRobin, 8);
    let _ids: Vec<_> = (0..3)
        .map(|i| e.admit(NewJob::divisible(Some(1_000), 0), ms(i)).unwrap())
        .collect();
    e.pump(ms(0));
    let mut seen = Vec::new();
    for t in 1..=4 {
        let out = e.pump(ms(t));
        assert_eq!(out.grants.len(), 3, "all three jobs granted this round");
        // Cursor rotation: the first pick cycles through the job order.
        seen.push(out.grants[0].job);
        for g in &out.grants {
            assert!(e.report_slice(g.job, g.quota.min(1_000), ms(1), ms(t)));
        }
    }
    assert_eq!(seen[0], seen[3], "rotation must wrap after one full cycle over 3 jobs");
    assert_ne!(seen[0], seen[1]);
}

#[test]
fn deficit_round_robin_banks_unspent_quantum() {
    let mut e = engine(PolicyKind::Deficit, 8);
    let a = e.admit(NewJob::divisible(Some(1_000), 0), ms(0)).unwrap();
    let b = e.admit(NewJob::divisible(Some(1_000), 0), ms(1)).unwrap();
    e.pump(ms(0)); // admits
    let out = e.pump(ms(1));
    let qa = out.grants.iter().find(|g| g.job == a).unwrap().quota;
    assert_eq!(qa, 100, "quantum = default baseline");
    // a can only use 20 units → bank 80 deficit.
    assert!(e.report_slice(a, 20, ms(1), ms(2)));
    let out = e.pump(ms(3));
    let qa2 = out.grants.iter().find(|g| g.job == a).unwrap().quota;
    assert_eq!(qa2, 180, "quantum + banked deficit");
    let _ = b;
}

#[test]
fn fifo_runs_a_divisible_job_to_completion() {
    let mut e = engine(PolicyKind::Fifo, 4);
    let j = e.admit(NewJob::divisible(Some(500), 0), ms(0)).unwrap();
    let out = e.pump(ms(0));
    assert_eq!(out.grants[0].quota, 500, "FIFO grants the whole remainder");
    assert!(e.report_slice(j, 500, ms(5), ms(5)));
    assert_eq!(e.job(j).unwrap().remaining_work, Some(0));
    assert!(e.complete(j, ms(5)));
}

#[test]
fn determinism_same_input_same_decision_log() {
    let build = || {
        let mut e = engine(PolicyKind::AdaptiveTree, 2);
        for i in 0..5u64 {
            let _ = e.admit(NewJob::divisible(Some(100 * i), 0), ms(i));
        }
        e.pump(ms(5));
        e
    };
    let a = build();
    let b = build();
    assert_eq!(a.decisions().len(), b.decisions().len());
    for (da, db) in a.decisions().iter().zip(b.decisions().iter()) {
        assert_eq!(da.job, db.job);
        assert_eq!(da.kind, db.kind);
    }
}

#[test]
fn resources_shrink_slowly_and_recover_additively() {
    use aether_scheduler::Resources;
    let rcfg = ResourceConfig {
        max_active_handlers: Some(8),
        adapt_interval: ms(10),
        sustain_intervals: 2,
        min_slots: 2,
        ..ResourceConfig::default()
    };
    let mut r = Resources::new(rcfg);
    assert_eq!(r.handler_slots(), 8);
    // High pressure: halve once sustained, then cooldown blocks further change.
    assert_eq!(r.refresh(ms(0), 1.0, 10), None, "one interval is not sustain");
    assert_eq!(r.refresh(ms(20), 1.0, 10), Some(4), "halve after 2 sustained ticks");
    assert_eq!(r.refresh(ms(25), 1.0, 10), None, "cooldown blocks oscillation");
    // Low pressure: +1 only after a full sustain streak, one change per window.
    assert_eq!(r.refresh(ms(40), 0.0, 0), None, "first low tick is not sustain");
    assert_eq!(r.refresh(ms(50), 0.0, 0), Some(5));
    assert_eq!(r.refresh(ms(60), 0.0, 0), None, "streak restarted after the change");
    assert_eq!(r.refresh(ms(70), 0.0, 0), Some(6));
    assert_eq!(r.snapshot().capacity_changes, 3);
}

#[test]
fn zero_work_http_jobs_never_degrade_the_work_baseline() {
    let mut e = engine(PolicyKind::AdaptiveTree, 4);
    // HTTP handler/connection jobs report no work units. Feeding each as "1"
    // (the old rule) collapsed the baseline to 1 under normal traffic and
    // starved every divisible quota derived from it.
    for i in 0..40u64 {
        let id = e.admit(NewJob::handler(0), ms(i)).unwrap();
        e.pump(ms(i));
        assert!(e.complete(id, ms(i + 1)));
    }
    assert_eq!(e.baseline(), 100, "zero-work jobs must not sample the work baseline");
    assert_eq!(e.metrics().completed, 40);

    // Genuine ordinary divisible completions still feed the estimator.
    for i in 0..4u64 {
        let id = e.admit(NewJob::divisible(Some(30), 0), ms(50 + i)).unwrap();
        e.pump(ms(50 + i));
        e.complete(id, ms(51 + i));
    }
    assert_eq!(e.baseline(), 30, "non-large reported work must move the baseline");
}

/// A driver that overshoots its declared hint must keep receiving grants (all
/// four policies used to clamp to `remaining_work == Some(0)` and stall) and its
/// class must follow the executed work, not the stale hint.
#[test]
fn overshooting_a_declared_hint_keeps_granting_and_reclassifies() {
    let mut e = engine(PolicyKind::AdaptiveTree, 4);
    let j = e.admit(NewJob::divisible(Some(100), 0), ms(0)).unwrap();
    e.pump(ms(0));
    assert!(e.report_slice(j, 100, ms(1), ms(1)));
    assert_eq!(e.job(j).unwrap().remaining_work, Some(0));
    let out = e.pump(ms(2));
    assert!(
        out.grants.iter().any(|g| g.job == j && g.quota > 0),
        "a job past its declared hint must not stall: {:?}",
        out.grants
    );
    assert!(e.report_slice(j, 400, ms(1), ms(3)));
    assert_eq!(
        e.job(j).unwrap().class,
        JobClass::Large,
        "executed work must correct a stale size hint"
    );
}

/// The quota boost must reward *staleness* (no executed slice for a while),
/// not the job's age since admission — an actively-running large job used to
/// carry a permanent +50% boost once it passed the starvation guard.
#[test]
fn wait_boost_rewards_staleness_not_age() {
    let mut e = engine(PolicyKind::AdaptiveTree, 4);
    let big = e.admit(NewJob::divisible(Some(10_000), 0), ms(0)).unwrap();
    e.pump(ms(0));
    assert!(e.report_slice(big, 100, ms(10), ms(2500)));
    let g = e.pump(ms(2500)).grants.iter().find(|x| x.job == big).unwrap().quota;
    assert_eq!(g, 100, "a freshly-executed slice resets the boost");
    let g = e.pump(ms(4500)).grants.iter().find(|x| x.job == big).unwrap().quota;
    assert_eq!(g, 150, "2 s without progress earns the full 1 + wait_boost share");
}

/// Completing a divisible job accounts its whole declared total: reported
/// slices plus the unreported remainder exactly once.
#[test]
fn completion_accounts_the_whole_declared_total() {
    let mut e = engine(PolicyKind::Fifo, 4);
    let j = e.admit(NewJob::divisible(Some(500), 0), ms(0)).unwrap();
    e.pump(ms(0));
    assert!(e.report_slice(j, 200, ms(1), ms(1)));
    assert!(e.complete(j, ms(2)));
    assert_eq!(e.metrics().work_executed, 500, "200 reported + 300 implicit remainder");
    assert_eq!(e.metrics().completed, 1);
}
