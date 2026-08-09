//! Child task construction, authority, and lifecycle accounting.
//!
//! `slime-root` owns every seL4 object a child task is built from. A child's
//! CSpace holds only capabilities its generation grants imply:
//!
//! | slot | capability |
//! | ---- | ---------- |
//! | 0 | null |
//! | 1 | root service endpoint, badged, rights derived from declared grants |
//! | 2 | the task's own TCB, only when supervision requires it |
//! | 3 | root service endpoint, badged as this task's fault handler |
//!
//! Slot 3 exists because the non-MCS kernel resolves a thread's fault handler
//! CPtr *in that thread's own CSpace* (`sendFaultIPC` in
//! `src/kernel/faulthandler.c`). It is a second badge on the same endpoint
//! object as slot 1, not a distinct authority: `slime-root` therefore blocks on
//! exactly one endpoint and tells requests from faults by badge. No untyped,
//! CNode, VSpace, ASID pool, or IRQ authority is ever placed in a child CSpace.
//!
//! Construction is staged: every object is allocated and every capability
//! installed before any thread is activated, so a failure part-way through
//! leaves a task that has never run, plus a cleanup record naming exactly the
//! root CSlots to revoke and delete.

use sel4::{CapTypeForObjectOfFixedSize, CapTypeForObjectOfVariableSize};

use crate::child_vspace::{ChildImage, ChildVSpace, ScratchPage, VSpaceError, create_child_vspace};
use crate::generation::Authority;
use crate::object_allocator::{AllocError, ObjectAllocator, TaskArenaId};

/// Child tasks one generation may run.
///
/// Thirty-two until P5.4.9. The C8.10 full-graph boot runs every C8 role at
/// once, and on this root that is **thirty-seven** live tasks rather than the
/// oracle's twenty: the root launches all twenty components the generation
/// declares (P5.2), and `init` then spawns seventeen of them again as the
/// composition's own children — the fabric plus sixteen participants — because
/// a spawned child is the only one holding a control endpoint init minted.
///
/// Forty-eight, with headroom rather than exactly 37, on `channel::MAX_CHANNELS`
/// and B28's shared rule: a bound raised to the first passing number moves again
/// at the next graph, and exhaustion here is not a clean refusal — `serve_spawn`
/// answers `DestinationSlotsExhausted` and the parent reports a spawn failure,
/// which reads as a composition defect rather than an exhausted table.
///
/// The cost is the per-task graph table this bounds alongside it:
/// `GraphTables` holds `MAX_GRAPH_TASKS` (== this) tables of `MAX_TASK_CAPS`
/// capabilities, about 1.5 KiB each, so the sixteen added entries cost ~24 KiB
/// of `.bss`. Both tables live in `static`s for backlog B3's reason.
pub const MAX_TASKS: usize = 48;

/// Slots in a child CNode: null, service endpoint, own TCB, fault handler.
pub const CHILD_CNODE_SIZE_BITS: usize = 2;

/// Child CSpace slot holding the badged root service endpoint.
pub const CHILD_SLOT_SERVICE: sel4::CPtrBits = 1;
/// Child CSpace slot holding the task's own TCB, when supervised.
pub const CHILD_SLOT_TCB: sel4::CPtrBits = 2;
/// Child CSpace slot holding the task's badged fault-handler endpoint.
pub const CHILD_SLOT_FAULT: sel4::CPtrBits = 3;

/// Child scheduling priority. Strictly below the root task's own priority so
/// the root service loop always preempts a child that becomes runnable.
pub const CHILD_PRIORITY: sel4::Word = 254;

/// Generation-local task identity. Not derived from any capability pointer,
/// badge, object address, or physical resource.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TaskId(pub u32);

impl TaskId {
    /// Routing token for this task's service requests. Even, and never zero, so
    /// an unbadged arrival is always distinguishable from a task's.
    pub const fn service_badge(self) -> sel4::Badge {
        ((self.0 as sel4::Badge) + 1) << 1
    }

    /// Routing token for this task's fault messages: its service badge with the
    /// low bit set.
    pub const fn fault_badge(self) -> sel4::Badge {
        self.service_badge() | 1
    }

