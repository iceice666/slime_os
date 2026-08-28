#![no_std]
#![no_main]

use core::ptr;
use core::sync::atomic::{Ordering, fence};

use slime_components::virtio_mmio::{
    self, DESC_F_NEXT, DESC_F_WRITE, MediatedMmio, observe_used, publish_available,
    write_descriptor, write_u16,
};
use slime_proto::block_v2::{
    self, DEVICE_STATUS_IO_ERR, DEVICE_STATUS_OK, DEVICE_STATUS_UNSUPPORTED, WireBlockReply,
    WireBlockRequest,
};
use slime_proto::io_queue::{
    self, REQUEST_PAYLOAD_BYTES, STATUS_BAD_SLICE, STATUS_DEVICE_ERROR, STATUS_MALFORMED,
    STATUS_OK, STATUS_UNSUPPORTED,
};
use slime_proto::io_queue_ring::{Outstanding, Queue, QueueError};
use slime_proto::{valid_block_v2_request, valid_buffer_slice};
use slime_rt::{
    DmaDirection, DmaMapping, ERR_SUCCESS, MAX_CAPS_PER_MSG, MAX_MSG, capability_import,
    debug_write, exit, io_device_bind, io_dma_map, io_queue_map, io_request_begin,
    io_request_settle, notification_signal, resolve_binding, shared_buffer_loan_map, yield_now,
};

slime_rt::entry!(main);

const PEER_SLOT: u32 = 0;
const DEVICE_SLOT: u32 = 1;
const MMIO_SLOT: u32 = 2;
const DMA_SLOT: u32 = 4;
const IO0_BASE: u64 = 0x0000_0020_0000_0000;
const DATA_BASE: u64 = 0x0000_0020_0001_0000;
const DEVICE_QUEUE_BASE: u64 = 0x0000_0020_0003_0000;
const PAGE: u64 = 4096;
const IO0_BYTES: u64 = 4096;
const DATA_BYTES: u64 = 8 * PAGE;
const DEVICE_QUEUE_PAGES: u32 = 4;
const IO0_SLOTS: usize = 8;
const VIRTQUEUE_SIZE: usize = 16;
const AVAIL_OFFSET: usize = VIRTQUEUE_SIZE * 16;
const USED_OFFSET: usize = 0x1000;
const CONTROL_OFFSET: usize = 0x2000;
const CONTROL_STRIDE: usize = 32;
const VIRTIO_DEVICE_BLOCK: u32 = 2;
const COMPLETION_POLLS: u32 = 100_000_000;

fn main(_startup_arg: u32) {
    let request_ready = binding(b"notification:io-block-request-ready+wait");
    let completion_ready = binding(b"notification:io-block-completion-ready+signal");
    let state_changed = binding(b"notification:io-block-state-changed+signal");
    let device = io_device_bind(DEVICE_SLOT).unwrap_or_else(|_| fail(b"device bind"));
    let mmio = MediatedMmio::new(DEVICE_SLOT, MMIO_SLOT, device.epoch);
    let queue_dma = io_queue_map(
        DMA_SLOT,
        device.epoch,
        DEVICE_QUEUE_BASE,
        DEVICE_QUEUE_PAGES,
    )
    .unwrap_or_else(|_| fail(b"device queue map"));
    debug_write(b"[virtio-blk-driver] mmio mechanism=mediated-bounded-read32-write32\n");
    let handshake = mmio
        .begin(VIRTIO_DEVICE_BLOCK)
        .unwrap_or_else(|_| fail(b"virtio handshake"));
    handshake
        .configure_queue(
            0,
            VIRTQUEUE_SIZE as u16,
            PAGE as u32,
            PAGE as u32,
            queue_dma.iova,
        )
        .unwrap_or_else(|_| fail(b"virtqueue setup"));
    let mmio = handshake.finish();
    let capacity = read_capacity(mmio);

    let (io0_loan, data_loan) = receive_loans();
    if shared_buffer_loan_map(io0_loan, IO0_BASE, 0, IO0_BYTES) != ERR_SUCCESS
        || shared_buffer_loan_map(data_loan, DATA_BASE, 0, DATA_BYTES) != ERR_SUCCESS
    {
        fail(b"loan map");
    }
    let data_read = io_dma_map(DMA_SLOT, data_loan, device.epoch, DmaDirection::DeviceRead)
        .unwrap_or_else(|_| fail(b"device-read dma"));
    let data_write = io_dma_map(DMA_SLOT, data_loan, device.epoch, DmaDirection::DeviceWrite)
        .unwrap_or_else(|_| fail(b"device-write dma"));
    let io0_bytes =
        unsafe { core::slice::from_raw_parts_mut(IO0_BASE as *mut u8, IO0_BYTES as usize) };
    let mut queue = Queue::attach(io0_bytes, IO0_SLOTS).unwrap_or_else(|_| fail(b"io0 attach"));
    let mut outstanding = Outstanding::<IO0_SLOTS>::new(queue.epoch());
    send_ready(capacity);
    debug_write(b"[virtio-blk-driver] ready capacity=");
    debug_u64(capacity);
    debug_write(b" epoch=");
    debug_u64(device.epoch);
    debug_write(b"\n");

    // Single pass, not a loop: the peer provisions its whole request set behind
    // one `request_ready` signal, `drain` settles all of it, and the only exit
    // from `await_shutdown` is the peer's shutdown command. A lost peer diverges
    // through `peer_dead`. There is no second iteration to reach.
    if slime_rt::notification_wait(request_ready).is_err() {
        peer_dead(
            &mut queue,
            &mut outstanding,
            completion_ready,
            state_changed,
        );
    }
    drain(
        &mut queue,
        &mut outstanding,
        mmio,
        queue_dma,
        data_read,
        data_write,
        capacity,
        completion_ready,
    );
    await_shutdown();
    debug_write(b"[virtio-blk-driver] peer complete, exiting\n");
    exit(0);
}

