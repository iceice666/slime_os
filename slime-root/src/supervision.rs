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
//! A record is therefore not consumed by any single reader. It is reclaimed
//! when *no reader remains*: [`sweep`] frees every record that no live holder
//! can still name. So [`MAX_RECORDS`] bounds the records **observable at once**,
//! not the tasks a boot may ever create — a long-running graph that spawns and
//! reaps repeatedly is bounded by how many outcomes are owed simultaneously,
//! which is a property of the graph rather than of its age.
//!
//! # Why a derived sweep rather than a reference count
//!
//! The set of live holders is already represented: a record is observable
//! exactly when some holder holds a `Resource::Supervision` naming it, in a
//! live [`CapabilityTable`](crate::graph::CapabilityTable) or parked in
//! [`Transit`](crate::transit::Transit). Deriving it is the same choice, for the
//! same reason, as [`TaskTable::live_children`](crate::task::TaskTable), and it
//! fails in the safe direction: a count that misses a decrement leaks a record
//! forever, whereas a sweep that fails to run merely leaves a record in place,
//! still answering correctly, until the next one needs the slot.
//!
//! Both holders must be consulted. A capability mid-transfer is held by no
//! table at all — that is what `Transit` owns — so a sweep reading only the
//! graph would free a record whose handle is in flight. See
//! [`Transit::holds_supervision`](crate::transit::Transit::holds_supervision).
//!
//! This closes backlog **B16**.

use crate::task::TaskId;

/// Termination records that may be *awaiting collection* at once.
///
/// Matches [`crate::task::MAX_TASKS`]. Not a bound on how many tasks a boot may
/// create: [`sweep`] reclaims records no live holder can name, so this bounds
/// the outcomes owed simultaneously rather than cumulatively.
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
    /// Records ever written, never decremented.
    ///
    /// Split from `len` because [`sweep`] makes the live count a measure of
    /// what is *owed* rather than of what happened, and a boot's transcript
    /// needs the latter: a graph that recorded and collected forty outcomes
    /// ends with `len` at zero, which is indistinguishable from a supervision
    /// path that never ran.
    recorded: usize,
}

