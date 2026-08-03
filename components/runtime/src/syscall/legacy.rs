//! Legacy trap transport: the custom Slime kernel.
//!
//! Every operation is a direct trap into `kernel/src/syscall/mod.rs`, with byte
//! buffers passed as pointers into the caller's own mapped memory. Only the
//! trap instruction and the register mapping differ per architecture; that
//! lives in [`super::arch`], selected by `cfg(target_arch)`.
//!
//! This backend serves the frozen oracle build. It disappears with the custom
//! kernel once parent integration removes it; the seL4 backend in
//! [`super::sel4`] is the one that survives.

use super::{
    ERR_INVALID_ARG, ERR_SUCCESS, MAX_CAPS_PER_MSG, MAX_DIRECTORY_PATH, MAX_MSG, MAX_WAIT_SOURCES,
    MIN_TRANSFER_WINDOW, SYS_BLOCK_TRANSACT, SYS_CAP_DROP, SYS_CAP_TRANSFER, SYS_DEBUG_WRITE,
    SYS_DIRECTORY_COMMIT, SYS_DIRECTORY_DERIVE, SYS_DIRECTORY_INSPECT, SYS_ENDPOINT_CREATE,
    SYS_EXIT, SYS_GENERATION_RECEIVE, SYS_GENERATION_TRANSACT, SYS_HEALTH_CONFIRM, SYS_INPUT_READ,
    SYS_RECOVERY_RECONSTRUCT, SYS_RECV, SYS_SEND, SYS_SHARED_BUFFER_CREATE, SYS_SHARED_BUFFER_LOAN,
    SYS_SHARED_BUFFER_LOAN_MAP, SYS_SHARED_BUFFER_MAP, SYS_SHARED_BUFFER_RELEASE,
    SYS_SHARED_BUFFER_RETURN, SYS_SHARED_BUFFER_REVOKE, SYS_SHARED_BUFFER_SEAL,
    SYS_SHARED_BUFFER_UNMAP, SYS_SPAWN, SYS_STORE_TRANSACT, SYS_SUPERVISION_STATUS, SYS_UNHEALTHY,
    SYS_WAIT, SYS_YIELD, SpawnGrant, WaitSource,
};
use crate::arch::{raw_syscall, raw_syscall_pair};

/// The trap ABI addresses caller memory directly, so no window is needed. The
/// bounds are still checked, so a component that binds a window builds the same
/// way against either backend.
pub fn transfer_window_bind(base: u64, len: usize) -> i64 {
    if base == 0 || len < MIN_TRANSFER_WINDOW {
        ERR_INVALID_ARG
    } else {
        ERR_SUCCESS
    }
}

pub fn yield_now() {
    unsafe {
        raw_syscall(SYS_YIELD, 0, 0, 0, 0, 0);
    }
}

pub fn wait(sources: &[WaitSource]) {
    let mut descriptors = [0u64; MAX_WAIT_SOURCES];
    let count = sources.len().min(MAX_WAIT_SOURCES);
    for (slot, source) in descriptors.iter_mut().zip(sources.iter()) {
        *slot = source.descriptor();
    }
    unsafe {
        raw_syscall(SYS_WAIT, descriptors.as_ptr() as u64, count as u64, 0, 0, 0);
    }
}

pub fn send(slot: u32, payload: &[u8], caps: &[u32]) -> i64 {
    unsafe {
        raw_syscall(
            SYS_SEND,
            slot as u64,
            payload.as_ptr() as u64,
            payload.len() as u64,
            caps.as_ptr() as u64,
            caps.len() as u64,
        )
    }
}

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

pub fn spawn(executable_slot: u32, grants: &[SpawnGrant]) -> (i64, u64) {
    unsafe {
        raw_syscall_pair(
            SYS_SPAWN,
            executable_slot as u64,
            grants.as_ptr() as u64,
            grants.len() as u64,
            0,
            0,
        )
    }
}

pub fn endpoint_create(factory_slot: u32) -> (i64, u64) {
    unsafe { raw_syscall_pair(SYS_ENDPOINT_CREATE, factory_slot as u64, 0, 0, 0, 0) }
}

pub fn cap_transfer(endpoint_slot: u32, capability_slot: u32, descriptor: &[u8; 64]) -> i64 {
    unsafe {
        raw_syscall(
            SYS_CAP_TRANSFER,
            endpoint_slot as u64,
            capability_slot as u64,
            descriptor.as_ptr() as u64,
            0,
            0,
        )
    }
}

pub fn shared_buffer_create(factory_slot: u32, pages: usize, writable: bool) -> (i64, u64) {
    unsafe {
        raw_syscall_pair(
            SYS_SHARED_BUFFER_CREATE,
            factory_slot as u64,
            pages as u64,
            u64::from(writable),
            0,
            0,
        )
    }
}

