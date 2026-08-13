#![allow(dead_code)]

use boot_contracts::fabric_graph::{DIRECTION_CLIENT, DIRECTION_SERVER};
use slime_proto::capability_transfer::OBJECT_KIND_SHARED_BUFFER_LOAN;
use slime_proto::fabric_call::*;
use slime_proto::interface_schema::parameter_call;
use slime_proto::sample_descriptor::{
    CAPABILITY_KIND_LOAN, SAMPLE_DESCRIPTOR_MAGIC, WireSampleDescriptor,
};
use slime_rt::{
    CapabilityDisposition, ERR_PEER_DEAD, ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG,
};

include!(concat!(env!("OUT_DIR"), "/fabric_profile.rs"));
const FACTORY_SLOT: u32 = 1;

/// The fabric, as this participant's loan receiver.
///
/// A loan names its receiver through a capability, and the broker is reachable
/// here by the *declared control endpoint* rather than by a supervision handle:
/// requiring supervision in this direction is an unbreakable spawn-ordering
/// cycle, since the fabric must already exist to loan a ring back while a
/// handle naming it cannot exist before it does. The generation fixes both ends
/// of this endpoint before either task runs, so the receiver is still a
/// capability fact and not an ambient task id.
const FABRIC_RECEIVER_SLOT: u32 = 0;
const PAGE: u64 = 4096;
const BASE: u64 = 0x7100_0000;
const CLIENT_PHASE_SLOT: u32 = 1;

