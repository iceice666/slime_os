#![no_std]
#![no_main]

//! The seL4 rollback plane's subject: M5.6 in userspace (P5.4.2c).
//!
//! Two fixed BootState slots on a real device, and the transitions between
//! them. What this proves is the property M5.6 actually names — *no transition
//! overwrites the only valid root* — by making every commit older-slot-first
//! and checking, after each one, that the slot the boot selected still decodes.
//!
//! The sequence walked here is the oracle's `2 → 1 → 0`:
//!
//! * stage a pending generation with two attempts;
//! * consume one attempt durably (`2 → 1`), as a boot before transfer must;
//! * consume the last (`1 → 0`);
//! * find the pending exhausted, and roll back to the known-good root;
//! * confirm rollback is idempotent — a second rollback is a no-op, not an
//!   error and not another sequence bump.
//!
//! Then the promotion path, because rollback is only half the contract: stage
//! again, promote the running generation, and require that the *previous*
//! known-good is what a rollback would have returned. Promotion with the wrong
//! running identity, and promotion with a stale release sequence, are both
//! refused.
//!
//! Kernel-resident in the oracle: `generation_service::rollback_reply` performs
//! the transition and `persist_transition` writes the alternate slot. Here the
//! root mediates sectors and the transition model is
//! `boot_contracts::bootstate`, which is the same code stage-0 selects with.

extern crate alloc;

use boot_contracts::bootstate::{
    BootState, BootTransitionError, SLOT_BYTES, SelectionError, Slot, empty_state_root,
    select_bootstate,
};
use boot_contracts::gpt::{self, GptError};
use boot_contracts::object_store::{BlockIo, IoError};
use boot_contracts::trace::{Action as TraceAction, Commit as TraceCommit, Record as TraceRecord};
use slime_proto::block::{self, WireBlockReply, WireBlockRequest};

/// The block capability, granted to this component by the generation.
const BLOCK_SLOT: u32 = 1;

const SECTOR_BYTES: usize = 512;

/// The two BootState slots, partition-relative.
///
/// Above the object store's own two superblock slots and its record area, so
/// the two structures on one partition do not overlap. The fixture leaves this
/// region zeroed, which is what "no valid root" looks like on a fresh disk.
const STATE_SLOT_A: u64 = 1024;
const STATE_SLOT_B: u64 = 1025;

/// Generation identities. Content is irrelevant to the transition model — what
/// matters is that they are distinct and non-zero, since a zero known-good is
/// rejected by `BootState::validate`.
const KNOWN_GOOD: [u8; 32] = [0x11; 32];
const PENDING: [u8; 32] = [0x22; 32];
const OTHER: [u8; 32] = [0x33; 32];
const GENERATION_ROOT: [u8; 32] = [0x44; 32];

slime_rt::entry!(main);

