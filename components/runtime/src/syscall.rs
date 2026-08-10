//! The Slime operation API components program against.
//!
//! Operation numbers, arguments, errors, bounds, and transfer semantics are
//! defined once here. [`sel4_transport`] calls the badged root service endpoint
//! in child CSpace slot 1: the operation number is the message label, at most
//! four fast message registers cross in each direction, and larger payloads use
//! the component's bound transfer window.

use sel4_transport as transport;

mod sel4_transport;
mod wire;

pub use sel4_transport::ROOT_SERVICE_SLOT;

/// Declare the root-mapped startup transfer window. Called once by
/// [`crate::runtime::start`], before the component body runs, so that `recv`,
/// `spawn` and `wait` have somewhere to stage payloads.
pub(crate) fn bind_startup_window(base: usize) -> i64 {
    sel4_transport::bind_startup_window(base)
}

pub(crate) fn early_debug_write(bytes: &[u8]) {
    sel4_transport::early_debug_write(bytes)
}
const SYS_SEND: u64 = 1;
const SYS_RECV: u64 = 2;
const SYS_EXIT: u64 = 3;
const SYS_SPAWN: u64 = 4;
const SYS_HEALTH_CONFIRM: u64 = 8;
const SYS_UNHEALTHY: u64 = 9;
const SYS_RECOVERY_RECONSTRUCT: u64 = 10;
const SYS_ENDPOINT_CREATE: u64 = 11;
const SYS_SUPERVISION_STATUS: u64 = 12;
const SYS_CAP_DROP: u64 = 13;
const SYS_DIRECTORY_INSPECT: u64 = 14;
const SYS_DIRECTORY_DERIVE: u64 = 15;
const SYS_DIRECTORY_COMMIT: u64 = 16;

const SYS_GENERATION_TRANSACT: u64 = 18;
pub const SYS_GENERATION_RECEIVE: u64 = 19;
const SYS_WAIT: u64 = 20;
const SYS_SHARED_BUFFER_CREATE: u64 = 21;
const SYS_SHARED_BUFFER_RELEASE: u64 = 22;
const SYS_SHARED_BUFFER_MAP: u64 = 23;
const SYS_SHARED_BUFFER_UNMAP: u64 = 24;
const SYS_SHARED_BUFFER_SEAL: u64 = 25;
const SYS_SHARED_BUFFER_LOAN: u64 = 26;
const SYS_SHARED_BUFFER_LOAN_MAP: u64 = 27;
const SYS_SHARED_BUFFER_RETURN: u64 = 28;
const SYS_SHARED_BUFFER_REVOKE: u64 = 29;
const SYS_CAP_TRANSFER: u64 = 30;
const SYS_TRANSFER_WINDOW_BIND: u64 = 31;
/// B25: derive a second supervision handle naming a task already supervised.
const SYS_SUPERVISION_DERIVE: u64 = 32;

pub const ERR_SUCCESS: i64 = 0;
pub const ERR_BAD_CAP: i64 = -1;
pub const ERR_PEER_DEAD: i64 = -2;
pub const ERR_WOULDBLOCK: i64 = -3;
pub const ERR_INVALID_ARG: i64 = -4;
pub const ERR_OUT_OF_MEMORY: i64 = -5;

pub const MAX_MSG: usize = 64;
pub const MAX_CAPS_PER_MSG: usize = 4;

/// Size of the root-mapped startup transfer window: enough for the largest
/// single frame any operation stages.
pub(crate) const MIN_TRANSFER_WINDOW: usize = 4096;

