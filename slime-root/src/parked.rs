//! Held replies for operations that block.
//!
//! `recv` and `wait` are calls: the component invokes the root endpoint and
//! stays blocked in `seL4_Call` until the root answers. When there is nothing
//! to answer with yet — an empty queue, no ready wait source — the root must
//! hold the caller's reply authority and use it later, once a peer's `send` or
//! death makes the answer exist.
//!
//! Under the non-MCS kernel this root task is configured for, `seL4_Reply` uses
//! an *implicit* reply capability that the current thread's next `seL4_Recv`
//! overwrites. Holding one across the service loop's next iteration therefore
//! means moving it somewhere durable first: `seL4_CNode_SaveCaller` transfers it
//! into a root CSlot, and a later `seL4_Send` on that slot delivers the reply.
//!
//! Two consequences shape how this module is used, and both are why the save
//! happens at the top of the dispatch rather than at the point the operation
//! discovers it must block:
//!
//! - the save must happen before any intervening invocation, because the reply
//!   slot belongs to the calling thread and is transient;
//! - a saved reply that turns out not to be needed has to be released, or the
//!   root leaks a CSlot per non-blocking call — and the boot's cleanup
//!   accounting would never reach zero.
//!
//! So the dispatcher saves speculatively for every operation that *could* park,
//! then either [`Parked::commit`]s the save into a held reply or
//! [`Parked::discard`]s it and answers over the implicit slot as usual.

use crate::ipc::{IpcError, Response};
use crate::object_allocator::ObjectAllocator;
use crate::task::TaskId;

/// Tasks that may hold a parked reply at once. One per task, since a component
/// is single-threaded and blocked while parked.
pub const MAX_PARKED: usize = crate::task::MAX_TASKS;

/// Why a task is parked, so the right thing wakes it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParkReason {
    /// Blocked in `wait` on a source set. The set itself lives in the wait
    /// registration; this only records that the task is parked in a wait, whose
    /// answer carries no payload.
    ///
    /// The only variant since P5.5.1. There was a `Receive` beside it, because
    /// `recv` parked the caller and the wake had to hand it the message. That
    /// made any component sweeping several sources freeze at the first empty
    /// one, so `recv` became non-blocking — as `kernel/src/ipc/mod.rs` always
    /// had it — and `wait` is now the only operation that parks. The enum is
    /// kept rather than collapsed into a bare marker: it names *why* a reply is
    /// held, and a second reason is a plausible future rather than a
    /// contradiction.
    Wait,
}

/// A reply capability saved out of the implicit slot, and the task it answers.
#[derive(Clone, Copy)]
struct Held {
    task: TaskId,
    reason: ParkReason,
    slot: sel4::init_thread::Slot<sel4::cap_type::Unspecified>,
}

/// A reply authority saved but not yet decided about.
///
/// Deliberately not `Copy` and deliberately without a `Drop` impl: the
/// dispatcher must resolve every one of these by calling [`Parked::commit`] or
/// [`Parked::discard`], and a type that could silently vanish would hide a
/// leaked CSlot. `no_std` gives no destructor that can fail, so the discipline
/// is the review-visible one of consuming the value.
#[must_use = "a saved reply must be committed or discarded, or its CSlot leaks"]
pub struct SavedReply {
    task: TaskId,
    slot: sel4::init_thread::Slot<sel4::cap_type::Unspecified>,
}

/// Replies the root holds on behalf of blocked tasks.
pub struct ParkedReplies {
    entries: [Option<Held>; MAX_PARKED],
    len: usize,
    /// CSlots handed back after a discarded or delivered reply, so the boot's
    /// accounting can show the save path is not a leak.
    recycled: usize,
}