    /// Recover a task and the kind of arrival from a received badge.
    pub const fn from_badge(badge: sel4::Badge) -> Option<(Self, Arrival)> {
        let index = (badge >> 1) as u32;
        if index == 0 {
            return None;
        }
        let arrival = if badge & 1 == 0 {
            Arrival::Request
        } else {
            Arrival::Fault
        };
        Some((Self(index - 1), arrival))
    }
}

/// What a badge on the root endpoint denotes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Arrival {
    Request,
    Fault,
}

/// Whether a task holds a capability to its own TCB.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Supervision {
    /// The task may suspend itself; slot 2 holds its own TCB.
    SelfManaged,
    /// The task holds no TCB capability; only `slime-root` may stop it.
    RootOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskError {
    Alloc(AllocError),
    VSpace(VSpaceError),
    /// [`MAX_TASKS`] child tasks already exist.
    TableFull {
        limit: usize,
    },
    /// Installing a capability into the child CSpace failed.
    Mint {
        slot: sel4::CPtrBits,
        error: sel4::Error,
    },
    /// `seL4_TCB_Configure` failed.
    Configure(sel4::Error),
    /// `seL4_TCB_SetSchedParams` failed.
    SchedParams(sel4::Error),
    /// Writing the initial register state failed.
    WriteRegisters(sel4::Error),
    /// Resuming the thread failed.
    Resume(sel4::Error),
    /// The task's entry point does not fit a machine word.
    EntryOutOfRange {
        entry: u64,
    },
    /// B38 fixture-only failure after arena-backed objects exist.
    ForcedConstructionFailure,
    UnknownTask(TaskId),
    /// Revoking or deleting the task arena failed. No slot is reused on error.
    Cleanup(AllocError),
}

impl From<AllocError> for TaskError {
    fn from(error: AllocError) -> Self {
        Self::Alloc(error)
    }
}

impl From<VSpaceError> for TaskError {
    fn from(error: VSpaceError) -> Self {
        Self::VSpace(error)
    }
}

/// Fallible transitions after task allocation ownership begins. Kept explicit
/// so fault-injection harnesses can stop at every boundary and assert that the
/// same cleanup owner covers the allocated prefix.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConstructionStage {
    VSpace,
    CNode,
    Tcb,
    ServiceMint,
    FaultMint,
    SelfTcbMint,
    Configure,
    SchedParams,
    Entry,
    WriteRegisters,
}

impl ConstructionStage {
    pub const ALL: [Self; 10] = [
        Self::VSpace,
        Self::CNode,
        Self::Tcb,
        Self::ServiceMint,
        Self::FaultMint,
        Self::SelfTcbMint,
        Self::Configure,
        Self::SchedParams,
        Self::Entry,
        Self::WriteRegisters,
    ];
    pub const fn index(self) -> usize {
        match self {
            Self::VSpace => 0,
            Self::CNode => 1,
            Self::Tcb => 2,
            Self::ServiceMint => 3,
            Self::FaultMint => 4,
            Self::SelfTcbMint => 5,
            Self::Configure => 6,
            Self::SchedParams => 7,
            Self::Entry => 8,
            Self::WriteRegisters => 9,
        }
    }
}

/// Sole lifetime anchor for every root capability and kernel object of a task.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CleanupRecord {
    pub task: TaskId,
    pub arena: TaskArenaId,
    slots: usize,
}

impl CleanupRecord {
    pub const fn slot_count(&self) -> usize {
        self.slots
    }

    pub fn revoke(&self, allocator: &mut ObjectAllocator) -> Result<usize, TaskError> {
        allocator
            .release_task_arena(self.arena)
            .map_err(TaskError::Cleanup)
    }
}

/// One constructed child task. Every capability is held in root CSpace.
#[derive(Clone, Copy, Debug)]
pub struct Task {
    pub id: TaskId,
    pub cnode: sel4::cap::CNode,
    pub tcb: sel4::cap::Tcb,
    pub vspace: ChildVSpace,
    pub authority: Authority,
    pub supervision: Supervision,
    pub entry: u64,
    pub activated: bool,
    pub cleanup: CleanupRecord,
    /// The task that spawned this one, if any. `None` for a component the root
    /// launched from the generation.
    ///
    /// Recorded so a spawner's live-child count can be derived rather than
    /// tracked: a counter would need decrementing on both death paths and on
    /// every unwind, and a missed decrement would silently tighten a bound the
    /// generation declared. Counting the table is O(MAX_TASKS) on a path that
    /// already allocates a VSpace.
    pub spawner: Option<TaskId>,
    /// Generation executable catalogue index used to build this task.
    pub executable: Option<usize>,
    /// Root instance index, absent for dynamically spawned tasks and fixtures.
    pub instance: Option<usize>,
}