pub fn shared_buffer_release(slot: u32) -> i64 {
    unsafe { raw_syscall(SYS_SHARED_BUFFER_RELEASE, slot as u64, 0, 0, 0, 0) }
}

pub fn shared_buffer_map(slot: u32, base: u64, offset: u64, length: u64, writable: bool) -> i64 {
    unsafe {
        raw_syscall(
            SYS_SHARED_BUFFER_MAP,
            slot as u64,
            base,
            offset,
            length,
            u64::from(writable),
        )
    }
}

pub fn shared_buffer_unmap(slot: u32, base: u64) -> i64 {
    unsafe { raw_syscall(SYS_SHARED_BUFFER_UNMAP, slot as u64, base, 0, 0, 0) }
}

pub fn shared_buffer_seal(slot: u32) -> i64 {
    unsafe { raw_syscall(SYS_SHARED_BUFFER_SEAL, slot as u64, 0, 0, 0, 0) }
}

pub fn shared_buffer_loan(
    buffer_slot: u32,
    receiver_slot: u32,
    offset: u64,
    length: u64,
) -> (i64, u64) {
    unsafe {
        raw_syscall_pair(
            SYS_SHARED_BUFFER_LOAN,
            buffer_slot as u64,
            receiver_slot as u64,
            offset,
            length,
            0,
        )
    }
}

pub fn shared_buffer_loan_map(loan_slot: u32, base: u64, offset: u64, length: u64) -> i64 {
    unsafe {
        raw_syscall(
            SYS_SHARED_BUFFER_LOAN_MAP,
            loan_slot as u64,
            base,
            offset,
            length,
            0,
        )
    }
}

pub fn shared_buffer_return(loan_slot: u32) -> i64 {
    unsafe { raw_syscall(SYS_SHARED_BUFFER_RETURN, loan_slot as u64, 0, 0, 0, 0) }
}

pub fn shared_buffer_revoke(buffer_slot: u32, loan_id: u64) -> i64 {
    unsafe {
        raw_syscall(
            SYS_SHARED_BUFFER_REVOKE,
            buffer_slot as u64,
            loan_id,
            0,
            0,
            0,
        )
    }
}

pub fn supervision_status(slot: u32) -> (i64, u64) {
    unsafe { raw_syscall_pair(SYS_SUPERVISION_STATUS, slot as u64, 0, 0, 0, 0) }
}

pub fn cap_drop(slot: u32) -> i64 {
    unsafe { raw_syscall(SYS_CAP_DROP, slot as u64, 0, 0, 0, 0) }
}

pub fn directory_inspect(
    slot: u32,
    required_rights: u32,
    root: &mut [u8; 32],
    scope: &mut [u8; MAX_DIRECTORY_PATH],
) -> i64 {
    unsafe {
        raw_syscall(
            SYS_DIRECTORY_INSPECT,
            slot as u64,
            required_rights as u64,
            root.as_mut_ptr() as u64,
            scope.as_mut_ptr() as u64,
            0,
        )
    }
}

pub fn directory_derive(slot: u32, relative: &[u8], rights: u32) -> i64 {
    unsafe {
        raw_syscall(
            SYS_DIRECTORY_DERIVE,
            slot as u64,
            relative.as_ptr() as u64,
            relative.len() as u64,
            rights as u64,
            0,
        )
    }
}

pub fn directory_commit(slot: u32, expected: &[u8; 32], new: &[u8; 32]) -> i64 {
    unsafe {
        raw_syscall(
            SYS_DIRECTORY_COMMIT,
            slot as u64,
            expected.as_ptr() as u64,
            new.as_ptr() as u64,
            0,
            0,
        )
    }
}

pub fn input_read(slot: u32) -> (i64, u64) {
    unsafe { raw_syscall_pair(SYS_INPUT_READ, slot as u64, 0, 0, 0, 0) }
}

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

pub fn generation_transact(slot: u32, request: &[u8; 64], reply: &mut [u8; 64]) -> i64 {
    unsafe {
        raw_syscall(
            SYS_GENERATION_TRANSACT,
            slot as u64,
            request.as_ptr() as u64,
            reply.as_mut_ptr() as u64,
            0,
            0,
        )
    }
}

pub fn health_confirm(slot: u32) -> i64 {
    unsafe { raw_syscall(SYS_HEALTH_CONFIRM, slot as u64, 0, 0, 0, 0) }
}

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

pub fn generation_receive(receiver_slot: u32, transfer_slot: u32) -> i64 {
    unsafe {
        raw_syscall(
            SYS_GENERATION_RECEIVE,
            receiver_slot as u64,
            transfer_slot as u64,
            0,
            0,
            0,
        )
    }
}

pub fn unhealthy() -> ! {
    unsafe {
        raw_syscall(SYS_UNHEALTHY, 0, 0, 0, 0, 0);
    }
    loop {
        core::hint::spin_loop();
    }
}