#[allow(clippy::too_many_arguments)]
fn drain(
    queue: &mut Queue<'_>,
    outstanding: &mut Outstanding<IO0_SLOTS>,
    mmio: MediatedMmio,
    queue_dma: DmaMapping,
    data_read: DmaMapping,
    data_write: DmaMapping,
    capacity: u64,
    completion_ready: u32,
) {
    let mut payload = [0u8; REQUEST_PAYLOAD_BYTES];
    loop {
        let submission = match queue.take_request(&mut payload, DATA_BYTES) {
            Ok(value) => value,
            Err(QueueError::Empty) => break,
            Err(_) => continue,
        };
        let Some(request) = WireBlockRequest::decode(&payload[..submission.payload_len]) else {
            complete(
                queue,
                submission.request_id,
                STATUS_MALFORMED,
                0,
                malformed_reply(0),
                completion_ready,
            );
            continue;
        };
        if !valid_block_v2_request(&request)
            || !slice_matches(&submission.slice, &request)
            || !valid_buffer_slice(&submission.slice, DATA_BYTES)
        {
            complete(
                queue,
                submission.request_id,
                STATUS_BAD_SLICE,
                0,
                malformed_reply(request.op),
                completion_ready,
            );
            continue;
        }
        if request.op == block_v2::OP_GEOMETRY {
            complete(
                queue,
                submission.request_id,
                STATUS_OK,
                0,
                reply(request.op, 0, DEVICE_STATUS_OK, capacity),
                completion_ready,
            );
            continue;
        }
        if request.op != block_v2::OP_FLUSH
            && request
                .lba
                .checked_add(u64::from(request.sector_count))
                .is_none_or(|end| end > capacity)
        {
            complete(
                queue,
                submission.request_id,
                STATUS_DEVICE_ERROR,
                0,
                reply(request.op, 0, DEVICE_STATUS_IO_ERR, capacity),
                completion_ready,
            );
            continue;
        }
        let mapping = if request.op == block_v2::OP_READ {
            data_write
        } else {
            data_read
        };
        outstanding
            .admit(
                submission.request_id,
                submission.slice.lease,
                submission.slice.length,
            )
            .unwrap_or_else(|_| fail(b"outstanding admit"));
        outstanding
            .start(submission.request_id)
            .unwrap_or_else(|_| fail(b"outstanding start"));
        if io_request_begin(DMA_SLOT, mapping, submission.request_id) != ERR_SUCCESS {
            settle_device_error(
                queue,
                outstanding,
                submission.request_id,
                request.op,
                completion_ready,
            );
            continue;
        }
        let outcome = execute(mmio, queue_dma, mapping, submission.slice.offset, &request);
        let _ = io_request_settle(DMA_SLOT, mapping, submission.request_id);
        let settled = outstanding
            .settle(submission.request_id, outcome.status)
            .unwrap_or_else(|_| fail(b"outstanding settle"));
        if settled.lease != submission.slice.lease {
            fail(b"lease identity");
        }
        complete(
            queue,
            submission.request_id,
            outcome.status,
            outcome.bytes,
            outcome.payload,
            completion_ready,
        );
    }
}

