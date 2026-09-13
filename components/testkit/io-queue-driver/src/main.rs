#![no_std]
#![no_main]

use slime_proto::io_queue::{
    self, REQUEST_PAYLOAD_BYTES, STATE_RESET, STATUS_MALFORMED, STATUS_OK, STATUS_RESET,
};
use slime_proto::io_queue_ring::{Outstanding, Queue};
use slime_rt::{
    ERR_SUCCESS, MAX_CAPS_PER_MSG, MAX_MSG, debug_write, exit, notification_signal,
    resolve_binding, shared_buffer_loan_map, yield_now,
};

slime_rt::entry!(main);

const PEER_SLOT: u32 = 0;
const BASE: u64 = 0x0000_0014_0000_0000;
const PAGE: u64 = 4096;
const SLOTS: usize = 4;
const NORMAL_REQUESTS: usize = 4;
const BACKPRESSURE_REQUESTS: usize = 4;
const RESET_REQUESTS: usize = 2;
const DUPLICATE_COMMAND: u8 = 1;
const RESET_ACK: u8 = 2;

// This echo token is deliberately a local testkit protocol payload. It never
// crosses persistence, boot, or a general process API, so the request and
// completion bodies are only an eight-byte little-endian word.
fn main(_startup_arg: u32) {
    let request_ready = binding(b"notification:io-queue-request-ready+wait");
    let completion_ready = binding(b"notification:io-queue-completion-ready+signal");
    let state_changed = binding(b"notification:io-queue-state-changed+signal");

    let mut descriptor = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    let descriptor_len = loop {
        match slime_rt::recv(PEER_SLOT, &mut descriptor, &mut received) {
            slime_rt::ERR_WOULDBLOCK => yield_now(),
            result if result < 0 => fail(b"queue descriptor receive"),
            result => break result as usize,
        }
    };
    if descriptor_len != MAX_MSG {
        fail(b"queue descriptor shape");
    }
    let loan_slot = slime_rt::capability_import().unwrap_or_else(|_| fail(b"queue loan import"));
    if shared_buffer_loan_map(loan_slot, BASE, 0, PAGE) != ERR_SUCCESS {
        fail(b"queue loan map");
    }
    let bytes = unsafe { core::slice::from_raw_parts_mut(BASE as *mut u8, PAGE as usize) };
    let mut queue = Queue::attach(bytes, SLOTS).unwrap_or_else(|_| fail(b"queue attach"));
    let mut outstanding = Outstanding::<8>::new(queue.epoch());
    send_ready();

    wait_request(request_ready);
    drain_echoes(
        &mut queue,
        &mut outstanding,
        NORMAL_REQUESTS,
        completion_ready,
    );
    debug_write(b"[io-queue-driver] round trip drained=4 echoed=4\n");

    wait_request(request_ready);
    drain_echoes(
        &mut queue,
        &mut outstanding,
        BACKPRESSURE_REQUESTS,
        completion_ready,
    );

    let (command, settled) = recv_command();
    if command != DUPLICATE_COMMAND {
        fail(b"duplicate command");
    }
    queue
        .complete(settled, STATUS_OK, 0, &settled.to_le_bytes(), false)
        .unwrap_or_else(|_| fail(b"duplicate completion publish"));
    signal(completion_ready);
    debug_write(b"[io-queue-driver] duplicate completion published\n");

    wait_request(request_ready);
    let mut body = [0u8; REQUEST_PAYLOAD_BYTES];
    for _ in 0..RESET_REQUESTS {
        let submission = match queue.take_request(&mut body, PAGE) {
            Ok(submission) => submission,
            Err(error) => {
                if error.request_id != 0 {
                    queue
                        .complete(error.request_id, STATUS_MALFORMED, 0, &[], false)
                        .unwrap_or_else(|_| fail(b"malformed reset completion"));
                    signal(completion_ready);
                }
                fail(b"reset request drain")
            }
        };
        outstanding
            .admit(
                submission.request_id,
                submission.slice.lease,
                submission.slice.length,
            )
            .unwrap_or_else(|_| fail(b"reset request admission"));
        outstanding
            .start(submission.request_id)
            .unwrap_or_else(|_| fail(b"reset request start"));
    }
    queue.begin_reset();
    signal(state_changed);

    let mut released = 0usize;
    let mut released_requests = [0u64; RESET_REQUESTS];
    let settled = outstanding.settle_all(STATUS_RESET, |entry| {
        if entry.state != STATE_RESET || entry.lease == 0 {
            fail(b"reset settlement state");
        }
        if released_requests[..released].contains(&entry.request_id) {
            fail(b"lease released twice");
        }
        released_requests[released] = entry.request_id;
        released += 1;
    });
    if settled != RESET_REQUESTS || released != settled || !outstanding.is_empty() {
        fail(b"reset settlement count");
    }
    for request_id in [30u64, 31] {
        queue
            .complete(request_id, STATUS_RESET, 0, &[], true)
            .unwrap_or_else(|_| fail(b"reset completion publish"));
    }
    signal(completion_ready);
    debug_write(b"[io-queue-driver] reset settled=2 leases=2\n");

    let (command, _) = recv_command();
    if command != RESET_ACK {
        fail(b"reset acknowledgement");
    }
    let fresh = queue
        .advance_epoch()
        .unwrap_or_else(|_| fail(b"epoch advance"));
    outstanding
        .adopt_epoch(fresh)
        .unwrap_or_else(|_| fail(b"driver epoch adoption"));
    signal(state_changed);
    debug_write(b"[io-queue-driver] fresh epoch active\n");
    exit(0)
}

