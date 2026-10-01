use slime_proto::{
    spawn::{self, WireSpawnFrame, WireSpawnReply, WireSpawnRequest},
    spawn_arguments::SpawnArguments,
    valid_spawn_reply, valid_spawn_request,
};

fn request() -> WireSpawnRequest {
    WireSpawnRequest {
        magic: spawn::SPAWN_MAGIC,
        version: spawn::FORMAT_VERSION,
        flags: 0,
        command_len: 7,
        argument_count: 0,
        environment_count: 0,
        capability_roles: 0,
        client_budget: 1,
        command: *b"sysinfo\0\0\0\0\0\0\0\0\0",
        argument_len0: 0,
        argument_len1: 0,
        argument_len2: 0,
        argument_len3: 0,
        environment: [0; 8],
        grant_rights: 0,
        argument_bytes: 0,
        supervision_handle: 0,
        reserved: [0; 4],
    }
}

fn frame(sequence: u16, offset: u16, bytes: &[u8], final_frame: bool) -> WireSpawnFrame {
    let mut payload = [0; spawn::FRAME_PAYLOAD_BYTES];
    payload[..bytes.len()].copy_from_slice(bytes);
    WireSpawnFrame {
        magic: spawn::FRAME_MAGIC,
        version: spawn::FORMAT_VERSION,
        sequence,
        offset,
        length: bytes.len() as u16,
        flags: if final_frame {
            spawn::FRAME_FLAG_FINAL
        } else {
            0
        },
        payload,
    }
}

#[test]
fn request_and_reply_round_trip() {
    let r = request();
    assert_eq!(WireSpawnRequest::decode(&r.encode()), Some(r));
    assert!(valid_spawn_request(&r));
    let reply = WireSpawnReply {
        magic: spawn::SPAWN_MAGIC,
        version: spawn::FORMAT_VERSION,
        status: -1,
        termination_kind: 4,
        supervision_slot: 3,
        detail: 7,
    };
    assert_eq!(WireSpawnReply::decode(&reply.encode()), Some(reply));
    assert!(valid_spawn_reply(&reply));
    assert!(WireSpawnRequest::decode(&r.encode()[..spawn::REQUEST_LEN - 1]).is_none());
    assert!(WireSpawnFrame::decode(&[0; spawn::FRAME_LEN - 1]).is_none());
}

#[test]
fn header_versions_counts_lengths_and_padding_fail_closed() {
    let base = request();
    for bad in [
        WireSpawnRequest { version: 1, ..base },
        WireSpawnRequest {
            command_len: 17,
            ..base
        },
        WireSpawnRequest {
            argument_count: 5,
            ..base
        },
        WireSpawnRequest {
            argument_bytes: 257,
            argument_count: 1,
            argument_len0: 257,
            ..base
        },
        WireSpawnRequest {
            argument_bytes: 1,
            ..base
        },
        WireSpawnRequest {
            argument_len1: 1,
            argument_bytes: 1,
            argument_count: 1,
            ..base
        },
        WireSpawnRequest {
            environment_count: 2,
            ..base
        },
        WireSpawnRequest {
            reserved: [1; 4],
            ..base
        },
    ] {
        assert!(!valid_spawn_request(&bad));
    }
}

#[test]
fn detached_uses_service_budget_and_wait_has_explicit_handle() {
    let detached = WireSpawnRequest {
        flags: spawn::REQUEST_FLAG_DETACHED,
        client_budget: 0,
        ..request()
    };
    assert!(valid_spawn_request(&detached));
    assert!(!valid_spawn_request(&WireSpawnRequest {
        client_budget: 1,
        ..detached
    }));
    let wait = WireSpawnRequest {
        flags: spawn::REQUEST_FLAG_WAIT,
        command_len: 0,
        command: [0; 16],
        supervision_handle: 3,
        ..request()
    };
    assert!(valid_spawn_request(&wait));
    assert!(!valid_spawn_request(&WireSpawnRequest {
        supervision_handle: 0,
        ..wait
    }));
}

#[test]
fn four_arguments_and_256_bytes_assemble_and_forward() {
    let r = WireSpawnRequest {
        argument_count: 4,
        argument_len0: 64,
        argument_len1: 64,
        argument_len2: 64,
        argument_len3: 64,
        argument_bytes: 256,
        ..request()
    };
    let bytes: [u8; 256] = core::array::from_fn(|i| i as u8);
    let mut args = SpawnArguments::new(&r).unwrap();
    assert!(!args.is_complete());
    for (sequence, part) in bytes.chunks(spawn::FRAME_PAYLOAD_BYTES).enumerate() {
        let offset = sequence * spawn::FRAME_PAYLOAD_BYTES;
        let f = frame(
            sequence as u16,
            offset as u16,
            part,
            offset + part.len() == bytes.len(),
        );
        assert_eq!(WireSpawnFrame::decode(&f.encode()), Some(f));
        assert_eq!(args.push(&f), Ok(offset + part.len() == bytes.len()));
    }
    for index in 0..4 {
        assert_eq!(
            args.argument(index),
            Some(&bytes[index * 64..(index + 1) * 64])
        );
    }
    assert!(args.argument(4).is_none());
    let mut forwarded = SpawnArguments::new(&r).unwrap();
    for sequence in 0..6 {
        forwarded.push(&args.frame(sequence).unwrap()).unwrap();
    }
    assert!(args.frame(6).is_none());
    assert_eq!(forwarded.argument(3), args.argument(3));
}

#[test]
fn missing_duplicate_gap_order_overrun_and_completion_fail_closed() {
    let r = WireSpawnRequest {
        argument_count: 1,
        argument_len0: 4,
        argument_bytes: 4,
        ..request()
    };
    let first = frame(0, 0, b"ab", false);
    let final_frame = frame(1, 2, b"cd", true);
    let mut partial = SpawnArguments::new(&r).unwrap();
    assert!(!partial.is_complete());
    assert!(partial.argument(0).is_none());
    assert_eq!(partial.push(&first), Ok(false));
    assert!(partial.push(&first).is_err());
    for bad in [
        frame(2, 2, b"cd", true),
        frame(1, 3, b"c", true),
        frame(1, 2, b"cde", true),
        frame(1, 2, b"c", true),
        frame(1, 2, b"cd", false),
        WireSpawnFrame {
            flags: 2,
            ..final_frame
        },
        WireSpawnFrame {
            version: 1,
            ..final_frame
        },
        WireSpawnFrame {
            payload: [1; spawn::FRAME_PAYLOAD_BYTES],
            ..final_frame
        },
    ] {
        let mut args = SpawnArguments::new(&r).unwrap();
        args.push(&first).unwrap();
        assert!(args.push(&bad).is_err());
        assert!(!args.is_complete());
    }
    let mut args = SpawnArguments::new(&r).unwrap();
    assert!(args.push(&final_frame).is_err());
    args.push(&first).unwrap();
    args.push(&final_frame).unwrap();
    assert!(args.push(&final_frame).is_err());
}

#[test]
fn empty_arguments_require_explicit_completion() {
    let r = WireSpawnRequest {
        argument_count: 2,
        ..request()
    };
    let mut args = SpawnArguments::new(&r).unwrap();
    assert!(!args.is_complete());
    assert_eq!(args.push(&frame(0, 0, b"", true)), Ok(true));
    assert_eq!(args.argument(0), Some(&b""[..]));
    assert_eq!(args.argument(1), Some(&b""[..]));
    assert_eq!(args.frame(0), Some(frame(0, 0, b"", true)));
    assert!(SpawnArguments::new(&request()).unwrap().is_complete());
}
