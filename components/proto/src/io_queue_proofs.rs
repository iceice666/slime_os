//! Kani proof harnesses for the IO0 substrate's pure logic.
//!
//! # What this layer owns, and why it is not the model
//!
//! `contracts/io-queue/model/io-queue.zt` checks an *abstraction* of this
//! substrate: it enumerates every interleaving of a small transition system and
//! settles reachability, single-assignment, and lease-lifetime questions no
//! single QEMU schedule can answer. It deliberately disclaims the wire layer --
//! sequence encoding, slot arithmetic, and bounds -- because a model that
//! restated field offsets would be a second, drifting copy of the contract.
//!
//! That disclaimer is what this file closes. Kani checks *this* code rather
//! than an abstraction of it, over all inputs of the declared types rather than
//! the handful a `#[test]` names. The two layers answer different questions and
//! neither subsumes the other: the model knows about time and this does not;
//! this knows about `u64` wraparound and the model cannot express it.
//!
//! # Why these functions
//!
//! Every harness below targets an operation whose precondition is established
//! by *different code than the operation itself*. That gap is where a bounds
//! bug survives review: each side looks locally correct.
//!
//! - [`queue_slot_index`] masks with `slot_count - 1`. That is modular
//!   reduction only when `slot_count` is a power of two, and it underflows at
//!   zero. Its own doc comment says the precondition "is validated by different
//!   code". The harness pins the two halves together.
//! - `valid_queue_header` is the sole place cursor ordering is established, and
//!   `Queue::submitted`/`completions_pending` subtract those cursors with no
//!   further check. Release builds wrap on overflow, so if the validator ever
//!   admitted `tail > head` the occupancy would read as an enormous positive
//!   number and the ring-full check would pass while the ring was full.
//! - `terminal_state_for_status` decides whether a request can be settled at
//!   all. If some status were both `valid_completion_status` and yielded
//!   `None`, a driver could produce a completion the client must refuse to
//!   settle -- the leaked lease the whole substrate exists to prevent.
//! - `valid_buffer_slice` is the last check before a device is programmed with
//!   an offset and length. `offset + length` is precisely where a hostile
//!   descriptor aims.
//!
//! # Bounds and what they cost
//!
//! Slot counts are quantified over the admissible range by proving the property
//! for a symbolic `slot_count` constrained to it, not by enumerating instances.
//! Where an array must be sized at compile time -- the `Outstanding<N>` table --
//! `N` is fixed small, and that is a genuine limit stated rather than hidden:
//! those harnesses are evidence about `Outstanding<2>`, and the model covers
//! the capacity-independent lifetime argument.
//!
//! # Reading a failure
//!
//! Kani reports a concrete counterexample. A failure here is a real input the
//! shipped code mishandles, not a modelling artifact -- there is no model. The
//! source compiled by these proofs is `components/proto/src/lib.rs` itself,
//! reached through `verification/io-proofs/Cargo.toml`.

use crate::io_queue;
use crate::io_queue_ring::{Outstanding, QueueError};
use crate::{
    queue_slot_index, terminal_request_state, terminal_state_for_status, valid_buffer_slice,
    valid_completion_status, valid_queue_header,
};

/// A `slot_count` the substrate would actually accept.
///
/// Constrained rather than enumerated: the proofs below hold for every
/// admissible depth, not for a list of them.
fn any_admissible_slot_count() -> usize {
    let slot_count: usize = kani::any();
    kani::assume(
        (io_queue::MIN_QUEUE_SLOTS..=io_queue::MAX_QUEUE_SLOTS).contains(&slot_count)
            && slot_count.is_power_of_two(),
    );
    slot_count
}

/// A header that `valid_queue_header` accepts, built from symbolic cursors.
///
/// Every field the validator inspects is symbolic; the assumption is only that
/// the validator said yes. So a property proved from here is a property of
/// *every* header any peer could present and be believed.
fn any_valid_header(slot_count: usize) -> io_queue::WireQueueHeader {
    let header = io_queue::WireQueueHeader {
        magic: io_queue::QUEUE_MAGIC,
        version: io_queue::FORMAT_VERSION,
        slot_count: slot_count as u32,
        request_slot_len: io_queue::REQUEST_SLOT_LEN as u32,
        completion_slot_len: io_queue::COMPLETION_SLOT_LEN as u32,
        client_reserved: [0; 4],
        submit_head: kani::any(),
        complete_tail: kani::any(),
        client_padding: [0; 24],
        driver_state: kani::any(),
        driver_reserved: [0; 4],
        epoch: kani::any(),
        complete_head: kani::any(),
        submit_tail: kani::any(),
        driver_padding: [0; 32],
    };
    kani::assume(valid_queue_header(&header, slot_count));
    header
}

