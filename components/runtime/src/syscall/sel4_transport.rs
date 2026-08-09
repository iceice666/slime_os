//! Native seL4 transport for the Slime operation API.
//!
//! Every policy-bearing operation is a `seL4_Call` on the badged root service
//! endpoint the generation installed at child CSpace slot
//! [`ROOT_SERVICE_SLOT`]. That endpoint is the component's *only* root
//! authority: this module never names another capability, never consults
//! `BootInfo`, and holds no init-thread CNode, VSpace, or TCB capability, so a
//! component cannot address a slot its generation did not declare.
//!
//! The wire form is deliberately narrow (see [`super::wire`]):
//!
//! - the message label is the Slime operation number,
//! - at most [`wire::FAST_REGISTERS`] fast message registers cross in each
//!   direction — `MR0` carries the operand(s), `MR1` a second operand or a
//!   transfer descriptor, `MR2`/`MR3` further operands or an inline payload,
//! - a reply puts the logical result in `MR0` — the same named `ERR_*` values
//!   the trap ABI returns — and its auxiliary value, or reply descriptor, in
//!   `MR1`.
//!
//! Payloads that do not fit the inline registers travel through the transfer
//! window: a shared buffer the component mapped itself and declared with
//! [`transfer_window_bind`]. Nothing is ever truncated to fit — an operation
//! whose payload needs the window while none is bound fails with
//! [`ERR_INVALID_ARG`] and transfers nothing.
//!
//! `yield_now` carries no policy, so it maps straight onto `seL4_Yield` instead
//! of crossing the endpoint.

use core::ptr;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use sel4::{CallWithMRs, MessageInfoBuilder, Word, cap};

use super::wire::{
    self, FORM_INLINE, FORM_WINDOW, MAX_DESCRIPTOR_CAPS, MAX_DESCRIPTOR_LEN, clear_unnamed_slots,
    descriptor, descriptor_caps, descriptor_form, descriptor_len, fits_inline, frame_caps_offset,
    frame_len, pack_bytes, slot_pair, slot_with_flag,
};
use super::{
    ERR_INVALID_ARG, ERR_SUCCESS, MAX_CAPS_PER_MSG, MAX_DIRECTORY_PATH, MAX_MSG, MAX_WAIT_SOURCES,
    MIN_TRANSFER_WINDOW, SYS_BLOCK_TRANSACT, SYS_CAP_DROP, SYS_CAP_TRANSFER, SYS_DIRECTORY_COMMIT,
    SYS_DIRECTORY_DERIVE, SYS_DIRECTORY_INSPECT, SYS_ENDPOINT_CREATE, SYS_EXIT,
    SYS_GENERATION_RECEIVE, SYS_GENERATION_TRANSACT, SYS_HEALTH_CONFIRM, SYS_INPUT_READ,
    SYS_RECOVERY_RECONSTRUCT, SYS_RECV, SYS_SEND, SYS_SHARED_BUFFER_CREATE, SYS_SHARED_BUFFER_LOAN,
    SYS_SHARED_BUFFER_LOAN_MAP, SYS_SHARED_BUFFER_MAP, SYS_SHARED_BUFFER_RELEASE,
    SYS_SHARED_BUFFER_RETURN, SYS_SHARED_BUFFER_REVOKE, SYS_SHARED_BUFFER_SEAL,
    SYS_SHARED_BUFFER_UNMAP, SYS_SPAWN, SYS_STORE_TRANSACT, SYS_SUPERVISION_DERIVE,
    SYS_SUPERVISION_STATUS, SYS_TRANSFER_WINDOW_BIND, SYS_UNHEALTHY, SYS_WAIT, SpawnGrant,
    WaitSource,
};

/// The child CSpace slot holding the badged root service endpoint. Slot 0 is
/// null and every other slot belongs to the generation's declared grants; this
/// module addresses neither.
pub const ROOT_SERVICE_SLOT: sel4::CPtrBits = 1;

/// Bytes of a spawn grant record in the transfer window: slot word, then rights
/// word.
const GRANT_RECORD_BYTES: usize = 16;

