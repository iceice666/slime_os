//! The IO0 substrate's structural refusals, at the wire level.
//!
//! These test the contract's validators rather than the cursor discipline: what
//! a party must refuse when it reads bytes a peer wrote. The cursor and lease
//! discipline is exercised in `io_queue_ring.rs`.

use slime_proto::io_queue::{
    self, COMPLETION_PAYLOAD_BYTES, COMPLETION_SLOT_LEN, QUEUE_HEADER_LEN, REQUEST_PAYLOAD_BYTES,
    REQUEST_SLOT_LEN, WireBufferSlice, WireCompletionSlot, WireQueueHeader, WireRequestSlot,
};
use slime_proto::{
    queue_slot_index, terminal_request_state, terminal_state_for_status, valid_buffer_slice,
    valid_completion_slot, valid_completion_status, valid_queue_badge, valid_queue_header,
    valid_request_slot,
};

const SLOTS: usize = 8;
const EPOCH: u64 = 7;
const MAPPED: u64 = 4096;

fn header() -> WireQueueHeader {
    WireQueueHeader {
        magic: io_queue::QUEUE_MAGIC,
        version: io_queue::FORMAT_VERSION,
        slot_count: SLOTS as u32,
        request_slot_len: REQUEST_SLOT_LEN as u32,
        completion_slot_len: COMPLETION_SLOT_LEN as u32,
        client_reserved: [0; 4],
        submit_head: 5,
        complete_tail: 3,
        client_padding: [0; 24],
        driver_state: io_queue::DRIVER_ACTIVE,
        driver_reserved: [0; 4],
        epoch: EPOCH,
        complete_head: 4,
        submit_tail: 4,
        driver_padding: [0; 32],
    }
}

fn slice() -> WireBufferSlice {
    WireBufferSlice {
        buffer: 11,
        lease: 22,
        offset: 512,
        length: 1024,
        direction: io_queue::DIRECTION_DEVICE_WRITE,
        reserved: [0; 4],
    }
}

fn request() -> WireRequestSlot {
    let slice = slice();
    WireRequestSlot {
        magic: io_queue::REQUEST_MAGIC,
        state: io_queue::SLOT_READY,
        flags: 0,
        payload_len: 4,
        request_id: 91,
        epoch: EPOCH,
        slice_buffer: slice.buffer,
        slice_lease: slice.lease,
        slice_offset: slice.offset,
        slice_length: slice.length,
        slice_direction: slice.direction,
        slice_reserved: slice.reserved,
        payload: {
            let mut payload = [0u8; REQUEST_PAYLOAD_BYTES];
            payload[..4].copy_from_slice(&[1, 2, 3, 4]);
            payload
        },
    }
}

fn completion() -> WireCompletionSlot {
    WireCompletionSlot {
        magic: io_queue::COMPLETION_MAGIC,
        status: io_queue::STATUS_OK,
        flags: 0,
        payload_len: 2,
        request_id: 91,
        epoch: EPOCH,
        transferred: 1024,
        payload: {
            let mut payload = [0u8; COMPLETION_PAYLOAD_BYTES];
            payload[..2].copy_from_slice(&[9, 9]);
            payload
        },
    }
}

#[test]
fn every_record_round_trips_at_its_declared_length() {
    let encoded = header().encode();
    assert_eq!(encoded.len(), QUEUE_HEADER_LEN);
    assert_eq!(WireQueueHeader::decode(&encoded), Some(header()));

    let encoded = request().encode();
    assert_eq!(encoded.len(), REQUEST_SLOT_LEN);
    assert_eq!(WireRequestSlot::decode(&encoded), Some(request()));

    let encoded = completion().encode();
    assert_eq!(encoded.len(), COMPLETION_SLOT_LEN);
    assert_eq!(WireCompletionSlot::decode(&encoded), Some(completion()));

    let encoded = slice().encode();
    assert_eq!(encoded.len(), io_queue::SLICE_LEN);
    assert_eq!(WireBufferSlice::decode(&encoded), Some(slice()));
}

