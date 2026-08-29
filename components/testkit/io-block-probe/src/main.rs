#![no_std]
#![no_main]

use slime_proto::block_v2::{self, WireBlockReply, WireBlockRequest};
use slime_proto::capability_transfer::OBJECT_KIND_SHARED_BUFFER_LOAN;
use slime_proto::io_queue::{
    self, COMPLETION_PAYLOAD_BYTES, STATUS_BAD_RIGHTS, STATUS_BAD_SLICE, STATUS_DEVICE_ERROR,
    STATUS_MALFORMED, STATUS_OK, WireBufferSlice,
};
use slime_proto::io_queue_ring::{Outstanding, Queue, QueueError, format};
use slime_rt::{
    CapabilityDisposition, ERR_SUCCESS, MAX_CAPS_PER_MSG, MAX_MSG, capability_delegate,
    debug_write, exit, notification_signal, resolve_binding, shared_buffer_create,
    shared_buffer_loan, shared_buffer_map, yield_now,
};

slime_rt::entry!(main);

const PEER_SLOT: u32 = 0;
const FACTORY_SLOT: u32 = 1;
const READONLY_PEER_SLOT: u32 = 8;
const IO0_BASE: u64 = 0x0000_001f_0000_0000;
const DATA_BASE: u64 = 0x0000_001f_0001_0000;
const READONLY_IO0_BASE: u64 = 0x0000_001f_0009_0000;
const READONLY_DATA_BASE: u64 = 0x0000_001f_000a_0000;
const PAGE: u64 = 4096;
const IO0_SLOTS: usize = 8;
const EPOCH: u64 = 1;
const RIGHT_BUFFER_MAP: u64 = 1 << 9;
const RIGHT_BUFFER_WRITE: u64 = 1 << 8;
const SECTOR_BYTES: usize = block_v2::SECTOR_BYTES;
const TEST_LBA: u64 = 3;
const WRITE_PREFIX: &[u8] = b"SLIMEIO2-WRITTEN";

