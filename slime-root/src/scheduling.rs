//! C9.3 declared scheduling class, and the promotion authority over it.
//!
//! The generation-authenticated resource resolves a class by instance name; the
//! band mapping it carries turns that class into the exact seL4 TCB priority a
//! thread runs at. Runtime state is keyed by `TaskId`, which is never reused, so
//! a promoted class cannot be inherited by a later task at the same index.
//!
//! What the root owns here is mechanism only. It applies a declared band to a
//! TCB, it answers what class a task currently runs at, and it refuses a
//! promotion the generation did not declare. It chooses no class, so a
//! composition that wants a different band layout edits its manifest rather than
//! this file.
//!
//! Three properties are worth naming because they are the milestone's checks:
//!
//! * **Deny-by-default is an answer, not a refusal.** An instance no policy
//!   names runs at the root's own child priority — exactly what the builder left
//!   in its `ScheduleRecord` — and reads its class back as `undeclared`. Every
//!   thread runs at *some* priority, so "the generation said nothing about me"
//!   has a correct answer; what it does not have is a *band*, which is why it is
//!   not reported as `normal`.
//! * **No component widens itself.** The decoder refuses a declared self-edge,
//!   and [`SchedulingService::promote`] separately refuses a *caller* equal to
//!   the subject. Both are needed: the first stops the generation from
//!   expressing it, the second stops a caller from reaching a legitimate edge
//!   from the wrong side.
//! * **CPU quantity is not bounded here.** `KernelIsMCS OFF` leaves the kernel
//!   no budget to charge and B77 made both readers refuse a nonzero
//!   `budget_us`/`period_us`, so a class orders CPU access rather than
//!   reserving an amount of it.

use boot_contracts::generation::Generation;
use boot_contracts::scheduling_class::{
    SchedulingClass, UNDECLARED_CLASS_ID, class_name, instance_identity, is_declared_class,
};

use crate::task::{CHILD_PRIORITY, TaskId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulingError {
    /// The caller holds no declared promotion edge over this subject.
    Undeclared,
    /// The generation names no such instance, or the resource is inconsistent
    /// with it.
    Malformed,
    /// The caller named itself as the subject. Distinct from `Undeclared`
    /// because it is refused even when an edge exists: a component may not widen
    /// its own class through an authority it legitimately holds.
    SelfPromotion,
    /// The requested class is above the ceiling this edge declares.
    AboveCeiling,
    /// The requested class has no declared band, so it names no priority.
    UnknownClass,
    /// Applying the priority to the subject's TCB failed.
    SchedParams,
}

/// One task's resolved class, as installed at launch and possibly promoted since.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskClass {
    identity: [u8; 32],
    class_id: u32,
    worker_class_id: u32,
    priority: sel4::Word,
    worker_priority: sel4::Word,
}

impl TaskClass {
    /// What a task in no declared band runs at: the root's own child priority,
    /// one below the root, so the service loop always preempts a runnable child
    /// (B48).
    ///
    /// The class id is `undeclared` rather than `normal`, and that is the
    /// correction a review forced. `normal` names a band whose priority the
    /// generation declares; this thread is not in it. Reporting `normal` here
    /// would make `CLASS_READ` name a band at a priority that band does not
    /// have, and would make promoting such a subject *to* `normal` look like a
    /// no-op class change while silently moving its priority from 254 to
    /// whatever `normal` maps to.
    pub const DEFAULT: Self = Self {
        identity: [0; 32],
        class_id: UNDECLARED_CLASS_ID,
        worker_class_id: UNDECLARED_CLASS_ID,
        priority: CHILD_PRIORITY,
        worker_priority: CHILD_PRIORITY,
    };

    pub const fn class_id(self) -> u32 {
        self.class_id
    }
    pub const fn worker_class_id(self) -> u32 {
        self.worker_class_id
    }
    pub const fn priority(self) -> sel4::Word {
        self.priority
    }
    pub const fn worker_priority(self) -> sel4::Word {
        self.worker_priority
    }
    pub const fn name(self) -> &'static str {
        class_name(self.class_id)
    }
}

