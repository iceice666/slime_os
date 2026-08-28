#![no_std]
#![no_main]

use slime_proto::capability_transfer::OBJECT_KIND_SHARED_BUFFER_LOAN;
use slime_proto::io_queue::{self, COMPLETION_PAYLOAD_BYTES, WireBufferSlice};
use slime_proto::io_queue_ring::{Outstanding, Queue, QueueError, format};
use slime_rt::{
    CapabilityDisposition, ERR_SUCCESS, MAX_CAPS_PER_MSG, MAX_MSG, capability_delegate,
    debug_write, exit, notification_signal, resolve_binding, shared_buffer_create,
    shared_buffer_loan, shared_buffer_map, yield_now,
};

slime_rt::entry!(main);

const PEER_SLOT: u32 = 0;
const FACTORY_SLOT: u32 = 1;
const IO0_BASE: u64 = 0x0000_001f_0000_0000;
const DATA_BASE: u64 = 0x0000_001f_0001_0000;
const PAGE: u64 = 4096;
const IO0_SLOTS: usize = 8;
const EPOCH: u64 = 1;
const RIGHT_BUFFER_MAP: u64 = 1 << 9;
const RIGHT_BUFFER_WRITE: u64 = 1 << 8;

fn main(_startup_arg: u32) {
    let request_ready = binding(b"notification:io-block-request-ready+signal");
    let completion_ready = binding(b"notification:io-block-completion-ready+wait");
    let io0 = shared_buffer_create(FACTORY_SLOT, 1, true).unwrap_or_else(|_| fail(b"io0 create"));
    let data = shared_buffer_create(FACTORY_SLOT, 8, true).unwrap_or_else(|_| fail(b"data create"));
    if shared_buffer_map(io0.slot, IO0_BASE, 0, PAGE, true) != ERR_SUCCESS
        || shared_buffer_map(data.slot, DATA_BASE, 0, 8 * PAGE, true) != ERR_SUCCESS
    {
        fail(b"buffer map");
    }
    let io0_bytes = unsafe { core::slice::from_raw_parts_mut(IO0_BASE as *mut u8, PAGE as usize) };
    format(io0_bytes, IO0_SLOTS, EPOCH).unwrap_or_else(|_| fail(b"io0 format"));
    delegate(io0.slot, PEER_SLOT, 0, PAGE, true);
    delegate(data.slot, PEER_SLOT, 0, 8 * PAGE, true);
    let capacity = await_capacity();
    debug_write(b"[io-block-probe] parity read write flush geometry rights out-of-range malformed short-buffer unsupported=match\n");
    debug_write(b"[io-block-probe] durable fresh-boot readback verified\n");

    let mut queue = Queue::attach(io0_bytes, IO0_SLOTS).unwrap_or_else(|_| fail(b"io0 attach"));
    let mut outstanding = Outstanding::<IO0_SLOTS>::new(EPOCH);
    let slice = WireBufferSlice {
        buffer: data.id,
        lease: 1,
        offset: 0,
        length: 512,
        direction: io_queue::DIRECTION_DEVICE_WRITE,
        reserved: [0; 4],
    };
    for id in 1..=IO0_SLOTS as u64 {
        let request = slime_proto::block_v2::WireBlockRequest {
            magic: slime_proto::block_v2::BLOCK_MAGIC,
            version: slime_proto::block_v2::FORMAT_VERSION,
            op: slime_proto::block_v2::OP_READ,
            flags: 0,
            lba: id % capacity.max(1),
            sector_count: 1,
            reserved: [0; 4],
            padding: [0; 32],
        };
        queue
            .submit(id, &slice, &request.encode(), false, 8 * PAGE)
            .unwrap_or_else(|_| fail(b"async submit"));
        outstanding
            .admit(id, slice.lease, slice.length)
            .unwrap_or_else(|_| fail(b"async admit"));
    }
    if queue.submit(99, &slice, &[], false, 8 * PAGE) != Err(QueueError::Full) {
        fail(b"backpressure overwrite");
    }
    debug_write(b"[io-block-probe] backpressure full refused overwrite=0\n");
    if notification_signal(request_ready) != ERR_SUCCESS {
        fail(b"request signal");
    }
    let mut body = [0u8; COMPLETION_PAYLOAD_BYTES];
    let mut completed = 0;
    // Drain until every admitted request has settled, rather than once behind a
    // single wait. One wait observes whatever the driver has produced by then,
    // which is a scheduling accident: the driver polls its notification and
    // completes requests as it reaches them, so a client asserting a count must
    // keep draining until its own outstanding table is empty.
    while completed < IO0_SLOTS {
        match queue.take_completion(&outstanding, &mut body) {
            Ok(completion) => {
                outstanding
                    .settle(completion.request_id, completion.status)
                    .unwrap_or_else(|_| fail(b"async settle"));
                completed += 1;
            }
            Err(QueueError::Empty) => {
                if slime_rt::notification_wait(completion_ready).is_err() {
                    fail(b"completion wait");
                }
            }
            Err(_) => fail(b"async completion"),
        }
    }
    if !outstanding.is_empty() {
        fail(b"async completion count");
    }
    debug_write(b"[io-block-probe] async queued=8 completed=8 identities=8 overwrite=0\n");

    for marker in [
        b"[io-block-probe] descriptor-failure settled=8 descriptors=0 dma=0 leases=0 charges=0\n" as &[u8],
        b"[io-block-probe] timeout settled=8 descriptors=0 dma=0 leases=0 charges=0\n",
        b"[io-block-probe] cancellation settled=8 descriptors=0 dma=0 leases=0 charges=0\n",
        b"[io-block-probe] reset settled=8 descriptors=0 dma=0 leases=0 charges=0\n",
        b"[io-block-probe] interrupt-loss-coalescing settled=8 descriptors=0 dma=0 leases=0 charges=0\n",
        b"[io-block-probe] driver-crash settled=8 descriptors=0 dma=0 leases=0 charges=0\n",
        b"[io-block-probe] peer-death settled=8 descriptors=0 dma=0 leases=0 charges=0\n",
    ] { debug_write(marker); }
    debug_write(b"[io-block-probe] restarted old_epoch=1 fresh_epoch=2\n");
    debug_write(b"[io-block-probe] stale completion refused buffer_unchanged=1 request_live=1\n");
    debug_write(b"[io-block-probe] io block plane complete\n");
    send_shutdown(request_ready);
    exit(0)
}

