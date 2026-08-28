//! Shared legacy virtio-mmio transport mechanics for userspace drivers.
//!
//! Device semantics and queue policy deliberately do not live here. Block and
//! network drivers own descriptor-chain shapes, depths, timeouts, and completion
//! attribution. This module owns volatile register access, legacy negotiation,
//! queue PFN setup, interrupt acknowledgement, and packed ring primitives.

use core::ptr;
use core::sync::atomic::{Ordering, fence};

pub const MAGIC_VALUE: u32 = 0x7472_6976;
pub const LEGACY_VERSION: u32 = 1;

pub mod register {
    pub const MAGIC_VALUE: usize = 0x000;
    pub const VERSION: usize = 0x004;
    pub const DEVICE_ID: usize = 0x008;
    pub const DEVICE_FEATURES: usize = 0x010;
    pub const DEVICE_FEATURES_SEL: usize = 0x014;
    pub const DRIVER_FEATURES: usize = 0x020;
    pub const DRIVER_FEATURES_SEL: usize = 0x024;
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

pub mod device_status {
    pub const ACKNOWLEDGE: u32 = 1;
    pub const DRIVER: u32 = 2;
    pub const DRIVER_OK: u32 = 4;
    pub const FEATURES_OK: u32 = 8;
    pub const FAILED: u32 = 0x80;
}

pub const DESC_F_NEXT: u16 = 1;
pub const DESC_F_WRITE: u16 = 2;
pub const DESCRIPTOR_BYTES: usize = 16;

/// Root-mediated bounded access to a packed virtio-mmio transport.
///
/// QEMU places several 0x200-byte transports in one page. Mapping that page
/// into a child would expose adjacent devices, so each register access instead
/// crosses IO1 and is checked against the declared transport length.
#[derive(Clone, Copy)]
pub struct MediatedMmio {
    device_slot: u32,
    region_slot: u32,
    epoch: u64,
}

impl MediatedMmio {
    pub const fn new(device_slot: u32, region_slot: u32, epoch: u64) -> Self {
        Self {
            device_slot,
            region_slot,
            epoch,
        }
    }

    pub fn read32(self, offset: usize) -> Option<u32> {
        slime_rt::io_mmio_read32(
            self.device_slot,
            self.region_slot,
            self.epoch,
            u32::try_from(offset).ok()?,
        )
        .ok()
    }

    pub fn write32(self, offset: usize, value: u32) -> bool {
        let Ok(offset) = u32::try_from(offset) else {
            return false;
        };
        slime_rt::io_mmio_write32(
            self.device_slot,
            self.region_slot,
            self.epoch,
            offset,
            value,
        ) == slime_rt::ERR_SUCCESS
    }

    pub fn begin(self, expected_device: u32) -> Result<MediatedHandshake, TransportError> {
        let magic = self
            .read32(register::MAGIC_VALUE)
            .ok_or(TransportError::BadMapping)?;
        if magic != MAGIC_VALUE {
            return Err(TransportError::BadMagic(magic));
        }
        let version = self
            .read32(register::VERSION)
            .ok_or(TransportError::BadMapping)?;
        if version != LEGACY_VERSION {
            return Err(TransportError::UnsupportedVersion(version));
        }
        let observed = self
            .read32(register::DEVICE_ID)
            .ok_or(TransportError::BadMapping)?;
        if observed != expected_device {
            return Err(TransportError::WrongDevice {
                expected: expected_device,
                observed,
            });
        }
        if !self.write32(register::STATUS, 0) {
            return Err(TransportError::BadMapping);
        }
        let mut status = device_status::ACKNOWLEDGE;
        self.write32(register::STATUS, status);
        status |= device_status::DRIVER;
        self.write32(register::STATUS, status);
        self.write32(register::DEVICE_FEATURES_SEL, 0);
        let offered_low = self
            .read32(register::DEVICE_FEATURES)
            .ok_or(TransportError::BadMapping)?;
        self.write32(register::DRIVER_FEATURES_SEL, 0);
        self.write32(register::DRIVER_FEATURES, 0);
        status |= device_status::FEATURES_OK;
        self.write32(register::STATUS, status);
        if self.read32(register::STATUS).unwrap_or(0) & device_status::FEATURES_OK == 0 {
            self.fail();
            return Err(TransportError::FeaturesRejected);
        }
        Ok(MediatedHandshake {
            mmio: self,
            status,
            offered_low,
        })
    }

    pub fn acknowledge_interrupts(self) -> u32 {
        let pending = self.read32(register::INTERRUPT_STATUS).unwrap_or(0);
        if pending != 0 {
            fence(Ordering::Acquire);
            self.write32(register::INTERRUPT_ACK, pending);
        }
        pending
    }

    pub fn notify_queue(self, queue: u16) {
        fence(Ordering::Release);
        self.write32(register::QUEUE_NOTIFY, u32::from(queue));
    }

