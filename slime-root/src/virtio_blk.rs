//! A bounded virtio-mmio block driver for `slime-root` (P5.4.2b).
//!
//! Deliberately the smallest driver that can carry a sector: one virtqueue, one
//! outstanding request, one fixed data buffer. The store above it reads and
//! writes 512 bytes at a time and waits for each, so depth would buy nothing
//! and cost the correctness argument — a single in-flight request means the
//! used ring has at most one entry to interpret and no completion can be
//! attributed to the wrong caller.
//!
//! **What it does not do.** No feature negotiation beyond acknowledging the
//! handshake, no indirect descriptors, no multiqueue, no interrupt-driven
//! completion. Completion is polled on the used ring's index, because the
//! device asserts a level-triggered line that must be cleared through
//! `InterruptACK` before the handler is acknowledged, and a root task that is
//! also the IPC dispatcher has nowhere to block. [`crate::device::DeviceIrq`]
//! binds the line so a future service can wait on it; this driver does not.
//!
//! Layout: the whole queue lives in one granule, which is what bounds the queue
//! size. Descriptors, available ring, and used ring are placed at the offsets
//! below with the alignment virtio 1.0 requires, and the request header, data
//! buffer, and status byte take a second granule.

use core::ptr;
use core::sync::atomic::{Ordering, fence};

use crate::device::{DeviceError, DmaPage, MappedGranule};

/// Sector size the store contract fixes.
pub const SECTOR_BYTES: usize = 512;

/// Queue entries. Eight descriptors is three per request with room to spare,
/// and keeps the rings inside one granule with the alignment below.
const QUEUE_SIZE: usize = 8;

// Offsets within the queue page. Virtio 1.0 requires the descriptor table to be
// 16-byte aligned, the available ring 2-byte, and the used ring 4-byte; these
// are granule-relative and the page itself is granule-aligned.
const DESC_OFFSET: usize = 0;
const DESC_BYTES: usize = QUEUE_SIZE * 16;
const AVAIL_OFFSET: usize = DESC_OFFSET + DESC_BYTES;
const AVAIL_BYTES: usize = 6 + QUEUE_SIZE * 2;
const USED_OFFSET: usize = 0x800;

// Offsets within the request page.
const HEADER_OFFSET: usize = 0;
const HEADER_BYTES: usize = 16;
const DATA_OFFSET: usize = 0x200;
const STATUS_OFFSET: usize = 0x400;

/// virtio-mmio registers beyond the identifying quarter `device` names.
mod reg {
    pub const DEVICE_FEATURES: usize = 0x010;
    pub const DEVICE_FEATURES_SEL: usize = 0x014;
    pub const DRIVER_FEATURES: usize = 0x020;
    pub const DRIVER_FEATURES_SEL: usize = 0x024;
    /// Legacy only: the page size the device uses to interpret `QUEUE_PFN`.
    /// Must be written before the PFN or the device derives the wrong base.
    pub const GUEST_PAGE_SIZE: usize = 0x028;
    pub const QUEUE_SEL: usize = 0x030;
    pub const QUEUE_NUM_MAX: usize = 0x034;
    pub const QUEUE_NUM: usize = 0x038;
    pub const QUEUE_ALIGN: usize = 0x03c;
    pub const QUEUE_PFN: usize = 0x040;
    pub const QUEUE_NOTIFY: usize = 0x050;
    pub const INTERRUPT_STATUS: usize = 0x060;
    pub const INTERRUPT_ACK: usize = 0x064;
    pub const STATUS: usize = 0x070;
    pub const CONFIG: usize = 0x100;
}

mod status {
    pub const ACKNOWLEDGE: u32 = 1;
    pub const DRIVER: u32 = 2;
    pub const DRIVER_OK: u32 = 4;
    pub const FEATURES_OK: u32 = 8;
    pub const FAILED: u32 = 0x80;
}

mod request {
    pub const IN: u32 = 0;
    pub const OUT: u32 = 1;
    pub const FLUSH: u32 = 4;
    pub const STATUS_OK: u8 = 0;
}

const DESC_F_NEXT: u16 = 1;
const DESC_F_WRITE: u16 = 2;

