use slime_proto::{
    network_service::{self, WireNetworkCompletion, WireNetworkRequest},
    valid_network_completion, valid_network_loan, valid_network_request,
};

#[test]
fn launch_chunks_have_bounded_roles_lengths_and_canonical_padding() {
    use network_service::{LAUNCH_SEED, LAUNCH_URL, WireNetworkLaunch};
    use slime_proto::valid_network_launch;
    let seed = WireNetworkLaunch {
        magic: network_service::NETWORK_MAGIC,
        version: network_service::FORMAT_VERSION,
        kind: LAUNCH_SEED,
        reserved: 0,
        total_len: 32,
        offset: 0,
        length: 32,
        reserved2: [0; 2],
        payload: [0; 48],
    };
    assert!(valid_network_launch(&seed));
    assert_eq!(WireNetworkLaunch::decode(&seed.encode()), Some(seed));
    assert_eq!(seed.encode().len(), 64);
    assert!(WireNetworkLaunch::decode(&seed.encode()[..63]).is_none());
    for frame in [
        WireNetworkLaunch { magic: 0, ..seed },
        WireNetworkLaunch { version: 2, ..seed },
        WireNetworkLaunch { kind: 3, ..seed },
        WireNetworkLaunch {
            reserved: 1,
            ..seed
        },
        WireNetworkLaunch {
            reserved2: [1, 0],
            ..seed
        },
        WireNetworkLaunch { length: 0, ..seed },
        WireNetworkLaunch { length: 49, ..seed },
        WireNetworkLaunch { offset: 1, ..seed },
        WireNetworkLaunch {
            total_len: 33,
            ..seed
        },
        WireNetworkLaunch {
            payload: [1; 48],
            ..seed
        },
    ] {
        assert!(!valid_network_launch(&frame), "{frame:?}");
    }
    let url = WireNetworkLaunch {
        kind: LAUNCH_URL,
        total_len: 2048,
        offset: 2016,
        ..seed
    };
    assert!(valid_network_launch(&url));
    assert!(!valid_network_launch(&WireNetworkLaunch {
        offset: 2017,
        ..url
    }));
    assert!(!valid_network_launch(&WireNetworkLaunch {
        total_len: 2049,
        ..url
    }));
    assert!(!valid_network_launch(&WireNetworkLaunch {
        total_len: 0,
        offset: 0,
        ..url
    }));
}

#[test]
fn session_abort_is_control_only_and_cannot_select_another_holder() {
    let request = WireNetworkRequest {
        magic: network_service::NETWORK_MAGIC,
        version: network_service::FORMAT_VERSION,
        op: network_service::OP_ABORT,
        transport: network_service::TRANSPORT_NONE,
        flags: 0,
        port: 0,
        name_len: 0,
        capability: u64::MAX,
        address_kind: network_service::ADDRESS_NONE,
        reserved: [0; 7],
        endpoint: [0; 24],
    };
    assert!(slime_proto::valid_network_abort(&request));
    assert!(!valid_network_request(&request));
    for malformed in [
        WireNetworkRequest {
            capability: 1,
            ..request
        },
        WireNetworkRequest {
            flags: 1,
            ..request
        },
        WireNetworkRequest {
            endpoint: [1; 24],
            ..request
        },
        WireNetworkRequest {
            port: 80,
            ..request
        },
        WireNetworkRequest {
            transport: network_service::TRANSPORT_TCP,
            ..request
        },
        WireNetworkRequest {
            name_len: 1,
            ..request
        },
        WireNetworkRequest {
            address_kind: network_service::ADDRESS_DNS,
            ..request
        },
        WireNetworkRequest {
            reserved: [1; 7],
            ..request
        },
        WireNetworkRequest {
            op: network_service::OP_CLOSE,
            ..request
        },
    ] {
        assert!(!slime_proto::valid_network_abort(&malformed));
    }
    let completion = WireNetworkCompletion {
        magic: network_service::NETWORK_MAGIC,
        version: network_service::FORMAT_VERSION,
        op: network_service::OP_ABORT,
        capability_kind: network_service::CAPABILITY_NONE,
        status_detail: network_service::STATUS_SUCCESS,
        flags: 0,
        capability: 0,
    };
    assert!(valid_network_completion(&completion));
    assert!(!valid_network_completion(&WireNetworkCompletion {
        capability: 1,
        ..completion
    }));
}

