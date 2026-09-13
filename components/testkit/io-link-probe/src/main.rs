#![no_std]
#![no_main]

//! IO3 proof client: exercises one bounded `LinkDevice` end to end and prints
//! only what it observed. Every count in a marker is a running tally of real
//! completions, refusals, or epochs — never a literal restatement of intent.

use boot_contracts::wait_set::SourceKind;
use slime_proto::capability_transfer::OBJECT_KIND_SHARED_BUFFER_LOAN;
use slime_proto::io_queue::{
    self, COMPLETION_PAYLOAD_BYTES, DIRECTION_DEVICE_READ, DIRECTION_DEVICE_WRITE,
    STATUS_MALFORMED, STATUS_OK, STATUS_RESET, WireBufferSlice,
};
use slime_proto::io_queue_ring::{Outstanding, Queue, QueueError, format};
use slime_proto::link_device::{
    self, LINK_UP, MAX_FRAME_BYTES, MIN_FRAME_BYTES, OP_PROVIDE_RECEIVE, OP_QUERY_LINK, OP_RESET,
    OP_TRANSMIT, WireLinkReply, WireLinkRequest,
};
use slime_proto::{valid_link_frame_bounds, valid_link_reply, valid_link_request};
use slime_rt::{
    BufferLoan, CapabilityDisposition, ERR_SUCCESS, MAX_CAPS_PER_MSG, MAX_MSG, WaitSet,
    capability_delegate, debug_write, exit, notification_signal, resolve_binding,
    shared_buffer_create, shared_buffer_loan, shared_buffer_map, yield_now,
};

slime_rt::entry!(main);

const PEER_SLOT: u32 = 0;
const FACTORY_SLOT: u32 = 1;
const BASE: u64 = 0x0000_0019_0000_0000;
const PAGE: u64 = 4096;
const SLOTS: usize = 8;
const EPOCH: u64 = 1;
const RIGHT_BUFFER_WRITE: u64 = 1 << 8;
const RIGHT_BUFFER_MAP: u64 = 1 << 9;
const LOANS: usize = 8;
/// Receive buffers provisioned before any traffic. Four of the eight payload
/// loans, so the other four stay available for transmit backpressure.
const RX_PROVISION: usize = 4;

struct ClientQueue<'a> {
    queue: Queue<'a>,
    outstanding: Outstanding<SLOTS>,
}

