//! The C8.11 semantic-trace record's structural and ordering invariants.
//!
//! This record is the deterministic evidence stream the repeated-boot
//! comparison reads, so the properties under test are the ones that make two
//! runs of identical inputs byte-comparable: a family's unused fields are zero
//! rather than incidental, the tie order is total, and a saturated sink still
//! reports the loss it refused. Every rejected pattern here is bytes a
//! miswired emitter can actually produce, not a hypothetical.

use slime_proto::fabric_trace::{
    self, FLAG_DROPPED, FLAG_TERMINAL, KIND_CALL, KIND_DENIAL, KIND_FAULT, KIND_INTERPOSITION,
    KIND_OPERATION, KIND_QOS, KIND_RESOURCE, KIND_ROUTE, KIND_SCHEMA, KIND_VISIBILITY, MAX_KIND,
    MAX_ORDER_CLASS, MAX_TRACE_DEPTH, ORDER_ACK, ORDER_DATA, ORDER_PEER_DEATH, ORDER_TIME,
    TERMINAL_RESERVE, TRACE_MAGIC, TRACE_RECORD_LEN, WireTraceRecord,
};
use slime_proto::{trace_records_ordered, valid_trace_record};

const ROUTE: u64 = 0x1164_1539_08db_137b;

/// A well-formed call record: an edge, a correlation, and a data class.
fn call() -> WireTraceRecord {
    WireTraceRecord {
        magic: TRACE_MAGIC,
        version: fabric_trace::FORMAT_VERSION,
        kind: KIND_CALL,
        flags: 0,
        route_identity: ROUTE,
        correlation: 9,
        sequence: 3,
        now_ns: 1_000_025,
        status: 0,
        event: 0,
        high_water: 2,
        order_class: ORDER_DATA as u8,
        reserved: [0; 3],
    }
}

/// The clock advance that closes one simulated instant.
fn time_advance(now_ns: u64) -> WireTraceRecord {
    WireTraceRecord {
        kind: KIND_QOS,
        route_identity: 0,
        correlation: 0,
        sequence: 5,
        now_ns,
        status: 0,
        event: 0,
        high_water: 0,
        order_class: ORDER_TIME as u8,
        ..call()
    }
}

#[test]
fn a_well_formed_record_is_admitted_and_round_trips_its_exact_length() {
    let record = call();
    assert!(valid_trace_record(&record));
    assert_eq!(record.encode().len(), TRACE_RECORD_LEN);
    assert_eq!(WireTraceRecord::decode(&record.encode()), Some(record));
}

#[test]
fn a_truncated_buffer_decodes_to_nothing() {
    let encoded = call().encode();
    assert!(WireTraceRecord::decode(&encoded[..TRACE_RECORD_LEN - 1]).is_none());
}

#[test]
fn the_encoding_is_byte_deterministic() {
    // Two independently constructed equal records must encode identically, or
    // the repeated-boot trace comparison compares padding rather than evidence.
    assert_eq!(call().encode(), call().encode());
}

#[test]
fn a_foreign_magic_or_version_is_refused() {
    let mut bad = call();
    bad.magic = TRACE_MAGIC ^ 1;
    assert!(!valid_trace_record(&bad));
    let mut bad = call();
    bad.version = fabric_trace::FORMAT_VERSION + 1;
    assert!(!valid_trace_record(&bad));
}

#[test]
fn every_declared_family_is_admitted_in_its_own_shape() {
    let families = [
        // schema admission: no edge, no correlation, no outcome
        WireTraceRecord {
            kind: KIND_SCHEMA,
            route_identity: 0,
            correlation: 0,
            status: 0,
            event: 0,
            ..call()
        },
        WireTraceRecord {
            kind: KIND_ROUTE,
            correlation: 0,
            event: 0,
            ..call()
        },
        WireTraceRecord {
            kind: KIND_QOS,
            event: 7,
            ..call()
        },
        call(),
        WireTraceRecord {
            kind: KIND_OPERATION,
            ..call()
        },
        WireTraceRecord {
            kind: KIND_VISIBILITY,
            correlation: 0,
            event: 1,
            ..call()
        },
        WireTraceRecord {
            kind: KIND_INTERPOSITION,
            correlation: 0,
            event: 1,
            ..call()
        },
        WireTraceRecord {
            kind: KIND_DENIAL,
            route_identity: 0,
            correlation: 0,
            event: 0,
            status: -13,
            ..call()
        },
        WireTraceRecord {
            kind: KIND_FAULT,
            status: -4,
            ..call()
        },
        WireTraceRecord {
            kind: KIND_RESOURCE,
            route_identity: 0,
            correlation: 0,
            status: 0,
            event: 5,
            ..call()
        },
        time_advance(2_000_050),
    ];
    for record in families {
        assert!(
            valid_trace_record(&record),
            "refused family {}",
            record.kind
        );
    }
}

