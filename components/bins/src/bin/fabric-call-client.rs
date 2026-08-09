#![no_std]
#![no_main]

#[path = "../fabric_call_scenario.rs"]
mod scenario;

use slime_proto::fabric_call::{
    FLAG_NON_IDEMPOTENT, KIND_REQUEST, STATUS_MALFORMED_REPLY, STATUS_PEER_DEAD, STATUS_REJECTED,
    STATUS_RETRY_EXHAUSTED, STATUS_SUCCESS, STATUS_TIMEOUT,
};

slime_rt::entry!(main);

fn main() {
    scenario::boot_park(
        boot_contracts::fabric_graph::DIRECTION_CLIENT,
        b"fabric-call-client",
    );
    let route = scenario::request_role(boot_contracts::fabric_graph::DIRECTION_CLIENT);
    let session = scenario::client_session(0);

    scenario::send_call(
        route,
        scenario::envelope(
            session,
            1,
            KIND_REQUEST,
            FLAG_NON_IDEMPOTENT,
            STATUS_SUCCESS,
            7,
        ),
    );
    let reply = scenario::expect_reply(route, session, 1, STATUS_SUCCESS);
    if u64::from_le_bytes(reply.payload[..8].try_into().expect("reply payload")) != 11 {
        scenario::fail(b"reply payload")
    }
    slime_rt::debug_write(b"[fabric-call-client] success correlated\n");
    scenario::send_large_request(route, 2);
    scenario::expect_large_reply(route, 2);
    slime_rt::debug_write(b"[fabric-call-client] shared reply verified\n");

    scenario::send_call(
        route,
        scenario::envelope(session, 3, KIND_REQUEST, 0, STATUS_SUCCESS, 8),
    );
    scenario::expect_reply(route, session, 3, STATUS_REJECTED);
    slime_rt::debug_write(b"[fabric-call-client] rejection distinct\n");

    scenario::send_call(
        route,
        scenario::envelope(session, 4, KIND_REQUEST, 0, STATUS_SUCCESS, 9),
    );
    scenario::expect_terminal(route, session, 4, STATUS_MALFORMED_REPLY);
    slime_rt::debug_write(b"[fabric-call-client] malformed reply distinct\n");
    signal_client_b(0);
    wait_client_b(0);

    scenario::send_call(
        route,
        scenario::envelope(session, 5, KIND_REQUEST, 0, STATUS_SUCCESS, 5),
    );
    signal_phase(1);
    scenario::expect_terminal(route, session, 5, STATUS_TIMEOUT);
    slime_rt::debug_write(b"[fabric-call-client] timeout distinct\n");

    for request_id in 6..=9 {
        scenario::send_call(
            route,
            scenario::envelope(
                session,
                request_id,
                KIND_REQUEST,
                0,
                STATUS_SUCCESS,
                request_id + 100,
            ),
        );
    }
    scenario::send_call(
        route,
        scenario::envelope(session, 10, KIND_REQUEST, 0, STATUS_SUCCESS, 10),
    );
    scenario::expect_terminal(route, session, 10, STATUS_RETRY_EXHAUSTED);
    slime_rt::debug_write(b"[fabric-call-client] retry exhaustion distinct\n");

    signal_phase(2);
    for request_id in 6..=9 {
        scenario::expect_terminal(route, session, request_id, STATUS_TIMEOUT);
    }

    scenario::send_call(
        route,
        scenario::envelope(session, 11, KIND_REQUEST, 0, STATUS_SUCCESS, 11),
    );
    scenario::expect_terminal_parked(route, session, 11, STATUS_PEER_DEAD);
    slime_rt::debug_write(b"[fabric-call-client] peer death distinct\n");
    signal_phase(3);
}

fn signal_phase(phase: u8) {
    loop {
        match slime_rt::send(3, &[phase], &[]) {
            slime_rt::ERR_SUCCESS => return,
            slime_rt::ERR_WOULDBLOCK => slime_rt::yield_now(),
            _ => scenario::fail(b"time phase send"),
        }
    }
}

fn signal_client_b(phase: u8) {
    loop {
        match slime_rt::send(4, &[phase], &[]) {
            slime_rt::ERR_SUCCESS => return,
            slime_rt::ERR_WOULDBLOCK => slime_rt::yield_now(),
            _ => scenario::fail(b"client phase send"),
        }
    }
}

fn wait_client_b(expected: u8) {
    let mut bytes = [0u8; slime_rt::MAX_MSG];
    let mut caps = [0u64; slime_rt::MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(4, &mut bytes, &mut caps) {
            slime_rt::ERR_WOULDBLOCK => slime_rt::wait(&[slime_rt::WaitSource::Endpoint(4)]),
            value if value < 0 => scenario::fail(b"client phase receive"),
            1 if bytes[0] == expected => return,
            _ => scenario::fail(b"client phase mismatch"),
        }
    }
}
