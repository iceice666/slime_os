//! Cursor discipline and lease bookkeeping for the I/O queue (IO0).
//!
//! The contract in `contracts/io-queue/v1/` says what the bytes mean; this says
//! how to move through them safely. It is deliberately separate from the
//! generated codec: the codec is regenerated from the schema and must stay
//! mechanical, while the discipline below is the part that would be easy to get
//! subtly wrong in each of the drivers and clients that needs it.
//!
//! # Two halves, and why they are in one module
//!
//! [`SubmissionQueue`] is the client's view and [`CompletionQueue`] the
//! driver's, over the same mapping. They are written together because the
//! invariant that matters spans both: a request occupies a submission slot
//! until its completion is *consumed*, not until it is produced. Splitting them
//! into separate modules would make that easy to state and impossible to check.
//!
//! [`Outstanding`] is the third piece, and it is not shared memory at all. It
//! is each side's own fixed-capacity record of which requests are live, which
//! epoch they belong to, and which lease each retains. The rule that every
//! terminal transition is single-assignment and every lease releases exactly
//! once lives there, because it is a property of a party's own bookkeeping
//! rather than of the bytes between them.
//!
//! # Single writer per field
//!
//! The client owns `submit_head` and `complete_tail`; the driver owns
//! `submit_tail`, `complete_head`, `epoch`, and `driver_state`. No field has two
//! writers, which is what makes the ready-state handshake sufficient without a
//! lock. The header's two-cache-line split keeps those two sets of writes off
//! each other's lines.
//!
//! # What this is not
//!
//! Not a lock, and not a memory model. Every operation is expressed against a
//! caller-supplied byte slice, so the caller owns the ordering guarantees its
//! platform needs -- on the seL4 side that is the shared mapping plus the
//! notification that follows a publish. What this owns is the *protocol*: which
//! slot a sequence occupies, when a slot may be read, when a ring is full, which
//! epoch a completion belongs to, and what a reader must conclude when it finds
//! something it did not expect.

use crate::io_queue::{
    self, COMPLETION_PAYLOAD_BYTES, COMPLETION_SLOT_LEN, QUEUE_HEADER_LEN, REQUEST_PAYLOAD_BYTES,
    REQUEST_SLOT_LEN, WireCompletionSlot, WireQueueHeader, WireRequestSlot,
};
use crate::{
    queue_slot_index, terminal_state_for_status, valid_completion_slot, valid_queue_header,
    valid_request_slot,
};

/// Why a queue operation could not proceed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueueError {
    /// The mapping is too small for the header plus the rings it declares, or
    /// the header does not describe a queue this party can use.
    ///
    /// One error for both because the caller's response must be the same:
    /// refuse the queue. A mapping that disagrees with its header is not a
    /// recoverable condition -- there is no correct way to read half of it.
    Malformed,
    /// Every slot in the ring holds an entry the peer has not consumed.
    ///
    /// This is backpressure, not failure: the caller waits for a notification
    /// and retries. The ring never overwrites an unconsumed entry, which is why
    /// this exists as a distinct answer rather than a silent drop.
    Full,
    /// No entry is ready. The caller waits for a notification.
    Empty,
    /// The protocol payload does not fit one slot.
    TooLarge,
    /// The queue's epoch is not the one the caller is serving.
    ///
    /// A restart, reset, or generation transition advances the epoch, so this
    /// is how a party learns its view is stale. It is distinct from
    /// [`QueueError::Malformed`] because the mapping is well-formed -- it just
    /// belongs to a different driver incarnation.
    StaleEpoch,
    /// The driver is resetting or dead, so no new work may be submitted.
    ///
    /// Terminal answers may still be drained: that is how a reset reaches
    /// quiescence without a timeout.
    Closed,
    /// The outstanding table is full, or the request identity is already live.
    ///
    /// Duplicate identities within an epoch are refused rather than merged,
    /// because two requests sharing an identity cannot both be settled.
    Exhausted,
    /// No outstanding request matches this completion.
    ///
    /// This is the late-completion rejection: a request already settled by
    /// cancellation, reset, or peer death is no longer in the table, so a
    /// completion naming it cannot resurrect it or its lease.
    Unknown,
}

