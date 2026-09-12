#![no_std]
#![no_main]

//! IO11 client: the one component granted a TCP destination on the `sel4-io-tcp`
//! plane. It lends the network service a data queue, opens its declared
//! destination, sends one seeded stream and reads the echo back, then observes
//! the refused and the undeclared destination before closing everything it
//! opened. Every count in a marker is observed.

use slime_components::tick_clock::TickClock;
use slime_proto::io_queue::{
    self, COMPLETION_PAYLOAD_BYTES, DIRECTION_DEVICE_READ, DIRECTION_DEVICE_WRITE, WireBufferSlice,
};
use slime_proto::io_queue_ring::{Outstanding, Queue, QueueError, format};
use slime_proto::network_service::{
    self, WireLoanDelegation, WireNetworkCompletion, WireNetworkRequest,
};
use slime_proto::valid_network_completion;
use slime_rt::{
    CapabilityDisposition, ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG,
    capability_delegate, debug_write, exit, monotonic_frequency, monotonic_read,
    shared_buffer_create, shared_buffer_loan, shared_buffer_map, yield_now,
};

slime_rt::entry!(main);

/// The service endpoint and the buffer factory sit at fixed slots: a loan
/// names its receiver by endpoint slot, and the root reads that number as an
/// endpoint only while no shared-buffer capability occupies it.
const SERVICE_SLOT: u32 = 0;
const FACTORY_SLOT: u32 = 1;
const SHUTDOWN_CAPABILITY: u64 = u64::MAX;
const PEER: [u8; 4] = [10, 0, 0, 2];
const UNDECLARED: [u8; 4] = [10, 0, 0, 3];
const ECHO_PORT: u16 = 4242;
const REFUSED_PORT: u16 = 4243;
const STATUS_DENIED: i32 = -1;
const STATUS_UNREACHABLE: i32 = -5;
const PAGE: u64 = 4096;
const BASE: u64 = 0x0000_0019_0000_0000;
const SLOTS: usize = 8;
const EPOCH: u64 = 1;
const OBJECT_KIND_SHARED_BUFFER_LOAN: u32 =
    slime_proto::capability_transfer::OBJECT_KIND_SHARED_BUFFER_LOAN;
const RIGHT_BUFFER_WRITE: u64 = 1 << 8;
const RIGHT_BUFFER_MAP: u64 = 1 << 9;
/// How long the service stays bound to the link after the client's own arms
/// are done: the plane's peer needs a resident stack to address its ICMP echo
/// requests to, and a peer cannot tell a client when it is done.
const HOLD_MS: u64 = 3000;
/// The whole stream fits one page and the destination's byte budget, so one
/// SEND carries it; the echo comes back in whatever pieces the peer chose.
const STREAM_BYTES: usize = 4096;

struct DataQueue {
    queue: Queue<'static>,
    outstanding: Outstanding<SLOTS>,
    next_id: u64,
}

#[derive(Clone, Copy)]
struct Page {
    buffer: u64,
    lease: u64,
    base: u64,
}

impl Page {
    fn bytes(self) -> &'static mut [u8] {
        // Mapped at `base` for the life of this task; the service writes it
        // only through requests this task submitted.
        unsafe { core::slice::from_raw_parts_mut(self.base as *mut u8, PAGE as usize) }
    }
}

