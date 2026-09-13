//! Typed recording and deterministic replay over C8.11's trace record (C9.5).
//!
//! In `slime-proto` rather than in `components/lib` for `trace_sink`'s reason:
//! this is a bounded state machine with host-testable invariants, and B23's rule
//! is that such a mechanism lives where `cargo test` can reach it rather than
//! only in a booted image.
//!
//! # What this owns, and what it deliberately does not
//!
//! A recorder captures the three nondeterminism sources a deterministic
//! component can be replayed from — clock reads, timer expiries, and lifecycle
//! transitions — as `fabric-trace/v1` records of C9.5's four kinds, then
//! serializes them into a caller-supplied buffer. A replayer validates that
//! stream *whole* before consuming any of it, and answers each recorded input in
//! order instead of asking the live source.
//!
//! It owns no clock, no timer, and no transport. [`Recorder::clock_read`] takes
//! the value the caller already obtained, and [`Replay::clock_read`] returns the
//! value the recording holds; neither performs IPC. That split is what makes the
//! two paths comparable: a recorder that read the clock itself and a replayer
//! that did not would differ in their *own* code, so a divergence could not be
//! attributed to the recording.
//!
//! # Why the stream is validated whole, before any of it is replayed
//!
//! C9.5 requires that "a truncated or reordered trace is refused rather than
//! partially replayed", and partial replay is the failure mode that makes a
//! deterministic claim worthless: a replayer that consumed nine of ten records
//! and then hit a malformed tenth has already produced nine outputs nobody can
//! compare against anything. [`Replay::open`] therefore walks every record —
//! checking magic, version, per-kind field rules, the declared total order, and
//! the terminal marker — and answers `Err` without exposing a single input if any
//! check fails. A replayer that opens successfully is replaying a complete
//! recording or nothing.
//!
//! # Where the byte bound comes from
//!
//! The declared `record_capacity` from `contracts/recording-policy/v1`, read
//! through the self-scoped `RECORDING_SOURCES` operation *before* the stream is
//! mapped, so the bound is authenticated rather than inferred from the bytes.
//! Both tables here are fixed arrays sized by [`MAX_RECORD_CAPACITY`], so neither
//! end allocates.

#![allow(dead_code)]

use crate::fabric_trace::{
    self, FLAG_TERMINAL, MAX_TRACE_DEPTH, ORDER_DATA, ORDER_TIME, TRACE_RECORD_LEN, WireTraceRecord,
};
use crate::{trace_records_ordered, valid_trace_record};

/// Records one recording stream may hold.
///
/// `contracts/recording-policy/v1`'s `maxRecordCapacity`. Expressed here as
/// `MAX_TRACE_DEPTH` rather than as its own literal because the two are the same
/// number for a structural reason rather than by coincidence: a recorder
/// accumulates into this array and a C8.11 sink accumulates into one of that
/// depth, so a recording longer than a sink could hold would declare a stream no
/// recorder could have produced. `boot-contracts` asserts the contract's own
/// constant equals this, so a divergence is a build failure.
pub const MAX_RECORD_CAPACITY: usize = MAX_TRACE_DEPTH;

/// One recorded record, in bytes — the trace record's own length.
pub const RECORD_BYTES: usize = TRACE_RECORD_LEN;

/// The largest stream this format admits, in bytes.
pub const MAX_STREAM_BYTES: usize = MAX_RECORD_CAPACITY * RECORD_BYTES;

/// Why a recording or replay step was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordingError {
    /// The declared capacity is spent. Bounded rather than growing: the
    /// generation fixed the stream's length.
    Full,
    /// A record this emitter built is not a well-formed trace record. An
    /// emitter defect rather than a bounded condition.
    Malformed,
    /// A record would sort before one already recorded.
    OutOfOrder,
    /// The stream's byte length is not a whole number of records, or exceeds the
    /// declared capacity.
    BadLength,
    /// The stream carries no terminal record, so a complete recording and a
    /// truncated one would be indistinguishable.
    Truncated,
    /// The stream's records are not in the contract's declared total order.
    Reordered,
    /// A replay asked for an input kind the next record does not carry, or the
    /// recording is spent.
    Exhausted,
    /// The recording is complete: its terminal was recorded, so nothing may
    /// follow. A stream with records past its terminal is one that was appended
    /// to after being declared complete.
    Closed,
}