/// The bytes one queue mapping must be, for a given ring depth.
///
/// Both rings carry `slot_count` slots: a completion ring smaller than its
/// submission ring could strand a terminal answer, and a request whose terminal
/// state cannot be delivered is exactly the leak this substrate prevents.
pub fn mapping_bytes(slot_count: usize) -> Option<usize> {
    let requests = slot_count.checked_mul(REQUEST_SLOT_LEN)?;
    let completions = slot_count.checked_mul(COMPLETION_SLOT_LEN)?;
    QUEUE_HEADER_LEN
        .checked_add(requests)?
        .checked_add(completions)
}

fn admissible_slot_count(slot_count: usize) -> bool {
    (io_queue::MIN_QUEUE_SLOTS..=io_queue::MAX_QUEUE_SLOTS).contains(&slot_count)
        && slot_count.is_power_of_two()
}

/// Write an initial header and zero both rings.
///
/// Called once by whoever provisions the buffer, before either party attaches.
/// The rings are zeroed before the header lands, so a party that attaches the
/// instant the header appears never sees a slot body left by whatever used this
/// memory before.
///
/// `epoch` must be non-zero: epoch zero means "no driver has claimed this
/// queue", and a queue formatted as active at epoch zero would admit work no
/// incarnation owns.
pub fn format(bytes: &mut [u8], slot_count: usize, epoch: u64) -> Result<(), QueueError> {
    if !admissible_slot_count(slot_count) || epoch == 0 {
        return Err(QueueError::Malformed);
    }
    let required = mapping_bytes(slot_count).ok_or(QueueError::Malformed)?;
    if bytes.len() < required {
        return Err(QueueError::Malformed);
    }
    bytes[QUEUE_HEADER_LEN..required].fill(0);
    let header = WireQueueHeader {
        magic: io_queue::QUEUE_MAGIC,
        version: io_queue::FORMAT_VERSION,
        slot_count: slot_count as u32,
        request_slot_len: REQUEST_SLOT_LEN as u32,
        completion_slot_len: COMPLETION_SLOT_LEN as u32,
        client_reserved: [0; 4],
        submit_head: 0,
        complete_tail: 0,
        client_padding: [0; 24],
        driver_state: io_queue::DRIVER_ACTIVE,
        driver_reserved: [0; 4],
        epoch,
        complete_head: 0,
        submit_tail: 0,
        driver_padding: [0; 32],
    };
    bytes[..QUEUE_HEADER_LEN].copy_from_slice(&header.encode());
    Ok(())
}

/// A validated view of one queue mapping.
///
/// Constructing this is the only place a header is trusted, so no caller can
/// reach a slot without having gone through the bounds check first. Both the
/// client and the driver attach the same way and then use the half that belongs
/// to them; the type does not enforce which side a caller is, because a mapping
/// grants no authority on its own -- the capability that produced it does.
pub struct Queue<'a> {
    bytes: &'a mut [u8],
    slot_count: usize,
    completion_base: usize,
}

impl<'a> Queue<'a> {
    /// Validate a mapping against what the generation provisioned.
    ///
    /// `expected_slots` comes from the caller's own provisioning record, never
    /// from the mapping: that is exactly the value a hostile or broken peer
    /// would want to choose.
    pub fn attach(bytes: &'a mut [u8], expected_slots: usize) -> Result<Self, QueueError> {
        if !admissible_slot_count(expected_slots) {
            return Err(QueueError::Malformed);
        }
        let required = mapping_bytes(expected_slots).ok_or(QueueError::Malformed)?;
        if bytes.len() < required {
            return Err(QueueError::Malformed);
        }
        let header =
            WireQueueHeader::decode(&bytes[..QUEUE_HEADER_LEN]).ok_or(QueueError::Malformed)?;
        if !valid_queue_header(&header, expected_slots) {
            return Err(QueueError::Malformed);
        }
        let completion_base = QUEUE_HEADER_LEN + expected_slots * REQUEST_SLOT_LEN;
        Ok(Self {
            bytes,
            slot_count: expected_slots,
            completion_base,
        })
    }

