//! Which generation instance each running task represents.
//!
//! Not a channel concern, though it lived in `channel.rs` until B46's scoping
//! pass: the table answers "which declaration is this task" and "has this
//! declaration ever run", both of which outlive any IPC model. It was also
//! sized by `MAX_CHANNELS`, which bounded the wrong thing — a graph can
//! declare more instances than channels or fewer, and neither number
//! constrains the other.

use crate::generation::MAX_ADMITTED_INSTANCES as MAX_INSTANCES;
use crate::task::{MAX_TASKS, TaskId};

/// Why a launch could not be recorded.
///
/// Its own type rather than `ChannelError`: recording which declaration a task
/// represents has nothing to do with channels, and borrowing that error made
/// a full instance table report `UnlaidSlot`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaunchError {
    /// This instance already has a live task. A respawn must collect the dead
    /// one first.
    AlreadyLive,
    /// Every table entry is taken.
    TableFull,
}

/// Which launched generation instance and executable each task represents.
pub struct LaunchedInstances {
    entries: [Option<LaunchedInstance>; MAX_TASKS],
    len: usize,
    /// Which instances have ever been launched, kept past their collection.
    ///
    /// `entries` answers "is this instance live"; releasing a dead task clears
    /// it, which is right for liveness and wrong for provenance. A *respawn*
    /// is the same declaration launched again, and the spawn preflight has to
    /// tell it from a first launch: the declared grant set describes the first
    /// one, and a retry after collection carries whatever its owner still
    /// holds (B51).
    ///
    /// A bitmap rather than a list, because the only question is yes/no and
    /// the answer must outlive every table entry the instance had.
    launched_once: [bool; MAX_INSTANCES],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LaunchedInstance {
    pub instance: usize,
    pub executable: usize,
    pub task: TaskId,
}

impl LaunchedInstances {
    pub const fn new() -> Self {
        Self {
            entries: [None; MAX_TASKS],
            len: 0,
            launched_once: [false; MAX_INSTANCES],
        }
    }

    /// Whether `instance` has been launched at least once, live or not.
    pub fn ever_launched(&self, instance: usize) -> bool {
        self.launched_once.get(instance).copied().unwrap_or(false)
    }

    pub fn record(
        &mut self,
        instance: usize,
        executable: usize,
        task: TaskId,
    ) -> Result<(), LaunchError> {
        if self.task_for_instance(instance).is_some() {
            return Err(LaunchError::AlreadyLive);
        }
        let slot = self
            .entries
            .iter_mut()
            .find(|entry| entry.is_none())
            .ok_or(LaunchError::TableFull)?;
        *slot = Some(LaunchedInstance {
            instance,
            executable,
            task,
        });
        self.len += 1;
        if let Some(seen) = self.launched_once.get_mut(instance) {
            *seen = true;
        }
        Ok(())
    }

    pub fn task_for_instance(&self, instance: usize) -> Option<TaskId> {
        self.entries
            .iter()
            .flatten()
            .find(|launched| launched.instance == instance)
            .map(|launched| launched.task)
    }

    pub fn instance_for_task(&self, task: TaskId) -> Option<usize> {
        self.entries
            .iter()
            .flatten()
            .find(|launched| launched.task == task)
            .map(|launched| launched.instance)
    }

    pub fn release_by_task(&mut self, task: TaskId) -> Option<LaunchedInstance> {
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.is_some_and(|launched| launched.task == task))?;
        let released = entry.take();
        if released.is_some() {
            self.len = self.len.saturating_sub(1);
        }
        released
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = LaunchedInstance> + '_ {
        self.entries.iter().flatten().copied()
    }
}

impl Default for LaunchedInstances {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{LaunchError, LaunchedInstances};
    use crate::task::TaskId;

    #[test]
    fn a_live_declared_instance_cannot_be_recorded_twice() {
        let mut launched = LaunchedInstances::new();
        launched.record(3, 2, TaskId(7)).expect("first launch");
        assert_eq!(
            launched.record(3, 2, TaskId(8)),
            Err(LaunchError::AlreadyLive),
            "a declaration with a live task is not launchable again"
        );
        assert_eq!(launched.task_for_instance(3), Some(TaskId(7)));
        assert_eq!(launched.len(), 1);
    }

    #[test]
    fn provenance_outlives_the_table_entry() {
        // B51: releasing a dead task clears its entry, which is right for
        // liveness and wrong for provenance. The spawn preflight tells a
        // respawn from a first launch by this bit, so it must survive
        // collection.
        let mut launched = LaunchedInstances::new();
        launched.record(1, 0, TaskId(4)).expect("first launch");
        assert!(launched.ever_launched(1));

        assert!(launched.release_by_task(TaskId(4)).is_some());
        assert_eq!(launched.task_for_instance(1), None, "the task is collected");
        assert!(
            launched.ever_launched(1),
            "but the declaration is still known to have run"
        );
    }
}
