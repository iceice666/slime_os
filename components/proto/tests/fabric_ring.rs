//! The v2 shared ring's structural invariants (B46).
//!
//! A ring is memory a peer writes, so every field these tests exercise is
//! attacker-controlled in the sense that matters: a publisher with a stale
//! mapping or a wild index produces bytes, and the reader's only protection is
//! refusing to believe them. Each case here is a byte pattern a broken or
//! hostile writer can actually produce, not a hypothetical.

use slime_proto::fabric_ring::{
    self, BADGE_CREDIT_RETURNED, BADGE_PRODUCER_ENDED, BADGE_SAMPLE_READY, MAX_INLINE_BYTES,
    MAX_RING_SLOTS, MIN_RING_SLOTS, PRODUCER_ACTIVE, PRODUCER_DEAD, PRODUCER_FINISHED,
    RING_HEADER_LEN, RING_MAGIC, RING_SLOT_LEN, SLOT_CLAIMED, SLOT_EMPTY, SLOT_MAGIC, SLOT_READY,
    WireRingHeader, WireRingSlot,
};
use slime_proto::{ring_slot_index, valid_ring_badge, valid_ring_header, valid_ring_slot};

const TYPE: u64 = 0x5445_4C45_4D45_5452;
const SLOTS: usize = 8;

fn header() -> WireRingHeader {
    WireRingHeader {
        magic: RING_MAGIC,
        version: fabric_ring::FORMAT_VERSION,
        slot_count: SLOTS as u32,
        slot_len: RING_SLOT_LEN as u32,
        head: 4,
        tail: 1,
        type_identity: TYPE,
        producer_state: PRODUCER_ACTIVE,
        lost: 0,
        reserved: [0; 16],
    }
}

fn slot(sequence: u64) -> WireRingSlot {
    let mut payload = [0u8; 32];
    payload[..4].copy_from_slice(b"data");
    WireRingSlot {
        magic: SLOT_MAGIC,
        state: SLOT_READY,
        flags: 0,
        payload_len: 4,
        sequence,
        type_identity: TYPE,
        payload,
    }
}

#[test]
fn a_well_formed_header_and_slot_are_admitted() {
    assert!(valid_ring_header(&header(), TYPE, SLOTS));
    assert!(valid_ring_slot(&slot(7), TYPE, 7));
}

#[test]
fn both_records_round_trip_their_exact_length() {
    let encoded = header().encode();
    assert_eq!(encoded.len(), RING_HEADER_LEN);
    assert_eq!(WireRingHeader::decode(&encoded), Some(header()));

    let encoded = slot(3).encode();
    assert_eq!(encoded.len(), RING_SLOT_LEN);
    assert_eq!(WireRingSlot::decode(&encoded), Some(slot(3)));
}

#[test]
fn a_truncated_buffer_decodes_to_nothing() {
    // The ring is mapped memory; a reader that indexed past its own mapping
    // because the last slot was short would fault rather than refuse.
    let encoded = header().encode();
    assert_eq!(
        WireRingHeader::decode(&encoded[..RING_HEADER_LEN - 1]),
        None
    );
    let encoded = slot(1).encode();
    assert_eq!(WireRingSlot::decode(&encoded[..RING_SLOT_LEN - 1]), None);
}

#[test]
fn a_header_claiming_more_slots_than_provisioned_is_refused() {
    // The reader's bound comes from provisioning, not from the header. A
    // writer that inflates `slot_count` is asking the reader to walk off the
    // end of its own mapping.
    let mut inflated = header();
    inflated.slot_count = (SLOTS * 2) as u32;
    assert!(!valid_ring_header(&inflated, TYPE, SLOTS));
}

#[test]
fn a_slot_count_that_is_not_a_power_of_two_is_refused() {
    // `ring_slot_index` masks. A non-power-of-two count would make the mask
    // wrong and alias two sequences onto one slot.
    let mut odd = header();
    odd.slot_count = 6;
    assert!(!valid_ring_header(&odd, TYPE, 6));
}

#[test]
fn slot_counts_outside_the_declared_range_are_refused() {
    let mut small = header();
    small.slot_count = (MIN_RING_SLOTS - 1) as u32;
    assert!(!valid_ring_header(&small, TYPE, MIN_RING_SLOTS - 1));

    let mut large = header();
    large.slot_count = (MAX_RING_SLOTS * 2) as u32;
    assert!(!valid_ring_header(&large, TYPE, MAX_RING_SLOTS * 2));
}

#[test]
fn a_tail_past_the_head_is_refused() {
    // `head - tail` is the occupancy. Believing an inverted pair yields a huge
    // count on unsigned subtraction, and the reader tries to consume it.
    let mut inverted = header();
    inverted.tail = inverted.head + 1;
    assert!(!valid_ring_header(&inverted, TYPE, SLOTS));
}

#[test]
fn an_occupancy_larger_than_the_ring_is_refused() {
    // More outstanding samples than there are slots means at least one was
    // overwritten while still counted as present.
    let mut overfull = header();
    overfull.tail = 0;
    overfull.head = SLOTS as u64 + 1;
    assert!(!valid_ring_header(&overfull, TYPE, SLOTS));
}

