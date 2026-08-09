#![allow(dead_code)]

use boot_contracts::fabric_graph::{
    CONTRACT_KIND_CALL, DIRECTION_CLIENT, DIRECTION_SERVER, route_identity,
};
use slime_proto::capability_transfer::{
    FABRIC_REQUEST_MAGIC, FORMAT_VERSION as TRANSFER_VERSION, OBJECT_KIND_ENDPOINT, REQUEST_LEN,
    WireCapabilityTransfer, WireFabricRequest,
};
use slime_proto::fabric_call::*;
use slime_proto::interface_schema::parameter_call;
use slime_proto::sample_descriptor::{
    CAPABILITY_KIND_LOAN, SAMPLE_DESCRIPTOR_MAGIC, WireSampleDescriptor,
};
use slime_rt::{
    ERR_BAD_CAP, ERR_PEER_DEAD, ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, WaitSource,
};

const CONTROL_SLOT: u32 = 0;
const FACTORY_SLOT: u32 = 1;
const FABRIC_SUPERVISION_SLOT: u32 = 2;
const ROUTE_NAME: &str = "parameters";
const RIGHT_SEND: u64 = 1;
const RIGHT_RECV: u64 = 2;
const PAGE: u64 = 4096;
const BASE: u64 = 0x7100_0000;
const CLIENT_PHASE_SLOT: u32 = 1;

pub fn run_client_b() {
    boot_park(DIRECTION_CLIENT, b"fabric-call-client-b");
    let route = request_role(DIRECTION_CLIENT);
    let session = client_session(1);
    send_call(
        route,
        envelope(
            session,
            21,
            KIND_REQUEST,
            FLAG_NON_IDEMPOTENT,
            STATUS_SUCCESS,
            1,
        ),
    );
    expect_reply(route, session, 21, STATUS_REJECTED);
    slime_rt::debug_write(b"[fabric-call-client-b] original request settled\n");
    send_call(
        route,
        envelope(
            session,
            21,
            KIND_REQUEST,
            FLAG_NON_IDEMPOTENT,
            STATUS_SUCCESS,
            1,
        ),
    );
    expect_terminal(route, session, 21, STATUS_DUPLICATE);
    slime_rt::debug_write(b"[fabric-call-client-b] duplicate rejected\n");

    send_call(
        route,
        envelope(session, 22, KIND_REQUEST, 0, STATUS_SUCCESS, 2),
    );
    send_call(
        route,
        envelope(session, 22, KIND_CANCEL, 0, STATUS_CANCELLED, 0),
    );
    expect_terminal(route, session, 22, STATUS_CANCELLED);
    slime_rt::debug_write(b"[fabric-call-client-b] cancellation observed\n");

    send_call(
        route,
        envelope(
            0x000e_0000_0000_0001,
            23,
            KIND_REQUEST,
            0,
            STATUS_SUCCESS,
            3,
        ),
    );
    expect_terminal(route, 0x000e_0000_0000_0001, 23, STATUS_STALE);
    slime_rt::debug_write(b"[fabric-call-client-b] stale session observed\n");

    wait_client_phase(0);
    for request_id in 100..124 {
        send_call(
            route,
            envelope(
                0x000e_0000_0000_0001,
                request_id,
                KIND_REQUEST,
                0,
                STATUS_SUCCESS,
                request_id,
            ),
        );
    }
    for _ in 0..128 {
        slime_rt::yield_now();
    }
    for request_id in 100..124 {
        expect_terminal(route, 0x000e_0000_0000_0001, request_id, STATUS_STALE);
    }
    slime_rt::debug_write(b"[fabric-call-client-b] terminal backpressure recovered\n");
    signal_client_phase(0);

    slime_rt::debug_write(b"[fabric-call-client-b] unrelated route intact\n");
}

