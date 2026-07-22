use slime_proto::{
    spawn::{
        CAPABILITY_ROLE_STDOUT, FORMAT_VERSION, REPLY_LEN, REQUEST_LEN, SPAWN_MAGIC,
        WireSpawnReply, WireSpawnRequest,
    },
    valid_spawn_reply, valid_spawn_request,
};

#[test]
fn request_round_trips_byte_identically() {
    let request = WireSpawnRequest {
        magic: SPAWN_MAGIC,
        version: FORMAT_VERSION,
        flags: 0,
        command_len: 7,
        argument_count: 1,
        environment_count: 1,
        capability_roles: CAPABILITY_ROLE_STDOUT,
        client_budget: 1,
        command: *b"sysinfo\0\0\0\0\0\0\0\0\0",
        arguments: [3, b'-', b'v', b'v', 0, 0, 0, 0],
        environment: [6, b'K', b'=', b'V', b'A', b'L', b'1', 0],
        grant_rights: 1,
        reserved: [0; 6],
    };
    let encoded = request.encode();
    assert_eq!(
        WireSpawnRequest::decode(&encoded).unwrap().encode(),
        encoded
    );
    assert!(valid_spawn_request(&request));
}

#[test]
fn reply_round_trips_and_short_messages_fail_closed() {
    let reply = WireSpawnReply {
        magic: SPAWN_MAGIC,
        version: FORMAT_VERSION,
        status: -1,
        termination_kind: 4,
        task_id: 9,
        supervision_slot: 3,
        detail: 7,
    };
    let encoded = reply.encode();
    assert_eq!(WireSpawnReply::decode(&encoded).unwrap().encode(), encoded);
    assert!(WireSpawnRequest::decode(&encoded[..REQUEST_LEN - 1]).is_none());
    assert!(WireSpawnReply::decode(&encoded[..REPLY_LEN - 1]).is_none());
    assert!(valid_spawn_reply(&reply));
    assert!(!valid_spawn_reply(&WireSpawnReply {
        version: FORMAT_VERSION + 1,
        ..reply
    }));
}

#[test]
fn request_validation_rejects_unknown_versions_and_bounds() {
    let base = WireSpawnRequest {
        magic: SPAWN_MAGIC,
        version: FORMAT_VERSION,
        flags: 0,
        command_len: 7,
        argument_count: 0,
        environment_count: 0,
        capability_roles: 0,
        client_budget: 1,
        command: *b"sysinfo\0\0\0\0\0\0\0\0\0",
        arguments: [0; 8],
        environment: [0; 8],
        grant_rights: 0,
        reserved: [0; 6],
    };
    assert!(!valid_spawn_request(&WireSpawnRequest {
        version: FORMAT_VERSION + 1,
        ..base
    }));
    assert!(!valid_spawn_request(&WireSpawnRequest {
        command_len: 17,
        ..base
    }));
    assert!(!valid_spawn_request(&WireSpawnRequest {
        argument_count: 3,
        ..base
    }));
    assert!(!valid_spawn_request(&WireSpawnRequest {
        environment_count: 2,
        ..base
    }));
    assert!(!valid_spawn_request(&WireSpawnRequest {
        argument_count: 1,
        arguments: [8; 8],
        ..base
    }));
    assert!(!valid_spawn_request(&WireSpawnRequest {
        environment_count: 1,
        environment: [6, b'K', b'=', b'V', 0, 0, 0, 1],
        ..base
    }));
}
