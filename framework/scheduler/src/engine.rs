use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use crate::config::{ResourceConfig, SchedulingConfig};
use crate::job::{JobClass, JobId, JobRecord, JobState, NewJob, Work};
use crate::policy::{SchedulingPolicy, build_policy};
use crate::resources::{ResourceSnapshot, Resources};
use crate::tree::JobTree;

/// A scheduling decision recorded for observability (design: metrics must show
/// the scheduler's decisions and their reasons).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionKind {
    Queued,
    Admitted,
    StarvationPromoted,
    RejectedQueueFull,
    RejectedMemory,
    Reclassified { from: JobClass, to: JobClass },
    Quota { work: Work },
    CapacityChanged { slots: usize },
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub now: Duration,
    pub job: JobId,
    pub kind: DecisionKind,
}

#[derive(Debug, Default, Clone)]
pub struct SchedulerMetrics {
    pub admitted: u64,
    pub rejected_queue_full: u64,
    pub rejected_memory: u64,
    pub completed: u64,
    pub cancelled: u64,
    pub starvation_promotions: u64,
    pub class_promotions: u64,
    pub class_demotions: u64,
    pub grants_issued: u64,
    pub slice_reports: u64,
    pub work_executed: Work,
    pub peak_queue: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grant {
    pub job: JobId,
    pub quota: Work,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RoundOutcome {
    pub admitted: Vec<JobId>,
    pub grants: Vec<Grant>,
    pub capacity_changed: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    QueueFull,
    MemoryBudget,
}

/// Everything a policy can see and adjust. Public so policies live outside the
/// engine module; the traversal itself only happens through [`SchedulerEngine`].
pub struct SchedulerState {
    pub cfg: SchedulingConfig,
    pub jobs: HashMap<JobId, JobRecord>,
    pub tree: JobTree,
    pub resources: Resources,
    pub metrics: SchedulerMetrics,
    pub decisions: VecDeque<Decision>,
    pub(crate) seq_next: u64,
    pub(crate) now: Duration,
}

impl SchedulerState {
    #[must_use]
    pub fn new(cfg: SchedulingConfig, rcfg: ResourceConfig) -> Self {
        Self {
            jobs: HashMap::new(),
            tree: JobTree::new(),
            resources: Resources::new(rcfg),
            metrics: SchedulerMetrics::default(),
            decisions: VecDeque::new(),
            seq_next: 0,
            now: Duration::ZERO,
            cfg,
        }
    }

    #[must_use]
    pub fn job(&self, id: JobId) -> Option<&JobRecord> {
        self.jobs.get(&id)
    }

    #[must_use]
    pub fn job_mut(&mut self, id: JobId) -> Option<&mut JobRecord> {
        self.jobs.get_mut(&id)
    }

    /// All queued jobs, deterministically ordered by arrival sequence.
    #[must_use]
    pub fn queued_sorted(&self) -> Vec<JobId> {
        let mut ids = self.tree.queued_ids();
        ids.sort_by_key(|id| self.jobs.get(id).map(|j| j.seq).unwrap_or(u64::MAX));
        ids
    }

    /// Oldest-not-yet-granted active divisible job (FIFO-style run-to-completion).
    #[must_use]
    pub fn first_active_divisible(&mut self) -> Option<JobId> {
        for i in 0..self.tree.active.len() {
            let id = self.tree.active[i];
            if self.jobs.get(&id).is_some_and(|j| j.kind.is_divisible() && !j.granted_this_round) {
                return Some(id);
            }
        }
        None
    }

    /// Rotation pick: next ungranted active divisible job after the cursor. The
    /// cursor itself is advanced once per round by the engine, so successive
    /// rounds start at successive jobs.
    #[must_use]
    pub fn rotate_active_divisible(&self) -> Option<JobId> {
        let n = self.tree.active.len();
        if n == 0 {
            return None;
        }
        for i in 0..n {
            let idx = (self.tree.rotation_cursor + i) % n;
            let id = self.tree.active[idx];
            if self.jobs.get(&id).is_some_and(|j| j.kind.is_divisible() && !j.granted_this_round) {
                return Some(id);
            }
        }
        None
    }

    /// Move a job to another class node; counts promotions/demotions and logs it.
    pub fn set_class(&mut self, id: JobId, to: JobClass) {
        let from = match self.jobs.get(&id) {
            Some(j) => j.class,
            None => return,
        };
        if from == to {
            return;
        }
        if self.tree.is_queued(id, from) {
            self.tree.reclassify(id, from, to);
        }
        if let Some(j) = self.jobs.get_mut(&id) {
            j.class = to;
        }
        if to == JobClass::Large {
            self.metrics.class_promotions += 1;
        } else if from == JobClass::Large {
            self.metrics.class_demotions += 1;
        }
        self.log(id, DecisionKind::Reclassified { from, to });
    }

    pub(crate) fn log(&mut self, job: JobId, kind: DecisionKind) {
        let cap = self.cfg.decision_log_capacity.max(8);
        self.decisions.push_back(Decision { now: self.now, job, kind });
        while self.decisions.len() > cap {
            self.decisions.pop_front();
        }
    }
}

/// The engine: owns the tree, runs one deterministic traversal per `pump`, and is
/// the only place that mutates job lifecycle state. Pure sync code — callers pass
/// an explicit clock, drivers run on whatever runtime they like.
pub struct SchedulerEngine {
    pub(crate) st: SchedulerState,
    policy: Box<dyn SchedulingPolicy>,
    rejected_in_window: usize,
}

impl SchedulerEngine {
    #[must_use]
    pub fn new(cfg: SchedulingConfig, rcfg: ResourceConfig) -> Self {
        let policy = build_policy(&cfg);
        Self { st: SchedulerState::new(cfg, rcfg), policy, rejected_in_window: 0 }
    }

    #[must_use]
    pub fn with_policy(cfg: SchedulingConfig, rcfg: ResourceConfig, policy: Box<dyn SchedulingPolicy>) -> Self {
        Self { st: SchedulerState::new(cfg, rcfg), policy, rejected_in_window: 0 }
    }

    #[must_use]
    pub fn policy_name(&self) -> &'static str {
        self.policy.name()
    }

    #[must_use]
    pub fn config(&self) -> &SchedulingConfig {
        &self.st.cfg
    }

    #[must_use]
    pub fn job(&self, id: JobId) -> Option<&JobRecord> {
        self.st.job(id)
    }

    #[must_use]
    pub fn job_state(&self, id: JobId) -> Option<JobState> {
        self.st.job(id).map(|j| j.state)
    }

    #[must_use]
    pub fn queued_len(&self) -> usize {
        self.st.tree.queued_count()
    }

    #[must_use]
    pub fn metrics(&self) -> &SchedulerMetrics {
        &self.st.metrics
    }

    #[must_use]
    pub fn resources(&self) -> ResourceSnapshot {
        self.st.resources.snapshot()
    }

    #[must_use]
    pub fn baseline(&self) -> Work {
        self.policy.baseline().unwrap_or(self.st.cfg.default_baseline)
    }

    #[must_use]
    pub fn decisions(&self) -> &VecDeque<Decision> {
        &self.st.decisions
    }

    /// Register a new job. Overflow policy: when the queue is at `max_queue` the
    /// arrival is rejected outright — no silent dropping, no priority guessing.
    pub fn admit(&mut self, req: NewJob, now: Duration) -> Result<JobId, RejectReason> {
        self.st.now = now;
        if self.st.resources.over_ceiling(req.memory_bytes) {
            self.st.metrics.rejected_memory += 1;
            let id = JobId(self.st.seq_next);
            self.st.log(id, DecisionKind::RejectedMemory);
            return Err(RejectReason::MemoryBudget);
        }
        if self.st.tree.queued_count() >= self.st.cfg.max_queue {
            self.st.metrics.rejected_queue_full += 1;
            self.rejected_in_window += 1;
            let id = JobId(self.st.seq_next);
            self.st.log(id, DecisionKind::RejectedQueueFull);
            return Err(RejectReason::QueueFull);
        }
        let id = JobId(self.st.seq_next);
        self.st.seq_next += 1;
        self.st.jobs.insert(id, JobRecord::new(id, id.0, &req, now));
        self.st.tree.queue(id, JobClass::Unknown);
        self.st.metrics.peak_queue = self.st.metrics.peak_queue.max(self.st.tree.queued_count());
        self.st.log(id, DecisionKind::Queued);
        self.policy.on_arrive(&mut self.st, id);
        Ok(id)
    }

    /// One round: resource refresh → admission by arrival precedence (starvation
    /// guard leading) → bounded rotation of divisible jobs under a round budget.
    pub fn pump(&mut self, now: Duration) -> RoundOutcome {
        self.st.now = now;
        let mut out = RoundOutcome::default();

        let max_queue = self.st.cfg.max_queue.max(1);
        let queue_ratio = self.st.tree.queued_count() as f64 / max_queue as f64;
        let rejected = std::mem::take(&mut self.rejected_in_window);
        if let Some(slots) = self.st.resources.refresh(now, queue_ratio, rejected) {
            self.st.log(JobId(u64::MAX), DecisionKind::CapacityChanged { slots });
            out.capacity_changed = Some(slots);
        }

        // Admission walk: strict arrival precedence; a job that does not fit stops
        // the walk (head-of-line is the price of strict precedence; the starvation
        // guard bounds its cost).
        loop {
            let Some(id) = self.policy.pick_admission(&mut self.st, now) else { break };
            let (kind, mem, starved) = match self.st.job(id) {
                Some(j) => (j.kind, j.memory_bytes, j.queue_wait(now) >= self.st.cfg.starvation_guard),
                None => break,
            };
            if !self.st.resources.try_admit(kind, mem) {
                break;
            }
            let class = if let Some(j) = self.st.job_mut(id) {
                j.state = JobState::Active;
                j.admitted_at = Some(now);
                j.class
            } else {
                continue;
            };
            self.st.tree.unqueue(id, class);
            self.st.tree.promote_active(id);
            self.st.metrics.admitted += 1;
            if starved {
                self.st.metrics.starvation_promotions += 1;
                self.st.log(id, DecisionKind::StarvationPromoted);
            }
            self.st.log(id, DecisionKind::Admitted);
            out.admitted.push(id);
        }

        // Execution rotation for divisible jobs, bounded by the round budget.
        for i in 0..self.st.tree.active.len() {
            let id = self.st.tree.active[i];
            if let Some(j) = self.st.jobs.get_mut(&id) {
                j.granted_this_round = false;
            }
        }
        let baseline_v = self.policy.baseline().unwrap_or(self.st.cfg.default_baseline).max(1);
        let slots = self.st.resources.handler_slots();
        let mut budget = self.policy.round_budget(&self.st, slots, baseline_v);
        loop {
            let Some(id) = self.policy.pick_execution(&mut self.st, now) else { break };
            let q = self.policy.quota(&mut self.st, id, now, budget).min(budget);
            if let Some(j) = self.st.jobs.get_mut(&id) {
                j.granted_this_round = true;
                if q > 0 {
                    j.last_grant = q;
                }
            }
            if q > 0 {
                budget = budget.saturating_sub(q);
                out.grants.push(Grant { job: id, quota: q });
                self.st.metrics.grants_issued += 1;
                self.st.log(id, DecisionKind::Quota { work: q });
            }
        }
        // Advance the rotation start for the next round.
        let n = self.st.tree.active.len();
        if n > 0 {
            self.st.tree.rotation_cursor = (self.st.tree.rotation_cursor + 1) % n;
        }
        out
    }

    /// Driver reports one completed slice of a `Divisible` job. Returns false for
    /// unknown, terminal, or indivisible jobs (work can only be reported for work
    /// that was actually granted a slice).
    pub fn report_slice(&mut self, id: JobId, work: Work, elapsed: Duration, now: Duration) -> bool {
        self.st.now = now;
        let accepted = self.st.job(id).is_some_and(|j| j.state == JobState::Active && j.kind.is_divisible());
        if !accepted {
            return false;
        }
        if let Some(j) = self.st.job_mut(id) {
            j.executed_work = j.executed_work.saturating_add(work);
            j.total_runtime = j.total_runtime.saturating_add(elapsed);
            j.slices = j.slices.saturating_add(1);
            j.remaining_work = j.remaining_work.map(|r| r.saturating_sub(work));
            j.last_executed_at = Some(now);
        }
        self.st.metrics.slice_reports += 1;
        self.st.metrics.work_executed = self.st.metrics.work_executed.saturating_add(work);
        self.policy.on_slice(&mut self.st, id, work, elapsed);
        true
    }

    /// Terminal: job finished normally. Releases its slot and memory exactly once.
    pub fn complete(&mut self, id: JobId, now: Duration) -> bool {
        self.finish(id, now, true)
    }

    /// Terminal: disconnect, timeout, or shutdown drain. The job is never
    /// rescheduled as if healthy; terminal states are sticky.
    pub fn cancel(&mut self, id: JobId, now: Duration) -> bool {
        self.finish(id, now, false)
    }

    fn finish(&mut self, id: JobId, now: Duration, completed: bool) -> bool {
        self.st.now = now;
        let (state, kind, mem, class, was_active, implicit_work) = match self.st.job(id) {
            Some(j) if j.state.is_terminal() => return false,
            Some(j) => (
                j.state,
                j.kind,
                j.memory_bytes,
                j.class,
                j.state == JobState::Active,
                // A completed job with a declared total has, by definition, now
                // executed all of it (indivisible jobs report no slices).
                // `remaining_work` is already decremented by every slice report,
                // so the unreported remainder *is* the whole remaining total —
                // subtracting `executed_work` again here would double-count it
                // and silently under-report completed work and baseline samples.
                if completed { j.remaining_work.unwrap_or(0) } else { 0 },
            ),
            None => return false,
        };
        let _ = state;
        if was_active {
            self.st.tree.remove_active(id);
            self.st.resources.release(kind, mem);
        } else {
            self.st.tree.unqueue(id, class);
        }
        if let Some(j) = self.st.job_mut(id) {
            j.state = if completed { JobState::Completed } else { JobState::Cancelled };
            j.executed_work = j.executed_work.saturating_add(implicit_work);
            j.remaining_work = j.remaining_work.map(|r| r.saturating_sub(implicit_work));
        }
        self.st.metrics.work_executed = self.st.metrics.work_executed.saturating_add(implicit_work);
        if completed {
            self.policy.on_complete(&mut self.st, id, now);
            self.st.metrics.completed += 1;
            self.st.log(id, DecisionKind::Completed);
        } else {
            self.st.metrics.cancelled += 1;
            self.st.log(id, DecisionKind::Cancelled);
        }
        self.st.jobs.remove(&id);
        true
    }

    /// Slots currently granted for adaptive handler execution (integration hint).
    #[must_use]
    pub fn handler_slots(&self) -> usize {
        self.st.resources.handler_slots()
    }
}