/// A capability rights bitset shared with generation format v3.
pub type Rights = u64;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SpawnGrant {
    pub slot: u32,
    pub rights: Rights,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spawned {
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

/// Relinquishes the CPU for the rest of this time slice. The only operation
/// with no policy content, so the seL4 transport issues `seL4_Yield` directly
/// rather than crossing the root service endpoint.
pub fn yield_now() {
    transport::yield_now();
}

/// Maximum number of sources a single [`wait`] call may register.
pub const MAX_WAIT_SOURCES: usize = 9;

const WAIT_KIND_ENDPOINT: u64 = 0;
const WAIT_KIND_INPUT: u64 = 1;
const WAIT_KIND_SUPERVISION: u64 = 2;
const WAIT_KIND_SEND_CAPACITY: u64 = 3;

/// One source to block on in [`wait`]. Each maps to a non-blocking poll ABI:
/// after `wait` returns, re-poll the same source(s) to consume the event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitSource {
    /// Endpoint capability slot: woken by a peer `send` or peer death.
    Endpoint(u32),
    /// Endpoint capability slot: woken when the peer receive queue has room or
    /// the peer dies.
    SendCapacity(u32),
    /// Keyboard input: woken by a key event or scripted byte.
    Input,
    /// Supervision capability slot: woken when the supervised child exits.
    Supervision(u32),
}

impl WaitSource {
    fn descriptor(self) -> u64 {
        match self {
            WaitSource::Endpoint(slot) => WAIT_KIND_ENDPOINT << 32 | slot as u64,
            WaitSource::SendCapacity(slot) => WAIT_KIND_SEND_CAPACITY << 32 | slot as u64,
            WaitSource::Input => WAIT_KIND_INPUT << 32,
            WaitSource::Supervision(slot) => WAIT_KIND_SUPERVISION << 32 | slot as u64,
        }
    }
}

/// Blocks the caller until one of `sources` (at most [`MAX_WAIT_SOURCES`])
/// becomes ready, consuming no CPU while parked. This is the blocking
/// counterpart to a busy `yield_now` retry loop: sweep every source with its
/// non-blocking call first, and only call `wait` once all returned
/// `ERR_WOULDBLOCK`/`Ok(None)`. Spurious wakeups are possible, so the caller
/// must re-poll after `wait` returns rather than assume readiness.
pub fn wait(sources: &[WaitSource]) {
    debug_assert!(
        sources.len() <= MAX_WAIT_SOURCES,
        "wait() drops sources beyond MAX_WAIT_SOURCES"
    );
    transport::wait(sources);
}

/// Sends `payload` (at most [`MAX_MSG`] bytes) over the endpoint in
/// capability slot `slot`, transferring the capabilities named in `caps`
/// (at most [`MAX_CAPS_PER_MSG`]).
pub fn send(slot: u32, payload: &[u8], caps: &[u32]) -> i64 {
    transport::send(slot, payload, caps)
}

/// Receives into `buf` (must be [`MAX_MSG`] bytes) and `cap_out` (must be
/// [`MAX_CAPS_PER_MSG`] entries) from the endpoint in capability slot `slot`.
/// Returns the received byte count, or a negative error.
pub fn recv(slot: u32, buf: &mut [u8; MAX_MSG], cap_out: &mut [u64; MAX_CAPS_PER_MSG]) -> i64 {
    transport::recv(slot, buf, cap_out)
}

pub fn exit(status: i64) -> ! {
    transport::exit(status)
}

/// Spawns the executable in `executable_slot`. Each grant is a non-consuming
/// narrow copy; the source capability remains in the spawner. Success returns
/// both the child task id and a supervision capability slot.
pub fn spawn(executable_slot: u32, grants: &[SpawnGrant]) -> Result<Spawned, i64> {
    // The root still answers with a result word and the supervision slot; the
    // word is an error code or a success marker, never an identity the caller
    // keeps (B42).
    let (result, supervision_slot) = transport::spawn(executable_slot, grants);
    if result < 0 {
        Err(result)
    } else {
        Ok(Spawned {
            supervision_slot: supervision_slot as u32,
        })
    }
}

/// Mint a bounded channel pair through an `EndpointFactory` capability.
pub fn endpoint_create(factory_slot: u32) -> Result<(u32, u32), i64> {
    let (first, second) = transport::endpoint_create(factory_slot);
    if first < 0 {
        Err(first)
    } else {
        Ok((first as u32, second as u32))
    }
}

/// Move the capability in `capability_slot` to the peer of the endpoint in
/// `endpoint_slot`, with its rights narrowed to the mask the 64-byte
/// `descriptor` declares (C8.3).
///
/// Unlike [`send`]'s capability attachment, which moves a capability at its
/// full held rights, this is a bounded narrow-on-transfer move: the source
/// needs `RIGHT_TRANSFER`, the destination mask must be a subset of the source
/// rights and of the object's meaningful rights, and the destination loses
/// `RIGHT_TRANSFER` unless the descriptor explicitly retains it. On success the
/// source capability is consumed; on failure it is left untouched.
///
/// The descriptor crosses as the message payload, so the receiver parses
/// exactly the bytes the mechanism owner enforced.
pub fn cap_transfer(endpoint_slot: u32, capability_slot: u32, descriptor: &[u8; 64]) -> i64 {
    transport::cap_transfer(endpoint_slot, capability_slot, descriptor)
}

/// A shared buffer allocated through a `SharedBufferFactory` capability: the
/// slot holding the new `SharedBuffer` handle, plus the assigned unforgeable
/// identity that names it across a transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharedBuffer {
    pub slot: u32,
    pub id: u64,
}

