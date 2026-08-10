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
use crate::ipc;
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
    loop {
        let reception = ipc::recv_request_with(context.endpoint, buffer);
        let Some((id, _)) = TaskId::from_badge(reception.badge) else {
            continue;
        };
        let Ok(request) = reception.request else {
            continue;
        };
        // SAFETY: the caller's contract. The table is only read.
        let windows = unsafe { &*context.windows };
        write_payload(
            windows.bound(id),
            &request.mrs[..request.len],
            &context.scratch,
            buffer,
        );
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
    let Ok(frame) =
        transfer_window::read_staged_array_with(window, words[1], words, scratch, buffer)
    else {
        return;
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