struct Outcome {
    status: u32,
    bytes: u64,
    payload: [u8; block_v2::REPLY_LEN],
}

fn execute(
    mmio: MediatedMmio,
    queue_dma: DmaMapping,
    data_dma: DmaMapping,
    slice_offset: u64,
    request: &WireBlockRequest,
) -> Outcome {
    let queue = unsafe {
        core::slice::from_raw_parts_mut(
            DEVICE_QUEUE_BASE as *mut u8,
            DEVICE_QUEUE_PAGES as usize * PAGE as usize,
        )
    };
    let kind = match request.op {
        block_v2::OP_READ => 0u32,
        block_v2::OP_WRITE => 1u32,
        block_v2::OP_FLUSH => 4u32,
        _ => {
            return Outcome {
                status: STATUS_UNSUPPORTED,
                bytes: 0,
                payload: reply(request.op, 0, DEVICE_STATUS_UNSUPPORTED, 0),
            };
        }
    };
    let slot = 0usize;
    let control = CONTROL_OFFSET + slot * CONTROL_STRIDE;
    queue[control..control + 4].copy_from_slice(&kind.to_le_bytes());
    queue[control + 8..control + 16].copy_from_slice(&request.lba.to_le_bytes());
    queue[control + 16] = 0xff;
    let control_iova = queue_dma.iova + control as u64;
    let bytes = request.sector_count as u64 * block_v2::SECTOR_BYTES as u64;
    if !write_descriptor(queue, 0, 0, control_iova, 16, DESC_F_NEXT, 1)
        || !write_descriptor(
            queue,
            0,
            1,
            data_dma.iova + slice_offset,
            bytes as u32,
            DESC_F_NEXT
                | if request.op == block_v2::OP_READ {
                    DESC_F_WRITE
                } else {
                    0
                },
            2,
        )
        || !write_descriptor(queue, 0, 2, control_iova + 16, 1, DESC_F_WRITE, 0)
        || !write_u16(queue, AVAIL_OFFSET + 4, 0)
        || !publish_available(queue, AVAIL_OFFSET + 2, 1)
    {
        return Outcome {
            status: STATUS_DEVICE_ERROR,
            bytes: 0,
            payload: reply(request.op, 0, DEVICE_STATUS_IO_ERR, 1),
        };
    }
    mmio.notify_queue(0);
    for _ in 0..COMPLETION_POLLS {
        if observe_used(queue, USED_OFFSET + 2).unwrap_or(0) != 0 {
            mmio.acknowledge_interrupts();
            fence(Ordering::Acquire);
            let status = unsafe { ptr::read_volatile(queue.as_ptr().add(control + 16)) };
            return if status == 0 {
                Outcome {
                    status: STATUS_OK,
                    bytes,
                    payload: reply(request.op, request.sector_count, DEVICE_STATUS_OK, 0),
                }
            } else {
                Outcome {
                    status: STATUS_DEVICE_ERROR,
                    bytes: 0,
                    payload: reply(request.op, 0, u32::from(status), 0),
                }
            };
        }
        yield_now();
    }
    mmio.fail();
    Outcome {
        status: STATUS_DEVICE_ERROR,
        bytes: 0,
        payload: reply(request.op, 0, DEVICE_STATUS_IO_ERR, 2),
    }
}

fn slice_matches(slice: &io_queue::WireBufferSlice, request: &WireBlockRequest) -> bool {
    match request.op {
        block_v2::OP_READ => {
            slice.direction == io_queue::DIRECTION_DEVICE_WRITE
                && slice.length == u64::from(request.sector_count) * block_v2::SECTOR_BYTES as u64
        }
        block_v2::OP_WRITE => {
            slice.direction == io_queue::DIRECTION_DEVICE_READ
                && slice.length == u64::from(request.sector_count) * block_v2::SECTOR_BYTES as u64
        }
        block_v2::OP_FLUSH | block_v2::OP_GEOMETRY => slice.direction == io_queue::DIRECTION_NONE,
        _ => false,
    }
}

