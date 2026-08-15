//! Emitting one worker's bounded semantic trace to serial (C8.11).
//!
//! `slime_proto::trace_sink` owns the accumulation rules; this owns the *shape
//! of the line* a gate reads. The two are separate because the sink's
//! invariants are host-testable and the rendering is not: rendering exists only
//! to get bytes past `debug_write`, and its whole contract is that identical
//! records produce identical lines.
//!
//! # Why the trace is emitted at the end rather than as it happens
//!
//! A `debug_write` is a root round trip, so emitting inside a broker's sweep
//! would put the trace on the same critical path as the traffic it describes —
//! and interleave records from concurrent workers in whatever order the
//! scheduler happened to run them. Accumulating into the bounded sink and
//! flushing once means the serial order is the *declared* order, not the
//! scheduling order, which is exactly the property C8.11 asks for: identical
//! inputs produce identical trace artifacts "independent of serial-log
//! interleaving".
//!
//! # Fields, and what is deliberately absent
//!
//! A line carries only what the contract bounds: family, order class,
//! simulated time, route identity, correlation, sequence, status, event, and a
//! high-water count. No task id, no slot number, no address, no component name
//! — a trace naming those would differ between two runs of the same graph, and
//! could not serve as the comparison artifact.
//!
//! # Why the capacity is a parameter
//!
//! [`Trace::new`] takes the sink depth rather than reading
//! `FABRIC_TRACE_DEPTH` itself. That constant exists only for a generation
//! that declares a fabric graph, so reading it here would make this module
//! — and therefore the whole crate — fail to compile for a fabric-less
//! manifest such as `sel4.zti`, over a constant that nothing in `console` or
//! `init` reads. Taking it as an argument keeps the module independent of any
//! one generation, and each worker supplies the depth its own graph declared.

#![allow(dead_code)]

use slime_proto::fabric_trace::{
    self, FLAG_DROPPED, FLAG_TERMINAL, ORDER_ACK, ORDER_DATA, ORDER_PEER_DEATH, ORDER_TIME,
    WireTraceRecord,
};
use slime_proto::trace_sink::{TraceError, TraceSink};

/// One worker's trace: the bounded sink plus the clock its records are stamped
/// with.
///
/// # Recording performs no IPC
///
/// [`Trace::advance`], [`Trace::edge`], [`Trace::peer_death`],
/// [`Trace::resource`], and [`Trace::terminal`] touch only in-memory state.
/// [`Trace::flush`] is the sole method that writes to serial, and it runs once,
/// after the worker's loop has ended.
///
/// That split is load-bearing rather than incidental. A `debug_write` is a root
/// round trip, and a broker replying to a client holds a reply capability that
/// "was stored when the thread was last called" -- which *any* intervening IPC
/// overwrites. The call broker therefore replies to a terminal acknowledgement
/// and only then records it. Moving an emission ahead of such a reply, or giving
/// a recording method serial output, would corrupt the reply capability rather
/// than merely reorder the trace.
pub struct Trace {
    sink: TraceSink,
    /// The worker's simulated clock. Records are stamped from here rather than
    /// from a parameter so a caller cannot accidentally date a record to an
    /// instant the worker has not reached.
    now_ns: u64,
    /// Per-instant emission counter, reset by each advance. It is what breaks
    /// ties within one class at one instant, so two records the worker emits in
    /// order stay ordered in the artifact.
    sequence: u64,
    /// Records the sink refused as malformed or out of order.
    ///
    /// Counted rather than discarded, and reported by `flush`. A caller that
    /// ignores the `Result` -- which every emission site does, because a trace
    /// defect must not take down a worker mid-traffic -- would otherwise make a
    /// miswired record indistinguishable from an event that never happened.
    /// That exact confusion hid a fault record whose status field was zero: the
    /// record was rejected, the transcript simply lacked a line, and nothing
    /// said so. A nonzero count here is a defect in the emitter, not
    /// backpressure, which is why it is reported separately from the sink's own
    /// saturation count.
    rejected: u32,
}

impl Trace {
    /// Build a trace at the depth this worker's generation declared.
    ///
    /// `const fn` so a broker built in a `const fn` can hold one.
    ///
    /// The depth bound is enforced by a `const _: ()` assert in each worker that
    /// includes this module, not here: `with_const_capacity`'s own assert is
    /// reached from `fn main` and so evaluates at runtime, which would make an
    /// over-declared depth a boot panic instead of a build failure.
    pub const fn new(capacity: usize) -> Self {
        Self {
            sink: TraceSink::with_const_capacity(capacity),
            now_ns: 0,
            sequence: 0,
            rejected: 0,
        }
    }