impl Terminations {
    pub const fn new() -> Self {
        Self {
            entries: [None; MAX_RECORDS],
            len: 0,
            recorded: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// How many records this boot has written, including ones since reclaimed.
    pub const fn recorded(&self) -> usize {
        self.recorded
    }

    /// Record how `task` ended.
    ///
    /// The first record for a task wins. A task cannot end twice, but the exit
    /// and fault paths both reclaim, and a task that faults while exiting must
    /// not have its recorded outcome rewritten by the second arm to run.
    ///
    /// Reports whether the outcome is now recorded. `false` means the table was
    /// full: the caller should [`sweep`] and retry, and report the loss if the
    /// retry also fails. Dropping a record silently is the
    /// parent-waits-forever failure this module exists to prevent, so the
    /// result is deliberately not ignorable.
    #[must_use = "a dropped termination record makes a parent wait forever"]
    pub fn record(&mut self, task: TaskId, termination: Termination) -> bool {
        if self.get(task).is_some() {
            return true;
        }
        let Some(slot) = self.entries.iter_mut().find(|entry| entry.is_none()) else {
            return false;
        };
        *slot = Some((task, termination));
        self.len += 1;
        self.recorded += 1;
        true
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

/// Reclaim every record no live holder can still observe, reporting how many
/// were freed.
///
/// A free function rather than a method: `supervision` would otherwise depend
/// on `graph` and `transit`, and both call sites already hold `graph` mutably.
///
/// # What makes a record unobservable
///
/// Both readers of [`Terminations::get`] are reached only through a handle the
/// caller holds. `serve_supervision_status` resolves the record through a slot
/// in the caller's own table, so a record no holder can *name* is one no holder
/// can read. The wait-readiness probe reads a `TaskId` directly, but that id can
/// only have come from a `WaitTarget::Supervision` built by
/// `resolve_wait_source`, which resolved `RIGHT_SUPERVISE` against the caller's
/// table one syscall earlier. A waiter that has since dropped its handle is
/// precisely a waiter that can no longer collect an outcome.
///
/// Parked waits are therefore *not* an input here. A parked wait does not imply
/// a live handle — nothing stops a waiter dropping its slot after registering —
/// and a waiter with no handle has no way to observe the record it is parked on.
///
/// # Ordering against teardown
///
/// Both call sites record *before* the teardown that erases the dying task:
/// `GraphTables::release` and, inside `reclaim_dead_task`, `Transit::reclaim`.
/// So a sweep fired while a task is dying still sees that task's own table and
/// its in-flight entries as live, and will not collect records it holds handles
/// for. That is the safe direction: the records stay, keep answering correctly,
/// and are collected by the next sweep once the teardown has run.
///
/// The consequence worth stating is that one sweep may free less than the
/// theoretical maximum. It cannot free too much, which is the only direction
/// that would be a defect.
pub fn sweep(
    terminations: &mut Terminations,
    graph: &crate::graph::GraphTables,
    transit: &crate::transit::Transit,
) -> usize {
    let mut freed = 0;
    for entry in terminations.entries.iter_mut() {
        let Some((task, _)) = *entry else { continue };
        if graph.holds_supervision(task) || transit.holds_supervision(task) {
            continue;
        }
        *entry = None;
        freed += 1;
    }
    // `saturating_sub` rather than `-=`: `freed` counts entries that were
    // `Some`, and every such entry was counted in `len`, so the two cannot
    // disagree. But this is a `no_std` root task, where a wrap would not panic
    // — it would make `len` enormous and `is_empty` permanently false, turning
    // a bookkeeping slip into a boot that misreports its own teardown.
    terminations.len = terminations.len.saturating_sub(freed);
    freed
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
        assert!(terminations.record(TaskId(3), Termination::Exit(0)));

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
        assert!(terminations.record(TaskId(1), Termination::Fault(7)));

        assert_eq!(terminations.get(TaskId(1)), Some(Termination::Fault(7)));
        assert_eq!(terminations.get(TaskId(1)), Some(Termination::Fault(7)));
    }

    #[test]
    fn the_first_outcome_wins() {
        let mut terminations = Terminations::new();
        assert!(terminations.record(TaskId(1), Termination::Exit(0)));
        assert!(terminations.record(TaskId(1), Termination::Fault(7)));

        assert_eq!(
            terminations.get(TaskId(1)),
            Some(Termination::Exit(0)),
            "a second arm reclaiming the same task cannot rewrite how it ended"
        );
        assert_eq!(terminations.len(), 1);
    }

    #[test]
    fn a_full_table_reports_the_drop_rather_than_panicking() {
        let mut terminations = Terminations::new();
        for index in 0..MAX_RECORDS {
            assert!(terminations.record(TaskId(index as u32), Termination::Exit(0)));
        }
        assert!(
            !terminations.record(TaskId(MAX_RECORDS as u32), Termination::Exit(1)),
            "a full table reports the loss so the caller can sweep and retry"
        );

        assert_eq!(terminations.len(), MAX_RECORDS);
        assert_eq!(terminations.get(TaskId(MAX_RECORDS as u32)), None);
    }

    /// Authority to observe a spawned child's termination; see
    /// `main.rs::RIGHT_SUPERVISE`, which this must agree with. Matches the
    /// same local copy `channel.rs` keeps, for the same reason: the constant
    /// lives in the binary root and is not importable from a module.
    const RIGHT_SUPERVISE: u64 = 1 << 18;

    /// A supervision handle naming `child`.
    fn handle(child: TaskId) -> crate::graph::Capability {
        crate::graph::Capability {
            resource: crate::graph::Resource::Supervision { task: child },
            rights: RIGHT_SUPERVISE,
        }
    }

    /// A graph holding one supervision handle, in `holder`'s table, naming
    /// `child`.
    fn graph_holding(holder: TaskId, child: TaskId) -> crate::graph::GraphTables {
        let mut graph = crate::graph::GraphTables::new();
        let table = graph.create(holder).expect("a fresh table");
        table.install(1, handle(child)).expect("an empty slot");
        graph
    }

    #[test]
    fn a_record_no_holder_can_name_is_reclaimed() {
        let mut terminations = Terminations::new();
        assert!(terminations.record(TaskId(7), Termination::Exit(0)));

        // No table holds a handle: the last holder collected or dropped it.
        let graph = crate::graph::GraphTables::new();
        let transit = crate::transit::Transit::new();

        assert_eq!(super::sweep(&mut terminations, &graph, &transit), 1);
        assert_eq!(terminations.len(), 0);
        assert_eq!(
            terminations.recorded(),
            1,
            "the cumulative count is what the boot marker reports, and a \
             reclaimed record still happened"
        );
    }

    #[test]
    fn a_record_survives_while_a_handle_remains() {
        // The two-parents case: one holder collecting must not free the record
        // the other is still owed.
        let mut terminations = Terminations::new();
        assert!(terminations.record(TaskId(7), Termination::Exit(0)));

        let graph = graph_holding(TaskId(2), TaskId(7));
        let transit = crate::transit::Transit::new();

        assert_eq!(super::sweep(&mut terminations, &graph, &transit), 0);
        assert_eq!(terminations.get(TaskId(7)), Some(Termination::Exit(0)));
    }

    #[test]
    fn a_record_survives_while_its_only_handle_is_parked_in_transit() {
        // The case a graph-only predicate would miss: mid-transfer, the
        // capability is held by no table at all, and freeing the record here
        // would leave the receiver waiting forever. B16, reintroduced by its
        // own fix.
        let mut terminations = Terminations::new();
        assert!(terminations.record(TaskId(7), Termination::Exit(0)));

        let graph = crate::graph::GraphTables::new();
        let mut transit = crate::transit::Transit::new();
        transit
            .depart(handle(TaskId(7)), TaskId(2), TaskId(3))
            .expect("transit has room");

        assert_eq!(super::sweep(&mut terminations, &graph, &transit), 0);
        assert_eq!(terminations.get(TaskId(7)), Some(Termination::Exit(0)));
    }

    #[test]
    fn a_swept_table_admits_more_records_than_it_holds_at_once() {
        // B16's exit condition in miniature: more tasks over a lifetime than
        // `MAX_RECORDS`, every one recorded, because each outcome is collected
        // before the next child dies.
        let mut terminations = Terminations::new();
        let graph = crate::graph::GraphTables::new();
        let transit = crate::transit::Transit::new();

        for index in 0..(MAX_RECORDS * 2) {
            let child = TaskId(index as u32);
            if !terminations.record(child, Termination::Exit(0)) {
                super::sweep(&mut terminations, &graph, &transit);
                assert!(
                    terminations.record(child, Termination::Exit(0)),
                    "the sweep frees records no holder can name"
                );
            }
        }

        assert_eq!(terminations.recorded(), MAX_RECORDS * 2);
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
