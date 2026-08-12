//! The console dispatcher: a second root thread serving debug output (B41).
//!
//! Console and debug traffic used to arrive on the same badged endpoint as
//! lifecycle, storage, and fabric traffic, so a noisy client consumed the
//! highest-priority service loop and a console defect shared the system-wide
//! dispatcher's fault domain. Each process now holds a console endpoint of its
//! own, and this is what receives on it.
//!
//! # Why a thread rather than a poll
//!
//! Three single-threaded routes were tried and rejected:
//!
//! * polling the console endpoint with `seL4_NBRecv` before the blocking
//!   receive starves the console whenever the main endpoint is busy, and a
//!   client still blocks on its send until the next poll;
//! * `seL4_NBSend` never blocks the client but drops the message when nothing
//!   is receiving, and a console that silently loses lines is worse than one
//!   sharing a dispatcher;
//! * serving only the inline form does not help — `fits_inline` carries 16
//!   bytes and the overwhelming majority of debug lines are longer.
//!
//! # What this thread touches
//!
//! Deliberately almost nothing. It shares the root's VSpace and CSpace, and it
//! reads the window table to resolve a caller's staged payload. It never
//! allocates, never spawns, never mutates the graph, and holds no reference to
//! the tables the main dispatcher owns.
//!
//! Its scratch page is its own. [`crate::transfer_window::read_staged_array`]
//! maps a caller's frame into the root's VSpace at `ScratchPage::addr()`, so a
//! shared scratch address would let the two threads overwrite each other's
//! mapping; a second claimed root-image page costs one page and removes the
//! hazard entirely.

/// Authority to read decoded key events, matching the generation's
/// `inputRead` right.
const RIGHT_INPUT_READ: u64 = 1 << 23;

/// Authority to read sectors, held on a `Block` (P5.4.2c). Numbered as
/// `blockRead` in the generation's own rights table
/// (`scripts/build/build-generation.py`), which is the same numbering
/// `kernel/src/capability/mod.rs` uses.
pub const RIGHT_BLOCK_READ: u64 = 1 << 10;
/// Authority to write sectors and to flush. One bit for both, matching the
/// oracle: a caller that may change what is on the device may also ask for it
/// to be made durable, and a flush without writes is a no-op.
pub const RIGHT_BLOCK_WRITE: u64 = 1 << 11;

use crate::child_vspace::ScratchPage;
use crate::graph::{self, GraphTables};
use crate::ipc::{self, IpcError, Response};
use crate::task::{MAX_TASKS, TaskId};
use crate::transfer_window::{self, Window, WindowTable, descriptor_thread};

/// Stack for the console thread, in the root's own image so it is mapped
/// before the thread runs.
///
/// Two granules: the receive loop and its bounded copy need little, but the
/// thread's TLS block is allocated on this stack by
/// `TlsImage::with_initialize_on_stack`, so it has to hold that too.
#[repr(align(4096))]
pub struct ConsoleStack(pub [u8; 8192]);

impl ConsoleStack {
    pub const fn new() -> Self {
        Self([0; 8192])
    }
}

impl Default for ConsoleStack {
    fn default() -> Self {
        Self::new()
    }
}

