//! Shared participant behaviour for the C8.7 operation gate.
//!
//! Two clients and one server drive every required arm of the milestone over
//! real syscalls: concurrent non-cross-correlated operations, unauthorized
//! observation/retrieval/cancellation, duplicate goals, feedback after terminal
//! state, duplicate results, a cancellation race, result expiry, participant
//! restart, and peer death while a distinct operation route remains live.

#![allow(dead_code)]

use boot_contracts::fabric_graph::{
    CONTRACT_KIND_OPERATION, DIRECTION_CLIENT, DIRECTION_SERVER, route_identity,
};
use slime_proto::capability_transfer::{
    FABRIC_REQUEST_MAGIC, FORMAT_VERSION as TRANSFER_VERSION, OBJECT_KIND_ENDPOINT, REQUEST_LEN,
    WireCapabilityTransfer, WireFabricRequest,
};
use slime_proto::fabric_operation::*;
use slime_proto::fabric_time::{TIME_ADVANCE_LEN, TIME_ADVANCE_MAGIC, WireTimeAdvance};
use slime_proto::interface_schema::navigation_operation;
use slime_rt::{
    ERR_BAD_CAP, ERR_PEER_DEAD, ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, WaitSource,
};

/// Slot layout every operation participant is spawned with. Slot 0 is always the
/// fabric control endpoint; the phase channels exist only to order the
/// transcript, and carry one byte each.
const CONTROL_SLOT: u32 = 0;
/// The private client-A/client-B coordination channel. Both hold it, so either
/// may signal or wait. The restarted client receives this endpoint at slot 1.
const PHASE_SLOT: u32 = 1;
const RESTART_START_SLOT: u32 = 2;
/// Client A's send half of the channel that releases the time service. Kept
/// separate from the A/B channel so a phase meant for the clock can never be
/// consumed by client B, which would deadlock both.
const PHASE_TIME_SLOT: u32 = 2;
const ROUTE_NAME: &str = "navigation";
const BACKUP_ROUTE_NAME: &str = "navigation-backup";
const RIGHT_SEND: u64 = 1;
const RIGHT_RECV: u64 = 2;

/// Goal payload values, which the server reads as its own policy input. The
/// fabric never interprets these: goal policy is the application's.
const GOAL_COMPLETES: u64 = 1;
const GOAL_REJECTED: u64 = 2;
const GOAL_FEEDBACK_THEN_RESULT: u64 = 3;
const GOAL_CANCELLABLE: u64 = 4;
const GOAL_DUPLICATE_RESULT: u64 = 5;
const GOAL_FEEDBACK_AFTER_TERMINAL: u64 = 6;
const GOAL_NEVER_ANSWERS: u64 = 7;
const GOAL_KILLS_SERVER: u64 = 8;