fn dns_request(op: u8, transport: u8, name: &[u8], port: u16) -> WireNetworkRequest {
    let mut endpoint = [0u8; 24];
    endpoint[..name.len()].copy_from_slice(name);
    WireNetworkRequest {
        magic: network_service::NETWORK_MAGIC,
        version: network_service::FORMAT_VERSION,
        op,
        transport,
        flags: 0,
        port,
        name_len: name.len() as u16,
        capability: 0,
        address_kind: network_service::ADDRESS_DNS,
        reserved: [0; 7],
        endpoint,
    }
}

#[test]
fn request_and_completion_round_trip_exact_payload_sizes() {
    let request = dns_request(
        network_service::OP_CONNECT,
        network_service::TRANSPORT_TCP,
        b"api.example",
        443,
    );
    assert!(valid_network_request(&request));
    let encoded = request.encode();
    assert_eq!(encoded.len(), 56);
    assert_eq!(WireNetworkRequest::decode(&encoded), Some(request));
    assert!(WireNetworkRequest::decode(&encoded[..55]).is_none());

    let completion = WireNetworkCompletion {
        magic: network_service::NETWORK_MAGIC,
        version: network_service::FORMAT_VERSION,
        op: network_service::OP_CONNECT,
        capability_kind: network_service::CAPABILITY_TCP_CONNECTION,
        status_detail: 0,
        flags: 0,
        capability: 9,
    };
    assert!(valid_network_completion(&completion));
    let encoded = completion.encode();
    assert_eq!(encoded.len(), 24);
    assert_eq!(WireNetworkCompletion::decode(&encoded), Some(completion));
    assert!(WireNetworkCompletion::decode(&encoded[..23]).is_none());
}

#[test]
fn payload_loan_descriptor_and_endpoint_readiness_are_typed() {
    let descriptor = network_service::WireNetworkLoan {
        magic: network_service::NETWORK_MAGIC,
        version: network_service::FORMAT_VERSION,
        role: network_service::LOAN_ROLE_DATA,
        reserved0: [0; 1],
        buffer: 17,
        lease: 23,
        length: network_service::DATA_BYTES as u64,
        reserved: [0; 32],
    };
    assert!(valid_network_loan(
        &descriptor,
        network_service::LOAN_ROLE_DATA
    ));
    let bytes = descriptor.encode();
    assert_eq!(bytes.len(), 64);
    assert_eq!(
        network_service::WireNetworkLoan::decode(&bytes),
        Some(descriptor)
    );
    assert!(network_service::WireNetworkLoan::decode(&bytes[..63]).is_none());
    let readiness = WireNetworkCompletion {
        magic: network_service::NETWORK_MAGIC,
        version: network_service::FORMAT_VERSION,
        op: network_service::OP_ATTACH,
        capability_kind: network_service::CAPABILITY_NONE,
        status_detail: network_service::STATUS_SUCCESS,
        flags: 0,
        capability: 0,
    };
    assert!(valid_network_completion(&readiness));
    assert!(!valid_network_completion(&WireNetworkCompletion {
        capability: 1,
        ..readiness
    }));
    for status_detail in [1, -9, i32::MIN, i32::MAX] {
        assert!(!valid_network_completion(&WireNetworkCompletion {
            status_detail,
            ..readiness
        }));
    }
    assert!(!valid_network_completion(&WireNetworkCompletion {
        flags: network_service::FLAG_END_OF_STREAM,
        ..readiness
    }));
    let request = dns_request(
        network_service::OP_ATTACH,
        network_service::TRANSPORT_TCP,
        b"api.example",
        443,
    );
    assert!(!valid_network_request(&request));
}