/// Everything the console loop reads, gathered so the thread's entry point can
/// take a single pointer.
///
/// `windows` is a shared pointer to the main dispatcher's table. A window is
/// declared once during task construction and released once during
/// reclamation, never per request, so what this races is spawn and teardown
/// rather than steady traffic. See [`serve`] for the rule that makes that
/// safe.
pub struct ConsoleContext {
    pub endpoint: sel4::cap::Endpoint,
    pub scratch: ScratchPage,
    pub windows:
        *const WindowTable<{ crate::task::MAX_TASKS * crate::child_vspace::MAX_CHILD_THREADS }>,
    /// This thread's own IPC buffer, named on every invocation rather than
    /// discovered through the crate's ambient slot.
    ///
    /// There is one such slot per address space on this target, and a receive
    /// holds it borrowed for as long as it blocks — so two threads sharing it
    /// deadlock on the borrow, not on the endpoint. Naming the buffer here
    /// avoids the slot entirely.
    pub buffer: *mut sel4::IpcBuffer,
    /// The state answering input reads.
    ///
    /// Input moved here whole rather than being shared with the main
    /// dispatcher: its cursor is per-task session state that nothing else
    /// touches, so relocating it removes the coupling instead of guarding it.
    pub input: *mut ScriptedInput,
    /// The capability table, read to check the caller holds input authority.
    pub graph: *const GraphTables,
    /// The block devices this thread drives (B43).
    ///
    /// Owned here rather than shared with the main dispatcher: whoever answers
    /// block requests *is* the driver, and splitting the answer from the
    /// device tables would leave the authority in two places. The selector
    /// variant launches no components, so it keeps its own direct access and
    /// never constructs this thread.
    pub devices: *mut crate::device::BlockDevices,
    /// The namespace roots (B45). Owned here because inspect and commit are
    /// the only writers and both live here.
    pub namespaces: *mut crate::directory::Namespaces,
    /// The interned scopes, read-only here.
    ///
    /// `DirectoryDerive` stayed on the main dispatcher precisely because it is
    /// the only writer of *both* this table and the caller's `GraphTables`
    /// entry — and the main loop already writes that entry on `cap_drop` and
    /// on a spawn's result. Two threads writing one task's capability table
    /// is a data race, so derive did not move; a scope, once interned, is
    /// never mutated or freed, so reading it from here is sound.
    pub scopes: *const crate::graph::ScopeTable,
}

/// The scripted key source, owned by this thread (B41).
///
/// Per-task cursors, because the script is a *session* rather than a shared
/// queue: the root launches every declared component, so two copies of a
/// console component run and both read input. One cursor would let the
/// root-launched copy drain the script before the spawned one asked.
pub struct ScriptedInput {
    bytes: &'static [u8],
    cursors: [usize; MAX_TASKS],
}

impl ScriptedInput {
    pub const fn new(bytes: &'static [u8]) -> Self {
        Self {
            bytes,
            cursors: [0; MAX_TASKS],
        }
    }

    /// The next key for `task`, or the escape byte once its script is spent —
    /// an exhausted session ends the reader rather than blocking it forever.
    fn next_event(&mut self, task: TaskId) -> Option<u64> {
        let cursor = self.cursors.get_mut(task.0 as usize)?;
        match self.bytes.get(*cursor).copied() {
            Some(byte) => {
                *cursor += 1;
                Some(encode_key(byte))
            }
            None => Some(encode_key(0x1b)),
        }
    }
}

