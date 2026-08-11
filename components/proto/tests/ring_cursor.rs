//! Ring cursor discipline: capacity, wrap, loss, and lifecycle (B46).
//!
//! These are the properties the seven B46 gates check behaviourally today over
//! logical channels — backpressure, bounded queues, peer death, and buffered
//! stream recovery. Proving them here first means the cutover has something to
//! be checked *against*, rather than the gates being the only thing that would
//! notice a regression.

use slime_proto::fabric_ring::{
    FLAG_LAST, MAX_INLINE_BYTES, PRODUCER_ACTIVE, PRODUCER_DEAD, PRODUCER_FINISHED,
    RING_HEADER_LEN, RING_SLOT_LEN, SLOT_CLAIMED, SLOT_MAGIC, SLOT_READY,
};
use slime_proto::ring::{Ring, RingError};

const TYPE: u64 = 0x5445_4C45_4D45_5452;
const SLOTS: usize = 4;

fn buffer() -> [u8; RING_HEADER_LEN + SLOTS * RING_SLOT_LEN] {
    let mut bytes = [0u8; RING_HEADER_LEN + SLOTS * RING_SLOT_LEN];
    Ring::format(&mut bytes, TYPE, SLOTS).expect("format");
    bytes
}

#[test]
fn a_formatted_ring_attaches_and_starts_empty() {
    let mut bytes = buffer();
    let ring = Ring::attach(&mut bytes, TYPE, SLOTS).expect("attach");
    assert_eq!(ring.occupancy(), 0);
    assert_eq!(ring.producer_state(), PRODUCER_ACTIVE);
    assert_eq!(ring.lost(), 0);
}

#[test]
fn a_published_sample_comes_back_byte_for_byte() {
    let mut bytes = buffer();
    let mut ring = Ring::attach(&mut bytes, TYPE, SLOTS).expect("attach");
    assert_eq!(ring.publish(b"hello", false).expect("publish"), 1);
    assert_eq!(ring.occupancy(), 1);

    let mut out = [0u8; MAX_INLINE_BYTES];
    let (length, last) = ring.consume(&mut out).expect("consume");
    assert_eq!(&out[..length], b"hello");
    assert!(!last);
    assert_eq!(ring.occupancy(), 0);
}

#[test]
fn samples_are_delivered_in_order() {
    let mut bytes = buffer();
    let mut ring = Ring::attach(&mut bytes, TYPE, SLOTS).expect("attach");
    for payload in [b"a1", b"b2", b"c3"] {
        ring.publish(payload, false).expect("publish");
    }
    let mut out = [0u8; MAX_INLINE_BYTES];
    for expected in [b"a1", b"b2", b"c3"] {
        let (length, _) = ring.consume(&mut out).expect("consume");
        assert_eq!(&out[..length], expected.as_slice());
    }
    assert_eq!(ring.consume(&mut out), Err(RingError::Empty));
}

#[test]
fn a_full_ring_refuses_rather_than_overwriting() {
    // Backpressure. The publisher is told to wait; the unread samples stay
    // readable. A ring that overwrote here would silently drop the oldest
    // sample while the subscriber still believed it was coming.
    let mut bytes = buffer();
    let mut ring = Ring::attach(&mut bytes, TYPE, SLOTS).expect("attach");
    for index in 0..SLOTS {
        ring.publish(&[index as u8], false).expect("publish");
    }
    assert_eq!(ring.occupancy(), SLOTS as u64);
    assert_eq!(ring.publish(b"overflow", false), Err(RingError::Full));

    // And the samples already there are intact.
    let mut out = [0u8; MAX_INLINE_BYTES];
    let (length, _) = ring.consume(&mut out).expect("consume");
    assert_eq!(&out[..length], &[0u8]);
}

#[test]
fn consuming_one_sample_returns_exactly_one_slot_of_credit() {
    let mut bytes = buffer();
    let mut ring = Ring::attach(&mut bytes, TYPE, SLOTS).expect("attach");
    for index in 0..SLOTS {
        ring.publish(&[index as u8], false).expect("publish");
    }
    let mut out = [0u8; MAX_INLINE_BYTES];
    ring.consume(&mut out).expect("consume");

    ring.publish(b"next", false).expect("one slot freed");
    assert_eq!(ring.publish(b"another", false), Err(RingError::Full));
}

#[test]
fn the_ring_wraps_without_losing_anything_when_kept_drained() {
    // Several times around, which is the case where an off-by-one in the mask
    // or the sequence would surface as a wrong payload rather than an error.
    let mut bytes = buffer();
    let mut ring = Ring::attach(&mut bytes, TYPE, SLOTS).expect("attach");
    let mut out = [0u8; MAX_INLINE_BYTES];
    for round in 0..(SLOTS as u8 * 5) {
        ring.publish(&[round, round ^ 0xff], false)
            .expect("publish");
        let (length, _) = ring.consume(&mut out).expect("consume");
        assert_eq!(&out[..length], &[round, round ^ 0xff], "round {round}");
    }
    assert_eq!(ring.lost(), 0);
}