pub fn run_server() {
    boot_park(DIRECTION_SERVER, b"fabric-call-server");
    let route = request_role(DIRECTION_SERVER);
    let mut executed_non_idempotent = false;
    loop {
        let mut bytes = [0u8; MAX_MSG];
        let mut caps = [0u64; MAX_CAPS_PER_MSG];
        let length = match slime_rt::recv(route, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => {
                slime_rt::wait(&[WaitSource::Endpoint(route)]);
                continue;
            }
            ERR_PEER_DEAD => return,
            value if value < 0 => fail(b"server receive"),
            value => value as usize,
        };
        if length != MAX_MSG {
            fail(b"server record length")
        }
        let magic = u32::from_le_bytes(bytes[..4].try_into().expect("record prefix"));
        match magic {
            CALL_MAGIC => {
                release_caps(&caps);
                let request =
                    WireCallEnvelope::decode(&bytes).unwrap_or_else(|| fail(b"server decode"));
                if !matches!(request.kind, KIND_REQUEST | KIND_CANCEL)
                    || request.session != 0x000e_0000_0000_0001
                {
                    fail(b"server received invalid call record")
                }
                if request.kind == KIND_CANCEL {
                    send_call(
                        route,
                        envelope(
                            request.session,
                            request.request_id,
                            KIND_REPLY,
                            0,
                            STATUS_CANCELLED,
                            0,
                        ),
                    );
                    slime_rt::debug_write(b"[fabric-call-server] cancellation settled\n");
                    continue;
                }
                if request.payload_len == 8
                    && u64::from_le_bytes(request.payload[..8].try_into().expect("request payload"))
                        == 11
                {
                    slime_rt::debug_write(b"[fabric-call-server] injected peer death\n");
                    slime_rt::exit(0)
                }
                if let Some(reply) = handle_inline(request, &mut executed_non_idempotent) {
                    send_call(route, reply);
                }
            }
            SAMPLE_DESCRIPTOR_MAGIC => {
                let loan_slot = caps[0] as u32;
                let descriptor = WireSampleDescriptor::decode(&bytes)
                    .unwrap_or_else(|| fail(b"shared request decode"));
                if loan_slot == 0
                    || !slime_proto::valid_sample_descriptor(
                        &descriptor,
                        descriptor.loan_id,
                        parameter_call::TYPE_TAG,
                        PAGE,
                    )
                {
                    fail(b"shared request invalid")
                }
                verify_large(loan_slot, &descriptor);
                send_large_reply(route, descriptor.sequence);
                slime_rt::debug_write(b"[fabric-call-server] shared request verified\n");
            }
            _ => fail(b"unknown server record"),
        }
    }
}

fn handle_inline(
    request: WireCallEnvelope,
    executed_non_idempotent: &mut bool,
) -> Option<WireCallEnvelope> {
    let value = if request.payload_len == 8 {
        u64::from_le_bytes(request.payload[..8].try_into().expect("request payload"))
    } else {
        0
    };
    match value {
        7 => {
            if *executed_non_idempotent {
                fail(b"non-idempotent request executed twice")
            }
            *executed_non_idempotent = true;
            slime_rt::debug_write(b"[fabric-call-server] non-idempotent execution once\n");
            Some(envelope(
                request.session,
                request.request_id,
                KIND_REPLY,
                request.flags,
                STATUS_SUCCESS,
                11,
            ))
        }
        8 | 1 | 10 => Some(envelope(
            request.session,
            request.request_id,
            KIND_REPLY,
            0,
            STATUS_REJECTED,
            0,
        )),
        9 => {
            let mut malformed = envelope(
                request.session,
                request.request_id,
                KIND_REPLY,
                0,
                STATUS_SUCCESS,
                3,
            );
            malformed.type_identity ^= 1;
            Some(malformed)
        }
        5 | 11 | 106 | 107 | 108 | 109 => None,
        _ => Some(envelope(
            request.session,
            request.request_id,
            KIND_REPLY,
            0,
            STATUS_REJECTED,
            0,
        )),
    }
}

/// C8.10 full-graph boot arm: take the declared call role, then park forever.
///
/// A no-op outside the boot generation, so a caller writes `boot_park(..)` as
/// the first line of its scenario. The boot gate asserts a provisioned graph at
/// rest with no traffic; the call scenario's own correlation, duplicate,
/// timeout, and peer-death arms stay `just fabric_call_check`'s to prove.
pub fn boot_park(direction: u32, name: &'static [u8]) {
    if !slime_components::fabric_boot::active() {
        return;
    }
    // `request_role` already verifies the descriptor names this exact
    // (route, direction) edge and carries no more rights than declared, so the
    // marker below reports a checked role rather than merely a received one.
    let _route = request_role(direction);
    slime_rt::debug_write(b"[");
    slime_rt::debug_write(name);
    slime_rt::debug_write(b"] boot role provisioned\n");
    slime_components::fabric_boot::park(name)
}