/// Serve console requests until the endpoint is destroyed.
///
/// # The window rule
///
/// A caller's window is resolved per message and used for the duration of one
/// copy. The main dispatcher releases a window during reclamation and the
/// frame itself is revoked by the task's cleanup record, so a release that
/// interleaves with a copy makes the *mapping* fail rather than the read: the
/// copy happens inside `read_staged_array`, which maps the frame and
/// propagates the kernel's refusal when the capability is gone. A torn read is
/// not reachable, because the unit of use is one map/copy/unmap and the kernel
/// serialises those against revocation.
///
/// The badge identifies the sender, and a console endpoint is minted per
/// process carrying that process's service badge, so a client cannot claim to
/// be another. It carries write authority only, so it cannot receive here
/// either.
///
/// # Safety
///
/// `context.windows` must point to a [`WindowTable`] that outlives this
/// thread, and `context.buffer` to this thread's own IPC buffer.
pub unsafe fn serve(context: &ConsoleContext) -> ! {
    // SAFETY: the buffer is this thread's own, named by `tcb_configure` and
    // referenced by nothing else, so the reborrow each iteration is unique.
    let buffer = unsafe { &mut *context.buffer };
    // SAFETY: the caller's contract; both outlive this thread, and the input
    // cursor is owned here rather than shared with the main dispatcher.
    let windows = unsafe { &*context.windows };
    let graph = unsafe { &*context.graph };
    let input = unsafe { &mut *context.input };
    let devices = unsafe { &mut *context.devices };
    let namespaces = unsafe { &mut *context.namespaces };
    let scopes = unsafe { &*context.scopes };

    // Both endpoints are bound to one notification so a single blocking wait
    // covers the pair: only one thread serves them, and a second blocking
    // receive would leave whichever endpoint it was not parked on unanswered.
    // One endpoint, two message kinds. A second endpoint would need a second
    // blocking receive and so a third thread; the console and input paths are
    // both "the terminal", so one queue between them is the honest shape and
    // the label says which.
    let mut pending: Option<Response> = None;
    loop {
        let received = match pending.take() {
            Some(reply) => ipc::reply_recv_console(context.endpoint, reply, buffer),
            None => ipc::recv_console(context.endpoint, buffer),
        };
        let Ok(message) = received else {
            continue;
        };
        let Some((id, _)) = TaskId::from_badge(message.badge) else {
            continue;
        };
        let thread = descriptor_thread(message.mrs[1]);
        let window = windows.bound(id, thread);
        match message.kind {
            ipc::ConsoleKind::Write => write_payload(
                window,
                &message.mrs[..message.len],
                &context.scratch,
                buffer,
            ),
            ipc::ConsoleKind::InputRead => {
                pending = Some(serve_input_read(graph, input, id, &message.mrs));
            }
            ipc::ConsoleKind::DirectoryInspect => {
                pending = Some(crate::directory::serve_directory_inspect(
                    graph,
                    namespaces,
                    scopes,
                    window,
                    &context.scratch,
                    id,
                    &message.mrs[..message.len],
                    buffer,
                ));
            }
            ipc::ConsoleKind::DirectoryCommit => {
                pending = Some(crate::directory::serve_directory_commit(
                    graph,
                    namespaces,
                    scopes,
                    window,
                    &context.scratch,
                    id,
                    &message.mrs[..message.len],
                    buffer,
                ));
            }
            ipc::ConsoleKind::BlockTransact => {
                pending = Some(serve_block_transact(
                    graph,
                    devices,
                    window,
                    &context.scratch,
                    id,
                    &message.mrs[..message.len],
                    buffer,
                ));
            }
        }
    }
}

/// Print one debug payload, staged inline or through the caller's window.
fn write_payload(
    window: Option<Window>,
    words: &[sel4::Word],
    scratch: &ScratchPage,
    buffer: &mut sel4::IpcBuffer,
) {
    if words.len() < 2 {
        return;
    }
    let frame =
        match transfer_window::read_staged_array_with(window, words[1], words, scratch, buffer) {
            Ok(frame) => frame,
            Err(error) => {
                sel4::debug_println!("SLIME_ROOT console staging refused: {error:?}");
                return;
            }
        };
    let bytes = frame.bytes();
    // One `debug_print!` for the whole payload, not `debug_println!`: the
    // component's bytes carry their own newline, and adding one would reflow
    // every marker the transcripts record.
    if let Ok(text) = core::str::from_utf8(bytes) {
        sel4::debug_print!("{text}");
    } else {
        // Not text. Refused explicitly rather than printed lossily, so a
        // component cannot inject bytes the transcript readers would
        // misinterpret.
        sel4::debug_println!("SLIME_ROOT console refused non-utf8 bytes={}", bytes.len());
    }
}

/// Encode one scripted byte as the runtime's key event, matching the oracle's
/// `syscall::encode_key_event` numbering so `slime_rt::input_read` decodes it
/// unchanged.
const fn encode_key(byte: u8) -> u64 {
    // The numbering is `syscall::decode` in `components/runtime`, read from the
    // decoder rather than guessed: 1..=13 are named keys, and a printable
    // character is `0x100 | ch`. Getting this wrong produced a session where
    // every keystroke arrived as a space, which is a decoder disagreement that
    // looks like a broken keyboard.
    let code: u64 = match byte {
        0x1b => 1,
        0x08 => 2,
        b'\t' => 3,
        b'\n' => 4,
        b' ' => 9,
        printable => 0x100 | printable as u64,
    };
    // Bit 32 is `pressed`. A script byte is a keypress, and without this every
    // event decoded as a *release* — which `dango.rs` discards, so the session
    // consumed its whole script and typed nothing.
    code | (1 << 32)
}