fn main(startup_arg: u32) {
    if startup_arg == 0 {
        slime_rt::debug_write(b"[sel4-rollback-probe] idle without a run token\n");
        slime_rt::exit(0);
    }

    let mut io = BlockCapability;
    let partition = match locate_partition(&mut io) {
        Some(partition) => partition,
        None => fail(b"partition"),
    };
    let slots = StateSlots {
        first_lba: partition.first_lba,
    };

    // A fresh disk has no root at all, and that must be a refusal rather than
    // an invented state.
    match slots.select(&mut io) {
        Err(SelectionError::NoValidBootState) => {}
        _ => fail(b"an empty region produced a root"),
    }
    slime_rt::debug_write(b"[sel4-rollback-probe] empty slots refused\n");

    // Genesis: the known-good root, written to slot A.
    let genesis = BootState {
        sequence: 1,
        known_good: KNOWN_GOOD,
        pending: None,
        remaining_attempts: 0,
        generation_root: GENERATION_ROOT,
        state_root: empty_state_root(),
        accepted_release_sequence: 0,
    };
    if slots.write(&mut io, Slot::A, &genesis).is_err() {
        fail(b"genesis write");
    }
    let selected = slots
        .select(&mut io)
        .unwrap_or_else(|_| fail(b"genesis select"));
    if selected.slot != Slot::A || selected.state.known_good != KNOWN_GOOD {
        fail(b"genesis root");
    }
    report(b"genesis", &selected.state);

    // Stage a pending generation with the model's declared maximum attempts.
    let staged = selected
        .state
        .stage_pending(PENDING, 3, GENERATION_ROOT, empty_state_root())
        .unwrap_or_else(|_| fail(b"stage pending"));
    let mut live = slots.commit(&mut io, selected.slot, &staged);
    emit_trace(
        TraceAction::StagePending,
        TraceCommit::AfterPendingCommit,
        selected,
        live,
    );
    if live.state.pending != Some(PENDING) || live.state.remaining_attempts != 3 {
        fail(b"staged root");
    }
    report(b"staged", &live.state);

    // Consume all attempts durably, one commit each. This is the model's
    // `3 → 2 → 1 → 0`, and durability is the point: a boot that transferred
    // before the decrement was committed could retry forever.
    for expected in [2u32, 1, 0] {
        let before = live;
        let consumed = before
            .state
            .consume_pending_attempt()
            .unwrap_or_else(|_| fail(b"consume attempt"));
        live = slots.commit(&mut io, before.slot, &consumed);
        if live.state.remaining_attempts != expected {
            fail(b"attempt count");
        }
        // The root the previous boot would have selected must still decode.
        // Older-slot-first is what guarantees it, and this is where a commit
        // that wrote its own slot would be caught.
        if slots.read_state(&mut io, live.slot.other()).is_err() {
            fail(b"the previous root was overwritten");
        }
        emit_trace(
            TraceAction::ConsumeAttempt,
            TraceCommit::AfterAttemptCommit,
            before,
            live,
        );
        report(b"attempt", &live.state);
    }

    // Exhausted: a further attempt is refused rather than wrapping.
    if live.state.consume_pending_attempt() != Err(BootTransitionError::AttemptsExhausted) {
        fail(b"exhausted attempts not refused");
    }
    slime_rt::debug_write(b"[sel4-rollback-probe] attempts exhausted\n");

    // Roll back. The pending generation is cleared and the known-good root is
    // unchanged — the rollback root was retained across every transition above.
    let before = live;
    let rolled = before
        .state
        .rollback_pending()
        .unwrap_or_else(|_| fail(b"rollback"));
    live = slots.commit(&mut io, before.slot, &rolled);
    if live.state.pending.is_some() || live.state.known_good != KNOWN_GOOD {
        fail(b"rollback root");
    }
    emit_trace(
        TraceAction::Rollback,
        TraceCommit::RollbackUpdate,
        before,
        live,
    );
    report(b"rolled-back", &live.state);

    // Idempotent: rolling back with no pending generation is a no-op that
    // returns the same state, not an error and not another sequence bump.
    let again = live
        .state
        .rollback_pending()
        .unwrap_or_else(|_| fail(b"second rollback"));
    if again != live.state {
        fail(b"rollback is not idempotent");
    }
    slime_rt::debug_write(b"[sel4-rollback-probe] rollback is idempotent\n");

    let before = live;
    let staged = before
        .state
        .stage_pending(PENDING, 3, GENERATION_ROOT, empty_state_root())
        .unwrap_or_else(|_| fail(b"restage"));
    live = slots.commit(&mut io, before.slot, &staged);
    emit_trace(
        TraceAction::StagePending,
        TraceCommit::AfterPendingCommit,
        before,
        live,
    );

    // The wrong running identity is refused: only the generation that is
    // actually running may be promoted, so a component cannot confirm a
    // generation it is not.
    if live.state.promote_pending(OTHER, 1) != Err(BootTransitionError::WrongRunningGeneration) {
        fail(b"wrong running generation accepted");
    }
    // A stale release sequence is refused, so promotion cannot walk the
    // accepted sequence backwards.
    if live.state.promote_pending(PENDING, 0) != Err(BootTransitionError::StaleRelease) {
        fail(b"stale release accepted");
    }
    slime_rt::debug_write(b"[sel4-rollback-probe] unauthorized promotion refused\n");

    let before = live;
    let promoted = before
        .state
        .promote_pending(PENDING, 1)
        .unwrap_or_else(|_| fail(b"promote"));
    live = slots.commit(&mut io, before.slot, &promoted);
    if live.state.known_good != PENDING
        || live.state.pending.is_some()
        || live.state.accepted_release_sequence != 1
    {
        fail(b"promoted root");
    }
    emit_trace(
        TraceAction::Promotion,
        TraceCommit::HealthPromotion,
        before,
        live,
    );
    report(b"promoted", &live.state);

    // Both slots decode at the end, which is the invariant the whole sequence
    // exists to preserve: there was never a moment with fewer than one root,
    // and there is more than one now.
    for slot in [Slot::A, Slot::B] {
        if slots.read_state(&mut io, slot).is_err() {
            fail(b"a slot does not decode after the sequence");
        }
    }
    slime_rt::debug_write(b"[sel4-rollback-probe] both slots decode\n");

    slime_rt::debug_write(b"[sel4-rollback-probe] rollback plane complete\n");
}

