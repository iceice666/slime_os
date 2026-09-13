#![no_std]
#![no_main]

use boot_contracts::wait_set::SourceKind;
use slime_proto::capability_transfer::OBJECT_KIND_SHARED_BUFFER_LOAN;
use slime_proto::io_queue::{
    self, COMPLETION_PAYLOAD_BYTES, DIRECTION_DEVICE_READ, STATUS_OK, STATUS_RESET, WireBufferSlice,
};
use slime_proto::io_queue_ring::{Outstanding, Queue, QueueError, format};
use slime_rt::{
    CapabilityDisposition, ERR_SUCCESS, MAX_CAPS_PER_MSG, MAX_MSG, WaitSet, capability_delegate,
    debug_write, exit, notification_signal, resolve_binding, shared_buffer_create,
    shared_buffer_loan, shared_buffer_map, yield_now,
};

slime_rt::entry!(main);

const PEER_SLOT: u32 = 0;
const FACTORY_SLOT: u32 = 1;
const BASE: u64 = 0x0000_0013_0000_0000;
const PAGE: u64 = 4096;
const SLOTS: usize = 4;
const EPOCH: u64 = 1;
const NORMAL_REQUESTS: usize = 4;
const RESET_REQUESTS: usize = 2;
const RIGHT_BUFFER_WRITE: u64 = 1 << 8;
const DUPLICATE_COMMAND: u8 = 1;
const RIGHT_BUFFER_MAP: u64 = 1 << 9;
const RESET_ACK: u8 = 2;

// This echo token is deliberately a local testkit protocol payload. It never
// crosses persistence, boot, or a general process API, so an eight-byte LE word
// is sufficient and no new Zutai wire contract is warranted.
fn payload(token: u64) -> [u8; 8] {
    token.to_le_bytes()
}

fn main(_startup_arg: u32) {
    let request_ready = binding(b"notification:io-queue-request-ready+signal");
    let completion_ready = binding(b"notification:io-queue-completion-ready+wait");
    let state_changed = binding(b"notification:io-queue-state-changed+wait");
    let mut completions = WaitSet::declared(completion_ready)
        .unwrap_or_else(|_| fail(b"completion wait set declaration"));
    completions
        .register_slot(SourceKind::Stream, PEER_SLOT)
        .unwrap_or_else(|_| fail(b"completion wait set registration"));

    let buffer = shared_buffer_create(FACTORY_SLOT, 1, true)
        .unwrap_or_else(|_| fail(b"shared queue create"));
    if shared_buffer_map(buffer.slot, BASE, 0, PAGE, true) != ERR_SUCCESS {
        fail(b"shared queue map");
    }
    let bytes = unsafe { core::slice::from_raw_parts_mut(BASE as *mut u8, PAGE as usize) };
    format(bytes, SLOTS, EPOCH).unwrap_or_else(|_| fail(b"queue format"));
    let loan = shared_buffer_loan(buffer.slot, PEER_SLOT, 0, PAGE, true)
        .unwrap_or_else(|_| fail(b"shared queue loan"));
    let mut descriptor = [0u8; MAX_MSG];
    descriptor[..8].copy_from_slice(&buffer.id.to_le_bytes());
    descriptor[8..16].copy_from_slice(&loan.id.to_le_bytes());
    if capability_delegate(
        PEER_SLOT,
        loan.slot,
        CapabilityDisposition::Move,
        OBJECT_KIND_SHARED_BUFFER_LOAN,
        RIGHT_BUFFER_MAP | RIGHT_BUFFER_WRITE,
        &descriptor,
    ) != ERR_SUCCESS
    {
        fail(b"queue loan delegation");
    }
    await_message();

    let mut queue = Queue::attach(bytes, SLOTS).unwrap_or_else(|_| fail(b"queue attach"));
    let slice = WireBufferSlice {
        buffer: buffer.id,
        lease: loan.id,
        offset: 2048,
        length: 64,
        direction: DIRECTION_DEVICE_READ,
        reserved: [0; 4],
    };
    let mut outstanding = Outstanding::<8>::new(EPOCH);

    for index in 0..NORMAL_REQUESTS {
        let request_id = 1 + index as u64;
        submit_checked(
            &mut queue,
            &mut outstanding,
            request_id,
            &slice,
            &payload(0x1000 + request_id),
        )
        .unwrap_or_else(|_| fail(b"round trip submit"));
    }
    signal(request_ready);
    wait_ready(&mut completions);
    drain_echoes(&mut queue, &mut outstanding, NORMAL_REQUESTS, 0x1000);
    debug_write(b"[io-queue-client] round trip echoes=4 drained=all\n");

    for index in 0..SLOTS {
        let request_id = 10 + index as u64;
        submit_checked(
            &mut queue,
            &mut outstanding,
            request_id,
            &slice,
            &payload(0x2000 + request_id),
        )
        .unwrap_or_else(|_| fail(b"backpressure fill"));
    }
    if queue.submit(99, &slice, &payload(99), false, PAGE) != Err(QueueError::Full) {
        fail(b"full ring overwrote a request");
    }
    debug_write(b"[io-queue-client] backpressure full refused overwrite=0\n");
    signal(request_ready);
    wait_ready(&mut completions);
    drain_echoes(&mut queue, &mut outstanding, SLOTS, 0x2000);

    send_command(DUPLICATE_COMMAND, 10);
    wait_ready(&mut completions);
    let mut body = [0u8; COMPLETION_PAYLOAD_BYTES];
    if queue.take_completion(&outstanding, &mut body) != Err(QueueError::Unknown) {
        fail(b"settled completion accepted twice");
    }
    debug_write(b"[io-queue-client] unknown completion refused\n");

    for index in 0..RESET_REQUESTS {
        let request_id = 30 + index as u64;
        submit_checked(
            &mut queue,
            &mut outstanding,
            request_id,
            &slice,
            &payload(0x3000 + request_id),
        )
        .unwrap_or_else(|_| fail(b"reset submit"));
    }
    signal(request_ready);
    if slime_rt::notification_wait(state_changed).is_err() {
        fail(b"resetting notification");
    }
    if queue.driver_state() != io_queue::DRIVER_RESETTING {
        fail(b"driver resetting state not observed");
    }
    debug_write(b"[io-queue-client] driver resetting observed\n");
    wait_ready(&mut completions);
    drain_resets(&mut queue, &mut outstanding, RESET_REQUESTS);
    send_command(RESET_ACK, 0);
    if slime_rt::notification_wait(state_changed).is_err() {
        fail(b"fresh epoch notification");
    }
    let fresh = queue.epoch();
    if fresh <= EPOCH || queue.driver_state() != io_queue::DRIVER_ACTIVE {
        fail(b"fresh active epoch not observed");
    }
    if submit_checked(&mut queue, &mut outstanding, 40, &slice, &payload(40))
        != Err(QueueError::StaleEpoch)
    {
        fail(b"old epoch submission accepted");
    }
    outstanding
        .adopt_epoch(fresh)
        .unwrap_or_else(|_| fail(b"fresh epoch adoption"));
    debug_write(b"[io-queue-client] fresh epoch observed old epoch refused\n");

    let malformed = WireBufferSlice {
        offset: PAGE - 8,
        length: 16,
        ..slice
    };
    let before = queue.submitted();
    if submit_checked(&mut queue, &mut outstanding, 41, &malformed, &payload(41))
        != Err(QueueError::Malformed)
        || queue.submitted() != before
        || !outstanding.is_empty()
    {
        fail(b"malformed slice reached submission ring");
    }
    debug_write(b"[io-queue-client] malformed slice refused before submission\n");
    debug_write(b"[io-queue-client] io queue plane complete\n");
    exit(0)
}

