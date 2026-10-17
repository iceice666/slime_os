#![no_std]
#![no_main]

//! A bounded userspace virtio-rng driver serving `contracts/entropy-source/v1`
//! to the entropy service over its one declared endpoint.
//!
//! Each fill places one device-writable descriptor in the queue pages and
//! answers with exactly the bytes the device reported writing. Without the
//! device every fill is answered `unavailable`: this driver never invents a
//! byte, and it never prints one.

use core::ptr;
use core::sync::atomic::{Ordering, fence};

use slime_components::virtio_mmio::{
    DESC_F_WRITE, DESCRIPTOR_BYTES, MediatedHandshake, MediatedMmio, TransportError, observe_used,
    publish_available, read_u32, used_ring_progress, write_descriptor, write_u16,
};
use slime_proto::entropy_source::{
    self, ENTROPY_SOURCE_MAGIC, MAX_FILL, OP_FILL, OP_RELEASE, STATUS_DEVICE_ERROR,
    STATUS_MALFORMED, STATUS_OK, STATUS_UNAVAILABLE, WireEntropySourceReply,
    WireEntropySourceRequest,
};
use slime_proto::valid_entropy_source_request;
use slime_rt::{
    MAX_CAPS_PER_MSG, MAX_MSG, debug_write, exit, io_device_bind, io_queue_map, recv_blocking,
    yield_now,
};

slime_rt::entry!(main);

const PEER_SLOT: u32 = 0;
const DEVICE_SLOT: u32 = 1;
const MMIO_SLOT: u32 = 2;
// Slot 3 holds the `virtio-rng-interrupt` grant. Completion is polled, so the
// line is bound by the composition but never waited on.
const DMA_SLOT: u32 = 4;
const VIRTIO_DEVICE_RNG: u32 = 4;
const REQUEST_QUEUE: u16 = 0;
const QUEUE_BASE: u64 = 0x0000_0020_0003_0000;
const PAGE: u64 = 4096;
const QUEUE_PAGES: u32 = 2;
const QUEUE_SIZE: usize = 8;
const AVAIL_OFFSET: usize = QUEUE_SIZE * DESCRIPTOR_BYTES;
const USED_OFFSET: usize = 0x1000;
// The device-writable buffer shares the bidirectional queue mapping, after the
// used ring, so no payload mapping is needed.
const BUFFER_OFFSET: usize = 0x1800;
const COMPLETION_POLLS: u32 = 10_000_000;
/// Device completions one fill may take. A device may legitimately return
/// fewer bytes than offered; one that keeps returning none is failed.
const MAX_ROUNDS: usize = MAX_FILL + 8;

fn main(_startup_arg: u32) {
    let device = io_device_bind(DEVICE_SLOT).unwrap_or_else(|_| fail(b"device bind"));
    let mmio = MediatedMmio::new(DEVICE_SLOT, MMIO_SLOT, device.epoch);
    let queue_dma = io_queue_map(DMA_SLOT, device.epoch, QUEUE_BASE, QUEUE_PAGES)
        .unwrap_or_else(|_| fail(b"queue map"));
    let mut rng = match mmio.begin(VIRTIO_DEVICE_RNG) {
        Ok(handshake) => Some(Rng::start(handshake, queue_dma.iova)),
        Err(
            TransportError::BadMapping
            | TransportError::WrongDevice { .. }
            | TransportError::BadMagic(_)
            | TransportError::UnsupportedVersion(_),
        ) => {
            debug_write(b"[virtio-rng-driver] device absent\n");
            None
        }
        Err(_) => fail(b"virtio handshake"),
    };
    if rng.is_some() {
        debug_write(b"[virtio-rng-driver] device ready\n");
    }
    let mut message = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        let received = recv_blocking(PEER_SLOT, &mut message, &mut caps);
        if received < 0 {
            fail(b"receive");
        }
        let request = (received as usize == entropy_source::REQUEST_LEN)
            .then(|| WireEntropySourceRequest::decode(&message[..entropy_source::REQUEST_LEN]))
            .flatten()
            .filter(valid_entropy_source_request);
        let Some(request) = request else {
            answer(STATUS_MALFORMED, &[]);
            continue;
        };
        match request.op {
            OP_FILL => {
                let mut bytes = [0u8; MAX_FILL];
                let wanted = &mut bytes[..request.length as usize];
                match rng.as_mut() {
                    None => answer(STATUS_UNAVAILABLE, &[]),
                    Some(device) => match device.fill(wanted) {
                        Ok(()) => answer(STATUS_OK, wanted),
                        Err(()) => answer(STATUS_DEVICE_ERROR, &[]),
                    },
                }
            }
            OP_RELEASE => {
                answer(STATUS_OK, &[]);
                break;
            }
            _ => answer(STATUS_MALFORMED, &[]),
        }
    }
    if let Some(device) = rng {
        device.mmio.reset();
    }
    exit(0);
}