    pub fn now_ns(&self) -> u64 {
        self.now_ns
    }

    /// Advance the clock and record the advance that closes the old instant.
    ///
    /// The clock moves whether or not the record fits. A saturated sink is a
    /// bounded, reported condition, but the *clock* is not evidence — it is the
    /// worker's own notion of now, and the records that follow are stamped from
    /// it. Returning early on a refused push left the clock frozen at the
    /// instant the sink filled, so every later record claimed an instant the
    /// worker had already passed: an entire trace stamped `now=0`, in declared
    /// order only because nothing ever moved. The refusal is still counted, and
    /// still returned to the caller.
    ///
    /// A retreat is rejected before anything changes: the sink's order would
    /// refuse the record anyway, and failing here names the actual defect — a
    /// clock that went backwards — rather than an out-of-order trace record.
    pub fn advance(&mut self, now_ns: u64) -> Result<(), TraceError> {
        if now_ns < self.now_ns {
            return Err(TraceError::OutOfOrder);
        }
        let record = WireTraceRecord {
            order_class: ORDER_TIME as u8,
            ..self.blank(fabric_trace::KIND_QOS, ORDER_TIME)
        };
        // Routed through `push` so a refused advance is counted like any other
        // refused record, but the result is captured rather than propagated
        // early: the clock must move regardless.
        let result = self.push(record);
        self.now_ns = now_ns;
        // The sequence is scoped to one instant and the instant has changed, so
        // it resets whether or not the advance record itself was stored. Keeping
        // the old counter after a refused advance would number the new instant's
        // records from wherever the previous one stopped, which says nothing
        // about either.
        self.sequence = 0;
        result
    }

    /// Record one event on a declared edge.
    ///
    /// Errors are returned rather than fatal: a saturated sink is a bounded,
    /// reported condition, and a worker must keep serving traffic when its
    /// trace fills. Losing the count is what would be a defect, and the sink
    /// counts it.
    pub fn edge(
        &mut self,
        kind: u32,
        order_class: u32,
        route_identity: u64,
        correlation: u64,
        status: i32,
        event: u32,
    ) -> Result<(), TraceError> {
        let record = WireTraceRecord {
            route_identity,
            correlation,
            status,
            event,
            ..self.blank(kind, order_class)
        };
        self.push(record)
    }

    /// Record one peer death on a declared edge.
    ///
    /// A dedicated helper rather than an `edge` call, because the status a fault
    /// record carries must be a *failure* code and each plane's own
    /// `STATUS_PEER_DEAD` is a positive protocol enumerator (6 on the call
    /// plane, 5 on the operation plane). Passing one of those produced a record
    /// the validator rejected as malformed, silently, on both request/response
    /// workers. Deriving the status here means no caller chooses it, so the
    /// three planes cannot disagree about the sign of the same fact.
    pub fn peer_death(&mut self, route_identity: u64) -> Result<(), TraceError> {
        self.edge(
            fabric_trace::KIND_FAULT,
            ORDER_PEER_DEATH,
            route_identity,
            0,
            slime_rt::ERR_PEER_DEAD as i32,
            slime_proto::fabric_qos::EVENT_PEER_DEAD,
        )
    }

    /// Record one resource high-water count. Carries no edge: it is the
    /// worker's own accounting, not an event on a route.
    ///
    /// `counter` is a `RESOURCE_*` code naming *what* was counted. A bare
    /// number would not be evidence: a reader could not tell a frame count from
    /// an operation count, and two runs reporting different tables would
    /// compare as equal.
    pub fn resource(&mut self, counter: u32, high_water: u32) -> Result<(), TraceError> {
        let record = WireTraceRecord {
            high_water,
            event: counter,
            ..self.blank(fabric_trace::KIND_RESOURCE, ORDER_DATA)
        };
        self.push(record)
    }

    /// Record the terminal marker that says this worker's trace is complete.
    ///
    /// Always the `complete` counter: the terminal is the same fact for every
    /// worker, and letting each pass its own code would make the one record a
    /// reader must find in order to trust the trace vary by emitter.
    pub fn terminal(&mut self) -> Result<(), TraceError> {
        let record = WireTraceRecord {
            flags: FLAG_TERMINAL,
            event: fabric_trace::RESOURCE_COMPLETE,
            ..self.blank(fabric_trace::KIND_RESOURCE, ORDER_TIME)
        };
        self.push(record)
    }