pub fn request_role(direction: u32) -> u32 {
    let route = route_identity(
        ROUTE_NAME,
        &parameter_call::INTERFACE_IDENTITY,
        CONTRACT_KIND_CALL,
    );
    let mut route_name = [0u8; 32];
    route_name[..ROUTE_NAME.len()].copy_from_slice(ROUTE_NAME.as_bytes());
    let request = WireFabricRequest {
        magic: FABRIC_REQUEST_MAGIC,
        version: TRANSFER_VERSION,
        flags: 0,
        direction,
        type_identity: parameter_call::TYPE_TAG,
        route_name_len: ROUTE_NAME.len() as u32,
        route_name,
        reserved: [0; 4],
    };
    send_raw(CONTROL_SLOT, &request.encode());
    let mut message = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(CONTROL_SLOT, &mut message, &mut caps) {
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(CONTROL_SLOT)]),
            value if value < 0 => fail(b"role receive"),
            value => {
                if value as usize != MAX_MSG || caps[0] == 0 {
                    fail(b"role shape")
                }
                let descriptor = WireCapabilityTransfer::decode(&message)
                    .unwrap_or_else(|| fail(b"role decode"));
                if !slime_proto::valid_capability_transfer(
                    &descriptor,
                    &route,
                    direction,
                    OBJECT_KIND_ENDPOINT,
                ) || descriptor.rights_mask != RIGHT_SEND | RIGHT_RECV
                {
                    fail(b"role authority")
                }
                let slot = caps[0] as u32;
                let mut discard = [0u8; MAX_MSG];
                let mut no_caps = [0u64; MAX_CAPS_PER_MSG];
                if slime_rt::recv(slot, &mut discard, &mut no_caps) == ERR_BAD_CAP
                    || slime_rt::send(slot, b"probe", &[]) == ERR_BAD_CAP
                {
                    fail(b"call role missing one direction")
                }
                return slot;
            }
        }
    }
}

pub fn client_session(index: usize) -> u64 {
    0x00c1_0000_0000_0001 + index as u64 * 0x0001_0000_0000_0000
}

pub fn envelope(
    session: u64,
    request_id: u64,
    kind: u32,
    flags: u32,
    status: i32,
    value: u64,
) -> WireCallEnvelope {
    let mut payload = [0u8; 16];
    let payload_len = if value == 0 { 0 } else { 8 };
    payload[..8].copy_from_slice(&value.to_le_bytes());
    WireCallEnvelope {
        magic: CALL_MAGIC,
        version: FORMAT_VERSION,
        kind,
        flags,
        session,
        request_id,
        type_identity: parameter_call::TYPE_TAG,
        status,
        payload_len,
        payload,
    }
}

pub fn send_call(slot: u32, message: WireCallEnvelope) {
    send_raw(slot, &message.encode())
}

pub fn send_time(slot: u32, now_ns: u64) {
    let value = WireCallTimeAdvance {
        magic: CALL_TIME_MAGIC,
        version: FORMAT_VERSION,
        flags: 0,
        reserved0: 0,
        now_ns,
        reserved: [0; 40],
    };
    send_raw(slot, &value.encode());
}

pub fn send_large_request(route: u32, request_id: u64) {
    let buffer = slime_rt::shared_buffer_create(FACTORY_SLOT, 1, true)
        .unwrap_or_else(|_| fail(b"large create"));
    if slime_rt::shared_buffer_map(buffer.slot, BASE, 0, PAGE, true) != ERR_SUCCESS {
        fail(b"large map")
    }
    unsafe {
        for index in 0..PAGE as usize {
            (BASE as *mut u8)
                .add(index)
                .write_volatile((index % 251) as u8);
        }
    }
    if slime_rt::shared_buffer_seal(buffer.slot) != ERR_SUCCESS {
        fail(b"large seal")
    }
    let loan = slime_rt::shared_buffer_loan(buffer.slot, FABRIC_SUPERVISION_SLOT, 0, PAGE)
        .unwrap_or_else(|_| fail(b"large loan"));
    let descriptor = WireSampleDescriptor {
        magic: SAMPLE_DESCRIPTOR_MAGIC,
        version: slime_proto::sample_descriptor::FORMAT_VERSION,
        flags: 0,
        capability_kind: CAPABILITY_KIND_LOAN,
        loan_id: loan.id,
        offset: 0,
        length: PAGE,
        type_identity: parameter_call::TYPE_TAG,
        sequence: request_id,
        reserved: [0; 8],
    };
    send_with_cap(route, &descriptor.encode(), loan.slot);
    if slime_rt::shared_buffer_unmap(buffer.slot, BASE) != ERR_SUCCESS
        || slime_rt::shared_buffer_release(buffer.slot) != ERR_SUCCESS
    {
        fail(b"large reclaim")
    }
}