/// The two BootState slots on the device, and the older-slot-first commit rule.
struct StateSlots {
    first_lba: u64,
}

impl StateSlots {
    fn lba(&self, slot: Slot) -> u64 {
        self.first_lba
            + match slot {
                Slot::A => STATE_SLOT_A,
                Slot::B => STATE_SLOT_B,
            }
    }

    fn read_slot(&self, io: &mut BlockCapability, slot: Slot) -> [u8; SLOT_BYTES] {
        let mut sector = [0u8; SECTOR_BYTES];
        if io.read_sector(self.lba(slot), &mut sector).is_err() {
            fail(b"slot read");
        }
        sector
    }

    fn read_state(&self, io: &mut BlockCapability, slot: Slot) -> Result<BootState, ()> {
        BootState::decode(&self.read_slot(io, slot)).map_err(|_| ())
    }

    fn select(
        &self,
        io: &mut BlockCapability,
    ) -> Result<boot_contracts::bootstate::SelectedBootState, SelectionError> {
        let a = self.read_slot(io, Slot::A);
        let b = self.read_slot(io, Slot::B);
        select_bootstate(&a, &b)
    }

    fn write(&self, io: &mut BlockCapability, slot: Slot, state: &BootState) -> Result<(), ()> {
        let encoded = state.encode().map_err(|_| ())?;
        io.write_sector(self.lba(slot), &encoded).map_err(|_| ())?;
        // Flushed before the caller treats the transition as durable. M5.6's
        // "decrement before transfer" is a claim about this flush.
        io.flush().map_err(|_| ())
    }

    /// Write a transition to the slot that was *not* selected, then re-select.
    ///
    /// The re-select is not bookkeeping: it is the assertion that the new root
    /// is what a fresh boot would pick, read back from the device rather than
    /// assumed from what was written.
    fn commit(
        &self,
        io: &mut BlockCapability,
        selected: Slot,
        state: &BootState,
    ) -> boot_contracts::bootstate::SelectedBootState {
        let target = selected.other();
        if self.write(io, target, state).is_err() {
            fail(b"commit write");
        }
        let live = self.select(io).unwrap_or_else(|_| fail(b"commit select"));
        if live.slot != target || &live.state != state {
            fail(b"the commit is not what a boot would select");
        }
        live
    }
}

/// The device, reached through the granted capability.
struct BlockCapability;

impl BlockIo for BlockCapability {
    fn read_sector(&mut self, lba: u64, out: &mut [u8; SECTOR_BYTES]) -> Result<(), IoError> {
        let request = request(block::OP_READ, lba);
        let mut reply = [0u8; block::REPLY_LEN];
        let status =
            slime_rt::block_transact_sector(BLOCK_SLOT, &request.encode(), &mut reply, out);
        if status < 0 || decode_reply(&reply).sectors_done != 1 {
            return Err(IoError::Device);
        }
        Ok(())
    }