    fn blank(&self, kind: u32, order_class: u32) -> WireTraceRecord {
        WireTraceRecord {
            magic: fabric_trace::TRACE_MAGIC,
            version: fabric_trace::FORMAT_VERSION,
            kind,
            flags: 0,
            route_identity: 0,
            correlation: 0,
            // Every record carries the live counter, including the time class.
            // Hard-coding zero there made two records closing one instant --
            // a clock advance and the terminal, say -- share a key, so their
            // arrangement fell back to arrival order and the gate's "terminal is
            // the last record" assertion held only by accident.
            sequence: self.sequence,
            now_ns: self.now_ns,
            status: 0,
            event: 0,
            high_water: 0,
            order_class: order_class as u8,
            reserved: [0; 3],
        }
    }

    fn push(&mut self, record: WireTraceRecord) -> Result<(), TraceError> {
        // The sink's floor is this worker's clock: a record dated before the
        // instant the worker has reached is a defect, while one bearing the live
        // instant is merely placed in its declared position.
        let result = self.sink.push(record, self.now_ns);
        // Unconditionally, because `sequence` is the sort's tie-break key rather
        // than a count of stored records. Incrementing only on success let a
        // refused record's ordinal be reused, so two records at one instant in
        // one class could share a key -- and equal keys fall back to arrival
        // order, which is the scheduler. A strictly monotone key keeps the
        // arrangement determined by the declared order even across a rejection;
        // the gap it leaves is itself evidence that a record was refused.
        self.sequence += 1;
        match result {
            Ok(()) => {}
            // Saturation is the sink's own bounded condition and it keeps that
            // count itself. Malformed and out-of-order are emitter defects, and
            // counting them here is what makes them visible at all: every call
            // site discards the `Result` so a trace defect cannot kill a worker
            // mid-traffic.
            Err(TraceError::Saturated) => {}
            Err(_) => self.rejected = self.rejected.saturating_add(1),
        }
        result
    }

    /// Write the whole trace to serial, in declared order, then the saturation
    /// report if any records were refused.
    ///
    /// The report comes last because it counts every refusal, and the sink
    /// cannot know which refusal was the final one until the flush.
    pub fn flush(&self, worker: &[u8]) {
        for record in self.sink.records() {
            write_record(worker, record);
        }
        if let Some(report) = self.sink.saturation_record(self.now_ns) {
            write_record(worker, &report);
        }
        // One write, for the same reason a record is: another task's serial
        // output must not be able to land in the middle of the line a gate parses.
        let mut line = Line::new();
        line.put(b"[trace] ");
        line.put(worker);
        // The declared capacity is reported alongside the count so a gate can
        // assert the generation's number reached the running sink. A plane whose
        // records fit comfortably would otherwise pass under any declared depth.
        line.put(b" complete capacity=");
        line.decimal(self.sink.capacity() as u64);
        line.put(b" records=");
        line.decimal(self.sink.len() as u64);
        line.put(b" dropped=");
        line.decimal(self.sink.dropped() as u64);
        line.put(b" rejected=");
        line.decimal(self.rejected as u64);
        line.emit();
    }
}

fn kind_name(kind: u32) -> &'static [u8] {
    match kind {
        fabric_trace::KIND_SCHEMA => b"schema",
        fabric_trace::KIND_ROUTE => b"route",
        fabric_trace::KIND_QOS => b"qos",
        fabric_trace::KIND_CALL => b"call",
        fabric_trace::KIND_OPERATION => b"operation",
        fabric_trace::KIND_VISIBILITY => b"visibility",
        fabric_trace::KIND_INTERPOSITION => b"interposition",
        fabric_trace::KIND_DENIAL => b"denial",
        fabric_trace::KIND_FAULT => b"fault",
        fabric_trace::KIND_RESOURCE => b"resource",
        _ => b"unknown",
    }
}

fn order_name(order_class: u8) -> &'static [u8] {
    match u32::from(order_class) {
        ORDER_DATA => b"data",
        ORDER_ACK => b"ack",
        ORDER_PEER_DEATH => b"peer-death",
        ORDER_TIME => b"time",
        _ => b"unknown",
    }
}