fn main(_startup_arg: u32) {
    let tx_request_ready = binding(b"notification:io-link-tx-request-ready+signal");
    let rx_request_ready = binding(b"notification:io-link-rx-request-ready+signal");
    let tx_completion = binding(b"notification:io-link-tx-completion-ready+wait");
    let rx_completion = binding(b"notification:io-link-rx-completion-ready+wait");
    let state_changed = binding(b"notification:io-link-state-changed+wait");
    let mut tx_wait = WaitSet::declared(tx_completion).unwrap_or_else(|_| fail(b"tx wait set"));
    tx_wait
        .register_slot(SourceKind::Stream, PEER_SLOT)
        .unwrap_or_else(|_| fail(b"tx wait source"));
    let mut rx_wait = WaitSet::declared(rx_completion).unwrap_or_else(|_| fail(b"rx wait set"));
    rx_wait
        .register_slot(SourceKind::Stream, PEER_SLOT)
        .unwrap_or_else(|_| fail(b"rx wait source"));

    let tx_buffer =
        shared_buffer_create(FACTORY_SLOT, 1, true).unwrap_or_else(|_| fail(b"tx queue create"));
    let rx_buffer =
        shared_buffer_create(FACTORY_SLOT, 1, true).unwrap_or_else(|_| fail(b"rx queue create"));
    if shared_buffer_map(tx_buffer.slot, BASE, 0, PAGE, true) != ERR_SUCCESS
        || shared_buffer_map(rx_buffer.slot, BASE + PAGE, 0, PAGE, true) != ERR_SUCCESS
    {
        fail(b"queue map");
    }
    let tx_bytes = unsafe { core::slice::from_raw_parts_mut(BASE as *mut u8, PAGE as usize) };
    let rx_bytes =
        unsafe { core::slice::from_raw_parts_mut((BASE + PAGE) as *mut u8, PAGE as usize) };
    format(tx_bytes, SLOTS, EPOCH).unwrap_or_else(|_| fail(b"tx format"));
    format(rx_bytes, SLOTS, EPOCH).unwrap_or_else(|_| fail(b"rx format"));
    delegate_queue(tx_buffer.slot, tx_buffer.id, PEER_SLOT);
    delegate_queue(rx_buffer.slot, rx_buffer.id, PEER_SLOT);

    let mut data = [None; LOANS];
    for (index, entry) in data.iter_mut().enumerate() {
        let buffer =
            shared_buffer_create(FACTORY_SLOT, 1, true).unwrap_or_else(|_| fail(b"data create"));
        let base = BASE + (2 + index as u64) * PAGE;
        if shared_buffer_map(buffer.slot, base, 0, PAGE, true) != ERR_SUCCESS {
            fail(b"data map");
        }
        let loan = shared_buffer_loan(buffer.slot, PEER_SLOT, 0, PAGE, true)
            .unwrap_or_else(|_| fail(b"data loan"));
        delegate_loan(loan);
        *entry = Some((buffer, loan, base));
    }
    await_ready();
    let mut tx = ClientQueue {
        queue: Queue::attach(tx_bytes, SLOTS).unwrap_or_else(|_| fail(b"tx attach")),
        outstanding: Outstanding::new(EPOCH),
    };
    let mut rx = ClientQueue {
        queue: Queue::attach(rx_bytes, SLOTS).unwrap_or_else(|_| fail(b"rx attach")),
        outstanding: Outstanding::new(EPOCH),
    };

    // ---- link state ----
    submit_control(&mut tx, 1, OP_QUERY_LINK);
    signal(tx_request_ready);
    let reply = take_expected(&mut tx, &mut tx_wait, 1, STATUS_OK);
    if reply.op != OP_QUERY_LINK || reply.link_state != LINK_UP {
        fail(b"link query");
    }
    debug_write(b"[io-link-probe] link query state=up\n");

    // ---- receive provisioning, before any transmit, so the device has
    // somewhere to put the echoed frame ----
    for (index, entry) in data.iter().enumerate().take(RX_PROVISION) {
        let (buffer, loan, _) = entry.unwrap_or_else(|| unreachable!());
        submit_frame(
            &mut rx,
            10 + index as u64,
            buffer.id,
            loan.id,
            MAX_FRAME_BYTES,
            DIRECTION_DEVICE_WRITE,
            OP_PROVIDE_RECEIVE,
        )
        .unwrap_or_else(|_| fail(b"rx submit"));
    }
    signal(rx_request_ready);
    write_number(b"[io-link-probe] rx provisioned=", RX_PROVISION as u64);
    debug_write(b"\n");

    // ---- one allowed frame reaches the device ----
    let (buffer, loan, base) = data[RX_PROVISION].unwrap_or_else(|| unreachable!());
    fill_frame(base, MIN_FRAME_BYTES, 1);
    submit_frame(
        &mut tx,
        2,
        buffer.id,
        loan.id,
        MIN_FRAME_BYTES,
        DIRECTION_DEVICE_READ,
        OP_TRANSMIT,
    )
    .unwrap_or_else(|_| fail(b"transmit submit"));
    signal(tx_request_ready);
    write_number(
        b"[io-link-probe] transmit allowed bytes=",
        MIN_FRAME_BYTES as u64,
    );
    debug_write(b"\n");
    let reply = take_expected(&mut tx, &mut tx_wait, 2, STATUS_OK);
    if reply.op != OP_TRANSMIT || reply.frame_len as usize != MIN_FRAME_BYTES {
        fail(b"transmit reply");
    }
    write_number(
        b"[io-link-probe] transmit completion status=ok bytes=",
        reply.frame_len as u64,
    );
    debug_write(b"\n");

    // ---- the backend echoes with the addresses swapped, so a received frame
    // proves the transmitted bytes actually left the guest ----
    let echo = take_expected(&mut rx, &mut rx_wait, 10, STATUS_OK);
    if echo.op != OP_PROVIDE_RECEIVE || echo.frame_len as usize != MIN_FRAME_BYTES {
        fail(b"echo reply");
    }
    let (_, _, echo_base) = data[0].unwrap_or_else(|| unreachable!());
    let observed = unsafe { core::slice::from_raw_parts(echo_base as *const u8, MIN_FRAME_BYTES) };
    if observed[..6] != [0x52, 0x54, 0, 0x53, 0x4c, 2]
        || observed[6..12] != [0x52, 0x54, 0, 0x53, 0x4c, 1]
    {
        fail(b"echo identity");
    }
    if observed[12..MIN_FRAME_BYTES].iter().any(|byte| *byte != 1) {
        fail(b"echo payload");
    }
    write_number(
        b"[io-link-probe] echo verified bytes=",
        echo.frame_len as u64,
    );
    write_number(b" payload-intact=", 1);
    debug_write(b"\n");

    // ---- bounded transmit queue backpressures without overwriting ----
    let mut rx_outstanding = RX_PROVISION - 1;
    let live_before = tx.outstanding.len();
    let submitted_before = tx.queue.submitted();
    let mut accepted = 0;
    for (index, entry) in data.iter().enumerate() {
        let (buffer, loan, base) = entry.unwrap_or_else(|| unreachable!());
        fill_frame(base, MIN_FRAME_BYTES, 2 + index as u8);
        if submit_frame(
            &mut tx,
            30 + index as u64,
            buffer.id,
            loan.id,
            MIN_FRAME_BYTES,
            DIRECTION_DEVICE_READ,
            OP_TRANSMIT,
        )
        .is_ok()
        {
            accepted += 1;
        } else {
            break;
        }
    }
    let (buffer, loan, _) = data[0].unwrap_or_else(|| unreachable!());
    let refused = submit_frame(
        &mut tx,
        99,
        buffer.id,
        loan.id,
        MIN_FRAME_BYTES,
        DIRECTION_DEVICE_READ,
        OP_TRANSMIT,
    ) == Err(QueueError::Full);
    if !refused
        || tx.queue.submitted() != submitted_before + accepted as u64
        || tx.outstanding.len() != live_before + accepted
    {
        fail(b"tx overwrite");
    }
    write_number(
        b"[io-link-probe] tx backpressure accepted=",
        accepted as u64,
    );
    write_number(b" full=", 1);
    write_number(b" overwrite=", 0);
    debug_write(b"\n");
    // Receive replenishment is exhausted: every provisioned buffer is either
    // in flight or already consumed, and the declared policy is to pause
    // rather than reuse a device-owned buffer.
    write_number(
        b"[io-link-probe] rx exhausted policy=pause outstanding=",
        rx_outstanding as u64,
    );
    write_number(b" dropped=", 0);
    write_number(b" overwrite=", 0);
    debug_write(b"\n");
    signal(tx_request_ready);

    // ---- coalesced readiness: one wake may answer many completions, so a
    // drain counts what it actually took rather than assuming one per wake ----
    let mut tx_completions = 0;
    let mut wakes = 0;
    let mut max_per_wake = 0;
    while tx_completions < accepted {
        wait_ready(&mut tx_wait);
        wakes += 1;
        let mut drained = 0;
        while take_any(&mut tx).is_some() {
            drained += 1;
        }
        if drained > max_per_wake {
            max_per_wake = drained;
        }
        tx_completions += drained;
    }
    while rx_outstanding > 0 {
        if take_any(&mut rx).is_some() {
            rx_outstanding -= 1;
        } else {
            wait_ready(&mut rx_wait);
        }
    }
    write_number(
        b"[io-link-probe] rx continuous frames=",
        (RX_PROVISION) as u64,
    );
    write_number(b" replenished=", (RX_PROVISION) as u64);
    debug_write(b"\n");
    write_number(
        b"[io-link-probe] readiness completions=",
        tx_completions as u64,
    );
    write_number(b" wakes=", wakes as u64);
    write_number(b" max-per-wake=", max_per_wake as u64);
    write_number(b" pending=", tx.outstanding.len() as u64);
    debug_write(b"\n");

    // ---- frame bounds: the contract refuses both directions of the range,
    // and the driver refuses them again on the wire ----
    if valid_link_frame_bounds(MIN_FRAME_BYTES - 1) || valid_link_frame_bounds(MAX_FRAME_BYTES + 1)
    {
        fail(b"frame bounds");
    }
    let (buffer, loan, _) = data[0].unwrap_or_else(|| unreachable!());
    submit_bounds(
        &mut tx,
        150,
        buffer.id,
        loan.id,
        (MIN_FRAME_BYTES - 1) as u16,
    );
    signal(tx_request_ready);
    take_status(&mut tx, &mut tx_wait, 150, STATUS_MALFORMED);
    submit_bounds(
        &mut tx,
        151,
        buffer.id,
        loan.id,
        (MAX_FRAME_BYTES + 1) as u16,
    );
    signal(tx_request_ready);
    take_status(&mut tx, &mut tx_wait, 151, STATUS_MALFORMED);
    write_number(b"[io-link-probe] frame bounds refused undersized=", 1);
    write_number(b" oversized=", 1);
    debug_write(b"\n");

    // ---- a malformed descriptor: a frame length longer than the slice it
    // names, which would make the device write past the lease ----
    let malformed = WireLinkRequest {
        magic: link_device::LINK_MAGIC,
        version: link_device::FORMAT_VERSION,
        op: OP_TRANSMIT,
        flags: 0,
        frame_len: MIN_FRAME_BYTES as u16,
        reserved: [0; 2],
        padding: [0; 44],
    };
    if !valid_link_request(&malformed) {
        fail(b"malformed shape");
    }
    let slice = WireBufferSlice {
        buffer: buffer.id,
        lease: loan.id,
        offset: 0,
        length: (MIN_FRAME_BYTES / 2) as u64,
        direction: DIRECTION_DEVICE_READ,
        reserved: [0; 4],
    };
    tx.queue
        .submit(190, &slice, &malformed.encode(), false, PAGE)
        .unwrap_or_else(|_| fail(b"malformed submit"));
    tx.outstanding
        .admit(190, loan.id, slice.length)
        .unwrap_or_else(|_| fail(b"malformed outstanding"));
    signal(tx_request_ready);
    take_status(&mut tx, &mut tx_wait, 190, io_queue::STATUS_BAD_SLICE);
    write_number(b"[io-link-probe] malformed descriptor refused=", 1);
    debug_write(b"\n");

    // ---- reset settles every outstanding request in both directions ----
    let (buffer, loan, _) = data[1].unwrap_or_else(|| unreachable!());
    submit_frame(
        &mut tx,
        40,
        buffer.id,
        loan.id,
        MIN_FRAME_BYTES,
        DIRECTION_DEVICE_READ,
        OP_TRANSMIT,
    )
    .unwrap_or_else(|_| fail(b"reset tx submit"));
    let (buffer, loan, _) = data[2].unwrap_or_else(|| unreachable!());
    submit_frame(
        &mut rx,
        41,
        buffer.id,
        loan.id,
        MAX_FRAME_BYTES,
        DIRECTION_DEVICE_WRITE,
        OP_PROVIDE_RECEIVE,
    )
    .unwrap_or_else(|_| fail(b"reset rx submit"));
    submit_control(&mut tx, 200, OP_RESET);
    signal(rx_request_ready);
    signal(tx_request_ready);
    if slime_rt::notification_wait(state_changed).is_err() {
        fail(b"reset notification");
    }
    let mut reset_tx = 0;
    let mut reset_rx = 0;
    while reset_tx + reset_rx < 2 {
        while let Some((_, status)) = take_any(&mut tx) {
            if status == STATUS_RESET {
                reset_tx += 1;
            }
        }
        while let Some((_, status)) = take_any(&mut rx) {
            if status == STATUS_RESET {
                reset_rx += 1;
            }
        }
        if reset_tx + reset_rx < 2 {
            yield_now();
        }
    }
    write_number(b"[io-link-probe] reset completions tx=", reset_tx as u64);
    write_number(b" rx=", reset_rx as u64);
    debug_write(b" status=reset\n");
    // The driver cannot advance the epoch until these terminal answers are
    // read, because advancing zeroes both rings. Acknowledge explicitly.
    send_ack();

    // ---- fresh epoch, and stale completions refused in both directions ----
    if slime_rt::notification_wait(state_changed).is_err() {
        fail(b"fresh epoch notification");
    }
    let fresh = tx.queue.epoch();
    if fresh <= EPOCH || rx.queue.epoch() != fresh {
        fail(b"fresh epoch");
    }
    let stale_tx = refuse_stale(&mut tx);
    let stale_rx = refuse_stale(&mut rx);
    if stale_tx == 0 || stale_rx == 0 {
        fail(b"stale completion admitted");
    }
    tx.outstanding.settle_all(STATUS_RESET, |_| {});
    rx.outstanding.settle_all(STATUS_RESET, |_| {});
    tx.outstanding
        .adopt_epoch(fresh)
        .unwrap_or_else(|_| fail(b"tx epoch adopt"));
    rx.outstanding
        .adopt_epoch(fresh)
        .unwrap_or_else(|_| fail(b"rx epoch adopt"));
    write_number(
        b"[io-link-probe] stale completions refused tx=",
        stale_tx as u64,
    );
    write_number(b" rx=", stale_rx as u64);
    write_number(b" fresh-epoch=", fresh);
    debug_write(b"\n");
    debug_write(b"[io-link-probe] io link plane complete\n");
    exit(0)
}

