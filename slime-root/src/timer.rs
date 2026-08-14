//! Bounded timer and wake scheduling semantics for a non-MCS seL4 runtime.
//!
//! The queue and its transitions are pure mechanism over a fixed-capacity
//! array: no allocation, no interior mutability, no ambient clock. Platform
//! access is confined to [`PlatformTimer`].
//!
//! # What is observed
//!
//! `crate::platform_timer::PhysicalTimerAdapter` implements that trait against
//! the EL1 physical timer (`CNTP_*`, PPI 30) and `slime-root`'s startup drives
//! it: the boot gate (`scripts/check/check-sel4-root-boot.py`) requires ordered
//! serial evidence that the root task claimed that IRQ through
//! `IRQControl_GetTrigger`, bound it to a notification, had one deadline
//! programmed here delivered as a real interrupt, acknowledged it device-first
//! then through `IRQHandler_Ack`, drained exactly one wake out of this queue,
//! and saw the monotonic counter advance across the wait. Hardware delivery
//! into this state machine is therefore established, not assumed.
//!
//! # What is still not established
//!
//! No temporal isolation: nothing bounds how long another thread may keep the
//! CPU between the compare condition becoming true and this queue being
//! serviced. No CPU reservation: non-MCS seL4 has no budget to charge, so a
//! wake only re-enters the ready order. No deadline guarantee: a programmed
//! deadline is a one-shot compare, and lateness is neither bounded nor
//! reported. Only one deadline is ever armed in hardware — the earliest queued
//! one — and the ordering of wake decisions is all this module promises.

use crate::event::{
    EventBatch, EventSequence, MonotonicInstant, ReadyCause, SchedulingEvent, SchedulingEventKind,
    TaskEpoch, TimerCancelReason, TimerId,
};

/// What the caller must do to the one-shot platform deadline after a
/// transition. Computed from the earliest queued deadline before and after.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeadlineProgramming {
    Unchanged,
    Program(MonotonicInstant),
    Disarm,
}