/// Bytes of a wait-source record in the transfer window.
const WAIT_RECORD_BYTES: usize = 8;

/// Bytes of the immutable directory root in an inspect reply frame.
const DIRECTORY_ROOT_BYTES: usize = 32;

/// Bytes of a block, store, or generation protocol frame.
const TRANSACT_BYTES: usize = 64;

/// The root service endpoint. Reconstructed per call from a constant slot, so
/// no capability is cached in component-writable state.
fn root_service() -> cap::Endpoint {
    cap::Endpoint::from_bits(ROOT_SERVICE_SLOT)
}

/// The bound transfer window. `WINDOW_LEN == 0` means none is bound, which is
/// every component's initial state.
static WINDOW_BASE: AtomicU64 = AtomicU64::new(0);
static WINDOW_LEN: AtomicUsize = AtomicUsize::new(0);

fn window() -> Result<(*mut u8, usize), i64> {
    let len = WINDOW_LEN.load(Ordering::Acquire);
    if len == 0 {
        return Err(ERR_INVALID_ARG);
    }
    Ok((WINDOW_BASE.load(Ordering::Acquire) as *mut u8, len))
}

/// Declares an already-mapped shared buffer as this component's transfer
/// window, giving operations whose payload exceeds the inline registers a
/// bounded place to put it. `base` must be the address a prior
/// [`shared_buffer_map`] established for `buffer_slot`, and the mapping must
/// cover at least [`MIN_TRANSFER_WINDOW`] bytes.
///
/// The root service records the same buffer against this child, so both ends
/// address exactly the declared region and nothing else.
pub fn transfer_window_bind(buffer_slot: u32, base: u64, len: usize) -> i64 {
    if base == 0 || len < MIN_TRANSFER_WINDOW {
        return ERR_INVALID_ARG;
    }
    let result = result_of(
        SYS_TRANSFER_WINDOW_BIND,
        &[buffer_slot as Word, base as Word, len as Word],
    );
    if result == ERR_SUCCESS {
        WINDOW_BASE.store(base, Ordering::Release);
        WINDOW_LEN.store(len, Ordering::Release);
    }
    result
}

/// Slot value naming the startup window the root mapped rather than a
/// component-created shared buffer. A component's own buffers are named by
/// CSpace slots the generation granted, and slot 0 is null in every child
/// CSpace, so it cannot collide with one.
pub const STARTUP_WINDOW_SLOT: u32 = 0;

/// Declare the root-mapped startup window at `base` as this component's
/// transfer window.
///
/// Called once from [`crate::runtime::start`] before the component body runs.
/// Unlike [`transfer_window_bind`], the region is not a shared buffer this
/// component allocated: `slime-root` mapped it when it built the VSpace, which
/// is what lets a component with no `SharedBufferFactory` grant use `recv`.
pub fn bind_startup_window(base: usize) -> i64 {
    transfer_window_bind(STARTUP_WINDOW_SLOT, base as u64, MIN_TRANSFER_WINDOW)
}

/// Copies `bytes` and `caps` into the transfer window, returning the transfer
/// descriptor for `MR1`. Short capability-free payloads stay in the fast
/// registers and need no window at all. Anything larger fails rather than
/// truncating when no window is bound or the frame overruns the declared
/// region.
fn stage(bytes: &[u8], caps: &[u32]) -> Result<u64, i64> {
    if bytes.len() > MAX_DESCRIPTOR_LEN || caps.len() > MAX_DESCRIPTOR_CAPS {
        return Err(ERR_INVALID_ARG);
    }
    if fits_inline(bytes.len(), caps.len()) {
        return Ok(descriptor(bytes.len(), 0, FORM_INLINE));
    }
    let (base, capacity) = window()?;
    if frame_len(bytes.len(), caps.len()) > capacity {
        return Err(ERR_INVALID_ARG);
    }
    // SAFETY: `base` and `capacity` describe the mapping this component
    // established and declared, and the frame was just bounded by `capacity`.
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), base, bytes.len());
        let slots = base.add(frame_caps_offset(bytes.len())).cast::<u64>();
        for (index, slot) in caps.iter().enumerate() {
            slots.add(index).write(u64::from(*slot));
        }
    }
    Ok(descriptor(bytes.len(), caps.len(), FORM_WINDOW))
}