    fn header(&self) -> WireQueueHeader {
        // Validated at `attach`, and only this type writes it afterwards.
        WireQueueHeader::decode(&self.bytes[..QUEUE_HEADER_LEN])
            .unwrap_or_else(|| unreachable!("attach validated the header"))
    }

    fn put_header(&mut self, header: WireQueueHeader) {
        self.bytes[..QUEUE_HEADER_LEN].copy_from_slice(&header.encode());
    }

    fn request_range(&self, sequence: u64) -> core::ops::Range<usize> {
        let index = queue_slot_index(sequence, self.slot_count);
        let start = QUEUE_HEADER_LEN + index * REQUEST_SLOT_LEN;
        start..start + REQUEST_SLOT_LEN
    }

    fn completion_range(&self, sequence: u64) -> core::ops::Range<usize> {
        let index = queue_slot_index(sequence, self.slot_count);
        let start = self.completion_base + index * COMPLETION_SLOT_LEN;
        start..start + COMPLETION_SLOT_LEN
    }

    /// The epoch this queue is serving.
    pub fn epoch(&self) -> u64 {
        self.header().epoch
    }

    /// The driver's declared state.
    pub fn driver_state(&self) -> u32 {
        self.header().driver_state
    }

    /// Slots per ring, as validated at attach.
    pub fn slot_count(&self) -> usize {
        self.slot_count
    }

    /// Requests submitted but not yet taken by the driver.
    pub fn submitted(&self) -> u64 {
        let header = self.header();
        header.submit_head - header.submit_tail
    }

    /// Completions produced but not yet taken by the client.
    pub fn completions_pending(&self) -> u64 {
        let header = self.header();
        header.complete_head - header.complete_tail
    }

    /// Whether new work may be submitted.
    ///
    /// A resetting or dead driver refuses submissions while still permitting
    /// its client to drain terminal answers.
    pub fn accepting(&self) -> bool {
        self.header().driver_state == io_queue::DRIVER_ACTIVE
    }

    // -- client half ------------------------------------------------------

    /// Submit one request. Client side.
    ///
    /// Writes the whole slot -- body and ready state in one copy -- then
    /// advances `submit_head`. A driver interrupting at any point sees either
    /// the previous request or nothing new, never this one half-written,
    /// because `submit_head` is what makes a slot visible and it moves last.
    ///
    /// The lease named by `slice` must already be live: this cannot verify
    /// that, because a lease is root-held state and the client holds only a
    /// handle. What this does enforce is that the slice is structurally
    /// admissible against `mapped_len`, so a descriptor that would send a
    /// device past the end of its lease is refused before the driver sees it.
    pub fn submit(
        &mut self,
        request_id: u64,
        slice: &io_queue::WireBufferSlice,
        payload: &[u8],
        fenced: bool,
        mapped_len: u64,
    ) -> Result<u64, QueueError> {
        if payload.len() > REQUEST_PAYLOAD_BYTES {
            return Err(QueueError::TooLarge);
        }
        let mut header = self.header();
        if header.driver_state != io_queue::DRIVER_ACTIVE {
            return Err(QueueError::Closed);
        }
        if header.submit_head - header.submit_tail >= self.slot_count as u64 {
            return Err(QueueError::Full);
        }
        if request_id == 0 {
            return Err(QueueError::Malformed);
        }
        let mut body = [0u8; REQUEST_PAYLOAD_BYTES];
        body[..payload.len()].copy_from_slice(payload);
        let slot = WireRequestSlot {
            magic: io_queue::REQUEST_MAGIC,
            state: io_queue::SLOT_READY,
            flags: if fenced { io_queue::FLAG_FENCED } else { 0 },
            payload_len: payload.len() as u32,
            request_id,
            epoch: header.epoch,
            slice_buffer: slice.buffer,
            slice_lease: slice.lease,
            slice_offset: slice.offset,
            slice_length: slice.length,
            slice_direction: slice.direction,
            slice_reserved: slice.reserved,
            payload: body,
        };
        // Validate what is about to be written rather than trusting the
        // caller: this is the one place a malformed slice can still be refused
        // without a device having been programmed.
        if !valid_request_slot(&slot, header.epoch, mapped_len) {
            return Err(QueueError::Malformed);
        }
        // Sequences number from one, so zero always means "never written".
        let sequence = header.submit_head + 1;
        let range = self.request_range(sequence);
        self.bytes[range].copy_from_slice(&slot.encode());
        header.submit_head = sequence;
        self.put_header(header);
        Ok(sequence)
    }