/// One component's bounded recording, accumulated in memory and serialized once.
///
/// Recording performs no IPC, on `fabric_trace_log::Trace`'s rule and for its
/// reason: a `debug_write` or a buffer write is a root round trip, and putting
/// one on the path of the work being recorded would make the recording describe
/// a different execution than the one that ran.
pub struct Recorder {
    records: [WireTraceRecord; MAX_RECORD_CAPACITY],
    len: usize,
    capacity: usize,
    now_ns: u64,
    sequence: u64,
    refused: u32,
    /// Whether the terminal has been recorded.
    ///
    /// A recorder is single-use: `terminal()` declares the recording complete,
    /// and a record after it would serialize a stream this module's own
    /// `Replay::open` refuses for having records past its terminal. Without
    /// this flag a caller could append and only discover the defect on the
    /// consuming side, one process away from the bug (found by review).
    closed: bool,
}

impl Recorder {
    /// Build a recorder bounded by the generation's declared capacity.
    ///
    /// A capacity above the contract ceiling is clamped to it rather than
    /// panicking, because the ceiling is already enforced twice before this: the
    /// decoder refuses an over-declared resource, and admission refuses the
    /// generation. Reaching here with a larger number would mean the root served
    /// a capacity no admitted resource could hold, and clamping keeps a
    /// component from indexing past its own fixed array on the strength of a
    /// number it did not choose.
    pub const fn new(capacity: usize) -> Self {
        let capacity = if capacity > MAX_RECORD_CAPACITY {
            MAX_RECORD_CAPACITY
        } else {
            capacity
        };
        Self {
            records: [BLANK; MAX_RECORD_CAPACITY],
            len: 0,
            capacity,
            now_ns: 0,
            sequence: 0,
            refused: 0,
            closed: false,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Records this recorder refused for want of capacity.
    ///
    /// Reported rather than silent, on the trace sink's rule: a recording that
    /// dropped inputs is not a complete recording, and a replayer must be able
    /// to tell that from a short run.
    pub const fn refused(&self) -> u32 {
        self.refused
    }

    /// Advance the recorder's clock, so records that follow carry the new
    /// instant.
    ///
    /// A retreat is refused before anything changes: the declared order would
    /// refuse the next record anyway, and failing here names the actual defect.
    pub fn advance(&mut self, now_ns: u64) -> Result<(), RecordingError> {
        if self.closed {
            return Err(RecordingError::Closed);
        }
        if now_ns < self.now_ns {
            return Err(RecordingError::OutOfOrder);
        }
        if now_ns != self.now_ns {
            self.now_ns = now_ns;
            self.sequence = 0;
        }
        Ok(())
    }

    /// Record one clock read: which clock answered, and what it answered.
    ///
    /// The value goes in `correlation` because the *answer* is the
    /// nondeterminism being captured. Replaying a clock read means returning the
    /// recorded value rather than asking the root again.
    pub fn clock_read(&mut self, source: u32, value: u64) -> Result<(), RecordingError> {
        self.push(WireTraceRecord {
            correlation: value,
            event: source,
            ..self.blank(fabric_trace::KIND_CLOCK_READ, ORDER_DATA)
        })
    }

    /// Record one timer expiry, naming the timer the root assigned.
    pub fn timer_expiry(&mut self, timer: u64) -> Result<(), RecordingError> {
        self.push(WireTraceRecord {
            correlation: timer,
            ..self.blank(fabric_trace::KIND_TIMER_EXPIRY, ORDER_DATA)
        })
    }

    /// Record one admitted lifecycle transition, naming the state reached.
    pub fn lifecycle(&mut self, state_id: u32) -> Result<(), RecordingError> {
        self.push(WireTraceRecord {
            event: state_id,
            ..self.blank(fabric_trace::KIND_LIFECYCLE, ORDER_DATA)
        })
    }

    /// Record one typed output: which channel, and what value.
    ///
    /// This is the family two boots are compared on, and a recorder emits it too
    /// so the recording carries the outputs the *recorded* run produced. A replay
    /// that reproduces them is then checkable against the recording itself rather
    /// than only against a second replay.
    pub fn output(&mut self, channel: u32, value: u64) -> Result<(), RecordingError> {
        self.push(WireTraceRecord {
            correlation: value,
            event: channel,
            ..self.blank(fabric_trace::KIND_OUTPUT, ORDER_DATA)
        })
    }

    /// Record the terminal marker that says this recording is complete.
    ///
    /// Required: a stream without it is refused by [`Replay::open`], because a
    /// complete recording and one cut short by a reset must not read alike. It
    /// spends an ordinary slot rather than a reservation — unlike C8.11's sink,
    /// this stream has one writer and one length, so a recorder that filled its
    /// declared capacity with inputs and could not terminate has over-declared
    /// its inputs rather than under-declared its reserve.
    pub fn terminal(&mut self) -> Result<(), RecordingError> {
        self.push(WireTraceRecord {
            flags: FLAG_TERMINAL,
            event: fabric_trace::RESOURCE_COMPLETE,
            ..self.blank(fabric_trace::KIND_RESOURCE, ORDER_TIME)
        })?;
        // Only on success: a refused terminal leaves the recorder usable, because
        // the caller may have run out of capacity and needs to report that rather
        // than being locked out of its own recorder.
        self.closed = true;
        Ok(())
    }

    /// Whether the terminal has been recorded, so the stream is complete.
    pub const fn is_closed(&self) -> bool {
        self.closed
    }

    /// Serialize the recording into `out`, returning the byte count written.
    ///
    /// `out` must hold `len() * RECORD_BYTES`. The caller supplies it — normally
    /// a mapped shared buffer — so this module never creates or maps one: the
    /// buffer's authority is the caller's capability, and a serializer that
    /// allocated its own destination would need one.
    pub fn serialize(&self, out: &mut [u8]) -> Result<usize, RecordingError> {
        let needed = self.len * RECORD_BYTES;
        if out.len() < needed {
            return Err(RecordingError::BadLength);
        }
        for (index, record) in self.records[..self.len].iter().enumerate() {
            let start = index * RECORD_BYTES;
            out[start..start + RECORD_BYTES].copy_from_slice(&record.encode());
        }
        Ok(needed)
    }

    fn blank(&self, kind: u32, order_class: u32) -> WireTraceRecord {
        WireTraceRecord {
            magic: fabric_trace::TRACE_MAGIC,
            version: fabric_trace::FORMAT_VERSION,
            kind,
            flags: 0,
            route_identity: 0,
            correlation: 0,
            sequence: self.sequence,
            now_ns: self.now_ns,
            status: 0,
            event: 0,
            high_water: 0,
            order_class: order_class as u8,
            reserved: [0; 3],
        }
    }

    fn push(&mut self, record: WireTraceRecord) -> Result<(), RecordingError> {
        // A terminal closes the recording, so nothing may follow it. Refused here
        // rather than in each recording method, because the guarantee is about
        // the *stream* and every method reaches it through this one.
        if self.closed {
            return Err(RecordingError::Closed);
        }
        if !valid_trace_record(&record) {
            return Err(RecordingError::Malformed);
        }
        if let Some(last) = self.records[..self.len].last()
            && !trace_records_ordered(last, &record)
        {
            return Err(RecordingError::OutOfOrder);
        }
        if self.len == self.capacity {
            self.refused = self.refused.saturating_add(1);
            return Err(RecordingError::Full);
        }
        self.records[self.len] = record;
        self.len += 1;
        self.sequence += 1;
        Ok(())
    }
}

/// A validated recording, replayed in order.
///
/// Holding a `Replay` is itself the evidence that the stream was complete and
/// in order: [`Replay::open`] is the only constructor and it refuses anything
/// else, so no consumer has to re-check what it reads.
#[derive(Debug)]
pub struct Replay {
    records: [WireTraceRecord; MAX_RECORD_CAPACITY],
    len: usize,
    cursor: usize,
}

impl Replay {
    /// Validate `bytes` whole and take ownership of the replay position.
    ///
    /// `capacity` is the generation's declared record capacity, read through
    /// `RECORDING_SOURCES` *before* the stream is mapped, so the bound is
    /// authenticated rather than inferred from the bytes themselves. A stream
    /// that is not a whole number of records, is longer than the declared
    /// capacity, contains a malformed record, is out of order, or lacks a
    /// terminal marker is refused here — with nothing exposed, which is what
    /// makes "refused rather than partially replayed" true rather than intended.
    pub fn open(bytes: &[u8], capacity: usize) -> Result<Self, RecordingError> {
        if capacity > MAX_RECORD_CAPACITY {
            return Err(RecordingError::BadLength);
        }
        if !bytes.len().is_multiple_of(RECORD_BYTES) {
            return Err(RecordingError::BadLength);
        }
        let len = bytes.len() / RECORD_BYTES;
        if len == 0 || len > capacity {
            return Err(RecordingError::BadLength);
        }
        let mut records = [BLANK; MAX_RECORD_CAPACITY];
        for index in 0..len {
            let start = index * RECORD_BYTES;
            let record = WireTraceRecord::decode(&bytes[start..start + RECORD_BYTES])
                .ok_or(RecordingError::BadLength)?;
            if !valid_trace_record(&record) {
                return Err(RecordingError::Malformed);
            }
            if index > 0 && !trace_records_ordered(&records[index - 1], &record) {
                return Err(RecordingError::Reordered);
            }
            records[index] = record;
        }
        // The terminal must be the *last* record and must be the canonical one,
        // both checked before a single input is exposed.
        //
        // "Canonical" is load-bearing rather than pedantic. `valid_trace_record`
        // permits `FLAG_TERMINAL` on any `KIND_RESOURCE` counter, so a stream
        // ending in a terminal-flagged *frames* record would have opened here and
        // handed out every preceding input before `finish` noticed (found by
        // review). Pinning the exact kind, event, and order class makes the
        // record that closes a recording one shape rather than a family.
        let terminal = records[len - 1];
        if terminal.flags & FLAG_TERMINAL == 0 {
            return Err(RecordingError::Truncated);
        }
        if terminal.kind != fabric_trace::KIND_RESOURCE
            || terminal.event != fabric_trace::RESOURCE_COMPLETE
            || u32::from(terminal.order_class) != ORDER_TIME
        {
            return Err(RecordingError::Malformed);
        }
        // A stream with records after its terminal was appended to after being
        // declared complete, and replaying the tail would replay inputs the
        // recorder never claimed to have finished capturing.
        if records[..len - 1]
            .iter()
            .any(|record| record.flags & FLAG_TERMINAL != 0)
        {
            return Err(RecordingError::Malformed);
        }
        Ok(Self {
            records,
            len,
            cursor: 0,
        })
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Records not yet replayed, terminal included.
    pub const fn remaining(&self) -> usize {
        self.len - self.cursor
    }

    /// The next record's kind, or `None` at the end of the stream.
    pub fn peek_kind(&self) -> Option<u32> {
        (self.cursor < self.len).then(|| self.records[self.cursor].kind)
    }

    /// Answer the next recorded clock read for `source`.
    ///
    /// Refused when the next record is not a clock read of that source: a replay
    /// that skipped ahead to find a matching record would be reordering the
    /// inputs, which is the divergence the ordering rules exist to prevent.
    ///
    /// The source is checked *before* the cursor moves. Consuming the record and
    /// then refusing it would leave the replay one record further along after a
    /// failed step, so a caller that recovered would continue from a shifted
    /// stream — a refused step must leave the position untouched (found by
    /// review).
    pub fn clock_read(&mut self, source: u32) -> Result<u64, RecordingError> {
        if self.peek(fabric_trace::KIND_CLOCK_READ)?.event != source {
            return Err(RecordingError::Exhausted);
        }
        Ok(self.take(fabric_trace::KIND_CLOCK_READ)?.correlation)
    }

    /// Answer the next recorded timer expiry, returning the timer it named.
    pub fn timer_expiry(&mut self) -> Result<u64, RecordingError> {
        Ok(self.take(fabric_trace::KIND_TIMER_EXPIRY)?.correlation)
    }

    /// Answer the next recorded lifecycle transition, returning its state id.
    pub fn lifecycle(&mut self) -> Result<u32, RecordingError> {
        Ok(self.take(fabric_trace::KIND_LIFECYCLE)?.event)
    }

    /// The next recorded output, as `(channel, value)`.
    ///
    /// A replayer reads these to compare its own outputs against the recorded
    /// run's, field by field.
    pub fn output(&mut self) -> Result<(u32, u64), RecordingError> {
        let record = self.take(fabric_trace::KIND_OUTPUT)?;
        Ok((record.event, record.correlation))
    }

    /// Consume the terminal record, proving the whole stream was replayed.
    ///
    /// Refused when records remain: a replay that stopped early reproduced a
    /// prefix, and a prefix of a deterministic run is not the run.
    pub fn finish(&mut self) -> Result<(), RecordingError> {
        if self.remaining() != 1 {
            return Err(RecordingError::Exhausted);
        }
        let record = self.take(fabric_trace::KIND_RESOURCE)?;
        if record.flags & FLAG_TERMINAL == 0 {
            return Err(RecordingError::Truncated);
        }
        Ok(())
    }

    /// The next record if it is of `kind`, without moving the cursor.
    ///
    /// Separate from [`Self::take`] so a step that must inspect a field before
    /// committing can refuse without advancing.
    fn peek(&self, kind: u32) -> Result<WireTraceRecord, RecordingError> {
        if self.cursor >= self.len {
            return Err(RecordingError::Exhausted);
        }
        let record = self.records[self.cursor];
        if record.kind != kind {
            return Err(RecordingError::Exhausted);
        }
        Ok(record)
    }

    fn take(&mut self, kind: u32) -> Result<WireTraceRecord, RecordingError> {
        if self.cursor >= self.len {
            return Err(RecordingError::Exhausted);
        }
        let record = self.records[self.cursor];
        if record.kind != kind {
            return Err(RecordingError::Exhausted);
        }
        self.cursor += 1;
        Ok(record)
    }
}

/// A zeroed record, filling the unwritten tail of either fixed array. Never
/// observable: both types slice to their own `len`.
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