/// Every index a validated ring can produce is a slot that exists.
///
/// This is the bounds check behind every `request_range`/`completion_range`
/// call. `sequence` is unconstrained across all of `u64`, so this also covers
/// the wraparound a long-lived queue would eventually reach.
#[kani::proof]
fn queue_slot_index_is_in_bounds() {
    let slot_count = any_admissible_slot_count();
    let sequence: u64 = kani::any();
    assert!(queue_slot_index(sequence, slot_count) < slot_count);
}

/// The mask is real modular reduction, not merely something small.
///
/// A masking bug that still landed in range would keep the bounds proof above
/// green while silently aliasing two live sequences onto one slot -- which is
/// the overwrite the model forbids at the abstract level.
#[kani::proof]
fn queue_slot_index_is_modular() {
    let slot_count = any_admissible_slot_count();
    let sequence: u64 = kani::any();
    let index = queue_slot_index(sequence, slot_count);
    assert!(index == (sequence % slot_count as u64) as usize);
}

/// Consecutive sequences occupy distinct slots within one ring's depth.
///
/// This is what makes "a slot is free once its predecessor is consumed" true.
/// If two sequences less than `slot_count` apart could share a slot, the
/// occupancy check would be guarding the wrong thing.
#[kani::proof]
fn distinct_live_sequences_occupy_distinct_slots() {
    let slot_count = any_admissible_slot_count();
    let a: u64 = kani::any();
    let b: u64 = kani::any();
    kani::assume(a != b);
    // Both within one ring's worth of each other: the window a correct peer
    // can have outstanding at once.
    kani::assume(a < b && b - a < slot_count as u64);
    assert!(queue_slot_index(a, slot_count) != queue_slot_index(b, slot_count));
}

/// A believed header never makes an occupancy subtraction underflow.
///
/// `Queue::submitted` and `Queue::completions_pending` subtract cursors
/// directly. Release profiles wrap, so an admitted `tail > head` would not
/// panic -- it would report a near-`u64::MAX` occupancy, and every
/// "ring is full" comparison downstream would read false. This proves the
/// validator is strong enough that the unchecked subtractions are safe.
#[kani::proof]
fn validated_header_cursors_never_underflow() {
    let slot_count = any_admissible_slot_count();
    let header = any_valid_header(slot_count);

    assert!(header.submit_tail <= header.submit_head);
    assert!(header.complete_tail <= header.complete_head);

    // The values `submitted()` and `completions_pending()` compute.
    let submitted = header.submit_head - header.submit_tail;
    let pending = header.complete_head - header.complete_tail;

    // Bounded by the ring, so the full-checks in `submit`/`complete` compare
    // meaningful quantities.
    assert!(submitted <= slot_count as u64);
    assert!(pending <= slot_count as u64);
}

/// A believed header never claims more answers than questions.
///
/// A completion the client cannot match to a submission is one it would settle
/// against a request it never made.
#[kani::proof]
fn validated_header_never_has_excess_completions() {
    let slot_count = any_admissible_slot_count();
    let header = any_valid_header(slot_count);
    assert!(header.complete_head <= header.submit_head);
}

/// An active queue always names a real driver incarnation.
///
/// Epoch zero means "no driver has claimed this queue", so an active queue at
/// epoch zero would admit work no incarnation owns and no incarnation would
/// ever settle.
#[kani::proof]
fn active_header_has_nonzero_epoch() {
    let slot_count = any_admissible_slot_count();
    let header = any_valid_header(slot_count);
    kani::assume(header.driver_state == io_queue::DRIVER_ACTIVE);
    assert!(header.epoch != 0);
}

/// The mapping size arithmetic never silently wraps.
///
/// `mapping_bytes` is what every bounds check is taken against, so an overflow
/// here would produce a small `required` and admit a mapping far too short.
#[kani::proof]
fn mapping_bytes_is_exact_for_admissible_depths() {
    let slot_count = any_admissible_slot_count();
    let bytes = crate::io_queue_ring::mapping_bytes(slot_count)
        .expect("an admissible depth always has a size");

    // The completion ring's last byte is inside the mapping.
    let completion_base = io_queue::QUEUE_HEADER_LEN + slot_count * io_queue::REQUEST_SLOT_LEN;
    let end = completion_base + slot_count * io_queue::COMPLETION_SLOT_LEN;
    assert!(end == bytes);

    // Every slot of both rings lies strictly inside, and the two rings do not
    // overlap: the request ring ends exactly where the completion ring starts.
    assert!(io_queue::QUEUE_HEADER_LEN < completion_base);
    assert!(completion_base < bytes);
}

