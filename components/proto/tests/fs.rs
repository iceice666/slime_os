use slime_proto::{
    fs::{self, WireFsReply, WireFsRequest},
    valid_fs_reply, valid_fs_request,
};

fn request(op: u8, name: &[u8]) -> WireFsRequest {
    let mut encoded_name = [0u8; fs::MAX_NAME_BYTES];
    encoded_name[..name.len()].copy_from_slice(name);
    WireFsRequest {
        magic: fs::FS_MAGIC,
        version: fs::FORMAT_VERSION,
        op,
        flags: 0,
        name_len: name.len() as u8,
        reserved0: 0,
        name: encoded_name,
        payload_len: 0,
        hash0: 0,
        hash1: 0,
        hash2: 0,
        hash3: 0,
    }
}

#[test]
fn request_round_trips_byte_identically() {
    for request in [request(fs::OP_LIST, b""), request(fs::OP_READ, b"note")] {
        assert!(valid_fs_request(&request));
        let encoded = request.encode();
        assert_eq!(WireFsRequest::decode(&encoded), Some(request));
        assert_eq!(WireFsRequest::decode(&encoded).unwrap().encode(), encoded);
    }
    let mut write = request(fs::OP_WRITE, b"new.txt");
    write.payload_len = 16;
    write.hash0 = 1;
    assert!(valid_fs_request(&write));
    assert_eq!(WireFsRequest::decode(&write.encode()), Some(write));
}
#[test]
fn malformed_names_versions_and_bounds_fail_closed() {
    assert!(WireFsRequest::decode(&[0; fs::REQUEST_LEN - 1]).is_none());
    for invalid in [b".".as_slice(), b"..", b"a/b", b"bad name"] {
        assert!(!valid_fs_request(&request(fs::OP_READ, invalid)));
    }
    let mut unknown = request(fs::OP_READ, b"note");
    unknown.version += 1;
    assert!(!valid_fs_request(&unknown));
    let mut trailing = request(fs::OP_READ, b"note");
    trailing.name[5] = b'x';
    assert!(!valid_fs_request(&trailing));
    assert!(!valid_fs_request(&request(fs::OP_READ, b"")));
    let mut list_with_hash = request(fs::OP_LIST, b"");
    list_with_hash.hash0 = 1;
    assert!(!valid_fs_request(&list_with_hash));
    let mut empty_write = request(fs::OP_WRITE, b"new.txt");
    empty_write.payload_len = 16;
    assert!(!valid_fs_request(&empty_write));
    let mut oversized_write = empty_write;
    oversized_write.hash0 = 1;
    oversized_write.payload_len = 32 * 1024 + 1;
    assert!(!valid_fs_request(&oversized_write));
}

#[test]
fn replies_round_trip_and_enforce_entry_bound() {
    let reply = WireFsReply {
        magic: fs::FS_MAGIC,
        version: fs::FORMAT_VERSION,
        status: 0,
        entry_count: 2,
        object_type: 7,
        payload_len: 16,
        hash0: 1,
        hash1: 2,
        hash2: 3,
        hash3: 4,
        reserved: 0,
    };
    assert!(valid_fs_reply(&reply));
    let encoded = reply.encode();
    assert_eq!(WireFsReply::decode(&encoded), Some(reply));
    assert!(WireFsReply::decode(&encoded[..fs::REPLY_LEN - 1]).is_none());
    assert!(!valid_fs_reply(&WireFsReply {
        entry_count: fs::MAX_ENTRIES as u32 + 1,
        ..reply
    }));
}