    /// Take the next completion. Client side.
    ///
    /// `expected` names the request this completion must answer and the epoch
    /// it must belong to, both from the caller's own outstanding table. A
    /// completion that matches neither is [`QueueError::Unknown`] -- the late
    /// completion this substrate exists to reject -- and the caller must
    /// discard it without touching the request or its lease.
    ///
    /// The completion is consumed either way: leaving a rejected entry in the
    /// ring would wedge every later answer behind it.
    pub fn take_completion<const N: usize>(
        &mut self,
        expected: &Outstanding<N>,
        out: &mut [u8; COMPLETION_PAYLOAD_BYTES],
    ) -> Result<Completion, QueueError> {
        let mut header = self.header();
        if header.complete_head == header.complete_tail {
            return Err(QueueError::Empty);
        }
        let sequence = header.complete_tail + 1;
        let range = self.completion_range(sequence);
        let slot = WireCompletionSlot::decode(&self.bytes[range]).ok_or(QueueError::Malformed)?;

        // Consume before judging. A completion whose identity is unknown is
        // still an entry the driver produced and the ring must advance past.
        header.complete_tail = sequence;
        self.put_header(header);

        let live = expected
            .find(slot.request_id, slot.epoch)
            .ok_or(QueueError::Unknown)?;
        if !valid_completion_slot(&slot, live.request_id, live.epoch, live.slice_length) {
            return Err(QueueError::Malformed);
        }
        let length = slot.payload_len as usize;
        out[..length].copy_from_slice(&slot.payload[..length]);
        out[length..].fill(0);
        Ok(Completion {
            request_id: slot.request_id,
            epoch: slot.epoch,
            status: slot.status,
            transferred: slot.transferred,
            payload_len: length,
            epoch_ended: slot.flags & io_queue::FLAG_EPOCH_ENDED != 0,
        })
    }

    // -- driver half ------------------------------------------------------

