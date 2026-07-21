//! Syscall ABI wrappers (`kernel/src/syscall/mod.rs` is authoritative).
//!
//! Trap: `int 0x80`. Number in `rax`; arguments in `rdi, rsi, rdx, r10, r8`;
//! return in `rax` as `i64` (negative = error). The kernel's trap handler
//! saves and restores every general-purpose register across the trap, so no
//! register beyond `rax` needs to be marked clobbered here.

const SYS_YIELD: u64 = 0;
const SYS_SEND: u64 = 1;
const SYS_RECV: u64 = 2;
const SYS_EXIT: u64 = 3;
const SYS_SPAWN: u64 = 4;
const SYS_DEBUG_WRITE: u64 = 5;
const SYS_BLOCK_TRANSACT: u64 = 6;
const SYS_STORE_TRANSACT: u64 = 7;
const SYS_HEALTH_CONFIRM: u64 = 8;
const SYS_UNHEALTHY: u64 = 9;
const SYS_RECOVERY_RECONSTRUCT: u64 = 10;

pub const ERR_SUCCESS: i64 = 0;
pub const ERR_BAD_CAP: i64 = -1;
pub const ERR_PEER_DEAD: i64 = -2;
pub const ERR_WOULDBLOCK: i64 = -3;
pub const ERR_INVALID_ARG: i64 = -4;
pub const ERR_OUT_OF_MEMORY: i64 = -5;

pub const MAX_MSG: usize = 64;
pub const MAX_CAPS_PER_MSG: usize = 4;

#[inline(always)]
unsafe fn raw_syscall(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> i64 {
    let ret: i64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") nr => ret,
            in("rdi") a1,
            in("rsi") a2,
            in("rdx") a3,
            in("r10") a4,
            in("r8") a5,
            options(nostack),
        );
    }
    ret
}

pub fn yield_now() {
    unsafe {
        raw_syscall(SYS_YIELD, 0, 0, 0, 0, 0);
    }
}

/// Sends `payload` (at most [`MAX_MSG`] bytes) over the endpoint in
/// capability slot `slot`, transferring the capabilities named in `caps`
/// (at most [`MAX_CAPS_PER_MSG`]).
pub fn send(slot: u32, payload: &[u8], caps: &[u32]) -> i64 {
    unsafe {
        raw_syscall(
            SYS_SEND,
            slot as u64,
            payload.as_ptr() as u64,
            payload.len() as u64,
            caps.len() as u64,
            caps.as_ptr() as u64,
        )
    }
}

/// Receives into `buf` (must be [`MAX_MSG`] bytes) and `cap_out` (must be
/// [`MAX_CAPS_PER_MSG`] entries) from the endpoint in capability slot `slot`.
/// Returns the received byte count, or a negative error.
pub fn recv(slot: u32, buf: &mut [u8; MAX_MSG], cap_out: &mut [u64; MAX_CAPS_PER_MSG]) -> i64 {
    unsafe {
        raw_syscall(
            SYS_RECV,
            slot as u64,
            buf.as_mut_ptr() as u64,
            cap_out.as_mut_ptr() as u64,
            0,
            0,
        )
    }
}

pub fn exit(status: i64) -> ! {
    unsafe {
        raw_syscall(SYS_EXIT, status as u64, 0, 0, 0, 0);
    }
    loop {
        core::hint::spin_loop();
    }
}

/// Spawns the executable held in capability slot `executable_slot`, granting
/// it the capabilities in `cap_slots`, and records it under `name_id` (see
/// `component_name_from_id` in `kernel/src/syscall/mod.rs`). Returns the new
/// task id, or a negative error.
pub fn spawn(executable_slot: u32, cap_slots: &[u32], name_id: u64) -> i64 {
    unsafe {
        raw_syscall(
            SYS_SPAWN,
            executable_slot as u64,
            cap_slots.as_ptr() as u64,
            cap_slots.len() as u64,
            name_id,
            0,
        )
    }
}

/// Writes `bytes` to the kernel debug/serial log. Returns the byte count
/// written.
pub fn debug_write(bytes: &[u8]) -> i64 {
    unsafe {
        raw_syscall(
            SYS_DEBUG_WRITE,
            bytes.as_ptr() as u64,
            bytes.len() as u64,
            0,
            0,
            0,
        )
    }
}

/// Issues a 64-byte block-protocol request/reply pair against the block
/// device capability in slot `slot`. A non-negative return means the
/// transaction was delivered; the block-protocol outcome is in `reply`
/// (`OFF_REPLY_STATUS`), not in the syscall return value.
pub fn block_transact(slot: u32, request: &[u8; 64], reply: &mut [u8; 64]) -> i64 {
    unsafe {
        raw_syscall(
            SYS_BLOCK_TRANSACT,
            slot as u64,
            request.as_ptr() as u64,
            reply.as_mut_ptr() as u64,
            0,
            0,
        )
    }
}

/// Issues a 64-byte store-protocol request/reply pair against the object
/// store capability in slot `slot`. Same delivered-vs-outcome distinction as
/// [`block_transact`].
pub fn store_transact(slot: u32, request: &[u8; 64], reply: &mut [u8; 64]) -> i64 {
    unsafe {
        raw_syscall(
            SYS_STORE_TRANSACT,
            slot as u64,
            request.as_ptr() as u64,
            reply.as_mut_ptr() as u64,
            0,
            0,
        )
    }
}

/// Confirms the currently running pending generation using the
/// `GenerationControl` capability in `slot`.
pub fn health_confirm(slot: u32) -> i64 {
    unsafe { raw_syscall(SYS_HEALTH_CONFIRM, slot as u64, 0, 0, 0, 0) }
}

/// Scrubs and reconstructs BootState on the explicitly granted repair target.
pub fn recovery_reconstruct(generation_control_slot: u32, block_slot: u32, flags: u32) -> i64 {
    unsafe {
        raw_syscall(
            SYS_RECOVERY_RECONSTRUCT,
            generation_control_slot as u64,
            block_slot as u64,
            flags as u64,
            0,
            0,
        )
    }
}

/// Terminates the current component with an explicit unhealthy status.
pub fn unhealthy() -> ! {
    unsafe {
        raw_syscall(SYS_UNHEALTHY, 0, 0, 0, 0, 0);
    }
    loop {
        core::hint::spin_loop();
    }
}
