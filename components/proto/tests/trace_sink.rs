//! The bounded trace sink's capacity, ordering, and loss-reporting invariants
//! (C8.11).
//!
//! Every property here is one the gate depends on: a sink that grew, silently
//! dropped, or reordered would still produce a plausible-looking transcript, so
//! these are the failures that would otherwise pass unnoticed.

use slime_proto::fabric_trace::{
    self, FLAG_DROPPED, FLAG_TERMINAL, KIND_CALL, KIND_QOS, KIND_RESOURCE, MAX_TRACE_DEPTH,
    ORDER_DATA, ORDER_TIME, TERMINAL_RESERVE, TRACE_MAGIC, WireTraceRecord,
};
use slime_proto::trace_sink::{TraceError, TraceSink};
use slime_proto::valid_trace_record;

const ROUTE: u64 = 0x1164_1539_08db_137b;
const DEPTH: usize = 8;

fn call(sequence: u64, now_ns: u64) -> WireTraceRecord {
    WireTraceRecord {
        magic: TRACE_MAGIC,
        version: fabric_trace::FORMAT_VERSION,
        kind: KIND_CALL,
        flags: 0,
        route_identity: ROUTE,
        correlation: sequence + 1,
        sequence,
        now_ns,
        status: 0,
        event: 0,
        high_water: 0,
        order_class: ORDER_DATA as u8,
        reserved: [0; 3],
    }
}

fn terminal(now_ns: u64) -> WireTraceRecord {
    WireTraceRecord {
        kind: KIND_QOS,
        flags: FLAG_TERMINAL,
        route_identity: 0,
        correlation: 0,
        sequence: 0,
        order_class: ORDER_TIME as u8,
        ..call(0, now_ns)
    }
}

#[test]
fn a_capacity_outside_the_contract_is_refused() {
    assert_eq!(
        TraceSink::new(MAX_TRACE_DEPTH + 1).err(),
        Some(TraceError::BadCapacity)
    );
    // A sink whose whole depth is the terminal reservation could never record
    // an ordinary event, so it is not a usable sink.
    assert_eq!(
        TraceSink::new(TERMINAL_RESERVE).err(),
        Some(TraceError::BadCapacity)
    );
    assert!(TraceSink::new(TERMINAL_RESERVE + 1).is_ok());
    assert!(TraceSink::new(MAX_TRACE_DEPTH).is_ok());
}

#[test]
fn a_fresh_sink_is_empty_and_reports_no_loss() {
    let sink = TraceSink::new(DEPTH).expect("capacity");
    assert!(sink.is_empty());
    assert_eq!(sink.dropped(), 0);
    assert!(sink.records().is_empty());
    // A clean run must emit no saturation evidence at all, so a reader can tell
    // "nothing was dropped" from "the count happened to be zero".
    assert!(sink.saturation_record(10).is_none());
}

#[test]
fn records_are_kept_in_emission_order() {
    let mut sink = TraceSink::new(DEPTH).expect("capacity");
    for index in 0..3u64 {
        sink.push(call(index, 100 + index), 0).expect("push");
    }
    assert_eq!(sink.len(), 3);
    let sequences = sink.records().iter().map(|record| record.sequence);
    assert!(sequences.eq([0, 1, 2]));
}

#[test]
fn a_malformed_record_never_enters_the_sink() {
    let mut sink = TraceSink::new(DEPTH).expect("capacity");
    let mut bad = call(0, 100);
    bad.magic ^= 1;
    assert!(!valid_trace_record(&bad));
    assert_eq!(sink.push(bad, 0).err(), Some(TraceError::Malformed));
    assert!(sink.is_empty());
    // A refused malformed record is not backpressure, so it must not be counted
    // as dropped evidence either.
    assert_eq!(sink.dropped(), 0);
}

#[test]
fn a_record_dated_before_the_clock_is_refused() {
    let mut sink = TraceSink::new(DEPTH).expect("capacity");
    sink.push(call(1, 200), 0).expect("push");
    // The clock has reached 200, so 100 is a retired instant. It cannot acquire
    // new evidence: the clock is monotone by contract.
    assert_eq!(
        sink.push(call(2, 100), 200).err(),
        Some(TraceError::OutOfOrder)
    );
    assert_eq!(sink.len(), 1);
}

#[test]
fn records_observed_out_of_class_order_are_placed_not_refused() {
    // A broker genuinely observes an acknowledgement before a data record at one
    // instant: its sweep drains client endpoints before server replies. The tie
    // order says how those records are *arranged*, not which the worker may see
    // first, so the sink sorts them rather than discarding real evidence.
    let mut sink = TraceSink::new(DEPTH).expect("capacity");
    let ack = WireTraceRecord {
        order_class: fabric_trace::ORDER_ACK as u8,
        ..call(1, 300)
    };
    let data = call(2, 300);
    sink.push(ack, 300).expect("ack observed first");
    sink.push(data, 300).expect("data observed second");
    assert_eq!(sink.len(), 2);
    // Declared order, not arrival order: data sorts before ack at one instant.
    let classes = sink
        .records()
        .iter()
        .map(|record| u32::from(record.order_class));
    assert!(classes.eq([ORDER_DATA, fabric_trace::ORDER_ACK]));
}