#[test]
fn a_short_buffer_decodes_to_nothing_rather_than_a_partial_record() {
    let encoded = header().encode();
    assert_eq!(
        WireQueueHeader::decode(&encoded[..QUEUE_HEADER_LEN - 1]),
        None
    );
    let encoded = request().encode();
    assert_eq!(
        WireRequestSlot::decode(&encoded[..REQUEST_SLOT_LEN - 1]),
        None
    );
    let encoded = completion().encode();
    assert_eq!(
        WireCompletionSlot::decode(&encoded[..COMPLETION_SLOT_LEN - 1]),
        None
    );
}

#[test]
fn the_header_places_its_two_writers_on_separate_cache_lines() {
    // The reason this is a test rather than a comment: false sharing is a
    // performance fault, so nothing else in the suite would ever notice if a
    // schema edit moved `submit_head` and `complete_head` onto one line.
    assert_eq!(QUEUE_HEADER_LEN, 128);
    for offset in [
        io_queue::OFF_HEADER_MAGIC,
        io_queue::OFF_HEADER_SUBMIT_HEAD,
        io_queue::OFF_HEADER_COMPLETE_TAIL,
    ] {
        assert!(
            offset < 64,
            "client-written field at {offset} left its line"
        );
    }
    for offset in [
        io_queue::OFF_HEADER_DRIVER_STATE,
        io_queue::OFF_HEADER_EPOCH,
        io_queue::OFF_HEADER_COMPLETE_HEAD,
        io_queue::OFF_HEADER_SUBMIT_TAIL,
    ] {
        assert!(
            offset >= 64,
            "driver-written field at {offset} left its line"
        );
    }
}

#[test]
fn the_request_slot_inlines_the_slice_at_matching_offsets() {
    // A standalone slice validator is only meaningful if the inlined copy has
    // the same shape; the generator checks this, and so does this test, because
    // a generator change could silently relax it.
    assert_eq!(
        io_queue::OFF_REQUEST_SLICE_LEASE - io_queue::OFF_REQUEST_SLICE_BUFFER,
        io_queue::OFF_SLICE_LEASE - io_queue::OFF_SLICE_BUFFER
    );
    assert_eq!(
        io_queue::OFF_REQUEST_SLICE_DIRECTION - io_queue::OFF_REQUEST_SLICE_BUFFER,
        io_queue::OFF_SLICE_DIRECTION - io_queue::OFF_SLICE_BUFFER
    );
    assert_eq!(
        io_queue::OFF_REQUEST_PAYLOAD - io_queue::OFF_REQUEST_SLICE_BUFFER,
        io_queue::SLICE_LEN
    );
}

#[test]
fn a_well_formed_header_is_accepted() {
    assert!(valid_queue_header(&header(), SLOTS));
}

#[test]
fn a_header_claiming_a_shape_the_reader_did_not_provision_is_refused() {
    let mut bad = header();
    bad.slot_count = (SLOTS * 2) as u32;
    assert!(!valid_queue_header(&bad, SLOTS));

    // Non-power-of-two, so masking would not be the modulus.
    let mut bad = header();
    bad.slot_count = 6;
    bad.submit_head = 0;
    bad.submit_tail = 0;
    bad.complete_head = 0;
    bad.complete_tail = 0;
    assert!(!valid_queue_header(&bad, 6));

    let mut bad = header();
    bad.magic ^= 1;
    assert!(!valid_queue_header(&bad, SLOTS));

    let mut bad = header();
    bad.version += 1;
    assert!(!valid_queue_header(&bad, SLOTS));

    let mut bad = header();
    bad.request_slot_len += 1;
    assert!(!valid_queue_header(&bad, SLOTS));
}

