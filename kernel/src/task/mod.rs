use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;
use core::arch::global_asm;
use core::sync::atomic::Ordering;
use spin::{LazyLock, Mutex};

use crate::capability::{Capability, CapabilityTable, KernelObject};
use crate::gdt::{USER_CODE_SELECTOR, USER_DATA_SELECTOR};
use crate::memory::address_space::AddressSpace;
use crate::memory::pmm::FRAME_ALLOCATOR;
use crate::memory::vmm::{MapError, PTE_NO_EXECUTE, PTE_PRESENT, PTE_USER, PTE_WRITABLE};
use crate::memory::{PAGE_SIZE, VirtAddr};
use crate::trap::UserFrame;

pub const KERNEL_STACK_SIZE: usize = 64 * 1024;
/// Hard bound on simultaneously live tasks. Sized against the kernel heap:
/// every task eagerly allocates a [`KERNEL_STACK_SIZE`] stack, and
/// `MAX_TASKS * KERNEL_STACK_SIZE` must stay well within the heap budget.
pub const MAX_TASKS: usize = 16;
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

static SCHEDULER: LazyLock<Mutex<Scheduler>> = LazyLock::new(|| Mutex::new(Scheduler::new()));

global_asm!(
    r#"
    .global switch_to_user
    switch_to_user:
        mov rax, [rdi+0]
        mov rbx, [rdi+8]
        mov rcx, [rdi+16]
        mov rdx, [rdi+24]
        mov rsi, [rdi+32]
        mov rbp, [rdi+48]
        mov r8,  [rdi+56]
        mov r9,  [rdi+64]
        mov r10, [rdi+72]
        mov r11, [rdi+80]
        mov r12, [rdi+88]
        mov r13, [rdi+96]
        mov r14, [rdi+104]
        mov r15, [rdi+112]
        push qword ptr [rdi+152]
        push qword ptr [rdi+144]
        push qword ptr [rdi+136]
        push qword ptr [rdi+128]
        push qword ptr [rdi+120]
        mov rdi, [rdi+40]
        iretq
    "#,
);

unsafe extern "C" {
    fn switch_to_user(frame: *const UserFrame) -> !;
}

pub fn spawn_user(image: &[u8]) -> Result<TaskId, SpawnError> {
    spawn_with_caps(image, Vec::new())
}

/// Validate a spawn grant list against a capability table. The executable
/// slot must name an `Executable` carrying `RIGHT_EXEC`; every granted slot
/// must exist, be unique, differ from the executable slot, and carry
/// `RIGHT_TRANSFER` — the same condition IPC sends enforce, so a capability
/// received without transfer rights cannot be laundered into a spawned
/// component. Pure: takes no scheduler lock and is directly unit-testable.
pub fn preflight_spawn_grant(
    caps: &CapabilityTable,
    executable_slot: u32,
    cap_slots: &[u32],
) -> Result<(&'static [u8], Vec<Capability>), SpawnError> {
    let executable = caps
        .get(executable_slot)
        .filter(|cap| cap.rights & crate::capability::RIGHT_EXEC != 0)
        .and_then(|cap| match cap.object {
            KernelObject::Executable(bytes) => Some(bytes),
            _ => None,
        })
        .ok_or(SpawnError::BadExecutable)?;
    for (index, slot) in cap_slots.iter().enumerate() {
        if *slot == executable_slot || cap_slots[..index].contains(slot) {
            return Err(SpawnError::BadCapability);
        }
        let Some(cap) = caps.get(*slot) else {
            return Err(SpawnError::BadCapability);
        };
        if cap.rights & crate::capability::RIGHT_TRANSFER == 0 {
            return Err(SpawnError::BadCapability);
        }
    }
    let granted = cap_slots
        .iter()
        .map(|slot| {
            caps.get(*slot)
                .expect("capability changed after preflight")
                .clone()
        })
        .collect();
    Ok((executable, granted))
}

pub fn spawn_from_cap(executable_slot: u32, cap_slots: &[u32]) -> Result<TaskId, SpawnError> {
    let (code, caps) =
        with_current_mut(|task| preflight_spawn_grant(&task.caps, executable_slot, cap_slots))?;
    let id = spawn_with_caps(code, caps)?;
    with_current_mut(|task| {
        for slot in cap_slots {
            let _ = task.caps.take(*slot);
        }
    });
    Ok(id)
}

#[derive(Debug)]
pub enum SpawnError {
    BadExecutable,
    BadCapability,
    /// The executable bytes are not a valid component image
    /// (`contracts/component/v1`). Generation decoding validates images
    /// eagerly, so this can only fire for executables sourced outside a
    /// decoded generation.
    BadImage(crate::component::ImageError),
    /// The live task table is at [`MAX_TASKS`]; no new task may be spawned.
    TooManyTasks,
    Map(MapError),
}

impl From<MapError> for SpawnError {
    fn from(error: MapError) -> Self {
        SpawnError::Map(error)
    }
}

pub fn spawn_with_caps(image: &[u8], caps: Vec<Capability>) -> Result<TaskId, SpawnError> {
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
            rsp: USER_STACK_TOP,
            ss: USER_DATA_SELECTOR as u64 | 3,
        },
        caps: cap_table,
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

pub fn termination_summary(id: TaskId) -> Option<TermReason> {
    SCHEDULER
        .lock()
        .terminated
        .iter()
        .find(|(task_id, _)| *task_id == id)
        .map(|(_, reason)| *reason)
}

pub fn with_current_mut<R>(f: impl FnOnce(&mut Task) -> R) -> R {
    let mut sched = SCHEDULER.lock();
    let id = sched.current.expect("no current task");
    let idx = sched.index_of(id).expect("current task missing");
    f(&mut sched.tasks[idx])
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
    let result = {
        let mut sched = SCHEDULER.lock();
        if let Some(id) = sched.current
            && let Some(idx) = sched.index_of(id)
        {
            sched.tasks[idx].saved = *frame;
            sched.tasks[idx].state = TaskState::Ready;
            sched.ready.push_back(id);
        }
        schedule_next(&mut sched, frame)
    };
    finish_schedule(result);
}

pub fn terminate(frame: &mut UserFrame, reason: TermReason) {
    let result = {
        let mut sched = SCHEDULER.lock();
        if let Some(id) = sched.current
            && let Some(idx) = sched.index_of(id)
        {
            sched.tasks[idx].state = TaskState::Terminated(reason);
            sched.tasks[idx].saved = *frame;
            let drained = sched.tasks[idx].caps.drain();
            for cap in drained {
                if let KernelObject::Endpoint(ep) = cap.object {
                    ep.owner_alive.store(false, Ordering::Release);
                }
            }
            sched.terminated.push((id, reason));
        }
        schedule_next(&mut sched, frame)
    };
    finish_schedule(result);
}

fn finish_schedule(result: ScheduleResult) {
    match result {
        ScheduleResult::Selected => {}
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
        sched.tasks[idx].address_space.switch();
        *frame = sched.tasks[idx].saved;
        return ScheduleResult::Selected;
    }
    sched.current = None;
    sched
        .on_idle
        .map_or(ScheduleResult::Halt, ScheduleResult::Idle)
}

pub fn run() -> ! {
    let frame = {
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
        frame
    };

    // SAFETY: `frame` contains a valid ring-3 iret frame for the selected task.
    unsafe { switch_to_user(&frame) }
}
