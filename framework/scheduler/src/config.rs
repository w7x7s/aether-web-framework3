use std::time::Duration;

use crate::job::Work;

/// Which selection/quota algorithm the engine runs. All policies share admission,
/// queue bounds, rejection and resource caps, so comparisons isolate scheduling
/// logic only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyKind {
    Fifo,
    RoundRobin,
    Deficit,
    AdaptiveTree,
}

/// Tunables for the scheduling algorithm. Defaults match
/// `docs/ADAPTIVE_TREE_SCHEDULING_DESIGN.md` §4.
#[derive(Debug, Clone)]
pub struct SchedulingConfig {
    pub policy: PolicyKind,
    /// Baseline used before `min_samples` non-large completions exist.
    pub default_baseline: Work,
    pub min_samples: usize,
    /// Recency window for the baseline median (completed non-large jobs).
    pub sample_window: usize,
    /// Large if measured work > `hi × baseline`.
    pub large_factor_hi: f64,
    /// Non-large only if measured work < `lo × baseline`; between the two the
    /// previous class sticks (anti-flapping).
    pub large_factor_lo: f64,
    /// Reference large-job share: `quota_alpha × baseline` per round.
    pub quota_alpha: f64,
    pub quota_min: f64,
    pub quota_max: f64,
    /// Slice duration the quota model aims for.
    pub target_slice_ms: u64,
    /// EWMA weight for the per-job learned work rate.
    pub rate_lambda: f64,
    /// Quota boost proportional to the job's time without an executed slice
    /// (`JobRecord::grant_wait`) over `starvation_guard`, capped at 1. A job that
    /// keeps progressing is never boosted; only a stalled one is.
    pub wait_boost: f64,
    /// Queued jobs waiting at least this long are promoted to the head of the
    /// admission order (oldest first).
    pub starvation_guard: Duration,
    /// AdaptiveTree round budget: `slots × baseline × round_factor` work units.
    pub round_factor: Work,
    /// Maximum queued (waiting) jobs; overflow arrivals are rejected.
    pub max_queue: usize,
    /// How long an admitted job may wait in the queue before the caller gives up
    /// and cancels it. Enforced by drivers (e.g. the Aether serve layer), not by
    /// the engine itself.
    pub max_queue_wait: Duration,
    /// Bounded decision log capacity for observability.
    pub decision_log_capacity: usize,
}

impl Default for SchedulingConfig {
    fn default() -> Self {
        Self {
            policy: PolicyKind::AdaptiveTree,
            default_baseline: 100,
            min_samples: 4,
            sample_window: 32,
            large_factor_hi: 4.0,
            large_factor_lo: 1.5,
            quota_alpha: 1.0,
            quota_min: 0.25,
            quota_max: 4.0,
            target_slice_ms: 10,
            rate_lambda: 0.3,
            wait_boost: 0.5,
            starvation_guard: Duration::from_secs(2),
            round_factor: 4,
            max_queue: 1_024,
            max_queue_wait: Duration::from_secs(5),
            decision_log_capacity: 256,
        }
    }
}

/// Safe, explicit resource ceilings. Anything the platform cannot measure is
/// configured, never presented as a measurement (see design §6).
#[derive(Debug, Clone)]
pub struct ResourceConfig {
    pub max_connections: usize,
    /// `None` → `4 × available_parallelism()`.
    pub max_active_handlers: Option<usize>,
    /// `None` → `available_parallelism()`.
    pub max_cpu_heavy: Option<usize>,
    /// Separate, non-adaptive budget for divisible jobs. Trade-off (design §6):
    /// a divisible job reserves its slot for its whole lifetime so it is never
    /// re-admitted mid-way — re-admission would expose long jobs to queue-full
    /// rejection between slices. Keeping this separate from the adaptive handler
    /// budget means one big background job cannot starve HTTP handler capacity.
    /// `None` → 4.
    pub max_active_divisible: Option<usize>,
    /// Registered in-flight bytes ceiling (framework-tracked buffers). Not an
    /// OS free-memory claim.
    pub memory_budget_bytes: u64,
    /// Minimum gap between two capacity changes (anti-oscillation).
    pub adapt_interval: Duration,
    /// Consecutive pressure readings required before adapting.
    pub sustain_intervals: u32,
    /// Pressure above this counts high; below `low_pressure` counts low.
    pub high_pressure: f64,
    pub low_pressure: f64,
    /// Floor for adaptive handler slots under any shrink.
    pub min_slots: usize,
}

impl Default for ResourceConfig {
    fn default() -> Self {
        Self {
            max_connections: 10_000,
            max_active_handlers: None,
            max_cpu_heavy: None,
            max_active_divisible: None,
            memory_budget_bytes: 256 * 1_024 * 1_024,
            adapt_interval: Duration::from_secs(1),
            sustain_intervals: 2,
            high_pressure: 1.0,
            low_pressure: 0.3,
            min_slots: 2,
        }
    }
}