#[test]
fn impossible_positions_are_refused_rather_than_subtracted() {
    // A tail past its head yields a huge occupancy under wrapping subtraction,
    // and a reader that believed it would try to consume that many entries.
    let mut bad = header();
    bad.submit_tail = bad.submit_head + 1;
    assert!(!valid_queue_header(&bad, SLOTS));

    let mut bad = header();
    bad.complete_tail = bad.complete_head + 1;
    assert!(!valid_queue_header(&bad, SLOTS));

    let mut bad = header();
    bad.submit_head = bad.submit_tail + SLOTS as u64 + 1;
    assert!(!valid_queue_header(&bad, SLOTS));

    let mut bad = header();
    bad.complete_head = bad.complete_tail + SLOTS as u64 + 1;
    assert!(!valid_queue_header(&bad, SLOTS));
}

#[test]
fn more_completions_than_submissions_is_refused() {
    // A completion answers a request. More answers than questions means the
    // driver invented one, and a client consuming it would settle a request it
    // never made.
    let mut bad = header();
    bad.complete_head = bad.submit_head + 1;
    assert!(!valid_queue_header(&bad, SLOTS));
}

#[test]
fn an_active_driver_at_epoch_zero_is_refused() {
    // Epoch zero means no incarnation has claimed the queue, so an active
    // driver there would admit work nobody owns.
    let mut bad = header();
    bad.epoch = 0;
    assert!(!valid_queue_header(&bad, SLOTS));

    // A never-claimed queue may legitimately read zero while dead.
    let mut unclaimed = header();
    unclaimed.epoch = 0;
    unclaimed.driver_state = io_queue::DRIVER_DEAD;
    unclaimed.submit_head = 0;
    unclaimed.submit_tail = 0;
    unclaimed.complete_head = 0;
    unclaimed.complete_tail = 0;
    assert!(valid_queue_header(&unclaimed, SLOTS));
}

#[test]
fn an_unknown_driver_state_is_refused() {
    let mut bad = header();
    bad.driver_state = 9;
    assert!(!valid_queue_header(&bad, SLOTS));
}

#[test]
fn dirty_reserved_or_padding_bytes_are_refused() {
    // Two byte-distinct headers must not decode to the same meaning, or a
    // reader's view and a writer's intent can diverge invisibly.
    for mutate in [
        (|h: &mut WireQueueHeader| h.client_reserved[0] = 1) as fn(&mut WireQueueHeader),
        |h: &mut WireQueueHeader| h.client_padding[7] = 1,
        |h: &mut WireQueueHeader| h.driver_reserved[3] = 1,
        |h: &mut WireQueueHeader| h.driver_padding[31] = 1,
    ] {
        let mut bad = header();
        mutate(&mut bad);
        assert!(!valid_queue_header(&bad, SLOTS));
    }
}

#[test]
fn a_slice_inside_its_lease_mapping_is_accepted() {
    assert!(valid_buffer_slice(&slice(), MAPPED));
    // Exactly to the end is inside.
    let mut exact = slice();
    exact.offset = MAPPED - exact.length;
    assert!(valid_buffer_slice(&exact, MAPPED));
}

#[test]
fn a_slice_past_its_lease_mapping_is_refused() {
    let mut bad = slice();
    bad.offset = MAPPED - bad.length + 1;
    assert!(!valid_buffer_slice(&bad, MAPPED));

    let mut bad = slice();
    bad.length = MAPPED + 1;
    bad.offset = 0;
    assert!(!valid_buffer_slice(&bad, MAPPED));
}

#[test]
fn a_slice_whose_extent_overflows_is_refused_rather_than_wrapped() {
    // This is where a hostile descriptor aims: `offset + length` wrapping to a
    // small number would pass a naive bound check.
    let mut bad = slice();
    bad.offset = u64::MAX - 4;
    bad.length = 64;
    assert!(!valid_buffer_slice(&bad, MAPPED));
}