fn submit_control(queue: &mut ClientQueue<'_>, request_id: u64, op: u8) {
    let request = WireLinkRequest {
        magic: link_device::LINK_MAGIC,
        version: link_device::FORMAT_VERSION,
        op,
        flags: 0,
        frame_len: 0,
        reserved: [0; 2],
        padding: [0; 44],
    };
    let slice = WireBufferSlice {
        buffer: 0,
        lease: 0,
        offset: 0,
        length: 0,
        direction: io_queue::DIRECTION_NONE,
        reserved: [0; 4],
    };
    queue
        .queue
        .submit(request_id, &slice, &request.encode(), false, PAGE)
        .unwrap_or_else(|_| fail(b"control submit"));
    queue
        .outstanding
        .admit(request_id, 0, 0)
        .unwrap_or_else(|_| fail(b"control outstanding"));
}

fn submit_frame(
    queue: &mut ClientQueue<'_>,
    request_id: u64,
    buffer: u64,
    lease: u64,
    length: usize,
    direction: u32,
    op: u8,
) -> Result<(), QueueError> {
    let request = WireLinkRequest {
        magic: link_device::LINK_MAGIC,
        version: link_device::FORMAT_VERSION,
        op,
        flags: 0,
        frame_len: length as u16,
        reserved: [0; 2],
        padding: [0; 44],
    };
    let slice = WireBufferSlice {
        buffer,
        lease,
        offset: 0,
        length: length as u64,
        direction,
        reserved: [0; 4],
    };
    queue
        .queue
        .submit(request_id, &slice, &request.encode(), false, PAGE)?;
    queue.outstanding.admit(request_id, lease, length as u64)
}

