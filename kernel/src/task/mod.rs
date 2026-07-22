use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;
use core::arch::global_asm;
use spin::{LazyLock, Mutex};

use crate::capability::{Capability, CapabilityTable, KernelObject, RIGHT_SPAWN, RIGHT_TRANSFER};
use crate::gdt::{USER_CODE_SELECTOR, USER_DATA_SELECTOR};
use crate::memory::address_space::AddressSpace;
use crate::memory::pmm::FRAME_ALLOCATOR;
use crate::memory::vmm::{MapError, PTE_NO_EXECUTE, PTE_PRESENT, PTE_USER, PTE_WRITABLE};
use crate::memory::{PAGE_SIZE, VirtAddr};
use crate::trap::UserFrame;

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
pub enum TaskState {
    Ready,
    Running,
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

fn remove_task(sched: &mut Scheduler, id: TaskId) {
    if let Some(index) = sched.index_of(id) {
        sched.tasks.remove(index);
    }
    sched.ready.retain(|ready| *ready != id);
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
    pub rights: u32,
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
    crate::serial_println!(
        "[spawn-debug] slot={} present={} executable={} rights={:#x}",
        executable_slot,
        caps.get(executable_slot).is_some(),
        caps.get(executable_slot)
            .is_some_and(|cap| matches!(cap.object, KernelObject::Executable { .. })),
        caps.get(executable_slot).map_or(0, |cap| cap.rights),
    );
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
    let (spawner, plan) = with_current_mut(|task| {
        if task.live_children >= task.spawn_budget {
            return Err(SpawnError::BudgetExhausted);
        }
        if task.caps.available_slots() == 0 {
            return Err(SpawnError::BadCapability);
        }
        let plan = preflight_spawn_grant(&task.caps, executable_slot, grants)?;
        Ok((task.id, plan))
    })?;
    let id = spawn_with_caps_for(plan.image, plan.caps, Some(spawner), plan.spawn_budget)?;
    let handle = with_current_mut(|task| {
        task.caps
            .insert(Capability {
                object: KernelObject::Supervision(id),
                rights: crate::capability::RIGHT_SUPERVISE,
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
    /// The executable bytes are not a valid component image
    /// (`contracts/component/v1`). Generation decoding validates images
    /// eagerly, so this can only fire for executables sourced outside a
    /// decoded generation.
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

    let decoded = crate::component::decode(image).map_err(SpawnError::BadImage)?;

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

fn without_interrupts<T>(f: impl FnOnce() -> T) -> T {
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
    let mut physical = [crate::memory::PhysAddr(0); crate::capability::MAX_CAPS];
    if destination.len() > physical.len() {
        return false;
    }
    let copied = {
        let sched = SCHEDULER.lock();
        let Some(id) = sched.current else {
            return false;
        };
        let Some(index) = sched.index_of(id) else {
            return false;
        };
        for (offset, slot) in physical.iter_mut().take(destination.len()).enumerate() {
            let Some(address) = addr.checked_add(offset as u64) else {
                return false;
            };
            let Some(translated) = crate::memory::vmm::translate_in(
                sched.tasks[index].address_space.pml4(),
                crate::memory::VirtAddr(address),
            ) else {
                return false;
            };
            *slot = translated;
        }
        destination.len()
    };
    for (destination, physical) in destination.iter_mut().zip(physical).take(copied) {
        // SAFETY: the scheduler lookup proved this physical byte is mapped by
        // the current task; HHDM provides a stable kernel alias.
        *destination = unsafe { physical.to_virt().as_mut_ptr::<u8>().read() };
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
        if let Some(id) = sched.current
            && let Some(idx) = sched.index_of(id)
        {
            sched.tasks[idx].saved = *frame;
            sched.tasks[idx].state = TaskState::Ready;
            sched.ready.push_back(id);
        }
        let result = schedule_next(&mut sched, frame);
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
            let _drained = sched.tasks[idx].caps.drain();

            sched.terminated.push((id, reason));
            let spawner = sched.tasks[idx].spawner;
            if let Some(spawner) = spawner
                && let Some(parent_idx) = sched.index_of(spawner)
            {
                sched.tasks[parent_idx].live_children =
                    sched.tasks[parent_idx].live_children.saturating_sub(1);
            }
        }
        let result = schedule_next(&mut sched, frame);
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

fn schedule_next(sched: &mut Scheduler, frame: &mut UserFrame) -> ScheduleResult {
    while let Some(id) = sched.ready.pop_front() {
        let Some(idx) = sched.index_of(id) else {
            continue;
        };
        if matches!(sched.tasks[idx].state, TaskState::Terminated(_)) {
            continue;
        }
        sched.tasks[idx].state = TaskState::Running;
        sched.current = Some(id);
        crate::gdt::set_rsp0(sched.tasks[idx].kernel_stack_top());
        *frame = sched.tasks[idx].saved;
        return ScheduleResult::Selected;
    }
    sched.current = None;
    sched
        .on_idle
        .map_or(ScheduleResult::Halt, ScheduleResult::Idle)
}

pub fn run() -> ! {
    let (frame, pml4) = {
        let mut sched = SCHEDULER.lock();
        let mut frame = UserFrame {
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
        };
        match schedule_next(&mut sched, &mut frame) {
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