#[test]
fn a_zero_length_or_unattributable_slice_is_refused() {
    let mut bad = slice();
    bad.length = 0;
    assert!(!valid_buffer_slice(&bad, MAPPED));

    // Bytes a device may touch must be attributable to a lease that can be
    // settled exactly once, and to a buffer identity.
    let mut bad = slice();
    bad.lease = 0;
    assert!(!valid_buffer_slice(&bad, MAPPED));

    let mut bad = slice();
    bad.buffer = 0;
    assert!(!valid_buffer_slice(&bad, MAPPED));
}

#[test]
fn an_unknown_direction_is_refused() {
    let mut bad = slice();
    bad.direction = 3;
    assert!(!valid_buffer_slice(&bad, MAPPED));
}

#[test]
fn a_control_slice_must_be_entirely_empty() {
    let empty = WireBufferSlice {
        buffer: 0,
        lease: 0,
        offset: 0,
        length: 0,
        direction: io_queue::DIRECTION_NONE,
        reserved: [0; 4],
    };
    assert!(valid_buffer_slice(&empty, MAPPED));

    // A half-filled no-direction slice would carry a lease identity the
    // substrate would then have to decide whether to settle.
    for mutate in [
        (|s: &mut WireBufferSlice| s.lease = 1) as fn(&mut WireBufferSlice),
        |s: &mut WireBufferSlice| s.buffer = 1,
        |s: &mut WireBufferSlice| s.offset = 1,
        |s: &mut WireBufferSlice| s.length = 1,
        |s: &mut WireBufferSlice| s.reserved[0] = 1,
    ] {
        let mut bad = empty;
        mutate(&mut bad);
        assert!(!valid_buffer_slice(&bad, MAPPED));
    }
}

#[test]
fn a_well_formed_request_is_accepted() {
    assert!(valid_request_slot(&request(), EPOCH, MAPPED));
}

#[test]
fn a_request_from_another_epoch_is_refused() {
    // The stale-submission rejection: work from a driver incarnation that no
    // longer exists must not reach a device.
    let mut stale = request();
    stale.epoch = EPOCH - 1;
    assert!(!valid_request_slot(&stale, EPOCH, MAPPED));

    let mut future = request();
    future.epoch = EPOCH + 1;
    assert!(!valid_request_slot(&future, EPOCH, MAPPED));

    let mut zero = request();
    zero.epoch = 0;
    assert!(!valid_request_slot(&zero, 0, MAPPED));
}

#[test]
fn a_request_identity_of_zero_is_refused() {
    // Reserved so a zeroed slot cannot be mistaken for request zero.
    let mut bad = request();
    bad.request_id = 0;
    assert!(!valid_request_slot(&bad, EPOCH, MAPPED));
}

#[test]
fn a_claimed_or_empty_request_slot_is_refused() {
    // `SLOT_CLAIMED` means the writer is mid-copy; refusing it is what makes a
    // torn write unobservable rather than merely unlikely.
    for state in [io_queue::SLOT_EMPTY, io_queue::SLOT_CLAIMED, 9] {
        let mut bad = request();
        bad.state = state;
        assert!(!valid_request_slot(&bad, EPOCH, MAPPED));
    }
}

#[test]
fn an_unknown_request_flag_is_refused() {
    let mut bad = request();
    bad.flags = !io_queue::KNOWN_REQUEST_FLAGS;
    assert!(!valid_request_slot(&bad, EPOCH, MAPPED));

    let mut fenced = request();
    fenced.flags = io_queue::FLAG_FENCED;
    assert!(valid_request_slot(&fenced, EPOCH, MAPPED));
}

#[test]
fn an_oversized_or_dirtily_padded_request_payload_is_refused() {
    let mut bad = request();
    bad.payload_len = (REQUEST_PAYLOAD_BYTES + 1) as u32;
    assert!(!valid_request_slot(&bad, EPOCH, MAPPED));

    let mut bad = request();
    bad.payload[REQUEST_PAYLOAD_BYTES - 1] = 1;
    assert!(!valid_request_slot(&bad, EPOCH, MAPPED));

    // A full-length payload has no padding to be dirty.
    let mut full = request();
    full.payload_len = REQUEST_PAYLOAD_BYTES as u32;
    full.payload = [7; REQUEST_PAYLOAD_BYTES];
    assert!(valid_request_slot(&full, EPOCH, MAPPED));
}

