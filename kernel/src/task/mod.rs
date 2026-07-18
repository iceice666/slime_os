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
pub const USER_STACK_PAGES: usize = 4;
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

pub fn spawn_user(code: &'static [u8]) -> Result<TaskId, MapError> {
    spawn_with_caps(code, Vec::new())
}

pub fn spawn_from_cap(executable_slot: u32, cap_slots: &[u32]) -> Result<TaskId, SpawnError> {
    let (code, caps) = with_current_mut(|task| {
        let executable = task
            .caps
            .get(executable_slot)
            .filter(|cap| cap.rights & crate::capability::RIGHT_EXEC != 0)
            .and_then(|cap| match cap.object {
                KernelObject::Executable(bytes) => Some(bytes),
                KernelObject::Endpoint(_) => None,
            })
            .ok_or(SpawnError::BadExecutable)?;
        for (index, slot) in cap_slots.iter().enumerate() {
            if *slot == executable_slot
                || cap_slots[..index].contains(slot)
                || task.caps.get(*slot).is_none()
            {
                return Err(SpawnError::BadCapability);
            }
        }
        let caps = cap_slots
            .iter()
            .map(|slot| {
                task.caps
                    .get(*slot)
                    .expect("capability changed after preflight")
                    .clone()
            })
            .collect();
        Ok((executable, caps))
    })?;
    let id = spawn_with_caps(code, caps).map_err(SpawnError::Map)?;
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
    Map(MapError),
}

pub fn spawn_with_caps(code: &'static [u8], caps: Vec<Capability>) -> Result<TaskId, MapError> {
    let mut address_space = AddressSpace::new()?;

    let pages = code.len().div_ceil(PAGE_SIZE);
    for i in 0..pages {
        let frame = FRAME_ALLOCATOR
            .lock()
            .alloc()
            .ok_or(MapError::OutOfFrames)?;
        // SAFETY: `frame` is fresh and HHDM mapped.
        unsafe {
            let dst = frame.to_virt().as_mut_ptr::<u8>();
            core::ptr::write_bytes(dst, 0, PAGE_SIZE);
            let start = i * PAGE_SIZE;
            let end = (start + PAGE_SIZE).min(code.len());
            core::ptr::copy_nonoverlapping(code[start..end].as_ptr(), dst, end - start);
        }
        address_space.map_user(
            VirtAddr(ENTRY_VA + (i * PAGE_SIZE) as u64),
            frame,
            PTE_USER | PTE_PRESENT,
        )?;
    }

    for i in 0..USER_STACK_PAGES {
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
        let _ = cap_table.insert(cap);
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
            rip: ENTRY_VA,
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