    pub fn fail(self) {
        self.write32(register::STATUS, device_status::FAILED);
    }
    pub fn reset(self) {
        self.write32(register::STATUS, 0);
    }
}

pub struct MediatedHandshake {
    mmio: MediatedMmio,
    status: u32,
    offered_low: u32,
}

impl MediatedHandshake {
    pub const fn offered_low(&self) -> u32 {
        self.offered_low
    }

    pub fn configure_queue(
        &self,
        queue: u16,
        size: u16,
        page_bytes: u32,
        alignment: u32,
        queue_iova: u64,
    ) -> Result<(), TransportError> {
        if page_bytes == 0
            || !page_bytes.is_power_of_two()
            || !queue_iova.is_multiple_of(u64::from(page_bytes))
        {
            return Err(TransportError::BadQueueAddress);
        }
        self.mmio.write32(register::QUEUE_SEL, u32::from(queue));
        let available = self
            .mmio
            .read32(register::QUEUE_NUM_MAX)
            .ok_or(TransportError::BadMapping)?;
        if available < u32::from(size) {
            self.mmio.fail();
            return Err(TransportError::QueueTooSmall {
                required: size,
                available,
            });
        }
        let pfn = u32::try_from(queue_iova / u64::from(page_bytes))
            .map_err(|_| TransportError::BadQueueAddress)?;
        self.mmio.write32(register::QUEUE_NUM, u32::from(size));
        self.mmio.write32(register::GUEST_PAGE_SIZE, page_bytes);
        self.mmio.write32(register::QUEUE_ALIGN, alignment);
        self.mmio.write32(register::QUEUE_PFN, pfn);
        Ok(())
    }

