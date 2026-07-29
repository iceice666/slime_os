use slime_proto::fabric_visibility::{
    EVENT_PROXY_LOST, FORMAT_VERSION, INTERPOSITION_TRACE_MAGIC, RECORD_LEN, STATUS_END,
    STATUS_RECORD, TRACE_RELAYED, VISIBILITY_QOS_MAGIC, VISIBILITY_REQUEST_MAGIC,
    VISIBILITY_ROUTE_MAGIC, WireInterpositionTrace, WireVisibilityQosRecord, WireVisibilityRequest,
    WireVisibilityRouteRecord,
};
use slime_proto::{
    valid_interposition_trace, valid_visibility_qos_record, valid_visibility_request,
    valid_visibility_route_record,
};

#[test]
fn every_visibility_record_is_exactly_one_kernel_message() {
    let request = WireVisibilityRequest {
        magic: VISIBILITY_REQUEST_MAGIC,
        version: FORMAT_VERSION,
        cursor: 3,
        flags: 0,
        reserved: [0; 56],
    };
    assert_eq!(request.encode().len(), RECORD_LEN);
    assert_eq!(
        WireVisibilityRequest::decode(&request.encode()),
        Some(request)
    );
    assert!(valid_visibility_request(&request));

    let mut route_name = [0; 16];
    route_name[..9].copy_from_slice(b"telemetry");
    let route = WireVisibilityRouteRecord {
        magic: VISIBILITY_ROUTE_MAGIC,
        version: FORMAT_VERSION,
        status: STATUS_RECORD,
        cursor: 4,
        contract_kind: 1,
        route_name_len: 9,
        reserved0: [0; 3],
        route_name,
        schema_identity: [0x5a; 32],
        flags: 0,
    };
    assert_eq!(
        WireVisibilityRouteRecord::decode(&route.encode()),
        Some(route)
    );
    assert!(valid_visibility_route_record(&route));

    let qos = WireVisibilityQosRecord {
        magic: VISIBILITY_QOS_MAGIC,
        version: FORMAT_VERSION,
        status: STATUS_RECORD,
        cursor: 5,
        flags: 0,
        route_name,
        reliability: 2,
        durability: 1,
        liveliness: 2,
        matched: 1,
        history_depth: 4,
        retained_depth: 0,
        deadline_ns: 10,
        lifespan_ns: 20,
        lease_ns: 30,
        event_mask: EVENT_PROXY_LOST,
    };
    assert_eq!(WireVisibilityQosRecord::decode(&qos.encode()), Some(qos));
    assert!(valid_visibility_qos_record(&qos));
}

#[test]
fn terminal_view_is_graph_independent_and_malformed_bytes_fail_closed() {
    let end = WireVisibilityRouteRecord {
        magic: VISIBILITY_ROUTE_MAGIC,
        version: FORMAT_VERSION,
        status: STATUS_END,
        cursor: u8::MAX,
        contract_kind: 0,
        route_name_len: 0,
        reserved0: [0; 3],
        route_name: [0; 16],
        schema_identity: [0; 32],
        flags: 0,
    };
    assert!(valid_visibility_route_record(&end));

    let mut dirty = end;
    dirty.schema_identity[0] = 1;
    assert!(!valid_visibility_route_record(&dirty));

    let mut request = WireVisibilityRequest {
        magic: VISIBILITY_REQUEST_MAGIC,
        version: FORMAT_VERSION,
        cursor: 0,
        flags: 0,
        reserved: [0; 56],
    };
    request.reserved[55] = 1;
    assert!(!valid_visibility_request(&request));
}

#[test]
fn interposition_trace_encoding_is_byte_deterministic() {
    let trace = WireInterpositionTrace {
        magic: INTERPOSITION_TRACE_MAGIC,
        version: FORMAT_VERSION,
        event: TRACE_RELAYED,
        flags: 0,
        route_identity: [0xa5; 32],
        sequence: 7,
        reserved: [0; 16],
    };
    let first = trace.encode();
    let second = trace.encode();
    assert_eq!(first, second);
    assert_eq!(WireInterpositionTrace::decode(&first), Some(trace));
    assert!(valid_interposition_trace(&trace));

    let mut stale = trace;
    stale.sequence = 0;
    assert!(!valid_interposition_trace(&stale));
}