#[test]
fn equal_keys_keep_their_arrival_order() {
    // Two records with the same key are not an ordering defect, and the sort is
    // stable so their relative order is the order the worker emitted them.
    let mut sink = TraceSink::new(DEPTH).expect("capacity");
    let mut first = call(4, 300);
    first.correlation = 11;
    let mut second = call(4, 300);
    second.correlation = 22;
    sink.push(first, 300).expect("first");
    sink.push(second, 300).expect("second");
    assert_eq!(sink.len(), 2);
    assert_eq!(sink.records()[0].correlation, 11);
    assert_eq!(sink.records()[1].correlation, 22);
}

#[test]
fn ordinary_records_stop_at_the_reservation_and_are_counted() {
    let mut sink = TraceSink::new(DEPTH).expect("capacity");
    let ordinary = DEPTH - TERMINAL_RESERVE;
    for index in 0..ordinary as u64 {
        sink.push(call(index, 100), 0)
            .expect("within the ordinary bound");
    }
    assert_eq!(sink.len(), ordinary);
    // The reservation is not available to ordinary evidence, even though the
    // sink has not reached its declared depth.
    assert_eq!(
        sink.push(call(ordinary as u64, 100), 0).err(),
        Some(TraceError::Saturated)
    );
    assert_eq!(sink.len(), ordinary);
    assert_eq!(sink.dropped(), 1);
    // And the sink still does not grow under continued pressure.
    for _ in 0..5 {
        assert_eq!(
            sink.push(call(ordinary as u64, 100), 0).err(),
            Some(TraceError::Saturated)
        );
    }
    assert_eq!(sink.len(), ordinary);
    assert_eq!(sink.dropped(), 6);
}

#[test]
fn a_saturated_sink_still_records_its_terminal_evidence() {
    let mut sink = TraceSink::new(DEPTH).expect("capacity");
    for index in 0..(DEPTH - TERMINAL_RESERVE) as u64 {
        sink.push(call(index, 100), 0).expect("fill");
    }
    assert_eq!(
        sink.push(call(99, 100), 0).err(),
        Some(TraceError::Saturated)
    );
    // This is the property the reservation exists for: a full sink can still
    // say the trace ended, so a reader distinguishes completion from truncation.
    for _ in 0..TERMINAL_RESERVE {
        sink.push(terminal(100), 0)
            .expect("terminal fits the reservation");
    }
    assert_eq!(sink.len(), DEPTH);
    // Past the reservation even a terminal record is refused, and it is not
    // counted as ordinary loss.
    let before = sink.dropped();
    assert_eq!(
        sink.push(terminal(100), 0).err(),
        Some(TraceError::Saturated)
    );
    assert_eq!(sink.dropped(), before);
}

#[test]
fn the_saturation_report_is_a_valid_countable_record() {
    let mut sink = TraceSink::new(DEPTH).expect("capacity");
    for index in 0..(DEPTH - TERMINAL_RESERVE) as u64 {
        sink.push(call(index, 100), 0).expect("fill");
    }
    for _ in 0..3 {
        assert!(sink.push(call(99, 100), 0).is_err());
    }
    let report = sink.saturation_record(400).expect("loss must be reported");
    assert!(valid_trace_record(&report), "the report must be readable");
    assert_eq!(report.kind, KIND_RESOURCE);
    assert_eq!(report.flags, FLAG_DROPPED);
    assert_eq!(report.high_water, 3);
    assert_eq!(report.now_ns, 400);
    // It closes an instant rather than occurring within one.
    assert_eq!(u32::from(report.order_class), ORDER_TIME);
}

#[test]
fn saturation_keeps_the_records_that_sort_first() {
    // Membership under saturation must be decided by the declared order, not by
    // arrival: two boots whose sweeps differ would otherwise retain different
    // subsets while reporting the same drop count, which is exactly the
    // scheduler dependence the artifact must not have.
    let mut sink = TraceSink::new(DEPTH).expect("capacity");
    let ordinary = DEPTH - TERMINAL_RESERVE;
    // Fill with a late instant.
    for index in 0..ordinary as u64 {
        sink.push(call(index, 900), 0).expect("fill");
    }
    // A record from an earlier instant arrives after the sink is full. It sorts
    // before everything stored, so it belongs in the trace and the last-sorting
    // record is the one that goes.
    // Stored, displacing the incumbent: `Ok` because this record is in the
    // trace, while `dropped` records that one record was lost.
    let early = call(77, 100);
    sink.push(early, 0)
        .expect("an earlier record belongs in the trace");
    assert_eq!(sink.len(), ordinary, "the sink must not grow");
    assert_eq!(sink.dropped(), 1, "the eviction is counted");
    assert_eq!(sink.records()[0].now_ns, 100, "the earlier record was kept");
    assert_eq!(sink.records()[0].sequence, 77);

    // A record that sorts *after* everything stored is the one the order drops,
    // and the retained set is untouched.
    let late = call(88, 1_000);
    assert_eq!(sink.push(late, 0).err(), Some(TraceError::Saturated));
    assert_eq!(sink.len(), ordinary);
    assert_eq!(sink.dropped(), 2);
    assert!(
        sink.records().iter().all(|record| record.sequence != 88),
        "the last-sorting record must not be stored"
    );
}