#[test]
fn provisioning_loans_require_exact_role_size_identity_and_reserved_bytes() {
    use network_service::{LOAN_ROLE_DATA, LOAN_ROLE_RING, WireNetworkLoan};
    for (role, length) in [
        (LOAN_ROLE_RING, network_service::RING_BYTES as u64),
        (LOAN_ROLE_DATA, network_service::DATA_BYTES as u64),
    ] {
        let descriptor = WireNetworkLoan {
            magic: network_service::NETWORK_MAGIC,
            version: network_service::FORMAT_VERSION,
            role,
            reserved0: [0; 1],
            buffer: 17,
            lease: 23,
            length,
            reserved: [0; 32],
        };
        assert!(valid_network_loan(&descriptor, role));
        for malformed in [
            WireNetworkLoan {
                magic: 0,
                ..descriptor
            },
            WireNetworkLoan {
                version: 0,
                ..descriptor
            },
            WireNetworkLoan {
                role: 0,
                ..descriptor
            },
            WireNetworkLoan {
                role: u8::MAX,
                ..descriptor
            },
            WireNetworkLoan {
                role: if role == LOAN_ROLE_RING {
                    LOAN_ROLE_DATA
                } else {
                    LOAN_ROLE_RING
                },
                ..descriptor
            },
            WireNetworkLoan {
                buffer: 0,
                ..descriptor
            },
            WireNetworkLoan {
                lease: 0,
                ..descriptor
            },
            WireNetworkLoan {
                length: 0,
                ..descriptor
            },
            WireNetworkLoan {
                length: length - 1,
                ..descriptor
            },
            WireNetworkLoan {
                length: length + 1,
                ..descriptor
            },
            WireNetworkLoan {
                length: u64::MAX,
                ..descriptor
            },
            WireNetworkLoan {
                reserved0: [1],
                ..descriptor
            },
        ] {
            assert!(!valid_network_loan(&malformed, role), "{malformed:?}");
        }
        for index in 0..descriptor.reserved.len() {
            let mut malformed = descriptor;
            malformed.reserved[index] = 1;
            assert!(!valid_network_loan(&malformed, role));
        }
        for unknown_role in [0, 3, u8::MAX] {
            assert!(!valid_network_loan(&descriptor, unknown_role));
            assert!(!valid_network_loan(
                &WireNetworkLoan {
                    role: unknown_role,
                    ..descriptor
                },
                unknown_role
            ));
        }
    }
}

#[test]
fn validators_refuse_unknown_ops_flags_and_malformed_destinations() {
    let valid = dns_request(
        network_service::OP_CONNECT,
        network_service::TRANSPORT_TCP,
        b"api.example",
        443,
    );
    assert!(!valid_network_request(&WireNetworkRequest {
        op: 99,
        ..valid
    }));
    assert!(!valid_network_request(&WireNetworkRequest {
        flags: 2,
        ..valid
    }));
    assert!(!valid_network_request(&WireNetworkRequest {
        port: 0,
        ..valid
    }));
    assert!(!valid_network_request(&WireNetworkRequest {
        transport: network_service::TRANSPORT_NONE,
        ..valid
    }));
    let mut trailing = valid;
    trailing.endpoint[20] = 1;
    assert!(!valid_network_request(&trailing));
    let bad_name = dns_request(
        network_service::OP_CONNECT,
        network_service::TRANSPORT_TCP,
        b"*.example",
        443,
    );
    assert!(!valid_network_request(&bad_name));
}