/// Polls before a request is declared lost.
///
/// Generous against QEMU, which completes synchronously in practice, while
/// still bounding a wedged device to a failure rather than a hang. The root is
/// single-threaded, so a spin here blocks the whole graph.
const COMPLETION_POLLS: u32 = 100_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockError {
    /// The transport is not a legacy virtio-mmio device this driver speaks.
    UnsupportedVersion(u32),
    /// The device refused the feature handshake.
    FeaturesRejected,
    /// The device reports a queue too small to carry one request.
    QueueTooSmall(u32),
    /// A device or DMA resource could not be acquired.
    Resource(DeviceError),
    /// The request did not complete within [`COMPLETION_POLLS`].
    Timeout,
    /// Queue ownership became ambiguous after a timeout; only device reset may
    /// make this transport usable again.
    Poisoned,
    /// The device reported a non-zero status byte.
    Device(u8),
    /// The caller named a sector past the device's declared capacity.
    OutOfBounds { lba: u64, capacity: u64 },
}

/// A live virtio-mmio block device.
pub struct VirtioBlock {
    /// A borrowed view rather than the region itself: two transports can share
    /// one granule, so ownership of the mapping stays with the probe (B29).
    region: MappedGranule,
    offset: usize,
    queue: DmaPage,
    buffer: DmaPage,
    capacity: u64,
    /// Next available-ring index the driver will write, and the used-ring index
    /// it last observed. Equal at rest.
    avail_index: u16,
    used_index: u16,
    poisoned: bool,
}

impl VirtioBlock {
    /// Bring up the device whose registers start at `offset` within `region`.
    ///
    /// The legacy MMIO handshake, in the order the specification fixes: reset,
    /// acknowledge, driver, negotiate, features-ok, configure the queue,
    /// driver-ok. A device that refuses any step is left `FAILED` rather than
    /// half-configured.
    pub fn new(
        region: MappedGranule,
        offset: usize,
        queue: DmaPage,
        buffer: DmaPage,
    ) -> Result<Self, BlockError> {
        let version = region
            .read32(offset + crate::device::mmio::VERSION)
            .ok_or(BlockError::UnsupportedVersion(0))?;
        // Legacy (version 1) only: QEMU's `virtio-blk-device` presents that by
        // default, and the modern layout uses a different queue-address
        // register set this driver does not write.
        if version != 1 {
            return Err(BlockError::UnsupportedVersion(version));
        }
        region.write32(offset + reg::STATUS, 0);
        let mut state = status::ACKNOWLEDGE;
        region.write32(offset + reg::STATUS, state);
        state |= status::DRIVER;
        region.write32(offset + reg::STATUS, state);

        // Accept nothing from the low feature word. Every optional block
        // feature is a behaviour this driver does not implement, and the legacy
        // device works with none of them.
        region.write32(offset + reg::DEVICE_FEATURES_SEL, 0);
        let _offered = region.read32(offset + reg::DEVICE_FEATURES);
        region.write32(offset + reg::DRIVER_FEATURES_SEL, 0);
        region.write32(offset + reg::DRIVER_FEATURES, 0);
        state |= status::FEATURES_OK;
        region.write32(offset + reg::STATUS, state);
        if region.read32(offset + reg::STATUS).unwrap_or(0) & status::FEATURES_OK == 0 {
            region.write32(offset + reg::STATUS, status::FAILED);
            return Err(BlockError::FeaturesRejected);
        }

        region.write32(offset + reg::QUEUE_SEL, 0);
        let max = region.read32(offset + reg::QUEUE_NUM_MAX).unwrap_or(0);
        if (max as usize) < QUEUE_SIZE {
            region.write32(offset + reg::STATUS, status::FAILED);
            return Err(BlockError::QueueTooSmall(max));
        }
        region.write32(offset + reg::QUEUE_NUM, QUEUE_SIZE as u32);
        // Legacy queue addressing: the device is told a guest page size, a
        // page-frame number, and a ring alignment, and derives the descriptor,
        // available, and used addresses from them.
        //
        // `QUEUE_ALIGN` is `USED_OFFSET`, not the granule size. The legacy
        // layout places the used ring at the first multiple of the alignment
        // after the available ring, so aligning to 4096 would put it in the
        // *next* page — outside the one granule this queue occupies, where the
        // driver would poll an index the device never writes. That is exactly
        // the shape of a request that completes on the device and times out
        // here.
        region.write32(offset + reg::GUEST_PAGE_SIZE, GRANULE_BYTES as u32);
        region.write32(offset + reg::QUEUE_ALIGN, USED_OFFSET as u32);
        region.write32(
            offset + reg::QUEUE_PFN,
            (queue.physical_address() / GRANULE_BYTES) as u32,
        );

        state |= status::DRIVER_OK;
        region.write32(offset + reg::STATUS, state);

        // Capacity in 512-byte sectors, the first field of the block config
        // space. Read after DRIVER_OK so the device has published it.
        let low = u64::from(region.read32(offset + reg::CONFIG).unwrap_or(0));
        let high = u64::from(region.read32(offset + reg::CONFIG + 4).unwrap_or(0));
        Ok(Self {
            region,
            offset,
            queue,
            buffer,
            capacity: (high << 32) | low,
            avail_index: 0,
            used_index: 0,
            poisoned: false,
        })
    }

