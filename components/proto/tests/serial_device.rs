use slime_proto::serial_device::{
    FORMAT_VERSION, MAX_PAYLOAD, OP_WRITE, REPLY_LEN, REQUEST_LEN, SERIAL_MAGIC, STATUS_BAD_LENGTH,
    STATUS_BAD_OP, STATUS_DEVICE_ERROR, STATUS_MALFORMED, STATUS_NO_DEVICE, STATUS_OK,
    STATUS_TIMEOUT, WireSerialReply, WireSerialRequest,
};
use slime_proto::{valid_serial_reply, valid_serial_request};

fn write_request(bytes: &[u8]) -> WireSerialRequest {
    let mut payload = [0u8; MAX_PAYLOAD];
    payload[..bytes.len()].copy_from_slice(bytes);
    WireSerialRequest {
        magic: SERIAL_MAGIC,
        version: FORMAT_VERSION,
        op: OP_WRITE,
        flags: 0,
        length: bytes.len() as u16,
        reserved: [0; 2],
        payload,
    }
}

fn reply(status: i32, bytes_written: u16) -> WireSerialReply {
    WireSerialReply {
        magic: SERIAL_MAGIC,
        version: FORMAT_VERSION,
        bytes_written,
        status,
        detail: 0,
    }
}

#[test]
fn request_and_reply_round_trip_byte_identically() {
    let request = write_request(b"slime");
    let request_bytes = request.encode();
    assert_eq!(request_bytes.len(), REQUEST_LEN);
    let decoded = WireSerialRequest::decode(&request_bytes).expect("complete request");
    assert_eq!(decoded, request);
    assert_eq!(decoded.encode(), request_bytes);
    assert_eq!(&request_bytes[..4], b"SRDV");

    let answer = reply(STATUS_TIMEOUT, 3);
    let reply_bytes = answer.encode();
    assert_eq!(reply_bytes.len(), REPLY_LEN);
    let decoded = WireSerialReply::decode(&reply_bytes).expect("complete reply");
    assert_eq!(decoded, answer);
    assert_eq!(decoded.encode(), reply_bytes);
}

#[test]
fn short_buffers_decode_to_none() {
    let request = write_request(b"x").encode();
    assert_eq!(WireSerialRequest::decode(&request[..REQUEST_LEN - 1]), None);
    let answer = reply(STATUS_OK, 1).encode();
    assert_eq!(WireSerialReply::decode(&answer[..REPLY_LEN - 1]), None);
}

#[test]
fn a_request_is_one_kernel_message_whose_payload_is_the_rest() {
    assert_eq!(REQUEST_LEN, 64);
    assert_eq!(REQUEST_LEN - MAX_PAYLOAD, 12);
}

#[test]
fn lengths_from_one_to_the_payload_bound_are_admitted_and_no_others() {
    assert!(valid_serial_request(&write_request(&[0xA5])));
    assert!(valid_serial_request(&write_request(&[0xA5; MAX_PAYLOAD])));
    let mut empty = write_request(&[0xA5]);
    empty.length = 0;
    empty.payload[0] = 0;
    assert!(!valid_serial_request(&empty));
    let mut oversized = write_request(&[0xA5; MAX_PAYLOAD]);
    oversized.length = MAX_PAYLOAD as u16 + 1;
    assert!(!valid_serial_request(&oversized));
}

#[test]
fn bad_identity_operation_and_flags_are_rejected() {
    let base = write_request(b"ok");
    for request in [
        WireSerialRequest { magic: 0, ..base },
        WireSerialRequest {
            version: FORMAT_VERSION + 1,
            ..base
        },
        WireSerialRequest { op: 0, ..base },
        WireSerialRequest {
            op: OP_WRITE + 1,
            ..base
        },
        WireSerialRequest { flags: 1, ..base },
    ] {
        assert!(!valid_serial_request(&request));
    }
}

#[test]
fn dirty_reserved_and_unused_payload_are_rejected() {
    let mut reserved = write_request(b"ok");
    reserved.reserved[1] = 1;
    assert!(!valid_serial_request(&reserved));
    let mut trailing = write_request(b"ok");
    trailing.payload[2] = 1;
    assert!(!valid_serial_request(&trailing));
    // A zero byte inside the declared length is data, not padding.
    assert!(valid_serial_request(&write_request(&[0, 0, 0])));
}

#[test]
fn every_status_is_distinct_and_only_named_statuses_are_admitted() {
    let statuses = [
        STATUS_OK,
        STATUS_BAD_LENGTH,
        STATUS_BAD_OP,
        STATUS_NO_DEVICE,
        STATUS_DEVICE_ERROR,
        STATUS_TIMEOUT,
        STATUS_MALFORMED,
    ];
    for (index, status) in statuses.iter().enumerate() {
        assert!(valid_serial_reply(&reply(*status, 0)));
        assert!(statuses[index + 1..].iter().all(|other| other != status));
        assert!(*status == STATUS_OK || *status < 0);
    }
    assert!(!valid_serial_reply(&reply(1, 0)));
    assert!(!valid_serial_reply(&reply(STATUS_MALFORMED - 1, 0)));
    assert!(!valid_serial_reply(&WireSerialReply {
        magic: 0,
        ..reply(STATUS_OK, 0)
    }));
    assert!(valid_serial_reply(&reply(STATUS_OK, MAX_PAYLOAD as u16)));
    assert!(!valid_serial_reply(&reply(
        STATUS_OK,
        MAX_PAYLOAD as u16 + 1
    )));
}