    /// Take the next submitted request. Driver side.
    ///
    /// `mapped_len` is the driver's own knowledge of the lease mapping length,
    /// so a slice the client wrote past the end of its lease is refused here
    /// even if the client's own check was skipped or subverted. The request is
    /// consumed either way, and a malformed one must be answered with a
    /// [`io_queue::STATUS_MALFORMED`] completion rather than dropped: a request
    /// with no terminal state is a leaked lease.
    pub fn take_request(
        &mut self,
        out: &mut [u8; REQUEST_PAYLOAD_BYTES],
        mapped_len: u64,
    ) -> Result<Submission, QueueError> {
        let mut header = self.header();
        if header.submit_head == header.submit_tail {
            return Err(QueueError::Empty);
        }
        let sequence = header.submit_tail + 1;
        let range = self.request_range(sequence);
        let slot = WireRequestSlot::decode(&self.bytes[range]).ok_or(QueueError::Malformed)?;

        header.submit_tail = sequence;
        self.put_header(header);

        if !valid_request_slot(&slot, header.epoch, mapped_len) {
            // The identity is still reported when it is usable, so the driver
            // can answer the request it cannot honour. A zero identity means
            // even that is unavailable and the entry is unanswerable.
            return Err(if slot.epoch != header.epoch {
                QueueError::StaleEpoch
            } else {
                QueueError::Malformed
            });
        }
        let length = slot.payload_len as usize;
        out[..length].copy_from_slice(&slot.payload[..length]);
        out[length..].fill(0);
        Ok(Submission {
            request_id: slot.request_id,
            epoch: slot.epoch,
            slice: io_queue::WireBufferSlice {
                buffer: slot.slice_buffer,
                lease: slot.slice_lease,
                offset: slot.slice_offset,
                length: slot.slice_length,
                direction: slot.slice_direction,
                reserved: slot.slice_reserved,
            },
            payload_len: length,
            fenced: slot.flags & io_queue::FLAG_FENCED != 0,
        })
    }

    /// Publish one completion. Driver side.
    ///
    /// Refuses at capacity rather than overwriting, like the submission ring.
    /// A full completion ring is why `MAX_OUTSTANDING` equals the ring depth:
    /// a driver that admitted more work than it could answer would have to
    /// choose between overwriting a terminal state and never delivering one.
    pub fn complete(
        &mut self,
        request_id: u64,
        status: u32,
        transferred: u64,
        payload: &[u8],
        epoch_ended: bool,
    ) -> Result<u64, QueueError> {
        if payload.len() > COMPLETION_PAYLOAD_BYTES {
            return Err(QueueError::TooLarge);
        }
        let mut header = self.header();
        if header.complete_head - header.complete_tail >= self.slot_count as u64 {
            return Err(QueueError::Full);
        }
        if request_id == 0 || terminal_state_for_status(status).is_none() {
            return Err(QueueError::Malformed);
        }
        let mut body = [0u8; COMPLETION_PAYLOAD_BYTES];
        body[..payload.len()].copy_from_slice(payload);
        let slot = WireCompletionSlot {
            magic: io_queue::COMPLETION_MAGIC,
            status,
            flags: if epoch_ended {
                io_queue::FLAG_EPOCH_ENDED
            } else {
                0
            },
            payload_len: payload.len() as u32,
            request_id,
            epoch: header.epoch,
            transferred,
            payload: body,
        };
        let sequence = header.complete_head + 1;
        let range = self.completion_range(sequence);
        self.bytes[range].copy_from_slice(&slot.encode());
        header.complete_head = sequence;
        self.put_header(header);
        Ok(sequence)
    }

    /// Enter the resetting state. Driver side.
    ///
    /// Submissions stop being admitted immediately; the driver then settles
    /// every outstanding request and calls [`Queue::advance_epoch`]. Separating
    /// the two is what lets a client observe the reset and stop producing
    /// before the epoch moves under it.
    pub fn begin_reset(&mut self) {
        let mut header = self.header();
        if header.driver_state == io_queue::DRIVER_ACTIVE {
            header.driver_state = io_queue::DRIVER_RESETTING;
            self.put_header(header);
        }
    }

    /// Start a fresh epoch with both rings emptied. Driver side.
    ///
    /// Every position returns to zero because sequences are per-epoch: carrying
    /// them across would let a stale completion from the previous incarnation
    /// land on a live slot. The caller must have settled every outstanding
    /// request first -- this cannot check that, because the outstanding table
    /// is the caller's own state, which is exactly why
    /// [`Outstanding::settle_all`] returns the leases it released.
    pub fn advance_epoch(&mut self) -> Result<u64, QueueError> {
        let mut header = self.header();
        let next = header.epoch.checked_add(1).ok_or(QueueError::Malformed)?;
        let required = mapping_bytes(self.slot_count).ok_or(QueueError::Malformed)?;
        self.bytes[QUEUE_HEADER_LEN..required].fill(0);
        header.epoch = next;
        header.submit_head = 0;
        header.submit_tail = 0;
        header.complete_head = 0;
        header.complete_tail = 0;
        header.driver_state = io_queue::DRIVER_ACTIVE;
        self.put_header(header);
        Ok(next)
    }

