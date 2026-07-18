use core::arch::global_asm;

use crate::interrupts::InterruptDescriptorTable;
use crate::serial_println;
use crate::task::{self, TermReason, UserFaultReason};

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
    if f.cs & 3 == 3 {
        if vector == 0x80 {
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

pub fn install(idt: &mut InterruptDescriptorTable) {
    idt.entry(0).set_handler_raw(stub_addr(0), 0x8E);
    idt.entry(6).set_handler_raw(stub_addr(6), 0x8E);
    idt.entry(13).set_handler_raw(stub_addr(13), 0x8E);
    idt.entry(14).set_handler_raw(stub_addr(14), 0x8E);
    idt.entry(0x80).set_handler_raw(stub_addr(0x80), 0xEF);
}