#[test]
fn capability_operations_cannot_spell_a_raw_destination() {
    let request = WireNetworkRequest {
        magic: network_service::NETWORK_MAGIC,
        version: network_service::FORMAT_VERSION,
        op: network_service::OP_SEND,
        transport: network_service::TRANSPORT_NONE,
        flags: 0,
        port: 0,
        name_len: 0,
        capability: 42,
        address_kind: network_service::ADDRESS_NONE,
        reserved: [0; 7],
        endpoint: [0; 24],
    };
    assert!(valid_network_request(&request));
    assert!(!valid_network_request(&WireNetworkRequest {
        address_kind: network_service::ADDRESS_IPV4,
        endpoint: [1; 24],
        ..request
    }));
    assert!(!valid_network_request(&WireNetworkRequest {
        capability: 0,
        ..request
    }));

    let completion = WireNetworkCompletion {
        magic: network_service::NETWORK_MAGIC,
        version: network_service::FORMAT_VERSION,
        op: network_service::OP_SEND,
        capability_kind: network_service::CAPABILITY_NONE,
        status_detail: 0,
        flags: 0,
        capability: 0,
    };
    assert!(valid_network_completion(&completion));
    assert!(!valid_network_completion(&WireNetworkCompletion {
        capability_kind: network_service::CAPABILITY_TCP_CONNECTION,
        capability: 7,
        ..completion
    }));
}

fn payload_request(op: u8) -> WireNetworkRequest {
    WireNetworkRequest {
        magic: network_service::NETWORK_MAGIC,
        version: network_service::FORMAT_VERSION,
        op,
        transport: network_service::TRANSPORT_NONE,
        flags: 0,
        port: 0,
        name_len: 0,
        capability: 42,
        address_kind: network_service::ADDRESS_NONE,
        reserved: [0; 7],
        endpoint: [0; 24],
    }
}

#[test]
fn malformed_envelopes_consume_identity_and_cannot_alias_valid_requests() {
    use slime_proto::admit_network_request_id;
    use slime_proto::io_queue::{self, WireBufferSlice};
    use slime_proto::io_queue_ring::{Queue, format};
    let mut memory = [0; 4096];
    format(&mut memory, 4, 7).unwrap();
    let mut queue = Queue::attach(&mut memory, 4).unwrap();
    let slice = WireBufferSlice {
        buffer: 1,
        lease: 2,
        offset: 0,
        length: 2,
        direction: io_queue::DIRECTION_DEVICE_READ,
        reserved: [0; 4],
    };
    let mut last = 0;
    let mut out = [0; io_queue::REQUEST_PAYLOAD_BYTES];
    // Client's offered bound can exceed the service's mapped range.
    queue
        .submit(42, &slice, &[0; io_queue::REQUEST_PAYLOAD_BYTES], false, 2)
        .unwrap();
    let malformed = queue.take_request(&mut out, 1).unwrap_err();
    assert!(admit_network_request_id(
        &mut last,
        malformed.request_id,
        malformed.epoch,
        7
    ));
    queue
        .submit(42, &slice, &[0; io_queue::REQUEST_PAYLOAD_BYTES], false, 2)
        .unwrap();
    let valid = queue.take_request(&mut out, 2).unwrap();
    assert!(!admit_network_request_id(
        &mut last,
        valid.request_id,
        valid.epoch,
        7
    ));
    assert!(admit_network_request_id(&mut last, 43, 7, 7));
    queue
        .submit(43, &slice, &[0; io_queue::REQUEST_PAYLOAD_BYTES], false, 2)
        .unwrap();
    let malformed = queue.take_request(&mut out, 1).unwrap_err();
    assert!(!admit_network_request_id(
        &mut last,
        malformed.request_id,
        malformed.epoch,
        7
    ));
    assert!(!admit_network_request_id(&mut last, 44, 6, 7));
    assert!(!admit_network_request_id(&mut last, 0, 7, 7));
    assert_eq!(last, 43);
    assert!(admit_network_request_id(&mut last, u64::MAX, 7, 7));
    assert!(!admit_network_request_id(&mut last, u64::MAX, 7, 7));
}