    /// Sectors the device reports.
    pub fn capacity_sectors(&self) -> u64 {
        self.capacity
    }

    pub fn read_sector(
        &mut self,
        lba: u64,
        out: &mut [u8; SECTOR_BYTES],
    ) -> Result<(), BlockError> {
        self.bounds(lba)?;
        self.submit(request::IN, lba)?;
        // SAFETY: the request completed, so the device is no longer writing the
        // data buffer, and no other view of the page is live.
        let bytes = unsafe { self.buffer.bytes_mut() };
        out.copy_from_slice(&bytes[DATA_OFFSET..DATA_OFFSET + SECTOR_BYTES]);
        Ok(())
    }

    pub fn write_sector(&mut self, lba: u64, data: &[u8; SECTOR_BYTES]) -> Result<(), BlockError> {
        self.bounds(lba)?;
        // SAFETY: no request is in flight, so the device is not reading the
        // buffer, and no other view of the page is live.
        let bytes = unsafe { self.buffer.bytes_mut() };
        bytes[DATA_OFFSET..DATA_OFFSET + SECTOR_BYTES].copy_from_slice(data);
        self.submit(request::OUT, lba)
    }

    /// Ask the device to make every completed write durable.
    pub fn flush(&mut self) -> Result<(), BlockError> {
        self.submit(request::FLUSH, 0)
    }

    fn bounds(&self, lba: u64) -> Result<(), BlockError> {
        (lba < self.capacity)
            .then_some(())
            .ok_or(BlockError::OutOfBounds {
                lba,
                capacity: self.capacity,
            })
    }

    /// Build the three-descriptor chain, ring the doorbell, and wait.
    ///
    /// Every request has the same shape: a device-readable 16-byte header, a
    /// data buffer whose direction depends on the request type, and a
    /// device-writable status byte. A flush carries the data descriptor too —
    /// harmless, and it keeps one chain shape rather than two.
    fn submit(&mut self, kind: u32, lba: u64) -> Result<(), BlockError> {
        if self.poisoned {
            return Err(BlockError::Poisoned);
        }
        let header_paddr = self.buffer.physical_address() + HEADER_OFFSET;
        let data_paddr = self.buffer.physical_address() + DATA_OFFSET;
        let status_paddr = self.buffer.physical_address() + STATUS_OFFSET;
        // SAFETY: no request is in flight when `submit` is entered, so the
        // device is not touching either page; the views are dropped before the
        // doorbell below hands the queue over.
        {
            let bytes = unsafe { self.buffer.bytes_mut() };
            bytes[HEADER_OFFSET..HEADER_OFFSET + 4].copy_from_slice(&kind.to_le_bytes());
            bytes[HEADER_OFFSET + 4..HEADER_OFFSET + 8].copy_from_slice(&0u32.to_le_bytes());
            bytes[HEADER_OFFSET + 8..HEADER_OFFSET + 16].copy_from_slice(&lba.to_le_bytes());
            // A status the device must overwrite: leaving the previous
            // request's zero here would make a device that wrote nothing look
            // successful.
            bytes[STATUS_OFFSET] = 0xff;
        }
        let device_writes_data = kind == request::IN;
        {
            let queue = unsafe { self.queue.bytes_mut() };
            write_descriptor(
                queue,
                0,
                header_paddr as u64,
                HEADER_BYTES as u32,
                DESC_F_NEXT,
                1,
            );
            write_descriptor(
                queue,
                1,
                data_paddr as u64,
                SECTOR_BYTES as u32,
                DESC_F_NEXT | if device_writes_data { DESC_F_WRITE } else { 0 },
                2,
            );
            write_descriptor(queue, 2, status_paddr as u64, 1, DESC_F_WRITE, 0);
            let slot = (self.avail_index as usize) % QUEUE_SIZE;
            volatile_write_u16(queue, AVAIL_OFFSET + 4 + slot * 2, 0);
            self.avail_index = self.avail_index.wrapping_add(1);
            // Descriptor and ring-entry stores must be visible before the
            // release publication of `avail.idx` and the MMIO doorbell.
            fence(Ordering::Release);
            volatile_write_u16(queue, AVAIL_OFFSET + 2, self.avail_index);
            fence(Ordering::Release);
        }
        // The device may read the rings from this point.
        self.region.write32(self.offset + reg::QUEUE_NOTIFY, 0);
        self.await_completion()?;
        // QEMU may publish the used index before the status byte becomes
        // visible to this CPU. The virtio ordering contract says both belong
        // to one completion; wait boundedly for the sentinel to be replaced
        // instead of misclassifying the transient 0xff as a device error.
        for _ in 0..COMPLETION_POLLS {
            fence(Ordering::Acquire);
            let status = unsafe {
                let bytes = self.buffer.bytes_mut();
                ptr::read_volatile(bytes.as_ptr().add(STATUS_OFFSET))
            };
            if status != 0xff {
                return if status == request::STATUS_OK {
                    Ok(())
                } else {
                    Err(BlockError::Device(status))
                };
            }
            core::hint::spin_loop();
        }
        self.poisoned = true;
        Err(BlockError::Timeout)
    }