#[test]
fn an_empty_and_a_full_ring_are_both_admitted() {
    // The boundaries are legal states, not errors: equal indices is empty and
    // a difference of exactly `slot_count` is full. A bound that refused
    // either would make the ring unusable at capacity.
    let mut empty = header();
    empty.tail = 9;
    empty.head = 9;
    assert!(valid_ring_header(&empty, TYPE, SLOTS));

    let mut full = header();
    full.tail = 2;
    full.head = 2 + SLOTS as u64;
    assert!(valid_ring_header(&full, TYPE, SLOTS));
}

#[test]
fn every_declared_producer_state_is_admitted_and_others_are_not() {
    for state in [PRODUCER_ACTIVE, PRODUCER_FINISHED, PRODUCER_DEAD] {
        let mut record = header();
        record.producer_state = state;
        assert!(
            valid_ring_header(&record, TYPE, SLOTS),
            "state {state} is declared by the contract"
        );
    }
    let mut unknown = header();
    unknown.producer_state = 9;
    assert!(!valid_ring_header(&unknown, TYPE, SLOTS));
}

#[test]
fn a_header_for_another_route_type_is_refused() {
    // The type identity binds the ring to the route's declared interface, so a
    // publisher cannot feed another type's bytes to a subscriber expecting
    // this one.
    assert!(!valid_ring_header(&header(), TYPE ^ 1, SLOTS));
    let mut untyped = header();
    untyped.type_identity = 0;
    assert!(!valid_ring_header(&untyped, 0, SLOTS));
}

#[test]
fn a_claimed_slot_is_never_readable() {
    // This is what makes a torn write unobservable rather than unlikely: a
    // publisher preempted between claiming a slot and finishing its copy
    // leaves exactly this state, and the reader must not advance past it.
    let mut mid_write = slot(5);
    mid_write.state = SLOT_CLAIMED;
    assert!(!valid_ring_slot(&mid_write, TYPE, 5));

    let mut empty = slot(5);
    empty.state = SLOT_EMPTY;
    assert!(!valid_ring_slot(&empty, TYPE, 5));
}

#[test]
fn a_slot_from_a_previous_wrap_is_refused() {
    // The lagging-subscriber case. The slot is structurally perfect and
    // carries a real sample -- from `SLOTS` samples ago. Accepting it means
    // delivering stale data as new; refusing it is what lets the reader
    // notice it fell behind and count the loss.
    let stale = slot(3);
    assert!(!valid_ring_slot(&stale, TYPE, 3 + SLOTS as u64));
}

#[test]
fn sequence_zero_is_never_a_sample() {
    // Publishers number from one, so zero is an unwritten slot whose magic
    // happens to be set -- reusable memory, not a sample.
    assert!(!valid_ring_slot(&slot(0), TYPE, 0));
}

#[test]
fn a_slot_payload_longer_than_the_inline_bound_is_refused() {
    let mut oversized = slot(2);
    oversized.payload_len = MAX_INLINE_BYTES as u32 + 1;
    assert!(!valid_ring_slot(&oversized, TYPE, 2));

    let mut empty = slot(2);
    empty.payload_len = 0;
    assert!(!valid_ring_slot(&empty, TYPE, 2));
}

#[test]
fn padding_past_the_declared_length_must_be_zero() {
    // Two byte-distinct slots that decode to the same payload would both be
    // admissible, so a KEEP_LAST comparison on stored bytes could not treat
    // them as one sample.
    let mut dirty = slot(6);
    dirty.payload[MAX_INLINE_BYTES - 1] = 0xff;
    assert!(!valid_ring_slot(&dirty, TYPE, 6));
}

#[test]
fn an_unknown_slot_flag_is_refused() {
    let mut future = slot(4);
    future.flags = 0b1000;
    assert!(!valid_ring_slot(&future, TYPE, 4));
}

#[test]
fn the_last_flag_is_carried_on_a_normal_sample() {
    // `FLAG_LAST` rides the final sample rather than arriving separately, so a
    // subscriber sees the end in the same read that delivers the data.
    let mut final_sample = slot(9);
    final_sample.flags = fabric_ring::FLAG_LAST;
    assert!(valid_ring_slot(&final_sample, TYPE, 9));
}

#[test]
fn a_sequence_maps_to_its_slot_by_masking() {
    assert_eq!(ring_slot_index(0, SLOTS), 0);
    assert_eq!(ring_slot_index(7, SLOTS), 7);
    // The wrap: sequence 8 reuses slot 0, which is why a reader must check the
    // sequence rather than trusting the index it computed.
    assert_eq!(ring_slot_index(8, SLOTS), 0);
    assert_eq!(ring_slot_index(u64::MAX, SLOTS), SLOTS - 1);
}

#[test]
fn a_badge_carries_only_bits_this_version_defines() {
    for bit in [
        BADGE_SAMPLE_READY,
        BADGE_CREDIT_RETURNED,
        BADGE_PRODUCER_ENDED,
    ] {
        assert!(valid_ring_badge(bit));
    }
    // Coalescing is the normal case: a notification word is OR-ed, so several
    // signals before one wait arrive together and must stay legible.
    assert!(valid_ring_badge(BADGE_SAMPLE_READY | BADGE_PRODUCER_ENDED));
    // An empty wake says nothing, and an unknown bit is a peer signalling
    // something this reader cannot interpret.
    assert!(!valid_ring_badge(0));
    assert!(!valid_ring_badge(BADGE_SAMPLE_READY | 0b1_0000));
}