/// Allocate `pages` of shared memory through a `SharedBufferFactory` capability
/// (C7.2). Charged to this component's generation-declared quota (C7.3); a
/// component with no budget entry is denied. `writable` requests write and seal
/// authority on the returned handle.
pub fn shared_buffer_create(
    factory_slot: u32,
    pages: usize,
    writable: bool,
) -> Result<SharedBuffer, i64> {
    let (slot, id) = transport::shared_buffer_create(factory_slot, pages, writable);
    if slot < 0 {
        Err(slot)
    } else {
        Ok(SharedBuffer {
            slot: slot as u32,
            id,
        })
    }
}

/// Release a shared buffer and invalidate this holder's capability. Pages stay
/// retained while any loan is outstanding.
pub fn shared_buffer_release(slot: u32) -> i64 {
    transport::shared_buffer_release(slot)
}

/// Map an exact page-aligned subrange of a shared buffer at `base`. Requires
/// `RIGHT_BUFFER_MAP`; `writable` additionally requires `RIGHT_BUFFER_WRITE`
/// and fails once the region is sealed.
pub fn shared_buffer_map(slot: u32, base: u64, offset: u64, length: u64, writable: bool) -> i64 {
    transport::shared_buffer_map(slot, base, offset, length, writable)
}

/// Remove this holder's mapping of `slot` at `base` and return its charge.
pub fn shared_buffer_unmap(slot: u32, base: u64) -> i64 {
    transport::shared_buffer_unmap(slot, base)
}

/// Irreversibly seal a shared buffer read-only, downgrading every live writable
/// mapping first. Requires `RIGHT_BUFFER_WRITE`; write access never returns.
pub fn shared_buffer_seal(slot: u32) -> i64 {
    transport::shared_buffer_seal(slot)
}

/// An outstanding loan of a sealed shared-buffer subrange: the slot holding the
/// receiver-bound `SharedBufferLoan` handle, plus its assigned single-return
/// identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferLoan {
    pub slot: u32,
    pub id: u64,
}

/// Loan an exact sealed subrange to the task named by a `RIGHT_SUPERVISE`
/// capability (C7.5). The source needs `RIGHT_BUFFER_LOAN` and must already be
/// irreversibly sealed; the loan is read-only, single-return, and charged
/// against the lender's `loan_count` quota. The receiver is named through a
/// capability, never an ambient task id.
pub fn shared_buffer_loan(
    buffer_slot: u32,
    receiver_slot: u32,
    offset: u64,
    length: u64,
) -> Result<BufferLoan, i64> {
    let (slot, id) = transport::shared_buffer_loan(buffer_slot, receiver_slot, offset, length);
    if slot < 0 {
        Err(slot)
    } else {
        Ok(BufferLoan {
            slot: slot as u32,
            id,
        })
    }
}

/// Map a read-only subrange relative to a loan this task received. Offsets are
/// relative to the loaned region, so the receiver cannot address bytes outside
/// it even by naming the underlying buffer.
pub fn shared_buffer_loan_map(loan_slot: u32, base: u64, offset: u64, length: u64) -> i64 {
    transport::shared_buffer_loan_map(loan_slot, base, offset, length)
}

/// Return a loan as its named receiver, settling it once and releasing the
/// loan capability. A second return fails: the identity is single-use.
pub fn shared_buffer_return(loan_slot: u32) -> i64 {
    transport::shared_buffer_return(loan_slot)
}

/// Revoke an outstanding loan as its lender, naming it by the source buffer and
/// the loan's assigned identity.
pub fn shared_buffer_revoke(buffer_slot: u32, loan_id: u64) -> i64 {
    transport::shared_buffer_revoke(buffer_slot, loan_id)
}

