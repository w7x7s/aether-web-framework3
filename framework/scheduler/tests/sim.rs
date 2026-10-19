//! Invariant tests for the deterministic policy-comparison simulator.
//! These do not judge which policy is better (that is the benchmark's job);
//! they prove the comparison itself is fair and reproducible.

use aether_scheduler::{JobSpec, PolicyKind, Scenario, LARGE_WORK, run, workload};

const POLICIES: [PolicyKind; 4] = [PolicyKind::Fifo, PolicyKind::RoundRobin, PolicyKind::Deficit, PolicyKind::AdaptiveTree];
const SCENARIOS: [Scenario; 4] = [Scenario::MixedSteady, Scenario::Burst, Scenario::LargeHeavy, Scenario::UnknownHeavy];

#[test]
fn workload_is_reproducible_byte_for_byte() {
    let a = workload(42, Scenario::MixedSteady, 300);
    let b = workload(42, Scenario::MixedSteady, 300);
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(&b) {
        assert_eq!((x.arrival_ms, x.work, x.rate, x.known), (y.arrival_ms, y.work, y.rate, y.known));
    }
    // Different seed or scenario must produce a different list.
    let c = workload(43, Scenario::MixedSteady, 300);
    assert_ne!(a[100].work, c[100].work);
}

#[test]
fn every_job_is_accounted_for_exactly_once() {
    for scenario in SCENARIOS {
        let jobs = workload(7, scenario, 250);
        for policy in POLICIES {
            let r = run(policy, &jobs);
            assert_eq!(
                r.completed + r.rejected,
                jobs.len() as u64,
                "{:?} / {} lost or duplicated jobs",
                policy,
                scenario.name()
            );
        }
    }
}

#[test]
fn all_policies_do_identical_total_work_when_nothing_is_rejected() {
    for scenario in SCENARIOS {
        let jobs = workload(11, scenario, 200);
        let results: Vec<_> = POLICIES.iter().map(|p| run(*p, &jobs)).collect();
        let any_rejected = results.iter().any(|r| r.rejected > 0);
        if any_rejected {
            // When work is shed, throughput legitimately differs; skip this run.
            continue;
        }
        let expected: f64 = jobs.iter().map(|j| j.work as f64).sum();
        for r in &results {
            // throughput * makespan ~= total work for every policy.
            let served = r.throughput_work_per_s * r.makespan_ms as f64 / 1000.0;
            assert!(
                (served - expected).abs() / expected < 0.01,
                "{:?} served {:.0} of {:.0}",
                r.policy,
                served,
                expected
            );
        }
    }
}

#[test]
fn adaptive_tree_slices_large_jobs_instead_of_running_them_to_completion() {
    let jobs = workload(3, Scenario::LargeHeavy, 200);
    let fifo = run(PolicyKind::Fifo, &jobs);
    let adaptive = run(PolicyKind::AdaptiveTree, &jobs);
    // FIFO hands a slot to one divisible job until it finishes: one slice each.
    assert!(fifo.avg_slices_per_large < 1.5, "fifo avg slices {}", fifo.avg_slices_per_large);
    // Adaptive Tree must break large jobs into many quota slices.
    assert!(adaptive.avg_slices_per_large > 2.0, "adaptive avg slices {}", adaptive.avg_slices_per_large);
}

#[test]
fn adaptive_tree_limits_the_head_start_wait_of_small_jobs_under_a_burst() {
    let jobs = workload(5, Scenario::Burst, 250);
    let fifo = run(PolicyKind::Fifo, &jobs);
    let adaptive = run(PolicyKind::AdaptiveTree, &jobs);
    // Rotating large jobs must reduce the worst-case time a ready job waits
    // before its first slice. If this fails, the benchmark section has to
    // report it honestly instead of asserting it here.
    assert!(
        adaptive.max_start_wait_ms <= fifo.max_start_wait_ms,
        "adaptive max start wait {}ms > fifo {}ms",
        adaptive.max_start_wait_ms,
        fifo.max_start_wait_ms
    );
    let _ = adaptive.small_p99_ms;
}

#[test]
fn admission_rejection_never_loses_or_invents_work() {
    // A tiny queue with more jobs than it can hold must reject the excess.
    let jobs: Vec<JobSpec> = workload(9, Scenario::Burst, 400);
    let r = run(PolicyKind::AdaptiveTree, &jobs);
    assert!(r.rejected > 0, "expected queue-full rejections with 400 jobs");
    assert!(r.completed > 0);
    assert_eq!(r.completed + r.rejected, jobs.len() as u64);
}

#[test]
fn simulation_is_deterministic_across_runs() {
    let jobs = workload(21, Scenario::MixedSteady, 200);
    for policy in POLICIES {
        let a = run(policy, &jobs);
        let b = run(policy, &jobs);
        assert_eq!(a.completed, b.completed);
        assert_eq!(a.tat_p99_ms, b.tat_p99_ms);
        assert_eq!(a.max_start_wait_ms, b.max_start_wait_ms);
        assert_eq!(a.makespan_ms, b.makespan_ms);
    }
}

#[test]
fn large_floor_matches_generator_band() {
    let jobs = workload(1, Scenario::LargeHeavy, 300);
    let large = jobs.iter().filter(|j| j.work >= LARGE_WORK).count();
    assert!(large > 0, "large-heavy scenario must generate large jobs");
}