/// Status-to-terminal-state agrees exactly with status validity.
///
/// Both directions matter. A defined status with no terminal state is a
/// completion the client must refuse to settle, which leaks the lease. An
/// undefined status *with* one would let an unknown word settle a request.
#[kani::proof]
fn terminal_state_is_total_over_defined_statuses() {
    let status: u32 = kani::any();
    match terminal_state_for_status(status) {
        Some(state) => {
            assert!(valid_completion_status(status));
            // Whatever it maps to must actually be terminal, or the request
            // would be settled into a state it can still leave.
            assert!(terminal_request_state(state));
        }
        None => assert!(!valid_completion_status(status)),
    }
}

/// A slice the validator accepts names bytes inside the lease's mapping.
///
/// This is the last gate before a device is programmed. `offset + length` is
/// checked here over all of `u64`, including the pairs that overflow.
#[kani::proof]
fn accepted_slice_stays_inside_the_mapping() {
    let mapped_len: u64 = kani::any();
    let slice = io_queue::WireBufferSlice {
        buffer: kani::any(),
        lease: kani::any(),
        offset: kani::any(),
        length: kani::any(),
        direction: kani::any(),
        reserved: [0; 4],
    };
    kani::assume(valid_buffer_slice(&slice, mapped_len));

    match slice.direction {
        io_queue::DIRECTION_NONE => {
            // A control request touches no buffer, so it may not carry a
            // lease identity the substrate would have to settle.
            assert!(slice.buffer == 0 && slice.lease == 0);
            assert!(slice.offset == 0 && slice.length == 0);
        }
        _ => {
            // A transfer names a real lease and a non-empty range.
            assert!(slice.buffer != 0 && slice.lease != 0 && slice.length != 0);
            // The sum does not overflow, and the range is inside the mapping.
            let end = slice.offset.checked_add(slice.length);
            assert!(end.is_some());
            assert!(end.expect("checked just above") <= mapped_len);
        }
    }
}

/// A slice with an unknown direction is always refused.
///
/// Direction selects which of two very different validation paths runs, so an
/// unrecognised value must not fall through to either.
#[kani::proof]
fn unknown_direction_is_refused() {
    let mapped_len: u64 = kani::any();
    let direction: u32 = kani::any();
    kani::assume(
        direction != io_queue::DIRECTION_NONE
            && direction != io_queue::DIRECTION_DEVICE_READ
            && direction != io_queue::DIRECTION_DEVICE_WRITE,
    );
    let slice = io_queue::WireBufferSlice {
        buffer: kani::any(),
        lease: kani::any(),
        offset: kani::any(),
        length: kani::any(),
        direction,
        reserved: kani::any(),
    };
    assert!(!valid_buffer_slice(&slice, mapped_len));
}

/// Settling the same identity twice is refused, over symbolic inputs.
///
/// The model proves this of an abstract table. This proves it of the real
/// `Outstanding`, whose entry search is hand-written iteration -- the place an
/// off-by-one would let a second settle find a stale entry and release a lease
/// that was already released.
#[kani::proof]
#[kani::unwind(4)]
fn settle_is_single_assignment() {
    let mut table: Outstanding<2> = Outstanding::new(1);
    let request_id: u64 = kani::any();
    let lease: u64 = kani::any();
    kani::assume(request_id != 0);

    table
        .admit(request_id, lease, 0)
        .expect("an empty table admits a nonzero identity");

    let status: u32 = kani::any();
    kani::assume(terminal_state_for_status(status).is_some());

    let first = table.settle(request_id, status).expect("the entry is live");
    assert!(first.request_id == request_id);
    // The lease it retained comes back exactly once, so the caller knows what
    // to release.
    assert!(first.lease == lease);

    // Second attempt: the identity is gone, so there is nothing to release.
    assert!(table.settle(request_id, status) == Err(QueueError::Unknown));
}

/// A duplicate identity is refused rather than merged.
///
/// Two live entries sharing an identity could not both be settled, and the
/// second would silently inherit the first's lease.
#[kani::proof]
#[kani::unwind(4)]
fn admit_refuses_duplicate_identity() {
    let mut table: Outstanding<2> = Outstanding::new(1);
    let request_id: u64 = kani::any();
    kani::assume(request_id != 0);

    table
        .admit(request_id, kani::any(), 0)
        .expect("first admit");
    assert!(table.admit(request_id, kani::any(), 0) == Err(QueueError::Exhausted));
    // Exactly one entry is live, so exactly one lease is outstanding.
    assert!(table.len() == 1);
}

/// The table never exceeds the capacity the generation declared.
///
/// An unbounded outstanding table is an unbounded lease table.
#[kani::proof]
#[kani::unwind(4)]
fn admit_refuses_beyond_capacity() {
    let mut table: Outstanding<2> = Outstanding::new(1);
    let a: u64 = kani::any();
    let b: u64 = kani::any();
    let c: u64 = kani::any();
    kani::assume(a != 0 && b != 0 && c != 0);
    kani::assume(a != b && b != c && a != c);

    table.admit(a, kani::any(), 0).expect("first");
    table.admit(b, kani::any(), 0).expect("second");
    assert!(table.admit(c, kani::any(), 0) == Err(QueueError::Exhausted));
    assert!(table.len() == table.capacity());
}

