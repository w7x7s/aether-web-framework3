//! Policy comparison benchmark: FIFO vs Round Robin vs Deficit RR vs Adaptive Tree
//! on identical seeded workloads inside the deterministic simulator
//! (`aether_scheduler::sim`). Prints markdown tables; output is transcribed
//! verbatim into framework/docs/SCHEDULING_COMPARISON.md.
//!
//! Run: cargo run -p aether-benchmarks --bin sched_compare --release [job_count]

use std::env;

use aether_scheduler::{PolicyKind, Scenario, run, workload};

const POLICIES: [(PolicyKind, &str); 4] = [
    (PolicyKind::Fifo, "FIFO"),
    (PolicyKind::RoundRobin, "RoundRobin"),
    (PolicyKind::Deficit, "DeficitRR"),
    (PolicyKind::AdaptiveTree, "AdaptiveTree"),
];

const SCENARIOS: [(Scenario, u64); 4] = [
    (Scenario::MixedSteady, 7),
    (Scenario::Burst, 5),
    (Scenario::LargeHeavy, 3),
    (Scenario::UnknownHeavy, 9),
];

fn main() {
    let jobs_count = env::args()
        .nth(1)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(400);
    println!("workloads: {} jobs per scenario, seeds: mixed-steady=7 burst=5 large-heavy=3 unknown-heavy=9", jobs_count);
    println!("resources (identical for every policy): 3 divisible slots, queue bound 64, starvation guard 1500ms, adapt interval 100ms\n");

    for (scenario, seed) in SCENARIOS {
        let jobs = workload(seed, scenario, jobs_count);
        println!("## scenario: {} (seed {})\n", scenario.name(), seed);
        println!("| policy | done | rej | TAT mean ms | p50 | p95 | p99 | small p99 | large p99 | max start wait ms | starv. promos | makespan ms | thr. work/s | cap. changes | class promos | peak queue | slices/large |");
        println!("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|");
        for (policy, label) in POLICIES {
            let r = run(policy, &jobs);
            println!(
                "| {} | {} | {} | {:.1} | {} | {} | {} | {} | {} | {} | {} | {} | {:.0} | {} | {} | {} | {:.1} |",
                label,
                r.completed,
                r.rejected,
                r.tat_mean_ms,
                r.tat_p50_ms,
                r.tat_p95_ms,
                r.tat_p99_ms,
                r.small_p99_ms,
                r.large_p99_ms,
                r.max_start_wait_ms,
                r.starvation_promotions,
                r.makespan_ms,
                r.throughput_work_per_s,
                r.capacity_changes,
                r.class_promotions,
                r.peak_queue,
                r.avg_slices_per_large,
            );
        }
        println!();
    }
}