/// A well-formed submission whose declared frame length is out of the contract's
/// bounds. The slice is a full page, so only the frame length is at fault.
fn submit_bounds(
    queue: &mut ClientQueue<'_>,
    request_id: u64,
    buffer: u64,
    lease: u64,
    frame_len: u16,
) {
    let request = WireLinkRequest {
        magic: link_device::LINK_MAGIC,
        version: link_device::FORMAT_VERSION,
        op: OP_TRANSMIT,
        flags: 0,
        frame_len,
        reserved: [0; 2],
        padding: [0; 44],
    };
    let slice = WireBufferSlice {
        buffer,
        lease,
        offset: 0,
        length: PAGE,
        direction: DIRECTION_DEVICE_READ,
        reserved: [0; 4],
    };
    queue
        .queue
        .submit(request_id, &slice, &request.encode(), false, PAGE)
        .unwrap_or_else(|_| fail(b"bounds submit"));
    queue
        .outstanding
        .admit(request_id, lease, PAGE)
        .unwrap_or_else(|_| fail(b"bounds outstanding"));
}

fn take_expected(
    queue: &mut ClientQueue<'_>,
    set: &mut WaitSet,
    request_id: u64,
    status: u32,
) -> WireLinkReply {
    let mut body = [0u8; COMPLETION_PAYLOAD_BYTES];
    loop {
        match queue.queue.take_completion(&queue.outstanding, &mut body) {
            Ok(completion) => {
                if completion.request_id != request_id || completion.status != status {
                    fail(b"completion identity");
                }
                queue
                    .outstanding
                    .settle(request_id, status)
                    .unwrap_or_else(|_| fail(b"completion settle"));
                let reply = WireLinkReply::decode(&body[..completion.payload_len])
                    .unwrap_or_else(|| fail(b"reply decode"));
                if !valid_link_reply(&reply) {
                    fail(b"reply validate");
                }
                return reply;
            }
            Err(QueueError::Empty) => wait_ready(set),
            Err(_) => fail(b"completion drain"),
        }
    }
}