fn send_large_reply(route: u32, request_id: u64) {
    let buffer = slime_rt::shared_buffer_create(FACTORY_SLOT, 1, true)
        .unwrap_or_else(|_| fail(b"reply create"));
    if slime_rt::shared_buffer_map(buffer.slot, BASE, 0, PAGE, true) != ERR_SUCCESS {
        fail(b"reply map")
    }
    unsafe {
        for index in 0..PAGE as usize {
            (BASE as *mut u8)
                .add(index)
                .write_volatile((index % 239) as u8);
        }
    }
    if slime_rt::shared_buffer_seal(buffer.slot) != ERR_SUCCESS {
        fail(b"reply seal")
    }
    let loan = slime_rt::shared_buffer_loan(buffer.slot, FABRIC_SUPERVISION_SLOT, 0, PAGE)
        .unwrap_or_else(|_| fail(b"reply loan"));
    let descriptor = WireSampleDescriptor {
        magic: SAMPLE_DESCRIPTOR_MAGIC,
        version: slime_proto::sample_descriptor::FORMAT_VERSION,
        flags: 0,
        capability_kind: CAPABILITY_KIND_LOAN,
        loan_id: loan.id,
        offset: 0,
        length: PAGE,
        type_identity: parameter_call::TYPE_TAG,
        sequence: request_id,
        reserved: [0; 8],
    };
    send_with_cap(route, &descriptor.encode(), loan.slot);
    let _ = slime_rt::shared_buffer_unmap(buffer.slot, BASE);
    let _ = slime_rt::shared_buffer_release(buffer.slot);
}

pub fn expect_large_reply(slot: u32, request_id: u64) {
    let (descriptor, loan_slot) = recv_descriptor(slot);
    if descriptor.sequence != request_id {
        fail(b"large reply correlation")
    }
    if slime_rt::shared_buffer_loan_map(loan_slot, BASE, 0, descriptor.length) != ERR_SUCCESS {
        fail(b"large reply map")
    }
    let mismatch = unsafe {
        (0..descriptor.length as usize)
            .find(|index| (BASE as *const u8).add(*index).read_volatile() != (*index % 239) as u8)
    };
    if mismatch.is_some() {
        fail(b"large reply payload")
    }
    if slime_rt::shared_buffer_unmap(loan_slot, BASE) != ERR_SUCCESS
        || slime_rt::shared_buffer_return(loan_slot) != ERR_SUCCESS
    {
        fail(b"large reply return")
    }
}

fn verify_large(loan_slot: u32, descriptor: &WireSampleDescriptor) {
    if slime_rt::shared_buffer_loan_map(loan_slot, BASE, 0, descriptor.length) != ERR_SUCCESS {
        fail(b"shared request map")
    }
    let mismatch = unsafe {
        (0..descriptor.length as usize)
            .find(|index| (BASE as *const u8).add(*index).read_volatile() != (*index % 251) as u8)
    };
    if mismatch.is_some() {
        fail(b"shared request payload")
    }
    if slime_rt::shared_buffer_unmap(loan_slot, BASE) != ERR_SUCCESS
        || slime_rt::shared_buffer_return(loan_slot) != ERR_SUCCESS
    {
        fail(b"shared request return")
    }
}

