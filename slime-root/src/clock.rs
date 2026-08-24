//! C9.1 root-brokered clock, timer, and simulated-time authority.
//!
//! The generation-authenticated resource resolves authority by instance name.
//! Runtime state is keyed by `TaskId`, which is never reused, so timer ownership
//! remains fresh across a supervised restart without a second epoch database.

use boot_contracts::clock_authority::{ClockAuthority, holder_identity};
use boot_contracts::generation::{
    Generation, RIGHT_CLOCK_MONOTONIC_READ, RIGHT_CLOCK_SIMULATED_ADVANCE,
    RIGHT_CLOCK_SIMULATED_READ, RIGHT_CLOCK_TIMER_USE,
};

use crate::event::{MonotonicInstant, SchedulingEventKind, TaskEpoch, TimerId};
use crate::task::TaskId;
use crate::timer::{
    DeadlineProgramming, PlatformTimer, ServiceTimerError, TimerError, TimerScheduler,
};

pub const MAX_LIVE_TIMERS: usize = boot_contracts::clock_authority::MAX_LIVE_TIMERS;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClockError {
    Undeclared,
    Malformed,
    InvalidNotification,
    TimerLimit,
    TimerNotFound,
    TimeOverflow,
    SimulatedTimeOverflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskClockAuthority {
    flags: u64,
    timer_quota: u32,
    timer_signal: Option<sel4::cap::Notification>,
    timer_badge: u64,
}

impl TaskClockAuthority {
    pub const DENY: Self = Self {
        flags: 0,
        timer_quota: 0,
        timer_signal: None,
        timer_badge: 0,
    };

    pub const fn allows(self, authority: u64) -> bool {
        self.flags & authority == authority
    }

    pub const fn flags(self) -> u64 {
        self.flags
    }

    pub const fn timer_quota(self) -> u32 {
        self.timer_quota
    }

    pub const fn timer_badge(self) -> u64 {
        self.timer_badge
    }

    pub fn signal_timer(self) -> Result<(), ClockError> {
        let notification = self.timer_signal.ok_or(ClockError::InvalidNotification)?;
        notification.signal();
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct AuthorityEntry {
    task: TaskId,
    authority: TaskClockAuthority,
}

pub struct ClockService {
    scheduler: TimerScheduler<MAX_LIVE_TIMERS>,
    simulated_now: u64,
    authorities: [Option<AuthorityEntry>; crate::task::MAX_TASKS],
}

impl ClockService {
    pub const fn new() -> Self {
        Self {
            scheduler: TimerScheduler::new(),
            simulated_now: 0,
            authorities: [None; crate::task::MAX_TASKS],
        }
    }

    pub const fn live_timers(&self) -> usize {
        self.scheduler.len()
    }

    pub fn declare(
        &mut self,
        budget: Option<&ClockAuthority<'_>>,
        generation: &Generation<'_>,
        notifications: &crate::notification::NotificationTable,
        allocator: &mut crate::object_allocator::ObjectAllocator,
        arena: crate::object_allocator::TaskArenaId,
        task: TaskId,
        instance: usize,
    ) -> Result<TaskClockAuthority, ClockError> {
        if self
            .authorities
            .iter()
            .flatten()
            .any(|entry| entry.task == task)
        {
            return Err(ClockError::Malformed);
        }
        let slot = self
            .authorities
            .iter_mut()
            .find(|entry| entry.is_none())
            .ok_or(ClockError::TimerLimit)?;
        let instance_record = generation
            .instance(instance)
            .map_err(|_| ClockError::Malformed)?;
        let Some(entry) =
            budget.and_then(|budget| budget.authority_for(&holder_identity(instance_record.name)))
        else {
            *slot = Some(AuthorityEntry {
                task,
                authority: TaskClockAuthority::DENY,
            });
            return Ok(TaskClockAuthority::DENY);
        };
        let timer_signal = if entry.allows(RIGHT_CLOCK_TIMER_USE) {
            let notification = notifications
                .timer_target(generation, instance, entry.notification_grant_identity)
                .ok_or(ClockError::InvalidNotification)?;
            let signal_slot = allocator
                .reserve_slot_in::<sel4::cap_type::Notification>(arena)
                .map_err(|_| ClockError::TimerLimit)?;
            let root = sel4::init_thread::slot::CNODE.cap();
            root.absolute_cptr(signal_slot.cptr())
                .mint(
                    &root.absolute_cptr(notification),
                    sel4::CapRightsBuilder::none().write(true).build(),
                    entry.notification_badge,
                )
                .map_err(|_| ClockError::InvalidNotification)?;
            Some(signal_slot.cap())
        } else {
            None
        };
        let authority = TaskClockAuthority {
            flags: entry.authority_flags,
            timer_quota: entry.timer_quota,
            timer_signal,
            timer_badge: entry.notification_badge,
        };
        *slot = Some(AuthorityEntry { task, authority });
        Ok(authority)
    }

    pub fn authority(&self, task: TaskId) -> TaskClockAuthority {
        self.authorities
            .iter()
            .flatten()
            .find(|entry| entry.task == task)
            .map_or(TaskClockAuthority::DENY, |entry| entry.authority)
    }

    pub fn clear_task(&mut self, task: TaskId) {
        if let Some(entry) = self
            .authorities
            .iter_mut()
            .find(|entry| entry.as_ref().is_some_and(|entry| entry.task == task))
        {
            *entry = None;
        }
    }

    pub fn read_monotonic(
        authority: TaskClockAuthority,
        now: MonotonicInstant,
    ) -> Result<u64, ClockError> {
        if authority.allows(RIGHT_CLOCK_MONOTONIC_READ) {
            Ok(now.0)
        } else {
            Err(ClockError::Undeclared)
        }
    }

    pub fn arm(
        &mut self,
        authority: TaskClockAuthority,
        task: TaskId,
        now: MonotonicInstant,
        delay: u64,
    ) -> Result<(TimerId, DeadlineProgramming), ClockError> {
        if !authority.allows(RIGHT_CLOCK_TIMER_USE) {
            return Err(ClockError::Undeclared);
        }
        let owner = task_epoch(task);
        if self.scheduler.timers_of(owner) >= authority.timer_quota as usize {
            return Err(ClockError::TimerLimit);
        }
        let (timer, transition) = self
            .scheduler
            .schedule_after(owner, now, delay)
            .map_err(map_timer_error)?;
        Ok((timer, transition.programming))
    }

    pub fn cancel(
        &mut self,
        authority: TaskClockAuthority,
        task: TaskId,
        now: MonotonicInstant,
        timer: TimerId,
    ) -> Result<DeadlineProgramming, ClockError> {
        if !authority.allows(RIGHT_CLOCK_TIMER_USE) {
            return Err(ClockError::Undeclared);
        }
        self.scheduler
            .cancel(timer, task_epoch(task), now)
            .map(|transition| transition.programming)
            .map_err(map_timer_error)
    }

    pub fn cancel_task(
        &mut self,
        task: TaskId,
        now: MonotonicInstant,
    ) -> Result<DeadlineProgramming, ClockError> {
        self.scheduler
            .cancel_task(task_epoch(task), now)
            .map(|transition| transition.programming)
            .map_err(map_timer_error)
    }

    pub fn service_timer_source<P: PlatformTimer>(
        &mut self,
        platform: &mut P,
    ) -> TimerSourceOutcome<P::Error> {
        let authorities = &self.authorities;
        match self.scheduler.service_timer_source(platform, |owner| {
            authorities
                .iter()
                .flatten()
                .any(|entry| entry.task.0 == owner.task)
        }) {
            Ok(transition) => TimerSourceOutcome::complete(transition),
            Err(ServiceTimerError::Clock(error)) => {
                TimerSourceOutcome::failed(TimerSourceFailure::Clock(error))
            }
            Err(ServiceTimerError::Scheduler(error)) => {
                TimerSourceOutcome::failed(TimerSourceFailure::Scheduler(error))
            }
            Err(ServiceTimerError::Program { error, transition }) => {
                TimerSourceOutcome::after_mutation(transition, TimerSourceFailure::Program(error))
            }
            Err(ServiceTimerError::Acknowledge { error, transition }) => {
                TimerSourceOutcome::after_mutation(
                    transition,
                    TimerSourceFailure::Acknowledge(error),
                )
            }
        }
    }

    pub fn expire(&mut self, now: MonotonicInstant) -> Result<ExpiryBatch, ClockError> {
        let authorities = &self.authorities;
        let transition = self
            .scheduler
            .on_timer_expiry(now, |owner| {
                authorities
                    .iter()
                    .flatten()
                    .any(|entry| entry.task.0 == owner.task)
            })
            .map_err(map_timer_error)?;
        Ok(ExpiryBatch::from_timer_transition(transition))
    }

    pub fn read_simulated(&self, authority: TaskClockAuthority) -> Result<u64, ClockError> {
        if authority.allows(RIGHT_CLOCK_SIMULATED_READ) {
            Ok(self.simulated_now)
        } else {
            Err(ClockError::Undeclared)
        }
    }

    pub fn advance_simulated(
        &mut self,
        authority: TaskClockAuthority,
        delta: u64,
    ) -> Result<u64, ClockError> {
        if !authority.allows(RIGHT_CLOCK_SIMULATED_ADVANCE) {
            return Err(ClockError::Undeclared);
        }
        let previous = self.simulated_now;
        self.simulated_now = previous
            .checked_add(delta)
            .ok_or(ClockError::SimulatedTimeOverflow)?;
        Ok(previous)
    }
}

/// Due timer holders extracted from a scheduler transition for root delivery.
pub struct ExpiryBatch {
    tasks: [Option<TaskId>; MAX_LIVE_TIMERS],
    len: usize,
}
impl ExpiryBatch {
    /// Preserve already-decided wakes carried by a post-mutation platform error.
    pub fn from_timer_transition(
        transition: crate::timer::TimerTransition<MAX_LIVE_TIMERS>,
    ) -> Self {
        let mut tasks = [None; MAX_LIVE_TIMERS];
        let mut len = 0;
        for event in transition.events.iter() {
            if let SchedulingEventKind::TaskReady { task, .. } = event.kind
                && len < tasks.len()
            {
                tasks[len] = Some(TaskId(task.task));
                len += 1;
            }
        }
        Self { tasks, len }
    }

    pub fn tasks(&self) -> impl Iterator<Item = TaskId> + '_ {
        self.tasks[..self.len].iter().flatten().copied()
    }

    pub const fn due(&self) -> usize {
        self.len
    }
}

pub enum TimerSourceFailure<E> {
    Clock(E),
    Scheduler(TimerError),
    Program(E),
    Acknowledge(E),
}

pub struct TimerSourceOutcome<E> {
    pub expired: ExpiryBatch,
    pub failure: Option<TimerSourceFailure<E>>,
}

impl<E> TimerSourceOutcome<E> {
    fn complete(transition: crate::timer::TimerTransition<MAX_LIVE_TIMERS>) -> Self {
        Self {
            expired: ExpiryBatch::from_timer_transition(transition),
            failure: None,
        }
    }

    fn failed(failure: TimerSourceFailure<E>) -> Self {
        Self {
            expired: ExpiryBatch {
                tasks: [None; MAX_LIVE_TIMERS],
                len: 0,
            },
            failure: Some(failure),
        }
    }

    fn after_mutation(
        transition: crate::timer::TimerTransition<MAX_LIVE_TIMERS>,
        failure: TimerSourceFailure<E>,
    ) -> Self {
        Self {
            expired: ExpiryBatch::from_timer_transition(transition),
            failure: Some(failure),
        }
    }
}

impl Default for ClockService {
    fn default() -> Self {
        Self::new()
    }
}

pub const fn task_epoch(task: TaskId) -> TaskEpoch {
    TaskEpoch::new(task.0, 0)
}

fn map_timer_error(error: TimerError) -> ClockError {
    match error {
        TimerError::CapacityExhausted { .. } => ClockError::TimerLimit,
        TimerError::DurationOverflow { .. } | TimerError::IdentitySpaceExhausted => {
            ClockError::TimeOverflow
        }
        TimerError::TimerNotFound(_) | TimerError::TimerOwnerMismatch { .. } => {
            ClockError::TimerNotFound
        }
        TimerError::MonotonicRegression { .. } => ClockError::TimeOverflow,
    }
}

pub fn authority_object<'a>(
    generation: &Generation<'a>,
) -> Option<Result<ClockAuthority<'a>, boot_contracts::clock_authority::DecodeError>> {
    crate::generation::clock_authority_object(generation)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONO: TaskClockAuthority = TaskClockAuthority {
        flags: RIGHT_CLOCK_MONOTONIC_READ,
        timer_quota: 0,
        timer_signal: None,
        timer_badge: 0,
    };
    const TIMER: TaskClockAuthority = TaskClockAuthority {
        flags: RIGHT_CLOCK_TIMER_USE,
        timer_quota: 1,
        timer_signal: None,
        timer_badge: 1,
    };
    const SIM_READ: TaskClockAuthority = TaskClockAuthority {
        flags: RIGHT_CLOCK_SIMULATED_READ,
        timer_quota: 0,
        timer_signal: None,
        timer_badge: 0,
    };
    const SIM_ADVANCE: TaskClockAuthority = TaskClockAuthority {
        flags: RIGHT_CLOCK_SIMULATED_ADVANCE,
        timer_quota: 0,
        timer_signal: None,
        timer_badge: 0,
    };

    #[test]
    fn authorities_are_independent() {
        assert_eq!(
            ClockService::read_monotonic(MONO, MonotonicInstant(7)),
            Ok(7)
        );
        assert_eq!(
            ClockService::read_monotonic(SIM_READ, MonotonicInstant(7)),
            Err(ClockError::Undeclared)
        );
        let mut service = ClockService::new();
        assert_eq!(service.read_simulated(SIM_READ), Ok(0));
        assert_eq!(
            service.advance_simulated(SIM_READ, 1),
            Err(ClockError::Undeclared)
        );
        assert_eq!(service.advance_simulated(SIM_ADVANCE, 3), Ok(0));
        assert_eq!(service.read_simulated(SIM_READ), Ok(3));
    }

    #[test]
    fn per_task_timer_quota_does_not_consume_other_tasks_capacity() {
        let mut service = ClockService::new();
        let first = service
            .arm(TIMER, TaskId(1), MonotonicInstant(0), 5)
            .unwrap();
        assert_eq!(
            service.arm(TIMER, TaskId(1), MonotonicInstant(0), 6),
            Err(ClockError::TimerLimit)
        );
        assert!(
            service
                .arm(TIMER, TaskId(2), MonotonicInstant(0), 6)
                .is_ok()
        );
        assert_eq!(
            service.cancel(TIMER, TaskId(1), MonotonicInstant(1), first.0),
            Ok(DeadlineProgramming::Program(MonotonicInstant(6)))
        );
    }

    #[test]
    fn task_death_drops_only_that_tasks_timers() {
        let mut service = ClockService::new();
        service
            .arm(TIMER, TaskId(1), MonotonicInstant(0), 5)
            .unwrap();
        service
            .arm(TIMER, TaskId(2), MonotonicInstant(0), 6)
            .unwrap();
        assert_eq!(
            service.cancel_task(TaskId(1), MonotonicInstant(1)),
            Ok(DeadlineProgramming::Program(MonotonicInstant(6)))
        );
        assert_eq!(service.live_timers(), 1);
    }

    #[test]
    fn authority_slots_follow_live_tasks_not_lifetime_ids() {
        let mut service = ClockService::new();
        for offset in 0..crate::task::MAX_TASKS {
            service.authorities[offset] = Some(AuthorityEntry {
                task: TaskId(100 + offset as u32),
                authority: MONO,
            });
        }
        service.clear_task(TaskId(117));
        let slot = service
            .authorities
            .iter_mut()
            .find(|entry| entry.is_none())
            .expect("cleared authority slot");
        *slot = Some(AuthorityEntry {
            task: TaskId(10_000),
            authority: SIM_READ,
        });
        assert_eq!(service.authority(TaskId(10_000)), SIM_READ);
        assert_eq!(service.authority(TaskId(117)), TaskClockAuthority::DENY);
    }

    #[test]
    fn expiry_discards_a_timer_after_its_authority_is_cleared() {
        let task = TaskId(7);
        let mut service = ClockService::new();
        service.authorities[0] = Some(AuthorityEntry {
            task,
            authority: TIMER,
        });
        service.arm(TIMER, task, MonotonicInstant(0), 5).unwrap();
        service.clear_task(task);
        let expired = service.expire(MonotonicInstant(5)).unwrap();
        assert_eq!(expired.due(), 0);
        assert_eq!(expired.tasks().count(), 0);
        assert_eq!(service.live_timers(), 0);
    }
}