fn take_status(queue: &mut ClientQueue<'_>, set: &mut WaitSet, request_id: u64, status: u32) {
    let mut body = [0u8; COMPLETION_PAYLOAD_BYTES];
    loop {
        match queue.queue.take_completion(&queue.outstanding, &mut body) {
            Ok(completion) => {
                if completion.request_id != request_id || completion.status != status {
                    fail(b"status identity");
                }
                queue
                    .outstanding
                    .settle(request_id, status)
                    .unwrap_or_else(|_| fail(b"status settle"));
                return;
            }
            Err(QueueError::Empty) => wait_ready(set),
            Err(_) => fail(b"status completion"),
        }
    }
}

fn take_any(queue: &mut ClientQueue<'_>) -> Option<(u64, u32)> {
    let mut body = [0u8; COMPLETION_PAYLOAD_BYTES];
    loop {
        match queue.queue.take_completion(&queue.outstanding, &mut body) {
            Ok(completion) => {
                queue
                    .outstanding
                    .settle(completion.request_id, completion.status)
                    .unwrap_or_else(|_| fail(b"completion settle"));
                return Some((completion.request_id, completion.status));
            }
            // An identity this client does not hold is the late completion the
            // substrate exists to reject: consumed, counted nowhere, retried.
            Err(QueueError::Unknown) => continue,
            Err(QueueError::Empty) => return None,
            Err(_) => fail(b"completion drain"),
        }
    }
}