fn main(_startup_arg: u32) {
    let request_ready = binding(b"notification:io-block-request-ready+signal");
    let completion_ready = binding(b"notification:io-block-completion-ready+wait");
    let readonly_request_ready = binding(b"notification:io-block-readonly-request-ready+signal");
    let readonly_completion_ready =
        binding(b"notification:io-block-readonly-completion-ready+wait");
    let io0 = shared_buffer_create(FACTORY_SLOT, 1, true).unwrap_or_else(|_| fail(b"io0 create"));
    let data = shared_buffer_create(FACTORY_SLOT, 8, true).unwrap_or_else(|_| fail(b"data create"));
    if shared_buffer_map(io0.slot, IO0_BASE, 0, PAGE, true) != ERR_SUCCESS
        || shared_buffer_map(data.slot, DATA_BASE, 0, 8 * PAGE, true) != ERR_SUCCESS
    {
        fail(b"buffer map");
    }
    let io0_bytes = unsafe { core::slice::from_raw_parts_mut(IO0_BASE as *mut u8, PAGE as usize) };
    let data_bytes =
        unsafe { core::slice::from_raw_parts_mut(DATA_BASE as *mut u8, (8 * PAGE) as usize) };
    format(io0_bytes, IO0_SLOTS, EPOCH).unwrap_or_else(|_| fail(b"io0 format"));
    let _io0_lease = delegate(io0.slot, PEER_SLOT, 0, PAGE, true);
    let data_lease = delegate(data.slot, PEER_SLOT, 0, 8 * PAGE, true);
    let capacity = await_capacity(PEER_SLOT);
    if capacity <= TEST_LBA {
        fail(b"capacity too small");
    }

    let mut queue = Queue::attach(io0_bytes, IO0_SLOTS).unwrap_or_else(|_| fail(b"io0 attach"));
    let mut outstanding = Outstanding::<IO0_SLOTS>::new(EPOCH);
    let read_slice = data_slice(
        data.id,
        data_lease,
        SECTOR_BYTES as u64,
        io_queue::DIRECTION_DEVICE_WRITE,
    );
    for id in 1..=IO0_SLOTS as u64 {
        submit(
            &mut queue,
            &mut outstanding,
            id,
            &read_slice,
            &request(block_v2::OP_READ, id % capacity, 1).encode(),
        );
    }
    let mut full_refusals = 0u64;
    if queue.submit(99, &read_slice, &[], false, 8 * PAGE) == Err(QueueError::Full) {
        full_refusals += 1;
    } else {
        fail(b"backpressure overwrite");
    }
    write_number(
        b"[io-block-probe] backpressure full_refusals=",
        full_refusals,
    );
    write_number(
        b" overwrite=",
        queue.submitted().saturating_sub(IO0_SLOTS as u64),
    );
    debug_write(b"\n");
    signal(request_ready);

    let mut body = [0u8; COMPLETION_PAYLOAD_BYTES];
    let mut completed = 0u64;
    let mut identities = 0u64;
    let mut seen = 0u16;
    while completed < IO0_SLOTS as u64 {
        match queue.take_completion(&outstanding, &mut body) {
            Ok(completion) => {
                if completion.status != STATUS_OK
                    || !(1..=IO0_SLOTS as u64).contains(&completion.request_id)
                {
                    fail(b"async completion identity");
                }
                let bit = 1u16 << (completion.request_id - 1);
                if seen & bit != 0 {
                    fail(b"async duplicate identity");
                }
                seen |= bit;
                identities += 1;
                outstanding
                    .settle(completion.request_id, completion.status)
                    .unwrap_or_else(|_| fail(b"async settle"));
                completed += 1;
            }
            Err(QueueError::Empty) => wait_completion(completion_ready),
            Err(_) => fail(b"async completion"),
        }
    }
    if !outstanding.is_empty() {
        fail(b"async completion count");
    }
    write_number(b"[io-block-probe] async queued=", IO0_SLOTS as u64);
    write_number(b" completed=", completed);
    write_number(b" identities=", identities);
    write_number(b" overwrite=", queue.submitted());
    debug_write(b"\n");

    let mut next_id = 100u64;
    let mut reads = 0u64;
    let mut writes = 0u64;
    let mut flushes = 0u64;
    let mut geometries = 0u64;
    let mut readback_bytes = 0u64;
    let mut mismatches = 0u64;

    let initial = transact(
        &mut queue,
        &mut outstanding,
        request_ready,
        completion_ready,
        next_id,
        &read_slice,
        &request(block_v2::OP_READ, TEST_LBA, 1).encode(),
        STATUS_OK,
    );
    next_id += 1;
    if initial.op != block_v2::OP_READ
        || initial.sectors_done != 1
        || initial.device_status != block_v2::DEVICE_STATUS_OK
    {
        fail(b"read reply");
    }
    reads += 1;
    for (observed, expected) in data_bytes[..8].iter().zip(b"SLIMEIO2") {
        if observed != expected {
            mismatches += 1;
        }
    }

    let none = WireBufferSlice {
        buffer: 0,
        lease: 0,
        offset: 0,
        length: 0,
        direction: io_queue::DIRECTION_NONE,
        reserved: [0; 4],
    };
    let geometry = transact(
        &mut queue,
        &mut outstanding,
        request_ready,
        completion_ready,
        next_id,
        &none,
        &request(block_v2::OP_GEOMETRY, 0, 0).encode(),
        STATUS_OK,
    );
    next_id += 1;
    if geometry.op != block_v2::OP_GEOMETRY
        || geometry.device_status != block_v2::DEVICE_STATUS_OK
        || geometry.detail != capacity
    {
        fail(b"geometry reply");
    }
    geometries += 1;

    data_bytes[..SECTOR_BYTES].fill(0xa5);
    data_bytes[..WRITE_PREFIX.len()].copy_from_slice(WRITE_PREFIX);
    let write_slice = data_slice(
        data.id,
        data_lease,
        SECTOR_BYTES as u64,
        io_queue::DIRECTION_DEVICE_READ,
    );
    let write = transact(
        &mut queue,
        &mut outstanding,
        request_ready,
        completion_ready,
        next_id,
        &write_slice,
        &request(block_v2::OP_WRITE, TEST_LBA, 1).encode(),
        STATUS_OK,
    );
    next_id += 1;
    if write.op != block_v2::OP_WRITE
        || write.sectors_done != 1
        || write.device_status != block_v2::DEVICE_STATUS_OK
    {
        fail(b"write reply");
    }
    writes += 1;

    let flush = transact(
        &mut queue,
        &mut outstanding,
        request_ready,
        completion_ready,
        next_id,
        &none,
        &request(block_v2::OP_FLUSH, 0, 0).encode(),
        STATUS_OK,
    );
    next_id += 1;
    if flush.op != block_v2::OP_FLUSH || flush.device_status != block_v2::DEVICE_STATUS_OK {
        fail(b"flush reply");
    }
    flushes += 1;

    data_bytes[..SECTOR_BYTES].fill(0);
    let readback = transact(
        &mut queue,
        &mut outstanding,
        request_ready,
        completion_ready,
        next_id,
        &read_slice,
        &request(block_v2::OP_READ, TEST_LBA, 1).encode(),
        STATUS_OK,
    );
    next_id += 1;
    if readback.op != block_v2::OP_READ
        || readback.sectors_done != 1
        || readback.device_status != block_v2::DEVICE_STATUS_OK
    {
        fail(b"readback reply");
    }
    reads += 1;
    for (index, observed) in data_bytes[..SECTOR_BYTES].iter().enumerate() {
        let expected = if index < WRITE_PREFIX.len() {
            WRITE_PREFIX[index]
        } else {
            0xa5
        };
        if *observed == expected {
            readback_bytes += 1;
        } else {
            mismatches += 1;
        }
    }
    if readback_bytes != SECTOR_BYTES as u64 || mismatches != 0 {
        fail(b"readback bytes");
    }

    write_number(b"[io-block-probe] operations read=", reads);
    write_number(b" write=", writes);
    write_number(b" flush=", flushes);
    write_number(b" geometry=", geometries);
    debug_write(b"\n");
    write_number(
        b"[io-block-probe] byte-verification readback=",
        readback_bytes,
    );
    write_number(b" mismatches=", mismatches);
    debug_write(b"\n");

    let mut out_of_range = 0u64;
    let out_of_range_reply = transact(
        &mut queue,
        &mut outstanding,
        request_ready,
        completion_ready,
        next_id,
        &read_slice,
        &request(block_v2::OP_READ, capacity, 1).encode(),
        STATUS_DEVICE_ERROR,
    );
    next_id += 1;
    if out_of_range_reply.device_status == block_v2::DEVICE_STATUS_IO_ERR
        && out_of_range_reply.detail == capacity
    {
        out_of_range += 1;
    } else {
        fail(b"out-of-range reply");
    }

    let mut malformed = 0u64;
    let malformed_reply = transact(
        &mut queue,
        &mut outstanding,
        request_ready,
        completion_ready,
        next_id,
        &read_slice,
        &[0],
        STATUS_MALFORMED,
    );
    next_id += 1;
    if malformed_reply.device_status == block_v2::DEVICE_STATUS_IO_ERR {
        malformed += 1;
    } else {
        fail(b"malformed reply");
    }

    let mut short_buffer = 0u64;
    let short_slice = data_slice(
        data.id,
        data_lease,
        (SECTOR_BYTES - 1) as u64,
        io_queue::DIRECTION_DEVICE_WRITE,
    );
    let short_reply = transact(
        &mut queue,
        &mut outstanding,
        request_ready,
        completion_ready,
        next_id,
        &short_slice,
        &request(block_v2::OP_READ, 0, 1).encode(),
        STATUS_BAD_SLICE,
    );
    next_id += 1;
    if short_reply.device_status == block_v2::DEVICE_STATUS_IO_ERR {
        short_buffer += 1;
    } else {
        fail(b"short-buffer reply");
    }

    let mut unsupported = 0u64;
    let unsupported_reply = transact(
        &mut queue,
        &mut outstanding,
        request_ready,
        completion_ready,
        next_id,
        &none,
        &request(0xff, 0, 0).encode(),
        STATUS_BAD_SLICE,
    );
    if unsupported_reply.device_status == block_v2::DEVICE_STATUS_IO_ERR {
        unsupported += 1;
    } else {
        fail(b"unsupported reply");
    }

    let readonly_io0 = shared_buffer_create(FACTORY_SLOT, 1, true)
        .unwrap_or_else(|_| fail(b"readonly io0 create"));
    let readonly_data = shared_buffer_create(FACTORY_SLOT, 8, true)
        .unwrap_or_else(|_| fail(b"readonly data create"));
    if shared_buffer_map(readonly_io0.slot, READONLY_IO0_BASE, 0, PAGE, true) != ERR_SUCCESS
        || shared_buffer_map(readonly_data.slot, READONLY_DATA_BASE, 0, 8 * PAGE, true)
            != ERR_SUCCESS
    {
        fail(b"readonly buffer map");
    }
    let readonly_io0_bytes =
        unsafe { core::slice::from_raw_parts_mut(READONLY_IO0_BASE as *mut u8, PAGE as usize) };
    let readonly_data_bytes = unsafe {
        core::slice::from_raw_parts_mut(READONLY_DATA_BASE as *mut u8, (8 * PAGE) as usize)
    };
    format(readonly_io0_bytes, IO0_SLOTS, EPOCH).unwrap_or_else(|_| fail(b"readonly io0 format"));
    let _readonly_io0_lease = delegate(readonly_io0.slot, READONLY_PEER_SLOT, 0, PAGE, true);
    let readonly_data_lease = delegate(readonly_data.slot, READONLY_PEER_SLOT, 0, 8 * PAGE, true);
    let readonly_capacity = await_capacity(READONLY_PEER_SLOT);
    if readonly_capacity == 0 {
        fail(b"readonly capacity");
    }
    readonly_data_bytes[..SECTOR_BYTES].fill(0x5a);
    let mut readonly_queue = Queue::attach(readonly_io0_bytes, IO0_SLOTS)
        .unwrap_or_else(|_| fail(b"readonly io0 attach"));
    let mut readonly_outstanding = Outstanding::<IO0_SLOTS>::new(EPOCH);
    let readonly_write_slice = data_slice(
        readonly_data.id,
        readonly_data_lease,
        SECTOR_BYTES as u64,
        io_queue::DIRECTION_DEVICE_READ,
    );
    let mut missing_right = 0u64;
    let missing_right_reply = transact(
        &mut readonly_queue,
        &mut readonly_outstanding,
        readonly_request_ready,
        readonly_completion_ready,
        1,
        &readonly_write_slice,
        &request(block_v2::OP_WRITE, 0, 1).encode(),
        STATUS_BAD_RIGHTS,
    );
    if missing_right_reply.device_status == block_v2::DEVICE_STATUS_IO_ERR {
        missing_right += 1;
    } else {
        fail(b"missing right reply");
    }

    write_number(b"[io-block-probe] refusals out_of_range=", out_of_range);
    write_number(b" malformed=", malformed);
    write_number(b" short_buffer=", short_buffer);
    write_number(b" unsupported=", unsupported);
    write_number(b" missing_right=", missing_right);
    debug_write(b"\n");
    write_number(
        b"[io-block-probe] io block plane complete observed_operations=",
        reads + writes + flushes + geometries,
    );
    write_number(
        b" observed_refusals=",
        out_of_range + malformed + short_buffer + unsupported + missing_right,
    );
    debug_write(b"\n");
    send_shutdown(request_ready);
    send_shutdown_to(READONLY_PEER_SLOT, readonly_request_ready);
    exit(0)
}