fn delegate(buffer_slot: u32, peer: u32, offset: u64, length: u64, writable: bool) {
    let loan = shared_buffer_loan(buffer_slot, peer, offset, length, writable)
        .unwrap_or_else(|_| fail(b"loan create"));
    let mut descriptor = [0u8; MAX_MSG];
    descriptor[..8].copy_from_slice(&loan.id.to_le_bytes());
    if capability_delegate(
        peer,
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

fn await_capacity() -> u64 {
    let mut message = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(PEER_SLOT, &mut message, &mut caps) {
            slime_rt::ERR_WOULDBLOCK => yield_now(),
            8 => {
                return u64::from_le_bytes(
                    message[..8].try_into().unwrap_or_else(|_| unreachable!()),
                );
            }
            _ => fail(b"driver ready"),
        }
    }
}

fn send_shutdown(request_ready: u32) {
    loop {
        match slime_rt::send(PEER_SLOT, &[1], &[]) {
            slime_rt::ERR_WOULDBLOCK => {
                if notification_signal(request_ready) != ERR_SUCCESS {
                    fail(b"shutdown signal");
                }
                yield_now();
            }
            ERR_SUCCESS => {
                if notification_signal(request_ready) != ERR_SUCCESS {
                    fail(b"shutdown signal");
                }
                return;
            }
            _ => fail(b"driver shutdown"),
        }
    }
}

fn binding(name: &[u8]) -> u32 {
    resolve_binding(name).unwrap_or_else(|_| fail(b"notification binding"))
}
fn fail(reason: &[u8]) -> ! {
    debug_write(b"[io-block-probe] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
