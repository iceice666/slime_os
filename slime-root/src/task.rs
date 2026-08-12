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

use crate::child_vspace::{
    ChildImage, ChildVSpace, MAX_CHILD_THREADS, ScratchPage, VSpaceError, create_child_vspace,
};
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

/// Slots in a child CNode for the fixture paths: null, service endpoint, own
/// TCB, fault handler, and the console endpoint at [`CHILD_SLOT_CONSOLE`].
/// Six bits, since that slot sits above every grant-nameable one.
pub const CHILD_CNODE_SIZE_BITS: usize = 6;

/// Child CSpace slot holding the badged root service endpoint.
pub const CHILD_SLOT_SERVICE: sel4::CPtrBits = 1;
/// Child CSpace slot holding the task's own TCB, when supervised.
pub const CHILD_SLOT_TCB: sel4::CPtrBits = 2;
/// Child CSpace slot holding the task's badged fault-handler endpoint.
pub const CHILD_SLOT_FAULT: sel4::CPtrBits = 3;
/// Child CSpace slot holding the badged console/debug endpoint (B41).
///
/// Above every slot a generation grant can name: grant slots are the
/// component's own numbering and start at 0, so a low fixed slot would collide
/// with declared authority in every migrated fixture.
pub const CHILD_SLOT_CONSOLE: sel4::CPtrBits = 32;

/// Destination slots in a child's CSpace, resolved from the admitted plan.
///
/// The root installs three capabilities it owns rather than the child: the
/// badged service endpoint, the child's own TCB when self-managed, and the
/// badged fault endpoint. Their addresses are the generation's to declare, so
/// this carries what the plan said rather than what the root assumed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChildSlots {
    pub service: sel4::CPtrBits,
    /// The console/debug endpoint (B41). Separate from `service` so console
    /// traffic neither consumes the root's lifecycle dispatcher nor shares its
    /// fault domain.
    pub console: sel4::CPtrBits,
    pub tcb: sel4::CPtrBits,
    pub fault: sel4::CPtrBits,
}

impl ChildSlots {
    /// The four-slot shell the fixture paths construct outside any plan.
    pub const SHELL: Self = Self {
        service: CHILD_SLOT_SERVICE,
        console: CHILD_SLOT_CONSOLE,
        tcb: CHILD_SLOT_TCB,
        fault: CHILD_SLOT_FAULT,
    };

    /// Refuse a layout the child could not actually use.
    ///
    /// Every component resolves the root endpoint from a compiled-in constant
    /// (`ROOT_SERVICE_SLOT` in the runtime's seL4 transport), so a plan that
    /// puts the service endpoint anywhere else yields a child whose first
    /// syscall invokes an empty slot. The plan does not get to move it until
    /// the runtime reads the slot from the boot layout too.
    ///
    /// The slots must also be distinct and non-null, or one install silently
    /// overwrites another and an unbadged arrival stops being distinguishable.
    pub fn validate(self) -> Result<Self, TaskError> {
        let mismatch = |slot| {
            Err(TaskError::CSpaceMismatch {
                slot,
                occupied: false,
            })
        };
        if self.service != CHILD_SLOT_SERVICE {
            return mismatch(self.service);
        }
        if self.tcb == 0 || self.fault == 0 || self.console == 0 {
            return mismatch(0);
        }
        let slots = [self.service, self.console, self.tcb, self.fault];
        for (index, slot) in slots.iter().enumerate() {
            if slots[index + 1..].contains(slot) {
                return mismatch(*slot);
            }
        }
        Ok(self)
    }
}

