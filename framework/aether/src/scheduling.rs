//! Opt-in adaptive scheduling for the Aether serve path.
//!
//! [`SchedulingLayer`] wraps the protocol-independent `aether-scheduler` engine
//! and exposes three integration points, each honest about what it gates:
//!
//! 1. **Connection admission** — bounded queue + rejection (503) instead of the
//!    default unbounded blocking wait on the connection semaphore.
//! 2. **Handler execution admission** — a per-handler permit taken *before* the
//!    handler task is spawned. Handlers are indivisible: the scheduler never
//!    pretends to preempt one; it only bounds how many run at once.
//! 3. **Divisible work driver** ([`SchedulingLayer::run_slices`]) — for caller
//!    code with explicit safe yield points; the only place where the adaptive
//!    tree's per-round work quotas actually slice execution.
//!
//! Every guard is RAII: slot and memory registration are released exactly once on
//! normal completion, panic, timeout-abort, or future drop (cancellation), so no
//! path can leak capacity. Admission failures map to protocol-valid responses the
//! caller writes before any handler state exists.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use aether_scheduler::{
    JobId, JobState, NewJob, RejectReason, ResourceConfig, ResourceSnapshot, SchedulerEngine,
    SchedulerMetrics, SchedulingConfig, Work,
};
use tokio::sync::Notify;

/// Why admission did not produce a running job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmitFailure {
    QueueFull,
    MemoryBudget,
    TimedOut,
}

/// Outcome of one slice of divisible work: units actually executed, and whether
/// the job is finished.
#[derive(Debug, Clone, Copy)]
pub struct SliceProgress {
    pub work: Work,
    pub done: bool,
}

impl SliceProgress {
    #[must_use]
    pub fn progressed(work: Work) -> Self {
        Self { work, done: false }
    }

    #[must_use]
    pub fn finished(work: Work) -> Self {
        Self { work, done: true }
    }
}

/// Shared scheduling layer for one server. Cheap to clone behind an `Arc`.
pub struct SchedulingLayer {
    engine: Mutex<SchedulerEngine>,
    notify: Notify,
    epoch: Instant,
    queue_wait: Duration,
}

/// RAII admission token. Released as `complete` (explicit) or `cancel` (drop ⇒
/// disconnect, timeout-abort, panic, or shutdown) exactly once.
pub struct AdmissionGuard {
    layer: Arc<SchedulingLayer>,
    id: JobId,
    finished: bool,
}

impl SchedulingLayer {
    #[must_use]
    pub fn new(cfg: SchedulingConfig, rcfg: ResourceConfig) -> Arc<Self> {
        let queue_wait = cfg.max_queue_wait;
        Arc::new(Self {
            engine: Mutex::new(SchedulerEngine::new(cfg, rcfg)),
            notify: Notify::new(),
            epoch: Instant::now(),
            queue_wait,
        })
    }