impl ParkedReplies {
    pub const fn new() -> Self {
        Self {
            entries: [None; MAX_PARKED],
            len: 0,
            recycled: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn recycled(&self) -> usize {
        self.recycled
    }

    /// Whether `task` is currently parked.
    pub fn is_parked(&self, task: TaskId) -> bool {
        self.entries.iter().flatten().any(|held| held.task == task)
    }

    /// Why `task` is parked, if it is.
    pub fn reason(&self, task: TaskId) -> Option<ParkReason> {
        self.entries
            .iter()
            .flatten()
            .find(|held| held.task == task)
            .map(|held| held.reason)
    }

    /// Every task still holding a parked reply, in table order.
    ///
    /// For the terminal marker only. A healthy graph ends with this empty: a
    /// task parked at teardown is one the root owes an answer it never
    /// delivered, which is invisible in the `parked=` count alone — that number
    /// says *how many* are owed, and diagnosing one needs to know *which*.
    /// Backlog **B28** is exactly that case.
    pub fn tasks(&self) -> impl Iterator<Item = TaskId> + '_ {
        self.entries.iter().flatten().map(|held| held.task)
    }

    /// Save the calling thread's implicit reply authority into a fresh root
    /// CSlot.
    ///
    /// Must be the first invocation after receiving a request that might park:
    /// the implicit slot is transient and the next `seL4_Recv` clobbers it.
    pub fn save(
        &mut self,
        allocator: &mut ObjectAllocator,
        task: TaskId,
    ) -> Result<SavedReply, IpcError> {
        let slot = allocator
            .reserve_slot::<sel4::cap_type::Unspecified>()
            .map_err(|_| IpcError::DestinationSlotsExhausted)?;
        sel4::init_thread::slot::CNODE
            .cap()
            .absolute_cptr(slot.cptr())
            .save_caller()
            .map_err(|_| IpcError::TransferFailed)?;
        Ok(SavedReply { task, slot })
    }

    /// Keep `saved` as the answer `task` is waiting for.
    ///
    /// On failure the save is handed **back** rather than dropped, because the
    /// caller is blocked in a call and the only thing that can still answer it
    /// is this capability. Consuming it here would delete the one reply
    /// authority in existence and leave that component blocked forever — a hang
    /// with no marker, since the root would carry on serving everyone else.
    pub fn commit(
        &mut self,
        saved: SavedReply,
        reason: ParkReason,
    ) -> Result<(), (SavedReply, IpcError)> {
        if self.is_parked(saved.task) {
            // A task blocked in a call cannot issue a second one, so this is a
            // badge or bookkeeping defect rather than a component's doing.
            return Err((saved, IpcError::WaiterConflict));
        }
        let Some(entry) = self.entries.iter_mut().find(|entry| entry.is_none()) else {
            return Err((saved, IpcError::WaitSetFull));
        };
        *entry = Some(Held {
            task: saved.task,
            reason,
            slot: saved.slot,
        });
        self.len += 1;
        Ok(())
    }

    /// Drop a save the operation turned out not to need, freeing its CSlot.
    ///
    /// The implicit reply slot is *not* consumed by `save_caller` failing to be
    /// used — but it has already been moved out, so the caller must answer over
    /// the saved capability rather than `ipc::reply`. [`Self::answer_saved`] is
    /// that path.
    pub fn discard(&mut self, saved: SavedReply) {
        self.release_slot(saved.slot);
    }

    /// Answer over a saved reply capability without ever parking.
    ///
    /// Once `save_caller` has moved the authority, `seL4_Reply` has nothing to
    /// answer with, so every non-parking path for a parkable operation replies
    /// through here.
    pub fn answer_saved(&mut self, saved: SavedReply, response: Response) {
        send_reply(saved.slot, response);
        self.release_slot(saved.slot);
    }

    /// Deliver `response` to a parked task and forget the reply.
    ///
    /// The CSlot is released after the send, not merely counted. `recycled` used
    /// to be bumped here without a `delete_slot`, which made this the one path
    /// of three that leaked: `answer_saved` and `discard` both go through
    /// [`Self::release_slot`], and only this one did its own accounting. A boot
    /// that parks and wakes repeatedly therefore burned one root CSlot per wake
    /// while reporting them all as recycled, so the terminal `replies=` figure
    /// asserted the opposite of what happened.
    pub fn wake(&mut self, task: TaskId, response: Response) -> bool {
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.is_some_and(|held| held.task == task))
        else {
            return false;
        };
        let held = entry.take().expect("just matched");
        self.len -= 1;
        send_reply(held.slot, response);
        self.release_slot(held.slot);
        true
    }

    /// Forget a dying task's parked reply without answering it. The task is
    /// being torn down, so there is no one left to receive the answer.
    pub fn abandon(&mut self, task: TaskId) -> bool {
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.is_some_and(|held| held.task == task))
        else {
            return false;
        };
        let held = entry.take().expect("just matched");
        self.len -= 1;
        delete_slot(held.slot);
        self.recycled += 1;
        true
    }

    fn release_slot(&mut self, slot: sel4::init_thread::Slot<sel4::cap_type::Unspecified>) {
        delete_slot(slot);
        self.recycled += 1;
    }
}

impl Default for ParkedReplies {
    fn default() -> Self {
        Self::new()
    }
}

/// Send one reply message over a saved reply capability. Same two registers
/// `ipc::reply` writes, so a parked answer and an immediate one are
/// indistinguishable to the component.
fn send_reply(slot: sel4::init_thread::Slot<sel4::cap_type::Unspecified>, response: Response) {
    let words = [response.result as sel4::Word, response.aux];
    let info = sel4::MessageInfoBuilder::default()
        .length(words.len())
        .build();
    sel4::with_ipc_buffer_mut(|buffer| {
        buffer.msg_regs_mut()[..words.len()].copy_from_slice(&words);
    });
    slot.cap().send(info);
}

/// Delete a reply CSlot so the index can be reused by nothing and the
/// capability stops naming a thread.
fn delete_slot(slot: sel4::init_thread::Slot<sel4::cap_type::Unspecified>) {
    let _ = sel4::init_thread::slot::CNODE
        .cap()
        .absolute_cptr(slot.cptr())
        .delete();
}
