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
const SYS_ENDPOINT_CREATE: u64 = 11;
const SYS_SUPERVISION_STATUS: u64 = 12;
const SYS_CAP_DROP: u64 = 13;
const SYS_DIRECTORY_INSPECT: u64 = 14;
const SYS_DIRECTORY_DERIVE: u64 = 15;
const SYS_DIRECTORY_COMMIT: u64 = 16;
const SYS_INPUT_READ: u64 = 17;

const SYS_GENERATION_TRANSACT: u64 = 18;
pub const SYS_GENERATION_RECEIVE: u64 = 19;

pub const ERR_SUCCESS: i64 = 0;
pub const ERR_BAD_CAP: i64 = -1;
pub const ERR_PEER_DEAD: i64 = -2;
pub const ERR_WOULDBLOCK: i64 = -3;
pub const ERR_INVALID_ARG: i64 = -4;
pub const ERR_OUT_OF_MEMORY: i64 = -5;

pub const MAX_MSG: usize = 64;
pub const MAX_CAPS_PER_MSG: usize = 4;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SpawnGrant {
    pub slot: u32,
    pub rights: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spawned {
    pub task_id: u64,
    pub supervision_slot: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Termination {
    Exit(i64),
    Fault(u64),
    Timeout,
    PeerLoss,
    Unhealthy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKey {
    Escape,
    Backspace,
    Tab,
    Enter,
    LeftControl,
    LeftShift,
    RightShift,
    LeftAlt,
    Space,
    Up,
    Down,
    Left,
    Right,
    Character(char),
    Unknown(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputEvent {
    pub key: InputKey,
    pub pressed: bool,
}

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

#[inline(always)]
unsafe fn raw_syscall_pair(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> (i64, u64) {
    let ret: i64;
    let aux: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") nr => ret,
            in("rdi") a1,
            in("rsi") a2,
            inlateout("rdx") a3 => aux,
            in("r10") a4,
            in("r8") a5,
            options(nostack),
        );
    }
    (ret, aux)
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
            caps.as_ptr() as u64,
            caps.len() as u64,
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

/// Spawns the executable in `executable_slot`. Each grant is a non-consuming
/// narrow copy; the source capability remains in the spawner. Success returns
/// both the child task id and a supervision capability slot.
pub fn spawn(executable_slot: u32, grants: &[SpawnGrant]) -> Result<Spawned, i64> {
    let (task_id, supervision_slot) = unsafe {
        raw_syscall_pair(
            SYS_SPAWN,
            executable_slot as u64,
            grants.as_ptr() as u64,
            grants.len() as u64,
            0,
            0,
        )
    };
    if task_id < 0 {
        Err(task_id)
    } else {
        Ok(Spawned {
            task_id: task_id as u64,
            supervision_slot: supervision_slot as u32,
        })
    }
}

/// Mint a bounded channel pair through an `EndpointFactory` capability.
pub fn endpoint_create(factory_slot: u32) -> Result<(u32, u32), i64> {
    let (first, second) =
        unsafe { raw_syscall_pair(SYS_ENDPOINT_CREATE, factory_slot as u64, 0, 0, 0, 0) };
    if first < 0 {
        Err(first)
    } else {
        Ok((first as u32, second as u32))
    }
}

/// Query a child supervision handle. `Ok(None)` means the child is live; a
/// completed result consumes the handle slot so it can be reused.
pub fn supervision_status(slot: u32) -> Result<Option<Termination>, i64> {
    let (kind, detail) =
        unsafe { raw_syscall_pair(SYS_SUPERVISION_STATUS, slot as u64, 0, 0, 0, 0) };
    match kind {
        ERR_WOULDBLOCK => Ok(None),
        0 => Ok(Some(Termination::Exit(detail as i64))),
        1 => Ok(Some(Termination::Fault(detail))),
        2 => Ok(Some(Termination::Timeout)),
        3 => Ok(Some(Termination::PeerLoss)),
        4 => Ok(Some(Termination::Unhealthy)),
        error => Err(error),
    }
}

/// Releases the capability in `slot`, revoking this task's ownership of it.
pub fn cap_drop(slot: u32) -> i64 {
    unsafe { raw_syscall(SYS_CAP_DROP, slot as u64, 0, 0, 0, 0) }
}

pub const MAX_DIRECTORY_PATH: usize = 48;

/// Returns the current immutable root and this capability's enforced scope.
pub fn directory_inspect(
    slot: u32,
    required_rights: u32,
    root: &mut [u8; 32],
    scope: &mut [u8; MAX_DIRECTORY_PATH],
) -> Result<usize, i64> {
    let result = unsafe {
        raw_syscall(
            SYS_DIRECTORY_INSPECT,
            slot as u64,
            required_rights as u64,
            root.as_mut_ptr() as u64,
            scope.as_mut_ptr() as u64,
            0,
        )
    };
    if result < 0 {
        Err(result)
    } else {
        Ok(result as usize)
    }
}

/// Derives a capability scoped below `relative`, with a narrow rights mask.
pub fn directory_derive(slot: u32, relative: &[u8], rights: u32) -> Result<u32, i64> {
    let result = unsafe {
        raw_syscall(
            SYS_DIRECTORY_DERIVE,
            slot as u64,
            relative.as_ptr() as u64,
            relative.len() as u64,
            rights as u64,
            0,
        )
    };
    if result < 0 {
        Err(result)
    } else {
        Ok(result as u32)
    }
}

/// Atomically swaps a directory namespace root after the new snapshot object
/// has been committed. A stale expected root returns `ERR_WOULDBLOCK`.
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

/// Reads one decoded keyboard event through an explicit input capability.
pub fn input_read(slot: u32) -> Result<Option<InputEvent>, i64> {
    let (result, encoded) = unsafe { raw_syscall_pair(SYS_INPUT_READ, slot as u64, 0, 0, 0, 0) };
    if result == ERR_WOULDBLOCK {
        return Ok(None);
    }
    if result < 0 {
        return Err(result);
    }
    let code = encoded as u32;
    let key = match code {
        1 => InputKey::Escape,
        2 => InputKey::Backspace,
        3 => InputKey::Tab,
        4 => InputKey::Enter,
        5 => InputKey::LeftControl,
        6 => InputKey::LeftShift,
        7 => InputKey::RightShift,
        8 => InputKey::LeftAlt,
        9 => InputKey::Space,
        10 => InputKey::Up,
        11 => InputKey::Down,
        12 => InputKey::Left,
        13 => InputKey::Right,
        value if value & 0x1_0000 != 0 => InputKey::Unknown(value as u16),
        value if value & 0x100 != 0 => {
            let character = char::from_u32(value & !0x100).ok_or(ERR_INVALID_ARG)?;
            InputKey::Character(character)
        }
        _ => return Err(ERR_INVALID_ARG),
    };
    Ok(Some(InputEvent {
        key,
        pressed: encoded >> 32 != 0,
    }))
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

/// Issues a fixed generation-management request/reply pair through the
/// `GenerationControl` capability in `slot`.
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

/// Terminates the current component with an explicit unhealthy status.
pub fn unhealthy() -> ! {
    unsafe {
        raw_syscall(SYS_UNHEALTHY, 0, 0, 0, 0, 0);
    }
    loop {
        core::hint::spin_loop();
    }
}