#[derive(Clone, Copy)]
struct ClassEntry {
    task: TaskId,
    class: TaskClass,
}

pub struct SchedulingService {
    entries: [Option<ClassEntry>; crate::task::MAX_TASKS],
}

impl SchedulingService {
    pub const fn new() -> Self {
        Self {
            entries: [None; crate::task::MAX_TASKS],
        }
    }

    /// Resolve and record the class one launched instance runs at.
    ///
    /// Called once per live task, including tasks the policy does not name, so
    /// the table answers about a *live task* rather than about whether the
    /// generation happened to mention it — the same rule C9.1's authority table
    /// and C9.2's source table follow.
    pub fn declare(
        &mut self,
        policy: Option<&SchedulingClass<'_>>,
        generation: &Generation<'_>,
        task: TaskId,
        instance: usize,
    ) -> Result<TaskClass, SchedulingError> {
        if self
            .entries
            .iter()
            .flatten()
            .any(|entry| entry.task == task)
        {
            return Err(SchedulingError::Malformed);
        }
        let record = generation
            .instance(instance)
            .map_err(|_| SchedulingError::Malformed)?;
        let identity = instance_identity(record.name);
        // An instance the policy does not name keeps the root's own child
        // default, because that is exactly what its `ScheduleRecord` carries:
        // the builder substitutes a band's priority only for instances the
        // policy *names*. Synthesizing a `normal` assignment here would make the
        // root report a priority the thread is not running at, and would let a
        // later promotion compute a ceiling comparison from that wrong number
        // (found by review). So "declared" and "defaulted" stay distinct on both
        // sides of the boundary.
        let class = match policy.and_then(|policy| {
            policy
                .class_for(&identity)
                .map(|assignment| (policy, assignment))
        }) {
            Some((policy, assignment)) => TaskClass {
                identity,
                class_id: assignment.class_id,
                worker_class_id: assignment.worker_class_id,
                // An unbanded class still resolves to the child default rather
                // than to zero. A policy may declare only the bands it uses, and
                // an assignment naming an unbanded class does not decode, so
                // this arm is reachable only for a class the resource banded —
                // the fallback is the belt, not the mechanism.
                priority: band_priority(policy, assignment.class_id),
                worker_priority: band_priority(policy, assignment.worker_class_id),
            },
            None => TaskClass {
                identity,
                ..TaskClass::DEFAULT
            },
        };
        let slot = self
            .entries
            .iter_mut()
            .find(|entry| entry.is_none())
            .ok_or(SchedulingError::Malformed)?;
        *slot = Some(ClassEntry { task, class });
        Ok(class)
    }

    /// The class a live task currently runs at, or the default for one this
    /// service never recorded.
    pub fn class(&self, task: TaskId) -> TaskClass {
        self.entries
            .iter()
            .flatten()
            .find(|entry| entry.task == task)
            .map_or(TaskClass::DEFAULT, |entry| entry.class)
    }

    /// Drop a terminated task's row so a later task cannot inherit its class.
    pub fn release(&mut self, task: TaskId) {
        for entry in self.entries.iter_mut() {
            if entry.is_some_and(|recorded| recorded.task == task) {
                *entry = None;
            }
        }
    }