/// A line under construction, in a fixed buffer.
///
/// A whole line is assembled and emitted in **one** `debug_write`. Building it
/// field by field, as an earlier revision did, made the line racy: each
/// `debug_write` is a root round trip, so another task's serial output can land
/// between two of them — observed directly, a `SLIME_GRAPH supervision
/// collected` line spliced into the middle of an ack record, which the gate then
/// counted as one record fewer than the sink reported. C8.11 requires the trace
/// to be comparable *independent of serial-log interleaving*, and a line that
/// can be cut in half is not.
///
/// Capacity covers the worst case: the fixed field names, a 16-character route
/// identity, three 20-digit decimals, two 10-digit decimals, a signed 11-char
/// status, the longest worker and family names, and both flag words.
struct Line {
    bytes: [u8; 256],
    len: usize,
}

impl Line {
    const fn new() -> Self {
        Self {
            bytes: [0; 256],
            len: 0,
        }
    }

    /// Append, truncating rather than panicking.
    ///
    /// Truncation cannot happen for any record this contract admits — the buffer
    /// is sized for the worst case — but a rendering bug must not take down a
    /// worker mid-traffic, and a short line fails the gate's own record grammar
    /// loudly.
    fn put(&mut self, text: &[u8]) {
        let end = (self.len + text.len()).min(self.bytes.len());
        let take = end - self.len;
        self.bytes[self.len..end].copy_from_slice(&text[..take]);
        self.len = end;
    }

    fn decimal(&mut self, mut value: u64) {
        let mut digits = [0u8; 20];
        let mut index = digits.len();
        loop {
            index -= 1;
            digits[index] = b'0' + (value % 10) as u8;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        self.put(&digits[index..]);
    }

    fn signed(&mut self, value: i32) {
        if value < 0 {
            self.put(b"-");
            // Negate through `i64` so `i32::MIN` renders rather than overflowing.
            self.decimal((-(value as i64)) as u64);
        } else {
            self.decimal(value as u64);
        }
    }

    /// Route identity, fixed width. Fixed because a variable-length identity
    /// would make two otherwise identical traces differ in their column
    /// alignment, and the artifact is compared as bytes.
    fn hex16(&mut self, value: u64) {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = [0u8; 16];
        for (index, byte) in out.iter_mut().enumerate() {
            let shift = 60 - index * 4;
            *byte = HEX[((value >> shift) & 0xf) as usize];
        }
        self.put(&out);
    }

    fn emit(&mut self) {
        self.put(b"\n");
        slime_rt::debug_write(&self.bytes[..self.len]);
    }
}

/// One record, one line, one write. The field order is the record's own field
/// order, so a reader of the schema can read the line without a second table.
fn write_record(worker: &[u8], record: &WireTraceRecord) {
    let mut line = Line::new();
    line.put(b"[trace] ");
    line.put(worker);
    line.put(b" kind=");
    line.put(kind_name(record.kind));
    line.put(b" order=");
    line.put(order_name(record.order_class));
    line.put(b" now=");
    line.decimal(record.now_ns);
    line.put(b" route=");
    line.hex16(record.route_identity);
    line.put(b" correlation=");
    line.decimal(record.correlation);
    line.put(b" sequence=");
    line.decimal(record.sequence);
    line.put(b" status=");
    line.signed(record.status);
    line.put(b" event=");
    line.decimal(record.event as u64);
    line.put(b" high_water=");
    line.decimal(record.high_water as u64);
    if record.flags & FLAG_TERMINAL != 0 {
        line.put(b" terminal");
    }
    if record.flags & FLAG_DROPPED != 0 {
        line.put(b" dropped");
    }
    line.emit();
}

/// Project a 32-byte route identity onto the record's 8-byte `route_identity`.
///
/// The record carries eight bytes rather than thirty-two because it is a fixed
/// 64-byte slot and the full identity would consume half of it. The leading
/// eight bytes of a SHA-256 fold are what the rest of this repository already
/// uses as a short identity, and the property the trace needs is only that two
/// distinct admitted routes stay distinct in the artifact -- not that the
/// identity be reversible. A reader that needs the full identity reads the
/// graph, which is authenticated; the trace is evidence, not authority.
pub fn route_word(identity: &[u8; 32]) -> u64 {
    u64::from_le_bytes([
        identity[0],
        identity[1],
        identity[2],
        identity[3],
        identity[4],
        identity[5],
        identity[6],
        identity[7],
    ])
}