/// Client A: the correlation, feedback, result, retrieval, expiry, and
/// peer-death arms.
pub fn run_client() {
    let roles = request_roles(DIRECTION_CLIENT, 2);
    let route = roles[0];
    let backup_route = roles[1];
    let session = client_session(0);

    // Happy path: goal accepted, result delivered, exactly one terminal.
    send(route, goal(session, 1, GOAL_COMPLETES));
    expect_accepted(route, session, 1, STATUS_ACTIVE);
    let result = expect_result(route, session, 1, STATUS_SUCCESS);
    if u64::from_le_bytes(result.payload[..8].try_into().expect("result payload")) != 11 {
        fail(b"result payload")
    }
    expect_terminal(route, session, 1, STATUS_SUCCESS);
    slime_rt::debug_write(b"[fabric-op-client] success correlated\n");

    // Feedback is ordered, keyed to this operation, and precedes the result.
    send(route, goal(session, 2, GOAL_FEEDBACK_THEN_RESULT));
    expect_accepted(route, session, 2, STATUS_ACTIVE);
    for sequence in 1..=3 {
        let feedback = expect_feedback(route, session, 2);
        if feedback.sequence != sequence {
            fail(b"feedback order")
        }
    }
    expect_result(route, session, 2, STATUS_SUCCESS);
    expect_terminal(route, session, 2, STATUS_SUCCESS);
    slime_rt::debug_write(b"[fabric-op-client] feedback ordered\n");

    // A server-rejected goal is distinct from every other outcome.
    send(route, goal(session, 3, GOAL_REJECTED));
    expect_terminal(route, session, 3, STATUS_REJECTED);
    slime_rt::debug_write(b"[fabric-op-client] rejection distinct\n");

    // Feedback sent after the terminal result never reaches the client: the
    // operation is over, so progress afterwards is not observable.
    send(route, goal(session, 4, GOAL_FEEDBACK_AFTER_TERMINAL));
    expect_accepted(route, session, 4, STATUS_ACTIVE);
    expect_result(route, session, 4, STATUS_SUCCESS);
    expect_terminal(route, session, 4, STATUS_SUCCESS);
    slime_rt::debug_write(b"[fabric-op-client] terminal state closed\n");

    // A duplicate goal identity is refused rather than starting the work twice.
    send(route, goal(session, 4, GOAL_COMPLETES));
    expect_terminal(route, session, 4, STATUS_DUPLICATE);
    slime_rt::debug_write(b"[fabric-op-client] duplicate goal rejected\n");

    // A second result for one operation is dropped: one terminal per operation.
    send(route, goal(session, 5, GOAL_DUPLICATE_RESULT));
    expect_accepted(route, session, 5, STATUS_ACTIVE);
    expect_result(route, session, 5, STATUS_SUCCESS);
    expect_terminal(route, session, 5, STATUS_SUCCESS);
    slime_rt::debug_write(b"[fabric-op-client] single terminal enforced\n");

    // Retained retrieval: the result is claimable once after it was pushed.
    send(route, result_request(session, 5));
    expect_result(route, session, 5, STATUS_SUCCESS);
    slime_rt::debug_write(b"[fabric-op-client] result retrieved\n");
    // And only once — a second retrieval finds nothing to claim.
    send(route, result_request(session, 5));
    expect_terminal(route, session, 5, STATUS_STALE);
    slime_rt::debug_write(b"[fabric-op-client] retained result claimed once\n");

    // Client B now runs its authority-denial and cancellation arms.
    signal_phase(1);
    wait_phase(3);

    // Result expiry, driven only by the explicit time capability. Operation 6
    // completes and is retained, then the clock passes its retention window.
    // The original operation still has one terminal; a later result-call gets
    // the distinct transport-level expired outcome declared by the contract.
    send(route, goal(session, 6, GOAL_COMPLETES));
    expect_accepted(route, session, 6, STATUS_ACTIVE);
    expect_result(route, session, 6, STATUS_SUCCESS);
    expect_terminal(route, session, 6, STATUS_SUCCESS);
    // Admit the never-answered goal before the clock advances. One explicit
    // phase then crosses both its deadline and result 6's retention window.
    send(route, goal(session, 7, GOAL_NEVER_ANSWERS));
    expect_accepted(route, session, 7, STATUS_ACTIVE);
    signal_time_phase(3);
    expect_terminal_yielding(route, session, 7, STATUS_TIMEOUT);
    slime_rt::debug_write(b"[fabric-op-client] timeout distinct\n");
    // The retained identity remains as a bounded tombstone, so expiry is
    // distinguishable from an unknown or unauthorized operation.
    send(route, result_request(session, 6));
    expect_terminal(route, session, 6, STATUS_EXPIRED);
    slime_rt::debug_write(b"[fabric-op-client] result expiry observed\n");

    // Let client B arm a live operation first, so the server fault below lands
    // while B has real in-flight state on the same route. That is what makes B's
    // survival an observation rather than a race against broker shutdown.
    signal_phase(5);
    wait_phase(6);

    // Peer death settles the active operation and is distinct from a timeout.
    send(route, goal(session, 8, GOAL_KILLS_SERVER));
    expect_terminal_yielding(route, session, 8, STATUS_PEER_DEAD);
    slime_rt::debug_write(b"[fabric-op-client] peer death distinct\n");
    send_raw(backup_route, &[0xa7]);
    let mut probe = [0u8; MAX_MSG];
    let mut probe_caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(backup_route, &mut probe, &mut probe_caps) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            1 if probe[0] == 0xa7 => break,
            _ => fail(b"backup operation liveness"),
        }
    }
    slime_rt::debug_write(b"[fabric-op-client] unrelated operation route live\n");
}