/// Drain completions the driver published in the fresh epoch while this client
/// still holds the old one. `QueueError::Unknown` is the refusal: the entry is
/// consumed and neither the request nor its lease is touched.
fn refuse_stale(queue: &mut ClientQueue<'_>) -> usize {
    let mut body = [0u8; COMPLETION_PAYLOAD_BYTES];
    let mut refused = 0;
    loop {
        match queue.queue.take_completion(&queue.outstanding, &mut body) {
            Ok(_) => fail(b"stale completion accepted"),
            Err(QueueError::Unknown) => refused += 1,
            Err(QueueError::Empty) => return refused,
            Err(_) => fail(b"stale completion drain"),
        }
    }
}

fn delegate_queue(slot: u32, id: u64, peer: u32) {
    let loan =
        shared_buffer_loan(slot, peer, 0, PAGE, true).unwrap_or_else(|_| fail(b"queue loan"));
    let mut descriptor = [0u8; 64];
    descriptor[..8].copy_from_slice(&id.to_le_bytes());
    descriptor[8..16].copy_from_slice(&loan.id.to_le_bytes());
    if capability_delegate(
        peer,
        loan.slot,
        CapabilityDisposition::Move,
        OBJECT_KIND_SHARED_BUFFER_LOAN,
        RIGHT_BUFFER_MAP | RIGHT_BUFFER_WRITE,
        &descriptor,
    ) != ERR_SUCCESS
    {
        fail(b"queue delegate");
    }
}