fn request(op: u8, lba: u64, sector_count: u32) -> WireBlockRequest {
    WireBlockRequest {
        magic: block_v2::BLOCK_MAGIC,
        version: block_v2::FORMAT_VERSION,
        op,
        flags: 0,
        lba,
        sector_count,
        reserved: [0; 4],
        padding: [0; 32],
    }
}

fn data_slice(buffer: u64, lease: u64, length: u64, direction: u32) -> WireBufferSlice {
    WireBufferSlice {
        buffer,
        lease,
        offset: 0,
        length,
        direction,
        reserved: [0; 4],
    }
}

fn submit(
    queue: &mut Queue<'_>,
    outstanding: &mut Outstanding<IO0_SLOTS>,
    request_id: u64,
    slice: &WireBufferSlice,
    payload: &[u8],
) {
    queue
        .submit(request_id, slice, payload, false, 8 * PAGE)
        .unwrap_or_else(|_| fail(b"request submit"));
    outstanding
        .admit(request_id, slice.lease, slice.length)
        .unwrap_or_else(|_| fail(b"request admit"));
}

fn transact(
    queue: &mut Queue<'_>,
    outstanding: &mut Outstanding<IO0_SLOTS>,
    request_ready: u32,
    completion_ready: u32,
    request_id: u64,
    slice: &WireBufferSlice,
    payload: &[u8],
    expected_status: u32,
) -> WireBlockReply {
    submit(queue, outstanding, request_id, slice, payload);
    signal(request_ready);
    let mut body = [0u8; COMPLETION_PAYLOAD_BYTES];
    loop {
        match queue.take_completion(outstanding, &mut body) {
            Ok(completion) => {
                if completion.request_id != request_id || completion.status != expected_status {
                    fail(b"completion status");
                }
                outstanding
                    .settle(completion.request_id, completion.status)
                    .unwrap_or_else(|_| fail(b"completion settle"));
                return WireBlockReply::decode(&body[..completion.payload_len])
                    .unwrap_or_else(|| fail(b"completion reply"));
            }
            Err(QueueError::Empty) => wait_completion(completion_ready),
            Err(_) => fail(b"completion drain"),
        }
    }
}

fn wait_completion(completion_ready: u32) {
    if slime_rt::notification_wait(completion_ready).is_err() {
        fail(b"completion wait");
    }
}

fn signal(notification: u32) {
    if notification_signal(notification) != ERR_SUCCESS {
        fail(b"notification signal");
    }
}

fn delegate(buffer_slot: u32, peer: u32, offset: u64, length: u64, writable: bool) -> u64 {
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
    loan.id
}

fn await_capacity(peer: u32) -> u64 {
    let mut message = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(peer, &mut message, &mut caps) {
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
    send_shutdown_to(PEER_SLOT, request_ready);
}

fn send_shutdown_to(peer: u32, request_ready: u32) {
    loop {
        match slime_rt::send(peer, &[1], &[]) {
            slime_rt::ERR_WOULDBLOCK => {
                signal(request_ready);
                yield_now();
            }
            ERR_SUCCESS => {
                signal(request_ready);
                return;
            }
            _ => fail(b"driver shutdown"),
        }
    }
}

fn binding(name: &[u8]) -> u32 {
    resolve_binding(name).unwrap_or_else(|_| fail(b"notification binding"))
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
    debug_write(b"[io-block-probe] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
