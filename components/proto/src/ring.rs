//! Reading and writing the v2 shared ring (B46).
//!
//! The contract in `contracts/fabric-stream/v2/` says what the bytes mean;
//! this says how to move through them safely. It is deliberately separate from
//! the generated codec: the codec is regenerated from the schema and must stay
//! mechanical, while the cursor discipline below is the part that would be
//! easy to get subtly wrong in each of the components that needs it.
//!
//! # What this is not
//!
//! Not a lock, and not a memory model. Every operation here is expressed
//! against a caller-supplied byte slice, so the caller owns the ordering
//! guarantees its platform needs — on the seL4 side that is the shared mapping
//! and the notification that follows a publish. What this owns is the
//! *protocol*: which slot a sequence occupies, when a slot may be read, when
//! the ring is full, and what a reader must conclude when it finds a sequence
//! it did not expect.
//!
//! # Single writer, single reader
//!
//! One publisher owns `head` and the slot bodies; one subscriber owns `tail`.
//! A route with several subscribers provisions a ring each, which is why no
//! entry here is written by two parties. That is the property that makes the
//! claimed/ready handshake sufficient without a lock: a reader never observes
//! a partially written slot because it never advances past `SLOT_CLAIMED`, and
//! a writer never overwrites an unread slot because it stops at capacity.

use crate::fabric_ring::{
    self, MAX_INLINE_BYTES, RING_HEADER_LEN, RING_SLOT_LEN, WireRingHeader, WireRingSlot,
};
use crate::{ring_slot_index, valid_ring_header, valid_ring_slot};

/// Why a ring operation could not proceed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RingError {
    /// The mapping is too small for the header plus the slots it declares, or
    /// the header does not describe a ring this reader can use.
    ///
    /// One error for both because the caller's response is the same and must
    /// be: refuse the ring. A mapping that disagrees with its header is not a
    /// recoverable condition — there is no correct way to read half of it.
    Malformed,
    /// Every slot holds an unread sample. The publisher must wait for credit.
    Full,
    /// No sample is ready. The subscriber must wait for a notification.
    Empty,
    /// The payload does not fit one slot. Larger samples travel as a
    /// descriptor naming a loan, not inline.
    TooLarge,
}

/// A validated view of one ring's mapping.
///
/// Constructing this is the only place a header is trusted, so a caller cannot
/// reach a slot without having gone through the bounds check first.
pub struct Ring<'a> {
    bytes: &'a mut [u8],
    slot_count: usize,
    type_identity: u64,
}

impl<'a> Ring<'a> {
    /// Validate a mapping against what the fabric provisioned.
    ///
    /// `expected_slots` and `expected_type` come from the caller's own
    /// provisioning record, never from the mapping: those are exactly the
    /// values a hostile or broken peer would want to choose.
    pub fn attach(
        bytes: &'a mut [u8],
        expected_type: u64,
        expected_slots: usize,
    ) -> Result<Self, RingError> {
        let required = RING_HEADER_LEN
            .checked_add(
                expected_slots
                    .checked_mul(RING_SLOT_LEN)
                    .ok_or(RingError::Malformed)?,
            )
            .ok_or(RingError::Malformed)?;
        if bytes.len() < required {
            return Err(RingError::Malformed);
        }
        let header =
            WireRingHeader::decode(&bytes[..RING_HEADER_LEN]).ok_or(RingError::Malformed)?;
        if !valid_ring_header(&header, expected_type, expected_slots) {
            return Err(RingError::Malformed);
        }
        Ok(Self {
            bytes,
            slot_count: expected_slots,
            type_identity: expected_type,
        })
    }

    /// Write an initial header. Called once by whoever provisions the buffer,
    /// before either peer attaches.
    pub fn format(
        bytes: &mut [u8],
        type_identity: u64,
        slot_count: usize,
    ) -> Result<(), RingError> {
        if !(fabric_ring::MIN_RING_SLOTS..=fabric_ring::MAX_RING_SLOTS).contains(&slot_count)
            || !slot_count.is_power_of_two()
        {
            return Err(RingError::Malformed);
        }
        let required = RING_HEADER_LEN
            .checked_add(
                slot_count
                    .checked_mul(RING_SLOT_LEN)
                    .ok_or(RingError::Malformed)?,
            )
            .ok_or(RingError::Malformed)?;
        if bytes.len() < required || type_identity == 0 {
            return Err(RingError::Malformed);
        }
        // Zero the slots before the header, so a reader that attaches the
        // instant the header lands never sees a slot body left by whatever
        // used this memory before.
        bytes[RING_HEADER_LEN..required].fill(0);
        let header = WireRingHeader {
            magic: fabric_ring::RING_MAGIC,
            version: fabric_ring::FORMAT_VERSION,
            slot_count: slot_count as u32,
            slot_len: RING_SLOT_LEN as u32,
            head: 0,
            tail: 0,
            type_identity,
            producer_state: fabric_ring::PRODUCER_ACTIVE,
            lost: 0,
            reserved: [0; 16],
        };
        bytes[..RING_HEADER_LEN].copy_from_slice(&header.encode());
        Ok(())
    }

