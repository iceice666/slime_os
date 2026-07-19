//! Modern virtio PCI block transport for the M5.2 read-only vertical slice.
//!
//! The transport deliberately negotiates no optional feature bits and uses one
//! bounded split virtqueue. Requests are synchronous and polled with a timeout;
//! interrupts are not required for the first deterministic QEMU slice.

use core::mem::{align_of, size_of};
use core::ptr::{read_volatile, write_volatile};

use crate::block_proto::SECTOR_SIZE;
use crate::capability::PciFunctionInfo;
use crate::dma::{DMA_TABLE, DmaError};
use crate::memory::vmm::{self, MapError, PTE_CACHE_DISABLE, PTE_NO_EXECUTE, PTE_WRITABLE};
use crate::memory::{PAGE_SIZE, PhysAddr, VirtAddr, align_down, align_up};
use crate::pci::{self, BarInfo, BarKind, PciError};

const VIRTIO_VENDOR_ID: u16 = 0x1af4;
const VIRTIO_BLOCK_DEVICE_ID: u16 = 0x1042;
const VIRTIO_PCI_CAP_ID: u8 = 0x09;

const VIRTIO_PCI_CAP_COMMON_CFG: u8 = 1;
const VIRTIO_PCI_CAP_NOTIFY_CFG: u8 = 2;
const VIRTIO_PCI_CAP_DEVICE_CFG: u8 = 4;
const STATUS_ACKNOWLEDGE: u8 = 1;
const STATUS_DRIVER: u8 = 2;
const STATUS_DRIVER_OK: u8 = 4;
const STATUS_FEATURES_OK: u8 = 8;
const STATUS_FAILED: u8 = 128;

const VIRTIO_F_VERSION_1_HIGH: u32 = 1;
const VIRTQ_DESC_F_NEXT: u16 = 1;
const VIRTQ_DESC_F_WRITE: u16 = 2;
const QUEUE_INDEX: u16 = 0;
const QUEUE_SIZE: u16 = 8;

const VIRTIO_BLK_T_IN: u32 = 0;
const VIRTIO_BLK_S_OK: u8 = 0;

const COMMON_DEVICE_FEATURE_SELECT: usize = 0x00;
const COMMON_DEVICE_FEATURE: usize = 0x04;
const COMMON_DRIVER_FEATURE_SELECT: usize = 0x08;
const COMMON_DRIVER_FEATURE: usize = 0x0c;
const COMMON_DEVICE_STATUS: usize = 0x14;
const COMMON_QUEUE_SELECT: usize = 0x16;
const COMMON_QUEUE_SIZE: usize = 0x18;
const COMMON_QUEUE_ENABLE: usize = 0x1c;
const COMMON_QUEUE_NOTIFY_OFF: usize = 0x1e;
const COMMON_QUEUE_DESC: usize = 0x20;
const COMMON_QUEUE_DRIVER: usize = 0x28;
const COMMON_QUEUE_DEVICE: usize = 0x30;

const MMIO_SCRATCH_BASE: u64 = 0xffff_c100_0000_0000;
const MAX_MMIO_PAGES: usize = 64;
const MAX_CAPABILITIES: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtioBlkError {
    DeviceNotFound,
    Pci(PciError),
    Map(MapError),
    BadCapability,
    MissingCommonConfig,
    MissingNotifyConfig,
    MissingDeviceConfig,
    UnsupportedFeatures,
    QueueUnavailable,
    QueueTooSmall,
    QueueAddress,
    Capacity,
    OutOfRange,
    BufferSize,
    Dma(DmaError),
    Timeout,
    ResetTimeout,
    DeviceStatus(u8),
}

impl From<PciError> for VirtioBlkError {
    fn from(value: PciError) -> Self {
        Self::Pci(value)
    }
}

impl From<MapError> for VirtioBlkError {
    fn from(value: MapError) -> Self {
        Self::Map(value)
    }
}

