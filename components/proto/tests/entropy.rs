use slime_proto::{
    entropy_service::{
        self, ENTROPY_SERVICE_MAGIC, MAX_DRAW, OP_CLOSE, OP_DRAW, WireEntropyServiceReply,
        WireEntropyServiceRequest,
    },
    entropy_source::{
        self, ENTROPY_SOURCE_MAGIC, MAX_FILL, OP_FILL, OP_RELEASE, WireEntropySourceReply,
        WireEntropySourceRequest,
    },
    syscall_abi::MAX_MSG,
    valid_entropy_service_reply, valid_entropy_service_request, valid_entropy_source_reply,
    valid_entropy_source_request,
};

const PINNED_FILL: [u8; entropy_source::REQUEST_LEN] = [
    b'R', b'N', b'G', b'S', // magic
    1, 0, 0, 0, // version
    1, 0, 0, 0, // fill
    48, 0, 0, 0, // length
];

const PINNED_DRAW: [u8; entropy_service::REQUEST_LEN] = [
    b'E', b'N', b'T', b'R', // magic
    1, 0, 0, 0, // version
    1, 0, 0, 0, // draw
    32, 0, 0, 0, // length
];

fn source_request(op: u32, length: u32) -> WireEntropySourceRequest {
    WireEntropySourceRequest {
        magic: ENTROPY_SOURCE_MAGIC,
        version: entropy_source::FORMAT_VERSION,
        op,
        length,
    }
}

fn service_request(op: u32, length: u32) -> WireEntropyServiceRequest {
    WireEntropyServiceRequest {
        magic: ENTROPY_SERVICE_MAGIC,
        version: entropy_service::FORMAT_VERSION,
        op,
        length,
    }
}

#[test]
fn both_replies_fit_one_kernel_message() {
    assert!(entropy_source::REPLY_LEN <= MAX_MSG);
    assert!(entropy_service::REPLY_LEN <= MAX_MSG);
    assert_eq!(entropy_source::REPLY_LEN, 16 + MAX_FILL);
    assert_eq!(entropy_service::REPLY_LEN, 16 + MAX_DRAW);
}

#[test]
fn the_pinned_vectors_are_the_generated_encodings() {
    let fill = source_request(OP_FILL, 48);
    assert_eq!(fill.encode(), PINNED_FILL);
    assert_eq!(WireEntropySourceRequest::decode(&PINNED_FILL), Some(fill));
    assert!(WireEntropySourceRequest::decode(&PINNED_FILL[..15]).is_none());
    let draw = service_request(OP_DRAW, 32);
    assert_eq!(draw.encode(), PINNED_DRAW);
    assert_eq!(WireEntropyServiceRequest::decode(&PINNED_DRAW), Some(draw));
    assert!(WireEntropyServiceRequest::decode(&PINNED_DRAW[..15]).is_none());
}

#[test]
fn source_requests_are_refused_outside_their_bounds() {
    assert!(valid_entropy_source_request(&source_request(OP_FILL, 1)));
    assert!(valid_entropy_source_request(&source_request(OP_FILL, 48)));
    assert!(valid_entropy_source_request(&source_request(OP_RELEASE, 0)));
    assert!(!valid_entropy_source_request(&source_request(OP_FILL, 0)));
    assert!(!valid_entropy_source_request(&source_request(OP_FILL, 49)));
    assert!(!valid_entropy_source_request(&source_request(
        OP_RELEASE, 1
    )));
    assert!(!valid_entropy_source_request(&source_request(3, 1)));
    assert!(!valid_entropy_source_request(&WireEntropySourceRequest {
        magic: ENTROPY_SERVICE_MAGIC,
        ..source_request(OP_FILL, 1)
    }));
    assert!(!valid_entropy_source_request(&WireEntropySourceRequest {
        version: 2,
        ..source_request(OP_FILL, 1)
    }));
}

#[test]
fn service_requests_are_refused_outside_their_bounds() {
    assert!(valid_entropy_service_request(&service_request(OP_DRAW, 1)));
    assert!(valid_entropy_service_request(&service_request(OP_DRAW, 32)));
    assert!(valid_entropy_service_request(&service_request(OP_CLOSE, 0)));
    assert!(!valid_entropy_service_request(&service_request(OP_DRAW, 0)));
    assert!(!valid_entropy_service_request(&service_request(
        OP_DRAW, 33
    )));
    assert!(!valid_entropy_service_request(&service_request(
        OP_CLOSE, 4
    )));
    assert!(!valid_entropy_service_request(&service_request(0, 1)));
    assert!(!valid_entropy_service_request(&WireEntropyServiceRequest {
        magic: ENTROPY_SOURCE_MAGIC,
        ..service_request(OP_DRAW, 1)
    }));
}

#[test]
fn only_an_ok_reply_carries_bytes_and_none_past_its_length() {
    let mut bytes = [0u8; MAX_FILL];
    bytes[..4].copy_from_slice(&[1, 2, 3, 4]);
    let ok = WireEntropySourceReply {
        magic: ENTROPY_SOURCE_MAGIC,
        version: entropy_source::FORMAT_VERSION,
        status: entropy_source::STATUS_OK,
        length: 4,
        bytes,
    };
    assert!(valid_entropy_source_reply(&ok));
    assert_eq!(WireEntropySourceReply::decode(&ok.encode()), Some(ok));
    assert!(!valid_entropy_source_reply(&WireEntropySourceReply {
        length: 3,
        ..ok
    }));
    assert!(!valid_entropy_source_reply(&WireEntropySourceReply {
        length: 49,
        ..ok
    }));
    assert!(!valid_entropy_source_reply(&WireEntropySourceReply {
        status: entropy_source::STATUS_UNAVAILABLE,
        ..ok
    }));
    assert!(valid_entropy_source_reply(&WireEntropySourceReply {
        status: entropy_source::STATUS_UNAVAILABLE,
        length: 0,
        bytes: [0; MAX_FILL],
        ..ok
    }));
    assert!(!valid_entropy_source_reply(&WireEntropySourceReply {
        status: -9,
        length: 0,
        bytes: [0; MAX_FILL],
        ..ok
    }));

    let refused = WireEntropyServiceReply {
        magic: ENTROPY_SERVICE_MAGIC,
        version: entropy_service::FORMAT_VERSION,
        status: entropy_service::STATUS_EXHAUSTED,
        length: 0,
        bytes: [0; MAX_DRAW],
    };
    assert!(valid_entropy_service_reply(&refused));
    assert!(!valid_entropy_service_reply(&WireEntropyServiceReply {
        length: 32,
        ..refused
    }));
    assert!(valid_entropy_service_reply(&WireEntropyServiceReply {
        status: entropy_service::STATUS_OK,
        length: 32,
        bytes: [7; MAX_DRAW],
        ..refused
    }));
    assert!(valid_entropy_service_reply(&WireEntropyServiceReply {
        status: entropy_service::STATUS_OK,
        length: 0,
        ..refused
    }));
    assert!(!valid_entropy_service_reply(&WireEntropyServiceReply {
        status: entropy_service::STATUS_OK,
        length: 0,
        bytes: [7; MAX_DRAW],
        ..refused
    }));
}