pub fn recv_call(slot: u32) -> WireCallEnvelope {
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(slot, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(slot)]),
            ERR_PEER_DEAD => fail(b"call peer died"),
            value if value < 0 => fail(b"call receive"),
            value => {
                release_caps(&caps);
                if value as usize != MAX_MSG {
                    fail(b"call length")
                }
                return WireCallEnvelope::decode(&bytes).unwrap_or_else(|| fail(b"call decode"));
            }
        }
    }
}

fn recv_descriptor(slot: u32) -> (WireSampleDescriptor, u32) {
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(slot, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(slot)]),
            value if value < 0 => fail(b"descriptor receive"),
            value => {
                if value as usize != MAX_MSG || caps[0] == 0 {
                    fail(b"descriptor shape")
                }
                let descriptor = WireSampleDescriptor::decode(&bytes)
                    .unwrap_or_else(|| fail(b"descriptor decode"));
                if !slime_proto::valid_sample_descriptor(
                    &descriptor,
                    descriptor.loan_id,
                    parameter_call::TYPE_TAG,
                    PAGE,
                ) {
                    fail(b"descriptor invalid")
                }
                return (descriptor, caps[0] as u32);
            }
        }
    }
}

pub fn expect_reply(slot: u32, session: u64, request_id: u64, status: i32) -> WireCallEnvelope {
    let reply = recv_call(slot);
    if !slime_proto::valid_call_envelope(&reply, parameter_call::TYPE_TAG)
        || reply.session != session
        || reply.request_id != request_id
        || reply.kind != KIND_REPLY
        || reply.status != status
    {
        fail(b"reply mismatch")
    }
    reply
}

pub fn expect_terminal_parked(slot: u32, session: u64, request_id: u64, status: i32) {
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(slot, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(slot)]),
            ERR_PEER_DEAD => fail(b"terminal peer died"),
            value if value < 0 => fail(b"terminal receive"),
            value => {
                release_caps(&caps);
                if value as usize != MAX_MSG {
                    fail(b"terminal length")
                }
                let terminal =
                    WireCallEnvelope::decode(&bytes).unwrap_or_else(|| fail(b"terminal decode"));
                if !slime_proto::valid_call_envelope(&terminal, parameter_call::TYPE_TAG)
                    || terminal.session != session
                    || terminal.request_id != request_id
                    || terminal.kind != KIND_TERMINAL
                    || terminal.status != status
                {
                    fail(b"terminal mismatch")
                }
                return;
            }
        }
    }
}

pub fn expect_terminal(slot: u32, session: u64, request_id: u64, status: i32) {
    let terminal = recv_call(slot);
    if !slime_proto::valid_call_envelope(&terminal, parameter_call::TYPE_TAG)
        || terminal.session != session
        || terminal.request_id != request_id
        || terminal.kind != KIND_TERMINAL
        || terminal.status != status
    {
        fail(b"terminal mismatch")
    }
}

fn signal_client_phase(phase: u8) {
    loop {
        match slime_rt::send(CLIENT_PHASE_SLOT, &[phase], &[]) {
            ERR_SUCCESS => return,
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            _ => fail(b"client phase send"),
        }
    }
}

fn wait_client_phase(expected: u8) {
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(CLIENT_PHASE_SLOT, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(CLIENT_PHASE_SLOT)]),
            value if value < 0 => fail(b"client phase receive"),
            1 if bytes[0] == expected => return,
            _ => fail(b"client phase mismatch"),
        }
    }
}

fn send_raw(slot: u32, bytes: &[u8; MAX_MSG]) {
    loop {
        match slime_rt::send(slot, bytes, &[]) {
            ERR_SUCCESS => return,
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            _ => fail(b"call send"),
        }
    }
}

fn send_with_cap(slot: u32, bytes: &[u8; MAX_MSG], cap: u32) {
    loop {
        match slime_rt::send(slot, bytes, &[cap]) {
            ERR_SUCCESS => return,
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            _ => fail(b"call capability send"),
        }
    }
}

fn release_caps(caps: &[u64; MAX_CAPS_PER_MSG]) {
    for cap in caps.iter().filter(|cap| **cap != 0) {
        let _ = slime_rt::cap_drop(*cap as u32);
    }
}

pub fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[fabric-call] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

const _: () = assert!(REQUEST_LEN == MAX_MSG);
const _: () = assert!(CALL_LEN == MAX_MSG);
const _: () = assert!(CALL_TIME_LEN == MAX_MSG);