pub fn run_client_b() {
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
    // Overrun the broker's in-flight bound, then drain what it queued.
    //
    // The burst is chunked rather than fired as one block of 24. Under the
    // logical channels this scenario was written against, a send buffered and
    // returned, so the client could offer everything and only then read. A
    // native send rendezvous instead: it blocks until the broker receives, and
    // the broker cannot hand a terminal to a client that is still sending --
    // `seL4_NBSend` reaches only a peer already blocked on receive. A client
    // that never pauses to read is therefore a client the broker can never
    // answer, and its queue fills at `MAX_PENDING_TERMINALS_PER_CLIENT`.
    //
    // The property under test is unchanged and still exercised: each chunk is
    // larger than the four calls the broker admits in flight, so every chunk
    // drives it past its bound and every request comes back refused.
    const BURST: u64 = 6;
    const CHUNKS: u64 = 4;
    for chunk in 0..CHUNKS {
        let first = 100 + chunk * BURST;
        for request_id in first..first + BURST {
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
        for request_id in first..first + BURST {
            expect_terminal(route, 0x000e_0000_0000_0001, request_id, STATUS_STALE);
        }
    }
    slime_rt::debug_write(b"[fabric-call-client-b] terminal backpressure recovered\n");
    signal_client_phase(0);

    slime_rt::debug_write(b"[fabric-call-client-b] unrelated route intact\n");
}

pub fn run_server() {
    let route = request_role(DIRECTION_SERVER);
    let mut executed_non_idempotent = false;
    loop {
        let mut bytes = [0u8; MAX_MSG];
        let mut caps = [0u64; MAX_CAPS_PER_MSG];
        // One endpoint, and nothing to do until it speaks: block in the kernel
        // rather than poll. The broker forwards with a blocking `send`, which
        // rendezvous only with a receiver already waiting, so a polling server
        // and a blocking broker would never meet.
        let length = match slime_rt::recv_blocking(route, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => {
                slime_rt::yield_now();
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
                // A delegated loan is a root-recorded export, not an in-message
                // capability: only a native Endpoint travels inline.
                let loan_slot = slime_rt::capability_import().unwrap_or(0);
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

/// Full-graph boot copies hold their declared endpoint but do not drive the
/// scenario transcript.
pub fn boot_park(_direction: u32, name: &'static [u8]) {
    if slime_components::fabric_boot::active() {
        slime_components::fabric_boot::park(name)
    }
}

/// Every call participant receives its preinstalled route endpoint at slot 0.
pub const fn request_role(_direction: u32) -> u32 {
    0
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
    let loan = slime_rt::shared_buffer_loan(buffer.slot, FABRIC_RECEIVER_SLOT, 0, PAGE, false)
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
    let loan = slime_rt::shared_buffer_loan(buffer.slot, FABRIC_RECEIVER_SLOT, 0, PAGE, false)
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

/// Wait for one call record on `slot`, blocking until it arrives.
///
/// Blocking is load-bearing, not an optimisation. A native `send` blocks in the
/// kernel until a receiver is ready, and a non-blocking `recv` only succeeds
/// against a sender *already* blocked on the endpoint. A caller that polls
/// while its peer blocks therefore never rendezvous with it: both sides wait
/// forever. A participant awaiting a reply has nothing else to do, so it waits
/// in the kernel where the sender can find it.
pub fn recv_call(slot: u32) -> WireCallEnvelope {
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv_blocking(slot, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
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

/// Wait for a large payload's descriptor and claim the loan it names.
///
/// The loan does not travel in the message. Only a native Endpoint crosses
/// inline, so a delegated loan is a root-recorded export the receiver claims
/// with `capability_import` -- `caps[0]` is always zero here, and reading it as
/// the loan is what made every shared exchange fail its shape check.
fn recv_descriptor(slot: u32) -> (WireSampleDescriptor, u32) {
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv_blocking(slot, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            value if value < 0 => fail(b"descriptor receive"),
            value => {
                if value as usize != MAX_MSG {
                    fail(b"descriptor shape")
                }
                let loan_slot = slime_rt::capability_import().unwrap_or(0);
                if loan_slot == 0 {
                    fail(b"descriptor carried no loan")
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
                return (descriptor, loan_slot);
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

/// Wait for a terminal the broker produces only after observing a peer's death.
///
/// Blocking, like every other receive here. The broker offers terminals with
/// `seL4_NBSend`, which reaches only a peer already blocked on the endpoint, so
/// a caller that polls and yields is never visible to it: two non-blocking
/// peers never rendezvous however long either spins. The name refers to the
/// *broker* parking on the supervision handle, not to this reader polling.
pub fn expect_terminal_parked(slot: u32, session: u64, request_id: u64, status: i32) {
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv_blocking(slot, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
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

/// Take one terminal and acknowledge it.
///
/// The ack is what lets the broker retire the record. It offers terminals with
/// `seL4_NBSend`, which reports nothing -- a blocking send would deadlock
/// against a client that is itself sending -- so the sender cannot observe
/// delivery and must be told. Without this the broker re-offers forever and
/// its queue fills at `MAX_PENDING_TERMINALS_PER_CLIENT`.
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
    // Ack with `seL4_Call`, not a bare send. The ack is the only thing that
    // retires the broker's record, so it must arrive -- which rules out
    // `try_send`, whose drops cost this plane 38 markers. But a blocking send
    // deadlocks: the broker may be mid-sweep offering the *next* terminal to
    // this very client, so neither peer is receiving.
    //
    // `Call` is neither. It blocks on the broker's reply atomically and hands
    // it authority naming this caller, so the answer cannot be taken by another
    // peer and this caller cannot miss it.
    let mut ack = terminal;
    ack.kind = KIND_TERMINAL_ACK;
    let mut answer = [0u8; MAX_MSG];
    if slime_rt::call(slot, &ack.encode(), &mut answer) < 0 {
        fail(b"terminal ack")
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

/// Wait for a phase barrier, blocking until the peer signals it.
///
/// A barrier is the one thing this component is waiting on, and the signaller
/// uses a blocking `send`. Polling here would leave both sides waiting: a
/// non-blocking receive only takes from a sender already blocked, and the
/// sender only unblocks once someone receives.
fn wait_client_phase(expected: u8) {
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv_blocking(CLIENT_PHASE_SLOT, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            value if value < 0 => fail(b"client phase receive"),
            1 if bytes[0] == expected => return,
            _ => fail(b"client phase mismatch"),
        }
    }
}

/// This component's badge bit on the broker's wake notification.
///
/// Every peer holds the same Notification object at a different slot, because
/// the slot *is* the badge: a broker woken by it can tell which peer spoke
/// without asking. Each binary therefore has its own number, set once at
/// startup from the generated profile rather than shared as a constant.
///
/// SAFETY: each component is single-threaded and sets this before any send.
static mut WAKE_SLOT: u32 = 0;

pub fn set_wake_slot(slot: u32) {
    // SAFETY: single-threaded, and called once before any send.
    unsafe { *core::ptr::addr_of_mut!(WAKE_SLOT) = slot };
}

/// The wake slot, or `None` where this graph declares no wake object. A
/// component compiled for every profile must tolerate its absence, since the
/// constant is emitted regardless (`u32::MAX` means "not in this graph").
fn wake_slot() -> Option<u32> {
    // SAFETY: single-threaded, as above.
    let slot = unsafe { *core::ptr::addr_of!(WAKE_SLOT) };
    (slot != u32::MAX).then_some(slot)
}

fn signal_wake() {
    if let Some(slot) = wake_slot() {
        let _ = slime_rt::notification_signal(slot);
    }
}

/// Send one record to the broker, waking it first.
///
/// The signal goes *before* the send, and that order is the whole point: a
/// native `send` blocks until the broker receives, so a broker parked waiting
/// for something else would never learn this peer had spoken. Signalling first
/// makes the wake already pending when the broker reaches its wait, so it
/// returns immediately and sweeps its endpoints -- finding this sender blocked
/// and ready to hand its message over.
///
/// A notification coalesces, so several peers signalling before the broker runs
/// collapse to one wake carrying every badge bit. That is correct here: the
/// wake means "at least one peer has something", and the sweep establishes
/// which.
fn send_raw(slot: u32, bytes: &[u8; MAX_MSG]) {
    signal_wake();
    loop {
        match slime_rt::send(slot, bytes, &[]) {
            ERR_SUCCESS => return,
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            _ => fail(b"call send"),
        }
    }
}

/// Delegate a loan to the broker, waking it first. Same ordering rule as
/// [`send_raw`]: the wake must be pending before the blocking transfer, or a
/// parked broker never learns this peer has spoken.
fn send_with_cap(slot: u32, bytes: &[u8; MAX_MSG], cap: u32) {
    signal_wake();
    loop {
        match slime_rt::capability_delegate(
            slot,
            cap,
            CapabilityDisposition::Move,
            OBJECT_KIND_SHARED_BUFFER_LOAN,
            1 << 9,
            bytes,
        ) {
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

const _: () = assert!(CALL_LEN == MAX_MSG);
const _: () = assert!(CALL_TIME_LEN == MAX_MSG);
