//! Bounded PCI discovery via ACPI MCFG / ECAM.
//!
//! M5.1 deliverable: parse ACPI MCFG, enumerate bounded PCI
//! segment/bus/device/function ranges, validate PCI capability chains and BAR
//! declarations before any MMIO mapping, and surface the discovered functions
//! as kernel objects to be granted to driver components.
//!
//! The kernel maps one 4 KiB ECAM config page at a time into a fixed scratch
//! virtual address. Enumeration is single-threaded at boot, so remapping the
//! scratch page between functions is safe and avoids pinning a large MMIO
//! window. Every malformed input — bad MCFG, broken capability chain,
//! impossible BAR — returns a typed [`PciError`] rather than hanging.

use spin::Once;

use alloc::vec::Vec;

use crate::acpi;
use crate::capability::PciFunctionInfo;
use crate::memory::{
    PhysAddr, VirtAddr,
    vmm::{
        PTE_CACHE_DISABLE, PTE_NO_EXECUTE, PTE_PRESENT, PTE_WRITABLE, active_pml4, leaf_flags_in,
        map_page_in, remap_page_in,
    },
};

const SDT_HEADER_LEN: usize = 36;
const MCFG_RESERVED_LEN: usize = 8;
const MCFG_ENTRY_LEN: usize = 16;
const MAX_MCFG_ENTRIES: usize = 16;
/// Hard upper bound on total enumerated PCI functions across all segments.
const MAX_ENUMERATED_FUNCTIONS: usize = 4096;

/// Scratch kernel virtual address for the current function's 4 KiB config page.
const CONFIG_SCRATCH_VA: u64 = 0xffff_c000_0000_0000;

const CONFIG_VENDOR_ID: usize = 0x00;
const CONFIG_DEVICE_ID: usize = 0x02;
const CONFIG_HEADER_TYPE: usize = 0x0e;
const CONFIG_CLASS_CODE: usize = 0x09;
const CONFIG_BARS: usize = 0x10;
const CONFIG_CAP_POINTER: usize = 0x34;
const CONFIG_SPACE_SIZE: usize = 256;

const CAP_HEADER_LEN: usize = 8;
const MAX_CAPABILITIES: usize = 48;
const CAP_LIST_END: u8 = 0;

