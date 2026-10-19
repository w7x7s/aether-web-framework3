use std::time::Duration;

use crate::config::SchedulingConfig;
use crate::job::{JobRecord, Work};

/// Large-job per-round work quota (design §4.3).
///
/// `q = clamp(rate_ewma × target_slice_ms, quota_min × q0, quota_max × q0)`,
/// scaled by the job's adaptation multiplier and a wait boost, then hard-capped by
/// remaining work and the round budget. `q0 = quota_alpha × baseline` is the
/// reference share derived from the stable baseline. The average is not a size
/// ceiling and the quota is not a completion-time guarantee; every admitted large
/// job still gets at least `quota_min × q0` while capacity exists.
pub struct QuotaModel;

impl QuotaModel {
    #[must_use]
    pub fn grant(
        cfg: &SchedulingConfig,
        job: &mut JobRecord,
        baseline: Work,
        wait: Duration,
        budget_left: Work,
    ) -> Work {
        let baseline = baseline.max(1);
        let q0 = (cfg.quota_alpha * baseline as f64).round() as Work;
        let q0 = q0.max(1);
        let floor = ((cfg.quota_min * q0 as f64).round() as Work).max(1);
        let cap = (cfg.quota_max * q0 as f64).round().max(1.0) as Work;

        let target_ms = cfg.target_slice_ms.max(1) as f64;
        let rate = job.rate_ewma.unwrap_or(baseline as f64 / target_ms);
        let q_time = (rate * target_ms).round().max(0.0) as Work;

        let mut q = q_time.clamp(floor, cap);
        q = (q as f64 * job.adapt_multiplier.max(0.0)).round().clamp(floor as f64, cap as f64) as Work;

        let guard_ms = cfg.starvation_guard.as_millis().max(1) as f64;
        let boost = 1.0 + cfg.wait_boost * (wait.as_millis() as f64 / guard_ms).min(1.0);
        q = ((q as f64 * boost).round().min((cap as f64 * 2.0).round()).max(floor as f64).round()) as Work;

        if let Some(remaining) = job.grantable_remaining() {
            q = q.min(remaining);
        }
        q.min(budget_left)
    }

    /// Fold one measured slice into the job's learned rate and adaptation
    /// multiplier. Fast decrease (slice > 2× target ⇒ ×0.5), slow ramp
    /// (slice < 0.5× target with work left ⇒ ×1.25), one step per report.
    pub fn observe_slice(cfg: &SchedulingConfig, job: &mut JobRecord, work: Work, elapsed: Duration) {
        let t_ms = elapsed.as_millis() as f64;
        let t_ms = if t_ms < 1.0 { 1.0 } else { t_ms };
        let rate_now = work as f64 / t_ms;
        let lambda = cfg.rate_lambda.clamp(0.05, 1.0);
        job.rate_ewma = Some(match job.rate_ewma {
            Some(r) => (1.0 - lambda) * r + lambda * rate_now,
            None => rate_now,
        });

        let target = cfg.target_slice_ms.max(1) as f64;
        let has_more = job.remaining_work.map_or(true, |r| r > 0);
        if t_ms > 2.0 * target {
            job.adapt_multiplier = (job.adapt_multiplier * 0.5).clamp(0.25, 4.0);
        } else if t_ms < 0.5 * target && has_more {
            job.adapt_multiplier = (job.adapt_multiplier * 1.25).clamp(0.25, 4.0);
        }
    }
}