fn submit_checked(
    queue: &mut Queue<'_>,
    outstanding: &mut Outstanding<8>,
    request_id: u64,
    slice: &WireBufferSlice,
    body: &[u8],
) -> Result<(), QueueError> {
    if outstanding.epoch() != queue.epoch() {
        return Err(QueueError::StaleEpoch);
    }
    queue.submit(request_id, slice, body, false, PAGE)?;
    outstanding.admit(request_id, slice.lease, slice.length)
}

fn drain_echoes(queue: &mut Queue<'_>, outstanding: &mut Outstanding<8>, count: usize, base: u64) {
    let mut body = [0u8; COMPLETION_PAYLOAD_BYTES];
    for index in 0..count {
        let completion = queue
            .take_completion(outstanding, &mut body)
            .unwrap_or_else(|_| fail(b"completion drain"));
        if completion.status != STATUS_OK || completion.payload_len != 8 {
            fail(b"completion shape");
        }
        let token = u64::from_le_bytes(body[..8].try_into().unwrap_or_else(|_| unreachable!()));
        if token != base + completion.request_id {
            fail(b"completion echo");
        }
        outstanding
            .settle(completion.request_id, completion.status)
            .unwrap_or_else(|_| fail(b"completion settlement"));
        if index + 1 == count && queue.completions_pending() != 0 {
            fail(b"one wake did not drain all completions");
        }
    }
}

fn drain_resets(queue: &mut Queue<'_>, outstanding: &mut Outstanding<8>, count: usize) {
    let mut body = [0u8; COMPLETION_PAYLOAD_BYTES];
    for _ in 0..count {
        let completion = queue
            .take_completion(outstanding, &mut body)
            .unwrap_or_else(|_| fail(b"reset completion drain"));
        if completion.status != STATUS_RESET || !completion.epoch_ended {
            fail(b"reset completion shape");
        }
        outstanding
            .settle(completion.request_id, completion.status)
            .unwrap_or_else(|_| fail(b"reset settlement"));
    }
    if !outstanding.is_empty() || queue.completions_pending() != 0 {
        fail(b"reset did not drain all terminal states");
    }
}

fn wait_ready(set: &mut WaitSet) {
    if set.wait().unwrap_or_else(|_| fail(b"completion wait")) == 0 {
        fail(b"completion wake carried no declared badge");
    }
    while set.next_ready().is_some() {}
}

fn send_command(command: u8, request_id: u64) {
    let mut message = [0u8; 9];
    message[0] = command;
    message[1..].copy_from_slice(&request_id.to_le_bytes());
    loop {
        match slime_rt::send(PEER_SLOT, &message, &[]) {
            slime_rt::ERR_WOULDBLOCK => yield_now(),
            ERR_SUCCESS => return,
            _ => fail(b"control command"),
        }
    }
}

fn await_message() {
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(PEER_SLOT, &mut bytes, &mut caps) {
            slime_rt::ERR_WOULDBLOCK => yield_now(),
            result if result < 0 => fail(b"driver ready"),
            _ => return,
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
    debug_write(b"[io-queue-client] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