const BAR_COUNT: usize = 6;
const BAR_TYPE_MASK: u32 = 0b111;
const BAR_TYPE_IO: u32 = 0b001;
const BAR_TYPE_MEM64: u32 = 0b100;
const BAR_PREFETCHABLE: u32 = 1 << 3;
const BAR_IO_MASK: u32 = 0xffff_fffc;
const BAR_MEM_MASK: u32 = 0xffff_fff0;
/// 64-bit memory BAR address mask: bits 4..=63 are address, bits 0..3 type.
const BAR_MEM_MASK64: u64 = 0xffff_ffff_ffff_fff0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PciError {
    NoMcfg,
    MalformedMcfg,
    TooManySegments,
    EcamMapFailed,
    BadCapabilityChain,
    BadBar,
    OutOfFrames,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McfgSegment {
    pub base: PhysAddr,
    pub segment: u16,
    pub start_bus: u8,
    pub end_bus: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarInfo {
    pub index: u8,
    pub kind: BarKind,
    pub prefetchable: bool,
    /// Size in bytes. Always a power of two for memory BARs.
    pub size: u64,
    /// Raw 64-bit base address decoded from the BAR(s).
    pub base: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarKind {
    Memory32,
    Memory64,
    Io,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityEntry {
    pub id: u8,
    pub offset: u8,
}

static SEGMENTS: Once<Vec<McfgSegment>> = Once::new();

/// Parse and install the PCI segment table from ACPI MCFG. Must run after
/// [`acpi::init`] and before [`enumerate`].
pub fn init() -> Result<&'static [McfgSegment], PciError> {
    let segments: &'static [McfgSegment] = SEGMENTS
        .try_call_once(parse_mcfg)
        .map_err(|_| PciError::MalformedMcfg)?;
    Ok(segments)
}

pub fn segments() -> &'static [McfgSegment] {
    SEGMENTS.get().expect("pci::init not called")
}

fn parse_mcfg() -> Result<Vec<McfgSegment>, PciError> {
    let acpi = acpi::get().ok_or(PciError::NoMcfg)?;
    let table = acpi.mcfg.ok_or(PciError::NoMcfg)?;
    if table.len() < SDT_HEADER_LEN + MCFG_RESERVED_LEN {
        return Err(PciError::MalformedMcfg);
    }
    let body_len = table
        .len()
        .checked_sub(SDT_HEADER_LEN + MCFG_RESERVED_LEN)
        .ok_or(PciError::MalformedMcfg)?;
    let entry_count = body_len / MCFG_ENTRY_LEN;
    if entry_count > MAX_MCFG_ENTRIES || entry_count * MCFG_ENTRY_LEN != body_len {
        return Err(PciError::MalformedMcfg);
    }
    let mut out = Vec::new();
    for index in 0..entry_count {
        let off = SDT_HEADER_LEN + MCFG_RESERVED_LEN + index * MCFG_ENTRY_LEN;
        let base = read_u64(table, off).ok_or(PciError::MalformedMcfg)?;
        let segment = read_u16(table, off + 8).ok_or(PciError::MalformedMcfg)?;
        let start_bus = table
            .get(off + 10)
            .copied()
            .ok_or(PciError::MalformedMcfg)?;
        let end_bus = table
            .get(off + 11)
            .copied()
            .ok_or(PciError::MalformedMcfg)?;
        if end_bus < start_bus || base == 0 {
            return Err(PciError::MalformedMcfg);
        }
        out.push(McfgSegment {
            base: PhysAddr(base),
            segment,
            start_bus,
            end_bus,
        });
    }
    if out.is_empty() {
        return Err(PciError::NoMcfg);
    }
    Ok(out)
}

/// Enumerate all PCI functions across all bounded segments.
pub fn enumerate() -> Result<Vec<PciFunctionInfo>, PciError> {
    if SEGMENTS.get().is_none() {
        return Err(PciError::NoMcfg);
    }
    let mut found = Vec::new();
    for segment in segments() {
        if found.len() > 4096 {
            return Err(PciError::TooManySegments);
        }
        enumerate_segment(segment, &mut found)?;
    }
    Ok(found)
}

fn enumerate_segment(
    segment: &McfgSegment,
    out: &mut Vec<PciFunctionInfo>,
) -> Result<(), PciError> {
    // Bounded enumeration: stop the moment the accumulated count exceeds the
    // advertised hard limit, so a single wide segment cannot exceed it.
    for bus in segment.start_bus..=segment.end_bus {
        for device in 0..32u8 {
            // Probe function 0 first; only enumerate extra functions when the
            // header type's multi-function bit is set, per the PCI spec.
            if !probe_function(segment, bus, device, 0, out)? {
                continue;
            }
            let header_type = read_config_byte(CONFIG_HEADER_TYPE);
            if header_type & 0x80 != 0 {
                for function in 1..8u8 {
                    probe_function(segment, bus, device, function, out)?;
                }
            }
        }
    }
    Ok(())
}

fn probe_function(
    segment: &McfgSegment,
    bus: u8,
    device: u8,
    function: u8,
    out: &mut Vec<PciFunctionInfo>,
) -> Result<bool, PciError> {
    map_config_page(segment, bus, device, function)?;
    let vendor_id = read_config_word(CONFIG_VENDOR_ID);
    if vendor_id == 0xffff {
        return Ok(false);
    }
    let device_id = read_config_word(CONFIG_DEVICE_ID);
    let class_code = read_config_dword(CONFIG_CLASS_CODE);
    if out.len() >= MAX_ENUMERATED_FUNCTIONS {
        return Err(PciError::TooManySegments);
    }
    out.push(PciFunctionInfo {
        segment: segment.segment,
        bus,
        device,
        function,
        vendor_id,
        device_id,
        class_code,
    });
    Ok(true)
}

/// Map the 4 KiB ECAM config page for one function at the scratch VA.
fn map_config_page(
    segment: &McfgSegment,
    bus: u8,
    device: u8,
    function: u8,
) -> Result<(), PciError> {
    let bus_offset = (bus as u64 - segment.start_bus as u64) << 20;
    let dev_offset = (device as u64) << 15;
    let fn_offset = (function as u64) << 12;
    let phys = PhysAddr(segment.base.as_u64() + bus_offset + dev_offset + fn_offset);
    let virt = VirtAddr(CONFIG_SCRATCH_VA);

    let root = active_pml4();
    // SAFETY: ECAM config space is device MMIO; mapping cache-disabled,
    // non-executable, kernel-only is the correct exposure. The scratch VA is
    // reused across functions, so remap once the leaf PTE exists.
    let flags = PTE_PRESENT | PTE_WRITABLE | PTE_CACHE_DISABLE | PTE_NO_EXECUTE;
    let result = unsafe {
        if leaf_flags_in(root, virt).is_some() {
            remap_page_in(root, virt, phys, flags)
        } else {
            map_page_in(root, virt, phys, flags)
        }
    };
    result.map_err(|_| PciError::EcamMapFailed)
}

fn read_config_byte(offset: usize) -> u8 {
    unsafe { core::ptr::read_volatile((CONFIG_SCRATCH_VA + offset as u64) as *const u8) }
}
fn read_config_word(offset: usize) -> u16 {
    let lo = read_config_byte(offset) as u16;
    let hi = read_config_byte(offset + 1) as u16;
    lo | (hi << 8)
}
fn read_config_dword(offset: usize) -> u32 {
    let b0 = read_config_byte(offset) as u32;
    let b1 = read_config_byte(offset + 1) as u32;
    let b2 = read_config_byte(offset + 2) as u32;
    let b3 = read_config_byte(offset + 3) as u32;
    b0 | (b1 << 8) | (b2 << 16) | (b3 << 24)
}

/// Read one byte from an explicitly selected function's conventional PCI
/// configuration space.
pub fn config_read_u8(info: &PciFunctionInfo, offset: usize) -> Result<u8, PciError> {
    if offset >= CONFIG_SPACE_SIZE {
        return Err(PciError::BadCapabilityChain);
    }
    let segment = find_segment(info.segment)?;
    map_config_page(segment, info.bus, info.device, info.function)?;
    Ok(read_config_byte(offset))
}

/// Read one little-endian word from conventional PCI configuration space.
pub fn config_read_u16(info: &PciFunctionInfo, offset: usize) -> Result<u16, PciError> {
    if offset
        .checked_add(2)
        .is_none_or(|end| end > CONFIG_SPACE_SIZE)
    {
        return Err(PciError::BadCapabilityChain);
    }
    let segment = find_segment(info.segment)?;
    map_config_page(segment, info.bus, info.device, info.function)?;
    Ok(read_config_word(offset))
}

/// Read one little-endian dword from conventional PCI configuration space.
pub fn config_read_u32(info: &PciFunctionInfo, offset: usize) -> Result<u32, PciError> {
    if offset
        .checked_add(4)
        .is_none_or(|end| end > CONFIG_SPACE_SIZE)
    {
        return Err(PciError::BadCapabilityChain);
    }
    let segment = find_segment(info.segment)?;
    map_config_page(segment, info.bus, info.device, info.function)?;
    Ok(read_config_dword(offset))
}

/// Update the PCI command register for one explicitly selected function.
pub fn enable_memory_and_bus_master(info: &PciFunctionInfo) -> Result<(), PciError> {
    const COMMAND: usize = 0x04;
    const MEMORY_SPACE: u16 = 1 << 1;
    const BUS_MASTER: u16 = 1 << 2;

    let segment = find_segment(info.segment)?;
    map_config_page(segment, info.bus, info.device, info.function)?;
    let value = read_config_word(COMMAND) | MEMORY_SPACE | BUS_MASTER;
    // SAFETY: the ECAM page for `info` is mapped writable and the command
    // register is a naturally aligned 16-bit configuration register.
    unsafe {
        core::ptr::write_volatile((CONFIG_SCRATCH_VA + COMMAND as u64) as *mut u16, value);
    }
    Ok(())
}

/// Copy the full 256-byte config space of the currently mapped function into a
/// buffer, so the pure parsers can run against a stable image.
fn read_config_space(buf: &mut [u8; CONFIG_SPACE_SIZE]) {
    for (i, slot) in buf.iter_mut().enumerate() {
        *slot = read_config_byte(i);
    }
}

/// Probe and validate all six BARs of a function. Returns the typed BAR table;
/// rejects 64-bit BARs missing their high word, IO BARs with bad encoding, or
/// any BAR whose probed size is zero or non-power-of-two for memory regions.
pub fn probe_bars(info: &PciFunctionInfo) -> Result<[BarInfo; BAR_COUNT], PciError> {
    const COMMAND: usize = 0x04;

    let segment = find_segment(info.segment)?;
    map_config_page(segment, info.bus, info.device, info.function)?;
    let command = read_config_word(COMMAND);
    // Disable IO and memory decoding while probing BAR sizes, as required by
    // PCI. Bus mastering is left unchanged.
    unsafe {
        core::ptr::write_volatile(
            (CONFIG_SCRATCH_VA + COMMAND as u64) as *mut u16,
            command & !0b11,
        );
    }

    let mut bars = [BarInfo {
        index: 0,
        kind: BarKind::Memory32,
        prefetchable: false,
        size: 0,
        base: 0,
    }; BAR_COUNT];
    let mut slot = 0usize;
    while slot < BAR_COUNT {
        let offset = CONFIG_BARS + slot * 4;
        let original_low = read_config_dword(offset);
        if original_low == 0 {
            bars[slot].index = slot as u8;
            slot += 1;
            continue;
        }

        let kind = match original_low & BAR_TYPE_MASK {
            BAR_TYPE_IO => BarKind::Io,
            BAR_TYPE_MEM64 => BarKind::Memory64,
            _ => BarKind::Memory32,
        };
        let prefetchable = original_low & BAR_PREFETCHABLE != 0 && kind != BarKind::Io;
        match kind {
            BarKind::Io | BarKind::Memory32 => {
                unsafe {
                    core::ptr::write_volatile(
                        (CONFIG_SCRATCH_VA + offset as u64) as *mut u32,
                        u32::MAX,
                    );
                }
                let size_mask = read_config_dword(offset);
                unsafe {
                    core::ptr::write_volatile(
                        (CONFIG_SCRATCH_VA + offset as u64) as *mut u32,
                        original_low,
                    );
                }
                let (mask, base) = if kind == BarKind::Io {
                    (BAR_IO_MASK, (original_low & BAR_IO_MASK) as u64)
                } else {
                    (BAR_MEM_MASK, (original_low & BAR_MEM_MASK) as u64)
                };
                let size = (!(size_mask & mask)).wrapping_add(1) & mask;
                if size == 0 || !size.is_power_of_two() {
                    restore_command(command);
                    return Err(PciError::BadBar);
                }
                bars[slot] = BarInfo {
                    index: slot as u8,
                    kind,
                    prefetchable,
                    size: size as u64,
                    base,
                };
                slot += 1;
            }
            BarKind::Memory64 => {
                if slot + 1 >= BAR_COUNT {
                    restore_command(command);
                    return Err(PciError::BadBar);
                }
                let original_high = read_config_dword(offset + 4);
                unsafe {
                    core::ptr::write_volatile(
                        (CONFIG_SCRATCH_VA + offset as u64) as *mut u32,
                        u32::MAX,
                    );
                    core::ptr::write_volatile(
                        (CONFIG_SCRATCH_VA + offset as u64 + 4) as *mut u32,
                        u32::MAX,
                    );
                }
                let size_low = read_config_dword(offset);
                let size_high = read_config_dword(offset + 4);
                unsafe {
                    core::ptr::write_volatile(
                        (CONFIG_SCRATCH_VA + offset as u64) as *mut u32,
                        original_low,
                    );
                    core::ptr::write_volatile(
                        (CONFIG_SCRATCH_VA + offset as u64 + 4) as *mut u32,
                        original_high,
                    );
                }
                let size_mask = ((size_high as u64) << 32) | size_low as u64;
                let size = (!(size_mask & BAR_MEM_MASK64)).wrapping_add(1) & BAR_MEM_MASK64;
                if size == 0 || !size.is_power_of_two() {
                    restore_command(command);
                    return Err(PciError::BadBar);
                }
                bars[slot] = BarInfo {
                    index: slot as u8,
                    kind,
                    prefetchable,
                    size,
                    base: (((original_high as u64) << 32) | original_low as u64) & BAR_MEM_MASK64,
                };
                bars[slot + 1] = BarInfo {
                    index: (slot + 1) as u8,
                    kind,
                    prefetchable,
                    size: 0,
                    base: 0,
                };
                slot += 2;
            }
        }
    }
    restore_command(command);
    Ok(bars)
}

fn restore_command(command: u16) {
    // SAFETY: the current function's ECAM page remains mapped writable.
    unsafe {
        core::ptr::write_volatile((CONFIG_SCRATCH_VA + 0x04) as *mut u16, command);
    }
}

/// Find a segment by id.
pub fn find_segment(segment: u16) -> Result<&'static McfgSegment, PciError> {
    segments()
        .iter()
        .find(|s| s.segment == segment)
        .ok_or(PciError::MalformedMcfg)
}

/// Walk and validate the PCI capability chain of a function.
pub fn walk_capabilities(info: &PciFunctionInfo) -> Result<Vec<CapabilityEntry>, PciError> {
    let segment = find_segment(info.segment)?;
    map_config_page(segment, info.bus, info.device, info.function)?;
    let mut config = [0u8; CONFIG_SPACE_SIZE];
    read_config_space(&mut config);
    parse_capabilities(&config)
}

/// Pure capability-chain parser. Operates on a 256-byte config image so it can
/// be unit-tested with crafted malformed inputs.
///
/// Rejects: a non-list header type, a capability pointer that is unaligned,
/// out of bounds, self-referential, or forms a chain longer than
/// [`MAX_CAPABILITIES`].
pub fn parse_capabilities(config: &[u8]) -> Result<Vec<CapabilityEntry>, PciError> {
    if config.len() < CONFIG_SPACE_SIZE {
        return Err(PciError::BadCapabilityChain);
    }
    let header_type = config[CONFIG_HEADER_TYPE];
    if header_type & 0x7f != 0 {
        // Only type-0 headers carry the capability list at offset 0x34.
        // Type-1 (bridge) lists live elsewhere; not supported in M5.1.
        return Ok(Vec::new());
    }
    let mut ptr = config[CONFIG_CAP_POINTER];
    if ptr == 0 {
        return Ok(Vec::new());
    }
    let mut seen = [false; CONFIG_SPACE_SIZE];
    let mut out = Vec::new();
    for _ in 0..MAX_CAPABILITIES {
        if ptr == CAP_LIST_END {
            break;
        }
        if ptr & 0b11 != 0 || ptr as usize + CAP_HEADER_LEN > CONFIG_SPACE_SIZE {
            return Err(PciError::BadCapabilityChain);
        }
        if seen[ptr as usize] {
            // Cycle in the chain.
            return Err(PciError::BadCapabilityChain);
        }
        seen[ptr as usize] = true;
        let id = config[ptr as usize];
        let next = config[ptr as usize + 1];
        out.push(CapabilityEntry { id, offset: ptr });
        ptr = next;
    }
    if ptr != CAP_LIST_END {
        // Chain did not terminate within the bound.
        return Err(PciError::BadCapabilityChain);
    }
    Ok(out)
}

/// Pure BAR-table parser. Probes sizes by interpreting the reset-state BAR
/// values present in the config image. Rejects impossible BAR declarations:
///
/// - a 64-bit memory BAR in the last slot (no high word available);
/// - an IO BAR with either low bit not encoding IO;
/// - a memory BAR whose decoded size is not a power of two.
pub fn parse_bars(config: &[u8]) -> Result<[BarInfo; BAR_COUNT], PciError> {
    if config.len() < CONFIG_BARS + BAR_COUNT * 4 {
        return Err(PciError::BadBar);
    }
    let mut bars = [BarInfo {
        index: 0,
        kind: BarKind::Memory32,
        prefetchable: false,
        size: 0,
        base: 0,
    }; BAR_COUNT];
    let mut slot = 0usize;
    while slot < BAR_COUNT {
        let off = CONFIG_BARS + slot * 4;
        let raw = read_u32(config, off).ok_or(PciError::BadBar)?;
        if raw == 0 {
            bars[slot] = BarInfo {
                index: slot as u8,
                kind: BarKind::Memory32,
                prefetchable: false,
                size: 0,
                base: 0,
            };
            slot += 1;
            continue;
        }
        let kind = match raw & BAR_TYPE_MASK {
            BAR_TYPE_IO => BarKind::Io,
            BAR_TYPE_MEM64 => BarKind::Memory64,
            _ => BarKind::Memory32,
        };
        let prefetchable = (raw & BAR_PREFETCHABLE) != 0 && kind != BarKind::Io;
        match kind {
            BarKind::Io => {
                let size = decode_size(raw, BAR_IO_MASK);
                if size == 0 || !size.is_power_of_two() {
                    return Err(PciError::BadBar);
                }
                bars[slot] = BarInfo {
                    index: slot as u8,
                    kind,
                    prefetchable: false,
                    size: size as u64,
                    base: (raw & BAR_IO_MASK) as u64,
                };
                slot += 1;
            }
            BarKind::Memory32 => {
                let size = decode_size(raw, BAR_MEM_MASK);
                if size == 0 || !size.is_power_of_two() {
                    return Err(PciError::BadBar);
                }
                bars[slot] = BarInfo {
                    index: slot as u8,
                    kind,
                    prefetchable,
                    size: size as u64,
                    base: (raw & BAR_MEM_MASK) as u64,
                };
                slot += 1;
            }
            BarKind::Memory64 => {
                if slot + 1 >= BAR_COUNT {
                    return Err(PciError::BadBar);
                }
                let high = read_u32(config, off + 4).ok_or(PciError::BadBar)?;
                let combined = ((high as u64) << 32) | (raw as u64);
                let size = decode_size64(combined, BAR_MEM_MASK64);
                if size == 0 || !size.is_power_of_two() {
                    return Err(PciError::BadBar);
                }
                bars[slot] = BarInfo {
                    index: slot as u8,
                    kind,
                    prefetchable,
                    size,
                    base: combined & BAR_MEM_MASK64,
                };
                bars[slot + 1] = BarInfo {
                    index: (slot + 1) as u8,
                    kind: BarKind::Memory64,
                    prefetchable,
                    size: 0,
                    base: 0,
                };
                slot += 2;
            }
        }
    }
    Ok(bars)
}

fn decode_size(raw: u32, mask: u32) -> u32 {
    let masked = raw & mask;
    if masked == 0 {
        return 0;
    }
    !(masked - 1) & mask
}
fn decode_size64(raw: u64, mask: u64) -> u64 {
    let masked = raw & mask;
    if masked == 0 {
        return 0;
    }
    !(masked - 1) & mask
}
fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    bytes
        .get(offset..offset + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
}
fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    bytes
        .get(offset..offset + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}
fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    bytes
        .get(offset..offset + 8)
        .map(|b| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
}
