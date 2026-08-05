//! Termination records for spawned children, and the parent's view of them.
//!
//! A supervision handle is a logical capability naming a child task. Querying
//! one must answer *after* the child is gone — that is the whole point of the
//! operation — but by then `reclaim_dead_task` and `GraphTables::release` have
//! erased everything else the root knew about that task. So the outcome is
//! recorded here, at the moment of death, and outlives the task it describes.
//!
//! # Why a separate table rather than a field on the handle
//!
//! The handle lives in the parent's capability table, and the parent may hold
//! several. Writing the outcome into each holder's copy at death would mean
//! walking every table looking for handles naming the dying task, and would
//! make the answer depend on which copy was consulted. One record per dead
//! task, looked up by task id, has one writer and one truth.
//!
//! # Consumption
//!
//! `supervision_status` on a terminated child consumes the caller's handle
//! slot, matching `kernel/src/task/mod.rs::supervision_status`, so a parent
//! that collected an outcome cannot collect it twice. The *record* is not
//! consumed: two parents may hold handles to one child — `launch_sample_plane`
//! hands `sample-lender` a handle to `sample-receiver` that init also holds —
//! and each is owed the answer.
//!
//! Records are therefore never removed, which makes [`MAX_RECORDS`] a bound on
//! the tasks a boot may *ever* create rather than on those alive at once.
//! `TaskTable::reclaim` frees its entries while `next_id` keeps counting, so a
//! long-running graph that spawns and reaps repeatedly can exceed it while
//! never holding more than a few tasks.
//!
//! Past the bound `record` drops silently and `supervision_status` answers
//! `WouldBlock` forever — the parent-waits-forever failure this module exists
//! to prevent. It is not reachable by any declared seL4 generation, whose graphs
//! create a handful of tasks and exit, but the bound is a real one and it fails
//! in the wrong direction. Recorded as **B16** in `roadmap/00-backlog.md`; the
//! fix is to reclaim a record once every holder of a handle naming it has
//! collected or dropped, which needs a reference count this cutover does not
//! yet keep.

use crate::task::TaskId;

/// Terminated children one boot may record. Matches [`crate::task::MAX_TASKS`]:
/// every task that can die is one that was created, so a full task table is the
/// bound on records too.
pub const MAX_RECORDS: usize = crate::task::MAX_TASKS;

/// How a child ended. The discriminants are the wire values
/// `components/runtime/src/syscall.rs::supervision_status` decodes, and match
/// `kernel/src/syscall/mod.rs::sys_supervision_status`'s.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Termination {
    /// Called `exit` with this status.
    Exit(i64),
    /// Stopped by a fault the root supervised. The detail is the fault's
    /// reason code, not an address: an address would leak the child's layout
    /// to its parent.
    Fault(u64),
}

impl Termination {
    /// The `(kind, detail)` pair the operation answers with.
    pub const fn encode(self) -> (i64, u64) {
        match self {
            Self::Exit(status) => (0, status as u64),
            Self::Fault(reason) => (1, reason),
        }
    }
}

/// Every child this boot has seen end, and how.
pub struct Terminations {
    entries: [Option<(TaskId, Termination)>; MAX_RECORDS],
    len: usize,
}

