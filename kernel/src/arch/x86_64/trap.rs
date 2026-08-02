use core::arch::global_asm;

use crate::interrupts::InterruptDescriptorTable;
use crate::serial_println;
use crate::task::{self, TermReason, UserFaultReason};

/// One past the highest user virtual address: the low half of the 48-bit
/// canonical address space. Neutral syscall argument validation uses this
/// bound rather than an x86 canonical-address rule.
pub const USER_ADDRESS_TOP: u64 = 0x0000_8000_0000_0000;

/// Number of semantic syscall argument registers (`a0`..`a4`).
pub const SYSCALL_ARG_COUNT: usize = 5;

/// The user register state saved on kernel entry and restored on return.
///
/// The field names are the x86-64 registers this profile saves. Neutral code
/// never reads them by name: it goes through the semantic accessors below,
/// which implement the `docs/syscall-abi.md` calling convention for this
/// architecture. Anything reachable from architecture-neutral code must have
/// an accessor here rather than a direct field read.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct UserFrame {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

impl UserFrame {
    /// The requested syscall number (`rax` on this profile).
    pub fn syscall_number(&self) -> u64 {
        self.rax
    }

    /// Semantic syscall argument `index` (0-based), per the x86-64 calling
    /// convention `a0..a4 = rdi, rsi, rdx, r10, r8`.
    ///
    /// # Panics
    ///
    /// Panics if `index >= SYSCALL_ARG_COUNT`. Callers pass a literal position
    /// from the syscall table, so an out-of-range index is a kernel bug rather
    /// than untrusted input.
    pub fn arg(&self, index: usize) -> u64 {
        match index {
            0 => self.rdi,
            1 => self.rsi,
            2 => self.rdx,
            3 => self.r10,
            4 => self.r8,
            _ => panic!("syscall argument index out of range"),
        }
    }

    /// Set the primary syscall return value (`rax`).
    pub fn set_return(&mut self, value: u64) {
        self.rax = value;
    }

    /// Set the auxiliary syscall return value (`rdx`), used by the calls that
    /// `docs/syscall-abi.md` documents as returning a second value.
    pub fn set_aux_return(&mut self, value: u64) {
        self.rdx = value;
    }

    /// The faulting or trapping user instruction pointer, for diagnostics.
    pub fn instruction_pointer(&self) -> u64 {
        self.rip
    }

    /// Whether the frame was saved while executing at user privilege.
    pub fn from_user(&self) -> bool {
        self.cs & 3 == 3
    }

    /// Build the initial frame for a task entering userspace at `entry` with
    /// stack pointer `stack_top`.
    pub fn for_user_entry(entry: u64, stack_top: u64) -> Self {
        Self {
            rip: entry,
            cs: crate::gdt::USER_CODE_SELECTOR as u64 | 3,
            // IF set: the task runs with interrupts enabled so the timer can
            // preempt it.
            rflags: 0x200,
            rsp: stack_top,
            ss: crate::gdt::USER_DATA_SELECTOR as u64 | 3,
            ..Self::zeroed()
        }
    }

