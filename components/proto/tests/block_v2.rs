use slime_proto::block_v2::{self, WireBlockReply, WireBlockRequest};
use slime_proto::{valid_block_v2_completion, valid_block_v2_request};

fn request(op: u8, lba: u64, sector_count: u32) -> WireBlockRequest {
    WireBlockRequest {
        magic: block_v2::BLOCK_MAGIC,
        version: block_v2::FORMAT_VERSION,
        op,
        flags: 0,
        lba,
        sector_count,
        reserved: [0; 4],
        padding: [0; 32],
    }
}

#[test]
fn async_payloads_fill_the_io0_envelope_exactly_and_round_trip() {
    let request = request(block_v2::OP_READ, 7, 3);
    assert_eq!(request.encode().len(), 56);
    assert_eq!(WireBlockRequest::decode(&request.encode()), Some(request));

    let reply = WireBlockReply {
        magic: block_v2::BLOCK_MAGIC,
        version: block_v2::FORMAT_VERSION,
        op: block_v2::OP_READ,
        reserved: [0],
        sectors_done: 3,
        device_status: block_v2::DEVICE_STATUS_OK,
        detail: 0,
    };
    assert_eq!(reply.encode().len(), 24);
    assert_eq!(WireBlockReply::decode(&reply.encode()), Some(reply));
}

#[test]
fn multi_sector_bounds_and_overflow_are_refused_before_dma() {
    assert!(valid_block_v2_request(&request(block_v2::OP_READ, 1, 1)));
    assert!(valid_block_v2_request(&request(
        block_v2::OP_WRITE,
        8,
        block_v2::MAX_SECTORS_PER_REQUEST,
    )));
    assert!(!valid_block_v2_request(&request(block_v2::OP_READ, 1, 0)));
    assert!(!valid_block_v2_request(&request(
        block_v2::OP_WRITE,
        1,
        block_v2::MAX_SECTORS_PER_REQUEST + 1,
    )));
    assert!(!valid_block_v2_request(&request(
        block_v2::OP_READ,
        u64::MAX,
        2
    )));
}

#[test]
fn control_operations_carry_no_slice_geometry_in_the_payload() {
    assert!(valid_block_v2_request(&request(block_v2::OP_FLUSH, 0, 0)));
    assert!(valid_block_v2_request(&request(
        block_v2::OP_GEOMETRY,
        0,
        0
    )));
    assert!(!valid_block_v2_request(&request(block_v2::OP_FLUSH, 1, 0)));
    assert!(!valid_block_v2_request(&request(
        block_v2::OP_GEOMETRY,
        0,
        1
    )));
}

#[test]
fn completion_never_claims_more_than_the_requested_prefix() {
    let mut reply = WireBlockReply {
        magic: block_v2::BLOCK_MAGIC,
        version: block_v2::FORMAT_VERSION,
        op: block_v2::OP_WRITE,
        reserved: [0],
        sectors_done: 2,
        device_status: block_v2::DEVICE_STATUS_IO_ERR,
        detail: 9,
    };
    assert!(valid_block_v2_completion(&reply, 3));
    reply.sectors_done = 4;
    assert!(!valid_block_v2_completion(&reply, 3));
    reply.sectors_done = 0;
    reply.device_status = 99;
    assert!(!valid_block_v2_completion(&reply, 3));
}