    /// Spin until the used ring's index advances past what was last seen.
    ///
    /// The device's interrupt is bound but not serviced here: acknowledging a
    /// level-triggered line before clearing `InterruptACK` re-fires it, and the
    /// root has nowhere to block. `InterruptStatus` is cleared anyway, so a
    /// bound handler is left in a sane state.
    fn await_completion(&mut self) -> Result<(), BlockError> {
        for _ in 0..COMPLETION_POLLS {
            let index = {
                let queue = unsafe { self.queue.bytes_mut() };
                volatile_read_u16(queue, USED_OFFSET + 2)
            };
            if index != self.used_index {
                // Acquire orders the device's used-ring, status, and data
                // writes before the driver consumes any of them.
                fence(Ordering::Acquire);
                self.used_index = index;
                let pending = self
                    .region
                    .read32(self.offset + reg::INTERRUPT_STATUS)
                    .unwrap_or(0);
                if pending != 0 {
                    self.region
                        .write32(self.offset + reg::INTERRUPT_ACK, pending);
                }
                return Ok(());
            }
            core::hint::spin_loop();
        }
        // The device may still own descriptors and buffers. Fail the transport
        // permanently rather than allowing stale completion to satisfy a new
        // request that reused those bytes.
        self.poisoned = true;
        self.region
            .write32(self.offset + reg::STATUS, status::FAILED);
        Err(BlockError::Timeout)
    }
}

fn volatile_write_u16(bytes: &mut [u8], offset: usize, value: u16) {
    debug_assert!(offset + 2 <= bytes.len());
    // SAFETY: bounds are established above and byte pointers permit unaligned
    // accesses; each byte is volatile because the peer is an asynchronous DMA
    // device rather than Rust code.
    unsafe {
        ptr::write_volatile(bytes.as_mut_ptr().add(offset), value as u8);
        ptr::write_volatile(bytes.as_mut_ptr().add(offset + 1), (value >> 8) as u8);
    }
}

fn volatile_read_u16(bytes: &mut [u8], offset: usize) -> u16 {
    debug_assert!(offset + 2 <= bytes.len());
    unsafe {
        u16::from(ptr::read_volatile(bytes.as_ptr().add(offset)))
            | (u16::from(ptr::read_volatile(bytes.as_ptr().add(offset + 1))) << 8)
    }
}

fn write_descriptor(queue: &mut [u8], index: usize, addr: u64, len: u32, flags: u16, next: u16) {
    let base = DESC_OFFSET + index * 16;
    for (offset, byte) in addr
        .to_le_bytes()
        .into_iter()
        .chain(len.to_le_bytes())
        .chain(flags.to_le_bytes())
        .chain(next.to_le_bytes())
        .enumerate()
    {
        unsafe { ptr::write_volatile(queue.as_mut_ptr().add(base + offset), byte) };
    }
}

const GRANULE_BYTES: usize = 4096;

// The queue layout must fit one granule with the used ring at its own offset,
// and the request page's three regions must not overlap. Stated as asserts so a
// changed `QUEUE_SIZE` fails to compile rather than corrupting a ring.
const _: () = assert!(AVAIL_OFFSET + AVAIL_BYTES <= USED_OFFSET);
const _: () = assert!(USED_OFFSET + 6 + QUEUE_SIZE * 8 <= GRANULE_BYTES);
const _: () = assert!(HEADER_OFFSET + HEADER_BYTES <= DATA_OFFSET);
const _: () = assert!(DATA_OFFSET + SECTOR_BYTES <= STATUS_OFFSET);
const _: () = assert!(STATUS_OFFSET < GRANULE_BYTES);