impl DeadlineProgramming {
    fn between(before: Option<MonotonicInstant>, after: Option<MonotonicInstant>) -> Self {
        if before == after {
            Self::Unchanged
        } else if let Some(deadline) = after {
            Self::Program(deadline)
        } else {
            Self::Disarm
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimerError {
    /// The fixed deadline queue is full; the caller must shed or wait.
    CapacityExhausted {
        capacity: usize,
    },
    /// Timer ids, tie-order stamps, or event sequence numbers wrapped.
    IdentitySpaceExhausted,
    /// `now + ticks` does not fit the monotonic domain.
    DurationOverflow {
        now: MonotonicInstant,
        ticks: u64,
    },
    /// The adapter reported a time earlier than a previously observed one.
    MonotonicRegression {
        previous: MonotonicInstant,
        observed: MonotonicInstant,
    },
    TimerNotFound(TimerId),
    /// Cancellation was attempted by a task epoch that does not own the timer,
    /// including a reused task index carrying a newer epoch.
    TimerOwnerMismatch {
        timer: TimerId,
        expected: TaskEpoch,
        actual: TaskEpoch,
    },
}

/// Error returned by [`TimerScheduler::service_timer_source`].
///
/// A platform step that fails *after* [`TimerScheduler::on_timer_expiry`] has
/// already normalized due timers out of the queue carries the computed
/// [`TimerTransition`] alongside the error. Those timers are gone from the
/// queue and their deadlines cannot be recomputed on a retry, so the wake
/// events already decided for them would be silently lost unless the caller
/// can still observe them despite the platform failure.
#[derive(Debug, Eq, PartialEq)]
pub enum ServiceTimerError<E, const CAPACITY: usize> {
    /// The platform clock could not be read. Nothing was mutated.
    Clock(E),
    /// The scheduler rejected the observed time (see [`TimerError`]).
    /// Nothing was mutated.
    Scheduler(TimerError),
    /// The next deadline could not be programmed or disarmed. Due timers
    /// were already removed from the queue; `transition.events` still holds
    /// every wake this expiry decided.
    Program {
        error: E,
        transition: TimerTransition<CAPACITY>,
    },
    /// The IRQ could not be acknowledged after the next deadline was
    /// programmed. Due timers were already removed from the queue;
    /// `transition.events` still holds every wake this expiry decided.
    Acknowledge {
        error: E,
        transition: TimerTransition<CAPACITY>,
    },
}

/// Platform timer/IRQ operations, kept behind a trait so the queue above stays
/// host-testable against a fake. The live implementation is
/// `crate::platform_timer::PhysicalTimerAdapter` over the EL1 physical timer;
/// implementing this trait is not by itself evidence that a notification path
/// exists — the boot gate's ordered `SLIME_TIMER` markers are.
pub trait PlatformTimer {
    type Error;

    /// Monotonic counter in the same units as programmed deadlines. An adapter
    /// over a narrower wrapping counter must widen it or fail; the scheduler
    /// never infers a wrap.
    fn monotonic_now(&mut self) -> Result<MonotonicInstant, Self::Error>;

    /// Program one absolute deadline. A deadline already in the past must
    /// still raise the source promptly rather than being dropped.
    fn program_deadline(&mut self, deadline: MonotonicInstant) -> Result<(), Self::Error>;

    /// Stop delivery entirely when no deadline remains queued.
    fn disarm_timer(&mut self) -> Result<(), Self::Error>;

    /// Clear the device source and acknowledge the IRQ, called only after the
    /// next deadline state has been installed so no wake window is left open.
    fn acknowledge_timer_irq(&mut self) -> Result<(), Self::Error>;
}

pub fn apply_deadline_programming<P: PlatformTimer>(
    platform: &mut P,
    decision: DeadlineProgramming,
) -> Result<(), P::Error> {
    match decision {
        DeadlineProgramming::Unchanged => Ok(()),
        DeadlineProgramming::Program(deadline) => platform.program_deadline(deadline),
        DeadlineProgramming::Disarm => platform.disarm_timer(),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TimerEntry {
    id: TimerId,
    owner: TaskEpoch,
    deadline: MonotonicInstant,
    /// Monotonically increasing stamp giving equal deadlines a stable order.
    tie_order: u64,
}

#[derive(Debug, Eq, PartialEq)]
pub struct TimerTransition<const CAPACITY: usize> {
    pub events: EventBatch<CAPACITY>,
    pub programming: DeadlineProgramming,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WakeDisposition {
    Ready,
    /// The wake named a task epoch that is no longer live; a reused task index
    /// with a newer epoch is never woken by it.
    StaleEpoch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WakeTransition {
    pub disposition: WakeDisposition,
    pub event: Option<SchedulingEvent>,
}

/// Fixed-capacity deadline queue kept sorted by `(deadline, tie_order)`.
///
/// Insert and pop shift at most `CAPACITY` inline `Option<TimerEntry>` slots
/// and allocate nothing. Every emitted record carries a total-ordered sequence
/// number, so replaying identical inputs yields identical event order.
#[derive(Debug, Eq, PartialEq)]
pub struct TimerScheduler<const CAPACITY: usize> {
    entries: [Option<TimerEntry>; CAPACITY],
    len: usize,
    next_timer_id: u64,
    next_tie_order: u64,
    next_event_sequence: u64,
    last_observed: Option<MonotonicInstant>,
    pending_programming: Option<DeadlineProgramming>,
}

impl<const CAPACITY: usize> TimerScheduler<CAPACITY> {
    pub const fn new() -> Self {
        Self {
            entries: [const { None }; CAPACITY],
            len: 0,
            next_timer_id: 0,
            next_tie_order: 0,
            next_event_sequence: 0,
            last_observed: None,
            pending_programming: None,
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

    pub fn next_deadline(&self) -> Option<MonotonicInstant> {
        self.entry(0).map(|entry| entry.deadline)
    }

    /// Number of timers currently owned by `owner`.
    pub fn timers_of(&self, owner: TaskEpoch) -> usize {
        self.entries().filter(|entry| entry.owner == owner).count()
    }

    pub fn schedule_after(
        &mut self,
        owner: TaskEpoch,
        now: MonotonicInstant,
        ticks: u64,
    ) -> Result<(TimerId, TimerTransition<1>), TimerError> {
        let deadline = now
            .checked_add(ticks)
            .ok_or(TimerError::DurationOverflow { now, ticks })?;
        self.schedule_at(owner, now, deadline)
    }

    pub fn schedule_at(
        &mut self,
        owner: TaskEpoch,
        now: MonotonicInstant,
        deadline: MonotonicInstant,
    ) -> Result<(TimerId, TimerTransition<1>), TimerError> {
        self.check_observation(now)?;
        if self.len == CAPACITY {
            return Err(TimerError::CapacityExhausted { capacity: CAPACITY });
        }
        let id = TimerId(self.reserve_timer_id()?);
        let tie_order = self.reserve_tie_order()?;
        let sequence = EventSequence(self.reserve_event_sequences(1)?);

        let before = self.next_deadline();
        let entry = TimerEntry {
            id,
            owner,
            deadline,
            tie_order,
        };
        self.insert_sorted(entry);
        self.commit_observation(now);

        let mut events = EventBatch::new();
        let pushed = events.push(SchedulingEvent {
            sequence,
            observed_at: now,
            kind: SchedulingEventKind::TimerScheduled {
                timer: id,
                task: owner,
                deadline,
            },
        });
        debug_assert!(pushed, "one scheduled record always fits a 1-slot batch");
        Ok((
            id,
            TimerTransition {
                events,
                programming: DeadlineProgramming::between(before, self.next_deadline()),
            },
        ))
    }

    pub fn cancel(
        &mut self,
        timer: TimerId,
        owner: TaskEpoch,
        now: MonotonicInstant,
    ) -> Result<TimerTransition<1>, TimerError> {
        self.check_observation(now)?;
        let index = self
            .find_timer(timer)
            .ok_or(TimerError::TimerNotFound(timer))?;
        let entry = self.entry(index).ok_or(TimerError::TimerNotFound(timer))?;
        if entry.owner != owner {
            return Err(TimerError::TimerOwnerMismatch {
                timer,
                expected: entry.owner,
                actual: owner,
            });
        }
        let sequence = EventSequence(self.reserve_event_sequences(1)?);

        let before = self.next_deadline();
        self.remove_at(index);
        self.commit_observation(now);

        let mut events = EventBatch::new();
        let pushed = events.push(SchedulingEvent {
            sequence,
            observed_at: now,
            kind: SchedulingEventKind::TimerCancelled {
                timer,
                task: owner,
                reason: TimerCancelReason::Explicit,
            },
        });
        debug_assert!(pushed, "one cancel record always fits a 1-slot batch");
        Ok(TimerTransition {
            events,
            programming: DeadlineProgramming::between(before, self.next_deadline()),
        })
    }

    /// Cancel every timer owned by exactly this dead task epoch.
    ///
    /// Cancellations are emitted in queue order (deadline, then insertion), so
    /// the record stream is deterministic. Timers owned by the same task index
    /// under a different epoch belong to a different incarnation and survive.
    pub fn cancel_task(
        &mut self,
        dead: TaskEpoch,
        now: MonotonicInstant,
    ) -> Result<TimerTransition<CAPACITY>, TimerError> {
        self.check_observation(now)?;
        let cancel_count = self.timers_of(dead);
        let first_sequence = self.reserve_event_sequences(cancel_count)?;

        let before = self.next_deadline();
        let mut events = EventBatch::new();
        let mut index = 0;
        let mut emitted = 0_u64;

        while index < self.len {
            let Some(entry) = self.entry(index) else {
                break;
            };
            if entry.owner == dead {
                self.remove_at(index);
                let pushed = events.push(SchedulingEvent {
                    sequence: EventSequence(first_sequence.wrapping_add(emitted)),
                    observed_at: now,
                    kind: SchedulingEventKind::TimerCancelled {
                        timer: entry.id,
                        task: entry.owner,
                        reason: TimerCancelReason::TaskDeath,
                    },
                });
                debug_assert!(pushed, "cancels are bounded by queue occupancy");
                emitted = emitted.wrapping_add(1);
            } else {
                index += 1;
            }
        }
        self.commit_observation(now);

        Ok(TimerTransition {
            events,
            programming: DeadlineProgramming::between(before, self.next_deadline()),
        })
    }

    /// Turn one observation of the timer source into zero or more wakes.
    ///
    /// Every entry whose deadline is at or before `now` is popped in queue
    /// order. An entry whose captured epoch is no longer live yields a
    /// `StaleTimerDiscarded` record instead of waking the reused task index.
    pub fn on_timer_expiry<F>(
        &mut self,
        now: MonotonicInstant,
        mut epoch_is_live: F,
    ) -> Result<TimerTransition<CAPACITY>, TimerError>
    where
        F: FnMut(TaskEpoch) -> bool,
    {
        self.check_observation(now)?;
        let due_count = self
            .entries()
            .take_while(|entry| entry.deadline <= now)
            .count();
        let first_sequence = self.reserve_event_sequences(due_count)?;

        let before = self.next_deadline();
        let mut events = EventBatch::new();
        let mut emitted = 0_u64;

        for _ in 0..due_count {
            let Some(entry) = self.remove_at(0) else {
                break;
            };
            let kind = if epoch_is_live(entry.owner) {
                SchedulingEventKind::TaskReady {
                    task: entry.owner,
                    cause: ReadyCause::Timer {
                        timer: entry.id,
                        deadline: entry.deadline,
                    },
                }
            } else {
                SchedulingEventKind::StaleTimerDiscarded {
                    timer: entry.id,
                    task: entry.owner,
                    deadline: entry.deadline,
                }
            };
            let pushed = events.push(SchedulingEvent {
                sequence: EventSequence(first_sequence.wrapping_add(emitted)),
                observed_at: now,
                kind,
            });
            debug_assert!(pushed, "expiries are bounded by queue occupancy");
            emitted = emitted.wrapping_add(1);
        }
        self.commit_observation(now);

        Ok(TimerTransition {
            events,
            programming: DeadlineProgramming::between(before, self.next_deadline()),
        })
    }

    /// Service a wake already attributed to the timer adapter: read time,
    /// normalize expiries, install the next one-shot deadline, then
    /// acknowledge the source. `slime-root`'s startup reaches this call from a
    /// real, observed timer interrupt; see the module docs for exactly what
    /// that establishes and what it does not.
    ///
    /// A platform failure while reading the clock, or while the scheduler
    /// rejects the observed time, leaves the queue untouched: nothing here
    /// mutated the schedule and the caller may retry with a fresh
    /// observation. A platform failure while programming the next deadline
    /// or acknowledging the IRQ happens strictly after due timers have
    /// already been popped out of the queue, so [`ServiceTimerError::Program`]
    /// and [`ServiceTimerError::Acknowledge`] carry the already-computed
    /// [`TimerTransition`] rather than discarding it: those wake events can
    /// never be recomputed once the backing timers are gone, so the caller
    /// must still be able to act on them even though the platform step that
    /// reported this expiry failed.
    pub fn service_timer_source<P, F>(
        &mut self,
        platform: &mut P,
        epoch_is_live: F,
    ) -> Result<TimerTransition<CAPACITY>, ServiceTimerError<P::Error, CAPACITY>>
    where
        P: PlatformTimer,
        F: FnMut(TaskEpoch) -> bool,
    {
        let now = platform.monotonic_now().map_err(ServiceTimerError::Clock)?;
        let mut transition = self
            .on_timer_expiry(now, epoch_is_live)
            .map_err(ServiceTimerError::Scheduler)?;
        if let Some(pending) = self.pending_programming {
            transition.programming = pending;
        }
        if let Err(error) = apply_deadline_programming(platform, transition.programming) {
            self.pending_programming = Some(transition.programming);
            return Err(ServiceTimerError::Program { error, transition });
        }
        self.pending_programming = None;
        if let Err(error) = platform.acknowledge_timer_irq() {
            return Err(ServiceTimerError::Acknowledge { error, transition });
        }
        Ok(transition)
    }

    /// Normalize a badged notification wake for `task`.
    pub fn on_notification<F>(
        &mut self,
        task: TaskEpoch,
        badge: u64,
        now: MonotonicInstant,
        mut epoch_is_live: F,
    ) -> Result<WakeTransition, TimerError>
    where
        F: FnMut(TaskEpoch) -> bool,
    {
        let cause = epoch_is_live(task).then_some(ReadyCause::Notification { badge });
        self.normalize_wake(task, now, cause)
    }

    /// Normalize a voluntary yield. Non-MCS seL4 has no budget to charge, so a
    /// yield only re-enters the ready order; it grants no execution guarantee.
    pub fn on_yield<F>(
        &mut self,
        task: TaskEpoch,
        now: MonotonicInstant,
        mut epoch_is_live: F,
    ) -> Result<WakeTransition, TimerError>
    where
        F: FnMut(TaskEpoch) -> bool,
    {
        let cause = epoch_is_live(task).then_some(ReadyCause::Yield);
        self.normalize_wake(task, now, cause)
    }

    fn normalize_wake(
        &mut self,
        task: TaskEpoch,
        now: MonotonicInstant,
        cause: Option<ReadyCause>,
    ) -> Result<WakeTransition, TimerError> {
        self.check_observation(now)?;
        let Some(cause) = cause else {
            self.commit_observation(now);
            return Ok(WakeTransition {
                disposition: WakeDisposition::StaleEpoch,
                event: None,
            });
        };
        let sequence = EventSequence(self.reserve_event_sequences(1)?);
        self.commit_observation(now);
        Ok(WakeTransition {
            disposition: WakeDisposition::Ready,
            event: Some(SchedulingEvent {
                sequence,
                observed_at: now,
                kind: SchedulingEventKind::TaskReady { task, cause },
            }),
        })
    }

    fn entries(&self) -> impl Iterator<Item = TimerEntry> + '_ {
        self.entries[..self.len].iter().filter_map(|entry| *entry)
    }

    fn entry(&self, index: usize) -> Option<TimerEntry> {
        if index >= self.len {
            return None;
        }
        self.entries[index]
    }

    fn find_timer(&self, timer: TimerId) -> Option<usize> {
        self.entries().position(|entry| entry.id == timer)
    }

    /// Shift-insert keeping `(deadline, tie_order)` ascending. Equal deadlines
    /// keep insertion order because `tie_order` only increases.
    fn insert_sorted(&mut self, entry: TimerEntry) {
        let insert_at = self
            .entries()
            .position(|queued| {
                (entry.deadline, entry.tie_order) < (queued.deadline, queued.tie_order)
            })
            .unwrap_or(self.len);
        let mut index = self.len;
        while index > insert_at {
            self.entries[index] = self.entries[index - 1];
            index -= 1;
        }
        self.entries[insert_at] = Some(entry);
        self.len += 1;
    }

    fn remove_at(&mut self, index: usize) -> Option<TimerEntry> {
        if index >= self.len {
            return None;
        }
        let removed = self.entries[index].take()?;
        let mut cursor = index;
        while cursor + 1 < self.len {
            self.entries[cursor] = self.entries[cursor + 1];
            cursor += 1;
        }
        self.len -= 1;
        self.entries[self.len] = None;
        Some(removed)
    }

    fn check_observation(&self, now: MonotonicInstant) -> Result<(), TimerError> {
        match self.last_observed {
            Some(previous) if now < previous => Err(TimerError::MonotonicRegression {
                previous,
                observed: now,
            }),
            _ => Ok(()),
        }
    }

    fn commit_observation(&mut self, now: MonotonicInstant) {
        self.last_observed = Some(now);
    }

    fn reserve_timer_id(&mut self) -> Result<u64, TimerError> {
        let id = self.next_timer_id;
        self.next_timer_id = id
            .checked_add(1)
            .ok_or(TimerError::IdentitySpaceExhausted)?;
        Ok(id)
    }

    fn reserve_tie_order(&mut self) -> Result<u64, TimerError> {
        let stamp = self.next_tie_order;
        self.next_tie_order = stamp
            .checked_add(1)
            .ok_or(TimerError::IdentitySpaceExhausted)?;
        Ok(stamp)
    }

    fn reserve_event_sequences(&mut self, count: usize) -> Result<u64, TimerError> {
        let count = u64::try_from(count).map_err(|_| TimerError::IdentitySpaceExhausted)?;
        let first = self.next_event_sequence;
        self.next_event_sequence = first
            .checked_add(count)
            .ok_or(TimerError::IdentitySpaceExhausted)?;
        Ok(first)
    }
}

impl<const CAPACITY: usize> Default for TimerScheduler<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: TaskEpoch = TaskEpoch::new(1, 0);
    const B: TaskEpoch = TaskEpoch::new(2, 0);
    /// Task index 1 reincarnated: same index, newer epoch.
    const A_REBORN: TaskEpoch = TaskEpoch::new(1, 1);

    fn at(ticks: u64) -> MonotonicInstant {
        MonotonicInstant(ticks)
    }

    fn kinds<const N: usize>(
        transition: &TimerTransition<N>,
    ) -> impl Iterator<Item = SchedulingEventKind> + '_ {
        transition.events.iter().map(|event| event.kind)
    }

    fn live_all(_: TaskEpoch) -> bool {
        true
    }

    #[derive(Debug, Default, Eq, PartialEq)]
    struct FakeTimer {
        now: u64,
        programmed: Option<MonotonicInstant>,
        disarms: usize,
        acks: usize,
    }

    impl PlatformTimer for FakeTimer {
        type Error = ();

        fn monotonic_now(&mut self) -> Result<MonotonicInstant, Self::Error> {
            Ok(MonotonicInstant(self.now))
        }

        fn program_deadline(&mut self, deadline: MonotonicInstant) -> Result<(), Self::Error> {
            self.programmed = Some(deadline);
            Ok(())
        }

        fn disarm_timer(&mut self) -> Result<(), Self::Error> {
            self.programmed = None;
            self.disarms += 1;
            Ok(())
        }

        fn acknowledge_timer_irq(&mut self) -> Result<(), Self::Error> {
            self.acks += 1;
            Ok(())
        }
    }

    #[test]
    fn earlier_deadline_reprograms_and_later_one_does_not() {
        let mut scheduler = TimerScheduler::<4>::new();
        let (_, first) = scheduler.schedule_after(A, at(0), 100).unwrap();
        assert_eq!(first.programming, DeadlineProgramming::Program(at(100)));

        let (_, later) = scheduler.schedule_after(B, at(0), 200).unwrap();
        assert_eq!(later.programming, DeadlineProgramming::Unchanged);

        let (_, earlier) = scheduler.schedule_after(B, at(0), 10).unwrap();
        assert_eq!(earlier.programming, DeadlineProgramming::Program(at(10)));
        assert_eq!(scheduler.next_deadline(), Some(at(10)));
    }

    #[test]
    fn equal_deadlines_fire_in_insertion_order() {
        let mut scheduler = TimerScheduler::<4>::new();
        let (first, _) = scheduler.schedule_at(A, at(0), at(50)).unwrap();
        let (second, _) = scheduler.schedule_at(B, at(0), at(50)).unwrap();
        let (third, _) = scheduler.schedule_at(A, at(0), at(50)).unwrap();

        let fired = scheduler.on_timer_expiry(at(50), live_all).unwrap();
        let order: [TimerId; 3] =
            core::array::from_fn(|index| match fired.events.get(index).unwrap().kind {
                SchedulingEventKind::TaskReady {
                    cause: ReadyCause::Timer { timer, .. },
                    ..
                } => timer,
                other => panic!("unexpected event: {other:?}"),
            });
        assert_eq!(order, [first, second, third]);
        assert_eq!(fired.programming, DeadlineProgramming::Disarm);
    }

    #[test]
    fn event_sequences_are_dense_and_increasing() {
        let mut scheduler = TimerScheduler::<4>::new();
        let (_, a) = scheduler.schedule_at(A, at(0), at(5)).unwrap();
        let (_, b) = scheduler.schedule_at(B, at(0), at(6)).unwrap();
        let fired = scheduler.on_timer_expiry(at(6), live_all).unwrap();

        assert_eq!(a.events.get(0).unwrap().sequence, EventSequence(0));
        assert_eq!(b.events.get(0).unwrap().sequence, EventSequence(1));
        assert_eq!(fired.events.get(0).unwrap().sequence, EventSequence(2));
        assert_eq!(fired.events.get(1).unwrap().sequence, EventSequence(3));
    }

    #[test]
    fn stale_epoch_timer_never_wakes_the_reused_task_index() {
        let mut scheduler = TimerScheduler::<4>::new();
        scheduler.schedule_at(A, at(0), at(10)).unwrap();

        let fired = scheduler
            .on_timer_expiry(at(10), |task| task == A_REBORN)
            .unwrap();
        assert!(matches!(
            fired.events.get(0).unwrap().kind,
            SchedulingEventKind::StaleTimerDiscarded { task, .. } if task == A
        ));
        assert!(kinds(&fired).all(|kind| !matches!(kind, SchedulingEventKind::TaskReady { .. })));
    }

    #[test]
    fn task_death_cancels_only_that_incarnation() {
        let mut scheduler = TimerScheduler::<4>::new();
        scheduler.schedule_at(A, at(0), at(10)).unwrap();
        scheduler.schedule_at(A_REBORN, at(0), at(20)).unwrap();
        scheduler.schedule_at(B, at(0), at(30)).unwrap();

        let cancelled = scheduler.cancel_task(A, at(1)).unwrap();
        assert_eq!(cancelled.events.len(), 1);
        assert_eq!(cancelled.programming, DeadlineProgramming::Program(at(20)));
        assert_eq!(scheduler.timers_of(A), 0);
        assert_eq!(scheduler.timers_of(A_REBORN), 1);
        assert_eq!(scheduler.len(), 2);
    }

    #[test]
    fn task_death_cancellations_follow_queue_order() {
        let mut scheduler = TimerScheduler::<4>::new();
        let (late, _) = scheduler.schedule_at(A, at(0), at(30)).unwrap();
        let (early, _) = scheduler.schedule_at(A, at(0), at(10)).unwrap();

        let cancelled = scheduler.cancel_task(A, at(1)).unwrap();
        let order: [TimerId; 2] =
            core::array::from_fn(|index| match cancelled.events.get(index).unwrap().kind {
                SchedulingEventKind::TimerCancelled { timer, .. } => timer,
                other => panic!("unexpected event: {other:?}"),
            });
        assert_eq!(order, [early, late]);
        assert_eq!(cancelled.programming, DeadlineProgramming::Disarm);
        assert!(scheduler.is_empty());
    }

    #[test]
    fn cancel_rejects_a_foreign_or_reincarnated_owner() {
        let mut scheduler = TimerScheduler::<2>::new();
        let (timer, _) = scheduler.schedule_at(A, at(0), at(10)).unwrap();

        assert_eq!(
            scheduler.cancel(timer, A_REBORN, at(1)),
            Err(TimerError::TimerOwnerMismatch {
                timer,
                expected: A,
                actual: A_REBORN,
            })
        );
        assert_eq!(scheduler.len(), 1);

        let cancelled = scheduler.cancel(timer, A, at(1)).unwrap();
        assert_eq!(cancelled.programming, DeadlineProgramming::Disarm);
        assert_eq!(
            scheduler.cancel(timer, A, at(1)),
            Err(TimerError::TimerNotFound(timer))
        );
    }

    #[test]
    fn capacity_and_overflow_are_explicit_errors() {
        let mut scheduler = TimerScheduler::<2>::new();
        scheduler.schedule_at(A, at(0), at(1)).unwrap();
        scheduler.schedule_at(A, at(0), at(2)).unwrap();
        assert_eq!(
            scheduler.schedule_at(A, at(0), at(3)),
            Err(TimerError::CapacityExhausted { capacity: 2 })
        );

        let mut wide = TimerScheduler::<2>::new();
        assert_eq!(
            wide.schedule_after(A, at(u64::MAX - 1), 5),
            Err(TimerError::DurationOverflow {
                now: at(u64::MAX - 1),
                ticks: 5,
            })
        );
    }

    #[test]
    fn a_time_regression_is_rejected_rather_than_wrapped() {
        let mut scheduler = TimerScheduler::<2>::new();
        scheduler.schedule_at(A, at(100), at(200)).unwrap();
        assert_eq!(
            scheduler.on_timer_expiry(at(99), live_all),
            Err(TimerError::MonotonicRegression {
                previous: at(100),
                observed: at(99),
            })
        );
        assert_eq!(scheduler.len(), 1);
    }

    #[test]
    fn late_observation_drains_every_passed_deadline_at_once() {
        let mut scheduler = TimerScheduler::<4>::new();
        scheduler.schedule_at(A, at(0), at(10)).unwrap();
        scheduler.schedule_at(A, at(0), at(20)).unwrap();
        scheduler.schedule_at(B, at(0), at(90)).unwrap();

        let fired = scheduler.on_timer_expiry(at(50), live_all).unwrap();
        assert_eq!(fired.events.len(), 2);
        assert_eq!(fired.programming, DeadlineProgramming::Program(at(90)));
    }

    #[test]
    fn notification_and_yield_wakes_are_distinct_and_epoch_checked() {
        let mut scheduler = TimerScheduler::<2>::new();

        let woken = scheduler.on_notification(A, 0x40, at(1), live_all).unwrap();
        assert_eq!(woken.disposition, WakeDisposition::Ready);
        assert!(matches!(
            woken.event.unwrap().kind,
            SchedulingEventKind::TaskReady {
                cause: ReadyCause::Notification { badge: 0x40 },
                ..
            }
        ));

        let yielded = scheduler.on_yield(A, at(2), live_all).unwrap();
        assert!(matches!(
            yielded.event.unwrap().kind,
            SchedulingEventKind::TaskReady {
                cause: ReadyCause::Yield,
                ..
            }
        ));

        let stale = scheduler
            .on_notification(A, 0x40, at(3), |task| task == A_REBORN)
            .unwrap();
        assert_eq!(stale.disposition, WakeDisposition::StaleEpoch);
        assert!(stale.event.is_none());
    }

    #[test]
    fn servicing_the_adapter_programs_then_acknowledges() {
        let mut scheduler = TimerScheduler::<4>::new();
        let mut platform = FakeTimer::default();

        scheduler.schedule_at(A, at(0), at(10)).unwrap();
        scheduler.schedule_at(B, at(0), at(40)).unwrap();
        platform.program_deadline(at(10)).unwrap();

        platform.now = 10;
        let fired = scheduler
            .service_timer_source(&mut platform, live_all)
            .unwrap();
        assert_eq!(fired.events.len(), 1);
        assert_eq!(platform.programmed, Some(at(40)));
        assert_eq!(platform.acks, 1);

        platform.now = 40;
        scheduler
            .service_timer_source(&mut platform, live_all)
            .unwrap();
        assert_eq!(platform.programmed, None);
        assert_eq!(platform.disarms, 1);
        assert_eq!(platform.acks, 2);
    }

    /// Wraps [`FakeTimer`] but can be told to fail acknowledgement, to prove
    /// [`TimerScheduler::service_timer_source`] never discards wake events
    /// decided before a post-mutation platform failure.
    #[derive(Debug, Default)]
    struct FailingAckTimer {
        inner: FakeTimer,
        fail_ack: bool,
    }

    impl PlatformTimer for FailingAckTimer {
        type Error = ();

        fn monotonic_now(&mut self) -> Result<MonotonicInstant, Self::Error> {
            self.inner.monotonic_now()
        }

        fn program_deadline(&mut self, deadline: MonotonicInstant) -> Result<(), Self::Error> {
            self.inner.program_deadline(deadline)
        }

        fn disarm_timer(&mut self) -> Result<(), Self::Error> {
            self.inner.disarm_timer()
        }

        fn acknowledge_timer_irq(&mut self) -> Result<(), Self::Error> {
            if self.fail_ack {
                return Err(());
            }
            self.inner.acknowledge_timer_irq()
        }
    }

    #[test]
    fn a_failed_acknowledge_after_expiry_still_returns_the_computed_wakes() {
        // Regression for the ordering bug where `on_timer_expiry` already
        // popped the due timer and built its wake event before the platform
        // step failed, and the old `Result<_, PlatformTimerError<E>>` return
        // type had nowhere to put that event: it was silently dropped and
        // could never be recomputed, because the timer backing it is gone.
        let mut scheduler = TimerScheduler::<4>::new();
        let mut platform = FailingAckTimer {
            fail_ack: true,
            ..FailingAckTimer::default()
        };

        scheduler.schedule_at(A, at(0), at(10)).unwrap();
        platform.inner.now = 10;

        let error = scheduler
            .service_timer_source(&mut platform, live_all)
            .unwrap_err();
        match error {
            ServiceTimerError::Acknowledge { transition, .. } => {
                assert_eq!(transition.events.len(), 1);
                assert!(matches!(
                    transition.events.get(0).unwrap().kind,
                    SchedulingEventKind::TaskReady { task, .. } if task == A
                ));
            }
            other => panic!("unexpected error: {other:?}"),
        }
        // The due timer is gone from the queue even though acknowledgement
        // failed, which is exactly why its wake event had to travel inside
        // the error above instead of being recomputed later.
        assert!(scheduler.is_empty());
    }

    #[derive(Debug, Default)]
    struct FailingProgramTimer {
        inner: FakeTimer,
        fail_program: bool,
    }

    impl PlatformTimer for FailingProgramTimer {
        type Error = ();

        fn monotonic_now(&mut self) -> Result<MonotonicInstant, Self::Error> {
            self.inner.monotonic_now()
        }

        fn program_deadline(&mut self, deadline: MonotonicInstant) -> Result<(), Self::Error> {
            if self.fail_program {
                return Err(());
            }
            self.inner.program_deadline(deadline)
        }

        fn disarm_timer(&mut self) -> Result<(), Self::Error> {
            self.inner.disarm_timer()
        }

        fn acknowledge_timer_irq(&mut self) -> Result<(), Self::Error> {
            self.inner.acknowledge_timer_irq()
        }
    }

    #[test]
    fn failed_programming_is_retried_even_when_queue_deadline_is_unchanged() {
        let mut scheduler = TimerScheduler::<4>::new();
        let mut platform = FailingProgramTimer {
            fail_program: true,
            ..FailingProgramTimer::default()
        };
        scheduler.schedule_at(A, at(0), at(10)).unwrap();
        scheduler.schedule_at(B, at(0), at(40)).unwrap();
        platform.inner.now = 10;

        let first = scheduler
            .service_timer_source(&mut platform, live_all)
            .unwrap_err();
        assert!(matches!(
            first,
            ServiceTimerError::Program {
                transition: TimerTransition {
                    programming: DeadlineProgramming::Program(deadline),
                    ..
                },
                ..
            } if deadline == at(40)
        ));

        platform.fail_program = false;
        platform.inner.now = 11;
        let retried = scheduler
            .service_timer_source(&mut platform, live_all)
            .expect("retry reprograms hardware");
        assert_eq!(retried.programming, DeadlineProgramming::Program(at(40)));
        assert_eq!(platform.inner.programmed, Some(at(40)));
    }

    #[test]
    fn identical_input_streams_replay_identical_event_streams() {
        fn run() -> TimerScheduler<8> {
            let mut scheduler = TimerScheduler::<8>::new();
            scheduler.schedule_at(A, at(0), at(30)).unwrap();
            scheduler.schedule_at(B, at(0), at(30)).unwrap();
            scheduler.schedule_at(A_REBORN, at(0), at(10)).unwrap();
            scheduler.cancel_task(A, at(5)).unwrap();
            scheduler.on_timer_expiry(at(30), live_all).unwrap();
            scheduler.on_yield(B, at(31), live_all).unwrap();
            scheduler
        }
        assert_eq!(run(), run());
    }
}