/// Reserves window space for a reply the root service writes back, returning
/// the transfer descriptor for `MR1`.
fn reserve(bytes: usize, caps: usize) -> Result<u64, i64> {
    let (_, capacity) = window()?;
    if frame_len(bytes, caps) > capacity {
        return Err(ERR_INVALID_ARG);
    }
    Ok(descriptor(bytes, caps, FORM_WINDOW))
}

/// Copies a reply frame out of the transfer window. `reply` is the descriptor
/// the root service returned; a frame that overruns the caller's buffer, the
/// capability bound, or the window itself is rejected rather than clipped.
fn collect(
    reply: u64,
    bytes: &mut [u8],
    caps: Option<&mut [u64; MAX_CAPS_PER_MSG]>,
) -> Result<usize, i64> {
    let len = descriptor_len(reply);
    let cap_count = descriptor_caps(reply);
    if descriptor_form(reply) != FORM_WINDOW || len > bytes.len() || cap_count > MAX_CAPS_PER_MSG {
        return Err(ERR_INVALID_ARG);
    }
    let (base, capacity) = window()?;
    if frame_len(len, cap_count) > capacity {
        return Err(ERR_INVALID_ARG);
    }
    // SAFETY: the frame lies inside the window this component declared, and
    // both its byte count and its capability count were bounded above.
    unsafe {
        ptr::copy_nonoverlapping(base, bytes.as_mut_ptr(), len);
        if let Some(slots_out) = caps {
            let slots = base.add(frame_caps_offset(len)).cast::<u64>();
            for (index, slot) in slots_out.iter_mut().take(cap_count).enumerate() {
                *slot = slots.add(index).read();
            }
            clear_unnamed_slots(slots_out.as_mut_slice(), cap_count);
        }
    }
    Ok(len)
}

/// Issues one root service call with `operands` in the fast registers.
fn call(label: u64, operands: &[Word]) -> CallWithMRs {
    debug_assert!(operands.len() <= wire::FAST_REGISTERS);
    let mut mrs = [0 as Word; wire::FAST_REGISTERS];
    mrs[..operands.len()].copy_from_slice(operands);
    let info = MessageInfoBuilder::default()
        .label(label as Word)
        .length(operands.len())
        .build();
    root_service().call_with_mrs(info, mrs)
}

/// Splits a reply into its logical result and auxiliary word. A reply carrying
/// no result register is malformed, not a silent success.
fn outcome(reply: &CallWithMRs) -> Result<(i64, u64), i64> {
    if reply.info.length() < 1 {
        return Err(ERR_INVALID_ARG);
    }
    let aux = if reply.info.length() >= 2 {
        reply.msg[1]
    } else {
        0
    };
    Ok((reply.msg[0] as i64, aux))
}

/// One call, one logical `i64` result.
fn result_of(label: u64, operands: &[Word]) -> i64 {
    match outcome(&call(label, operands)) {
        Ok((result, _)) => result,
        Err(error) => error,
    }
}

/// One call, a logical result plus an auxiliary value.
fn pair_of(label: u64, operands: &[Word]) -> (i64, u64) {
    match outcome(&call(label, operands)) {
        Ok(pair) => pair,
        Err(error) => (error, 0),
    }
}

/// Builds the operand list for a payload-carrying request: primary operand,
/// transfer descriptor, then the inline payload when it stayed in registers.
fn payload_operands(
    primary: u64,
    transfer: u64,
    bytes: &[u8],
) -> ([Word; wire::FAST_REGISTERS], usize) {
    let mut operands = [0 as Word; wire::FAST_REGISTERS];
    operands[0] = primary as Word;
    operands[1] = transfer as Word;
    if descriptor_form(transfer) == FORM_INLINE && !bytes.is_empty() {
        let inline = pack_bytes(bytes);
        operands[2] = inline[0] as Word;
        operands[3] = inline[1] as Word;
        (operands, wire::FAST_REGISTERS)
    } else {
        (operands, 2)
    }
}