#[test]
fn an_unknown_kind_is_refused_rather_than_skipped() {
    let mut bad = call();
    bad.kind = MAX_KIND + 1;
    assert!(!valid_trace_record(&bad));
    bad.kind = 0;
    assert!(!valid_trace_record(&bad));
}

#[test]
fn a_family_must_zero_the_fields_it_does_not_use() {
    // A schema admission naming a route edge is either a miswired emitter or a
    // route record wearing the wrong kind; either way it is not comparable.
    let mut bad = WireTraceRecord {
        kind: KIND_SCHEMA,
        route_identity: 0,
        correlation: 0,
        status: 0,
        event: 0,
        ..call()
    };
    bad.route_identity = ROUTE;
    assert!(!valid_trace_record(&bad));

    // A resource high-water count is not an event on an edge.
    let mut bad = WireTraceRecord {
        kind: KIND_RESOURCE,
        route_identity: 0,
        correlation: 0,
        status: 0,
        event: 5,
        ..call()
    };
    bad.correlation = 1;
    assert!(!valid_trace_record(&bad));
}

#[test]
fn a_correlated_family_must_name_its_edge_and_request() {
    let mut bad = call();
    bad.route_identity = 0;
    assert!(!valid_trace_record(&bad));
    let mut bad = call();
    bad.correlation = 0;
    // Zero correlation on a call also fails the time-class rule, which is the
    // point: only a clock advance may carry neither a correlation nor an edge.
    assert!(!valid_trace_record(&bad));
}

#[test]
fn a_denial_names_nothing_and_reports_a_refusal() {
    // A refusal carries the fact that something was refused and nothing else:
    // naming the edge would confirm it exists, and echoing the correlation would
    // republish an identity the broker just rejected -- which on a shared route
    // may belong to another client.
    let denial = WireTraceRecord {
        kind: KIND_DENIAL,
        route_identity: 0,
        correlation: 0,
        event: 0,
        status: -13,
        ..call()
    };
    assert!(valid_trace_record(&denial));
    // A denial with a success status is not a denial.
    let mut bad = denial;
    bad.status = 0;
    assert!(!valid_trace_record(&bad));
    // And each field it must withhold is refused individually.
    for mutate in [
        (|record: &mut WireTraceRecord| record.route_identity = ROUTE) as fn(&mut WireTraceRecord),
        |record: &mut WireTraceRecord| record.correlation = 9,
        |record: &mut WireTraceRecord| record.event = 1,
    ] {
        let mut leaky = denial;
        mutate(&mut leaky);
        assert!(!valid_trace_record(&leaky), "a denial must name nothing");
    }
}

#[test]
fn only_a_clock_advance_may_claim_the_time_class() {
    // A data record claiming the class that sorts last at its instant would
    // make the tie order meaningless.
    let mut bad = call();
    bad.order_class = ORDER_TIME as u8;
    assert!(!valid_trace_record(&bad));
    // And a clock advance must not masquerade as data.
    let mut bad = time_advance(500);
    bad.order_class = ORDER_DATA as u8;
    assert!(!valid_trace_record(&bad));
}

#[test]
fn an_order_class_outside_the_declared_set_is_refused() {
    let mut bad = call();
    bad.order_class = 0;
    assert!(!valid_trace_record(&bad));
    bad.order_class = MAX_ORDER_CLASS as u8 + 1;
    assert!(!valid_trace_record(&bad));
}

#[test]
fn unknown_flags_and_contradictory_flags_are_refused() {
    let mut bad = call();
    bad.flags = 0b100;
    assert!(!valid_trace_record(&bad));
    // A saturation report is not the record that ends the stream.
    let mut bad = call();
    bad.flags = FLAG_TERMINAL | FLAG_DROPPED;
    bad.high_water = 1;
    assert!(!valid_trace_record(&bad));
}

#[test]
fn a_saturation_report_must_count_what_it_refused() {
    let mut record = call();
    record.flags = FLAG_DROPPED;
    record.high_water = 0;
    assert!(!valid_trace_record(&record), "silent loss must fail closed");
    record.high_water = 3;
    assert!(valid_trace_record(&record));
}

#[test]
fn a_terminal_record_needs_no_drop_count() {
    let mut record = call();
    record.flags = FLAG_TERMINAL;
    record.high_water = 0;
    assert!(valid_trace_record(&record));
}

#[test]
fn reserved_bytes_must_be_zero() {
    let mut bad = call();
    bad.reserved = [0, 1, 0];
    assert!(!valid_trace_record(&bad));
}