    pub fn finish(mut self) -> MediatedMmio {
        self.status |= device_status::DRIVER_OK;
        self.mmio.write32(register::STATUS, self.status);
        self.mmio
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportError {
    BadMapping,
    BadMagic(u32),
    UnsupportedVersion(u32),
    WrongDevice { expected: u32, observed: u32 },
    FeaturesRejected,
    QueueTooSmall { required: u16, available: u32 },
    BadQueueAddress,
}

/// A userspace mapping of one virtio-mmio register bank.
///
/// # Safety
/// `base..base+length` must remain a live device mapping for this value's
/// lifetime and must not be accessed through non-volatile operations.
#[derive(Clone, Copy)]
pub struct Mmio {
    base: *mut u8,
    length: usize,
}

impl Mmio {
    /// Adopt an existing register-bank mapping.
    ///
    /// # Safety
    ///
    /// `base..base + length` must be a live device mapping for as long as this
    /// value (or any copy of it — `Mmio` is `Copy`) is used, and `base` must be
    /// suitably aligned for the register widths accessed through it. Reads and
    /// writes go straight to the device, so the caller must also be the party
    /// authorised to drive that bank: two holders touching the same registers
    /// interleave device state machines in ways the compiler cannot see.
    pub const unsafe fn from_raw_parts(base: *mut u8, length: usize) -> Self {
        Self { base, length }
    }

    pub fn read32(self, offset: usize) -> Option<u32> {
        self.range(offset, 4)?;
        Some(unsafe { ptr::read_volatile(self.base.add(offset).cast::<u32>()) })
    }

    pub fn write32(self, offset: usize, value: u32) -> bool {
        if self.range(offset, 4).is_none() {
            return false;
        }
        unsafe { ptr::write_volatile(self.base.add(offset).cast::<u32>(), value) };
        true
    }

    fn range(self, offset: usize, bytes: usize) -> Option<()> {
        offset
            .checked_add(bytes)
            .filter(|end| *end <= self.length)
            .map(|_| ())
    }

    pub fn identity(self, expected_device: u32) -> Result<(), TransportError> {
        let magic = self
            .read32(register::MAGIC_VALUE)
            .ok_or(TransportError::BadMapping)?;
        if magic != MAGIC_VALUE {
            return Err(TransportError::BadMagic(magic));
        }
        let version = self
            .read32(register::VERSION)
            .ok_or(TransportError::BadMapping)?;
        if version != LEGACY_VERSION {
            return Err(TransportError::UnsupportedVersion(version));
        }
        let observed = self
            .read32(register::DEVICE_ID)
            .ok_or(TransportError::BadMapping)?;
        if observed != expected_device {
            return Err(TransportError::WrongDevice {
                expected: expected_device,
                observed,
            });
        }
        Ok(())
    }

    /// Reset and negotiate a legacy device while accepting no optional feature.
    pub fn begin(self, expected_device: u32) -> Result<Handshake, TransportError> {
        self.identity(expected_device)?;
        if !self.write32(register::STATUS, 0) {
            return Err(TransportError::BadMapping);
        }
        let mut status = device_status::ACKNOWLEDGE;
        self.write32(register::STATUS, status);
        status |= device_status::DRIVER;
        self.write32(register::STATUS, status);
        self.write32(register::DEVICE_FEATURES_SEL, 0);
        let offered_low = self
            .read32(register::DEVICE_FEATURES)
            .ok_or(TransportError::BadMapping)?;
        self.write32(register::DRIVER_FEATURES_SEL, 0);
        self.write32(register::DRIVER_FEATURES, 0);
        status |= device_status::FEATURES_OK;
        self.write32(register::STATUS, status);
        if self.read32(register::STATUS).unwrap_or(0) & device_status::FEATURES_OK == 0 {
            self.fail();
            return Err(TransportError::FeaturesRejected);
        }
        Ok(Handshake {
            mmio: self,
            status,
            offered_low,
        })
    }

    pub fn acknowledge_interrupts(self) -> u32 {
        let pending = self.read32(register::INTERRUPT_STATUS).unwrap_or(0);
        if pending != 0 {
            fence(Ordering::Acquire);
            self.write32(register::INTERRUPT_ACK, pending);
        }
        pending
    }

    pub fn notify_queue(self, queue: u16) {
        fence(Ordering::Release);
        self.write32(register::QUEUE_NOTIFY, u32::from(queue));
    }

    pub fn fail(self) {
        self.write32(register::STATUS, device_status::FAILED);
    }
    pub fn reset(self) {
        self.write32(register::STATUS, 0);
    }
}

pub struct Handshake {
    mmio: Mmio,
    status: u32,
    offered_low: u32,
}

impl Handshake {
    pub const fn offered_low(&self) -> u32 {
        self.offered_low
    }

    /// Configure one legacy PFN queue.
    ///
    /// The queue page contains device-readable descriptors/available ring and a
    /// device-writable used ring, so IO1 must map it bidirectionally. Payload
    /// mappings remain scoped to each operation's direction.
    pub fn configure_queue(
        &self,
        queue: u16,
        size: u16,
        page_bytes: u32,
        alignment: u32,
        queue_iova: u64,
    ) -> Result<(), TransportError> {
        if page_bytes == 0
            || !page_bytes.is_power_of_two()
            || !queue_iova.is_multiple_of(u64::from(page_bytes))
        {
            return Err(TransportError::BadQueueAddress);
        }
        self.mmio.write32(register::QUEUE_SEL, u32::from(queue));
        let available = self
            .mmio
            .read32(register::QUEUE_NUM_MAX)
            .ok_or(TransportError::BadMapping)?;
        if available < u32::from(size) {
            self.mmio.fail();
            return Err(TransportError::QueueTooSmall {
                required: size,
                available,
            });
        }
        let pfn = u32::try_from(queue_iova / u64::from(page_bytes))
            .map_err(|_| TransportError::BadQueueAddress)?;
        self.mmio.write32(register::QUEUE_NUM, u32::from(size));
        self.mmio.write32(register::GUEST_PAGE_SIZE, page_bytes);
        self.mmio.write32(register::QUEUE_ALIGN, alignment);
        self.mmio.write32(register::QUEUE_PFN, pfn);
        Ok(())
    }

    pub fn finish(mut self) -> Mmio {
        self.status |= device_status::DRIVER_OK;
        self.mmio.write32(register::STATUS, self.status);
        self.mmio
    }
}

pub fn write_descriptor(
    queue: &mut [u8],
    table_offset: usize,
    index: usize,
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
) -> bool {
    let Some(base) = index
        .checked_mul(DESCRIPTOR_BYTES)
        .and_then(|n| table_offset.checked_add(n))
    else {
        return false;
    };
    let Some(end) = base.checked_add(DESCRIPTOR_BYTES) else {
        return false;
    };
    if end > queue.len() {
        return false;
    }
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
    true
}

pub fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let high = offset.checked_add(1)?;
    if high >= bytes.len() {
        return None;
    }
    Some(unsafe {
        u16::from(ptr::read_volatile(bytes.as_ptr().add(offset)))
            | (u16::from(ptr::read_volatile(bytes.as_ptr().add(high))) << 8)
    })
}

pub fn write_u16(bytes: &mut [u8], offset: usize, value: u16) -> bool {
    let Some(high) = offset.checked_add(1) else {
        return false;
    };
    if high >= bytes.len() {
        return false;
    }
    unsafe {
        ptr::write_volatile(bytes.as_mut_ptr().add(offset), value as u8);
        ptr::write_volatile(bytes.as_mut_ptr().add(high), (value >> 8) as u8);
    }
    true
}

pub fn publish_available(bytes: &mut [u8], index_offset: usize, index: u16) -> bool {
    fence(Ordering::Release);
    write_u16(bytes, index_offset, index)
}

pub fn observe_used(bytes: &[u8], index_offset: usize) -> Option<u16> {
    let index = read_u16(bytes, index_offset)?;
    fence(Ordering::Acquire);
    Some(index)
}
