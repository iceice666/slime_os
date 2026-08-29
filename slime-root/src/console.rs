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

use crate::child_vspace::ScratchPage;
use crate::ipc::{self, IpcError, Response};
use crate::task::{MAX_TASKS, TaskId, TaskTable};
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
    /// Task records, read to resolve the caller's typed authority.
    pub tasks: *const TaskTable<MAX_TASKS>,
    /// The namespace roots (B45). Owned here because inspect and commit are
    /// the only writers and both live here.
    pub namespaces: *mut crate::directory::Namespaces,
    /// The interned scopes, read-only here. Directory derivation remains on the
    /// owning dispatcher because it mutates both this service state and one
    /// task's authority table.
    pub scopes: *const crate::directory::ScopeTable,
}

/// The terminal key source owned by this thread (B41).
///
/// Per-task cursors, because a script is a *session* rather than a shared
/// queue: the root launches every declared component, so two copies of a
/// console component run and both read input. One cursor would let the
/// root-launched copy drain the script before the spawned one asked.
///
/// An empty byte slice ordinarily reports `WouldBlock`. The QEMU product build
/// may additionally attach its polling PL011 receive path; deterministic plane
/// scripts remain per-task and take precedence over that live source.
pub struct ScriptedInput {
    bytes: &'static [u8],
    cursors: [usize; MAX_TASKS],
    uart: Option<crate::device::Pl011Input>,
}

impl ScriptedInput {
    pub const fn new(bytes: &'static [u8]) -> Self {
        Self {
            bytes,
            cursors: [0; MAX_TASKS],
            uart: None,
        }
    }

    pub fn with_pl011(mut self, uart: crate::device::Pl011Input) -> Self {
        self.uart = Some(uart);
        self
    }

    /// The next key for `task`. Finite non-empty scripts end with Escape so
    /// their test session terminates deterministically; an attached UART is a
    /// live queue and remains empty as `WouldBlock`.
    fn next_event(&mut self, task: TaskId) -> Option<u64> {
        let cursor = self.cursors.get_mut(task.0 as usize)?;
        if !self.bytes.is_empty() {
            return match self.bytes.get(*cursor).copied() {
                Some(byte) => {
                    *cursor += 1;
                    Some(encode_key(byte))
                }
                None => Some(encode_key(0x1b)),
            };
        }
        self.uart
            .as_ref()?
            .poll_byte()
            .map(normalize_terminal_byte)
            .map(encode_key)
    }
}

/// QEMU's stdio serial backend sends Enter as carriage return and terminals
/// commonly send Delete for Backspace. Slisp's input ABI names newline and
/// backspace explicitly, so normalize those host encodings at the driver edge.
const fn normalize_terminal_byte(byte: u8) -> u8 {
    match byte {
        b'\r' => b'\n',
        0x7f => 0x08,
        byte => byte,
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
    // SAFETY: the caller's contract; these pointers outlive this thread, and
    // the input cursor and namespace table are owned here rather than shared
    // with the main dispatcher.
    let windows = unsafe { &*context.windows };
    let tasks = unsafe { &*context.tasks };
    let input = unsafe { &mut *context.input };
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
                pending = Some(serve_input_read(tasks, input, id, &message.mrs));
            }
            ipc::ConsoleKind::DirectoryInspect => {
                pending = Some(crate::directory::serve_directory_inspect(
                    tasks,
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
                    tasks,
                    namespaces,
                    scopes,
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
fn serve_input_read<const TASKS: usize>(
    tasks: &TaskTable<TASKS>,
    input: &mut ScriptedInput,
    id: TaskId,
    words: &[sel4::Word],
) -> Response {
    let Some(table) = tasks.authority(id) else {
        return Response::error(IpcError::BadCapability);
    };
    if table.resolve_input(words[0] as u32).is_err() {
        return Response::error(IpcError::BadCapability);
    }
    match input.next_event(id) {
        Some(event) => Response::success(0, event),
        None => Response::error(IpcError::WouldBlock),
    }
}