/// Client B: the authority arms. Everything here is a denial that must hold even
/// though B knows the exact operation identity A used.
pub fn run_client_b() {
    let route = request_role(DIRECTION_CLIENT);
    let session = client_session(1);

    // A concurrent operation under B's own authority. Its feedback and result
    // must never carry A's identity, and vice versa.
    send(route, goal(session, 1, GOAL_FEEDBACK_THEN_RESULT));
    expect_accepted(route, session, 1, STATUS_ACTIVE);
    for sequence in 1..=3 {
        let feedback = expect_feedback(route, session, 1);
        if feedback.sequence != sequence || feedback.session != session {
            fail(b"cross-correlated feedback")
        }
    }
    expect_result(route, session, 1, STATUS_SUCCESS);
    expect_terminal(route, session, 1, STATUS_SUCCESS);
    slime_rt::debug_write(b"[fabric-op-client-b] concurrent operation isolated\n");

    wait_phase(1);

    // B knows A used operation id 5 and that its result was retained. Naming it
    // is refused exactly like naming an id that never existed: knowledge of an
    // identity is not authority over it.
    send(route, result_request(session, 5));
    expect_terminal(route, session, 5, STATUS_STALE);
    slime_rt::debug_write(b"[fabric-op-client-b] unauthorized retrieval denied\n");

    // Same for cancellation of an operation B does not own.
    send(route, cancel(session, 2));
    expect_terminal(route, session, 2, STATUS_STALE);
    slime_rt::debug_write(b"[fabric-op-client-b] unauthorized cancel denied\n");

    // A client may not emit transport records. Claiming to be the transport on
    // its own endpoint is refused.
    let mut forged = goal(session, 9, GOAL_COMPLETES);
    forged.kind = KIND_FEEDBACK;
    forged.status = STATUS_ACTIVE;
    forged.sequence = 1;
    send(route, forged);
    expect_terminal(route, session, 9, STATUS_STALE);
    slime_rt::debug_write(b"[fabric-op-client-b] forged transport record denied\n");

    // Produce one retained result, then exit. Init restarts this participant
    // with the same authenticated control channel and a fresh primary route;
    // the replacement proves retained correlation and replay suppression.
    send(route, goal(session, 9, GOAL_COMPLETES));
    expect_accepted(route, session, 9, STATUS_ACTIVE);
    expect_result(route, session, 9, STATUS_SUCCESS);
    expect_terminal(route, session, 9, STATUS_SUCCESS);
    slime_rt::debug_write(b"[fabric-op-client-b] restart state retained\n");
    slime_rt::exit(0);
}

/// Client B after supervised restart. The broker keeps the authenticated client
/// index and high-water mark, but supplies a fresh non-delegable role endpoint.
pub fn run_client_b_restarted() {
    wait_restart_start();
    let route = request_role(DIRECTION_CLIENT);
    let session = client_session(1);
    send(route, result_request(session, 9));
    expect_result(route, session, 9, STATUS_SUCCESS);
    send(route, goal(session, 9, GOAL_COMPLETES));
    expect_terminal(route, session, 9, STATUS_DUPLICATE);
    slime_rt::debug_write(b"[fabric-op-client-b] participant restart deterministic\n");

    // Continue the original client-B arms after restart.
    send(route, goal(session, 10, GOAL_CANCELLABLE));
    expect_accepted(route, session, 10, STATUS_ACTIVE);
    send(route, cancel(session, 10));
    expect_accepted(route, session, 10, STATUS_CANCEL_REQUESTED);
    send(route, cancel(session, 10));
    expect_accepted(route, session, 10, STATUS_CANCEL_REQUESTED);
    expect_result(route, session, 10, STATUS_CANCELLED);
    expect_terminal(route, session, 10, STATUS_CANCELLED);
    slime_rt::debug_write(b"[fabric-op-client-b] cancellation settled once\n");
    signal_phase(3);
    wait_phase(5);
    send(route, goal(session, 13, GOAL_NEVER_ANSWERS));
    expect_accepted(route, session, 13, STATUS_ACTIVE);
    signal_phase(6);
    expect_terminal_yielding(route, session, 13, STATUS_PEER_DEAD);
    signal_phase(7);
    slime_rt::debug_write(b"[fabric-op-client-b] concurrent peer fault isolated\n");
}