/// The negotiated device and its one request queue.
struct Rng {
    mmio: MediatedMmio,
    iova: u64,
    /// Free-running available and used indices; one descriptor is ever in
    /// flight, so the two differ by at most one.
    avail: u16,
    used: u16,
    /// Set once the device has broken the ring's rules; it is never driven
    /// again.
    failed: bool,
}

impl Rng {
    fn start(handshake: MediatedHandshake, iova: u64) -> Self {
        handshake
            .configure_queue(
                REQUEST_QUEUE,
                QUEUE_SIZE as u16,
                PAGE as u32,
                PAGE as u32,
                iova,
            )
            .unwrap_or_else(|_| fail(b"virtqueue setup"));
        Self {
            mmio: handshake.finish(),
            iova,
            avail: 0,
            used: 0,
            failed: false,
        }
    }

    fn queue() -> &'static mut [u8] {
        unsafe {
            core::slice::from_raw_parts_mut(
                QUEUE_BASE as *mut u8,
                QUEUE_PAGES as usize * PAGE as usize,
            )
        }
    }

    /// Fill `out` entirely with device bytes, over as many completions as the
    /// device needs.
    fn fill(&mut self, out: &mut [u8]) -> Result<(), ()> {
        if self.failed {
            return Err(());
        }
        let mut filled = 0;
        let mut rounds = 0;
        while filled < out.len() {
            rounds += 1;
            let step = if rounds > MAX_ROUNDS {
                Err(())
            } else {
                self.one(&mut out[filled..])
            };
            match step {
                Ok(written) => filled += written,
                Err(()) => {
                    self.failed = true;
                    self.mmio.fail();
                    out.fill(0);
                    return Err(());
                }
            }
        }
        Ok(())
    }

    /// Offer `out.len()` device-writable bytes and copy back what the device
    /// reports writing, which may be fewer.
    fn one(&mut self, out: &mut [u8]) -> Result<usize, ()> {
        let queue = Self::queue();
        let offered = out.len();
        queue[BUFFER_OFFSET..BUFFER_OFFSET + offered].fill(0);
        let next = self.avail.wrapping_add(1);
        if !write_descriptor(
            queue,
            0,
            0,
            self.iova + BUFFER_OFFSET as u64,
            offered as u32,
            DESC_F_WRITE,
            0,
        ) || !write_u16(
            queue,
            AVAIL_OFFSET + 4 + 2 * (usize::from(self.avail) % QUEUE_SIZE),
            0,
        ) || !publish_available(queue, AVAIL_OFFSET + 2, next)
        {
            return Err(());
        }
        self.avail = next;
        self.mmio.notify_queue(REQUEST_QUEUE);
        for _ in 0..COMPLETION_POLLS {
            let published = observe_used(queue, USED_OFFSET + 2).ok_or(())?;
            match used_ring_progress(published, self.used, 1) {
                None => return Err(()),
                Some(0) => yield_now(),
                Some(_) => {
                    let entry = USED_OFFSET + 4 + (usize::from(self.used) % QUEUE_SIZE) * 8;
                    let id = read_u32(queue, entry).ok_or(())?;
                    let written = read_u32(queue, entry + 4).ok_or(())? as usize;
                    self.used = self.used.wrapping_add(1);
                    self.mmio.acknowledge_interrupts();
                    if id != 0 || written > offered {
                        return Err(());
                    }
                    fence(Ordering::Acquire);
                    for (index, byte) in out[..written].iter_mut().enumerate() {
                        *byte = unsafe {
                            ptr::read_volatile(queue.as_ptr().add(BUFFER_OFFSET + index))
                        };
                    }
                    return Ok(written);
                }
            }
        }
        Err(())
    }
}

fn answer(status: i32, bytes: &[u8]) {
    let mut reply = WireEntropySourceReply {
        magic: ENTROPY_SOURCE_MAGIC,
        version: entropy_source::FORMAT_VERSION,
        status,
        length: bytes.len() as u32,
        bytes: [0; MAX_FILL],
    };
    reply.bytes[..bytes.len()].copy_from_slice(bytes);
    let _ = slime_rt::reply(&reply.encode());
}

fn fail(reason: &[u8]) -> ! {
    debug_write(b"[virtio-rng-driver] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