pub fn yield_now() {
    sel4::r#yield();
}

pub fn wait(sources: &[WaitSource]) {
    let count = sources.len().min(MAX_WAIT_SOURCES);
    let mut encoded = [0u8; MAX_WAIT_SOURCES * WAIT_RECORD_BYTES];
    for (index, source) in sources.iter().take(count).enumerate() {
        let start = index * WAIT_RECORD_BYTES;
        encoded[start..start + WAIT_RECORD_BYTES]
            .copy_from_slice(&source.descriptor().to_le_bytes());
    }
    let bytes = &encoded[..count * WAIT_RECORD_BYTES];
    let Ok(transfer) = stage(bytes, &[]) else {
        // The source set cannot cross intact without a window, and `wait`
        // returns `()` so there is no error to hand back. Yielding lets the
        // caller re-poll, which is right when the next poll can succeed.
        //
        // It announces itself because when the next poll *cannot* succeed this
        // is an invisible hang: the caller loops between `recv` and `wait`, and
        // `yield_now` is `sel4::r#yield()`, a kernel primitive the root task
        // never observes — so the graph deadlocks with no marker anywhere. B28
        // was characterized across eight refuted readings partly because this
        // arm could not be ruled out by reading a transcript; instrumenting it
        // is what excluded it, and the marker is kept so the next reader does
        // not have to add it again.
        //
        // Unreachable on every current plane: a component binds a window before
        // its first parking operation, and one wait record is eight bytes
        // against a `MIN_TRANSFER_WINDOW` of a page. That is why this costs
        // nothing to leave in.
        crate::debug_write(b"[rt] wait source set could not be staged\n");
        yield_now();
        return;
    };
    let (operands, used) = payload_operands(count as u64, transfer, bytes);
    let _ = call(SYS_WAIT, &operands[..used]);
}

pub fn send(slot: u32, payload: &[u8], caps: &[u32]) -> i64 {
    if payload.len() > MAX_MSG || caps.len() > MAX_CAPS_PER_MSG {
        return ERR_INVALID_ARG;
    }
    let transfer = match stage(payload, caps) {
        Ok(transfer) => transfer,
        Err(error) => return error,
    };
    let (operands, used) = payload_operands(slot as u64, transfer, payload);
    result_of(SYS_SEND, &operands[..used])
}

pub fn recv(slot: u32, buf: &mut [u8; MAX_MSG], cap_out: &mut [u64; MAX_CAPS_PER_MSG]) -> i64 {
    let transfer = match reserve(MAX_MSG, MAX_CAPS_PER_MSG) {
        Ok(transfer) => transfer,
        Err(error) => return error,
    };
    let (result, returned) = match outcome(&call(SYS_RECV, &[slot as Word, transfer as Word])) {
        Ok(pair) => pair,
        Err(error) => return error,
    };
    if result < 0 {
        return result;
    }
    match collect(returned, buf.as_mut_slice(), Some(cap_out)) {
        Ok(len) if len == result as usize => result,
        Ok(_) => ERR_INVALID_ARG,
        Err(error) => error,
    }
}

pub fn exit(status: i64) -> ! {
    let _ = call(SYS_EXIT, &[status as Word]);
    loop {
        core::hint::spin_loop();
    }
}

/// Largest grant count a spawn call can carry: matches the kernel's per-task
/// capability capacity (`kernel/src/capability/mod.rs::MAX_CAPS`), the real
/// bound a spawn's grant array is checked against server-side.
const MAX_SPAWN_GRANTS: usize = 64;

pub fn spawn(executable_slot: u32, grants: &[SpawnGrant]) -> (i64, u64) {
    let mut encoded = [0u8; MAX_SPAWN_GRANTS * GRANT_RECORD_BYTES];
    let Some(frame) = encoded.get_mut(..grants.len() * GRANT_RECORD_BYTES) else {
        return (ERR_INVALID_ARG, 0);
    };
    for (record, grant) in frame.chunks_exact_mut(GRANT_RECORD_BYTES).zip(grants) {
        record[..8].copy_from_slice(&u64::from(grant.slot).to_le_bytes());
        record[8..].copy_from_slice(&grant.rights.to_le_bytes());
    }
    let bytes = &encoded[..grants.len() * GRANT_RECORD_BYTES];
    let transfer = match stage(bytes, &[]) {
        Ok(transfer) => transfer,
        Err(error) => return (error, 0),
    };
    let (operands, used) = payload_operands(executable_slot as u64, transfer, bytes);
    pair_of(SYS_SPAWN, &operands[..used])
}