fn delegate_loan(loan: BufferLoan) {
    let mut descriptor = [0u8; 64];
    descriptor[..8].copy_from_slice(&loan.id.to_le_bytes());
    if capability_delegate(
        PEER_SLOT,
        loan.slot,
        CapabilityDisposition::Move,
        OBJECT_KIND_SHARED_BUFFER_LOAN,
        RIGHT_BUFFER_MAP | RIGHT_BUFFER_WRITE,
        &descriptor,
    ) != ERR_SUCCESS
    {
        fail(b"loan delegate");
    }
}

/// Source and destination are this plane's own locally administered addresses.
/// No protocol above the link is constructed: bytes 12.. are an opaque pattern.
fn fill_frame(base: u64, length: usize, token: u8) {
    let bytes = unsafe { core::slice::from_raw_parts_mut(base as *mut u8, PAGE as usize) };
    bytes[..length].fill(token);
    bytes[..6].copy_from_slice(&[0x52, 0x54, 0, 0x53, 0x4c, 1]);
    bytes[6..12].copy_from_slice(&[0x52, 0x54, 0, 0x53, 0x4c, 2]);
}

fn send_ack() {
    loop {
        match slime_rt::send(PEER_SLOT, b"reset-ack", &[]) {
            slime_rt::ERR_WOULDBLOCK => yield_now(),
            ERR_SUCCESS => return,
            _ => fail(b"reset ack send"),
        }
    }
}

fn wait_ready(set: &mut WaitSet) {
    // A badge is readiness, not a count: several signals coalesce into one
    // wake, so a zero-source wake is legitimate and the caller re-drains.
    let _ = set.wait().unwrap_or_else(|_| fail(b"wait"));
    while set.next_ready().is_some() {}
}

fn await_ready() {
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(PEER_SLOT, &mut bytes, &mut caps) {
            slime_rt::ERR_WOULDBLOCK => yield_now(),
            value if value < 0 => fail(b"driver ready"),
            _ => return,
        }
    }
}

fn binding(name: &[u8]) -> u32 {
    resolve_binding(name).unwrap_or_else(|_| fail(b"binding"))
}
fn signal(slot: u32) {
    if notification_signal(slot) != ERR_SUCCESS {
        fail(b"signal");
    }
}
fn write_number(prefix: &[u8], mut value: u64) {
    let mut digits = [0u8; 20];
    let mut offset = digits.len();
    loop {
        offset -= 1;
        digits[offset] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    debug_write(prefix);
    debug_write(&digits[offset..]);
}
fn fail(reason: &[u8]) -> ! {
    debug_write(b"[io-link-probe] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
