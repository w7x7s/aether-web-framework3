use std::time::Duration;

/// Abstract unit of work. Only drivers measure it; the engine never assumes a unit
/// costs a fixed amount of time or bytes.
pub type Work = u64;

/// Monotonic identifier for an admitted or rejected job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId(pub u64);

/// What kind of execution resource a job holds. Connection and Handler jobs are
/// indivisible by contract: they occupy their slot until completion. `Divisible`
/// jobs are driver-managed work that yields at safe points between slices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Connection,
    Handler,
    CpuHeavy,
    Divisible,
}

impl JobKind {
    #[must_use]
    pub fn is_divisible(self) -> bool {
        self == Self::Divisible
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Queued,
    Active,
    Completed,
    Cancelled,
    Rejected,
}

impl JobState {
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Rejected)
    }
}

/// Size class used by the adaptive tree. Classification always carries hysteresis;
/// see `Baseline::classify`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobClass {
    Small,
    Large,
    Unknown,
}

impl JobClass {
    #[must_use]
    pub fn index(self) -> usize {
        match self {
            Self::Small => 0,
            Self::Large => 1,
            Self::Unknown => 2,
        }
    }
}

/// Description of a job handed to [`SchedulerEngine::admit`].
#[derive(Debug, Clone, Copy)]
pub struct NewJob {
    pub kind: JobKind,
    /// Caller-declared expected work. Advisory: classification re-checks against
    /// executed work after every slice.
    pub work_hint: Option<Work>,
    /// Bytes this job registers against the memory budget (buffers it holds while
    /// active). Released exactly once on the terminal transition.
    pub memory_bytes: u64,
}

impl NewJob {
    #[must_use]
    pub fn connection(memory_bytes: u64) -> Self {
        Self { kind: JobKind::Connection, work_hint: None, memory_bytes }
    }

    #[must_use]
    pub fn handler(memory_bytes: u64) -> Self {
        Self { kind: JobKind::Handler, work_hint: None, memory_bytes }
    }

    #[must_use]
    pub fn cpu_heavy(memory_bytes: u64) -> Self {
        Self { kind: JobKind::CpuHeavy, work_hint: None, memory_bytes }
    }

    #[must_use]
    pub fn divisible(work_hint: Option<Work>, memory_bytes: u64) -> Self {
        Self { kind: JobKind::Divisible, work_hint, memory_bytes }
    }
}

/// Full scheduling-visible state of one job.
#[derive(Debug, Clone)]
pub struct JobRecord {
    pub id: JobId,
    /// Monotonic arrival sequence; the deterministic tie-breaker for arrival precedence.
    pub seq: u64,
    pub kind: JobKind,
    pub state: JobState,
    pub class: JobClass,
    pub arrival: Duration,
    pub admitted_at: Option<Duration>,
    pub work_hint: Option<Work>,
    pub executed_work: Work,
    /// `None` while the total is unknown; unknown jobs converge to a class from
    /// executed work instead of being guessed.
    pub remaining_work: Option<Work>,
    pub total_runtime: Duration,
    pub slices: u32,
    pub memory_bytes: u64,

    /// Learned work rate in work/ms, EWMA over measured slices. `None` until the
    /// first slice report.
    pub rate_ewma: Option<f64>,
    /// Wall time of the last reported slice; the clock the wait-boost runs from,
    /// so a job that keeps executing is never treated as starving.
    pub last_executed_at: Option<Duration>,
    /// Multiplicative quota adaptation (fast decrease, slow ramp), clamped by the
    /// quota model.
    pub adapt_multiplier: f64,
    /// Quota granted in the previous round.
    pub last_grant: Work,
    /// Set by the engine each round so a policy cannot grant a job two slices in
    /// one round.
    pub(crate) granted_this_round: bool,
}

impl JobRecord {
    pub(crate) fn new(id: JobId, seq: u64, job: &NewJob, arrival: Duration) -> Self {
        Self {
            id,
            seq,
            kind: job.kind,
            state: JobState::Queued,
            class: JobClass::Unknown,
            arrival,
            admitted_at: None,
            work_hint: job.work_hint,
            executed_work: 0,
            remaining_work: job.work_hint,
            total_runtime: Duration::ZERO,
            slices: 0,
            memory_bytes: job.memory_bytes,
            rate_ewma: None,
            last_executed_at: None,
            adapt_multiplier: 1.0,
            last_grant: 0,
            granted_this_round: false,
        }
    }

    /// Best current estimate of this job's total work for classification: the
    /// declared hint corrected upward by executed work. A hint is advisory; when
    /// the driver overshoots it, the observed total wins, otherwise a job that
    /// declared "small" and turned out huge would never be reclassified.
    #[must_use]
    pub fn measured_work(&self) -> Work {
        match self.work_hint {
            Some(hint) => hint.max(self.executed_work),
            None => self.executed_work,
        }
    }

    /// Remaining work as far as grants are concerned. Once a declared hint has
    /// been fully executed the remainder is no longer a bound — jobs that keep
    /// running past their hint must keep receiving grants (a zero clamp here
    /// would stall them forever), and the driver remains the authority on when
    /// the job is actually done.
    #[must_use]
    pub fn grantable_remaining(&self) -> Option<Work> {
        self.remaining_work.filter(|r| *r > 0)
    }

    /// Time without executed progress: since the last reported slice, falling
    /// back to admission (running, never-yet-sliced jobs) or arrival. This — not
    /// raw age — is what the starvation wait boost must reward.
    #[must_use]
    pub fn grant_wait(&self, now: Duration) -> Duration {
        let since = self.last_executed_at.unwrap_or(self.admitted_at.unwrap_or(self.arrival));
        now.saturating_sub(since)
    }

    /// Time accumulated since arrival (queue jobs) or since admission (active
    /// jobs): the delay signal used by the starvation guard and the wait boost.
    #[must_use]
    pub fn wait(&self, now: Duration) -> Duration {
        now.saturating_sub(self.admitted_at.unwrap_or(self.arrival))
    }

    /// Pure queueing delay, used for starvation accounting.
    #[must_use]
    pub fn queue_wait(&self, now: Duration) -> Duration {
        now.saturating_sub(self.arrival)
    }
}