pub fn endpoint_create(factory_slot: u32) -> (i64, u64) {
    pair_of(SYS_ENDPOINT_CREATE, &[factory_slot as Word])
}

pub fn cap_transfer(endpoint_slot: u32, capability_slot: u32, descriptor: &[u8; 64]) -> i64 {
    let transfer = match stage(descriptor.as_slice(), &[]) {
        Ok(transfer) => transfer,
        Err(error) => return error,
    };
    result_of(
        SYS_CAP_TRANSFER,
        &[
            slot_pair(endpoint_slot, capability_slot) as Word,
            transfer as Word,
        ],
    )
}

pub fn shared_buffer_create(factory_slot: u32, pages: usize, writable: bool) -> (i64, u64) {
    pair_of(
        SYS_SHARED_BUFFER_CREATE,
        &[
            slot_with_flag(factory_slot, writable) as Word,
            pages as Word,
        ],
    )
}

pub fn shared_buffer_release(slot: u32) -> i64 {
    result_of(SYS_SHARED_BUFFER_RELEASE, &[slot as Word])
}

pub fn shared_buffer_map(slot: u32, base: u64, offset: u64, length: u64, writable: bool) -> i64 {
    result_of(
        SYS_SHARED_BUFFER_MAP,
        &[
            slot_with_flag(slot, writable) as Word,
            base as Word,
            offset as Word,
            length as Word,
        ],
    )
}

pub fn shared_buffer_unmap(slot: u32, base: u64) -> i64 {
    result_of(SYS_SHARED_BUFFER_UNMAP, &[slot as Word, base as Word])
}

pub fn shared_buffer_seal(slot: u32) -> i64 {
    result_of(SYS_SHARED_BUFFER_SEAL, &[slot as Word])
}

pub fn shared_buffer_loan(
    buffer_slot: u32,
    receiver_slot: u32,
    offset: u64,
    length: u64,
) -> (i64, u64) {
    pair_of(
        SYS_SHARED_BUFFER_LOAN,
        &[
            slot_pair(buffer_slot, receiver_slot) as Word,
            offset as Word,
            length as Word,
        ],
    )
}

pub fn shared_buffer_loan_map(loan_slot: u32, base: u64, offset: u64, length: u64) -> i64 {
    result_of(
        SYS_SHARED_BUFFER_LOAN_MAP,
        &[
            loan_slot as Word,
            base as Word,
            offset as Word,
            length as Word,
        ],
    )
}

pub fn shared_buffer_return(loan_slot: u32) -> i64 {
    result_of(SYS_SHARED_BUFFER_RETURN, &[loan_slot as Word])
}

pub fn shared_buffer_revoke(buffer_slot: u32, loan_id: u64) -> i64 {
    result_of(
        SYS_SHARED_BUFFER_REVOKE,
        &[buffer_slot as Word, loan_id as Word],
    )
}

pub fn supervision_status(slot: u32) -> (i64, u64) {
    pair_of(SYS_SUPERVISION_STATUS, &[slot as Word])
}

pub fn supervision_derive(slot: u32) -> (i64, u64) {
    pair_of(SYS_SUPERVISION_DERIVE, &[slot as Word])
}

pub fn cap_drop(slot: u32) -> i64 {
    result_of(SYS_CAP_DROP, &[slot as Word])
}