    /// Record that the driver's task died without resetting.
    ///
    /// Written by the root during reclamation, so a client blocked on a dead
    /// driver learns it from the mapping rather than from a root round trip.
    pub fn mark_driver_dead(&mut self) {
        let mut header = self.header();
        if header.driver_state != io_queue::DRIVER_DEAD {
            header.driver_state = io_queue::DRIVER_DEAD;
            self.put_header(header);
        }
    }
}

/// One request as the driver received it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Submission {
    pub request_id: u64,
    pub epoch: u64,
    pub slice: io_queue::WireBufferSlice,
    pub payload_len: usize,
    pub fenced: bool,
}

/// One terminal answer as the client received it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Completion {
    pub request_id: u64,
    pub epoch: u64,
    pub status: u32,
    pub transferred: u64,
    pub payload_len: usize,
    pub epoch_ended: bool,
}

/// One live request in a party's own bookkeeping.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Live {
    pub request_id: u64,
    pub epoch: u64,
    /// The lease this request retains, or zero for a control request.
    pub lease: u64,
    /// The slice length, kept so a completion's `transferred` can be bounded
    /// without re-reading the submission slot -- which by then may have been
    /// overwritten by a later request.
    pub slice_length: u64,
    /// Lifecycle state. Always transient here: a terminal state means the entry
    /// has been removed.
    pub state: u32,
}

/// A party's fixed-capacity record of its live requests.
///
/// This is the single-assignment enforcement point. A terminal transition
/// *removes* the entry and returns the lease it retained, so a second terminal
/// transition for the same identity finds nothing and is refused. That is what
/// makes "every buffer lease released or invalidated exactly once" a structural
/// property rather than a convention each driver must remember.
///
/// `N` is the capacity, which the generation declares. It never exceeds
/// [`io_queue::MAX_OUTSTANDING`], and admission is refused at capacity rather
/// than growing: an unbounded outstanding table is an unbounded lease table.
pub struct Outstanding<const N: usize> {
    entries: [Option<Live>; N],
    epoch: u64,
}

impl<const N: usize> Outstanding<N> {
    /// An empty table serving `epoch`.
    pub fn new(epoch: u64) -> Self {
        Self {
            entries: [None; N],
            epoch,
        }
    }

