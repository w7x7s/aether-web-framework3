use std::collections::VecDeque;

use crate::job::{JobClass, JobId};

/// The adaptive tree's structure: a root (owned by the engine) indexing three
/// class nodes — `SMALL`, `LARGE`, `UNKNOWN` — with job leaves under their class.
/// A round is one deterministic traversal of this tree (design §4).
///
/// Queue membership operations scan linearly; queues are bounded by
/// `SchedulingConfig::max_queue` (default 1024), which keeps the scans cheap and
/// the traversal deterministic.
#[derive(Debug, Default)]
pub struct JobTree {
    queued: [VecDeque<JobId>; 3],
    /// Active divisible jobs in rotation order.
    pub active: Vec<JobId>,
    /// Rotation cursor advanced by round-robin style policies.
    pub rotation_cursor: usize,
}

impl JobTree {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn queue(&mut self, id: JobId, class: JobClass) {
        self.queued[class.index()].push_back(id);
    }

    #[must_use]
    pub fn is_queued(&self, id: JobId, class: JobClass) -> bool {
        self.queued[class.index()].contains(&id)
    }

    pub fn unqueue(&mut self, id: JobId, class: JobClass) -> bool {
        let node = &mut self.queued[class.index()];
        match node.iter().position(|j| *j == id) {
            Some(p) => {
                node.remove(p);
                true
            }
            None => false,
        }
    }

    pub fn reclassify(&mut self, id: JobId, from: JobClass, to: JobClass) {
        if from == to {
            return;
        }
        self.unqueue(id, from);
        // Keep arrival precedence: insert into the target class preserving seq order
        // is the engine's responsibility (callers re-push; ordering across classes is
        // resolved by a seq sort at pick time, so tail insertion is sufficient).
        self.queue(id, to);
    }

    pub fn promote_active(&mut self, id: JobId) {
        self.active.push(id);
    }

    pub fn remove_active(&mut self, id: JobId) -> bool {
        match self.active.iter().position(|j| *j == id) {
            Some(p) => {
                self.active.remove(p);
                if self.rotation_cursor > 0 && self.rotation_cursor >= self.active.len() {
                    self.rotation_cursor = self.active.len().saturating_sub(1);
                }
                true
            }
            None => false,
        }
    }

    #[must_use]
    pub fn queued_count(&self) -> usize {
        self.queued.iter().map(|n| n.len()).sum()
    }

    #[must_use]
    pub fn active_len(&self) -> usize {
        self.active.len()
    }

    #[must_use]
    pub fn queued_ids(&self) -> Vec<JobId> {
        self.queued.iter().flatten().copied().collect()
    }
}