fn main(_: u32) {
    let rate = monotonic_frequency().unwrap_or_else(|_| fail(b"clock rate"));
    write_number(b"[io-tcp-probe] clock rate=", rate);
    debug_write(b"\n");

    let (mut data, pages) = lend_queue();

    let tcp = call(destination(network_service::OP_CONNECT, PEER, ECHO_PORT));
    if tcp.status_detail != 0 || tcp.capability_kind != network_service::CAPABILITY_TCP_CONNECTION {
        fail(b"tcp connect");
    }
    debug_write(b"[io-tcp-probe] connect dst=10.0.0.2:4242 status=ok\n");
    debug_write(b"[io-tcp-probe] tcp capabilities=1 rights=connect,send,recv\n");

    let out = pages[0].bytes();
    for (index, byte) in out.iter_mut().enumerate().take(STREAM_BYTES) {
        *byte = expected(index);
    }
    let sent = transfer(
        &mut data,
        network_service::OP_SEND,
        tcp.capability,
        pages[0],
        0,
        STREAM_BYTES,
    );
    if sent.status != io_queue::STATUS_OK || sent.transferred != STREAM_BYTES as u64 {
        fail(b"send");
    }
    write_number(b"[io-tcp-probe] sent bytes=", sent.transferred);
    debug_write(b"\n");

    let mut received = 0usize;
    let mut mismatches = 0u64;
    let mut receives = 0u64;
    while received < STREAM_BYTES {
        let want = STREAM_BYTES - received;
        let got = transfer(
            &mut data,
            network_service::OP_RECV,
            tcp.capability,
            pages[1],
            0,
            want,
        );
        if got.status != io_queue::STATUS_OK
            || got.transferred == 0
            || got.transferred as usize > want
        {
            fail(b"recv");
        }
        receives += 1;
        let bytes = pages[1].bytes();
        for (index, byte) in bytes.iter().enumerate().take(got.transferred as usize) {
            if *byte != expected(received + index) {
                mismatches += 1;
            }
        }
        received += got.transferred as usize;
    }
    write_number(b"[io-tcp-probe] received bytes=", received as u64);
    write_number(b" completions=", receives);
    debug_write(b"\n");
    write_number(b"[io-tcp-probe] stream verified bytes=", received as u64);
    write_number(b" mismatches=", mismatches);
    debug_write(b"\n");

    let closed = call(capability(network_service::OP_CLOSE, tcp.capability));
    if closed.status_detail != 0 {
        fail(b"close");
    }
    debug_write(b"[io-tcp-probe] close status=ok\n");

    let refused = call(destination(network_service::OP_CONNECT, PEER, REFUSED_PORT));
    if refused.status_detail != STATUS_UNREACHABLE || refused.capability != 0 {
        fail(b"refused connect");
    }
    debug_write(b"[io-tcp-probe] connect dst=10.0.0.2:4243 status=refused\n");

    let denied = call(destination(
        network_service::OP_CONNECT,
        UNDECLARED,
        ECHO_PORT,
    ));
    if denied.status_detail != STATUS_DENIED || denied.capability != 0 {
        fail(b"undeclared connect");
    }
    write_number(b"[io-tcp-probe] undeclared destination refusals=", 1);
    debug_write(b"\n");

    // The hold starts now, not at process start: the arms above took their
    // own time, and the peer's pings need the whole window after them.
    let hold_base = monotonic_read().unwrap_or_else(|_| fail(b"monotonic read"));
    let hold = TickClock::new(rate, hold_base).unwrap_or_else(|_| fail(b"clock rate too slow"));
    while hold.millis(monotonic_read().unwrap_or_else(|_| fail(b"monotonic read"))) < HOLD_MS as i64
    {
        yield_now();
    }
    write_number(b"[io-tcp-probe] held ms=", HOLD_MS);
    debug_write(b"\n");

    let shutdown = call(capability(network_service::OP_CLOSE, SHUTDOWN_CAPABILITY));
    if shutdown.status_detail != 0 {
        fail(b"shutdown");
    }
    write_number(b"[io-tcp-probe] closed capabilities=", 1);
    write_number(b" shutdown=", u64::from(shutdown.status_detail == 0));
    debug_write(b"\n");
    exit(0)
}

/// The stream's byte at `index`: a cheap function of position, so a received
/// byte is checked against its expected value without a second copy of the
/// stream.
fn expected(index: usize) -> u8 {
    (index.wrapping_mul(7).wrapping_add(index >> 8)) as u8
}

/// Create the queue page and two data pages, lend each to the service, and
/// attach this side of the ring. The service maps them where it likes.
fn lend_queue() -> (DataQueue, [Page; 2]) {
    let queue_buffer =
        shared_buffer_create(FACTORY_SLOT, 1, true).unwrap_or_else(|_| fail(b"queue create"));
    if shared_buffer_map(queue_buffer.slot, BASE, 0, PAGE, true) != ERR_SUCCESS {
        fail(b"queue map");
    }
    let queue_bytes = unsafe { core::slice::from_raw_parts_mut(BASE as *mut u8, PAGE as usize) };
    format(queue_bytes, SLOTS, EPOCH).unwrap_or_else(|_| fail(b"queue format"));
    let queue_loan = shared_buffer_loan(queue_buffer.slot, SERVICE_SLOT, 0, PAGE, true)
        .unwrap_or_else(|_| fail(b"queue loan"));
    delegate(
        queue_loan.slot,
        queue_buffer.id,
        queue_loan.id,
        network_service::DELEGATION_QUEUE,
    );

    let mut pages = [Page {
        buffer: 0,
        lease: 0,
        base: 0,
    }; 2];
    for (index, page) in pages.iter_mut().enumerate() {
        let buffer =
            shared_buffer_create(FACTORY_SLOT, 1, true).unwrap_or_else(|_| fail(b"data create"));
        let base = BASE + (1 + index as u64) * PAGE;
        if shared_buffer_map(buffer.slot, base, 0, PAGE, true) != ERR_SUCCESS {
            fail(b"data map");
        }
        let loan = shared_buffer_loan(buffer.slot, SERVICE_SLOT, 0, PAGE, true)
            .unwrap_or_else(|_| fail(b"data loan"));
        delegate(
            loan.slot,
            buffer.id,
            loan.id,
            network_service::DELEGATION_DATA,
        );
        *page = Page {
            buffer: buffer.id,
            lease: loan.id,
            base,
        };
    }
    let data = DataQueue {
        queue: Queue::attach(queue_bytes, SLOTS).unwrap_or_else(|_| fail(b"queue attach")),
        outstanding: Outstanding::new(EPOCH),
        next_id: 1,
    };
    (data, pages)
}