impl Terminations {
    pub const fn new() -> Self {
        Self {
            entries: [None; MAX_RECORDS],
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Record how `task` ended.
    ///
    /// The first record for a task wins. A task cannot end twice, but the exit
    /// and fault paths both reclaim, and a task that faults while exiting must
    /// not have its recorded outcome rewritten by the second arm to run.
    ///
    /// A table this full silently drops the record rather than failing the
    /// boot: the consequence is a parent whose `supervision_status` keeps
    /// answering "still live", which is a bounded wrong answer, whereas
    /// aborting the graph over a bookkeeping bound would be a worse one. The
    /// bound is [`MAX_RECORDS`], which no graph within `MAX_TASKS` can reach.
    pub fn record(&mut self, task: TaskId, termination: Termination) {
        if self.get(task).is_some() {
            return;
        }
        if let Some(slot) = self.entries.iter_mut().find(|entry| entry.is_none()) {
            *slot = Some((task, termination));
            self.len += 1;
        }
    }

    /// How `task` ended, or `None` while it is still live.
    pub fn get(&self, task: TaskId) -> Option<Termination> {
        self.entries
            .iter()
            .flatten()
            .find(|(id, _)| *id == task)
            .map(|(_, termination)| *termination)
    }
}

impl Default for Terminations {
    fn default() -> Self {
        Self::new()
    }
}

/// Registrations one boot may hold: every task may wait on every source in one
/// wait set, and a task holds at most one parked wait at a time.
pub const MAX_WAITS: usize = crate::task::MAX_TASKS * crate::ipc::MAX_WAIT_SOURCES;

/// Which parked tasks are waiting on which child's termination.
///
/// A channel wait registers on the queue itself, because a queue is the thing
/// that becomes ready. A supervision wait has no queue: the readiness event is
/// a task dying, which is observed in the dispatcher rather than in any table.
/// So the registration lives here, and [`Self::waiters_for`] is what the death
/// path consults.
///
/// Registrations are dropped whenever the waiter's channel registrations are,
/// so the two halves of one wait set are always cleared together. A stale
/// entry would wake a task that is no longer parked, which `deliver_wake`
/// already tolerates but which would make the boot's counts wrong.
pub struct SupervisionWaits {
    entries: [Option<(TaskId, TaskId)>; MAX_WAITS],
    len: usize,
}

impl SupervisionWaits {
    pub const fn new() -> Self {
        Self {
            entries: [None; MAX_WAITS],
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Register `waiter` as parked on `child`'s termination.
    ///
    /// Idempotent: a wait set naming one child twice registers once, so the
    /// death path does not answer the same parked reply twice.
    pub fn register(&mut self, waiter: TaskId, child: TaskId) {
        if self
            .entries
            .iter()
            .flatten()
            .any(|(task, on)| *task == waiter && *on == child)
        {
            return;
        }
        if let Some(slot) = self.entries.iter_mut().find(|entry| entry.is_none()) {
            *slot = Some((waiter, child));
            self.len += 1;
        }
    }

    /// Every task registered on `child`'s termination.
    pub fn waiters_for(&self, child: TaskId) -> impl Iterator<Item = TaskId> + '_ {
        self.entries
            .iter()
            .flatten()
            .filter(move |(_, on)| *on == child)
            .map(|(waiter, _)| *waiter)
    }

    /// Drop every registration `waiter` holds.
    pub fn clear(&mut self, waiter: TaskId) {
        for entry in self.entries.iter_mut() {
            if entry.is_some_and(|(task, _)| task == waiter) {
                *entry = None;
                self.len -= 1;
            }
        }
    }
}

impl Default for SupervisionWaits {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_RECORDS, Termination, Terminations};
    use crate::task::TaskId;

    #[test]
    fn a_live_child_has_no_record() {
        let terminations = Terminations::new();
        assert_eq!(terminations.get(TaskId(3)), None);
    }

    #[test]
    fn an_outcome_survives_the_task_it_describes() {
        let mut terminations = Terminations::new();
        terminations.record(TaskId(3), Termination::Exit(0));

        assert_eq!(terminations.get(TaskId(3)), Some(Termination::Exit(0)));
        assert_eq!(
            terminations.get(TaskId(4)),
            None,
            "one task's outcome is not another's"
        );
    }

    #[test]
    fn a_record_is_readable_twice() {
        // Two parents may hold handles to one child, and each is owed the
        // answer; reading is not consuming.
        let mut terminations = Terminations::new();
        terminations.record(TaskId(1), Termination::Fault(7));

        assert_eq!(terminations.get(TaskId(1)), Some(Termination::Fault(7)));
        assert_eq!(terminations.get(TaskId(1)), Some(Termination::Fault(7)));
    }

    #[test]
    fn the_first_outcome_wins() {
        let mut terminations = Terminations::new();
        terminations.record(TaskId(1), Termination::Exit(0));
        terminations.record(TaskId(1), Termination::Fault(7));

        assert_eq!(
            terminations.get(TaskId(1)),
            Some(Termination::Exit(0)),
            "a second arm reclaiming the same task cannot rewrite how it ended"
        );
        assert_eq!(terminations.len(), 1);
    }

    #[test]
    fn a_full_table_drops_rather_than_panicking() {
        let mut terminations = Terminations::new();
        for index in 0..MAX_RECORDS {
            terminations.record(TaskId(index as u32), Termination::Exit(0));
        }
        terminations.record(TaskId(MAX_RECORDS as u32), Termination::Exit(1));

        assert_eq!(terminations.len(), MAX_RECORDS);
        assert_eq!(terminations.get(TaskId(MAX_RECORDS as u32)), None);
    }

    /// The single waiter registered on `child`, asserting there is exactly one.
    fn collect_one(waits: &super::SupervisionWaits, child: TaskId) -> [TaskId; 1] {
        let mut found = None;
        for waiter in waits.waiters_for(child) {
            assert!(found.is_none(), "expected exactly one waiter");
            found = Some(waiter);
        }
        [found.expect("a registered waiter")]
    }

    #[test]
    fn a_supervision_wait_is_registered_and_found_by_the_death_path() {
        let mut waits = super::SupervisionWaits::new();
        waits.register(TaskId(1), TaskId(2));

        let woken: [TaskId; 1] = collect_one(&waits, TaskId(2));
        assert_eq!(woken, [TaskId(1)]);
        assert_eq!(
            waits.waiters_for(TaskId(3)).count(),
            0,
            "another child's death wakes nobody"
        );
    }

    #[test]
    fn registering_the_same_child_twice_wakes_once() {
        let mut waits = super::SupervisionWaits::new();
        waits.register(TaskId(1), TaskId(2));
        waits.register(TaskId(1), TaskId(2));

        assert_eq!(waits.len(), 1);
        assert_eq!(waits.waiters_for(TaskId(2)).count(), 1);
    }

    #[test]
    fn clearing_a_waiter_drops_every_registration_it_held() {
        let mut waits = super::SupervisionWaits::new();
        waits.register(TaskId(1), TaskId(2));
        waits.register(TaskId(1), TaskId(3));
        waits.register(TaskId(4), TaskId(2));

        waits.clear(TaskId(1));

        assert_eq!(waits.len(), 1);
        assert_eq!(waits.waiters_for(TaskId(3)).count(), 0);
        assert_eq!(
            collect_one(&waits, TaskId(2)),
            [TaskId(4)],
            "another task's registration survives"
        );
    }

    #[test]
    fn the_encoding_matches_the_component_abi() {
        // `slime_rt::supervision_status` decodes 0 as an exit and 1 as a fault.
        assert_eq!(Termination::Exit(-2).encode(), (0, (-2i64) as u64));
        assert_eq!(Termination::Fault(9).encode(), (1, 9));
    }
}