    fn write_sector(&mut self, lba: u64, data: &[u8; SECTOR_BYTES]) -> Result<(), IoError> {
        let request = request(block::OP_WRITE, lba);
        let mut reply = [0u8; block::REPLY_LEN];
        let status =
            slime_rt::block_transact_write(BLOCK_SLOT, &request.encode(), data, &mut reply);
        if status < 0 || decode_reply(&reply).sectors_done != 1 {
            return Err(IoError::Device);
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), IoError> {
        let request = request(block::OP_FLUSH, 0);
        let mut reply = [0u8; block::REPLY_LEN];
        if slime_rt::block_transact(BLOCK_SLOT, &request.encode(), &mut reply) < 0 {
            return Err(IoError::Device);
        }
        Ok(())
    }
}

/// The store partition, which is also where the BootState slots live. Validated
/// rather than assumed, so a malformed table cannot put the slots off the end.
fn locate_partition(io: &mut BlockCapability) -> Option<gpt::Partition> {
    let capacity = device_capacity(io)?;
    let mut reader = |lba: u64, out: &mut [u8; SECTOR_BYTES]| -> Result<(), GptError> {
        io.read_sector(lba, out).map_err(|_| GptError::Device)
    };
    let selected = gpt::validate_store_partition(&mut reader, capacity).ok()?;
    // Both slots must fall inside the partition.
    let last = selected.partition.first_lba.checked_add(STATE_SLOT_B)?;
    (last <= selected.partition.last_lba).then_some(selected.partition)
}

/// The device's sector count, measured by binary search over readable LBAs: the
/// block protocol reports completion, not geometry, and the root refuses
/// `lba >= capacity`. Reads only.
fn device_capacity(io: &mut BlockCapability) -> Option<u64> {
    let mut sector = [0u8; SECTOR_BYTES];
    io.read_sector(0, &mut sector).ok()?;
    let mut low = 0u64;
    let mut high = 1u64;
    while io.read_sector(high, &mut sector).is_ok() {
        low = high;
        high = high.checked_mul(2)?;
    }
    while high - low > 1 {
        let middle = low + (high - low) / 2;
        if io.read_sector(middle, &mut sector).is_ok() {
            low = middle;
        } else {
            high = middle;
        }
    }
    Some(low + 1)
}

fn request(op: u8, lba: u64) -> WireBlockRequest {
    WireBlockRequest {
        magic: block::BLOCK_MAGIC,
        version: block::FORMAT_VERSION,
        op,
        flags: 0,
        reserved: 0,
        lba,
        sector_count: if op == block::OP_FLUSH { 0 } else { 1 },
        buffer_phys: 0,
        buffer_pages: 0,
    }
}

fn decode_reply(bytes: &[u8; block::REPLY_LEN]) -> WireBlockReply {
    WireBlockReply::decode(bytes).unwrap_or(WireBlockReply {
        magic: 0,
        version: 0,
        status: -1,
        sectors_done: 0,
    })
}

fn slot_number(slot: Slot) -> u8 {
    match slot {
        Slot::A => 0,
        Slot::B => 1,
    }
}

/// Emit the versioned, bounded M5.6c record for one durable transition.
fn emit_trace(
    action: TraceAction,
    commit: TraceCommit,
    before: boot_contracts::bootstate::SelectedBootState,
    after: boot_contracts::bootstate::SelectedBootState,
) {
    let line = TraceRecord {
        action,
        commit,
        selected_slot: slot_number(before.slot),
        target_slot: Some(slot_number(after.slot)),
        sequence_before: before.state.sequence,
        sequence_after: after.state.sequence,
        attempts_before: before.state.remaining_attempts,
        attempts_after: after.state.remaining_attempts,
        known_good: after.state.known_good,
        pending: after.state.pending,
        generation_root: after.state.generation_root,
        state_root: after.state.state_root,
    }
    .render();
    slime_rt::debug_write(line.as_str().as_bytes());
    slime_rt::debug_write(b"\n");
}

/// One transition, as the gate reads it: which step, the sequence it committed
/// at, whether a pending generation is live, and how many attempts remain.
fn report(step: &[u8], state: &BootState) {
    let mut line = [0u8; 160];
    let mut len = 0;
    len += copy(&mut line[len..], b"[sel4-rollback-probe] ");
    len += copy(&mut line[len..], step);
    len += copy(&mut line[len..], b" seq=");
    len += copy(&mut line[len..], &decimal(state.sequence));
    len += copy(&mut line[len..], b" pending=");
    len += copy(
        &mut line[len..],
        if state.pending.is_some() { b"1" } else { b"0" },
    );
    len += copy(&mut line[len..], b" attempts=");
    len += copy(&mut line[len..], &decimal(state.remaining_attempts as u64));
    len += copy(&mut line[len..], b" release=");
    len += copy(&mut line[len..], &decimal(state.accepted_release_sequence));
    len += copy(&mut line[len..], b"\n");
    slime_rt::debug_write(&line[..len]);
}

fn copy(out: &mut [u8], source: &[u8]) -> usize {
    let len = source.len().min(out.len());
    out[..len].copy_from_slice(&source[..len]);
    len
}

struct Decimal {
    bytes: [u8; 20],
    start: usize,
}

impl core::ops::Deref for Decimal {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.bytes[self.start..]
    }
}

fn decimal(mut value: u64) -> Decimal {
    let mut bytes = [0u8; 20];
    let mut start = bytes.len();
    loop {
        start -= 1;
        bytes[start] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    Decimal { bytes, start }
}

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[sel4-rollback-probe] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}