    fn header(&self) -> WireRingHeader {
        // Validated at `attach`, and only this type writes it afterwards.
        WireRingHeader::decode(&self.bytes[..RING_HEADER_LEN])
            .unwrap_or_else(|| unreachable!("attach validated the header"))
    }

    fn put_header(&mut self, header: WireRingHeader) {
        self.bytes[..RING_HEADER_LEN].copy_from_slice(&header.encode());
    }

    fn slot_range(&self, sequence: u64) -> core::ops::Range<usize> {
        let index = ring_slot_index(sequence, self.slot_count);
        let start = RING_HEADER_LEN + index * RING_SLOT_LEN;
        start..start + RING_SLOT_LEN
    }

    /// Samples published but not yet consumed.
    pub fn occupancy(&self) -> u64 {
        let header = self.header();
        header.head - header.tail
    }

    /// Whether the publisher has stopped, and how.
    pub fn producer_state(&self) -> u32 {
        self.header().producer_state
    }

    /// Publish one sample.
    ///
    /// Writes the body while the slot reads `SLOT_CLAIMED`, then marks it
    /// ready, then advances `head`. A reader interrupting at any point sees
    /// either the previous sample or nothing new — never this one half-written
    /// — because `head` is what makes a slot visible and it moves last.
    pub fn publish(&mut self, payload: &[u8], last: bool) -> Result<u64, RingError> {
        if payload.is_empty() || payload.len() > MAX_INLINE_BYTES {
            return Err(RingError::TooLarge);
        }
        let mut header = self.header();
        if header.head - header.tail >= self.slot_count as u64 {
            return Err(RingError::Full);
        }
        // Sequences number from one, so zero always means "never written".
        let sequence = header.head + 1;
        let range = self.slot_range(sequence);

        // No separate claim step. `WireRingSlot::encode` writes the whole slot
        // including its state, and `head` is what makes the slot visible --
        // so a reader either sees the previous sample or this complete one.
        // A claim marker would only matter if the body could be written in
        // pieces a reader might interleave with, and it cannot: the encode is
        // one copy, and nothing reads past `head`.
        //
        // `SLOT_CLAIMED` stays in the contract because it is what a *future*
        // writer that streams a payload in would need, and because
        // `valid_ring_slot` must keep refusing it either way.
        let mut body = [0u8; MAX_INLINE_BYTES];
        body[..payload.len()].copy_from_slice(payload);
        let slot = WireRingSlot {
            magic: fabric_ring::SLOT_MAGIC,
            state: fabric_ring::SLOT_READY,
            flags: if last { fabric_ring::FLAG_LAST } else { 0 },
            payload_len: payload.len() as u32,
            sequence,
            type_identity: self.type_identity,
            payload: body,
        };
        self.bytes[range].copy_from_slice(&slot.encode());

        header.head = sequence;
        if last {
            header.producer_state = fabric_ring::PRODUCER_FINISHED;
        }
        self.put_header(header);
        Ok(sequence)
    }

    /// Consume the next sample into `out`, returning its length and whether it
    /// was the last.
    ///
    /// A sequence other than the one owed is [`RingError::Malformed`], not a
    /// drop to be resynchronized past.
    ///
    /// This ring cannot drop: `publish` refuses at capacity rather than
    /// overwriting, and one reader owns `tail`, so the slot at `tail + 1` is
    /// either the awaited sample or has never been written. Finding a
    /// different sequence there means the mapping is not what this reader
    /// thinks it is — a second writer, or a stale view — and continuing would
    /// deliver another route's bytes.
    ///
    /// BEST_EFFORT delivery, where a slow subscriber is *meant* to lose
    /// samples, needs a publisher that overwrites; that is a policy the fabric
    /// applies above this cursor by draining on the subscriber's behalf, not a
    /// state this ring can reach on its own.
    pub fn consume(
        &mut self,
        out: &mut [u8; MAX_INLINE_BYTES],
    ) -> Result<(usize, bool), RingError> {
        let mut header = self.header();
        if header.head == header.tail {
            return Err(RingError::Empty);
        }
        let expected = header.tail + 1;
        let range = self.slot_range(expected);
        let slot = WireRingSlot::decode(&self.bytes[range]).ok_or(RingError::Malformed)?;

        if !valid_ring_slot(&slot, self.type_identity, expected) {
            return Err(RingError::Malformed);
        }

        let length = slot.payload_len as usize;
        out[..length].copy_from_slice(&slot.payload[..length]);
        out[length..].fill(0);

        header.tail = expected;
        self.put_header(header);
        Ok((length, slot.flags & fabric_ring::FLAG_LAST != 0))
    }

    /// Record that the publisher's task died without finishing.
    ///
    /// Written by the root during reclamation, so a subscriber blocked on a
    /// dead peer learns it from the ring rather than from a root round trip —
    /// which is what v1 needed a logical channel to report.
    pub fn mark_producer_dead(&mut self) {
        let mut header = self.header();
        if header.producer_state == fabric_ring::PRODUCER_ACTIVE {
            header.producer_state = fabric_ring::PRODUCER_DEAD;
            self.put_header(header);
        }
    }

    /// Samples this ring has dropped, as counted by `consume`.
    pub fn lost(&self) -> u32 {
        self.header().lost
    }
}