#[test]
fn a_request_carrying_a_bad_slice_is_refused_through_the_same_check() {
    let mut bad = request();
    bad.slice_offset = MAPPED;
    assert!(!valid_request_slot(&bad, EPOCH, MAPPED));

    let mut bad = request();
    bad.slice_direction = 7;
    assert!(!valid_request_slot(&bad, EPOCH, MAPPED));

    let mut bad = request();
    bad.slice_reserved[0] = 1;
    assert!(!valid_request_slot(&bad, EPOCH, MAPPED));
}

#[test]
fn a_well_formed_completion_is_accepted() {
    assert!(valid_completion_slot(&completion(), 91, EPOCH, 1024));
}

#[test]
fn a_completion_for_another_request_or_epoch_is_refused() {
    // The late-completion rejection. A request already settled by
    // cancellation, reset, or peer death is no longer held by the caller, so
    // no `expected_request` matches and it cannot be resurrected.
    assert!(!valid_completion_slot(&completion(), 92, EPOCH, 1024));
    assert!(!valid_completion_slot(&completion(), 91, EPOCH + 1, 1024));

    let mut zero = completion();
    zero.request_id = 0;
    assert!(!valid_completion_slot(&zero, 0, EPOCH, 1024));

    let mut zero = completion();
    zero.epoch = 0;
    assert!(!valid_completion_slot(&zero, 91, 0, 1024));
}

#[test]
fn a_completion_claiming_more_bytes_than_the_slice_covered_is_refused() {
    // A driver reporting more bytes than the slice authorized is claiming to
    // have touched memory the lease did not cover.
    assert!(!valid_completion_slot(&completion(), 91, EPOCH, 1023));

    let mut exact = completion();
    exact.transferred = 1024;
    assert!(valid_completion_slot(&exact, 91, EPOCH, 1024));

    let mut short = completion();
    short.transferred = 0;
    assert!(valid_completion_slot(&short, 91, EPOCH, 1024));
}

#[test]
fn a_refusal_that_also_claims_a_transfer_is_refused() {
    // Two different outcomes at once: a caller reading `transferred` without
    // branching on status first would trust it.
    for status in [
        io_queue::STATUS_CANCELLED,
        io_queue::STATUS_RESET,
        io_queue::STATUS_PEER_DEAD,
        io_queue::STATUS_MALFORMED,
        io_queue::STATUS_BAD_SLICE,
        io_queue::STATUS_DEVICE_ERROR,
    ] {
        let mut refused = completion();
        refused.status = status;
        refused.transferred = 1;
        assert!(!valid_completion_slot(&refused, 91, EPOCH, 1024));

        refused.transferred = 0;
        assert!(valid_completion_slot(&refused, 91, EPOCH, 1024));
    }
}

#[test]
fn an_unknown_completion_status_is_refused() {
    let mut bad = completion();
    bad.status = 11;
    bad.transferred = 0;
    assert!(!valid_completion_slot(&bad, 91, EPOCH, 1024));
    assert!(!valid_completion_status(11));
    assert!(!valid_completion_status(u32::MAX));
}

#[test]
fn every_defined_status_is_recognised() {
    for status in [
        io_queue::STATUS_OK,
        io_queue::STATUS_CANCELLED,
        io_queue::STATUS_RESET,
        io_queue::STATUS_PEER_DEAD,
        io_queue::STATUS_MALFORMED,
        io_queue::STATUS_BAD_SLICE,
        io_queue::STATUS_BAD_EPOCH,
        io_queue::STATUS_BAD_RIGHTS,
        io_queue::STATUS_EXHAUSTED,
        io_queue::STATUS_DEVICE_ERROR,
        io_queue::STATUS_UNSUPPORTED,
    ] {
        assert!(valid_completion_status(status));
    }
}