/// The operation server. Its policy is deliberately trivial and lives entirely
/// here: the fabric composes transport, the server decides outcomes.
pub fn run_server() {
    let route = request_role(DIRECTION_SERVER);
    let mut executed = [false; 16];
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
        release_caps(&caps);
        if length != MAX_MSG {
            fail(b"server record length")
        }
        let record =
            WireOperationEnvelope::decode(&bytes).unwrap_or_else(|| fail(b"server decode"));
        if !slime_proto::valid_operation_envelope(&record, navigation_operation::TYPE_TAG) {
            fail(b"server received invalid operation record")
        }
        match record.kind {
            KIND_GOAL => handle_goal(route, record, &mut executed),
            // A cancel request: the server accepts and reports CANCELLED as its
            // own result, which is what keeps goal policy out of the fabric.
            KIND_CANCEL => {
                send(route, result(record, STATUS_CANCELLED, 0));
                slime_rt::debug_write(b"[fabric-op-server] cancellation honoured\n");
            }
            _ => fail(b"unknown server record"),
        }
    }
}

fn handle_goal(route: u32, record: WireOperationEnvelope, executed: &mut [bool; 16]) {
    let value = u64::from_le_bytes(record.payload[..8].try_into().expect("goal payload"));
    // One execution per transport-correlated goal. The fabric suppresses
    // duplicates upstream, so a second execution here would be a real defect.
    let key = (record.operation_id as usize) % executed.len();
    if executed[key] {
        fail(b"goal executed twice")
    }
    executed[key] = true;
    match value {
        GOAL_COMPLETES => {
            send(route, accepted(record, STATUS_SUCCESS));
            send(route, result(record, STATUS_SUCCESS, 11));
        }
        GOAL_REJECTED => {
            send(route, accepted(record, STATUS_REJECTED));
            slime_rt::debug_write(b"[fabric-op-server] goal rejected\n");
        }
        GOAL_FEEDBACK_THEN_RESULT => {
            send(route, accepted(record, STATUS_SUCCESS));
            for sequence in 1..=3 {
                send(route, feedback(record, sequence));
            }
            send(route, result(record, STATUS_SUCCESS, 11));
            slime_rt::debug_write(b"[fabric-op-server] feedback streamed\n");
        }
        GOAL_CANCELLABLE => {
            send(route, accepted(record, STATUS_SUCCESS));
            slime_rt::debug_write(b"[fabric-op-server] awaiting cancellation\n");
        }
        GOAL_DUPLICATE_RESULT => {
            send(route, accepted(record, STATUS_SUCCESS));
            send(route, result(record, STATUS_SUCCESS, 11));
            // The second result must be dropped by the fabric rather than
            // producing a second terminal.
            send(route, result(record, STATUS_SUCCESS, 12));
            slime_rt::debug_write(b"[fabric-op-server] duplicate result emitted\n");
        }
        GOAL_FEEDBACK_AFTER_TERMINAL => {
            send(route, accepted(record, STATUS_SUCCESS));
            send(route, result(record, STATUS_SUCCESS, 11));
            // Feedback after the operation is over: the fabric must drop it.
            send(route, feedback(record, 1));
            slime_rt::debug_write(b"[fabric-op-server] post-terminal feedback emitted\n");
        }
        GOAL_NEVER_ANSWERS => {
            send(route, accepted(record, STATUS_SUCCESS));
            slime_rt::debug_write(b"[fabric-op-server] goal left unanswered\n");
        }
        GOAL_KILLS_SERVER => {
            slime_rt::debug_write(b"[fabric-op-server] injected peer death\n");
            slime_rt::exit(0)
        }
        _ => send(route, accepted(record, STATUS_REJECTED)),
    }
}

/// Ask the fabric for this component's declared role and verify what arrives.
///
/// The request's route name, direction, and type identity grant nothing: the
/// fabric authenticates by the control endpoint and answers from the graph. What
/// is checked here is the other half — that the capability received names
/// exactly the edge this component was provisioned for, and carries both
/// directions of its own role and nothing more.
pub fn request_role(direction: u32) -> u32 {
    request_roles(direction, 1)[0]
}