impl Task {
    /// Slots the child CSpace actually holds a capability in.
    pub fn granted_slots(&self) -> usize {
        match self.supervision {
            Supervision::SelfManaged => 3,
            Supervision::RootOnly => 2,
        }
    }

    /// Stop the thread. Idempotent from the root task's perspective.
    pub fn suspend(&self) -> Result<(), sel4::Error> {
        self.tcb.tcb_suspend()
    }
}

/// Fixed-capacity child task table. `slime-root` is the sole owner of every
/// object recorded here.
pub struct TaskTable<const CAPACITY: usize = MAX_TASKS> {
    tasks: [Option<Task>; CAPACITY],
    len: usize,
    next_id: u32,
    activated: usize,
    reclaimed_slots: usize,
}

impl<const CAPACITY: usize> TaskTable<CAPACITY> {
    pub const fn new() -> Self {
        Self {
            tasks: [const { None }; CAPACITY],
            len: 0,
            next_id: 0,
            activated: 0,
            reclaimed_slots: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn capacity(&self) -> usize {
        CAPACITY
    }

    pub const fn activated(&self) -> usize {
        self.activated
    }

    pub const fn reclaimed_slots(&self) -> usize {
        self.reclaimed_slots
    }

    /// How many live tasks `spawner` created and has not yet lost.
    ///
    /// Derived from the table rather than from a counter, so a task reclaimed
    /// by any path — clean exit, fault, or a spawn unwind — frees its parent's
    /// budget without a decrement anyone has to remember to write.
    pub fn live_children(&self, spawner: TaskId) -> usize {
        self.tasks
            .iter()
            .flatten()
            .filter(|task| task.spawner == Some(spawner))
            .count()
    }

    pub fn get(&self, id: TaskId) -> Option<&Task> {
        self.tasks.iter().flatten().find(|task| task.id == id)
    }

    pub fn tasks(&self) -> impl Iterator<Item = &Task> {
        self.tasks.iter().flatten()
    }

    /// Build a child task from a validated image.
    ///
    /// Allocation order is VSpace and image frames, then CNode, then TCB, then
    /// capability installation, then scheduling parameters and initial
    /// registers. The thread is left suspended; [`Self::activate`] starts it.
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        &mut self,
        allocator: &mut ObjectAllocator,
        image: &ChildImage<'_>,
        service_endpoint: sel4::cap::Endpoint,
        authority: Authority,
        supervision: Supervision,
        caller_vspace: sel4::cap::VSpace,
        scratch: &ScratchPage,
        asid_pool: sel4::cap::AsidPool,
        spawner: Option<TaskId>,
        executable: Option<usize>,
        instance: Option<usize>,
    ) -> Result<TaskId, TaskError> {
        let Some(index) = self.tasks.iter().position(Option::is_none) else {
            return Err(TaskError::TableFull { limit: CAPACITY });
        };
        let id = TaskId(self.next_id);
        let mut plan = image.vspace_arena_plan().map_err(VSpaceError::Image)?;
        plan.add(sel4::cap_type::CNode::object_blueprint(
            CHILD_CNODE_SIZE_BITS,
        ))
        .ok_or(TaskError::Alloc(AllocError::UntypedExhausted {
            size_bits: usize::BITS as usize,
            remaining: 0,
        }))?;
        plan.add(sel4::cap_type::Tcb::object_blueprint())
            .ok_or(TaskError::Alloc(AllocError::UntypedExhausted {
                size_bits: usize::BITS as usize,
                remaining: 0,
            }))?;
        let arena_bits =
            plan.required_size_bits()
                .ok_or(TaskError::Alloc(AllocError::UntypedExhausted {
                    size_bits: usize::BITS as usize,
                    remaining: 0,
                }))?;
        let arena = allocator.begin_task_arena(arena_bits)?;

        let construction = (|| {
            let vspace =
                create_child_vspace(allocator, arena, image, caller_vspace, scratch, asid_pool)?;
            let cnode = allocator
                .allocate_variable_in::<sel4::cap_type::CNode>(arena, CHILD_CNODE_SIZE_BITS)?
                .cap();
            let tcb = allocator
                .allocate_fixed_in::<sel4::cap_type::Tcb>(arena)?
                .cap();
            #[cfg(slime_b38_force_unwind)]
            if spawner.is_some() && crate::object_allocator::take_forced_unwind() {
                return Err(TaskError::ForcedConstructionFailure);
            }

            let root_cnode = sel4::init_thread::slot::CNODE.cap();
            // Slot 1 is invocation-only transport. In particular it must never
            // carry receive authority: all children share the root endpoint,
            // so a receiver could dequeue and answer another child's request
            // before the root dispatcher saw it.
            mint_child_slot(
                cnode,
                CHILD_SLOT_SERVICE,
                &root_cnode.absolute_cptr(service_endpoint),
                child_service_rights(authority),
                id.service_badge(),
            )?;
            // Slot 3: the same endpoint object under this task's fault badge.
            // The kernel requires a fault handler endpoint to carry send plus
            // grant or grant-reply authority, and resolves this CPtr in the
            // child's CSpace.
            mint_child_slot(
                cnode,
                CHILD_SLOT_FAULT,
                &root_cnode.absolute_cptr(service_endpoint),
                sel4::CapRightsBuilder::none()
                    .write(true)
                    .grant_reply(true)
                    .build(),
                id.fault_badge(),
            )?;
            if supervision == Supervision::SelfManaged {
                mint_child_slot(
                    cnode,
                    CHILD_SLOT_TCB,
                    &root_cnode.absolute_cptr(tcb),
                    sel4::CapRights::all(),
                    0,
                )?;
            }

            tcb.tcb_configure(
                sel4::CPtr::from_bits(CHILD_SLOT_FAULT),
                cnode,
                sel4::CNodeCapData::new(0, sel4::WORD_SIZE - CHILD_CNODE_SIZE_BITS),
                vspace.vspace,
                vspace.ipc_buffer_addr as sel4::Word,
                vspace.ipc_buffer,
            )
            .map_err(TaskError::Configure)?;
            tcb.tcb_set_sched_params(
                sel4::init_thread::slot::TCB.cap(),
                CHILD_PRIORITY,
                CHILD_PRIORITY,
            )
            .map_err(TaskError::SchedParams)?;

            let entry = image.entry();
            let mut context = sel4::UserContext::default();
            *context.pc_mut() =
                sel4::Word::try_from(entry).map_err(|_| TaskError::EntryOutOfRange { entry })?;
            // `resume = false`: nothing runs until every allocation for every
            // task in this generation has succeeded. The child runtime
            // establishes its own stack pointer at `_start`.
            tcb.tcb_write_all_registers(false, &mut context)
                .map_err(TaskError::WriteRegisters)?;
            Ok((vspace, cnode, tcb, entry))
        })();

        let (vspace, cnode, tcb, entry) = match construction {
            Ok(task) => task,
            Err(error) => {
                let cleanup =
                    construction_record(id, arena, allocator.arena_slot_count(arena).unwrap_or(0));
                cleanup.revoke(allocator)?;
                return Err(error);
            }
        };

        let cleanup = construction_record(id, arena, allocator.arena_slot_count(arena)?);
        self.tasks[index] = Some(Task {
            id,
            cnode,
            tcb,
            vspace,
            authority,
            supervision,
            entry,
            activated: false,
            cleanup,
            spawner,
            executable,
            instance,
        });
        self.len += 1;
        self.next_id += 1;
        Ok(id)
    }

    /// Start one constructed task.
    pub fn activate(&mut self, id: TaskId) -> Result<(), TaskError> {
        let task = self
            .tasks
            .iter_mut()
            .flatten()
            .find(|task| task.id == id)
            .ok_or(TaskError::UnknownTask(id))?;
        if task.activated {
            return Ok(());
        }
        task.tcb.tcb_resume().map_err(TaskError::Resume)?;
        task.activated = true;
        self.activated += 1;
        Ok(())
    }

    /// Start every constructed task that has not run yet.
    pub fn activate_all(&mut self) -> Result<usize, TaskError> {
        let mut started = 0;
        for index in 0..CAPACITY {
            let Some(task) = self.tasks[index].as_ref() else {
                continue;
            };
            if task.activated {
                continue;
            }
            let id = task.id;
            self.activate(id)?;
            started += 1;
        }
        Ok(started)
    }

    /// Suspend a task, revoke everything derived from its objects, and drop it.
    pub fn reclaim(
        &mut self,
        allocator: &mut ObjectAllocator,
        id: TaskId,
    ) -> Result<CleanupRecord, TaskError> {
        let index = self
            .tasks
            .iter()
            .position(|task| task.as_ref().is_some_and(|task| task.id == id))
            .ok_or(TaskError::UnknownTask(id))?;
        let task = self.tasks[index]
            .as_ref()
            .copied()
            .ok_or(TaskError::UnknownTask(id))?;
        // Failed arena cleanup stays owned by the table and is retryable.
        let _ = task.suspend();
        let reclaimed = task.cleanup.revoke(allocator)?;
        self.tasks[index] = None;
        self.len -= 1;
        if task.activated {
            self.activated -= 1;
        }
        self.reclaimed_slots += reclaimed;
        Ok(task.cleanup)
    }
}

impl<const CAPACITY: usize> Default for TaskTable<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

fn child_service_rights(_authority: Authority) -> sel4::CapRights {
    sel4::CapRightsBuilder::none()
        .write(true)
        .grant_reply(true)
        .build()
}

fn construction_record(task: TaskId, arena: TaskArenaId, slots: usize) -> CleanupRecord {
    CleanupRecord { task, arena, slots }
}

fn mint_child_slot(
    cnode: sel4::cap::CNode,
    slot: sel4::CPtrBits,
    source: &sel4::AbsoluteCPtr,
    rights: sel4::CapRights,
    badge: sel4::Badge,
) -> Result<(), TaskError> {
    cnode
        .absolute_cptr_from_bits_with_depth(slot, CHILD_CNODE_SIZE_BITS)
        .mint(source, rights, badge)
        .map_err(|error| TaskError::Mint { slot, error })
}

#[cfg(test)]
mod tests {
    use super::{
        Arrival, CHILD_CNODE_SIZE_BITS, CHILD_SLOT_FAULT, ConstructionStage, TaskId,
        child_service_rights, construction_record,
    };
    use crate::generation::Authority;
    use crate::object_allocator::TaskArenaId;