fn delegate(slot: u32, buffer: u64, lease: u64, kind: u8) {
    let descriptor = WireLoanDelegation {
        magic: network_service::DELEGATION_MAGIC,
        version: network_service::FORMAT_VERSION,
        kind,
        reserved: 0,
        buffer,
        lease,
        padding: [0; 40],
    }
    .encode();
    if capability_delegate(
        SERVICE_SLOT,
        slot,
        CapabilityDisposition::Move,
        OBJECT_KIND_SHARED_BUFFER_LOAN,
        RIGHT_BUFFER_MAP | RIGHT_BUFFER_WRITE,
        &descriptor,
    ) != ERR_SUCCESS
    {
        fail(b"loan delegate");
    }
}

struct Transferred {
    status: u32,
    transferred: u64,
}

/// Submit one data request over the ring and wait for its terminal answer.
fn transfer(
    data: &mut DataQueue,
    op: u8,
    id: u64,
    page: Page,
    offset: usize,
    length: usize,
) -> Transferred {
    let request_id = data.next_id;
    data.next_id += 1;
    let slice = WireBufferSlice {
        buffer: page.buffer,
        lease: page.lease,
        offset: offset as u64,
        length: length as u64,
        direction: if op == network_service::OP_SEND {
            DIRECTION_DEVICE_READ
        } else {
            DIRECTION_DEVICE_WRITE
        },
        reserved: [0; 4],
    };
    let payload = capability(op, id).encode();
    data.queue
        .submit(request_id, &slice, &payload, false, PAGE)
        .unwrap_or_else(|_| fail(b"data submit"));
    data.outstanding
        .admit(request_id, page.lease, length as u64)
        .unwrap_or_else(|_| fail(b"data admit"));
    let mut body = [0u8; COMPLETION_PAYLOAD_BYTES];
    loop {
        match data.queue.take_completion(&data.outstanding, &mut body) {
            Ok(completion) => {
                if completion.request_id != request_id {
                    fail(b"completion identity");
                }
                data.outstanding
                    .settle(request_id, completion.status)
                    .unwrap_or_else(|_| fail(b"completion settle"));
                let reply = WireNetworkCompletion::decode(&body[..completion.payload_len])
                    .unwrap_or_else(|| fail(b"data reply decode"));
                if !valid_network_completion(&reply) || reply.op != op {
                    fail(b"data reply");
                }
                return Transferred {
                    status: completion.status,
                    transferred: completion.transferred,
                };
            }
            Err(QueueError::Empty) => yield_now(),
            Err(_) => fail(b"data completion"),
        }
    }
}

fn destination(op: u8, address: [u8; 4], port: u16) -> WireNetworkRequest {
    let mut endpoint = [0; 24];
    endpoint[..4].copy_from_slice(&address);
    WireNetworkRequest {
        magic: network_service::NETWORK_MAGIC,
        version: network_service::FORMAT_VERSION,
        op,
        transport: network_service::TRANSPORT_TCP,
        flags: 0,
        port,
        name_len: 0,
        capability: 0,
        address_kind: network_service::ADDRESS_IPV4,
        reserved: [0; 7],
        endpoint,
    }
}

fn capability(op: u8, id: u64) -> WireNetworkRequest {
    WireNetworkRequest {
        magic: network_service::NETWORK_MAGIC,
        version: network_service::FORMAT_VERSION,
        op,
        transport: network_service::TRANSPORT_NONE,
        flags: 0,
        port: 0,
        name_len: 0,
        capability: id,
        address_kind: network_service::ADDRESS_NONE,
        reserved: [0; 7],
        endpoint: [0; 24],
    }
}

fn call(request: WireNetworkRequest) -> WireNetworkCompletion {
    let bytes = request.encode();
    loop {
        match slime_rt::send(SERVICE_SLOT, &bytes, &[]) {
            ERR_WOULDBLOCK => yield_now(),
            ERR_SUCCESS => break,
            _ => fail(b"send"),
        }
    }
    let mut out = [0; MAX_MSG];
    let mut caps = [0; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(SERVICE_SLOT, &mut out, &mut caps) {
            ERR_WOULDBLOCK => yield_now(),
            result if result < 0 => fail(b"recv"),
            result => {
                let reply = WireNetworkCompletion::decode(&out[..result as usize])
                    .unwrap_or_else(|| fail(b"reply"));
                if !valid_network_completion(&reply) {
                    fail(b"invalid completion");
                }
                return reply;
            }
        }
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
    debug_write(b"[io-tcp-probe] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