fn settle_device_error(
    queue: &mut Queue<'_>,
    outstanding: &mut Outstanding<IO0_SLOTS>,
    request_id: u64,
    op: u8,
    completion_ready: u32,
) {
    outstanding
        .settle(request_id, STATUS_DEVICE_ERROR)
        .unwrap_or_else(|_| fail(b"begin refusal settle"));
    complete(
        queue,
        request_id,
        STATUS_DEVICE_ERROR,
        0,
        reply(op, 0, DEVICE_STATUS_IO_ERR, 3),
        completion_ready,
    );
}

fn complete(
    queue: &mut Queue<'_>,
    request_id: u64,
    status: u32,
    transferred: u64,
    payload: [u8; block_v2::REPLY_LEN],
    signal: u32,
) {
    queue
        .complete(request_id, status, transferred, &payload, false)
        .unwrap_or_else(|_| fail(b"completion publish"));
    if notification_signal(signal) != ERR_SUCCESS {
        fail(b"completion signal");
    }
}

fn reply(op: u8, sectors_done: u32, device_status: u32, detail: u64) -> [u8; block_v2::REPLY_LEN] {
    WireBlockReply {
        magic: block_v2::BLOCK_MAGIC,
        version: block_v2::FORMAT_VERSION,
        op,
        reserved: [0],
        sectors_done,
        device_status,
        detail,
    }
    .encode()
}
fn malformed_reply(op: u8) -> [u8; block_v2::REPLY_LEN] {
    reply(op, 0, DEVICE_STATUS_IO_ERR, 0)
}

fn read_capacity(mmio: MediatedMmio) -> u64 {
    let low = u64::from(mmio.read32(virtio_mmio::register::CONFIG).unwrap_or(0));
    let high = u64::from(mmio.read32(virtio_mmio::register::CONFIG + 4).unwrap_or(0));
    (high << 32) | low
}

fn receive_loans() -> (u32, u32) {
    let mut message = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    let mut loans = [0u32; 2];
    for loan in &mut loans {
        loop {
            match slime_rt::recv(PEER_SLOT, &mut message, &mut caps) {
                slime_rt::ERR_WOULDBLOCK => yield_now(),
                result if result < 0 => fail(b"loan receive"),
                _ => {
                    *loan = capability_import().unwrap_or_else(|_| fail(b"loan import"));
                    break;
                }
            }
        }
    }
    (loans[0], loans[1])
}

fn send_ready(capacity: u64) {
    loop {
        match slime_rt::send(PEER_SLOT, &capacity.to_le_bytes(), &[]) {
            slime_rt::ERR_WOULDBLOCK => yield_now(),
            ERR_SUCCESS => return,
            _ => fail(b"ready send"),
        }
    }
}

fn await_shutdown() {
    let mut message = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(PEER_SLOT, &mut message, &mut caps) {
            slime_rt::ERR_WOULDBLOCK => yield_now(),
            1 if message[0] == 1 => return,
            _ => fail(b"shutdown command"),
        }
    }
}

fn peer_dead(
    queue: &mut Queue<'_>,
    outstanding: &mut Outstanding<IO0_SLOTS>,
    completion_ready: u32,
    state_changed: u32,
) -> ! {
    queue.mark_driver_dead();
    let _ = outstanding.settle_all(io_queue::STATUS_PEER_DEAD, |entry| {
        let _ = queue.complete(entry.request_id, io_queue::STATUS_PEER_DEAD, 0, &[], true);
    });
    let _ = notification_signal(completion_ready);
    let _ = notification_signal(state_changed);
    exit(0)
}

fn binding(name: &[u8]) -> u32 {
    resolve_binding(name).unwrap_or_else(|_| fail(b"notification binding"))
}
fn fail(reason: &[u8]) -> ! {
    debug_write(b"[virtio-blk-driver] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
fn debug_u64(mut value: u64) {
    let mut bytes = [0u8; 20];
    let mut cursor = bytes.len();
    loop {
        cursor -= 1;
        bytes[cursor] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    debug_write(&bytes[cursor..]);
}