pub fn directory_inspect(
    slot: u32,
    required_rights: u32,
    root: &mut [u8; DIRECTORY_ROOT_BYTES],
    scope: &mut [u8; MAX_DIRECTORY_PATH],
) -> i64 {
    let transfer = match reserve(DIRECTORY_ROOT_BYTES + MAX_DIRECTORY_PATH, 0) {
        Ok(transfer) => transfer,
        Err(error) => return error,
    };
    let (result, returned) = match outcome(&call(
        SYS_DIRECTORY_INSPECT,
        &[slot_pair(slot, required_rights) as Word, transfer as Word],
    )) {
        Ok(pair) => pair,
        Err(error) => return error,
    };
    if result < 0 {
        return result;
    }
    let scope_len = result as usize;
    if scope_len > MAX_DIRECTORY_PATH {
        return ERR_INVALID_ARG;
    }
    let mut frame = [0u8; DIRECTORY_ROOT_BYTES + MAX_DIRECTORY_PATH];
    match collect(returned, &mut frame, None) {
        Ok(len) if len == DIRECTORY_ROOT_BYTES + scope_len => {
            root.copy_from_slice(&frame[..DIRECTORY_ROOT_BYTES]);
            scope[..scope_len].copy_from_slice(&frame[DIRECTORY_ROOT_BYTES..len]);
            result
        }
        Ok(_) => ERR_INVALID_ARG,
        Err(error) => error,
    }
}

pub fn directory_derive(slot: u32, relative: &[u8], rights: u32) -> i64 {
    if relative.len() > MAX_DIRECTORY_PATH {
        return ERR_INVALID_ARG;
    }
    let transfer = match stage(relative, &[]) {
        Ok(transfer) => transfer,
        Err(error) => return error,
    };
    let (operands, used) = payload_operands(slot_pair(slot, rights), transfer, relative);
    result_of(SYS_DIRECTORY_DERIVE, &operands[..used])
}

pub fn directory_commit(slot: u32, expected: &[u8; 32], new: &[u8; 32]) -> i64 {
    let mut frame = [0u8; 64];
    frame[..32].copy_from_slice(expected);
    frame[32..].copy_from_slice(new);
    let transfer = match stage(&frame, &[]) {
        Ok(transfer) => transfer,
        Err(error) => return error,
    };
    result_of(SYS_DIRECTORY_COMMIT, &[slot as Word, transfer as Word])
}

pub fn input_read(slot: u32) -> (i64, u64) {
    pair_of(SYS_INPUT_READ, &[slot as Word])
}

/// The shared shape of the three 64-byte request/reply protocols: the request
/// crosses in the transfer window and the reply is written back over it.
fn transact(label: u64, slot: u32, request: &[u8; 64], reply_out: &mut [u8; 64]) -> i64 {
    let transfer = match stage(request.as_slice(), &[]) {
        Ok(transfer) => transfer,
        Err(error) => return error,
    };
    let (result, returned) = match outcome(&call(label, &[slot as Word, transfer as Word])) {
        Ok(pair) => pair,
        Err(error) => return error,
    };
    if result < 0 {
        return result;
    }
    match collect(returned, reply_out.as_mut_slice(), None) {
        Ok(TRANSACT_BYTES) => result,
        Ok(_) => ERR_INVALID_ARG,
        Err(error) => error,
    }
}

pub fn block_transact(slot: u32, request: &[u8; 64], reply: &mut [u8; 64]) -> i64 {
    transact(SYS_BLOCK_TRANSACT, slot, request, reply)
}

/// A block request whose reply carries a sector behind the record (P5.4.2c).
///
/// [`transact`]'s reply is exactly 64 bytes, which is right for the store,
/// generation, and directory protocols and wrong for a read: the sector has
/// nowhere to go. On the retired kernel the caller passed a buffer pointer in
/// `buffer_phys` and the kernel wrote through it; there is no such ambient
/// addressing here, so the sector comes back in the same window the request
/// went out through, immediately after the reply record.
pub fn block_transact_sector(
    slot: u32,
    request: &[u8; 64],
    reply_out: &mut [u8; 64],
    sector: &mut [u8; 512],
) -> i64 {
    let mut staged = [0u8; 64 + 512];
    staged[..64].copy_from_slice(request);
    let transfer = match stage(staged[..64].as_ref(), &[]) {
        Ok(transfer) => transfer,
        Err(error) => return error,
    };
    let (result, returned) =
        match outcome(&call(SYS_BLOCK_TRANSACT, &[slot as Word, transfer as Word])) {
            Ok(pair) => pair,
            Err(error) => return error,
        };
    if result < 0 {
        return result;
    }
    match collect(returned, staged.as_mut_slice(), None) {
        Ok(length) if length >= TRANSACT_BYTES => {
            reply_out.copy_from_slice(&staged[..TRANSACT_BYTES]);
            if length == TRANSACT_BYTES + 512 {
                sector.copy_from_slice(&staged[TRANSACT_BYTES..TRANSACT_BYTES + 512]);
            }
            result
        }
        Ok(_) => ERR_INVALID_ARG,
        Err(error) => error,
    }
}