/// Answer one input read.
///
/// The caller must hold an input capability at the slot it names — denial is a
/// missing or wrong-kinded capability, checked against the graph table, not a
/// policy the dispatcher applies.
fn serve_input_read(
    graph: &GraphTables,
    input: &mut ScriptedInput,
    id: TaskId,
    words: &[sel4::Word],
) -> Response {
    let Some(table) = graph.get(id) else {
        return Response::error(IpcError::BadCapability);
    };
    let Ok(capability) = table.resolve(words[0] as u32, RIGHT_INPUT_READ) else {
        return Response::error(IpcError::BadCapability);
    };
    if !matches!(capability.resource, graph::Resource::Input) {
        return Response::error(IpcError::BadCapability);
    }
    match input.next_event(id) {
        Some(event) => Response::success(0, event),
        // Only a task id past the cursor table, which cannot happen for a task
        // this dispatcher is serving.
        None => Response::error(IpcError::WouldBlock),
    }
}

/// Answer `BlockTransact`: one sector-granular device request (B43).
///
/// Serving this on the console thread rather than the universal dispatcher is
/// the point — a block request is a *device* request, and a slow disk must not
/// hold up lifecycle, supervision, or fabric traffic. The device tables came
/// here with it, because authority over them is the thing that must not be
/// shared: whoever answers block requests is the driver.
///
/// The device index is the capability's, not the request's, so a component
/// holding the source cannot name the receiver. Read-only authority is
/// enforced against the grant's rights, so a read-only capability cannot be
/// talked into a write by any request field.
///
/// The reply's sector is written behind the record in the caller's own window:
/// the retired kernel took a buffer pointer and wrote through it, and there is
/// no such ambient addressing here. `bytes_len` is the record plus sector
/// representation in the reply this cutover answers with.
#[allow(clippy::too_many_lines)]
pub(crate) fn serve_block_transact(
    graph: &GraphTables,
    devices: &mut crate::device::BlockDevices,
    window: Option<transfer_window::Window>,
    scratch: &ScratchPage,
    id: TaskId,
    words: &[sel4::Word],
    buffer: &mut sel4::IpcBuffer,
) -> Response {
    use slime_proto::block::{
        BLOCK_MAGIC, FORMAT_VERSION, OFF_REPLY_MAGIC, OFF_REPLY_SECTORS_DONE, OFF_REPLY_STATUS,
        OFF_REPLY_VERSION, OP_FLUSH, OP_READ, OP_WRITE, REPLY_LEN, WireBlockRequest,
    };

    let Some(slot) = words.first().map(|slot| *slot as u32) else {
        return Response::error(IpcError::InvalidLength);
    };
    let Some(table) = graph.get(id) else {
        return Response::error(IpcError::BadCapability);
    };
    let Some(capability) = table.get(slot) else {
        return Response::error(IpcError::BadCapability);
    };
    let graph::Resource::Block { device: index } = capability.resource else {
        return Response::error(IpcError::BadCapability);
    };
    let index = index as usize;
    // Which device: the capability's own index, placed by the generation. A
    // component holding the source cannot name the receiver, because the index
    // is in the capability rather than in the request.
    let Some(device) = devices.get_mut(index) else {
        // Authority the boot could not back: the generation granted the device
        // but none was attached. A bounded refusal, not a fault.
        return Response::error(IpcError::UnsupportedOperation);
    };
    let Some(transfer) = words.get(1).copied() else {
        return Response::error(IpcError::InvalidLength);
    };
    // The wide reader: a write carries its sector behind the 64-byte record, so
    // the request is 576 bytes and the *message* reader's 64-byte bound would
    // refuse it. `read_staged_array` refuses any descriptor naming a
    // capability, which is the rule this operation needs anyway.
    let frame =
        match transfer_window::read_staged_array_with(window, transfer, words, scratch, buffer) {
            Ok(frame) => frame,
            Err(error) => return Response::error(error),
        };
    let Some(request) = WireBlockRequest::decode(frame.bytes()) else {
        return Response::error(IpcError::InvalidLength);
    };
    if request.magic != BLOCK_MAGIC || request.version != FORMAT_VERSION {
        return Response::error(IpcError::InvalidLength);
    }
    let required = match request.op {
        OP_READ => RIGHT_BLOCK_READ,
        OP_WRITE | OP_FLUSH => RIGHT_BLOCK_WRITE,
        _ => return Response::error(IpcError::InvalidLength),
    };
    if capability.rights & required == 0 {
        sel4::debug_println!(
            "SLIME_GRAPH block refused task={} op={} class=rights",
            id.0,
            request.op,
        );
        return Response::error(IpcError::BadCapability);
    }
    // One sector per request. The reply carries `sectors_done`, so a partial
    // completion is representable — but nothing in this cutover produces one,
    // and accepting a count this driver would silently truncate is worse than
    // refusing it.
    if request.op != OP_FLUSH && request.sector_count != 1 {
        return Response::error(IpcError::InvalidLength);
    }

    let mut sector = [0u8; crate::virtio_blk::SECTOR_BYTES];
    let outcome = match request.op {
        OP_READ => device.read_sector(request.lba, &mut sector),
        OP_WRITE => {
            let bytes = frame.bytes();
            let start = slime_proto::block::REQUEST_LEN;
            match bytes.get(start..start + crate::virtio_blk::SECTOR_BYTES) {
                Some(payload) => {
                    sector.copy_from_slice(payload);
                    device.write_sector(request.lba, &sector)
                }
                None => return Response::error(IpcError::InvalidLength),
            }
        }
        _ => device.flush(),
    };
    let (status, sectors_done) = match outcome {
        Ok(()) => (0i32, if request.op == OP_FLUSH { 0 } else { 1u32 }),
        Err(error) => {
            sel4::debug_println!(
                "SLIME_GRAPH block failed task={} op={} lba={} {error:?}",
                id.0,
                request.op,
                request.lba,
            );
            (-1i32, 0)
        }
    };
    // The device index is part of the record: a plane holding two device
    // capabilities cannot otherwise tell which one answered, and "the right
    // device served this" is exactly what multi-device selection claims.
    sel4::debug_println!(
        "SLIME_GRAPH block served task={} device={index} op={} lba={} status={status} sectors={sectors_done}",
        id.0,
        request.op,
        request.lba,
    );

    // The reply is the 64-byte record, and for a successful read the sector
    // follows it in the caller's window.
    //
    // Written as one region rather than a `StagedFrame`, whose bound is
    // `MAX_STAGED_BYTES` — the *message* bound, 64 bytes. A sector is not a
    // message: it crosses no channel and is bounded by the window, exactly as
    // a console line is on its own endpoint. `write_staged_region` is the same
    // write path without the message-shaped ceiling.
    let mut reply = [0u8; REPLY_LEN + crate::virtio_blk::SECTOR_BYTES];
    reply[OFF_REPLY_MAGIC..OFF_REPLY_MAGIC + 4].copy_from_slice(&BLOCK_MAGIC.to_le_bytes());
    reply[OFF_REPLY_VERSION..OFF_REPLY_VERSION + 4].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    reply[OFF_REPLY_STATUS..OFF_REPLY_STATUS + 4].copy_from_slice(&status.to_le_bytes());
    reply[OFF_REPLY_SECTORS_DONE..OFF_REPLY_SECTORS_DONE + 4]
        .copy_from_slice(&sectors_done.to_le_bytes());
    let length = if request.op == OP_READ && status == 0 {
        reply[REPLY_LEN..].copy_from_slice(&sector);
        reply.len()
    } else {
        REPLY_LEN
    };
    match transfer_window::write_staged_region_with(window, &reply[..length], scratch, buffer) {
        Ok(descriptor) => Response::success(length as i64, descriptor),
        Err(error) => Response::error(error),
    }
}