    /// The epoch this table is serving.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Live requests.
    pub fn len(&self) -> usize {
        self.entries.iter().filter(|entry| entry.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.iter().all(|entry| entry.is_none())
    }

    /// Capacity, as declared by the generation.
    pub fn capacity(&self) -> usize {
        N
    }

    /// Admit one request as `STATE_QUEUED`.
    ///
    /// Refuses a duplicate identity within the epoch rather than merging: two
    /// requests sharing an identity cannot both be settled, and the second
    /// would silently inherit the first's lease.
    pub fn admit(
        &mut self,
        request_id: u64,
        lease: u64,
        slice_length: u64,
    ) -> Result<(), QueueError> {
        if request_id == 0 {
            return Err(QueueError::Malformed);
        }
        if self.find(request_id, self.epoch).is_some() {
            return Err(QueueError::Exhausted);
        }
        let slot = self
            .entries
            .iter_mut()
            .find(|entry| entry.is_none())
            .ok_or(QueueError::Exhausted)?;
        *slot = Some(Live {
            request_id,
            epoch: self.epoch,
            lease,
            slice_length,
            state: io_queue::STATE_QUEUED,
        });
        Ok(())
    }

    /// The live entry for an identity and epoch, if any.
    ///
    /// Both are required because an identity is unique only within an epoch. A
    /// completion carrying the right identity and the wrong epoch belongs to a
    /// dead incarnation and must not resolve.
    pub fn find(&self, request_id: u64, epoch: u64) -> Option<Live> {
        self.entries
            .iter()
            .flatten()
            .copied()
            .find(|entry| entry.request_id == request_id && entry.epoch == epoch)
    }

    /// Mark a queued request as in flight.
    ///
    /// Refused from any state other than `STATE_QUEUED`, so a driver cannot
    /// start the same request twice.
    pub fn start(&mut self, request_id: u64) -> Result<(), QueueError> {
        let epoch = self.epoch;
        let entry = self
            .entries
            .iter_mut()
            .flatten()
            .find(|entry| entry.request_id == request_id && entry.epoch == epoch)
            .ok_or(QueueError::Unknown)?;
        if entry.state != io_queue::STATE_QUEUED {
            return Err(QueueError::Malformed);
        }
        entry.state = io_queue::STATE_IN_FLIGHT;
        Ok(())
    }

    /// Settle one request into a terminal state, returning what it retained.
    ///
    /// The entry is removed, so this succeeds at most once per identity per
    /// epoch. A second attempt is [`QueueError::Unknown`] -- which is exactly
    /// the answer a late completion after cancellation must receive.
    pub fn settle(&mut self, request_id: u64, status: u32) -> Result<Settled, QueueError> {
        let terminal = terminal_state_for_status(status).ok_or(QueueError::Malformed)?;
        let epoch = self.epoch;
        let slot = self
            .entries
            .iter_mut()
            .find(|entry| {
                entry
                    .map(|live| live.request_id == request_id && live.epoch == epoch)
                    .unwrap_or(false)
            })
            .ok_or(QueueError::Unknown)?;
        let live = slot.take().unwrap_or_else(|| unreachable!("just matched"));
        Ok(Settled {
            request_id: live.request_id,
            epoch: live.epoch,
            lease: live.lease,
            state: terminal,
        })
    }

    /// Settle every live request into one terminal state.
    ///
    /// Used by reset (`STATUS_RESET`) and by the root on driver death
    /// (`STATUS_PEER_DEAD`). Each released lease is handed to `release` in
    /// turn, so no caller has to walk the table itself and risk releasing one
    /// twice or none at all. Returns how many were settled.
    pub fn settle_all(&mut self, status: u32, mut release: impl FnMut(Settled)) -> usize {
        let Some(terminal) = terminal_state_for_status(status) else {
            return 0;
        };
        let mut settled = 0;
        for slot in self.entries.iter_mut() {
            if let Some(live) = slot.take() {
                release(Settled {
                    request_id: live.request_id,
                    epoch: live.epoch,
                    lease: live.lease,
                    state: terminal,
                });
                settled += 1;
            }
        }
        settled
    }

    /// Adopt a fresh epoch.
    ///
    /// Refuses while any request is still live, because advancing with live
    /// entries would orphan their leases: nothing would ever match their
    /// identities again, so nothing would ever release them. Callers reset by
    /// calling [`Outstanding::settle_all`] first, which is what makes the leak
    /// impossible rather than merely discouraged.
    pub fn adopt_epoch(&mut self, epoch: u64) -> Result<(), QueueError> {
        if epoch == 0 || epoch <= self.epoch {
            return Err(QueueError::Malformed);
        }
        if !self.is_empty() {
            return Err(QueueError::Exhausted);
        }
        self.epoch = epoch;
        Ok(())
    }
}

/// What settling one request released.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Settled {
    pub request_id: u64,
    pub epoch: u64,
    /// The lease to release, or zero if the request held none.
    pub lease: u64,
    /// The terminal state assigned.
    pub state: u32,
}
