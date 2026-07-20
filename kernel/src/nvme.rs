//! Bounded read-only NVMe PCI transport for Framework storage bring-up.
//!
//! The controller is discovered through the same ACPI/PCI resource model as
//! virtio. Only the mandatory NVM command set, one namespace, one admin queue,
//! and one I/O queue are accepted. Writes remain unavailable: M5.7 may inspect
//! internal media, but cannot authorize modifying it.

use core::mem::size_of;
use core::ptr::{read_volatile, write_volatile};

use crate::block_proto::SECTOR_SIZE;
use crate::capability::{DmaRegion, PciFunctionInfo};
use crate::dma::{DMA_TABLE, DmaError};
use crate::memory::vmm::{self, MapError, PTE_CACHE_DISABLE, PTE_NO_EXECUTE, PTE_WRITABLE};
use crate::memory::{PAGE_SIZE, PhysAddr, VirtAddr, align_down, align_up};
use crate::pci::{self, BarKind, PciError};

const NVME_CLASS: u32 = 0x01_08_02;
const REG_CAP: usize = 0x00;
const REG_CC: usize = 0x14;
const REG_CSTS: usize = 0x1c;
const REG_AQA: usize = 0x24;
const REG_ASQ: usize = 0x28;
const REG_ACQ: usize = 0x30;
const REG_DOORBELL: usize = 0x1000;
const CC_ENABLE: u32 = 1;
const CSTS_READY: u32 = 1;
const CSTS_FATAL: u32 = 2;
const ADMIN_IDENTIFY: u8 = 0x06;
const ADMIN_CREATE_IO_CQ: u8 = 0x05;
const ADMIN_CREATE_IO_SQ: u8 = 0x01;
const NVM_READ: u8 = 0x02;
const QUEUE_DEPTH: usize = 8;
const MMIO_BASE: u64 = 0xffff_c200_0000_0000;
const MAX_MMIO_PAGES: usize = 64;
const POLL_LIMIT: u32 = 20_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NvmeError {
    DeviceNotFound,
    Pci(PciError),
    Map(MapError),
    Dma(DmaError),
    UnsupportedController,
    BadBar,
    QueueTooSmall,
    ControllerFatal,
    Timeout,
    Completion(u16),
    NamespaceMissing,
    UnsupportedBlockSize,
    Capacity,
    OutOfRange,
    BufferSize,
    ReadOnly,
}

impl NvmeError {
    pub fn requires_reinitialize(self) -> bool {
        !matches!(
            self,
            Self::OutOfRange | Self::BufferSize | Self::ReadOnly | Self::NamespaceMissing
        )
    }
}

impl From<PciError> for NvmeError {
    fn from(value: PciError) -> Self {
        Self::Pci(value)
    }
}

impl From<MapError> for NvmeError {
    fn from(value: MapError) -> Self {
        Self::Map(value)
    }
}