/// A block write, whose sector crosses in the request rather than the reply.
pub fn block_transact_write(
    slot: u32,
    request: &[u8; 64],
    sector: &[u8; 512],
    reply_out: &mut [u8; 64],
) -> i64 {
    let mut staged = [0u8; 64 + 512];
    staged[..64].copy_from_slice(request);
    staged[64..].copy_from_slice(sector);
    let transfer = match stage(staged.as_slice(), &[]) {
        Ok(transfer) => transfer,
        Err(error) => return error,
    };
    let (result, returned) =
        match outcome(&call(SYS_BLOCK_TRANSACT, &[slot as Word, transfer as Word])) {
            Ok(pair) => pair,
            Err(error) => return error,
        };
    if result < 0 {
        return result;
    }
    match collect(returned, reply_out.as_mut_slice(), None) {
        Ok(TRANSACT_BYTES) => result,
        Ok(_) => ERR_INVALID_ARG,
        Err(error) => error,
    }
}

pub fn store_transact(slot: u32, request: &[u8; 64], reply: &mut [u8; 64]) -> i64 {
    transact(SYS_STORE_TRANSACT, slot, request, reply)
}

pub fn generation_transact(slot: u32, request: &[u8; 64], reply: &mut [u8; 64]) -> i64 {
    transact(SYS_GENERATION_TRANSACT, slot, request, reply)
}

pub fn health_confirm(slot: u32) -> i64 {
    result_of(SYS_HEALTH_CONFIRM, &[slot as Word])
}

pub fn recovery_reconstruct(generation_control_slot: u32, block_slot: u32, flags: u32) -> i64 {
    result_of(
        SYS_RECOVERY_RECONSTRUCT,
        &[
            slot_pair(generation_control_slot, block_slot) as Word,
            flags as Word,
        ],
    )
}

pub fn generation_receive(receiver_slot: u32, transfer_slot: u32) -> i64 {
    result_of(
        SYS_GENERATION_RECEIVE,
        &[slot_pair(receiver_slot, transfer_slot) as Word],
    )
}

pub fn unhealthy() -> ! {
    let _ = call(SYS_UNHEALTHY, &[]);
    loop {
        core::hint::spin_loop();
    }
}

/// Write one diagnostic line through the root service.
///
/// **Not `seL4_DebugPutChar`, even where the kernel offers it.** That was the
/// implementation under `PRINTING`, and it emitted one syscall per byte — so
/// the root's own `debug_println!`, or another component's line, could land
/// mid-string and destroy a marker. A transcript would show ` QoS matched`
/// where `[fabric] QoS matched` was written, and whichever gate required that
/// marker failed on a boot that was otherwise correct (B18).
///
/// The root's graph loop is single-threaded and answers one request at a time,
/// so a line printed inside its `DebugWrite` arm cannot interleave with
/// anything. That makes atomicity structural rather than a matter of timing.
///
/// The cost is that this now needs a bound transfer window, where the direct
/// path needed nothing. Every launched component binds one before it runs, and
/// a task that has not is not yet in a state where its output would be
/// attributable.
pub fn debug_write(bytes: &[u8]) -> i64 {
    let transfer = match stage(bytes, &[]) {
        Ok(transfer) => transfer,
        Err(error) => return error,
    };
    let (operands, used) = payload_operands(0, transfer, bytes);
    result_of(super::SYS_DEBUG_WRITE, &operands[..used])
}
