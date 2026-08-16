//! Termination records for spawned children, and the parent's mediated view.
//!
//! A record outlives the task it describes and remains while a live logical
//! supervision handle can name it. Native Notifications replaced root wait
//! registrations; collection remains a root-mediated policy operation.

use crate::task::TaskId;

/// Termination records that may be *awaiting collection* at once.
///
/// Matches [`crate::task::MAX_TASKS`]. Not a bound on how many tasks a boot may
/// create: [`sweep`] reclaims records no live holder can name, so this bounds
/// the outcomes owed simultaneously rather than cumulatively.
pub const MAX_RECORDS: usize = crate::task::MAX_TASKS;

/// How a child ended. The discriminants are the wire values
/// `components/runtime/src/syscall.rs::supervision_status` decodes; see
/// `docs/syscall-abi.md`.
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

/// Reclaim every record no live task-owned supervision capability can observe.
pub fn sweep<const TASKS: usize>(
    terminations: &mut Terminations,
    tasks: &crate::task::TaskTable<TASKS>,
) -> usize {
    let mut freed = 0;
    for entry in terminations.entries.iter_mut() {
        let Some((task, _)) = *entry else { continue };
        if tasks.holds_supervision(task) {
            continue;
        }
        *entry = None;
        freed += 1;
    }
    terminations.len = terminations.len.saturating_sub(freed);
    freed
}

#[cfg(test)]
mod tests {
    use super::{MAX_RECORDS, Termination, Terminations};
    use crate::task::TaskId;

    #[test]
    fn outcomes_are_bounded_and_first_writer_wins() {
        let mut records = Terminations::new();
        assert!(records.record(TaskId(1), Termination::Exit(0)));
        assert!(records.record(TaskId(1), Termination::Fault(7)));
        assert_eq!(records.get(TaskId(1)), Some(Termination::Exit(0)));
        for index in 2..=MAX_RECORDS {
            assert!(records.record(TaskId(index as u32), Termination::Exit(0)));
        }
        assert!(!records.record(TaskId((MAX_RECORDS + 1) as u32), Termination::Exit(0)));
    }

    #[test]
    fn records_without_a_live_handle_are_swept() {
        let mut records = Terminations::new();
        assert!(records.record(TaskId(7), Termination::Fault(9)));
        let tasks = crate::task::TaskTable::<1>::new();
        assert_eq!(super::sweep(&mut records, &tasks), 1);
        assert!(records.is_empty());
    }
}
