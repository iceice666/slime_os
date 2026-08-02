use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;
use core::arch::global_asm;
use spin::{LazyLock, Mutex};

use crate::capability::{
    Capability, CapabilityTable, KernelObject, RIGHT_SPAWN, RIGHT_TRANSFER, Rights,
};
use crate::gdt::{USER_CODE_SELECTOR, USER_DATA_SELECTOR};
use crate::memory::address_space::AddressSpace;
use crate::memory::pmm::FRAME_ALLOCATOR;
use crate::memory::shared_buffer::{HolderQuota, SHARED_BUFFER_TABLE};
use crate::memory::vmm::{MapError, PTE_NO_EXECUTE, PTE_PRESENT, PTE_USER, PTE_WRITABLE};
use crate::memory::{PAGE_SIZE, VirtAddr};
use crate::trap::UserFrame;
use boot_contracts::target_profile::TargetProfile;

pub const KERNEL_STACK_SIZE: usize = 32 * 1024;
const SWITCH_STACK_SIZE: usize = 4096;
static mut SWITCH_STACK: [u8; SWITCH_STACK_SIZE] = [0; SWITCH_STACK_SIZE];

fn switch_stack_top() -> u64 {
    core::ptr::addr_of_mut!(SWITCH_STACK) as u64 + SWITCH_STACK_SIZE as u64
}
/// Hard global bound on simultaneously live tasks. The 24 MiB heap reserves
/// at most 2 MiB for eager kernel stacks, leaving generation/object-store
/// staging headroom. Per-spawner limits provide the finer M6.1 bound.
pub const MAX_TASKS: usize = 64;
pub const DEFAULT_SPAWN_BUDGET: u16 = 16;
pub const MAX_SPAWN_BUDGET: u16 = 32;
pub const ENTRY_VA: u64 = 0x0000_0000_0040_0000;
pub const USER_STACK_TOP: u64 = 0x0000_7fff_ffff_f000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserFaultReason {
    DivByZero,
    UndefinedOp,
    GeneralProt,
    PageFault,
    Unknown(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermReason {
    Exit(i64),
    Fault(UserFaultReason),
    Timeout,
    PeerLoss,
    Unhealthy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockReason {
    Endpoint,
    Input,
    Supervision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Ready,
    Running,
    Blocked(BlockReason),
    Terminated(TermReason),
}

pub type TaskId = u64;

pub struct Task {
    pub id: TaskId,
    pub state: TaskState,
    pub address_space: AddressSpace,
    pub kernel_stack: Box<[u8]>,
    pub saved: UserFrame,
    pub caps: CapabilityTable,
    pub spawner: Option<TaskId>,
    pub spawn_budget: u16,
    pub live_children: u16,
    /// Task to re-ready when this task terminates, set by `SYS_WAIT` on a
    /// supervising parent. Consumed on the first wake.
    pub wake_on_terminate: Option<TaskId>,
    /// This holder's generation-declared shared-buffer quota (C7.3). Charged as
    /// the creating supervision-subtree account. Defaults to
    /// [`HolderQuota::DENY`]: a component with no declared budget may hold no
    /// shared buffer.
    pub shared_buffer_quota: HolderQuota,
}

impl Task {
    fn kernel_stack_top(&self) -> u64 {
        let top = self.kernel_stack.as_ptr() as u64 + self.kernel_stack.len() as u64;
        top & !0xf
    }
}

struct Scheduler {
    tasks: Vec<Task>,
    ready: VecDeque<TaskId>,
    current: Option<TaskId>,
    next_id: TaskId,
    on_idle: Option<extern "C" fn()>,
    terminated: Vec<(TaskId, TermReason)>,
}

impl Scheduler {
    fn new() -> Self {
        Self {
            tasks: Vec::new(),
            ready: VecDeque::new(),
            current: None,
            next_id: 1,
            on_idle: None,
            terminated: Vec::new(),
        }
    }

    fn index_of(&self, id: TaskId) -> Option<usize> {
        self.tasks.iter().position(|task| task.id == id)
    }
}

/// Remove a task that was never scheduled, releasing everything it holds.
///
/// Used by the `spawn_from_cap` failure path, where a fully built task must be
/// undone because the spawner's capability table had no room for its
/// supervision handle. Dropping the `Task` runs `AddressSpace::drop`, which
/// returns the image pages, the stack pages, and the user-half page tables — so
/// a rejected spawn costs nothing (B9).
fn remove_task(sched: &mut Scheduler, id: TaskId) {
    if let Some(index) = sched.index_of(id) {
        sched.tasks.remove(index);
    }
    sched.ready.retain(|ready| *ready != id);
}

/// Remove a never-scheduled task and report whether it was present.
///
/// The test-facing form of [`remove_task`]: it lets a harness prove the release
/// path returns exactly what a spawn consumed, without needing to run the task
/// to termination first.
pub fn release_unscheduled(id: TaskId) -> bool {
    without_interrupts(|| {
        let mut sched = SCHEDULER.lock();
        let present = sched.index_of(id).is_some();
        remove_task(&mut sched, id);
        present
    })
}

/// The PML4 of a live task, or `None` when no such task exists.
///
/// Exposed for the reclamation harness, which needs to install a mapping into a
/// spawned task's address space to prove that releasing the task does not
/// double-free a frame owned by the shared-buffer table.
pub fn address_space_root(id: TaskId) -> Option<crate::memory::PhysAddr> {
    let sched = SCHEDULER.lock();
    sched
        .index_of(id)
        .map(|index| sched.tasks[index].address_space.pml4())
}

/// Release every terminated task except the one currently being executed on.
///
/// A task cannot be freed at the moment it terminates: `terminate` is still
/// running on that task's kernel stack and in its address space, and the `Task`
/// owns both. So termination only records the exit, and the frames come back
/// here — from a later scheduling event, once the CPU has moved on. `executing`
/// names the task this call is standing on and is always skipped.
///
/// Dropping the `Task` drops its `AddressSpace`, whose `Drop` returns the image
/// pages, the stack pages, and the user-half page tables (B9). Its kernel stack
/// is a boxed slice and returns to the heap with it.
///
/// The termination *reason* deliberately outlives the task: `sched.terminated`
/// is a separate log, so `supervision_status` and `SYS_WAIT` still answer for a
/// child whose frames are already gone. That is what lets this run eagerly
/// instead of waiting for every supervisor to collect.
///
/// Callers must hold `SCHEDULER`. Freeing takes `FRAME_ALLOCATOR` underneath
/// it, matching the `SCHEDULER -> FRAME_ALLOCATOR` order the adjacent
/// `reclaim_owner` call already establishes.
fn reap_terminated(sched: &mut Scheduler, executing: Option<TaskId>) {
    let mut index = 0;
    while index < sched.tasks.len() {
        let task = &sched.tasks[index];
        if !matches!(task.state, TaskState::Terminated(_)) || Some(task.id) == executing {
            index += 1;
            continue;
        }
        let id = task.id;
        // Dropping the task frees its address space and kernel stack.
        sched.tasks.remove(index);
        sched.ready.retain(|ready| *ready != id);
    }
}

/// Propagates active kernel-half mappings to every task address space.
/// Callers must not hold `SCHEDULER`; this function acquires that lock.
pub(crate) fn synchronize_kernel_mappings(source: crate::memory::PhysAddr) {
    let sched = SCHEDULER.lock();
    for task in &sched.tasks {
        let destination = task.address_space.pml4();
        if destination != source {
            crate::memory::vmm::copy_kernel_half(source, destination);
        }
    }
}

static SCHEDULER: LazyLock<Mutex<Scheduler>> = LazyLock::new(|| Mutex::new(Scheduler::new()));

/// Deferred wake queue. Wake sources (IPC send, keyboard IRQ, endpoint drop,
/// task termination) push a `TaskId` here instead of touching the scheduler
/// directly, because they may run while `SCHEDULER` is already held or from an
/// interrupt handler. `schedule_next` drains this queue under `SCHEDULER`
/// before dispatching, moving each still-`Blocked` task back to `Ready`.
///
/// Lock order is strict: `SCHEDULER` -> `Channel`/`QUEUE` -> `PENDING_WAKES`,
/// never the reverse. Pushing here never takes `SCHEDULER`.
static PENDING_WAKES: Mutex<Vec<TaskId>> = Mutex::new(Vec::new());

/// Records a pending wake for `id`. Safe to call from an interrupt handler,
/// from `Drop`, or while `SCHEDULER` is held: it disables interrupts and only
/// touches the leaf `PENDING_WAKES` lock. The actual `Blocked -> Ready`
/// transition happens later, in `schedule_next`.
pub fn wake(id: TaskId) {
    without_interrupts(|| {
        let mut pending = PENDING_WAKES.lock();
        if !pending.contains(&id) {
            pending.push(id);
        }
    });
}

/// Applies every deferred wake: a `Blocked` task becomes `Ready` and is
/// re-queued. Terminated or already-runnable tasks are ignored. Must be called
/// with `SCHEDULER` held.
fn drain_pending_wakes(sched: &mut Scheduler) {
    let drained: Vec<TaskId> = {
        let mut pending = PENDING_WAKES.lock();
        core::mem::take(&mut *pending)
    };
    for id in drained {
        if let Some(idx) = sched.index_of(id)
            && matches!(sched.tasks[idx].state, TaskState::Blocked(_))
        {
            sched.tasks[idx].state = TaskState::Ready;
            sched.ready.push_back(id);
        }
    }
}

/// Reports whether a task is present and not terminated. Used by `on_idle` to
/// distinguish a cleanly parked (`Blocked`) persistent service from one that
/// terminated.
pub fn is_live(id: TaskId) -> bool {
    let sched = SCHEDULER.lock();
    sched
        .index_of(id)
        .is_some_and(|idx| !matches!(sched.tasks[idx].state, TaskState::Terminated(_)))
}

global_asm!(
    r#"
    .global switch_to_user
    switch_to_user:
        mov rdx, rdi
        mov rax, [rdx+0]
        mov rbx, [rdx+8]
        mov rcx, [rdx+16]
        mov rsi, [rdx+32]
        mov rbp, [rdx+48]
        mov r8,  [rdx+56]
        mov r9,  [rdx+64]
        mov r10, [rdx+72]
        mov r11, [rdx+80]
        mov r12, [rdx+88]
        mov r13, [rdx+96]
        mov r14, [rdx+104]
        mov r15, [rdx+112]
        push qword ptr [rdx+152]
        push qword ptr [rdx+144]
        push qword ptr [rdx+136]
        push qword ptr [rdx+128]
        push qword ptr [rdx+120]
        mov rdi, [rdx+40]
        mov rdx, [rdx+24]
        iretq

    .global switch_address_space_and_user
    switch_address_space_and_user:
        cli
        mov rbx, rdi
        mov r12, rsi
        call {switch_stack_top}
        mov rsp, rax
        push rbx
        push r12
        call {tss_rsp0}
        pop r12
        pop rbx
        sub rax, 160
        mov rdi, rax
        mov rsi, r12
        mov rcx, 20
        rep movsq
        mov r10, rax
        mov cr3, rbx
        mov rsp, rax
        add rsp, 160
        push qword ptr [r10+152]
        push qword ptr [r10+144]
        push qword ptr [r10+136]
        push qword ptr [r10+128]
        push qword ptr [r10+120]
        mov rax, [r10+0]
        mov rbx, [r10+8]
        mov rcx, [r10+16]
        mov rdx, [r10+24]
        mov rsi, [r10+32]
        mov rdi, [r10+40]
        mov rbp, [r10+48]
        mov r8,  [r10+56]
        mov r9,  [r10+64]
        mov r11, [r10+80]
        mov r12, [r10+88]
        mov r13, [r10+96]
        mov r14, [r10+104]
        mov r15, [r10+112]
        mov r10, [r10+72]
        iretq
    "#,
    tss_rsp0 = sym crate::gdt::rsp0,
    switch_stack_top = sym switch_stack_top,
);

unsafe extern "C" {
    fn switch_address_space_and_user(pml4: u64, frame: *const UserFrame) -> !;
}

pub fn spawn_user(image: &[u8]) -> Result<TaskId, SpawnError> {
    spawn_with_caps(image, Vec::new())
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SpawnGrant {
    pub slot: u32,
    pub rights: Rights,
}

pub struct SpawnPlan {
    pub image: &'static [u8],
    pub name: Option<&'static str>,
    pub spawn_budget: u16,
    pub caps: Vec<Capability>,
}

/// Validate a spawn grant list against a capability table. The executable
/// slot must name an `Executable` carrying both `RIGHT_EXEC` and
/// `RIGHT_SPAWN`; every grant is a non-consuming derived copy whose requested
/// rights are narrow-only. Transferable derived copies additionally require
/// `RIGHT_TRANSFER` on the source capability.
pub fn preflight_spawn_grant(
    caps: &CapabilityTable,
    executable_slot: u32,
    grants: &[SpawnGrant],
) -> Result<SpawnPlan, SpawnError> {
    let (executable, name, spawn_budget) = caps
        .get(executable_slot)
        .filter(|cap| {
            cap.rights & (crate::capability::RIGHT_EXEC | RIGHT_SPAWN)
                == crate::capability::RIGHT_EXEC | RIGHT_SPAWN
        })
        .and_then(|cap| match cap.object {
            KernelObject::Executable {
                name,
                bytes,
                spawn_budget,
            } => Some((bytes, name, spawn_budget)),
            _ => None,
        })
        .ok_or(SpawnError::BadExecutable)?;
    let mut derived = Vec::with_capacity(grants.len());
    for (index, grant) in grants.iter().enumerate() {
        if grant.slot == executable_slot
            || grants[..index].iter().any(|seen| seen.slot == grant.slot)
        {
            return Err(SpawnError::BadCapability);
        }
        let Some(cap) = caps.get(grant.slot) else {
            return Err(SpawnError::BadCapability);
        };
        if grant.rights & RIGHT_TRANSFER != 0 && cap.rights & RIGHT_TRANSFER == 0 {
            return Err(SpawnError::BadCapability);
        }
        derived.push(
            cap.derive(grant.rights)
                .map_err(|_| SpawnError::BadCapability)?,
        );
    }
    Ok(SpawnPlan {
        image: executable,
        name,
        spawn_budget,
        caps: derived,
    })
}

pub fn spawn_from_cap(
    executable_slot: u32,
    grants: &[SpawnGrant],
) -> Result<(TaskId, u32), SpawnError> {
    let (spawner, plan, transferable_supervision) = with_current_mut(|task| {
        if task.live_children >= task.spawn_budget {
            return Err(SpawnError::BudgetExhausted);
        }
        if task.caps.available_slots() == 0 {
            return Err(SpawnError::BadCapability);
        }
        let transferable_supervision = task
            .caps
            .get(executable_slot)
            .is_some_and(|cap| cap.rights & RIGHT_TRANSFER != 0);
        let plan = preflight_spawn_grant(&task.caps, executable_slot, grants)?;
        Ok((task.id, plan, transferable_supervision))
    })?;
    let id = spawn_with_caps_for(plan.image, plan.caps, Some(spawner), plan.spawn_budget)?;
    let handle = with_current_mut(|task| {
        task.caps
            .insert(Capability {
                object: KernelObject::Supervision(id),
                rights: crate::capability::RIGHT_SUPERVISE
                    | if transferable_supervision {
                        RIGHT_TRANSFER
                    } else {
                        0
                    },
            })
            .map_err(|_| SpawnError::BadCapability)
    });
    let handle = match handle {
        Ok(handle) => handle,
        Err(error) => {
            let mut sched = SCHEDULER.lock();
            remove_task(&mut sched, id);
            return Err(error);
        }
    };
    with_current_mut(|task| task.live_children += 1);
    if let Some(name) = plan.name {
        crate::bootstrap::record_spawn(name, id);
    }
    Ok((id, handle))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnError {
    BadExecutable,
    BadCapability,
    /// The executable image does not match the profile this kernel admits.
    BadImage(crate::component::ImageError),
    /// The global live-task table is full.
    TooManyTasks,
    /// This spawner has reached its manifest-declared live-child budget.
    BudgetExhausted,
    Map(MapError),
}

impl From<MapError> for SpawnError {
    fn from(error: MapError) -> Self {
        SpawnError::Map(error)
    }
}

pub fn spawn_with_caps(image: &[u8], caps: Vec<Capability>) -> Result<TaskId, SpawnError> {
    spawn_with_caps_for(image, caps, None, DEFAULT_SPAWN_BUDGET)
}

pub fn spawn_with_caps_for(
    image: &[u8],
    caps: Vec<Capability>,
    spawner: Option<TaskId>,
    spawn_budget: u16,
) -> Result<TaskId, SpawnError> {
    {
        let sched = SCHEDULER.lock();
        let live = sched
            .tasks
            .iter()
            .filter(|task| !matches!(task.state, TaskState::Terminated(_)))
            .count();
        if live >= MAX_TASKS {
            return Err(SpawnError::TooManyTasks);
        }
    }

    let profile = TargetProfile::current().map_err(|_| SpawnError::BadExecutable)?;
    let decoded =
        crate::component::decode_for_profile(image, profile).map_err(SpawnError::BadImage)?;

    let mut address_space = AddressSpace::new()?;

    for segment in &decoded.segments {
        let bytes = decoded.segment_bytes(segment);
        let mut flags = PTE_USER | PTE_PRESENT;
        if segment.writable() {
            flags |= PTE_WRITABLE;
        }
        if !segment.executable() {
            flags |= PTE_NO_EXECUTE;
        }
        let pages = (segment.mem_len as usize).div_ceil(PAGE_SIZE);
        for i in 0..pages {
            let frame = FRAME_ALLOCATOR
                .lock()
                .alloc()
                .ok_or(MapError::OutOfFrames)?;
            // SAFETY: `frame` is fresh and HHDM mapped. The frame is zeroed
            // first, so the `mem_len` tail beyond `file_len` reads as zero
            // (`.bss`).
            unsafe {
                let dst = frame.to_virt().as_mut_ptr::<u8>();
                core::ptr::write_bytes(dst, 0, PAGE_SIZE);
                let start = i * PAGE_SIZE;
                if start < bytes.len() {
                    let end = (start + PAGE_SIZE).min(bytes.len());
                    core::ptr::copy_nonoverlapping(bytes[start..end].as_ptr(), dst, end - start);
                }
            }
            address_space.map_user(
                VirtAddr(ENTRY_VA + segment.vaddr_offset as u64 + (i * PAGE_SIZE) as u64),
                frame,
                flags,
            )?;
        }
    }

    let stack_pages = decoded.stack_bytes as usize / PAGE_SIZE;
    for i in 0..stack_pages {
        let frame = FRAME_ALLOCATOR
            .lock()
            .alloc()
            .ok_or(MapError::OutOfFrames)?;
        // SAFETY: `frame` is fresh and HHDM mapped.
        unsafe {
            core::ptr::write_bytes(frame.to_virt().as_mut_ptr::<u8>(), 0, PAGE_SIZE);
        }
        let va = USER_STACK_TOP - ((i + 1) * PAGE_SIZE) as u64;
        address_space.map_user(
            VirtAddr(va),
            frame,
            PTE_USER | PTE_PRESENT | PTE_WRITABLE | PTE_NO_EXECUTE,
        )?;
    }

    let mut cap_table = CapabilityTable::new();
    for cap in caps {
        cap_table
            .insert(cap)
            .map_err(|_| SpawnError::BadCapability)?;
    }

    let mut sched = SCHEDULER.lock();
    let id = sched.next_id;
    sched.next_id += 1;
    let task = Task {
        id,
        state: TaskState::Ready,
        address_space,
        kernel_stack: vec![0u8; KERNEL_STACK_SIZE].into_boxed_slice(),
        saved: UserFrame {
            rax: 0,
            rbx: 0,
            rcx: 0,
            rdx: 0,
            rsi: 0,
            rdi: 0,
            rbp: 0,
            r8: 0,
            r9: 0,
            r10: 0,
            r11: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rip: ENTRY_VA + decoded.entry_offset as u64,
            cs: USER_CODE_SELECTOR as u64 | 3,
            rflags: 0x200,
            rsp: USER_STACK_TOP - 16,
            ss: USER_DATA_SELECTOR as u64 | 3,
        },
        caps: cap_table,
        spawner,
        spawn_budget: spawn_budget.min(MAX_SPAWN_BUDGET),
        live_children: 0,
        wake_on_terminate: None,
        shared_buffer_quota: HolderQuota::DENY,
    };
    sched.tasks.push(task);
    sched.ready.push_back(id);
    Ok(id)
}

pub fn current_id() -> TaskId {
    SCHEDULER.lock().current.expect("no current task")
}

pub fn set_on_idle(f: extern "C" fn()) {
    SCHEDULER.lock().on_idle = Some(f);
}

pub fn supervision_status(slot: u32) -> Result<Option<TermReason>, crate::capability::CapError> {
    without_interrupts(|| {
        let mut sched = SCHEDULER.lock();
        let current = sched
            .current
            .ok_or(crate::capability::CapError::WrongObject)?;
        let current_index = sched
            .index_of(current)
            .ok_or(crate::capability::CapError::WrongObject)?;
        let child = sched.tasks[current_index]
            .caps
            .get(slot)
            .and_then(|cap| {
                (cap.rights & crate::capability::RIGHT_SUPERVISE != 0)
                    .then_some(&cap.object)
                    .and_then(|object| match object {
                        KernelObject::Supervision(child) => Some(*child),
                        _ => None,
                    })
            })
            .ok_or(crate::capability::CapError::WrongObject)?;
        let reason = sched
            .terminated
            .iter()
            .find(|(task_id, _)| *task_id == child)
            .map(|(_, reason)| *reason);
        if reason.is_some() {
            sched.tasks[current_index].caps.remove(slot).map(|_| ())?;
        }
        Ok(reason)
    })
}

/// A single source a `SYS_WAIT` caller wants to be woken on.
#[derive(Debug, Clone, Copy)]
pub enum WaitSource {
    /// Endpoint capability slot: ready when its receive queue is non-empty or
    /// the peer is gone.
    Endpoint(u32),
    /// Endpoint capability slot: ready when the peer receive queue has room or
    /// the peer is gone.
    SendCapacity(u32),
    /// Keyboard input: ready when a decoded event or scripted byte is pending.
    Input,
    /// Supervision capability slot: ready when the supervised child terminated.
    Supervision(u32),
}

/// Returns the `BlockReason` for the first source, used to label the parked
/// task. Wake logic never reads this discriminant; it only checks `Blocked`.
fn block_reason(sources: &[WaitSource]) -> BlockReason {
    match sources.first() {
        Some(WaitSource::Input) => BlockReason::Input,
        Some(WaitSource::Supervision(_)) => BlockReason::Supervision,
        _ => BlockReason::Endpoint,
    }
}

/// Reports whether a single source is already satisfied for the task at
/// `task_idx`. Endpoint sources clone their capability and probe the channel;
/// supervision sources scan the terminated log; input is a global check.
/// An invalid or missing source is reported ready (lenient): userspace
/// re-polls after the wake and discovers the error through the poll ABI.
fn source_ready(sched: &Scheduler, task_idx: usize, source: WaitSource) -> bool {
    match source {
        WaitSource::Input => crate::input::input_pending(),
        WaitSource::Endpoint(slot) => {
            let Some(cap) = sched.tasks[task_idx].caps.get(slot) else {
                return true;
            };
            if cap.rights & crate::capability::RIGHT_RECV == 0 {
                return true;
            }
            let KernelObject::Endpoint(endpoint) = &cap.object else {
                return true;
            };
            endpoint.has_pending() || endpoint.peer_dead()
        }
        WaitSource::SendCapacity(slot) => {
            let Some(cap) = sched.tasks[task_idx].caps.get(slot) else {
                return true;
            };
            if cap.rights & crate::capability::RIGHT_SEND == 0 {
                return true;
            }
            let KernelObject::Endpoint(endpoint) = &cap.object else {
                return true;
            };
            endpoint.can_send()
        }
        WaitSource::Supervision(slot) => {
            let child = sched.tasks[task_idx].caps.get(slot).and_then(|cap| {
                (cap.rights & crate::capability::RIGHT_SUPERVISE != 0)
                    .then_some(&cap.object)
                    .and_then(|object| match object {
                        KernelObject::Supervision(child) => Some(*child),
                        _ => None,
                    })
            });
            let Some(child) = child else {
                return true;
            };
            sched.terminated.iter().any(|(id, _)| *id == child)
        }
    }
}

/// Registers the current task as the waiter on every requested source, so a
/// later event (peer send, keyboard IRQ, child exit) re-readies it.
fn clear_waiters(sched: &Scheduler, task_idx: usize, current: TaskId) {
    for slot in 0..crate::capability::MAX_CAPS as u32 {
        let Some(cap) = sched.tasks[task_idx].caps.get(slot) else {
            continue;
        };
        if let KernelObject::Endpoint(endpoint) = &cap.object {
            endpoint.clear_send_waiter(current);
        }
    }
}

fn register_waiters(
    sched: &mut Scheduler,
    task_idx: usize,
    current: TaskId,
    sources: &[WaitSource],
) {
    for source in sources {
        match *source {
            WaitSource::Input => crate::input::register_waiter(current),
            WaitSource::Endpoint(slot) => {
                if let Some(cap) = sched.tasks[task_idx].caps.get(slot)
                    && let KernelObject::Endpoint(endpoint) = &cap.object
                {
                    endpoint.register_recv_waiter(current);
                }
            }
            WaitSource::SendCapacity(slot) => {
                if let Some(cap) = sched.tasks[task_idx].caps.get(slot)
                    && cap.rights & crate::capability::RIGHT_SEND != 0
                    && let KernelObject::Endpoint(endpoint) = &cap.object
                {
                    endpoint.register_send_waiter(current);
                }
            }
            WaitSource::Supervision(slot) => {
                let child = sched.tasks[task_idx].caps.get(slot).and_then(|cap| {
                    (cap.rights & crate::capability::RIGHT_SUPERVISE != 0)
                        .then_some(&cap.object)
                        .and_then(|object| match object {
                            KernelObject::Supervision(child) => Some(*child),
                            _ => None,
                        })
                });
                if let Some(child) = child
                    && let Some(child_idx) = sched.index_of(child)
                {
                    sched.tasks[child_idx].wake_on_terminate = Some(current);
                }
            }
        }
    }
}

/// Blocking multi-source wait. Runs with interrupts disabled (syscall gate is
/// an interrupt gate, but `without_interrupts` also covers non-syscall
/// callers). Re-checks readiness of every source before parking to close the
/// lost-wakeup race, then, if none is ready, saves the frame with a `0` return
/// value, marks the task `Blocked`, registers waiters, and schedules another
/// task. If any source is already ready it returns immediately without
/// blocking (the frame keeps its `0` return).
pub fn wait(frame: &mut UserFrame, sources: &[WaitSource]) {
    frame.rax = 0;
    let (result, pml4) = without_interrupts(|| {
        let mut sched = SCHEDULER.lock();
        drain_pending_wakes(&mut sched);
        let Some(current) = sched.current else {
            let result = schedule_next(&mut sched, frame, None);
            let pml4 = selected_pml4(&sched, &result);
            return (result, pml4);
        };
        let Some(idx) = sched.index_of(current) else {
            let result = schedule_next(&mut sched, frame, None);
            let pml4 = selected_pml4(&sched, &result);
            return (result, pml4);
        };
        let ready = sources
            .iter()
            .any(|source| source_ready(&sched, idx, *source));
        if ready {
            // A source became ready (possibly via a wake drained above): do not
            // park. The frame already returns 0; userspace re-polls.
            return (
                ScheduleResult::Selected,
                selected_pml4(&sched, &ScheduleResult::Selected),
            );
        }
        sched.tasks[idx].saved = *frame;
        sched.tasks[idx].state = TaskState::Blocked(block_reason(sources));
        clear_waiters(&sched, idx, current);
        register_waiters(&mut sched, idx, current, sources);
        let result = schedule_next(&mut sched, frame, Some(current));
        let pml4 = selected_pml4(&sched, &result);
        (result, pml4)
    });
    finish_schedule(result, pml4, frame);
}

pub fn termination_summary(id: TaskId) -> Option<TermReason> {
    SCHEDULER
        .lock()
        .terminated
        .iter()
        .find(|(task_id, _)| *task_id == id)
        .map(|(_, reason)| *reason)
}

pub fn with_current_mut<R>(f: impl FnOnce(&mut Task) -> R) -> R {
    without_interrupts(|| {
        let mut sched = SCHEDULER.lock();
        let id = sched.current.expect("no current task");
        let idx = sched.index_of(id).expect("current task missing");
        f(&mut sched.tasks[idx])
    })
}

/// Install a holder's generation-declared shared-buffer quota (C7.3). Called by
/// the bootstrap launcher after a component is spawned; a component whose
/// generation declares no budget keeps the deny-by-default [`HolderQuota::DENY`].
pub fn set_shared_buffer_quota(id: TaskId, quota: HolderQuota) -> bool {
    without_interrupts(|| {
        let mut sched = SCHEDULER.lock();
        if let Some(idx) = sched.index_of(id) {
            sched.tasks[idx].shared_buffer_quota = quota;
            true
        } else {
            false
        }
    })
}

pub(crate) fn without_interrupts<T>(f: impl FnOnce() -> T) -> T {
    let flags: u64;
    unsafe {
        core::arch::asm!("pushfq", "pop {}", out(reg) flags, options(nomem, preserves_flags));
        core::arch::asm!("cli", options(nomem, nostack, preserves_flags));
    }
    let result = f();
    if flags & (1 << 9) != 0 {
        unsafe { core::arch::asm!("sti", options(nomem, nostack, preserves_flags)) };
    }
    result
}

/// Copy bytes from the current task's mapped user address without switching
/// address spaces or holding the scheduler lock during the copy.
pub fn copy_from_current(addr: u64, destination: &mut [u8]) -> bool {
    if destination.is_empty() {
        return true;
    }
    // Capture the current task's page-table root under the scheduler lock, then
    // translate and copy without holding it. Syscalls run with interrupts
    // disabled on this uniprocessor, so the task cannot be preempted and its
    // user page tables are stable for the duration of the copy. Translating
    // each byte's address as we go imposes no fixed length bound; the caller is
    // responsible for sizing `destination` (a fixed protocol buffer, or up to
    // `MAX_CAPS` spawn grants). The former per-byte scratch array capped this
    // at 64 bytes, which the `u64`-rights `SpawnGrant` widening overran.
    let pml4 = {
        let sched = SCHEDULER.lock();
        let Some(id) = sched.current else {
            return false;
        };
        let Some(index) = sched.index_of(id) else {
            return false;
        };
        sched.tasks[index].address_space.pml4()
    };
    for (offset, destination) in destination.iter_mut().enumerate() {
        let Some(address) = addr.checked_add(offset as u64) else {
            return false;
        };
        let Some(translated) =
            crate::memory::vmm::translate_in(pml4, crate::memory::VirtAddr(address))
        else {
            return false;
        };
        // SAFETY: translation proved this physical byte is mapped by the
        // current task; HHDM provides a stable kernel alias.
        *destination = unsafe { translated.to_virt().as_mut_ptr::<u8>().read() };
    }
    true
}

enum ScheduleResult {
    Selected,
    Idle(extern "C" fn()),
    Halt,
}

pub fn yield_now(frame: &mut UserFrame) {
    let (result, pml4) = {
        let mut sched = SCHEDULER.lock();
        let executing = if let Some(id) = sched.current
            && let Some(idx) = sched.index_of(id)
        {
            sched.tasks[idx].saved = *frame;
            sched.tasks[idx].state = TaskState::Ready;
            sched.ready.push_back(id);
            sched.current = None;
            Some(id)
        } else {
            None
        };
        let result = schedule_next(&mut sched, frame, executing);
        let pml4 = selected_pml4(&sched, &result);
        (result, pml4)
    };
    finish_schedule(result, pml4, frame);
}

pub fn terminate(frame: &mut UserFrame, reason: TermReason) {
    let (result, pml4) = {
        let mut sched = SCHEDULER.lock();
        if let Some(id) = sched.current
            && let Some(idx) = sched.index_of(id)
        {
            sched.tasks[idx].state = TaskState::Terminated(reason);
            sched.tasks[idx].saved = *frame;
            // Dropping the drained values now, rather than at scope exit after
            // scheduling, makes endpoint peer death observable before any
            // supervisor or broker awakened by this termination runs.
            let drained = sched.tasks[idx].caps.drain();
            drop(drained);
            // Reclaim every shared buffer charged to this holder's account.
            // Peer death, supervised restart, and revocation all funnel through
            // task termination, so this one call restores the subtree's pages
            // and charges without disturbing any other owner's account.
            SHARED_BUFFER_TABLE.lock().reclaim_owner(id);

            sched.terminated.push((id, reason));
            // A parent parked in `SYS_WAIT` on this child's supervision slot is
            // re-readied. `wake` only enqueues; `schedule_next` (next line)
            // drains it under this same lock.
            if let Some(parent) = sched.tasks[idx].wake_on_terminate.take() {
                wake(parent);
            }
            let spawner = sched.tasks[idx].spawner;
            if let Some(spawner) = spawner
                && let Some(parent_idx) = sched.index_of(spawner)
            {
                sched.tasks[parent_idx].live_children =
                    sched.tasks[parent_idx].live_children.saturating_sub(1);
            }
        }
        let executing = sched.current;
        let result = schedule_next(&mut sched, frame, executing);
        let pml4 = selected_pml4(&sched, &result);
        (result, pml4)
    };
    finish_schedule(result, pml4, frame);
}

fn selected_pml4(sched: &Scheduler, result: &ScheduleResult) -> Option<u64> {
    if !matches!(result, ScheduleResult::Selected) {
        return None;
    }
    let id = sched.current.expect("selected task missing");
    let index = sched.index_of(id).expect("selected task absent");
    Some(sched.tasks[index].address_space.pml4().0)
}

fn finish_schedule(result: ScheduleResult, pml4: Option<u64>, frame: &UserFrame) {
    match result {
        ScheduleResult::Selected => unsafe {
            switch_address_space_and_user(pml4.expect("selected address space missing"), frame)
        },
        ScheduleResult::Idle(on_idle) => on_idle(),
        ScheduleResult::Halt => crate::hlt_loop(),
    }
}

/// Selects the next runnable task from the ready queue, loading its saved
/// frame and marking it `Running`. Returns its address space, or `None` when
/// no task is runnable. Skips terminated ids left in the queue.
fn pop_ready(sched: &mut Scheduler, frame: &mut UserFrame) -> Option<u64> {
    while let Some(id) = sched.ready.pop_front() {
        let Some(idx) = sched.index_of(id) else {
            continue;
        };
        if !matches!(sched.tasks[idx].state, TaskState::Ready) {
            continue;
        }
        sched.tasks[idx].state = TaskState::Running;
        sched.current = Some(id);
        crate::gdt::set_rsp0(sched.tasks[idx].kernel_stack_top());
        *frame = sched.tasks[idx].saved;
        return Some(sched.tasks[idx].address_space.pml4().0);
    }
    None
}

fn schedule_next(
    sched: &mut Scheduler,
    frame: &mut UserFrame,
    executing: Option<TaskId>,
) -> ScheduleResult {
    drain_pending_wakes(sched);
    let selected = pop_ready(sched, frame);
    // Reap after choosing: every terminated task other than the one we are
    // standing on is dead weight, and its image pages, stack pages, and
    // user-half tables return here (B9). A task that terminates last is
    // released by the next scheduling event rather than this one — a constant
    // one-task lag, not growth.
    reap_terminated(sched, executing);
    if selected.is_some() {
        return ScheduleResult::Selected;
    }
    sched.current = None;
    sched
        .on_idle
        .map_or(ScheduleResult::Halt, ScheduleResult::Idle)
}

/// Interactive idle path. Unlike the default `on_idle` (which exits QEMU when
/// the graph is healthy-idle), an interactive session must keep running so a
/// human keystroke can wake the blocked REPL. Parks the CPU with an atomic
/// `sti; hlt` until a wake source re-readies a task, then switches into it.
/// Never returns: it either enters a task or halts waiting for the next IRQ.
pub fn idle_dispatch() -> ! {
    loop {
        // Inspect the scheduler with interrupts disabled so a keyboard IRQ
        // cannot slip a wake between the readiness check and `hlt`.
        unsafe { core::arch::asm!("cli", options(nomem, nostack, preserves_flags)) };
        let selected = {
            let mut sched = SCHEDULER.lock();
            let mut frame = zeroed_frame();
            pop_ready_draining(&mut sched, &mut frame).map(|pml4| (frame, pml4))
        };
        match selected {
            // `iretq` inside the switch restores the task's rflags (IF set).
            Some((frame, pml4)) => unsafe { switch_address_space_and_user(pml4, &frame) },
            // Atomic on x86: a pending interrupt is delivered only after `hlt`
            // begins waiting, so no wake is lost. The handler returns here and
            // the loop re-checks under `cli`.
            None => unsafe {
                core::arch::asm!("sti; hlt", options(nomem, nostack, preserves_flags))
            },
        }
    }
}

/// Idle-loop counterpart to [`pop_ready`], draining wakes first.
///
/// Deliberately does **not** reap. `idle_dispatch` is reached from `on_idle`,
/// which `finish_schedule` calls while still executing on the last task's
/// kernel stack with its PML4 in CR3 — including when that task is the one that
/// just terminated. Freeing it here would pull the running stack and address
/// space out from under this very loop. The frames come back at the next real
/// scheduling event instead, which is the same one-task lag `schedule_next`
/// already accepts (B9).
fn pop_ready_draining(sched: &mut Scheduler, frame: &mut UserFrame) -> Option<u64> {
    drain_pending_wakes(sched);
    pop_ready(sched, frame)
}

fn zeroed_frame() -> UserFrame {
    UserFrame {
        rax: 0,
        rbx: 0,
        rcx: 0,
        rdx: 0,
        rsi: 0,
        rdi: 0,
        rbp: 0,
        r8: 0,
        r9: 0,
        r10: 0,
        r11: 0,
        r12: 0,
        r13: 0,
        r14: 0,
        r15: 0,
        rip: 0,
        cs: 0,
        rflags: 0,
        rsp: 0,
        ss: 0,
    }
}

pub fn run() -> ! {
    let (frame, pml4) = {
        let mut sched = SCHEDULER.lock();
        let mut frame = zeroed_frame();
        match schedule_next(&mut sched, &mut frame, None) {
            ScheduleResult::Selected => {}
            ScheduleResult::Idle(on_idle) => {
                drop(sched);
                on_idle();
                crate::hlt_loop();
            }
            ScheduleResult::Halt => crate::hlt_loop(),
        }
        let id = sched.current.expect("selected task missing");
        let index = sched.index_of(id).expect("selected task absent");
        (frame, sched.tasks[index].address_space.pml4())
    };

    unsafe { switch_address_space_and_user(pml4.0, &frame) }
}