#[test]
fn session_payload_binds_exact_identity_direction_and_checked_mapping_range() {
    use slime_proto::io_queue::{self, WireBufferSlice};
    use slime_proto::valid_network_payload_slice;
    let send = payload_request(network_service::OP_SEND);
    let recv = payload_request(network_service::OP_RECV);
    let slice = WireBufferSlice {
        buffer: 17,
        lease: 23,
        offset: 0,
        length: 4096,
        direction: io_queue::DIRECTION_DEVICE_READ,
        reserved: [0; 4],
    };
    let valid = |request: &WireNetworkRequest, slice: &WireBufferSlice| {
        valid_network_payload_slice(request, slice, 17, 23, 4096)
    };
    assert!(valid(&send, &slice));
    assert!(valid(
        &send,
        &WireBufferSlice {
            offset: 4095,
            length: 1,
            ..slice
        }
    ));
    assert!(valid(
        &recv,
        &WireBufferSlice {
            direction: io_queue::DIRECTION_DEVICE_WRITE,
            ..slice
        }
    ));
    assert!(!valid(&recv, &slice));
    for malformed in [
        WireBufferSlice {
            buffer: 18,
            ..slice
        },
        WireBufferSlice { buffer: 0, ..slice },
        WireBufferSlice { lease: 24, ..slice },
        WireBufferSlice { lease: 0, ..slice },
        WireBufferSlice { length: 0, ..slice },
        WireBufferSlice {
            length: 4097,
            ..slice
        },
        WireBufferSlice { offset: 1, ..slice },
        WireBufferSlice {
            offset: 4096,
            length: 1,
            ..slice
        },
        WireBufferSlice {
            offset: u64::MAX,
            length: 1,
            ..slice
        },
        WireBufferSlice {
            offset: 1,
            length: u64::MAX,
            ..slice
        },
        WireBufferSlice {
            reserved: [1, 0, 0, 0],
            ..slice
        },
        WireBufferSlice {
            direction: io_queue::DIRECTION_DEVICE_WRITE,
            ..slice
        },
        WireBufferSlice {
            direction: io_queue::DIRECTION_NONE,
            ..slice
        },
        WireBufferSlice {
            direction: u32::MAX,
            ..slice
        },
    ] {
        assert!(!valid(&send, &malformed), "{malformed:?}");
    }
    assert!(!valid_network_payload_slice(&send, &slice, 17, 23, 0));
    assert!(!valid_network_payload_slice(&send, &slice, 0, 23, 4096));
    assert!(!valid_network_payload_slice(&send, &slice, 17, 0, 4096));
    assert!(!valid(&WireNetworkRequest { op: 99, ..send }, &slice));
    assert!(!valid(
        &WireNetworkRequest {
            reserved: [1; 7],
            ..send
        },
        &slice
    ));
}

#[test]
fn control_requests_require_completely_empty_payload_slices() {
    use slime_proto::io_queue::{self, WireBufferSlice};
    use slime_proto::valid_network_payload_slice;
    let close = payload_request(network_service::OP_CLOSE);
    let connect = dns_request(
        network_service::OP_CONNECT,
        network_service::TRANSPORT_TCP,
        b"api.example",
        443,
    );
    let empty = WireBufferSlice {
        buffer: 0,
        lease: 0,
        offset: 0,
        length: 0,
        direction: io_queue::DIRECTION_NONE,
        reserved: [0; 4],
    };
    let valid = |request: &WireNetworkRequest, slice: &WireBufferSlice| {
        valid_network_payload_slice(request, slice, 17, 23, 4096)
    };
    for request in [close, connect] {
        assert!(valid(&request, &empty));
        for malformed in [
            WireBufferSlice {
                buffer: 17,
                ..empty
            },
            WireBufferSlice { lease: 23, ..empty },
            WireBufferSlice { offset: 1, ..empty },
            WireBufferSlice { length: 1, ..empty },
            WireBufferSlice {
                reserved: [0, 0, 0, 1],
                ..empty
            },
            WireBufferSlice {
                direction: io_queue::DIRECTION_DEVICE_READ,
                ..empty
            },
            WireBufferSlice {
                buffer: 17,
                lease: 23,
                length: 1,
                direction: io_queue::DIRECTION_DEVICE_READ,
                ..empty
            },
        ] {
            assert!(!valid(&request, &malformed), "{malformed:?}");
        }
    }
    assert!(!valid(&payload_request(network_service::OP_SEND), &empty));
    assert!(!valid(&payload_request(network_service::OP_RECV), &empty));
    assert!(!valid(&payload_request(network_service::OP_ATTACH), &empty));
}
