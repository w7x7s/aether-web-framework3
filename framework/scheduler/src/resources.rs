use std::time::Duration;

use crate::config::ResourceConfig;
use crate::job::JobKind;

/// Budgets and adaptive capacity (design §6).
///
/// Measured by the platform: `available_parallelism()` plus the framework's own
/// exact gauges (open connections, running handlers, cpu jobs, registered bytes).
/// Configured, never presented as measurement: `max_connections`, the handler and
/// cpu ceilings, and the registered-memory budget — the OS gives no reliable
/// free-memory figure here, so `memory_budget_bytes` is a registration ceiling.
#[derive(Debug)]
pub struct Resources {
    cfg: ResourceConfig,
    cores: usize,
    handler_ceiling: usize,
    cpu_ceiling: usize,
    divisible_slots: usize,
    /// Adaptive handler execution slots; starts at the ceiling and only shrinks
    /// under sustained pressure.
    handler_slots: usize,
    conn_used: usize,
    handler_used: usize,
    cpu_used: usize,
    divisible_used: usize,
    mem_used: u64,
    high_streak: u32,
    low_streak: u32,
    last_adjust: Option<Duration>,
    capacity_changes: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct ResourceSnapshot {
    pub cores: usize,
    pub handler_slots: usize,
    pub handler_ceiling: usize,
    pub divisible_slots: usize,
    pub active_connections: usize,
    pub active_handlers: usize,
    pub active_cpu: usize,
    pub active_divisible: usize,
    pub registered_memory_bytes: u64,
    pub capacity_changes: usize,
}

impl Resources {
    #[must_use]
    pub fn new(cfg: ResourceConfig) -> Self {
        let cores = std::thread::available_parallelism().map(|v| v.get()).unwrap_or(1);
        let handler_ceiling = cfg.max_active_handlers.unwrap_or(4 * cores).max(1);
        let cpu_ceiling = cfg.max_cpu_heavy.unwrap_or(cores).max(1);
        let divisible_slots = cfg.max_active_divisible.unwrap_or(4).max(1);
        let handler_slots = handler_ceiling.max(cfg.min_slots.max(1));
        Self {
            cfg,
            cores,
            handler_ceiling,
            cpu_ceiling,
            divisible_slots,
            handler_slots,
            conn_used: 0,
            handler_used: 0,
            cpu_used: 0,
            divisible_used: 0,
            mem_used: 0,
            high_streak: 0,
            low_streak: 0,
            last_adjust: None,
            capacity_changes: 0,
        }
    }

    /// A job whose own registration exceeds the whole budget can never be admitted.
    #[must_use]
    pub fn over_ceiling(&self, memory_bytes: u64) -> bool {
        memory_bytes > self.cfg.memory_budget_bytes
    }

    #[must_use]
    pub fn memory_available(&self) -> u64 {
        self.cfg.memory_budget_bytes.saturating_sub(self.mem_used)
    }

    /// Slot + memory check for promotion. False means "no capacity right now"; the
    /// job simply stays queued (backpressure), it is not rejected by this call.
    #[must_use]
    pub fn try_admit(&mut self, kind: JobKind, memory_bytes: u64) -> bool {
        if self.mem_used.saturating_add(memory_bytes) > self.cfg.memory_budget_bytes {
            return false;
        }
        let slot_ok = match kind {
            JobKind::Connection => {
                self.conn_used < self.cfg.max_connections
            }
            JobKind::Handler => self.handler_used < self.handler_slots,
            JobKind::CpuHeavy => self.cpu_used < self.cpu_ceiling,
            // Divisible jobs use their own static budget (see ResourceConfig docs):
            // a background job cannot starve HTTP handler capacity.
            JobKind::Divisible => self.divisible_used < self.divisible_slots,
        };
        if !slot_ok {
            return false;
        }
        match kind {
            JobKind::Connection => self.conn_used += 1,
            JobKind::Handler => self.handler_used += 1,
            JobKind::CpuHeavy => self.cpu_used += 1,
            JobKind::Divisible => self.divisible_used += 1,
        }
        self.mem_used += memory_bytes;
        true
    }

    /// Release exactly once per terminal transition (engine guarantees this).
    pub fn release(&mut self, kind: JobKind, memory_bytes: u64) {
        match kind {
            JobKind::Connection => self.conn_used = self.conn_used.saturating_sub(1),
            JobKind::Handler => self.handler_used = self.handler_used.saturating_sub(1),
            JobKind::CpuHeavy => self.cpu_used = self.cpu_used.saturating_sub(1),
            JobKind::Divisible => self.divisible_used = self.divisible_used.saturating_sub(1),
        }
        self.mem_used = self.mem_used.saturating_sub(memory_bytes);
    }

    /// One adaptation tick. `queue_ratio` is `queued_jobs / max_queue` supplied by
    /// the engine; pressure is `queue_ratio + reject_rate`, clamped to [0, 2]. High
    /// pressure for `sustain_intervals` consecutive ticks halves the slots (down to
    /// `min_slots`); low pressure for the same streak adds one slot back (up to the
    /// configured ceiling). At most one change per `adapt_interval`, which is what
    /// prevents fast oscillation loops.
    ///
    /// Note: shrinking is multiplicative (×0.5) and growth additive (+1) by design —
    /// recover slowly, shed quickly.
    pub fn refresh(&mut self, now: Duration, queue_ratio: f64, rejected_in_window: usize) -> Option<usize> {
        let q = queue_ratio.clamp(0.0, 1.0);
        let r = (rejected_in_window.min(100)) as f64 / 100.0;
        let pressure = (q + r).clamp(0.0, 2.0);

        if pressure > self.cfg.high_pressure {
            self.high_streak += 1;
            self.low_streak = 0;
        } else if pressure < self.cfg.low_pressure {
            self.low_streak += 1;
            self.high_streak = 0;
        } else {
            self.high_streak = 0;
            self.low_streak = 0;
        }

        let cooled = self.last_adjust.is_none_or(|t| now.saturating_sub(t) >= self.cfg.adapt_interval);
        if !cooled {
            return None;
        }

        let sustain = self.cfg.sustain_intervals.max(1);
        let min_slots = self.cfg.min_slots.max(1);
        if self.high_streak >= sustain && self.handler_slots > min_slots {
            self.handler_slots = (self.handler_slots / 2).max(min_slots);
        } else if self.low_streak >= sustain && self.handler_slots < self.handler_ceiling {
            self.handler_slots += 1;
        } else {
            return None;
        }
        self.high_streak = 0;
        self.low_streak = 0;
        self.last_adjust = Some(now);
        self.capacity_changes += 1;
        Some(self.handler_slots)
    }

    #[must_use]
    pub fn handler_slots(&self) -> usize {
        self.handler_slots
    }

    #[must_use]
    pub fn snapshot(&self) -> ResourceSnapshot {
        ResourceSnapshot {
            cores: self.cores,
            handler_slots: self.handler_slots,
            handler_ceiling: self.handler_ceiling,
            divisible_slots: self.divisible_slots,
            active_connections: self.conn_used,
            active_handlers: self.handler_used,
            active_cpu: self.cpu_used,
            active_divisible: self.divisible_used,
            registered_memory_bytes: self.mem_used,
            capacity_changes: self.capacity_changes,
        }
    }
}
