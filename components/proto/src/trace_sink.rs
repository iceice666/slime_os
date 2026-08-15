//! The bounded semantic-trace sink (C8.11).
//!
//! `contracts/fabric-trace/v1/` says what a record means; this says how a
//! worker accumulates records without growing, losing evidence silently, or
//! reordering an instant. It is separate from the generated codec for the same
//! reason [`crate::ring`] is: the codec stays mechanical and regenerable, while
//! the discipline below is the part each worker would otherwise reimplement
//! slightly differently.
//!
//! # What bounded means here
//!
//! Capacity is fixed at construction from the generation's declared
//! `traceDepth`, and the backing array is sized by `MAX_TRACE_DEPTH` at compile
//! time, so a sink never allocates and never exceeds the contract ceiling. Of
//! that capacity, `TERMINAL_RESERVE` slots are unavailable to ordinary
//! evidence: they exist so a saturated sink can still record that the trace
//! ended. Without the reservation, a sink that filled with routine records
//! would have nowhere to write the one record a reader needs to distinguish a
//! completed trace from a truncated one.
//!
//! # What ordered means here
//!
//! Records are kept sorted by the declared key `(now_ns, order_class,
//! sequence)`, inserted into position rather than appended. That ordering *is*
//! the determinism C8.11 asks for: identical inputs must produce a
//! byte-identical artifact "independent of serial-log interleaving", and a
//! worker's sweep visits its endpoints in whatever order they became ready. Two
//! boots that observe the same events in different sweep orders therefore write
//! the same artifact only because the sink, not the scheduler, decides the
//! sequence.
//!
//! An earlier revision refused any record that would land before the last one
//! recorded, on the theory that an emitter which observes events out of order
//! has a bug. That confused a tie order with an admission rule. The tie order
//! says how records bearing one instant are *arranged*; it says nothing about
//! the order in which a broker may legitimately observe them, and a broker
//! genuinely does see an acknowledgement before a data record at the same
//! simulated instant — its sweep drains client endpoints before server replies.
//! Refusing those records discarded real evidence and reported it as an
//! emitter defect.
//!
//! What remains rejected is a record dated *before the sink's own clock*: the
//! clock is monotone by contract, so a record from a retired instant is a
//! genuine defect rather than a permitted reordering.
//!
//! # Overflow
//!
//! One discipline: saturate. A full sink keeps the records the declared order
//! puts first, drops the one that sorts last, and counts every drop, reporting
//! the total in a single record flagged `FLAG_DROPPED`.
//!
//! "The one that sorts last" rather than "whichever arrived last", because
//! membership is what the byte comparison reads. Refusing by arrival would make
//! the retained set the first N records a worker happened to observe, so two
//! boots whose sweeps differ would keep different subsets while reporting the
//! same drop count — reintroducing scheduler dependence in exactly the regime
//! the declared depth governs.
//!
//! Dropping the *oldest* record was considered and rejected: the earliest
//! records are the admission and provisioning evidence a reader needs to
//! interpret everything after them. Under the declared order the oldest records
//! are also the earliest-sorting ones, so keeping the sorting prefix keeps them.

use crate::fabric_trace::{
    self, FLAG_DROPPED, FLAG_TERMINAL, MAX_TRACE_DEPTH, ORDER_TIME, TERMINAL_RESERVE,
    WireTraceRecord,
};
use crate::{trace_records_ordered, valid_trace_record};

/// Why a record could not be recorded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TraceError {
    /// The declared capacity is outside what the contract admits, or leaves no
    /// room for ordinary evidence once the terminal slots are reserved.
    BadCapacity,
    /// The record is not a well-formed member of any declared family.
    Malformed,
    /// The record would land before one already recorded. This is an ordering
    /// defect in the emitter, not backpressure.
    OutOfOrder,
    /// The sink is full. The record was counted, not stored, and the count is
    /// reported by [`TraceSink::saturation_record`].
    Saturated,
}

/// A fixed-capacity, append-only sink over one worker's trace stream.
pub struct TraceSink {
    records: [WireTraceRecord; MAX_TRACE_DEPTH],
    /// Declared capacity, including the terminal reservation.
    capacity: usize,
    len: usize,
    /// Ordinary records refused because the sink was full. Reported once, so
    /// loss is a counted fact rather than an absence.
    dropped: u32,
}

/// A zeroed record, used only to fill the unwritten tail of the array. It is
/// never observable: `records()` slices to `len`.
const BLANK: WireTraceRecord = WireTraceRecord {
    magic: 0,
    version: 0,
    kind: 0,
    flags: 0,
    route_identity: 0,
    correlation: 0,
    sequence: 0,
    now_ns: 0,
    status: 0,
    event: 0,
    high_water: 0,
    order_class: 0,
    reserved: [0; 3],
};

impl TraceSink {
    /// Build a sink at the generation's declared capacity.
    ///
    /// `capacity` comes from the resolved profile's `FABRIC_TRACE_DEPTH`, never
    /// from a peer: the depth is a generation fact, and a sink whose bound was
    /// chosen at runtime would not be comparable across boots.
    pub fn new(capacity: usize) -> Result<Self, TraceError> {
        if capacity > MAX_TRACE_DEPTH || capacity <= TERMINAL_RESERVE {
            return Err(TraceError::BadCapacity);
        }
        Ok(Self {
            records: [BLANK; MAX_TRACE_DEPTH],
            capacity,
            len: 0,
            dropped: 0,
        })
    }