impl From<DmaError> for NvmeError {
    fn from(value: DmaError) -> Self {
        Self::Dma(value)
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Submission {
    dword0: u32,
    nsid: u32,
    reserved: [u32; 4],
    prp1: u64,
    prp2: u64,
    cdw10: u32,
    cdw11: u32,
    cdw12: u32,
    cdw13: u32,
    cdw14: u32,
    cdw15: u32,
}

impl Submission {
    const fn zeroed() -> Self {
        Self {
            dword0: 0,
            nsid: 0,
            reserved: [0; 4],
            prp1: 0,
            prp2: 0,
            cdw10: 0,
            cdw11: 0,
            cdw12: 0,
            cdw13: 0,
            cdw14: 0,
            cdw15: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Completion {
    result: u32,
    reserved: u32,
    sq_head: u16,
    sq_id: u16,
    command_id: u16,
    status: u16,
}

struct Queue {
    submissions: DmaRegion,
    completions: DmaRegion,
    qid: u16,
    tail: u16,
    head: u16,
    phase: bool,
    next_command_id: u16,
}

impl Queue {
    fn allocate(qid: u16) -> Result<Self, NvmeError> {
        let submissions = DMA_TABLE.lock().pin(1)?;
        let completions = match DMA_TABLE.lock().pin(1) {
            Ok(region) => region,
            Err(error) => {
                DMA_TABLE.lock().release(&submissions)?;
                return Err(error.into());
            }
        };
        zero(&submissions);
        zero(&completions);
        Ok(Self {
            submissions,
            completions,
            qid,
            tail: 0,
            head: 0,
            phase: true,
            next_command_id: 1,
        })
    }

    fn release(&mut self) {
        let _ = DMA_TABLE.lock().release(&self.submissions);
        let _ = DMA_TABLE.lock().release(&self.completions);
    }
}

pub struct NvmeBlock {
    registers: Mmio,
    doorbell_stride: usize,
    admin: Queue,
    io: Queue,
    namespace_id: u32,
    capacity_sectors: u64,
}

impl NvmeBlock {
    pub fn find_and_init() -> Result<Self, NvmeError> {
        let function = pci::enumerate()?
            .into_iter()
            .find(|function| function.class_code & 0x00ff_ffff == NVME_CLASS)
            .ok_or(NvmeError::DeviceNotFound)?;
        Self::init(function)
    }

    pub fn init(function: PciFunctionInfo) -> Result<Self, NvmeError> {
        let bars = pci::probe_bars(&function)?;
        let bar = bars
            .iter()
            .find(|bar| bar.index == 0 && bar.size > REG_DOORBELL as u64 && bar.kind != BarKind::Io)
            .ok_or(NvmeError::BadBar)?;
        let registers = map_mmio(bar.base, bar.size)?;
        let cap = registers.read_u64(REG_CAP);
        let queue_entries = (cap as usize & 0xffff) + 1;
        let timeout = ((cap >> 24) & 0xff) as u8;
        let doorbell_stride = 4usize
            .checked_shl(((cap >> 32) & 0xf) as u32)
            .ok_or(NvmeError::UnsupportedController)?;
        validate_doorbells(bar.size, doorbell_stride)?;
        let nvm_supported = cap & (1u64 << 37) != 0;
        let min_page_shift = 12 + ((cap >> 48) & 0xf) as usize;
        if queue_entries < QUEUE_DEPTH
            || timeout == 0
            || !nvm_supported
            || min_page_shift > PAGE_SIZE.trailing_zeros() as usize
        {
            return Err(if queue_entries < QUEUE_DEPTH {
                NvmeError::QueueTooSmall
            } else {
                NvmeError::UnsupportedController
            });
        }

        pci::enable_memory_and_bus_master(&function)?;
        disable(&registers)?;
        let admin = Queue::allocate(0)?;
        let io = match Queue::allocate(1) {
            Ok(queue) => queue,
            Err(error) => {
                let mut admin = admin;
                admin.release();
                return Err(error);
            }
        };
        let mut device = Self {
            registers,
            doorbell_stride,
            admin,
            io,
            namespace_id: 1,
            capacity_sectors: 0,
        };
        device.configure_admin()?;
        device.identify_controller()?;
        device.identify_namespace()?;
        device.create_io_queues()?;
        Ok(device)
    }

    pub fn capacity_sectors(&self) -> u64 {
        self.capacity_sectors
    }

    pub fn read_sector(&mut self, lba: u64, output: &mut [u8]) -> Result<(), NvmeError> {
        if output.len() != SECTOR_SIZE {
            return Err(NvmeError::BufferSize);
        }
        if lba >= self.capacity_sectors {
            return Err(NvmeError::OutOfRange);
        }
        let data = DMA_TABLE.lock().pin(1)?;
        zero(&data);
        let mut command = Submission::zeroed();
        command.dword0 = NVM_READ as u32;
        command.nsid = self.namespace_id;
        command.prp1 = data.phys().as_u64();
        command.cdw10 = lba as u32;
        command.cdw11 = (lba >> 32) as u32;
        command.cdw12 = 0;
        let result = self.submit_io(command, Some(&data));
        if result.is_ok() {
            // SAFETY: completion is observed before copying from the pinned page.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    data.phys().to_virt().as_mut_ptr::<u8>() as *const u8,
                    output.as_mut_ptr(),
                    output.len(),
                )
            };
        }
        match result {
            Err(NvmeError::Timeout) => {
                let _ = self.quiesce_timed_out(&data);
                Err(NvmeError::Timeout)
            }
            result => {
                DMA_TABLE.lock().release(&data)?;
                result.map(|_| ())
            }
        }
    }

    pub fn write_sector(&mut self, _lba: u64, _input: &[u8]) -> Result<(), NvmeError> {
        Err(NvmeError::ReadOnly)
    }

    pub fn flush(&mut self) -> Result<(), NvmeError> {
        Err(NvmeError::ReadOnly)
    }

    pub fn reset(&mut self) -> Result<(), NvmeError> {
        disable(&self.registers)?;
        self.admin.submissions.set_outstanding(false);
        self.admin.completions.set_outstanding(false);
        self.io.submissions.set_outstanding(false);
        self.io.completions.set_outstanding(false);
        self.admin.tail = 0;
        self.admin.head = 0;
        self.admin.phase = true;
        self.io.tail = 0;
        self.io.head = 0;
        self.io.phase = true;
        zero(&self.admin.submissions);
        zero(&self.admin.completions);
        zero(&self.io.submissions);
        zero(&self.io.completions);
        self.configure_admin()?;
        self.create_io_queues()
    }

    fn quiesce_timed_out(&mut self, data: &DmaRegion) -> Result<(), NvmeError> {
        disable(&self.registers)?;
        self.admin.submissions.set_outstanding(false);
        self.admin.completions.set_outstanding(false);
        self.io.submissions.set_outstanding(false);
        self.io.completions.set_outstanding(false);
        data.set_outstanding(false);
        DMA_TABLE.lock().release(data)?;
        Ok(())
    }

    fn configure_admin(&mut self) -> Result<(), NvmeError> {
        self.registers.write_u32(
            REG_AQA,
            ((QUEUE_DEPTH as u32 - 1) << 16) | (QUEUE_DEPTH as u32 - 1),
        );
        self.registers
            .write_u64(REG_ASQ, self.admin.submissions.phys().as_u64());
        self.registers
            .write_u64(REG_ACQ, self.admin.completions.phys().as_u64());
        let cc = CC_ENABLE | (6 << 16) | (4 << 20);
        self.registers.write_u32(REG_CC, cc);
        wait_ready(&self.registers, true)
    }

    fn identify_controller(&mut self) -> Result<(), NvmeError> {
        let data = DMA_TABLE.lock().pin(1)?;
        zero(&data);
        let mut command = Submission::zeroed();
        command.dword0 = ADMIN_IDENTIFY as u32;
        command.prp1 = data.phys().as_u64();
        command.cdw10 = 1;
        let result = self.submit_admin(command, Some(&data));
        match result {
            Ok(_) => {
                let bytes = dma_bytes(&data);
                let namespace_count = u32::from_le_bytes(bytes[516..520].try_into().unwrap());
                DMA_TABLE.lock().release(&data)?;
                if namespace_count == 0 {
                    return Err(NvmeError::NamespaceMissing);
                }
                Ok(())
            }
            Err(NvmeError::Timeout) => {
                let _ = self.quiesce_timed_out(&data);
                Err(NvmeError::Timeout)
            }
            Err(error) => {
                DMA_TABLE.lock().release(&data)?;
                Err(error)
            }
        }
    }

    fn identify_namespace(&mut self) -> Result<(), NvmeError> {
        let data = DMA_TABLE.lock().pin(1)?;
        zero(&data);
        let mut command = Submission::zeroed();
        command.dword0 = ADMIN_IDENTIFY as u32;
        command.nsid = self.namespace_id;
        command.prp1 = data.phys().as_u64();
        let result = self.submit_admin(command, Some(&data));
        match result {
            Ok(_) => {
                let parsed = parse_namespace(dma_bytes(&data));
                DMA_TABLE.lock().release(&data)?;
                let (capacity, block_size) = parsed?;
                if block_size != SECTOR_SIZE {
                    return Err(NvmeError::UnsupportedBlockSize);
                }
                self.capacity_sectors = capacity;
                Ok(())
            }
            Err(NvmeError::Timeout) => {
                let _ = self.quiesce_timed_out(&data);
                Err(NvmeError::Timeout)
            }
            Err(error) => {
                DMA_TABLE.lock().release(&data)?;
                Err(error)
            }
        }
    }

    fn create_io_queues(&mut self) -> Result<(), NvmeError> {
        let mut completion = Submission::zeroed();
        completion.dword0 = ADMIN_CREATE_IO_CQ as u32;
        completion.prp1 = self.io.completions.phys().as_u64();
        completion.cdw10 = self.io.qid as u32 | ((QUEUE_DEPTH as u32 - 1) << 16);
        completion.cdw11 = 1;
        self.submit_admin(completion, None)?;

        let mut submission = Submission::zeroed();
        submission.dword0 = ADMIN_CREATE_IO_SQ as u32;
        submission.prp1 = self.io.submissions.phys().as_u64();
        submission.cdw10 = self.io.qid as u32 | ((QUEUE_DEPTH as u32 - 1) << 16);
        submission.cdw11 = 1 | ((self.io.qid as u32) << 16);
        self.submit_admin(submission, None)?;
        Ok(())
    }

    fn submit_admin(
        &mut self,
        command: Submission,
        data: Option<&DmaRegion>,
    ) -> Result<u32, NvmeError> {
        submit(
            &self.registers,
            self.doorbell_stride,
            &mut self.admin,
            command,
            data,
        )
    }

    fn submit_io(
        &mut self,
        command: Submission,
        data: Option<&DmaRegion>,
    ) -> Result<u32, NvmeError> {
        submit(
            &self.registers,
            self.doorbell_stride,
            &mut self.io,
            command,
            data,
        )
    }
}

impl Drop for NvmeBlock {
    fn drop(&mut self) {
        if disable(&self.registers).is_ok() {
            self.admin.submissions.set_outstanding(false);
            self.admin.completions.set_outstanding(false);
            self.io.submissions.set_outstanding(false);
            self.io.completions.set_outstanding(false);
        }
        self.admin.release();
        self.io.release();
    }
}

fn submit(
    registers: &Mmio,
    doorbell_stride: usize,
    queue: &mut Queue,
    mut command: Submission,
    data: Option<&DmaRegion>,
) -> Result<u32, NvmeError> {
    let command_id = queue.next_command_id;
    queue.next_command_id = queue.next_command_id.wrapping_add(1).max(1);
    command.dword0 |= (command_id as u32) << 16;
    queue.submissions.set_outstanding(true);
    queue.completions.set_outstanding(true);
    if let Some(data) = data {
        data.set_outstanding(true);
    }
    let submissions = queue
        .submissions
        .phys()
        .to_virt()
        .as_mut_ptr::<Submission>();
    // SAFETY: the queue owns one pinned page and `tail` is bounded by QUEUE_DEPTH.
    unsafe { write_volatile(submissions.add(queue.tail as usize), command) };
    core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
    queue.tail = (queue.tail + 1) % QUEUE_DEPTH as u16;
    registers.write_u32(
        doorbell_offset(queue.qid, false, doorbell_stride),
        queue.tail as u32,
    );

    let completions = queue
        .completions
        .phys()
        .to_virt()
        .as_mut_ptr::<Completion>() as *const Completion;
    for _ in 0..POLL_LIMIT {
        core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
        // SAFETY: `head` is bounded by QUEUE_DEPTH and the page remains pinned.
        let completion = unsafe { read_volatile(completions.add(queue.head as usize)) };
        if (completion.status & 1 != 0) == queue.phase {
            let outcome = if completion.command_id != command_id || completion.sq_id != queue.qid {
                Err(NvmeError::Completion(completion.status))
            } else if completion.status >> 1 != 0 {
                Err(NvmeError::Completion(completion.status >> 1))
            } else {
                Ok(completion.result)
            };
            queue.head += 1;
            if queue.head == QUEUE_DEPTH as u16 {
                queue.head = 0;
                queue.phase = !queue.phase;
            }
            registers.write_u32(
                doorbell_offset(queue.qid, true, doorbell_stride),
                queue.head as u32,
            );
            if let Some(data) = data {
                data.set_outstanding(false);
            }
            queue.submissions.set_outstanding(false);
            queue.completions.set_outstanding(false);
            return outcome;
        }
        core::hint::spin_loop();
    }
    Err(NvmeError::Timeout)
}

fn doorbell_offset(qid: u16, completion: bool, stride: usize) -> usize {
    REG_DOORBELL + (2 * qid as usize + usize::from(completion)) * stride
}

fn parse_namespace(bytes: &[u8]) -> Result<(u64, usize), NvmeError> {
    if bytes.len() < 132 {
        return Err(NvmeError::Capacity);
    }
    let capacity = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
    if capacity == 0 {
        return Err(NvmeError::Capacity);
    }
    let format = (bytes[26] & 0x0f) as usize;
    let offset = 128usize
        .checked_add(
            format
                .checked_mul(4)
                .ok_or(NvmeError::UnsupportedBlockSize)?,
        )
        .ok_or(NvmeError::UnsupportedBlockSize)?;
    let descriptor = bytes
        .get(offset..offset + 4)
        .ok_or(NvmeError::UnsupportedBlockSize)?;
    let metadata_size = u16::from_le_bytes(descriptor[0..2].try_into().unwrap());
    let shift = descriptor[2];
    if metadata_size != 0 || shift != SECTOR_SIZE.trailing_zeros() as u8 {
        return Err(NvmeError::UnsupportedBlockSize);
    }
    Ok((capacity, SECTOR_SIZE))
}

fn validate_doorbells(length: u64, stride: usize) -> Result<(), NvmeError> {
    let required = doorbell_offset(1, true, stride)
        .checked_add(size_of::<u32>())
        .ok_or(NvmeError::BadBar)? as u64;
    if required > length {
        return Err(NvmeError::BadBar);
    }
    Ok(())
}

fn disable(registers: &Mmio) -> Result<(), NvmeError> {
    registers.write_u32(REG_CC, registers.read_u32(REG_CC) & !CC_ENABLE);
    wait_ready(registers, false)
}

fn wait_ready(registers: &Mmio, ready: bool) -> Result<(), NvmeError> {
    for _ in 0..POLL_LIMIT {
        let status = registers.read_u32(REG_CSTS);
        if status & CSTS_FATAL != 0 {
            return Err(NvmeError::ControllerFatal);
        }
        if (status & CSTS_READY != 0) == ready {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err(NvmeError::Timeout)
}

fn map_mmio(physical: u64, length: u64) -> Result<Mmio, NvmeError> {
    let page_base = align_down(physical, PAGE_SIZE as u64);
    let page_offset = (physical - page_base) as usize;
    let map_bytes = align_up(
        (page_offset as u64)
            .checked_add(length)
            .ok_or(NvmeError::BadBar)?,
        PAGE_SIZE as u64,
    ) as usize;
    let pages = map_bytes / PAGE_SIZE;
    if pages == 0 || pages > MAX_MMIO_PAGES || length < REG_DOORBELL as u64 + 16 {
        return Err(NvmeError::BadBar);
    }
    for page in 0..pages {
        let virt = VirtAddr(MMIO_BASE + (page * PAGE_SIZE) as u64);
        let phys = PhysAddr(page_base + (page * PAGE_SIZE) as u64);
        // SAFETY: PCI BAR probing validated this cache-disabled, NX MMIO range.
        match unsafe {
            vmm::map_page(
                virt,
                phys,
                PTE_WRITABLE | PTE_CACHE_DISABLE | PTE_NO_EXECUTE,
            )
        } {
            Ok(()) => {}
            Err(MapError::AlreadyMapped) if vmm::translate(virt) == Some(phys) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(Mmio {
        base: MMIO_BASE + page_offset as u64,
        length: length as usize,
    })
}

#[derive(Clone, Copy)]
struct Mmio {
    base: u64,
    length: usize,
}

impl Mmio {
    fn check(&self, offset: usize, width: usize) {
        assert!(
            offset
                .checked_add(width)
                .is_some_and(|end| end <= self.length)
        );
    }

    fn read_u32(&self, offset: usize) -> u32 {
        self.check(offset, size_of::<u32>());
        // SAFETY: bounds checked against the mapped BAR.
        unsafe { read_volatile((self.base + offset as u64) as *const u32) }
    }

    fn read_u64(&self, offset: usize) -> u64 {
        self.check(offset, size_of::<u64>());
        // SAFETY: bounds checked against the mapped BAR.
        unsafe { read_volatile((self.base + offset as u64) as *const u64) }
    }

    fn write_u32(&self, offset: usize, value: u32) {
        self.check(offset, size_of::<u32>());
        // SAFETY: bounds checked against the mapped BAR.
        unsafe { write_volatile((self.base + offset as u64) as *mut u32, value) }
    }

    fn write_u64(&self, offset: usize, value: u64) {
        self.check(offset, size_of::<u64>());
        // SAFETY: bounds checked against the mapped BAR.
        unsafe { write_volatile((self.base + offset as u64) as *mut u64, value) }
    }
}

fn zero(region: &DmaRegion) {
    // SAFETY: the caller owns the pinned region and no command references it.
    unsafe {
        core::ptr::write_bytes(
            region.phys().to_virt().as_mut_ptr::<u8>(),
            0,
            region.pages() * PAGE_SIZE,
        )
    }
}

fn dma_bytes(region: &DmaRegion) -> &'static [u8] {
    // SAFETY: the region is pinned and remains alive for the returned borrow's use.
    unsafe {
        core::slice::from_raw_parts(
            region.phys().to_virt().as_mut_ptr::<u8>() as *const u8,
            region.pages() * PAGE_SIZE,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn namespace_parser_accepts_bounded_512_byte_format() {
        let mut bytes = [0u8; PAGE_SIZE];
        bytes[0..8].copy_from_slice(&32u64.to_le_bytes());
        bytes[26] = 0;
        bytes[130] = 9;
        assert_eq!(parse_namespace(&bytes), Ok((32, 512)));
    }

    #[test_case]
    fn namespace_parser_rejects_zero_capacity_bad_format_and_metadata() {
        let mut bytes = [0u8; PAGE_SIZE];
        bytes[26] = 0;
        bytes[130] = 9;
        assert_eq!(parse_namespace(&bytes), Err(NvmeError::Capacity));
        bytes[0..8].copy_from_slice(&1u64.to_le_bytes());
        bytes[26] = 15;
        bytes[128 + 15 * 4 + 2] = 63;
        assert_eq!(
            parse_namespace(&bytes),
            Err(NvmeError::UnsupportedBlockSize)
        );
        bytes[26] = 0;
        bytes[128..130].copy_from_slice(&8u16.to_le_bytes());
        bytes[130] = 9;
        assert_eq!(
            parse_namespace(&bytes),
            Err(NvmeError::UnsupportedBlockSize)
        );
    }

    #[test_case]
    fn doorbell_bounds_cover_io_completion_queue() {
        assert_eq!(validate_doorbells((REG_DOORBELL + 16) as u64, 4), Ok(()));
        assert_eq!(
            validate_doorbells((REG_DOORBELL + 16) as u64, 8),
            Err(NvmeError::BadBar)
        );
    }
}
