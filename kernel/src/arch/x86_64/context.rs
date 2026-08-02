//! x86-64 privilege transitions: entering and resuming userspace.
//!
//! The scheduler decides *which* task runs and hands this module a saved
//! [`UserFrame`] plus the address-space root to install. Restoring registers,
//! switching CR3, retargeting the ring-0 stack in the TSS, and issuing `iretq`
//! are ISA mechanism and stay here.
//!
//! The frame is addressed by hand-written byte offsets, pinned by the layout
//! assertions in [`super::trap`]. Both stubs restore every general-purpose
//! register from the frame, so a syscall or fault handler's edits to the frame
//! are what the task observes on return.

use core::arch::global_asm;

use super::trap::UserFrame;

/// Bytes of scratch stack the address-space switch runs on.
///
/// The switch cannot keep using the outgoing task's kernel stack: it installs
/// the incoming task's CR3 partway through, after which the outgoing stack's
/// mapping may be gone. It copies the frame onto this neutral stack — which
/// lives in the kernel half, mapped identically in every address space — and
/// resumes from there.
const SWITCH_STACK_SIZE: usize = 4096;

static mut SWITCH_STACK: [u8; SWITCH_STACK_SIZE] = [0; SWITCH_STACK_SIZE];

/// Top of the switch scratch stack. Called from assembly.
extern "C" fn switch_stack_top() -> u64 {
    core::ptr::addr_of_mut!(SWITCH_STACK) as u64 + SWITCH_STACK_SIZE as u64
}

global_asm!(
    r#"
    // switch_to_user(frame): resume `frame` in the current address space.
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
        // Build the iretq frame: SS, RSP, RFLAGS, CS, RIP.
        push qword ptr [rdx+152]
        push qword ptr [rdx+144]
        push qword ptr [rdx+136]
        push qword ptr [rdx+128]
        push qword ptr [rdx+120]
        mov rdi, [rdx+40]
        mov rdx, [rdx+24]
        iretq

    // switch_address_space_and_user(root, frame): install `root`, then resume
    // `frame`. Copies the frame to the shared switch stack first, because
    // `frame` may live in the outgoing address space.
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
        // Reserve one frame ({frame_bytes} bytes) and copy it across.
        sub rax, {frame_bytes}
        mov rdi, rax
        mov rsi, r12
        mov rcx, {frame_words}
        rep movsq
        mov r10, rax
        mov cr3, rbx
        mov rsp, rax
        add rsp, {frame_bytes}
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
    tss_rsp0 = sym super::gdt::rsp0,
    switch_stack_top = sym switch_stack_top,
    frame_bytes = const super::trap::USER_FRAME_BYTES,
    frame_words = const super::trap::USER_FRAME_BYTES / 8,
);

unsafe extern "C" {
    /// Install `root` as the address-space root and resume `frame` at user
    /// privilege. Does not return.
    ///
    /// # Safety
    ///
    /// `root` must name a live top-level table whose kernel half maps this
    /// kernel, and `frame` must be a saved user frame for a task in it.
    pub fn switch_address_space_and_user(root: u64, frame: *const UserFrame) -> !;
}