#[test]
fn an_empty_ring_reports_empty_rather_than_stale_bytes() {
    // The slot still holds the previous sample's bytes; only `head == tail`
    // says it must not be read again.
    let mut bytes = buffer();
    let mut ring = Ring::attach(&mut bytes, TYPE, SLOTS).expect("attach");
    ring.publish(b"once", false).expect("publish");
    let mut out = [0u8; MAX_INLINE_BYTES];
    ring.consume(&mut out).expect("consume");
    assert_eq!(ring.consume(&mut out), Err(RingError::Empty));
}

#[test]
fn this_ring_cannot_drop_a_sample() {
    // The property that makes `consume`'s sequence check a corruption test
    // rather than a drop counter: the publisher refuses at capacity, so the
    // slot the reader is owed is either the awaited sample or unwritten. No
    // sequence of public calls reaches "overwritten while we were away".
    let mut bytes = buffer();
    let mut ring = Ring::attach(&mut bytes, TYPE, SLOTS).expect("attach");
    let mut out = [0u8; MAX_INLINE_BYTES];

    // Publish until refused, never reading. Nothing is lost -- it is refused.
    let mut published = 0u64;
    while ring.publish(&[published as u8], false).is_ok() {
        published += 1;
        assert!(published <= SLOTS as u64, "capacity must bound the writer");
    }
    assert_eq!(published, SLOTS as u64);
    assert_eq!(ring.lost(), 0);

    // Everything published is still readable, in order.
    for index in 0..SLOTS as u8 {
        let (length, _) = ring.consume(&mut out).expect("nothing was overwritten");
        assert_eq!(&out[..length], &[index]);
    }
    assert_eq!(ring.lost(), 0);
}

#[test]
fn a_last_sample_finishes_the_producer() {
    // `FLAG_LAST` rides the final sample, so the subscriber sees the end in
    // the same read that delivers the data -- not as a separate message that
    // could arrive out of order or not at all.
    let mut bytes = buffer();
    let mut ring = Ring::attach(&mut bytes, TYPE, SLOTS).expect("attach");
    ring.publish(b"final", true).expect("publish");
    assert_eq!(ring.producer_state(), PRODUCER_FINISHED);

    let mut out = [0u8; MAX_INLINE_BYTES];
    let (length, last) = ring.consume(&mut out).expect("consume");
    assert_eq!(&out[..length], b"final");
    assert!(last, "the flag reaches the reader with the sample");
}

#[test]
fn peer_death_is_visible_in_the_ring_and_does_not_overwrite_a_clean_end() {
    // Peer death without a root round trip, which is what v1 needed a logical
    // channel for. A producer that already finished stays finished: "ended
    // cleanly" and "died mid-stream" call for different handling, and
    // reclamation must not relabel the first as the second.
    let mut bytes = buffer();
    let mut ring = Ring::attach(&mut bytes, TYPE, SLOTS).expect("attach");
    ring.mark_producer_dead();
    assert_eq!(ring.producer_state(), PRODUCER_DEAD);

    let mut clean = buffer();
    let mut ring = Ring::attach(&mut clean, TYPE, SLOTS).expect("attach");
    ring.publish(b"done", true).expect("publish");
    ring.mark_producer_dead();
    assert_eq!(
        ring.producer_state(),
        PRODUCER_FINISHED,
        "a clean end is not relabelled as a death"
    );
}

#[test]
fn samples_left_unread_survive_the_producer_dying() {
    // The reader must still drain what was published before the death.
    let mut bytes = buffer();
    let mut ring = Ring::attach(&mut bytes, TYPE, SLOTS).expect("attach");
    ring.publish(b"before", false).expect("publish");
    ring.mark_producer_dead();

    let mut out = [0u8; MAX_INLINE_BYTES];
    let (length, _) = ring.consume(&mut out).expect("consume");
    assert_eq!(&out[..length], b"before");
    assert_eq!(ring.consume(&mut out), Err(RingError::Empty));
}

#[test]
fn an_oversized_or_empty_payload_is_refused() {
    let mut bytes = buffer();
    let mut ring = Ring::attach(&mut bytes, TYPE, SLOTS).expect("attach");
    assert_eq!(ring.publish(&[], false), Err(RingError::TooLarge));
    assert_eq!(
        ring.publish(&[0u8; MAX_INLINE_BYTES + 1], false),
        Err(RingError::TooLarge)
    );
    // The bound itself is admissible.
    ring.publish(&[7u8; MAX_INLINE_BYTES], false)
        .expect("exact");
}

#[test]
fn a_mapping_too_small_for_its_slots_is_refused() {
    // Attaching is the only place the header is trusted, so this is where a
    // mapping that cannot hold what it claims must be caught.
    let mut small = [0u8; RING_HEADER_LEN + RING_SLOT_LEN];
    assert_eq!(
        Ring::format(&mut small, TYPE, SLOTS),
        Err(RingError::Malformed)
    );

    let mut bytes = buffer();
    let truncated = &mut bytes[..RING_HEADER_LEN + RING_SLOT_LEN];
    assert!(matches!(
        Ring::attach(truncated, TYPE, SLOTS),
        Err(RingError::Malformed)
    ));
}