/// Refuse a declared priority the root cannot safely run a child at.
///
/// Refused rather than clamped: a child at or above the root's priority can
/// keep the service loop from running, and every other child would then block
/// behind it on a root that never answers. `build-generation.py` bounds this
/// too, so a manifest never reaches here carrying one; this is the side that
/// holds when a generation arrives from somewhere else (B48).
pub const fn admit_priority(priority: sel4::Word) -> Result<sel4::Word, TaskError> {
    if priority > CHILD_PRIORITY {
        return Err(TaskError::PriorityAboveRoot { priority });
    }
    Ok(priority)
}

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
    /// The plan declares a worker thread the image cannot run: it was built
    /// without `slime_rt::entry!`'s worker form, so it has no second entry
    /// point or stack.
    MissingWorkerImage,
    Alloc(AllocError),
    /// A plan declared a child priority at or above the root's own, which
    /// would let the child keep the service loop from running (B48).
    PriorityAboveRoot {
        priority: sel4::Word,
    },
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
    /// The constructed CSpace does not match the admitted plan: a slot the
    /// plan declared is empty, or a slot it did not declare is occupied.
    CSpaceMismatch {
        slot: sel4::CPtrBits,
        occupied: bool,
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
    /// How large that CNode is, so a later install can address it.
    ///
    /// The plan's, not the compiled-in default: a CSpace is sized to the
    /// authority its instance declares, and resolving a slot at the wrong
    /// depth silently lands somewhere else.
    pub cnode_size_bits: usize,
    pub tcb: sel4::cap::Tcb,
    /// Additional threads of this process, indexed by thread number; index 0
    /// is always `None` because that is `tcb` above (B47). Allocated from the
    /// task's own arena, so teardown reclaims them with everything else.
    pub workers: [Option<sel4::cap::Tcb>; MAX_CHILD_THREADS],
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
        // The console/debug endpoint (B41). A separate object from the root
        // service endpoint, so console traffic has its own queue and its own
        // fault domain.
        console_endpoint: sel4::cap::Endpoint,
        authority: Authority,
        supervision: Supervision,
        caller_vspace: sel4::cap::VSpace,
        scratch: &ScratchPage,
        asid_pool: sel4::cap::AsidPool,
        spawner: Option<TaskId>,
        executable: Option<usize>,
        instance: Option<usize>,
        // The authenticated boot action, delivered in the thread's first C
        // parameter. Only the bootstrap instance reads it; every other
        // component receives zero and ignores it.
        startup_arg: u32,
        // CNode size in bits from the generation's admitted plan, so a child's
        // CSpace is exactly as large as its declared authority needs. Falls
        // back to the minimum shell for the fixture paths, which carry no plan.
        cnode_size_bits: usize,
        // Destination slots for the child's own TCB and fault endpoint, from
        // the same plan. The fixture paths pass the compiled-in shell slots.
        child_slots: ChildSlots,
        // Scheduling priority from the plan's `ScheduleRecord`. The fixture
        // paths pass `CHILD_PRIORITY`, which is also what an instance that
        // declares none resolves to (B48).
        priority: sel4::Word,
        // Threads this process runs, from the plan's process record. One for
        // every component that does not declare `extraThreads` (B47).
        threads: usize,
        // Each thread's declared priority, indexed by thread number. Index 0
        // is unused -- the main thread takes `priority` above -- and the rest
        // come from the plan's per-thread schedule records (B48).
        worker_priorities: [sel4::Word; MAX_CHILD_THREADS],
    ) -> Result<TaskId, TaskError> {
        admit_priority(priority)?;
        let Some(index) = self.tasks.iter().position(Option::is_none) else {
            return Err(TaskError::TableFull { limit: CAPACITY });
        };
        let id = TaskId(self.next_id);
        let mut plan = image.vspace_arena_plan().map_err(VSpaceError::Image)?;
        plan.add(sel4::cap_type::CNode::object_blueprint(cnode_size_bits))
            .ok_or(TaskError::Alloc(AllocError::UntypedExhausted {
                size_bits: usize::BITS as usize,
                remaining: 0,
            }))?;
        // One TCB per thread: a thread is exactly a TCB, a stack, an IPC
        // buffer, and a schedule, and the arena must cover all of them before
        // any is allocated.
        for _ in 0..threads {
            plan.add(sel4::cap_type::Tcb::object_blueprint())
                .ok_or(TaskError::Alloc(AllocError::UntypedExhausted {
                    size_bits: usize::BITS as usize,
                    remaining: 0,
                }))?;
        }
        let arena_bits =
            plan.required_size_bits()
                .ok_or(TaskError::Alloc(AllocError::UntypedExhausted {
                    size_bits: usize::BITS as usize,
                    remaining: 0,
                }))?;
        let arena = allocator.begin_task_arena(arena_bits)?;

        let construction = (|| {
            let vspace = create_child_vspace(
                allocator,
                arena,
                image,
                caller_vspace,
                scratch,
                asid_pool,
                threads,
            )?;
            let cnode = allocator
                .allocate_variable_in::<sel4::cap_type::CNode>(arena, cnode_size_bits)?
                .cap();
            let tcb = allocator
                .allocate_fixed_in::<sel4::cap_type::Tcb>(arena)?
                .cap();
            #[cfg(slime_b38_force_unwind)]
            if spawner.is_some() && crate::object_allocator::take_forced_unwind() {
                return Err(TaskError::ForcedConstructionFailure);
            }

            let root_cnode = sel4::init_thread::slot::CNODE.cap();
            let mut ledger = InstallLedger::default();
            // Slot 1 is invocation-only transport. In particular it must never
            // carry receive authority: all children share the root endpoint,
            // so a receiver could dequeue and answer another child's request
            // before the root dispatcher saw it.
            mint_child_slot(
                cnode,
                cnode_size_bits,
                child_slots.service,
                &root_cnode.absolute_cptr(service_endpoint),
                {
                    // Wrong rights: grant read on the shared root endpoint,
                    // which would let this child dequeue another's request.
                    #[cfg(slime_b40_mutate_wrong_rights)]
                    {
                        sel4::CapRights::all()
                    }
                    #[cfg(not(slime_b40_mutate_wrong_rights))]
                    {
                        child_service_rights(authority)
                    }
                },
                id.service_badge(),
                true,
                &mut ledger,
            )?;
            // Slot 3: the same endpoint object under this task's fault badge.
            // The kernel requires a fault handler endpoint to carry send plus
            // grant or grant-reply authority, and resolves this CPtr in the
            // child's CSpace.
            mint_child_slot(
                cnode,
                cnode_size_bits,
                {
                    #[cfg(slime_b40_mutate_wrong_slot)]
                    {
                        child_slots.fault.wrapping_add(1) % (1 << cnode_size_bits)
                    }
                    #[cfg(not(slime_b40_mutate_wrong_slot))]
                    {
                        child_slots.fault
                    }
                },
                &root_cnode.absolute_cptr(service_endpoint),
                sel4::CapRightsBuilder::none()
                    .write(true)
                    .grant_reply(true)
                    .build(),
                {
                    // Aliased: reuse the service badge so the fault slot holds
                    // a capability indistinguishable from the service one.
                    #[cfg(slime_b40_mutate_aliased)]
                    {
                        id.service_badge()
                    }
                    #[cfg(not(slime_b40_mutate_aliased))]
                    {
                        id.fault_badge()
                    }
                },
                true,
                &mut ledger,
            )?;
            // Write-only, and never receive: every child shares the console
            // dispatcher, so a receiver could dequeue another child's output
            // before the console saw it.
            mint_child_slot(
                cnode,
                cnode_size_bits,
                child_slots.console,
                &root_cnode.absolute_cptr(console_endpoint),
                // Write plus reply: a console write is one-way, but an input
                // read on the same endpoint is a Call. Never recv — every
                // child shares this dispatcher, so a receiver could answer
                // another child's read.
                sel4::CapRightsBuilder::none()
                    .write(true)
                    .grant_reply(true)
                    .build(),
                id.service_badge(),
                true,
                &mut ledger,
            )?;
            if supervision == Supervision::SelfManaged {
                mint_child_slot(
                    cnode,
                    cnode_size_bits,
                    child_slots.tcb,
                    &root_cnode.absolute_cptr(tcb),
                    sel4::CapRights::all(),
                    0,
                    false,
                    &mut ledger,
                )?;
                #[cfg(slime_b40_mutate_wrong_type)]
                {
                    // Wrong type: the plan binds a TCB here, so replace it
                    // with the CNode. Occupancy is unchanged, which is exactly
                    // why the audit must check the installed type and not only
                    // whether something is present.
                    let cptr =
                        cnode.absolute_cptr_from_bits_with_depth(child_slots.tcb, cnode_size_bits);
                    let _ = cptr.delete();
                    let _ = cptr.copy(&root_cnode.absolute_cptr(cnode), sel4::CapRights::all());
                }
            }

            // Audit the constructed CSpace against what the plan declared, by
            // asking the kernel rather than trusting the loop above. Every
            // slot the plan named must be occupied and every slot it did not
            // must be empty; a `Delete` on an empty slot succeeds and on a
            // full one is refused, which is the only way to observe occupancy
            // without destroying it.
            //
            // This catches an install that silently landed elsewhere — a wrong
            // depth, a stale constant, a plan naming a slot outside the CNode.
            audit_child_cspace(cnode, cnode_size_bits, child_slots, supervision)?;
            // Type is not an occupancy question, so it is probed separately.
            audit_child_types(cnode, cnode_size_bits, child_slots, supervision)?;

            tcb.tcb_configure(
                sel4::CPtr::from_bits(child_slots.fault),
                cnode,
                sel4::CNodeCapData::new(0, sel4::WORD_SIZE - cnode_size_bits),
                vspace.vspace,
                vspace.main().ipc_buffer_addr as sel4::Word,
                vspace.main().ipc_buffer,
            )
            .map_err(TaskError::Configure)?;
            tcb.tcb_set_sched_params(sel4::init_thread::slot::TCB.cap(), priority, priority)
                .map_err(TaskError::SchedParams)?;

            let entry = image.entry();
            let mut context = sel4::UserContext::default();
            *context.pc_mut() =
                sel4::Word::try_from(entry).map_err(|_| TaskError::EntryOutOfRange { entry })?;
            *context.c_param_mut(0) = sel4::Word::from(startup_arg);
            // `resume = false`: nothing runs until every allocation for every
            // task in this generation has succeeded. The child runtime
            // establishes its own stack pointer at `_start`.
            tcb.tcb_write_all_registers(false, &mut context)
                .map_err(TaskError::WriteRegisters)?;

            // Every thread beyond the main one (B47). Same CSpace and VSpace —
            // that is what makes them threads of one process rather than
            // separate tasks — with its own TCB, IPC buffer, stack, and
            // schedule.
            let mut workers = [None; MAX_CHILD_THREADS];
            #[allow(clippy::never_loop)]
            for (index, slot) in workers.iter_mut().enumerate().take(threads).skip(1) {
                // The image must declare a worker entry point and stack. A
                // plan asking for a thread an image cannot run is refused
                // rather than started at a garbage PC.
                let worker = image.worker().ok_or(TaskError::MissingWorkerImage)?;
                let worker_tcb = allocator
                    .allocate_fixed_in::<sel4::cap_type::Tcb>(arena)?
                    .cap();
                worker_tcb
                    .tcb_configure(
                        sel4::CPtr::from_bits(child_slots.fault),
                        cnode,
                        sel4::CNodeCapData::new(0, sel4::WORD_SIZE - cnode_size_bits),
                        vspace.vspace,
                        vspace.pages[index].ipc_buffer_addr as sel4::Word,
                        vspace.pages[index].ipc_buffer,
                    )
                    .map_err(TaskError::Configure)?;
                // The worker's own declared priority, not its main thread's
                // (B48). Below it, a component can hold a busy thread while
                // its own IPC stays responsive and unrelated services keep
                // running; the `ScheduleRecord` has always been per-thread.
                let worker_priority = admit_priority(worker_priorities[index])?;
                worker_tcb
                    .tcb_set_sched_params(
                        sel4::init_thread::slot::TCB.cap(),
                        worker_priority,
                        worker_priority,
                    )
                    .map_err(TaskError::SchedParams)?;
                // The thread index, in the register the runtime reads through
                // `TPIDR_EL0`. This is what lets a thread find its own IPC
                // buffer and transfer window without any shared state: the
                // kernel context-switches this register, so no two threads can
                // observe each other's value.

                let mut worker_context = sel4::UserContext::default();
                *worker_context.pc_mut() = worker.entry;
                *worker_context.sp_mut() = worker.stack_top;
                *worker_context.c_param_mut(0) = sel4::Word::from(startup_arg);
                // The thread index, in `TPIDR_EL0`. Set here rather than
                // through `seL4_TCB_SetTLSBase` because seL4 counts that
                // register in the general-purpose set: a later
                // `WriteRegisters` writes the whole set, so a separately
                // invoked TLS base is overwritten with this context's zero.
                //
                // This is what lets a thread find its own IPC buffer and
                // transfer window with no shared state — the kernel
                // context-switches the register, so no two threads can observe
                // each other's value.
                worker_context.inner_mut().tpidr_el0 = index as sel4::Word;
                worker_tcb
                    .tcb_write_all_registers(false, &mut worker_context)
                    .map_err(TaskError::WriteRegisters)?;
                *slot = Some(worker_tcb);
            }
            Ok((vspace, cnode, tcb, entry, workers))
        })();

        let (vspace, cnode, tcb, entry, workers) = match construction {
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
            cnode_size_bits,
            workers,
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
        // Workers start with the main thread. Their TLS base is already set,
        // so each finds its own IPC buffer on its first syscall (B47).
        for worker in task.workers.iter().flatten() {
            worker.tcb_resume().map_err(TaskError::Resume)?;
        }
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

/// Verify each installed capability has the type the plan declared.
///
/// Occupancy says a slot is full; it does not say what filled it. seL4 exposes
/// no "read this slot's type", but it does refuse a type-specific invocation
/// against the wrong object: `decodeInvocation` dispatches on the capability's
/// type and answers `InvalidCapability` when no branch matches. Suspending a
/// TCB is such an invocation, and it is idempotent on a thread the root has
/// not yet resumed, so it costs nothing to ask.
///
/// This catches the confusion that matters: a CNode or endpoint sitting where
/// the plan declared the child's own TCB would hand the child authority of an
/// entirely different shape.
fn audit_child_types(
    cnode: sel4::cap::CNode,
    cnode_size_bits: usize,
    slots: ChildSlots,
    supervision: Supervision,
) -> Result<(), TaskError> {
    if supervision != Supervision::SelfManaged {
        return Ok(());
    }
    // The slot lives in the child's CSpace, which the root cannot invoke
    // through directly, so copy it into a root slot and probe that. Slot 0 of
    // the child CNode is the null slot the plan never binds \u2014 the occupancy
    // audit that runs first has already established it is empty \u2014 so it is
    // available as scratch, and the copy is deleted before returning.
    let root_cnode = sel4::init_thread::slot::CNODE.cap();
    let scratch_bits = sel4::init_thread::slot::NULL.cptr().bits();
    let scratch = root_cnode.absolute_cptr_from_bits_with_depth(scratch_bits, sel4::WORD_SIZE);
    let source = cnode.absolute_cptr_from_bits_with_depth(slots.tcb, cnode_size_bits);
    if scratch.copy(&source, sel4::CapRights::all()).is_err() {
        return Err(TaskError::CSpaceMismatch {
            slot: slots.tcb,
            occupied: false,
        });
    }
    // `tcb_suspend` is refused with `InvalidCapability` for every non-TCB
    // type, and the thread has not been resumed yet, so it is a no-op here.
    let is_tcb = sel4::cap::Tcb::from_bits(scratch_bits)
        .tcb_suspend()
        .is_ok();
    let _ = scratch.delete();
    if !is_tcb {
        return Err(TaskError::CSpaceMismatch {
            slot: slots.tcb,
            occupied: true,
        });
    }
    Ok(())
}

/// Compare a constructed child CSpace to the slots the plan declared.
///
/// Occupancy is probed with `seL4_CNode_Move` onto the slot itself: the kernel
/// refuses a move whose destination is occupied and refuses one whose source
/// is empty, and in neither case is anything moved. That makes it a read-only
/// occupancy question with no scratch slot and no risk of destroying the
/// capability under audit.
fn audit_child_cspace(
    cnode: sel4::cap::CNode,
    cnode_size_bits: usize,
    slots: ChildSlots,
    supervision: Supervision,
) -> Result<(), TaskError> {
    let expect_tcb = supervision == Supervision::SelfManaged;
    // Negative-mutation probes for `just sel4_capability_layout_check`. Each
    // perturbs the constructed CSpace in one of the ways B40 names, so the
    // audit's refusal is observed rather than assumed. None is compiled into
    // the product image.
    #[cfg(slime_b40_mutate_missing)]
    {
        // Missing: delete a capability the plan declared.
        let _ = cnode
            .absolute_cptr_from_bits_with_depth(slots.fault, cnode_size_bits)
            .delete();
    }
    #[cfg(slime_b40_mutate_extra)]
    {
        // Extra: install a capability into a slot the plan left empty.
        let free = (0..(1u64 << cnode_size_bits) as sel4::CPtrBits)
            .find(|slot| {
                *slot != slots.service && *slot != slots.fault && *slot != slots.tcb && *slot != 0
            })
            .unwrap_or(0);
        let _ = cnode
            .absolute_cptr_from_bits_with_depth(free, cnode_size_bits)
            .copy(
                &cnode.absolute_cptr_from_bits_with_depth(slots.service, cnode_size_bits),
                sel4::CapRights::all(),
            );
    }

    for slot in 0..(1u64 << cnode_size_bits) {
        let slot = slot as sel4::CPtrBits;
        let declared = slot == slots.service
            || slot == slots.console
            || slot == slots.fault
            || (expect_tcb && slot == slots.tcb);
        let cptr = cnode.absolute_cptr_from_bits_with_depth(slot, cnode_size_bits);
        // `Move` onto itself: `DeleteFirst` means occupied, `FailedLookup`
        // means empty. Any other answer is the slot not being addressable,
        // which is itself a layout defect.
        let occupied = match cptr.move_(&cptr) {
            Err(sel4::Error::DeleteFirst) => true,
            Err(sel4::Error::FailedLookup) => false,
            _ => {
                return Err(TaskError::CSpaceMismatch {
                    slot,
                    occupied: false,
                });
            }
        };
        if occupied != declared {
            return Err(TaskError::CSpaceMismatch { slot, occupied });
        }
    }
    Ok(())
}

/// A source capability's address: root CNode, index, and resolution depth.
/// Two installs naming the same path under the same badge are indistinguishable
/// to the child, which is what makes them an alias.
type SourcePath = (sel4::CPtrBits, sel4::CPtrBits, usize);

/// Records what the root installed into a child CSpace, so an alias — the same
/// object under the same badge reachable at two addresses — is refused.
///
/// The kernel cannot answer "what is in this slot": occupancy is observable,
/// identity is not. So identity is tracked on the way in. Every install goes
/// through `mint_child_slot`, which is the only path that writes a child slot,
/// making this ledger complete by construction rather than by convention.
#[derive(Default)]
struct InstallLedger {
    entries: [Option<(SourcePath, sel4::Badge)>; MAX_CHILD_INSTALLS],
    len: usize,
}

/// Distinct capabilities the root installs into one child: service, console,
/// fault, TCB. The input slot names the console's endpoint, so it is not a
/// separate install.
const MAX_CHILD_INSTALLS: usize = 4;

impl InstallLedger {
    /// Record one install, refusing a source/badge pair already present.
    ///
    /// Two slots may legitimately hold the same *object* — the service and
    /// fault endpoints are one endpoint under two badges — so identity here is
    /// the pair, not the object alone. Equal pairs are indistinguishable to
    /// the child and to the kernel, which is what makes them an alias.
    fn record(
        &mut self,
        slot: sel4::CPtrBits,
        source: SourcePath,
        badge: sel4::Badge,
        rights: &sel4::CapRights,
        // Whether this install targets the shared root service endpoint. The
        // child's own TCB legitimately carries every right, so the receive ban
        // below applies only to the endpoint every child shares.
        shared_endpoint: bool,
    ) -> Result<(), TaskError> {
        // Rights are checked here rather than by probing the installed slot,
        // because seL4 masks a copy's rights silently and never reports back
        // what a capability carries. This is the single chokepoint every
        // child install passes through, so the check is complete.
        //
        // No child may hold receive authority on the root service endpoint:
        // all children share it, so a receiver could dequeue and answer
        // another child's request before the root dispatcher saw it. That is
        // a confinement property, not a policy, so it holds regardless of
        // what the grant asked for.
        if shared_endpoint && rights.clone().into_inner().get_capAllowRead() != 0 {
            return Err(TaskError::CSpaceMismatch {
                slot,
                occupied: true,
            });
        }
        for entry in self.entries.iter().flatten() {
            if *entry == (source, badge) {
                return Err(TaskError::CSpaceMismatch {
                    slot,
                    occupied: true,
                });
            }
        }
        if self.len >= MAX_CHILD_INSTALLS {
            return Err(TaskError::CSpaceMismatch {
                slot,
                occupied: true,
            });
        }
        self.entries[self.len] = Some((source, badge));
        self.len += 1;
        Ok(())
    }
}

fn mint_child_slot(
    cnode: sel4::cap::CNode,
    // Depth must match the CNode's own size: the child's CSpace is sized from
    // the admitted plan, so a fixed depth here would resolve the slot against
    // a guard the CNode does not have and install at the wrong address.
    cnode_size_bits: usize,
    slot: sel4::CPtrBits,
    source: &sel4::AbsoluteCPtr,
    rights: sel4::CapRights,
    badge: sel4::Badge,
    shared_endpoint: bool,
    ledger: &mut InstallLedger,
) -> Result<(), TaskError> {
    ledger.record(
        slot,
        // Identity is the full source path, not just its bits: two sources
        // addressed through different roots or at different depths can carry
        // colliding bits without naming the same capability.
        (
            source.root().bits(),
            source.path().bits(),
            source.path().depth(),
        ),
        badge,
        &rights,
        shared_endpoint,
    )?;
    cnode
        .absolute_cptr_from_bits_with_depth(slot, cnode_size_bits)
        .mint(source, rights, badge)
        .map_err(|error| TaskError::Mint { slot, error })
}

#[cfg(test)]
mod tests {
    use super::{
        Arrival, CHILD_CNODE_SIZE_BITS, CHILD_PRIORITY, CHILD_SLOT_CONSOLE, CHILD_SLOT_FAULT,
        CHILD_SLOT_SERVICE, ChildSlots, ConstructionStage, InstallLedger, MAX_CHILD_INSTALLS,
        TaskError, TaskId, admit_priority, child_service_rights, construction_record,
    };
    use crate::generation::Authority;
    use crate::object_allocator::TaskArenaId;

    /// B48: a declared priority at or above the root's is refused, not clamped.
    ///
    /// The builder bounds this too, so a manifest cannot carry one. This is
    /// the root's own guard, which is what holds for a generation that did not
    /// come from `build-generation.py` — and clamping instead would let such a
    /// generation silently run at a priority it did not ask for.
    #[test]
    fn a_priority_at_or_above_the_root_is_refused() {
        assert_eq!(admit_priority(0), Ok(0));
        assert_eq!(admit_priority(100), Ok(100));
        assert_eq!(
            admit_priority(CHILD_PRIORITY),
            Ok(CHILD_PRIORITY),
            "the default is itself admissible"
        );
        for priority in [CHILD_PRIORITY + 1, 255, sel4::Word::MAX] {
            assert_eq!(
                admit_priority(priority),
                Err(TaskError::PriorityAboveRoot { priority }),
                "priority {priority} would outrank the root's service loop"
            );
        }
    }

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

    /// Record an install of a non-shared capability carrying no rights, which
    /// is the shape the ledger's identity rules are about.
    fn record(
        ledger: &mut InstallLedger,
        slot: sel4::CPtrBits,
        source: sel4::CPtrBits,
        badge: sel4::Badge,
    ) -> Result<(), TaskError> {
        ledger.record(
            slot,
            (0, source, sel4::WORD_SIZE),
            badge,
            &sel4::CapRights::none(),
            false,
        )
    }

    /// The ledger's whole purpose: two slots holding a capability the child
    /// cannot tell apart is an alias, and the second install is refused.
    #[test]
    fn ledger_refuses_an_identical_source_and_badge() {
        let mut ledger = InstallLedger::default();
        assert!(record(&mut ledger, 1, 7, 0x20).is_ok());
        assert!(matches!(
            record(&mut ledger, 3, 7, 0x20),
            Err(TaskError::CSpaceMismatch { slot: 3, .. })
        ));
    }

    /// The service and fault slots are one endpoint under two badges, which is
    /// the intended layout rather than an alias.
    #[test]
    fn ledger_admits_one_object_under_distinct_badges() {
        let mut ledger = InstallLedger::default();
        assert!(record(&mut ledger, 1, 7, 0x20).is_ok());
        assert!(record(&mut ledger, 3, 7, 0x21).is_ok());
    }

    /// A different object under a badge already used is still distinguishable,
    /// so it is not an alias.
    #[test]
    fn ledger_admits_distinct_objects_under_one_badge() {
        let mut ledger = InstallLedger::default();
        assert!(record(&mut ledger, 1, 7, 0).is_ok());
        assert!(record(&mut ledger, 2, 9, 0).is_ok());
    }

    /// More installs than a child CSpace can legitimately receive means the
    /// construction path grew a case the ledger was never sized for.
    #[test]
    fn ledger_refuses_more_installs_than_a_child_receives() {
        let mut ledger = InstallLedger::default();
        for index in 0..MAX_CHILD_INSTALLS {
            assert!(
                record(
                    &mut ledger,
                    index as sel4::CPtrBits,
                    index as sel4::CPtrBits,
                    0
                )
                .is_ok()
            );
        }
        assert!(record(&mut ledger, 9, 99, 0).is_err());
    }

    /// The confinement property the ledger enforces: no child may hold receive
    /// authority on the endpoint every child shares, whatever the grant asked
    /// for. A child that could receive would dequeue another child's request
    /// before the root dispatcher saw it.
    #[test]
    fn ledger_refuses_receive_on_the_shared_endpoint() {
        let mut ledger = InstallLedger::default();
        assert!(matches!(
            ledger.record(
                1,
                (0, 7, sel4::WORD_SIZE),
                0x20,
                &sel4::CapRights::all(),
                true
            ),
            Err(TaskError::CSpaceMismatch { slot: 1, .. })
        ));
    }

    /// The child's own TCB is not shared, so it legitimately carries every
    /// right; the ban must not reach it.
    #[test]
    fn ledger_admits_full_rights_on_an_unshared_capability() {
        let mut ledger = InstallLedger::default();
        assert!(
            ledger
                .record(
                    2,
                    (0, 7, sel4::WORD_SIZE),
                    0,
                    &sel4::CapRights::all(),
                    false
                )
                .is_ok()
        );
    }

    /// A plan may not move the service endpoint: every component resolves it
    /// from a compiled-in constant, so a child given it anywhere else would
    /// invoke an empty slot on its first syscall.
    #[test]
    fn a_moved_service_slot_is_refused() {
        let moved = ChildSlots {
            service: CHILD_SLOT_SERVICE + 1,
            ..ChildSlots::SHELL
        };
        assert!(moved.validate().is_err());
        assert!(ChildSlots::SHELL.validate().is_ok());
    }

    /// Two capabilities in one slot means one silently overwrote the other.
    #[test]
    fn colliding_child_slots_are_refused() {
        let collided = ChildSlots {
            tcb: CHILD_SLOT_FAULT,
            ..ChildSlots::SHELL
        };
        assert!(collided.validate().is_err());
    }

    /// Slot 0 stays null so an unbadged arrival is distinguishable.
    #[test]
    fn a_null_child_slot_is_refused() {
        let nulled = ChildSlots {
            tcb: 0,
            ..ChildSlots::SHELL
        };
        assert!(nulled.validate().is_err());
    }

    /// The console slot must sit above every slot a generation grant can name,
    /// or it collides with declared authority in a migrated fixture.
    #[test]
    fn the_console_slot_clears_grant_numbering() {
        // 22 is the highest slot any seL4 fixture declares today; the bound is
        // what matters, not the exact figure.
        assert!(CHILD_SLOT_CONSOLE > 22);
        assert!(CHILD_SLOT_CONSOLE < (1 << CHILD_CNODE_SIZE_BITS));
    }

    /// A layout naming one slot twice is refused: one install would silently
    /// overwrite another.
    #[test]
    fn a_console_slot_colliding_with_another_is_refused() {
        let collided = ChildSlots {
            console: CHILD_SLOT_FAULT,
            ..ChildSlots::SHELL
        };
        assert!(collided.validate().is_err());
    }

    /// The fixture shell must address every slot it names inside the smallest
    /// CNode the root builds, or the fixture paths install out of bounds.
    #[test]
    fn the_shell_slots_fit_the_shell_cnode() {
        let shell = ChildSlots::SHELL;
        let bound = 1 << CHILD_CNODE_SIZE_BITS;
        assert!(shell.console < bound);
        assert!(shell.service < bound);
        assert!(shell.tcb < bound);
        assert!(shell.fault < bound);
        // Distinct, or one install would silently overwrite another.
        assert_ne!(shell.service, shell.tcb);
        assert_ne!(shell.service, shell.fault);
        assert_ne!(shell.tcb, shell.fault);
        // Slot 0 stays null: an unbadged arrival must be distinguishable.
        assert_ne!(shell.service, 0);
        assert_ne!(shell.tcb, 0);
        assert_ne!(shell.fault, 0);
    }
}