#[test]
fn the_declared_order_is_lexicographic_on_time_then_class_then_sequence() {
    let data = call();
    let ack = WireTraceRecord {
        order_class: ORDER_ACK as u8,
        ..data
    };
    let peer_death = WireTraceRecord {
        kind: KIND_FAULT,
        status: -4,
        order_class: ORDER_PEER_DEATH as u8,
        ..data
    };
    let time = time_advance(data.now_ns);
    // All four classes at one instant, in the declared order.
    for pair in [(data, ack), (ack, peer_death), (peer_death, time)] {
        assert!(trace_records_ordered(&pair.0, &pair.1));
        assert!(!trace_records_ordered(&pair.1, &pair.0));
    }
    // Within one class, sequence breaks the tie.
    let later = WireTraceRecord {
        sequence: data.sequence + 1,
        ..data
    };
    assert!(trace_records_ordered(&data, &later));
    assert!(!trace_records_ordered(&later, &data));
    // A later instant outranks any class at an earlier one.
    assert!(trace_records_ordered(&time, &call_at(data.now_ns + 1)));
}

fn call_at(now_ns: u64) -> WireTraceRecord {
    WireTraceRecord { now_ns, ..call() }
}

#[test]
fn a_bounded_sink_reserves_room_for_its_terminal_records() {
    // The reserve is part of the format, not a component's choice: a sink that
    // could spend its last slot on ordinary evidence would have nowhere to
    // write the record saying the trace ended.
    assert!(TERMINAL_RESERVE > 0);
    assert!(MAX_TRACE_DEPTH > TERMINAL_RESERVE);
}

#[test]
fn a_resource_record_must_name_which_count_it_carries() {
    // A bare number is not evidence: without a counter identity a reader cannot
    // tell frames from operations, and two runs counting different tables would
    // compare as equal.
    let mut record = WireTraceRecord {
        kind: KIND_RESOURCE,
        route_identity: 0,
        correlation: 0,
        status: 0,
        event: fabric_trace::RESOURCE_FRAMES,
        high_water: 7,
        ..call()
    };
    assert!(valid_trace_record(&record));
    record.event = 0;
    assert!(!valid_trace_record(&record));
    record.event = fabric_trace::MAX_RESOURCE_COUNTER + 1;
    assert!(!valid_trace_record(&record));
    // Every declared counter is admitted.
    for counter in [
        fabric_trace::RESOURCE_FRAMES,
        fabric_trace::RESOURCE_OPERATIONS,
        fabric_trace::RESOURCE_CALLS,
        fabric_trace::RESOURCE_SINK_DROPPED,
        fabric_trace::RESOURCE_ROLES,
        fabric_trace::RESOURCE_BUFFERS,
        fabric_trace::RESOURCE_RETRIES,
        fabric_trace::RESOURCE_RETAINED,
        fabric_trace::RESOURCE_QUEUE,
        fabric_trace::RESOURCE_HISTORY,
        fabric_trace::RESOURCE_EVENT,
        fabric_trace::RESOURCE_LOAN,
        fabric_trace::RESOURCE_MAPPING,
        fabric_trace::RESOURCE_CAPABILITY_SLOTS,
        fabric_trace::RESOURCE_COMPLETE,
    ] {
        record.event = counter;
        assert!(valid_trace_record(&record), "counter {counter} refused");
    }
}

#[test]
fn a_graph_record_must_name_a_declared_event() {
    // Visibility and interposition are graph-shaped: an edge, no outcome, and
    // an event naming what was observed or traversed. A bare nonzero number
    // is not evidence unless it is drawn from the declared vocabulary, the
    // same reason a resource counter is bounded.
    let mut record = WireTraceRecord {
        kind: KIND_VISIBILITY,
        correlation: 0,
        event: fabric_trace::GRAPH_VIEW_ANSWERED,
        ..call()
    };
    assert!(valid_trace_record(&record));
    record.event = 0;
    assert!(!valid_trace_record(&record));
    record.event = fabric_trace::MAX_GRAPH_EVENT + 1;
    assert!(!valid_trace_record(&record));
    for (kind, event) in [
        (KIND_VISIBILITY, fabric_trace::GRAPH_VIEW_ANSWERED),
        (KIND_VISIBILITY, fabric_trace::GRAPH_HOP_TRAVERSED),
        (KIND_INTERPOSITION, fabric_trace::GRAPH_VIEW_ANSWERED),
        (KIND_INTERPOSITION, fabric_trace::GRAPH_HOP_TRAVERSED),
    ] {
        record.kind = kind;
        record.event = event;
        assert!(
            valid_trace_record(&record),
            "kind {kind} event {event} refused"
        );
    }
}