#[test]
fn formatting_refuses_a_slot_count_the_contract_does_not_allow() {
    let mut bytes = [0u8; RING_HEADER_LEN + 8 * RING_SLOT_LEN];
    assert_eq!(Ring::format(&mut bytes, TYPE, 6), Err(RingError::Malformed));
    assert_eq!(Ring::format(&mut bytes, TYPE, 1), Err(RingError::Malformed));
    assert_eq!(Ring::format(&mut bytes, 0, 8), Err(RingError::Malformed));
    Ring::format(&mut bytes, TYPE, 8).expect("a power of two in range");
}

#[test]
fn attaching_with_the_wrong_type_or_slot_count_is_refused() {
    // Both come from the caller's provisioning record. A ring provisioned for
    // another route, or read with the wrong capacity, is refused rather than
    // reinterpreted.
    let mut bytes = buffer();
    assert!(matches!(
        Ring::attach(&mut bytes, TYPE ^ 1, SLOTS),
        Err(RingError::Malformed)
    ));
    assert!(matches!(
        Ring::attach(&mut bytes, TYPE, SLOTS * 2),
        Err(RingError::Malformed)
    ));
}

#[test]
fn formatting_clears_slot_bodies_left_by_previous_use() {
    // A buffer is reusable memory. A reader attaching the instant the header
    // lands must not find a slot body from whatever held this page before.
    let mut bytes = [0xabu8; RING_HEADER_LEN + SLOTS * RING_SLOT_LEN];
    Ring::format(&mut bytes, TYPE, SLOTS).expect("format");
    assert!(
        bytes[RING_HEADER_LEN..].iter().all(|byte| *byte == 0),
        "slot bodies are zeroed before the header is written"
    );
}

#[test]
fn a_claimed_slot_is_never_consumed() {
    // Simulate a publisher preempted mid-copy: the slot is claimed and `head`
    // has not moved. The reader must see an empty ring, not a partial sample.
    let mut bytes = buffer();
    {
        let mut ring = Ring::attach(&mut bytes, TYPE, SLOTS).expect("attach");
        ring.publish(b"visible", false).expect("publish");
    }
    // Hand-write a claimed slot into the position sequence 2 would occupy,
    // without advancing head -- exactly the state a publisher preempted
    // between claiming and committing leaves behind.
    let claimed_index = slime_proto::ring_slot_index(2, SLOTS);
    let next = RING_HEADER_LEN + claimed_index * RING_SLOT_LEN;
    bytes[next..next + 4].copy_from_slice(&SLOT_MAGIC.to_le_bytes());
    bytes[next + 4..next + 8].copy_from_slice(&SLOT_CLAIMED.to_le_bytes());

    let mut ring = Ring::attach(&mut bytes, TYPE, SLOTS).expect("attach");
    let mut out = [0u8; MAX_INLINE_BYTES];
    let (length, _) = ring.consume(&mut out).expect("the committed sample");
    assert_eq!(&out[..length], b"visible");
    assert_eq!(
        ring.consume(&mut out),
        Err(RingError::Empty),
        "the claimed slot is not visible: head never moved"
    );
}

#[test]
fn a_committed_slot_is_written_whole() {
    // One copy, not a patch around whatever was there. A slot rewritten in
    // pieces could be read with fields from two different samples; encoding
    // the whole record is what makes `head` sufficient to publish it.
    let mut bytes = buffer();
    let index = slime_proto::ring_slot_index(1, SLOTS);
    let at = RING_HEADER_LEN + index * RING_SLOT_LEN;
    bytes[at + 8..at + 12].copy_from_slice(&0xdead_beefu32.to_le_bytes());

    {
        let mut ring = Ring::attach(&mut bytes, TYPE, SLOTS).expect("attach");
        ring.publish(b"committed", false).expect("publish");
    }

    let state = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().expect("state"));
    assert_eq!(state, SLOT_READY);
    let flags = u32::from_le_bytes(bytes[at + 8..at + 12].try_into().expect("flags"));
    assert_eq!(
        flags, 0,
        "the poisoned field was overwritten, not preserved"
    );
}

#[test]
fn the_last_flag_is_the_only_slot_flag_this_version_sets() {
    let mut bytes = buffer();
    let mut ring = Ring::attach(&mut bytes, TYPE, SLOTS).expect("attach");
    ring.publish(b"plain", false).expect("publish");
    ring.publish(b"final", true).expect("publish");

    let mut out = [0u8; MAX_INLINE_BYTES];
    assert_eq!(ring.consume(&mut out).expect("first").1, false);
    assert_eq!(ring.consume(&mut out).expect("second").1, true);
    assert_eq!(FLAG_LAST, 1);
}
