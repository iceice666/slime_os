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

use crate::child_vspace::ScratchPage;
use crate::graph::{self, GraphTables};
use crate::ipc::{self, IpcError, Response};
use crate::task::{MAX_TASKS, TaskId};
use crate::transfer_window::{self, Window, WindowTable};

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
    pub windows: *const WindowTable<MAX_TASKS>,
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
        match message.kind {
            ipc::ConsoleKind::Write => write_payload(
                windows.bound(id),
                &message.mrs[..message.len],
                &context.scratch,
                buffer,
            ),
            ipc::ConsoleKind::InputRead => {
                pending = Some(serve_input_read(graph, input, id, &message.mrs));
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
