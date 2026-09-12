use slime_proto::{
    network_service::{self, WireLoanDelegation, WireNetworkCompletion, WireNetworkRequest},
    valid_loan_delegation, valid_network_completion, valid_network_request,
};

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

/// A delegation is a distinct record from a request: its own magic, so a
/// receiver tells the two apart by decoding rather than by length, and a
/// fixed 64-byte shape with every unnamed byte zero.
#[test]
fn a_loan_delegation_round_trips_and_is_told_apart_from_a_request() {
    let delegation = WireLoanDelegation {
        magic: network_service::DELEGATION_MAGIC,
        version: network_service::FORMAT_VERSION,
        kind: network_service::DELEGATION_DATA,
        reserved: 0,
        buffer: 7,
        lease: 11,
        padding: [0; 40],
    };
    assert!(valid_loan_delegation(&delegation));
    let encoded = delegation.encode();
    assert_eq!(encoded.len(), network_service::DELEGATION_BYTES);
    assert_eq!(WireLoanDelegation::decode(&encoded), Some(delegation));
    assert!(WireLoanDelegation::decode(&encoded[..63]).is_none());
    assert_ne!(
        network_service::DELEGATION_MAGIC,
        network_service::NETWORK_MAGIC
    );
    // A request's bytes never decode as a valid delegation, nor the reverse.
    let request = dns_request(
        network_service::OP_CONNECT,
        network_service::TRANSPORT_TCP,
        b"api.example",
        443,
    );
    let mut as_delegation = [0u8; network_service::DELEGATION_BYTES];
    as_delegation[..network_service::REQUEST_BYTES].copy_from_slice(&request.encode());
    assert!(!valid_loan_delegation(
        &WireLoanDelegation::decode(&as_delegation).unwrap()
    ));
    assert!(!WireNetworkRequest::decode(&encoded).is_some_and(|r| valid_network_request(&r)));

    for mutate in [
        (|d: &mut WireLoanDelegation| d.magic = network_service::NETWORK_MAGIC) as fn(&mut _),
        |d| d.version = 2,
        |d| d.kind = 0,
        |d| d.kind = 3,
        |d| d.reserved = 1,
        |d| d.padding[39] = 1,
    ] {
        let mut bad = delegation;
        mutate(&mut bad);
        assert!(!valid_loan_delegation(&bad));
    }
}