fn drain_echoes(
    queue: &mut Queue<'_>,
    outstanding: &mut Outstanding<8>,
    count: usize,
    completion_ready: u32,
) {
    let mut body = [0u8; REQUEST_PAYLOAD_BYTES];
    for index in 0..count {
        let submission = match queue.take_request(&mut body, PAGE) {
            Ok(submission) => submission,
            Err(error) => {
                if error.request_id != 0 {
                    queue
                        .complete(error.request_id, STATUS_MALFORMED, 0, &[], false)
                        .unwrap_or_else(|_| fail(b"malformed request completion"));
                    signal(completion_ready);
                }
                fail(b"request drain")
            }
        };
        if submission.payload_len != 8
            || submission.slice.direction != io_queue::DIRECTION_DEVICE_READ
            || submission.slice.offset + submission.slice.length > PAGE
        {
            fail(b"request validation");
        }
        outstanding
            .admit(
                submission.request_id,
                submission.slice.lease,
                submission.slice.length,
            )
            .unwrap_or_else(|_| fail(b"driver admission"));
        outstanding
            .start(submission.request_id)
            .unwrap_or_else(|_| fail(b"driver start"));
        let token = u64::from_le_bytes(body[..8].try_into().unwrap_or_else(|_| unreachable!()));
        let settled = outstanding
            .settle(submission.request_id, STATUS_OK)
            .unwrap_or_else(|_| fail(b"driver settlement"));
        if settled.lease != submission.slice.lease {
            fail(b"settled lease identity");
        }
        queue
            .complete(
                submission.request_id,
                STATUS_OK,
                submission.slice.length,
                &token.to_le_bytes(),
                false,
            )
            .unwrap_or_else(|_| fail(b"completion publish"));
        if index + 1 == count && queue.submitted() != 0 {
            fail(b"one wake did not drain every request");
        }
    }
    signal(completion_ready);
}

fn wait_request(slot: u32) {
    if slime_rt::notification_wait(slot).is_err() {
        fail(b"request readiness wait");
    }
}

fn recv_command() -> (u8, u64) {
    let mut message = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(PEER_SLOT, &mut message, &mut caps) {
            slime_rt::ERR_WOULDBLOCK => yield_now(),
            result if result < 0 => fail(b"control receive"),
            9 => {
                let request_id =
                    u64::from_le_bytes(message[1..9].try_into().unwrap_or_else(|_| unreachable!()));
                return (message[0], request_id);
            }
            _ => fail(b"control message shape"),
        }
    }
}

fn send_ready() {
    loop {
        match slime_rt::send(PEER_SLOT, b"ready", &[]) {
            slime_rt::ERR_WOULDBLOCK => yield_now(),
            ERR_SUCCESS => return,
            _ => fail(b"driver ready send"),
        }
    }
}

fn binding(name: &[u8]) -> u32 {
    resolve_binding(name).unwrap_or_else(|_| fail(b"notification binding"))
}

fn signal(slot: u32) {
    if notification_signal(slot) != ERR_SUCCESS {
        fail(b"notification signal");
    }
}

fn fail(reason: &[u8]) -> ! {
    debug_write(b"[io-queue-driver] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
