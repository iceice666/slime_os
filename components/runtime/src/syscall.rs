//! Narrow Slime service APIs exposed to components.
//!
//! Each root-served mechanism owns its message labels. [`sel4_transport`]
//! carries those labels over the badged root endpoint in child CSpace slot 1;
//! native endpoints and notifications remain direct seL4 capabilities.

use sel4_transport as transport;

mod sel4_transport;
mod wire;

pub use sel4_transport::ROOT_SERVICE_SLOT;

/// Record the root-mapped startup transfer window locally. The root created
/// and authenticated this mapping while constructing the thread, so no syscall
/// is needed to re-declare it.
pub(crate) fn bind_startup_window(base: usize) -> i64 {
    sel4_transport::bind_startup_window(base)
}

pub(crate) fn early_debug_write(bytes: &[u8]) {
    sel4_transport::early_debug_write(bytes)
}
mod lifecycle_labels {
    pub const EXIT: u64 = 3;
    pub const UNHEALTHY: u64 = 9;
}

mod spawn_labels {
    pub const SPAWN: u64 = 4;
}

mod supervision_labels {
    pub const STATUS: u64 = 12;
    pub const DERIVE: u64 = 32;
}

mod capability_table_labels {
    pub const DROP: u64 = 13;
}

mod directory_labels {
    pub const DERIVE: u64 = 15;
}

mod shared_buffer_labels {
    pub const CREATE: u64 = 21;
    pub const RELEASE: u64 = 22;
    pub const MAP: u64 = 23;
    pub const UNMAP: u64 = 24;
    pub const SEAL: u64 = 25;
    pub const LOAN: u64 = 26;
    pub const LOAN_MAP: u64 = 27;
    pub const RETURN: u64 = 28;
    pub const REVOKE: u64 = 29;
}

mod capability_transfer_labels {
    pub const EXPORT: u64 = 33;
    pub const IMPORT: u64 = 34;
    pub const EXPORT_CANCEL: u64 = 35;
    pub const EXPORT_FINALIZE: u64 = 36;
}

pub const ERR_SUCCESS: i64 = 0;
pub const ERR_BAD_CAP: i64 = -1;
pub const ERR_PEER_DEAD: i64 = -2;
pub const ERR_WOULDBLOCK: i64 = -3;
pub const ERR_INVALID_ARG: i64 = -4;
pub const ERR_OUT_OF_MEMORY: i64 = -5;

pub const MAX_MSG: usize = 64;
pub const MAX_CAPS_PER_MSG: usize = 1;

/// Whether delegation consumes the source logical capability or retains it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityDisposition {
    Move,
    Retain,
}

/// Size of the root-mapped startup transfer window: enough for the largest
/// single service frame.
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

/// Relinquishes the CPU for the rest of this time slice. This is direct
/// `seL4_Yield`, not a root-service request.
pub fn yield_now() {
    transport::yield_now();
}

/// Sends `payload` (at most [`MAX_MSG`] bytes) over the endpoint in
/// capability slot `slot`, transferring the logical capabilities named in
/// `caps` (at most [`MAX_CAPS_PER_MSG`]). Each logical slot names its
/// root-minted kernel token mirror for the native Endpoint send.
pub fn send(slot: u32, payload: &[u8], caps: &[u32]) -> i64 {
    transport::send(slot, payload, caps)
}

/// Sends `payload` over `slot` and waits for the reply, as one `seL4_Call`.
///
/// The primitive B46 names for synchronous RPC. A `send` followed by a `recv`
/// cannot substitute: `send` completes as soon as the peer receives, so the
/// caller must race back to a receive, and a peer that answers first leaves the
/// two never meeting. This blocks on the reply atomically and hands the callee
/// authority naming *this* caller, so the answer cannot be taken by another
/// peer on the same endpoint.
///
/// Use it where a bare send is either lossy or deadlock-prone: [`try_send`]
/// cannot report delivery, and [`send`] blocks against a peer that may itself
/// be sending.
pub fn call(slot: u32, payload: &[u8], reply: &mut [u8; MAX_MSG]) -> i64 {
    transport::call_endpoint(slot, payload, reply)
}

/// Answers the request this thread most recently received. Reaches that caller
/// alone and cannot block: it is already waiting in [`call`].
pub fn reply(payload: &[u8]) -> i64 {
    transport::reply_to_caller(payload)
}

/// Sends `payload` over `slot` only if a receiver is already waiting, dropping
/// it otherwise.
///
/// [`send`] blocks until a receiver arrives, which deadlocks any sender that
/// must stay responsive — a broker pushing an *unsolicited* event to a peer
/// that has moved on to reading its ring, for instance. The kernel's
/// `seL4_NBSend` discards the message rather than blocking and reports nothing
/// either way, so this is best-effort by construction and returns
/// [`ERR_SUCCESS`] for "attempted". Correct only where the message is advisory
/// and the protocol does not require it to arrive; a reply or a handshake step
/// must still use [`send`].
pub fn try_send(slot: u32, payload: &[u8], caps: &[u32]) -> i64 {
    transport::try_send(slot, payload, caps)
}