    #[test]
    fn badges_are_nonzero_and_round_trip() {
        for index in [0u32, 1, 7, 31] {
            let id = TaskId(index);
            assert_ne!(id.service_badge(), 0);
            assert_ne!(id.fault_badge(), 0);
            assert_eq!(
                TaskId::from_badge(id.service_badge()),
                Some((id, Arrival::Request))
            );
            assert_eq!(
                TaskId::from_badge(id.fault_badge()),
                Some((id, Arrival::Fault))
            );
        }
    }

    #[test]
    fn requests_and_faults_never_share_a_badge() {
        let a = TaskId(3);
        let b = TaskId(4);
        assert_ne!(a.service_badge(), a.fault_badge());
        assert_ne!(a.fault_badge(), b.service_badge());
    }

    #[test]
    fn an_unbadged_arrival_belongs_to_no_task() {
        assert_eq!(TaskId::from_badge(0), None);
        assert_eq!(TaskId::from_badge(1), None);
    }

    #[test]
    fn child_service_transport_is_send_and_grant_reply_only() {
        let ordinary = child_service_rights(Authority::NONE);
        assert_eq!(
            ordinary,
            sel4::CapRightsBuilder::none()
                .write(true)
                .grant_reply(true)
                .build()
        );

        let declared_transfer = child_service_rights(Authority {
            rights: boot_contracts::generation::RIGHT_TRANSFER,
            grants: 1,
        });
        assert_eq!(declared_transfer, ordinary);
    }

    #[test]
    fn construction_cleanup_owns_every_failure_transition() {
        let task = TaskId(7);
        let arena = TaskArenaId::from_raw(2, 9);
        for (transition, stage) in ConstructionStage::ALL.into_iter().enumerate() {
            let cleanup = construction_record(task, arena, transition);
            assert_eq!(cleanup.task, task, "stage {stage:?}");
            assert_eq!(cleanup.arena, arena, "stage {stage:?}");
            assert_eq!(cleanup.slot_count(), transition, "stage {stage:?}");
        }
    }

    #[test]
    fn every_granted_slot_fits_the_child_cnode() {
        assert!(CHILD_SLOT_FAULT < (1 << CHILD_CNODE_SIZE_BITS));
    }
}