    /// Promote `subject` to `class_id` on behalf of `caller`.
    ///
    /// The caller never names a priority: it names a *class*, and the priority
    /// comes from the generation's own band mapping. That is what keeps
    /// promotion inside the declared vocabulary — a holder cannot reach a
    /// priority between two bands, and therefore cannot reach a priority no
    /// class maps to.
    ///
    /// `subject_tcb` is resolved by the caller of this method from the subject's
    /// live task, so this function applies a priority to a TCB the root already
    /// owns rather than to one named on the wire.
    pub fn promote(
        &mut self,
        policy: Option<&SchedulingClass<'_>>,
        caller: TaskId,
        subject: TaskId,
        subject_tcb: sel4::cap::Tcb,
        class_id: u32,
    ) -> Result<TaskClass, SchedulingError> {
        // Refused before the edge lookup, and separately from the decoder's
        // structural refusal of a declared self-edge. A caller reaching its own
        // class through an authority it holds over *another* component is the
        // exact widening C9.3 forbids, and it is a runtime shape the resource
        // cannot express.
        if caller == subject {
            return Err(SchedulingError::SelfPromotion);
        }
        if !is_declared_class(class_id) {
            return Err(SchedulingError::UnknownClass);
        }
        let policy = policy.ok_or(SchedulingError::Undeclared)?;
        let caller_identity = self.recorded_identity(caller)?;
        let subject_identity = self.recorded_identity(subject)?;
        let ceiling = policy
            .promotion_ceiling(&caller_identity, &subject_identity)
            .ok_or(SchedulingError::Undeclared)?;
        let priority = policy
            .band_for(class_id)
            .ok_or(SchedulingError::UnknownClass)?;
        if priority > ceiling {
            return Err(SchedulingError::AboveCeiling);
        }
        // The subject's row is located *before* the kernel call, so a successful
        // `tcb_set_priority` cannot be followed by a failed lookup — that
        // ordering would leave the live TCB at the new band while the recorded
        // class still named the old one, and would report failure to the caller
        // (found by review). After this point nothing between here and the
        // mutation can fail.
        let row = self
            .entries
            .iter()
            .position(|entry| entry.is_some_and(|recorded| recorded.task == subject))
            .ok_or(SchedulingError::Malformed)?;
        let applied = sel4::Word::from(priority);
        // The root's own TCB is the scheduling authority, exactly as it is when
        // a task is first constructed. The ceiling above is Slime's; this is
        // seL4's, and both must hold.
        subject_tcb
            .tcb_set_priority(sel4::init_thread::slot::TCB.cap(), applied)
            .map_err(|_| SchedulingError::SchedParams)?;
        let entry = self.entries[row]
            .as_mut()
            .ok_or(SchedulingError::Malformed)?;
        entry.class.class_id = class_id;
        entry.class.priority = applied;
        Ok(entry.class)
    }

    fn recorded_identity(&self, task: TaskId) -> Result<[u8; 32], SchedulingError> {
        self.entries
            .iter()
            .flatten()
            .find(|entry| entry.task == task)
            .map(|entry| entry.class.identity)
            .ok_or(SchedulingError::Malformed)
    }
}

impl Default for SchedulingService {
    fn default() -> Self {
        Self::new()
    }
}