    /// An all-zero frame, for a task that has no saved user state yet.
    pub const fn zeroed() -> Self {
        Self {
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
}

global_asm!(
    r#"
    .macro PUSH_GPRS
        push r15
        push r14
        push r13
        push r12
        push r11
        push r10
        push r9
        push r8
        push rbp
        push rdi
        push rsi
        push rdx
        push rcx
        push rbx
        push rax
    .endm

    .macro POP_GPRS
        pop rax
        pop rbx
        pop rcx
        pop rdx
        pop rsi
        pop rdi
        pop rbp
        pop r8
        pop r9
        pop r10
        pop r11
        pop r12
        pop r13
        pop r14
        pop r15
    .endm

    .macro USER_TRAP name, vec
    .global \name
    \name:
        PUSH_GPRS
        mov rdi, \vec
        mov rsi, rsp
        call {trap_dispatch}
        POP_GPRS
        iretq
    .endm

    .macro USER_TRAP_ERR name, vec
    .global \name
    \name:
        add rsp, 8
        PUSH_GPRS
        mov rdi, \vec
        mov rsi, rsp
        call {trap_dispatch}
        POP_GPRS
        iretq
    .endm

    USER_TRAP trap_vec0, 0
    USER_TRAP trap_vec6, 6
    USER_TRAP_ERR trap_vec13, 13
    USER_TRAP_ERR trap_vec14, 14
    USER_TRAP trap_vec80, 0x80
    "#,
    trap_dispatch = sym trap_dispatch,
);

unsafe extern "C" {
    fn trap_vec0();
    fn trap_vec6();
    fn trap_vec13();
    fn trap_vec14();
    fn trap_vec80();
}

pub fn stub_addr(vec: u8) -> usize {
    match vec {
        0 => trap_vec0 as *const () as usize,
        6 => trap_vec6 as *const () as usize,
        13 => trap_vec13 as *const () as usize,
        14 => trap_vec14 as *const () as usize,
        0x80 => trap_vec80 as *const () as usize,
        _ => panic!("unsupported trap vector"),
    }
}

extern "C" fn trap_dispatch(vector: u8, frame: *mut UserFrame) {
    let f = unsafe { &mut *frame };
    if f.from_user() {
        if vector == crate::interrupts::SYSCALL_VECTOR {
            crate::syscall::dispatch(f);
            return;
        }

        let reason = match vector {
            0 => UserFaultReason::DivByZero,
            6 => UserFaultReason::UndefinedOp,
            13 => UserFaultReason::GeneralProt,
            14 => UserFaultReason::PageFault,
            _ => UserFaultReason::Unknown(vector),
        };
        serial_println!(
            "[fault] task {} {:?} rip={:#x}",
            task::current_id(),
            reason,
            f.rip
        );
        task::terminate(f, TermReason::Fault(reason));
        return;
    }

    serial_println!(
        "[kernel fault] vec={} rip={:#x} cs={:#x}",
        vector,
        f.rip,
        f.cs
    );
    crate::hlt_loop();
}

/// Byte size of a saved [`UserFrame`], as the context-switch stubs assume.
pub const USER_FRAME_BYTES: usize = core::mem::size_of::<UserFrame>();

// The `switch_to_user` and `switch_address_space_and_user` stubs in
// `crate::task` address this frame by hand-written byte offsets. Reordering,
// adding, or removing a field silently corrupts every privilege transition, so
// pin the offsets the assembly encodes.
const _: () = {
    assert!(USER_FRAME_BYTES == 160);
    assert!(core::mem::offset_of!(UserFrame, rax) == 0);
    assert!(core::mem::offset_of!(UserFrame, rbx) == 8);
    assert!(core::mem::offset_of!(UserFrame, rcx) == 16);
    assert!(core::mem::offset_of!(UserFrame, rdx) == 24);
    assert!(core::mem::offset_of!(UserFrame, rsi) == 32);
    assert!(core::mem::offset_of!(UserFrame, rdi) == 40);
    assert!(core::mem::offset_of!(UserFrame, rbp) == 48);
    assert!(core::mem::offset_of!(UserFrame, r8) == 56);
    assert!(core::mem::offset_of!(UserFrame, r9) == 64);
    assert!(core::mem::offset_of!(UserFrame, r10) == 72);
    assert!(core::mem::offset_of!(UserFrame, r11) == 80);
    assert!(core::mem::offset_of!(UserFrame, r12) == 88);
    assert!(core::mem::offset_of!(UserFrame, r13) == 96);
    assert!(core::mem::offset_of!(UserFrame, r14) == 104);
    assert!(core::mem::offset_of!(UserFrame, r15) == 112);
    assert!(core::mem::offset_of!(UserFrame, rip) == 120);
    assert!(core::mem::offset_of!(UserFrame, cs) == 128);
    assert!(core::mem::offset_of!(UserFrame, rflags) == 136);
    assert!(core::mem::offset_of!(UserFrame, rsp) == 144);
    assert!(core::mem::offset_of!(UserFrame, ss) == 152);
};

pub fn install(idt: &mut InterruptDescriptorTable) {
    idt.entry(0).set_handler_raw(stub_addr(0), 0x8E);
    idt.entry(6).set_handler_raw(stub_addr(6), 0x8E);
    idt.entry(13).set_handler_raw(stub_addr(13), 0x8E);
    idt.entry(14).set_handler_raw(stub_addr(14), 0x8E);
    idt.entry(0x80).set_handler_raw(stub_addr(0x80), 0xEE);
}