impl From<DmaError> for VirtioBlkError {
    fn from(value: DmaError) -> Self {
        Self::Dma(value)
    }
}

#[derive(Debug, Clone, Copy)]
struct VirtioCapability {
    cfg_type: u8,
    bar: u8,
    offset: u32,
    length: u32,
    notify_multiplier: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VirtqDescriptor {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

#[repr(C)]
struct VirtqAvail {
    flags: u16,
    idx: u16,
    ring: [u16; QUEUE_SIZE as usize],
    used_event: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VirtqUsedElem {
    id: u32,
    len: u32,
}

#[repr(C)]
struct VirtqUsed {
    flags: u16,
    idx: u16,
    ring: [VirtqUsedElem; QUEUE_SIZE as usize],
    avail_event: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VirtioBlkRequestHeader {
    request_type: u32,
    reserved: u32,
    sector: u64,
}

#[repr(C)]
struct QueueMemory {
    descriptors: [VirtqDescriptor; QUEUE_SIZE as usize],
    avail: VirtqAvail,
    used: VirtqUsed,
    header: VirtioBlkRequestHeader,
    status: u8,
}

pub struct VirtioBlock {
    common: MmioRegion,
    notify: MmioRegion,
    notify_multiplier: u32,
    queue_notify_offset: u16,
    queue_dma: crate::capability::DmaRegion,
    last_used: u16,
    capacity_sectors: u64,
}

impl VirtioBlock {
    pub fn find_and_init() -> Result<Self, VirtioBlkError> {
        let functions = pci::enumerate()?;
        let function = functions
            .iter()
            .find(|function| {
                function.vendor_id == VIRTIO_VENDOR_ID
                    && function.device_id == VIRTIO_BLOCK_DEVICE_ID
            })
            .copied()
            .ok_or(VirtioBlkError::DeviceNotFound)?;
        Self::init(function)
    }

    pub fn init(function: PciFunctionInfo) -> Result<Self, VirtioBlkError> {
        let bars = pci::probe_bars(&function)?;
        enable_function(&function)?;
        let capabilities = read_virtio_capabilities(&function)?;
        let common_cap = find_capability(&capabilities, VIRTIO_PCI_CAP_COMMON_CFG)
            .ok_or(VirtioBlkError::MissingCommonConfig)?;
        let notify_cap = find_capability(&capabilities, VIRTIO_PCI_CAP_NOTIFY_CFG)
            .ok_or(VirtioBlkError::MissingNotifyConfig)?;
        let device_cap = find_capability(&capabilities, VIRTIO_PCI_CAP_DEVICE_CFG)
            .ok_or(VirtioBlkError::MissingDeviceConfig)?;

        let common = map_capability(common_cap, &bars, 0)?;
        let notify = map_capability(notify_cap, &bars, 1)?;
        let device = map_capability(device_cap, &bars, 2)?;

        common.write_u8(COMMON_DEVICE_STATUS, 0);
        common.write_u8(COMMON_DEVICE_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);

        common.write_u32(COMMON_DEVICE_FEATURE_SELECT, 1);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        let high_features = common.read_u32(COMMON_DEVICE_FEATURE);
        if high_features & VIRTIO_F_VERSION_1_HIGH == 0 {
            common.write_u8(COMMON_DEVICE_STATUS, STATUS_FAILED);
            return Err(VirtioBlkError::UnsupportedFeatures);
        }
        common.write_u32(COMMON_DRIVER_FEATURE_SELECT, 0);
        common.write_u32(COMMON_DRIVER_FEATURE, 0);
        common.write_u32(COMMON_DRIVER_FEATURE_SELECT, 1);
        common.write_u32(COMMON_DRIVER_FEATURE, VIRTIO_F_VERSION_1_HIGH);
        common.write_u8(
            COMMON_DEVICE_STATUS,
            STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK,
        );
        if common.read_u8(COMMON_DEVICE_STATUS) & STATUS_FEATURES_OK == 0 {
            common.write_u8(COMMON_DEVICE_STATUS, STATUS_FAILED);
            return Err(VirtioBlkError::UnsupportedFeatures);
        }

        common.write_u16(COMMON_QUEUE_SELECT, QUEUE_INDEX);
        let available = common.read_u16(COMMON_QUEUE_SIZE);
        if available == 0 {
            return Err(VirtioBlkError::QueueUnavailable);
        }
        if available < QUEUE_SIZE {
            return Err(VirtioBlkError::QueueTooSmall);
        }
        common.write_u16(COMMON_QUEUE_SIZE, QUEUE_SIZE);

        let queue_pages = size_of::<QueueMemory>().div_ceil(PAGE_SIZE);
        let queue_dma = DMA_TABLE.lock().pin(queue_pages)?;
        zero_region(queue_dma.phys(), queue_dma.pages());
        let base = queue_dma.phys().as_u64();
        let desc_offset = offset_of_queue_descriptors();
        let avail_offset = offset_of_queue_avail();
        let used_offset = offset_of_queue_used();
        common.write_u64(COMMON_QUEUE_DESC, base + desc_offset as u64);
        common.write_u64(COMMON_QUEUE_DRIVER, base + avail_offset as u64);
        common.write_u64(COMMON_QUEUE_DEVICE, base + used_offset as u64);
        common.write_u16(COMMON_QUEUE_ENABLE, 1);
        if common.read_u16(COMMON_QUEUE_ENABLE) != 1 {
            common.write_u8(COMMON_DEVICE_STATUS, 0);
            DMA_TABLE.lock().release(&queue_dma)?;
            return Err(VirtioBlkError::QueueAddress);
        }
        let queue_notify_offset = common.read_u16(COMMON_QUEUE_NOTIFY_OFF);
        let capacity_sectors = device.read_u64(0);
        if capacity_sectors == 0 {
            common.write_u8(COMMON_DEVICE_STATUS, 0);
            DMA_TABLE.lock().release(&queue_dma)?;
            return Err(VirtioBlkError::Capacity);
        }

        common.write_u8(
            COMMON_DEVICE_STATUS,
            STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK,
        );
        Ok(Self {
            common,
            notify,
            notify_multiplier: notify_cap.notify_multiplier,
            queue_notify_offset,
            queue_dma,
            last_used: 0,
            capacity_sectors,
        })
    }

    pub fn capacity_sectors(&self) -> u64 {
        self.capacity_sectors
    }

    pub fn read_sector(&mut self, lba: u64, output: &mut [u8]) -> Result<(), VirtioBlkError> {
        if output.len() != SECTOR_SIZE {
            return Err(VirtioBlkError::BufferSize);
        }
        if lba >= self.capacity_sectors {
            return Err(VirtioBlkError::OutOfRange);
        }

        let data_dma = DMA_TABLE.lock().pin(1)?;
        zero_region(data_dma.phys(), data_dma.pages());
        data_dma.set_outstanding(true);
        self.queue_dma.set_outstanding(true);

        let result = self.submit_read(lba, &data_dma);
        if result.is_err() {
            // A failed or timed-out request may still be visible to the device.
            // Reset first so no DMA write can race with page reclamation.
            if self.reset().is_err() {
                // Preserve the pins if reset cannot be confirmed; reclaiming
                // them could expose unrelated future allocations to DMA.
                return Err(VirtioBlkError::ResetTimeout);
            }
        }
        if result.is_ok() {
            let source = data_dma.phys().to_virt().as_mut_ptr::<u8>();
            // SAFETY: the device completed the request before this copy.
            unsafe { core::ptr::copy_nonoverlapping(source, output.as_mut_ptr(), output.len()) };
        }

        data_dma.set_outstanding(false);
        self.queue_dma.set_outstanding(false);
        DMA_TABLE.lock().release(&data_dma)?;
        result
    }

    fn reset(&self) -> Result<(), VirtioBlkError> {
        self.common.write_u8(COMMON_DEVICE_STATUS, 0);
        for _ in 0..1_000_000 {
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            if self.common.read_u8(COMMON_DEVICE_STATUS) == 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(VirtioBlkError::ResetTimeout)
    }

    fn submit_read(
        &mut self,
        lba: u64,
        data_dma: &crate::capability::DmaRegion,
    ) -> Result<(), VirtioBlkError> {
        let queue = queue_memory(&self.queue_dma);
        queue.header = VirtioBlkRequestHeader {
            request_type: VIRTIO_BLK_T_IN,
            reserved: 0,
            sector: lba,
        };
        queue.status = 0xff;

        let queue_base = self.queue_dma.phys().as_u64();
        let header_address = queue_base + offset_of_queue_header() as u64;
        let status_address = queue_base + offset_of_queue_status() as u64;
        queue.descriptors[0] = VirtqDescriptor {
            addr: header_address,
            len: size_of::<VirtioBlkRequestHeader>() as u32,
            flags: VIRTQ_DESC_F_NEXT,
            next: 1,
        };
        queue.descriptors[1] = VirtqDescriptor {
            addr: data_dma.phys().as_u64(),
            len: SECTOR_SIZE as u32,
            flags: VIRTQ_DESC_F_NEXT | VIRTQ_DESC_F_WRITE,
            next: 2,
        };
        queue.descriptors[2] = VirtqDescriptor {
            addr: status_address,
            len: 1,
            flags: VIRTQ_DESC_F_WRITE,
            next: 0,
        };

        // SAFETY: virtqueue metadata is device-visible DMA memory. Volatile
        // accesses prevent compiler caching while the device updates it.
        let avail_index = unsafe { read_volatile(&queue.avail.idx) };
        unsafe {
            write_volatile(
                &mut queue.avail.ring[(avail_index as usize) % QUEUE_SIZE as usize],
                0,
            );
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        unsafe { write_volatile(&mut queue.avail.idx, avail_index.wrapping_add(1)) };
        let notify_offset = self.queue_notify_offset as usize * self.notify_multiplier as usize;
        self.notify.write_u16(notify_offset, QUEUE_INDEX);

        let mut spins = 0u32;
        loop {
            core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
            // SAFETY: `idx` is written asynchronously by the device.
            let used_index = unsafe { read_volatile(&queue.used.idx) };
            if used_index != self.last_used {
                // SAFETY: the device completed this used-ring element and
                // status byte before incrementing used.idx.
                let used = unsafe {
                    read_volatile(&queue.used.ring[(self.last_used as usize) % QUEUE_SIZE as usize])
                };
                if used.id >= QUEUE_SIZE as u32 || used.id != 0 {
                    return Err(VirtioBlkError::DeviceStatus(0xfe));
                }
                self.last_used = self.last_used.wrapping_add(1);
                let status = unsafe { read_volatile(&queue.status) };
                if status != VIRTIO_BLK_S_OK {
                    return Err(VirtioBlkError::DeviceStatus(status));
                }
                return Ok(());
            }
            spins = spins.wrapping_add(1);
            if spins == 10_000_000 {
                return Err(VirtioBlkError::Timeout);
            }
            core::hint::spin_loop();
        }
    }
}

impl Drop for VirtioBlock {
    fn drop(&mut self) {
        if self.reset().is_ok() {
            self.queue_dma.set_outstanding(false);
            let _ = DMA_TABLE.lock().release(&self.queue_dma);
        }
    }
}

#[derive(Clone, Copy)]
struct MmioRegion {
    base: u64,
    length: usize,
}

impl MmioRegion {
    fn check(&self, offset: usize, width: usize) {
        assert!(
            offset
                .checked_add(width)
                .is_some_and(|end| end <= self.length)
        );
    }

    fn read_u8(&self, offset: usize) -> u8 {
        self.check(offset, 1);
        // SAFETY: the region was validated and mapped cache-disabled.
        unsafe { read_volatile((self.base + offset as u64) as *const u8) }
    }

    fn read_u16(&self, offset: usize) -> u16 {
        self.check(offset, 2);
        // SAFETY: the region was validated and mapped cache-disabled.
        unsafe { read_volatile((self.base + offset as u64) as *const u16) }
    }

    fn read_u32(&self, offset: usize) -> u32 {
        self.check(offset, 4);
        // SAFETY: the region was validated and mapped cache-disabled.
        unsafe { read_volatile((self.base + offset as u64) as *const u32) }
    }

    fn read_u64(&self, offset: usize) -> u64 {
        self.check(offset, 8);
        // SAFETY: the region was validated and mapped cache-disabled.
        unsafe { read_volatile((self.base + offset as u64) as *const u64) }
    }

    fn write_u8(&self, offset: usize, value: u8) {
        self.check(offset, 1);
        // SAFETY: the region was validated and mapped cache-disabled.
        unsafe { write_volatile((self.base + offset as u64) as *mut u8, value) }
    }

    fn write_u16(&self, offset: usize, value: u16) {
        self.check(offset, 2);
        // SAFETY: the region was validated and mapped cache-disabled.
        unsafe { write_volatile((self.base + offset as u64) as *mut u16, value) }
    }

    fn write_u32(&self, offset: usize, value: u32) {
        self.check(offset, 4);
        // SAFETY: the region was validated and mapped cache-disabled.
        unsafe { write_volatile((self.base + offset as u64) as *mut u32, value) }
    }

    fn write_u64(&self, offset: usize, value: u64) {
        self.check(offset, 8);
        // SAFETY: the region was validated and mapped cache-disabled.
        unsafe { write_volatile((self.base + offset as u64) as *mut u64, value) }
    }
}

fn find_capability(
    capabilities: &[Option<VirtioCapability>],
    cfg_type: u8,
) -> Option<VirtioCapability> {
    capabilities
        .iter()
        .flatten()
        .find(|capability| capability.cfg_type == cfg_type)
        .copied()
}

fn enable_function(function: &PciFunctionInfo) -> Result<(), VirtioBlkError> {
    pci::enable_memory_and_bus_master(function)?;
    Ok(())
}

fn read_virtio_capabilities(
    function: &PciFunctionInfo,
) -> Result<[Option<VirtioCapability>; MAX_CAPABILITIES], VirtioBlkError> {
    const PCI_STATUS: usize = 0x06;
    const PCI_STATUS_CAP_LIST: u16 = 1 << 4;
    const PCI_CAP_POINTER: usize = 0x34;

    if pci::config_read_u16(function, PCI_STATUS)? & PCI_STATUS_CAP_LIST == 0 {
        return Err(VirtioBlkError::BadCapability);
    }
    let mut result = [None; MAX_CAPABILITIES];
    let mut seen = [false; 256];
    let mut pointer = pci::config_read_u8(function, PCI_CAP_POINTER)? as usize;
    let mut count = 0usize;
    while pointer != 0 {
        if pointer & 3 != 0 || pointer + 16 > 256 || seen[pointer] || count >= MAX_CAPABILITIES {
            return Err(VirtioBlkError::BadCapability);
        }
        seen[pointer] = true;
        let id = pci::config_read_u8(function, pointer)?;
        let next = pci::config_read_u8(function, pointer + 1)? as usize;
        let cap_len = pci::config_read_u8(function, pointer + 2)? as usize;
        if id == VIRTIO_PCI_CAP_ID {
            if cap_len < 16 || pointer + cap_len > 256 {
                return Err(VirtioBlkError::BadCapability);
            }
            let cfg_type = pci::config_read_u8(function, pointer + 3)?;
            let bar = pci::config_read_u8(function, pointer + 4)?;
            let offset = pci::config_read_u32(function, pointer + 8)?;
            let length = pci::config_read_u32(function, pointer + 12)?;
            let notify_multiplier = if cfg_type == VIRTIO_PCI_CAP_NOTIFY_CFG {
                if cap_len < 20 {
                    return Err(VirtioBlkError::BadCapability);
                }
                pci::config_read_u32(function, pointer + 16)?
            } else {
                0
            };
            // cfg_type 5 is PCI configuration access and legitimately carries
            // a zero-length BAR range. M5.2 does not consume it.
            if length == 0 && cfg_type != 5 {
                return Err(VirtioBlkError::BadCapability);
            }
            result[count] = Some(VirtioCapability {
                cfg_type,
                bar,
                offset,
                length,
                notify_multiplier,
            });
            count += 1;
        }
        pointer = next;
    }
    Ok(result)
}

fn map_capability(
    capability: VirtioCapability,
    bars: &[BarInfo; 6],
    window: usize,
) -> Result<MmioRegion, VirtioBlkError> {
    let bar = bars
        .get(capability.bar as usize)
        .filter(|bar| bar.size > 0 && bar.kind != BarKind::Io)
        .ok_or(VirtioBlkError::BadCapability)?;
    let offset = capability.offset as u64;
    let length = capability.length as u64;
    let end = offset
        .checked_add(length)
        .filter(|end| *end <= bar.size)
        .ok_or(VirtioBlkError::BadCapability)?;
    let physical = bar
        .base
        .checked_add(offset)
        .ok_or(VirtioBlkError::BadCapability)?;
    let page_base = align_down(physical, PAGE_SIZE as u64);
    let page_offset = (physical - page_base) as usize;
    let map_bytes = align_up((page_offset as u64) + length, PAGE_SIZE as u64) as usize;
    let pages = map_bytes / PAGE_SIZE;
    if pages == 0 || pages > MAX_MMIO_PAGES {
        return Err(VirtioBlkError::BadCapability);
    }
    let virtual_base = MMIO_SCRATCH_BASE + (window * MAX_MMIO_PAGES * PAGE_SIZE) as u64;
    for page in 0..pages {
        // SAFETY: the validated BAR range is device MMIO. Each capability gets
        // a disjoint fixed virtual window, mapped cache-disabled and NX.
        unsafe {
            vmm::map_page(
                VirtAddr(virtual_base + (page * PAGE_SIZE) as u64),
                PhysAddr(page_base + (page * PAGE_SIZE) as u64),
                PTE_WRITABLE | PTE_CACHE_DISABLE | PTE_NO_EXECUTE,
            )?;
        }
    }
    let _ = end;
    Ok(MmioRegion {
        base: virtual_base + page_offset as u64,
        length: capability.length as usize,
    })
}

fn zero_region(physical: PhysAddr, pages: usize) {
    // SAFETY: the caller owns these pinned pages and no request references them.
    unsafe { core::ptr::write_bytes(physical.to_virt().as_mut_ptr::<u8>(), 0, pages * PAGE_SIZE) }
}

fn queue_memory(region: &crate::capability::DmaRegion) -> &'static mut QueueMemory {
    assert!(size_of::<QueueMemory>() <= region.pages() * PAGE_SIZE);
    assert!(align_of::<QueueMemory>() <= PAGE_SIZE);
    // SAFETY: the region is pinned, zeroed, large enough, and exclusively used
    // by one VirtioBlock instance.
    unsafe { &mut *region.phys().to_virt().as_mut_ptr::<QueueMemory>() }
}

fn offset_of_queue_descriptors() -> usize {
    core::mem::offset_of!(QueueMemory, descriptors)
}

fn offset_of_queue_avail() -> usize {
    core::mem::offset_of!(QueueMemory, avail)
}

fn offset_of_queue_used() -> usize {
    core::mem::offset_of!(QueueMemory, used)
}

fn offset_of_queue_header() -> usize {
    core::mem::offset_of!(QueueMemory, header)
}

fn offset_of_queue_status() -> usize {
    core::mem::offset_of!(QueueMemory, status)
}