/// The priority a class runs at under one policy, falling back to the root's
/// child default when the policy declares no band for it.
fn band_priority(policy: &SchedulingClass<'_>, class_id: u32) -> sel4::Word {
    policy
        .band_for(class_id)
        .map_or(CHILD_PRIORITY, sel4::Word::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use boot_contracts::scheduling_class::{
        BAND_BYTES, CLASS_BEST_EFFORT, CLASS_FOREGROUND, CLASS_NORMAL, ENTRY_BYTES, FORMAT_VERSION,
        HEADER_BYTES, MAGIC, PROMOTION_BYTES,
    };

    fn band(class_id: u32, priority: u32) -> [u8; BAND_BYTES] {
        let mut bytes = [0u8; BAND_BYTES];
        bytes[..4].copy_from_slice(&class_id.to_le_bytes());
        bytes[4..8].copy_from_slice(&priority.to_le_bytes());
        bytes
    }

    fn assignment(name: &str, class_id: u32, worker: u32) -> [u8; ENTRY_BYTES] {
        let mut bytes = [0u8; ENTRY_BYTES];
        bytes[..32].copy_from_slice(&instance_identity(name));
        bytes[32..36].copy_from_slice(&class_id.to_le_bytes());
        bytes[36..40].copy_from_slice(&worker.to_le_bytes());
        bytes
    }

    fn promotion(holder: &str, subject: &str, ceiling: u32) -> [u8; PROMOTION_BYTES] {
        let mut bytes = [0u8; PROMOTION_BYTES];
        bytes[..32].copy_from_slice(&instance_identity(holder));
        bytes[32..64].copy_from_slice(&instance_identity(subject));
        bytes[64..68].copy_from_slice(&ceiling.to_le_bytes());
        bytes
    }

    /// The plane fixture's shape: three bands, two assignments, one edge.
    fn policy_bytes() -> [u8; HEADER_BYTES + 3 * BAND_BYTES + 2 * ENTRY_BYTES + PROMOTION_BYTES] {
        const TOTAL: usize = HEADER_BYTES + 3 * BAND_BYTES + 2 * ENTRY_BYTES + PROMOTION_BYTES;
        let mut bytes = [0u8; TOTAL];
        bytes[..8].copy_from_slice(&MAGIC);
        bytes[8..12].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes[12..16].copy_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        bytes[24..26].copy_from_slice(&3u16.to_le_bytes());
        bytes[26..28].copy_from_slice(&2u16.to_le_bytes());
        bytes[28..30].copy_from_slice(&1u16.to_le_bytes());
        bytes[30..32].copy_from_slice(&(TOTAL as u16).to_le_bytes());
        let mut offset = HEADER_BYTES;
        for record in [
            band(CLASS_FOREGROUND, 200),
            band(CLASS_NORMAL, 150),
            band(CLASS_BEST_EFFORT, 100),
        ] {
            bytes[offset..offset + BAND_BYTES].copy_from_slice(&record);
            offset += BAND_BYTES;
        }
        let mut entries = [
            assignment("sched-foreground", CLASS_FOREGROUND, CLASS_FOREGROUND),
            assignment("sched-burner", CLASS_BEST_EFFORT, CLASS_BEST_EFFORT),
        ];
        entries.sort_by_key(|entry| {
            let identity: [u8; 32] = entry[..32].try_into().expect("identity prefix");
            identity
        });
        for record in entries {
            bytes[offset..offset + ENTRY_BYTES].copy_from_slice(&record);
            offset += ENTRY_BYTES;
        }
        bytes[offset..offset + PROMOTION_BYTES].copy_from_slice(&promotion(
            "sched-foreground",
            "sched-burner",
            150,
        ));
        bytes
    }

    /// A declared policy turns a class into the band's priority, for both the
    /// main thread and workers.
    #[test]
    fn a_declared_class_resolves_to_its_bands_priority() {
        let bytes = policy_bytes();
        let policy = SchedulingClass::decode(&bytes).expect("valid policy");
        assert_eq!(policy.band_for(CLASS_FOREGROUND), Some(200));
        assert_eq!(band_priority(&policy, CLASS_BEST_EFFORT), 100);
    }

    /// A class the policy declares no band for falls back to the root's child
    /// default rather than to zero, so an omitted band cannot silently drop a
    /// thread to the lowest runnable priority.
    #[test]
    fn an_unbanded_class_falls_back_to_the_child_default() {
        const TOTAL: usize = HEADER_BYTES + BAND_BYTES;
        let mut bytes = [0u8; TOTAL];
        bytes[..8].copy_from_slice(&MAGIC);
        bytes[8..12].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes[12..16].copy_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        bytes[24..26].copy_from_slice(&1u16.to_le_bytes());
        bytes[30..32].copy_from_slice(&(TOTAL as u16).to_le_bytes());
        bytes[HEADER_BYTES..].copy_from_slice(&band(CLASS_FOREGROUND, 200));
        let policy = SchedulingClass::decode(&bytes).expect("valid policy");
        assert_eq!(band_priority(&policy, CLASS_NORMAL), CHILD_PRIORITY);
    }

    /// Deny-by-default: an unnamed instance is `normal`, and with no policy at
    /// all every task keeps the root's child default.
    #[test]
    fn an_undeclared_task_runs_at_the_default_class() {
        let service = SchedulingService::new();
        let class = service.class(TaskId(7));
        assert_eq!(class.class_id(), UNDECLARED_CLASS_ID);
        assert_eq!(class.priority(), CHILD_PRIORITY);
    }

    /// The invariant review found broken: an instance a *present* policy does
    /// not name resolves to the root's child default, not to the `normal` band.
    ///
    /// This is what keeps the root's answer equal to the priority the builder
    /// actually left in that instance's `ScheduleRecord`. Asserted against a
    /// policy that *does* declare a `normal` band, because that is the only
    /// shape in which the two answers differ: with 150 declared, a synthesized
    /// default would report 150 for a thread running at 254.
    #[test]
    fn an_unnamed_instance_keeps_the_child_default_under_a_declared_policy() {
        let bytes = policy_bytes();
        let policy = SchedulingClass::decode(&bytes).expect("valid policy");
        assert_eq!(policy.band_for(CLASS_NORMAL), Some(150));
        assert_ne!(u64::from(150u32), CHILD_PRIORITY);
        assert_eq!(
            policy.class_for(&instance_identity("sched-unnamed")),
            None,
            "the fixture shape must leave this instance unnamed"
        );
        // `declare` needs a `Generation`, which no host test can build, so the
        // property is asserted over the two functions it composes: the policy
        // reports no assignment, and `TaskClass::DEFAULT` is the child default.
        assert_eq!(TaskClass::DEFAULT.priority(), CHILD_PRIORITY);
        assert_eq!(TaskClass::DEFAULT.worker_priority(), CHILD_PRIORITY);
    }

    /// Promotion refuses a caller naming itself before it looks for an edge, so
    /// holding authority over another component never becomes authority over
    /// yourself.
    #[test]
    fn a_caller_may_not_promote_itself() {
        let bytes = policy_bytes();
        let policy = SchedulingClass::decode(&bytes).expect("valid policy");
        let mut service = SchedulingService::new();
        assert_eq!(
            service.promote(
                Some(&policy),
                TaskId(3),
                TaskId(3),
                sel4::init_thread::slot::TCB.cap(),
                CLASS_FOREGROUND,
            ),
            Err(SchedulingError::SelfPromotion)
        );
    }

    /// An undeclared class id is refused before any policy lookup, so a wire
    /// value outside the vocabulary cannot reach the band table.
    #[test]
    fn an_unknown_class_is_refused_without_a_policy() {
        let mut service = SchedulingService::new();
        assert_eq!(
            service.promote(
                None,
                TaskId(1),
                TaskId(2),
                sel4::init_thread::slot::TCB.cap(),
                42,
            ),
            Err(SchedulingError::UnknownClass)
        );
    }

    /// With no declared policy there is no edge to find, so a well-formed
    /// request is still refused.
    #[test]
    fn promotion_without_a_declared_policy_is_undeclared() {
        let mut service = SchedulingService::new();
        assert_eq!(
            service.promote(
                None,
                TaskId(1),
                TaskId(2),
                sel4::init_thread::slot::TCB.cap(),
                CLASS_FOREGROUND,
            ),
            Err(SchedulingError::Undeclared)
        );
    }

    /// A released row is gone, so a later task at the same table index inherits
    /// nothing.
    #[test]
    fn releasing_a_task_drops_its_recorded_class() {
        let mut service = SchedulingService::new();
        service.entries[0] = Some(ClassEntry {
            task: TaskId(4),
            class: TaskClass {
                class_id: CLASS_FOREGROUND,
                priority: 200,
                ..TaskClass::DEFAULT
            },
        });
        assert_eq!(service.class(TaskId(4)).priority(), 200);
        service.release(TaskId(4));
        assert_eq!(service.class(TaskId(4)).priority(), CHILD_PRIORITY);
    }
}
