use slime_proto::link_device::{
    self, LINK_MAGIC, LINK_UP, MAX_FRAME_BYTES, MIN_FRAME_BYTES, OP_PROVIDE_RECEIVE, OP_QUERY_LINK,
    OP_STATISTICS, OP_TRANSMIT, REPLY_LEN, REQUEST_LEN, WireLinkReply, WireLinkRequest,
};
use slime_proto::{
    valid_link_frame_bounds, valid_link_reply, valid_link_request, valid_link_state,
};

fn transmit_request() -> WireLinkRequest {
    WireLinkRequest {
        magic: LINK_MAGIC,
        version: link_device::FORMAT_VERSION,
        op: OP_TRANSMIT,
        flags: 0,
        frame_len: MIN_FRAME_BYTES as u16,
        reserved: [0; 2],
        padding: [0; 44],
    }
}

fn receive_reply() -> WireLinkReply {
    WireLinkReply {
        magic: LINK_MAGIC,
        version: link_device::FORMAT_VERSION,
        op: OP_PROVIDE_RECEIVE,
        link_state: LINK_UP,
        frame_len: MIN_FRAME_BYTES as u16,
        reserved: [0; 2],
        tx_frames: 0,
        rx_frames: 0,
        detail: 0,
    }
}

#[test]
fn request_and_reply_round_trip_byte_identically() {
    let request = transmit_request();
    let request_bytes = request.encode();
    assert_eq!(request_bytes.len(), REQUEST_LEN);
    let decoded = WireLinkRequest::decode(&request_bytes).expect("complete request");
    assert_eq!(decoded, request);
    assert_eq!(decoded.encode(), request_bytes);

    let reply = receive_reply();
    let reply_bytes = reply.encode();
    assert_eq!(reply_bytes.len(), REPLY_LEN);
    let decoded = WireLinkReply::decode(&reply_bytes).expect("complete reply");
    assert_eq!(decoded, reply);
    assert_eq!(decoded.encode(), reply_bytes);
}

#[test]
fn short_buffers_decode_to_none() {
    let request = transmit_request().encode();
    assert_eq!(WireLinkRequest::decode(&request[..REQUEST_LEN - 1]), None);

    let reply = receive_reply().encode();
    assert_eq!(WireLinkReply::decode(&reply[..REPLY_LEN - 1]), None);
}

#[test]
fn valid_payloads_and_vocabularies_are_admitted() {
    assert!(valid_link_request(&transmit_request()));

    let mut replenish = transmit_request();
    replenish.op = OP_PROVIDE_RECEIVE;
    replenish.frame_len = MAX_FRAME_BYTES as u16;
    assert!(valid_link_request(&replenish));

    let mut query = transmit_request();
    query.op = OP_QUERY_LINK;
    query.frame_len = 0;
    assert!(valid_link_request(&query));

    assert!(valid_link_reply(&receive_reply()));
    assert!(valid_link_frame_bounds(MIN_FRAME_BYTES));
    assert!(valid_link_frame_bounds(MAX_FRAME_BYTES));
    for state in [
        link_device::LINK_UNKNOWN,
        link_device::LINK_DOWN,
        link_device::LINK_UP,
    ] {
        assert!(valid_link_state(state));
    }
}

#[test]
fn bad_version_and_unknown_operation_are_rejected() {
    let mut bad_version = transmit_request();
    bad_version.version += 1;
    assert!(!valid_link_request(&bad_version));

    let mut bad_magic = transmit_request();
    bad_magic.magic ^= 1;
    assert!(!valid_link_request(&bad_magic));

    let mut unknown = transmit_request();
    unknown.op = 0xff;
    assert!(!valid_link_request(&unknown));

    let mut reply = receive_reply();
    reply.version += 1;
    assert!(!valid_link_reply(&reply));
    reply.version = link_device::FORMAT_VERSION;
    reply.magic ^= 1;
    assert!(!valid_link_reply(&reply));
    reply.magic ^= 1;
    reply.op = 0xff;
    assert!(!valid_link_reply(&reply));
}

#[test]
fn oversized_and_short_frames_are_rejected() {
    let mut oversized = transmit_request();
    oversized.frame_len = (MAX_FRAME_BYTES + 1) as u16;
    assert!(!valid_link_request(&oversized));
    assert!(!valid_link_frame_bounds(MAX_FRAME_BYTES + 1));

    let mut short = transmit_request();
    short.frame_len = (MIN_FRAME_BYTES - 1) as u16;
    assert!(!valid_link_request(&short));
    assert!(!valid_link_frame_bounds(MIN_FRAME_BYTES - 1));

    let mut reply = receive_reply();
    reply.frame_len = (MAX_FRAME_BYTES + 1) as u16;
    assert!(!valid_link_reply(&reply));
    reply.frame_len = (MIN_FRAME_BYTES - 1) as u16;
    assert!(!valid_link_reply(&reply));
}

#[test]
fn dirty_reserved_and_padding_are_rejected() {
    let mut dirty_reserved = transmit_request();
    dirty_reserved.reserved[0] = 1;
    assert!(!valid_link_request(&dirty_reserved));

    let mut dirty_padding = transmit_request();
    dirty_padding.padding[0] = 1;
    assert!(!valid_link_request(&dirty_padding));

    let mut dirty_reply = receive_reply();
    dirty_reply.reserved[1] = 1;
    assert!(!valid_link_reply(&dirty_reply));
}

#[test]
fn control_and_statistics_fields_are_operation_specific() {
    let mut query = receive_reply();
    query.op = OP_QUERY_LINK;
    query.frame_len = 0;
    assert!(valid_link_reply(&query));
    query.tx_frames = 1;
    assert!(!valid_link_reply(&query));

    let mut statistics = receive_reply();
    statistics.op = OP_STATISTICS;
    statistics.frame_len = 0;
    statistics.tx_frames = 7;
    statistics.rx_frames = 9;
    assert!(valid_link_reply(&statistics));

    let mut bad_state = statistics;
    bad_state.link_state = 0xff;
    assert!(!valid_link_reply(&bad_state));
    assert!(!valid_link_state(0xff));
}