    #[must_use]
    pub fn policy_name(&self) -> &'static str {
        self.lock().policy_name()
    }

    /// Live counters: admissions, rejections, starvation promotions, class moves.
    #[must_use]
    pub fn metrics(&self) -> SchedulerMetrics {
        self.lock().metrics().clone()
    }

    #[must_use]
    pub fn resources(&self) -> ResourceSnapshot {
        self.lock().resources()
    }

    /// Current stable baseline (AdaptiveTree); `default_baseline` otherwise.
    #[must_use]
    pub fn baseline(&self) -> Work {
        self.lock().baseline()
    }

    fn now(&self) -> Duration {
        self.epoch.elapsed()
    }

    fn lock(&self) -> MutexGuard<'_, SchedulerEngine> {
        // The engine is pure sync code that never panics internally; a foreign
        // panic while locked must not brick the server (fail-open policy, design §8).
        self.engine.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn pump_now(&self) {
        let mut e = self.lock();
        e.pump(self.now());
    }

    fn release(&self, id: JobId, completed: bool) {
        {
            let mut e = self.lock();
            let now = self.now();
            if completed {
                e.complete(id, now);
            } else {
                e.cancel(id, now);
            }
            // Freed capacity: give the oldest eligible waiter its shot immediately.
            e.pump(now);
        }
        self.notify.notify_waiters();
    }

    /// Admit a job and wait (bounded by `SchedulingConfig::max_queue_wait`) until
    /// the engine marks it Active. Queue overflow and impossible-fit jobs fail
    /// fast; wait expiry cancels the queued job so nothing lingers. Dropping the
    /// returned future while still queued is also safe: a `QueueToken` cancels
    /// the job exactly once, so aborts cannot leave orphaned queue entries.
    pub async fn admit(self: &Arc<Self>, job: NewJob) -> Result<AdmissionGuard, AdmitFailure> {
        let id = {
            let mut e = self.lock();
            let now = self.now();
            let id = e.admit(job, now).map_err(|r| match r {
                RejectReason::QueueFull => AdmitFailure::QueueFull,
                RejectReason::MemoryBudget => AdmitFailure::MemoryBudget,
            })?;
            e.pump(now);
            id
        };
        // Covers the window between engine admission and AdmissionGuard creation.
        let mut token = QueueToken { layer: Arc::clone(self), id, armed: true };
        if self.lock().job_state(id) == Some(JobState::Active) {
            token.disarm();
            return Ok(self.guard(id));
        }
        let deadline = Instant::now() + self.queue_wait;
        loop {
            // Register the wakeup BEFORE re-pumping so a release racing with the
            // check cannot be missed.
            let wait = self.notify.notified();
            tokio::pin!(wait);
            wait.as_mut().enable();
            self.pump_now();
            match self.lock().job_state(id) {
                Some(JobState::Active) => {
                    token.disarm();
                    return Ok(self.guard(id));
                }
                None => {
                    token.disarm();
                    return Err(AdmitFailure::QueueFull);
                }
                _ => {}
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                self.lock().cancel(id, self.now());
                token.disarm();
                return Err(AdmitFailure::TimedOut);
            };
            if tokio::time::timeout(remaining, wait).await.is_err() {
                self.lock().cancel(id, self.now());
                token.disarm();
                return Err(AdmitFailure::TimedOut);
            }
        }
    }

    fn guard(self: &Arc<Self>, id: JobId) -> AdmissionGuard {
        AdmissionGuard { layer: Arc::clone(self), id, finished: false }
    }

    /// Divisible-work driver: runs `f` one granted slice at a time, yielding
    /// between slices. `f` must return quickly enough to keep its slice honest —
    /// measured wall time feeds the quota adaptation. The job is cancelled if the
    /// driver future itself is dropped.
    pub async fn run_slices<F, Fut>(
        self: &Arc<Self>,
        work_hint: Option<Work>,
        memory_bytes: u64,
        mut f: F,
    ) -> Result<Work, AdmitFailure>
    where
        F: FnMut(Work) -> Fut,
        Fut: std::future::Future<Output = SliceProgress>,
    {
        let guard = self.admit(NewJob::divisible(work_hint, memory_bytes)).await?;
        let id = guard.id;
        let mut executed: Work = 0;
        loop {
            let grant = {
                let mut e = self.lock();
                let now = self.now();
                e.pump(now).grants.into_iter().find(|g| g.job == id).map(|g| g.quota)
            };
            let Some(quota) = grant.filter(|q| *q > 0) else {
                let wait = self.notify.notified();
                tokio::pin!(wait);
                wait.as_mut().enable();
                // Safety poll bound: grants can also come from time-driven
                // re-evaluation, not only from releases.
                let _ = tokio::time::timeout(Duration::from_millis(25), wait).await;
                continue;
            };
            let started = Instant::now();
            let progress = f(quota).await;
            let elapsed = started.elapsed();
            executed = executed.saturating_add(progress.work);
            self.lock().report_slice(id, progress.work, elapsed, self.now());
            if progress.done {
                break;
            }
        }
        guard.complete();
        Ok(executed)
    }
}

impl AdmissionGuard {
    /// The job succeeded; releases capacity and lets the next waiter in.
    pub fn complete(mut self) {
        self.finished = true;
        self.layer.release(self.id, true);
    }

    /// Explicit cancellation (client gone, shutdown): same release path, counted
    /// separately in metrics and never fed to baseline samples.
    pub fn cancel(mut self) {
        self.finished = true;
        self.layer.release(self.id, false);
    }

    #[must_use]
    pub fn id(&self) -> JobId {
        self.id
    }
}

impl Drop for AdmissionGuard {
    fn drop(&mut self) {
        if !self.finished {
            // Panic, abort, or future-drop: cancel exactly once.
            self.layer.release(self.id, false);
        }
    }
}

/// RAII containment for the waiting window of [`SchedulingLayer::admit`]: if the
/// future is dropped (task abort) before an `AdmissionGuard` exists, the queued
/// job is cancelled exactly once so the queue can never accumulate orphans.
struct QueueToken {
    layer: Arc<SchedulingLayer>,
    id: JobId,
    armed: bool,
}

impl QueueToken {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for QueueToken {
    fn drop(&mut self) {
        if self.armed {
            self.layer.release(self.id, false);
        }
    }
}