    /// Build a sink at a capacity fixed by a constant.
    ///
    /// Separate from [`TraceSink::new`] so a caller whose depth is a generation
    /// constant can build one in a `const fn`, which is what lets a worker hold
    /// its trace in a `const`-constructed broker.
    ///
    /// The bad-capacity case panics rather than returning, because a `Result`
    /// here would have to be unwrapped by every `const fn` caller. Note this is
    /// *not* by itself a build-time guarantee: a `const fn` invoked from a
    /// runtime context evaluates at runtime, and every worker reaches this from
    /// `fn main`. Each worker therefore carries its own `const _: ()` assert on
    /// the declared depth, which is what actually fails the build.
    pub const fn with_const_capacity(capacity: usize) -> Self {
        assert!(
            capacity <= MAX_TRACE_DEPTH && capacity > TERMINAL_RESERVE,
            "declared trace depth is outside the contract"
        );
        Self {
            records: [BLANK; MAX_TRACE_DEPTH],
            capacity,
            len: 0,
            dropped: 0,
        }
    }

    /// The records recorded so far, in emission order.
    pub fn records(&self) -> &[WireTraceRecord] {
        &self.records[..self.len]
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// How many ordinary records the sink refused.
    pub fn dropped(&self) -> u32 {
        self.dropped
    }

    /// The declared capacity this sink was built at, including the reservation.
    ///
    /// Exposed so a worker can report it and a gate can assert that the depth
    /// the *generation* declared is the depth the running sink actually holds.
    /// Without that, a plane whose records comfortably fit would pass no matter
    /// what the generation said, and the declared bound would be unenforced in
    /// exactly the case it is supposed to govern.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Slots an ordinary record may occupy: everything but the reservation.
    fn ordinary_capacity(&self) -> usize {
        self.capacity - TERMINAL_RESERVE
    }

    /// Record one event, in declared order.
    ///
    /// A record carrying `FLAG_TERMINAL` may use the reservation; anything else
    /// stops at `ordinary_capacity`.
    ///
    /// `floor_ns` is the sink's clock: the instant the emitting worker has
    /// reached. A record dated before it is refused, because the clock is
    /// monotone by contract and a retired instant cannot acquire new evidence.
    /// Within the live instant the record is *placed*, not appended — see the
    /// module header for why that is the determinism property rather than a
    /// convenience.
    pub fn push(&mut self, record: WireTraceRecord, floor_ns: u64) -> Result<(), TraceError> {
        if !valid_trace_record(&record) {
            return Err(TraceError::Malformed);
        }
        if record.now_ns < floor_ns {
            return Err(TraceError::OutOfOrder);
        }
        let terminal = record.flags & FLAG_TERMINAL != 0;
        let limit = if terminal {
            self.capacity
        } else {
            self.ordinary_capacity()
        };
        if self.len >= limit {
            // Saturated. Which record is dropped has to be decided by the
            // declared order, not by arrival: refusing whatever came last would
            // make the retained set "the first N the worker happened to observe",
            // and two boots whose sweeps differ would then keep different subsets
            // while reporting the same `dropped` count. Membership is what the
            // byte comparison reads, so the sink keeps the N records that sort
            // first and drops the one that sorts last.
            //
            // A terminal record that cannot fit is the one case with nothing left
            // to displace: the reservation itself is exhausted, so it is refused
            // without touching the ordinary drop counter.
            if terminal {
                return Err(TraceError::Saturated);
            }
            // Exactly one record is lost either way, so the drop is counted here
            // whether the loser is the arriving record or the one it displaces.
            self.dropped = self.dropped.saturating_add(1);
            // The last stored ordinary record is what this one must beat. `limit`
            // excludes the reservation, so a terminal already stored is never it.
            let last = limit - 1;
            if trace_records_ordered(&self.records[last], &record) {
                // The arriving record sorts at or after the incumbent, so the
                // order says the arriving record is the one to drop.
                return Err(TraceError::Saturated);
            }
            // It sorts earlier. Drop the incumbent by shortening the sink; the
            // insertion below then places the arriving record in its own
            // position, which may be anywhere at or before `last`.
            self.len = last;
        }
        // Insertion point: after every record the declared order puts first, so
        // equal keys keep their arrival order and the sort is stable.
        let mut index = self.len;
        while index > 0 && !trace_records_ordered(&self.records[index - 1], &record) {
            self.records[index] = self.records[index - 1];
            index -= 1;
        }
        self.records[index] = record;
        self.len += 1;
        Ok(())
    }

    /// The single record reporting how many ordinary records were refused.
    ///
    /// `None` when nothing was dropped, so a clean run emits no saturation
    /// evidence at all and a reader can tell the two cases apart. The record
    /// is built rather than stored because it must be emitted *after* the last
    /// refusal, and a sink cannot know which refusal was the last.
    pub fn saturation_record(&self, now_ns: u64) -> Option<WireTraceRecord> {
        if self.dropped == 0 {
            return None;
        }
        Some(WireTraceRecord {
            magic: fabric_trace::TRACE_MAGIC,
            version: fabric_trace::FORMAT_VERSION,
            kind: fabric_trace::KIND_RESOURCE,
            flags: FLAG_DROPPED,
            route_identity: 0,
            correlation: 0,
            sequence: 0,
            now_ns,
            status: 0,
            // A resource record's `event` names which count it carries. This
            // one is the sink reporting on itself.
            event: fabric_trace::RESOURCE_SINK_DROPPED,
            high_water: self.dropped,
            order_class: ORDER_TIME as u8,
            reserved: [0; 3],
        })
    }
}