/// Receives into `buf` (must be [`MAX_MSG`] bytes) and `cap_out` (must be
/// [`MAX_CAPS_PER_MSG`] entries) from the endpoint in capability slot `slot`.
/// A received kernel ticket lands in fixed CSpace slot 127, is authenticated
/// through that ticket endpoint, and is reported as the imported logical slot.
/// Returns the received byte count, or a negative error.
pub fn recv(slot: u32, buf: &mut [u8; MAX_MSG], cap_out: &mut [u64; MAX_CAPS_PER_MSG]) -> i64 {
    transport::recv(slot, buf, cap_out)
}

/// Blocking receive from a declared native endpoint.
pub fn recv_blocking(
    slot: u32,
    buf: &mut [u8; MAX_MSG],
    cap_out: &mut [u64; MAX_CAPS_PER_MSG],
) -> i64 {
    transport::recv_blocking(slot, buf, cap_out)
}

/// Signal the notification paired with one declared endpoint edge.
pub fn notification_signal(slot: u32) -> i64 {
    transport::notification_signal(slot)
}

/// Wait for the notification paired with one declared endpoint edge.
pub fn notification_wait(slot: u32) -> Result<u64, i64> {
    transport::notification_wait(slot)
}

/// Poll the notification paired with one declared endpoint edge.
pub fn notification_poll(slot: u32) -> Result<Option<u64>, i64> {
    transport::notification_poll(slot)
}

pub fn exit(status: i64) -> ! {
    transport::exit(status)
}

/// Spawns the executable in `executable_slot`. Each grant is a non-consuming
/// narrow copy; the source capability remains in the spawner. Success returns
/// only an opaque supervision capability slot; task identity stays root-local.
pub fn spawn(executable_slot: u32, grants: &[SpawnGrant]) -> Result<Spawned, i64> {
    let (result, supervision_slot) = transport::spawn(executable_slot, grants);
    if result < 0 {
        Err(result)
    } else {
        Ok(Spawned {
            supervision_slot: supervision_slot as u32,
        })
    }
}

/// Narrow and transfer one logical capability over a declared native endpoint.
/// The opaque 64-byte typed descriptor is carried atomically with the real
/// kernel ticket. Root authenticates `expected_kind` and attenuates to the
/// nonzero `rights_mask`; neither value is inferred from application bytes.
pub fn capability_delegate(
    endpoint_slot: u32,
    capability_slot: u32,
    disposition: CapabilityDisposition,
    expected_kind: u32,
    rights_mask: Rights,
    descriptor: &[u8; 64],
) -> i64 {
    transport::capability_delegate(
        endpoint_slot,
        capability_slot,
        disposition,
        expected_kind,
        rights_mask,
        descriptor,
    )
}

/// Claim the oldest root-recorded export addressed to this component.
///
/// The counterpart to [`capability_delegate`] for every object kind that is
/// not a native Endpoint: those have no kernel object to travel in the
/// message, so the descriptor arrives alone and this takes up the authority
/// behind it. Returns the slot the capability landed in.
pub fn capability_import() -> Result<u32, i64> {
    transport::capability_import()
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

/// Loan an exact subrange to the task named by a `RIGHT_SUPERVISE` capability
/// (C7.5). The source needs `RIGHT_BUFFER_LOAN`; the loan is single-return and
/// charged against the lender's `loan_count` quota. The receiver is named
/// through a capability, never an ambient task id.
///
/// `writable` decides what the receiver may map. A C7.6 sample loan is
/// read-only over a sealed region — the receiver reads bytes the lender
/// finished writing. A B46 stream ring is writable over an unsealed one, and
/// requires the lender to hold write authority it is handing on.
pub fn shared_buffer_loan(
    buffer_slot: u32,
    receiver_slot: u32,
    offset: u64,
    length: u64,
    writable: bool,
) -> Result<BufferLoan, i64> {
    let (slot, id) =
        transport::shared_buffer_loan(buffer_slot, receiver_slot, offset, length, writable);
    if slot < 0 {
        Err(slot)
    } else {
        Ok(BufferLoan {
            slot: slot as u32,
            id,
        })
    }
}

/// Map a subrange relative to a loan this task received, at the protection the
/// loan was minted with. Offsets are relative to the loaned region, so the
/// receiver cannot address bytes outside it even by naming the underlying
/// buffer.
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

/// Terminates the current component with an explicit unhealthy status.
pub fn unhealthy() -> ! {
    transport::unhealthy()
}
