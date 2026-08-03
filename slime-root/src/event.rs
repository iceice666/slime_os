//! Architecture-neutral scheduling events emitted by `slime-root`.
//!
//! These records normalize mechanism transitions for deterministic observation.
//! They describe readiness and timer ordering only; they do not assert temporal
//! isolation, CPU reservations, or execution-time guarantees.

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MonotonicInstant(pub u64);

impl MonotonicInstant {
    pub fn checked_add(self, ticks: u64) -> Option<Self> {
        self.0.checked_add(ticks).map(Self)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TaskEpoch {
    pub task: u32,
    pub epoch: u32,
}

impl TaskEpoch {
    pub const fn new(task: u32, epoch: u32) -> Self {
        Self { task, epoch }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TimerId(pub u64);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EventSequence(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimerCancelReason {
    Explicit,
    TaskDeath,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadyCause {
    Timer {
        timer: TimerId,
        deadline: MonotonicInstant,
    },
    Notification {
        badge: u64,
    },
    Yield,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulingEventKind {
    TimerScheduled {
        timer: TimerId,
        task: TaskEpoch,
        deadline: MonotonicInstant,
    },
    TimerCancelled {
        timer: TimerId,
        task: TaskEpoch,
        reason: TimerCancelReason,
    },
    TaskReady {
        task: TaskEpoch,
        cause: ReadyCause,
    },
    StaleTimerDiscarded {
        timer: TimerId,
        task: TaskEpoch,
        deadline: MonotonicInstant,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulingEvent {
    pub sequence: EventSequence,
    pub observed_at: MonotonicInstant,
    pub kind: SchedulingEventKind,
}

#[derive(Debug, Eq, PartialEq)]
pub struct EventBatch<const CAPACITY: usize> {
    entries: [Option<SchedulingEvent>; CAPACITY],
    len: usize,
}

impl<const CAPACITY: usize> EventBatch<CAPACITY> {
    pub const fn new() -> Self {
        Self {
            entries: [const { None }; CAPACITY],
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn capacity(&self) -> usize {
        CAPACITY
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, index: usize) -> Option<SchedulingEvent> {
        if index >= self.len {
            return None;
        }
        self.entries[index]
    }

    pub fn iter(&self) -> impl Iterator<Item = &SchedulingEvent> {
        self.entries[..self.len].iter().filter_map(Option::as_ref)
    }

    /// Append an event, returning `false` when the batch is already full.
    /// Producers size `CAPACITY` to their own bound, so a `false` result is an
    /// internal sizing bug rather than an expected runtime condition.
    #[must_use]
    pub(crate) fn push(&mut self, event: SchedulingEvent) -> bool {
        let Some(slot) = self.entries.get_mut(self.len) else {
            return false;
        };
        *slot = Some(event);
        self.len += 1;
        true
    }
}

impl<const CAPACITY: usize> Default for EventBatch<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}