fn request_roles(direction: u32, count: usize) -> [u32; 2] {
    let route = route_identity(
        ROUTE_NAME,
        &navigation_operation::INTERFACE_IDENTITY,
        CONTRACT_KIND_OPERATION,
    );
    let backup_route = route_identity(
        BACKUP_ROUTE_NAME,
        &navigation_operation::INTERFACE_IDENTITY,
        CONTRACT_KIND_OPERATION,
    );
    let mut route_name = [0u8; 32];
    route_name[..ROUTE_NAME.len()].copy_from_slice(ROUTE_NAME.as_bytes());
    let request = WireFabricRequest {
        magic: FABRIC_REQUEST_MAGIC,
        version: TRANSFER_VERSION,
        flags: 0,
        direction,
        type_identity: navigation_operation::TYPE_TAG,
        route_name_len: ROUTE_NAME.len() as u32,
        route_name,
        reserved: [0; 4],
    };
    send_raw(CONTROL_SLOT, &request.encode());
    let mut roles = [0u32; 2];
    for (index, role) in roles.iter_mut().enumerate().take(count) {
        let expected_route = if index == 0 { &route } else { &backup_route };
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
                        expected_route,
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
                        fail(b"operation role missing one direction")
                    }
                    *role = slot;
                    break;
                }
            }
        }
    }
    roles
}

pub fn client_session(index: usize) -> u64 {
    0x00c2_0000_0000_0001 + index as u64 * 0x0001_0000_0000_0000
}

fn envelope(
    session: u64,
    operation_id: u64,
    kind: u32,
    status: i32,
    sequence: u32,
    value: u64,
) -> WireOperationEnvelope {
    let mut payload = [0u8; 16];
    let payload_len = if value == 0 { 0 } else { 8 };
    payload[..8].copy_from_slice(&value.to_le_bytes());
    WireOperationEnvelope {
        magic: OPERATION_MAGIC,
        version: FORMAT_VERSION,
        kind,
        status,
        session,
        operation_id,
        type_identity: navigation_operation::TYPE_TAG,
        sequence,
        payload_len,
        payload,
    }
}

pub fn goal(session: u64, operation_id: u64, value: u64) -> WireOperationEnvelope {
    envelope(session, operation_id, KIND_GOAL, STATUS_SUCCESS, 0, value)
}

pub fn cancel(session: u64, operation_id: u64) -> WireOperationEnvelope {
    envelope(session, operation_id, KIND_CANCEL, STATUS_SUCCESS, 0, 0)
}

pub fn result_request(session: u64, operation_id: u64) -> WireOperationEnvelope {
    envelope(
        session,
        operation_id,
        KIND_RESULT_REQUEST,
        STATUS_SUCCESS,
        0,
        0,
    )
}

fn accepted(request: WireOperationEnvelope, status: i32) -> WireOperationEnvelope {
    envelope(
        request.session,
        request.operation_id,
        KIND_ACCEPTED,
        status,
        0,
        0,
    )
}

fn feedback(request: WireOperationEnvelope, sequence: u32) -> WireOperationEnvelope {
    envelope(
        request.session,
        request.operation_id,
        KIND_FEEDBACK,
        STATUS_ACTIVE,
        sequence,
        sequence as u64,
    )
}

fn result(request: WireOperationEnvelope, status: i32, value: u64) -> WireOperationEnvelope {
    envelope(
        request.session,
        request.operation_id,
        KIND_RESULT,
        status,
        0,
        value,
    )
}

pub fn send(slot: u32, record: WireOperationEnvelope) {
    send_raw(slot, &record.encode())
}

pub fn send_time(slot: u32, now_ns: u64) {
    let value = WireTimeAdvance {
        magic: TIME_ADVANCE_MAGIC,
        version: slime_proto::fabric_time::FORMAT_VERSION,
        flags: 0,
        reserved0: 0,
        now_ns,
        reserved: [0; 40],
    };
    send_raw(slot, &value.encode());
}