/// An epoch never advances over a live request.
///
/// Advancing with entries still live would orphan their leases: nothing would
/// match their identities again, so nothing would ever release them. This is
/// the implementation-level counterpart of the model's
/// `NoLiveRequestAcrossEpoch`.
#[kani::proof]
#[kani::unwind(4)]
fn epoch_never_advances_over_a_live_request() {
    let epoch: u64 = kani::any();
    kani::assume(epoch != 0 && epoch < u64::MAX);
    let mut table: Outstanding<2> = Outstanding::new(epoch);

    let request_id: u64 = kani::any();
    kani::assume(request_id != 0);
    table.admit(request_id, kani::any(), 0).expect("admit");

    let next: u64 = kani::any();
    assert!(table.adopt_epoch(next) != Ok(()));
    // Refused, so the table still serves the epoch whose leases it holds.
    assert!(table.epoch() == epoch);

    // Draining is what unblocks it: every lease is handed back exactly once.
    let mut released = 0;
    let settled = table.settle_all(io_queue::STATUS_RESET, |_| released += 1);
    assert!(settled == 1 && released == 1);
    assert!(table.is_empty());

    // And an epoch must still strictly advance.
    kani::assume(next > epoch);
    assert!(table.adopt_epoch(next) == Ok(()));
    assert!(table.epoch() == next);
}

/// A stale epoch is never adopted, so identities never collide across epochs.
///
/// Identity is unique only within an epoch. Adopting a stale or equal epoch
/// would make a dead incarnation's completion match a live request.
#[kani::proof]
#[kani::unwind(4)]
fn epoch_adoption_is_strictly_monotonic() {
    let epoch: u64 = kani::any();
    kani::assume(epoch != 0);
    let mut table: Outstanding<2> = Outstanding::new(epoch);

    let next: u64 = kani::any();
    kani::assume(next == 0 || next <= epoch);
    assert!(table.adopt_epoch(next) == Err(QueueError::Malformed));
    assert!(table.epoch() == epoch);
}

/// Draining releases every held lease exactly once, or nothing at all.
///
/// `settle_all` is the callback that prevents each driver from walking the
/// table itself and releasing one lease twice or none. An undefined status must
/// release nothing rather than release into an unknown terminal state.
#[kani::proof]
#[kani::unwind(4)]
fn settle_all_releases_each_lease_once() {
    let mut table: Outstanding<2> = Outstanding::new(1);
    let a: u64 = kani::any();
    let b: u64 = kani::any();
    kani::assume(a != 0 && b != 0 && a != b);
    table.admit(a, kani::any(), 0).expect("first");
    table.admit(b, kani::any(), 0).expect("second");

    let status: u32 = kani::any();
    let mut released = 0;
    let settled = table.settle_all(status, |_| released += 1);

    if terminal_state_for_status(status).is_some() {
        assert!(settled == 2 && released == 2);
        assert!(table.is_empty());
    } else {
        // Refused wholesale: nothing released, nothing lost.
        assert!(settled == 0 && released == 0);
        assert!(table.len() == 2);
    }
}

/// A request cannot be started twice, so a device is programmed once.
#[kani::proof]
#[kani::unwind(4)]
fn start_is_refused_from_any_nonqueued_state() {
    let mut table: Outstanding<2> = Outstanding::new(1);
    let request_id: u64 = kani::any();
    kani::assume(request_id != 0);
    table.admit(request_id, kani::any(), 0).expect("admit");

    table.start(request_id).expect("queued requests start");
    assert!(table.start(request_id) == Err(QueueError::Malformed));

    // An identity that was never admitted is unknown rather than malformed:
    // the caller must be able to tell "not mine" from "wrong state".
    let other: u64 = kani::any();
    kani::assume(other != 0 && other != request_id);
    assert!(table.start(other) == Err(QueueError::Unknown));
}

/// A completion from another epoch never resolves a live request.
///
/// This is the late-completion rejection the substrate exists for, proved of
/// the real lookup rather than an abstraction of it.
#[kani::proof]
#[kani::unwind(4)]
fn find_never_matches_a_foreign_epoch() {
    let epoch: u64 = kani::any();
    kani::assume(epoch != 0);
    let mut table: Outstanding<2> = Outstanding::new(epoch);

    let request_id: u64 = kani::any();
    kani::assume(request_id != 0);
    table.admit(request_id, kani::any(), 0).expect("admit");

    let foreign: u64 = kani::any();
    kani::assume(foreign != epoch);
    assert!(table.find(request_id, foreign).is_none());

    // The right identity in the right epoch still resolves, so the rejection
    // above is not simply refusing everything.
    assert!(table.find(request_id, epoch).is_some());
}