/// Query a child supervision handle. `Ok(None)` means the child is live; a
/// completed result consumes the handle slot so it can be reused.
/// Obtain a second supervision handle naming the same task (B25).
///
/// Returns the slot the new capability landed in. The caller keeps `slot`: this
/// is a derive, not a move, so a parent can introduce one child to several
/// others. Rights are the source's own and the task named is the same, so this
/// mints nothing the caller could not already have transferred.
pub fn supervision_derive(slot: u32) -> Result<u32, i64> {
    let (result, derived) = transport::supervision_derive(slot);
    if result < 0 {
        Err(result)
    } else {
        Ok(derived as u32)
    }
}

pub fn supervision_status(slot: u32) -> Result<Option<Termination>, i64> {
    let (kind, detail) = transport::supervision_status(slot);
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
    transport::cap_drop(slot)
}

pub const MAX_DIRECTORY_PATH: usize = 48;

/// Returns the current immutable root and this capability's enforced scope.
/// A namespace root identity: a SHA-256 over the directory object it names.
/// The mechanism never interprets it; the bound is here so a caller can size
/// its buffer without knowing that.
pub const DIRECTORY_ROOT_BYTES: usize = 32;

pub fn directory_inspect(
    slot: u32,
    required_rights: u32,
    root: &mut [u8; 32],
    scope: &mut [u8; MAX_DIRECTORY_PATH],
) -> Result<usize, i64> {
    let result = transport::directory_inspect(slot, required_rights, root, scope);
    if result < 0 {
        Err(result)
    } else {
        Ok(result as usize)
    }
}

/// Derives a capability scoped below `relative`, with a narrow rights mask.
pub fn directory_derive(slot: u32, relative: &[u8], rights: u32) -> Result<u32, i64> {
    let result = transport::directory_derive(slot, relative, rights);
    if result < 0 {
        Err(result)
    } else {
        Ok(result as u32)
    }
}

/// Atomically swaps a directory namespace root after the new snapshot object
/// has been committed. A stale expected root returns `ERR_WOULDBLOCK`.
pub fn directory_commit(slot: u32, expected: &[u8; 32], new: &[u8; 32]) -> i64 {
    transport::directory_commit(slot, expected, new)
}

/// Reads one decoded keyboard event through an explicit input capability.
pub fn input_read(slot: u32) -> Result<Option<InputEvent>, i64> {
    let (result, encoded) = transport::input_read(slot);
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

/// Writes `bytes` to the debug/serial log. Returns the byte count written.
pub fn debug_write(bytes: &[u8]) -> i64 {
    transport::debug_write(bytes)
}

/// Issues a 64-byte block-protocol request/reply pair against the block
/// device capability in slot `slot`. A non-negative return means the
/// transaction was delivered; the block-protocol outcome is in `reply`
/// (`OFF_REPLY_STATUS`), not in the return value.
pub fn block_transact(slot: u32, request: &[u8; 64], reply: &mut [u8; 64]) -> i64 {
    transport::block_transact(slot, request, reply)
}

/// A read whose sector returns through the caller's transfer window behind the
/// 64-byte reply record.
pub fn block_transact_sector(
    slot: u32,
    request: &[u8; 64],
    reply: &mut [u8; 64],
    sector: &mut [u8; 512],
) -> i64 {
    transport::block_transact_sector(slot, request, reply, sector)
}

/// A write whose sector crosses with the request, on the same rule.
pub fn block_transact_write(
    slot: u32,
    request: &[u8; 64],
    sector: &[u8; 512],
    reply: &mut [u8; 64],
) -> i64 {
    transport::block_transact_write(slot, request, sector, reply)
}

/// Issues a fixed generation-management request/reply pair through the
/// `GenerationControl` capability in `slot`.
pub fn generation_transact(slot: u32, request: &[u8; 64], reply: &mut [u8; 64]) -> i64 {
    transport::generation_transact(slot, request, reply)
}

/// Confirms the currently running pending generation using the
/// `GenerationControl` capability in `slot`.
pub fn health_confirm(slot: u32) -> i64 {
    transport::health_confirm(slot)
}

/// Scrubs and reconstructs BootState on the explicitly granted repair target.
pub fn recovery_reconstruct(generation_control_slot: u32, block_slot: u32, flags: u32) -> i64 {
    transport::recovery_reconstruct(generation_control_slot, block_slot, flags)
}

pub fn generation_receive(receiver_slot: u32, transfer_slot: u32) -> i64 {
    transport::generation_receive(receiver_slot, transfer_slot)
}

/// Terminates the current component with an explicit unhealthy status.
pub fn unhealthy() -> ! {
    transport::unhealthy()
}