fn recv_record(slot: u32) -> WireOperationEnvelope {
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(slot, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(slot)]),
            ERR_PEER_DEAD => fail(b"operation peer died"),
            value if value < 0 => fail(b"operation receive"),
            value => {
                release_caps(&caps);
                if value as usize != MAX_MSG {
                    fail(b"operation length")
                }
                let record = WireOperationEnvelope::decode(&bytes)
                    .unwrap_or_else(|| fail(b"operation decode"));
                if !slime_proto::valid_operation_envelope(&record, navigation_operation::TYPE_TAG) {
                    fail(b"operation record invalid")
                }
                return record;
            }
        }
    }
}

fn expect(
    slot: u32,
    session: u64,
    operation_id: u64,
    kind: u32,
    status: Option<i32>,
) -> WireOperationEnvelope {
    let record = recv_record(slot);
    if record.session != session
        || record.operation_id != operation_id
        || record.kind != kind
        || status.is_some_and(|expected| record.status != expected)
    {
        fail(b"operation record mismatch")
    }
    record
}

pub fn expect_accepted(slot: u32, session: u64, operation_id: u64, status: i32) {
    expect(slot, session, operation_id, KIND_ACCEPTED, Some(status));
}

pub fn expect_feedback(slot: u32, session: u64, operation_id: u64) -> WireOperationEnvelope {
    expect(slot, session, operation_id, KIND_FEEDBACK, None)
}

pub fn expect_result(
    slot: u32,
    session: u64,
    operation_id: u64,
    status: i32,
) -> WireOperationEnvelope {
    expect(slot, session, operation_id, KIND_RESULT, Some(status))
}

pub fn expect_terminal(slot: u32, session: u64, operation_id: u64, status: i32) {
    expect(slot, session, operation_id, KIND_TERMINAL, Some(status));
}

/// Await a terminal while yielding rather than parking. Used where the record is
/// produced by another component's death, so there is no endpoint whose
/// readiness would wake a parked receiver.
pub fn expect_terminal_yielding(slot: u32, session: u64, operation_id: u64, status: i32) {
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(slot, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            ERR_PEER_DEAD => fail(b"terminal peer died"),
            value if value < 0 => fail(b"terminal receive"),
            value => {
                release_caps(&caps);
                if value as usize != MAX_MSG {
                    fail(b"terminal length")
                }
                let record = WireOperationEnvelope::decode(&bytes)
                    .unwrap_or_else(|| fail(b"terminal decode"));
                if !slime_proto::valid_operation_envelope(&record, navigation_operation::TYPE_TAG)
                    || record.session != session
                    || record.operation_id != operation_id
                    || record.kind != KIND_TERMINAL
                    || record.status != status
                {
                    fail(b"terminal mismatch")
                }
                return;
            }
        }
    }
}
pub fn signal_phase(phase: u8) {
    loop {
        match slime_rt::send(PHASE_SLOT, &[phase], &[]) {
            ERR_SUCCESS => return,
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            _ => fail(b"phase send"),
        }
    }
}

pub fn signal_time_phase(phase: u8) {
    loop {
        match slime_rt::send(PHASE_TIME_SLOT, &[phase], &[]) {
            ERR_SUCCESS => return,
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            _ => fail(b"time phase send"),
        }
    }
}

pub fn wait_phase(expected: u8) {
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(PHASE_SLOT, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(PHASE_SLOT)]),
            value if value < 0 => fail(b"phase receive"),
            1 if bytes[0] == expected => return,
            _ => fail(b"phase mismatch"),
        }
    }
}

fn wait_restart_start() {
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(RESTART_START_SLOT, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(RESTART_START_SLOT)]),
            1 if bytes[0] == 1 => return,
            _ => fail(b"restart start"),
        }
    }
}

fn send_raw(slot: u32, bytes: &[u8]) {
    loop {
        match slime_rt::send(slot, bytes, &[]) {
            ERR_SUCCESS => return,
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            _ => fail(b"operation send"),
        }
    }
}

fn release_caps(caps: &[u64; MAX_CAPS_PER_MSG]) {
    for cap in caps.iter().filter(|cap| **cap != 0) {
        let _ = slime_rt::cap_drop(*cap as u32);
    }
}

pub fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[fabric-op] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

const _: () = assert!(REQUEST_LEN == MAX_MSG);
const _: () = assert!(OPERATION_LEN == MAX_MSG);
const _: () = assert!(TIME_ADVANCE_LEN == MAX_MSG);