#[test]
fn an_unknown_completion_flag_is_refused() {
    let mut bad = completion();
    bad.flags = !io_queue::KNOWN_COMPLETION_FLAGS;
    assert!(!valid_completion_slot(&bad, 91, EPOCH, 1024));

    let mut ended = completion();
    ended.flags = io_queue::FLAG_EPOCH_ENDED;
    assert!(valid_completion_slot(&ended, 91, EPOCH, 1024));
}

#[test]
fn an_oversized_or_dirtily_padded_completion_payload_is_refused() {
    let mut bad = completion();
    bad.payload_len = (COMPLETION_PAYLOAD_BYTES + 1) as u32;
    assert!(!valid_completion_slot(&bad, 91, EPOCH, 1024));

    let mut bad = completion();
    bad.payload[COMPLETION_PAYLOAD_BYTES - 1] = 1;
    assert!(!valid_completion_slot(&bad, 91, EPOCH, 1024));
}

#[test]
fn every_defined_status_maps_to_exactly_one_terminal_state() {
    // Totality is the point: "every submitted request reaches one terminal
    // state" is only checkable if no defined status is unclassified.
    assert_eq!(
        terminal_state_for_status(io_queue::STATUS_CANCELLED),
        Some(io_queue::STATE_CANCELLED)
    );
    assert_eq!(
        terminal_state_for_status(io_queue::STATUS_RESET),
        Some(io_queue::STATE_RESET)
    );
    assert_eq!(
        terminal_state_for_status(io_queue::STATUS_PEER_DEAD),
        Some(io_queue::STATE_PEER_DEAD)
    );
    // A device error is still an answer the driver produced for this request,
    // so the request completed; the failure is the status, not the state.
    for status in [
        io_queue::STATUS_OK,
        io_queue::STATUS_MALFORMED,
        io_queue::STATUS_BAD_SLICE,
        io_queue::STATUS_BAD_EPOCH,
        io_queue::STATUS_BAD_RIGHTS,
        io_queue::STATUS_EXHAUSTED,
        io_queue::STATUS_DEVICE_ERROR,
        io_queue::STATUS_UNSUPPORTED,
    ] {
        assert_eq!(
            terminal_state_for_status(status),
            Some(io_queue::STATE_COMPLETE),
            "status {status} is unclassified"
        );
    }
    assert_eq!(terminal_state_for_status(11), None);

    for state in [
        io_queue::STATE_COMPLETE,
        io_queue::STATE_CANCELLED,
        io_queue::STATE_RESET,
        io_queue::STATE_PEER_DEAD,
    ] {
        assert!(terminal_request_state(state));
    }
    for state in [io_queue::STATE_QUEUED, io_queue::STATE_IN_FLIGHT, 0, 9] {
        assert!(!terminal_request_state(state));
    }
}

#[test]
fn a_badge_carries_only_bits_this_version_defines() {
    assert!(valid_queue_badge(io_queue::BADGE_REQUEST_READY));
    assert!(valid_queue_badge(io_queue::BADGE_COMPLETION_READY));
    assert!(valid_queue_badge(io_queue::BADGE_DRIVER_STATE_CHANGED));
    // Coalescing ORs bits together, so a combination is normal.
    assert!(valid_queue_badge(io_queue::KNOWN_BADGE_BITS));
    // An unknown bit means a peer signalling something this reader cannot
    // interpret; treating it as one of the known bits would be worse.
    assert!(!valid_queue_badge(0));
    assert!(!valid_queue_badge(io_queue::KNOWN_BADGE_BITS | 8));
}

#[test]
fn the_slot_index_masks_within_the_ring() {
    for sequence in 0..(SLOTS as u64 * 3) {
        let index = queue_slot_index(sequence, SLOTS);
        assert!(index < SLOTS);
        assert_eq!(index, (sequence % SLOTS as u64) as usize);
    }
}
