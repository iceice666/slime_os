//! Bounded ACPI discovery for platform bring-up.
//!
//! Limine supplies the RSDP address. This module validates every checksum and
//! length before following the RSDT/XSDT, then copies only the platform facts
//! the kernel needs: MADT interrupt routing and FADT power-management fields.
//! Firmware tables stay firmware-owned; no policy is derived from them here.

use core::slice;

use spin::Once;

use crate::memory::PhysAddr;

const SDT_HEADER_LEN: usize = 36;
const RSDP_V1_LEN: usize = 20;
const RSDP_V2_MIN_LEN: usize = 36;
const MAX_RSDP_LEN: usize = 4096;
const MAX_TABLE_LEN: usize = 1024 * 1024;
const MAX_ROOT_ENTRIES: usize = 256;
const MAX_IO_APICS: usize = 8;
const MAX_OVERRIDES: usize = 16;

static TABLES: Once<AcpiInfo> = Once::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpiError {
    MissingRsdp,
    UnsupportedLimineRevision,
    InvalidRsdpSignature,
    InvalidRsdpLength,
    InvalidChecksum,
    InvalidAddress,
    InvalidTableLength,
    InvalidRootSignature,
    TooManyRootEntries,
    MissingMadt,
    MissingFadt,
    MissingDsdt,
    MissingSleepState,
    MalformedMadt,
    MalformedFadt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootKind {
    Rsdt,
    Xsdt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedRsdp {
    pub revision: u8,
    pub rsdt_address: u32,
    pub xsdt_address: u64,
    pub length: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenericAddress {
    pub address_space: u8,
    pub bit_width: u8,
    pub bit_offset: u8,
    pub access_size: u8,
    pub address: u64,
}

impl GenericAddress {
    pub const fn system_io(address: u32, bit_width: u8) -> Self {
        Self {
            address_space: 1,
            bit_width,
            bit_offset: 0,
            access_size: 0,
            address: address as u64,
        }
    }

    pub const fn is_present(self) -> bool {
        self.address != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoApicInfo {
    pub id: u8,
    pub address: u32,
    pub gsi_base: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterruptOverride {
    pub source_irq: u8,
    pub gsi: u32,
    pub flags: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterruptRoute {
    pub gsi: u32,
    pub active_low: bool,
    pub level_triggered: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MadtInfo {
    pub local_apic_address: u64,
    pub io_apics: [Option<IoApicInfo>; MAX_IO_APICS],
    pub overrides: [Option<InterruptOverride>; MAX_OVERRIDES],
}

impl MadtInfo {
    pub fn route_for_isa_irq(&self, irq: u8) -> InterruptRoute {
        let Some(route) = self
            .overrides
            .iter()
            .flatten()
            .find(|route| route.source_irq == irq)
        else {
            return InterruptRoute {
                gsi: irq as u32,
                active_low: false,
                level_triggered: false,
            };
        };

        let polarity = route.flags & 0b11;
        let trigger = (route.flags >> 2) & 0b11;
        InterruptRoute {
            gsi: route.gsi,
            // ISA-conforming (00) means active-high and edge-triggered.
            active_low: polarity == 0b11,
            level_triggered: trigger == 0b11,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerInfo {
    pub hardware_reduced: bool,
    pub reset_register: Option<GenericAddress>,
    pub reset_value: u8,
    pub pm1a_control: Option<GenericAddress>,
    pub pm1b_control: Option<GenericAddress>,
    pub sleep_control: Option<GenericAddress>,
    pub sleep_status: Option<GenericAddress>,
    pub s5_type_a: u8,
    pub s5_type_b: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcpiInfo {
    pub revision: u8,
    pub root_kind: RootKind,
    pub table_count: usize,
    pub madt: MadtInfo,
    pub power: PowerInfo,
    pub i8042_present: bool,
    /// The raw MCFG table, if firmware provided one. PCI ECAM parsing lives
    /// in [`crate::pci`]; ACPI only surfaces the validated table bytes.
    pub mcfg: Option<&'static [u8]>,
}

#[derive(Clone, Copy)]
struct FadtInfo {
    dsdt_address: u64,
    hardware_reduced: bool,
    reset_register: Option<GenericAddress>,
    reset_value: u8,
    pm1a_control: Option<GenericAddress>,
    pm1b_control: Option<GenericAddress>,
    sleep_control: Option<GenericAddress>,
    sleep_status: Option<GenericAddress>,
    i8042_present: bool,
}

pub fn init() -> Result<&'static AcpiInfo, AcpiError> {
    let info = discover()?;
    Ok(TABLES.call_once(|| info))
}

pub fn get() -> Option<&'static AcpiInfo> {
    TABLES.get()
}

pub fn parse_rsdp(bytes: &[u8]) -> Result<ParsedRsdp, AcpiError> {
    if bytes.len() < RSDP_V1_LEN {
        return Err(AcpiError::InvalidRsdpLength);
    }
    if bytes.get(..8) != Some(b"RSD PTR ") {
        return Err(AcpiError::InvalidRsdpSignature);
    }
    if !checksum_valid(&bytes[..RSDP_V1_LEN]) {
        return Err(AcpiError::InvalidChecksum);
    }

    let revision = bytes[15];
    let rsdt_address = read_u32(bytes, 16).ok_or(AcpiError::InvalidRsdpLength)?;
    if revision < 2 {
        return Ok(ParsedRsdp {
            revision,
            rsdt_address,
            xsdt_address: 0,
            length: RSDP_V1_LEN as u32,
        });
    }

    let length = read_u32(bytes, 20).ok_or(AcpiError::InvalidRsdpLength)? as usize;
    if !(RSDP_V2_MIN_LEN..=MAX_RSDP_LEN).contains(&length) || bytes.len() < length {
        return Err(AcpiError::InvalidRsdpLength);
    }
    if !checksum_valid(&bytes[..length]) {
        return Err(AcpiError::InvalidChecksum);
    }
    Ok(ParsedRsdp {
        revision,
        rsdt_address,
        xsdt_address: read_u64(bytes, 24).ok_or(AcpiError::InvalidRsdpLength)?,
        length: length as u32,
    })
}

pub fn parse_s5_aml(aml: &[u8]) -> Option<(u8, u8)> {
    let mut offset = 0;
    while offset + 6 <= aml.len() {
        if aml[offset] != 0x08 {
            offset += 1;
            continue;
        }

        let mut name = offset + 1;
        if aml.get(name) == Some(&b'\\') {
            name += 1;
        }
        if aml.get(name..name + 4) != Some(b"_S5_") {
            offset += 1;
            continue;
        }

        let package = name + 4;
        if aml.get(package) != Some(&0x12) {
            offset += 1;
            continue;
        }
        let (package_len, length_bytes) = decode_package_length(aml.get(package + 1..)?)?;
        let body = package + 1 + length_bytes;
        let end = package.checked_add(1)?.checked_add(package_len)?;
        if body >= end || end > aml.len() || aml[body] < 2 {
            offset += 1;
            continue;
        }

        let (type_a, used_a) = decode_aml_integer(aml.get(body + 1..end)?)?;
        let (type_b, _) = decode_aml_integer(aml.get(body + 1 + used_a..end)?)?;
        if type_a <= u8::MAX as u64 && type_b <= u8::MAX as u64 {
            return Some((type_a as u8, type_b as u8));
        }
        offset += 1;
    }
    None
}

fn discover() -> Result<AcpiInfo, AcpiError> {
    let raw = crate::boot::rsdp_address();
    if raw == 0 {
        return Err(AcpiError::MissingRsdp);
    }
    let rsdp_virt = PhysAddr(raw).to_virt().as_u64();
    let legacy = unsafe { slice::from_raw_parts(rsdp_virt as *const u8, RSDP_V1_LEN) };
    let rsdp_len = if legacy[15] >= 2 {
        let extended_prefix = unsafe { slice::from_raw_parts(rsdp_virt as *const u8, 24) };
        read_u32(extended_prefix, 20).ok_or(AcpiError::InvalidRsdpLength)? as usize
    } else {
        RSDP_V1_LEN
    };
    if !(RSDP_V1_LEN..=MAX_RSDP_LEN).contains(&rsdp_len) {
        return Err(AcpiError::InvalidRsdpLength);
    }
    let rsdp_bytes = unsafe { slice::from_raw_parts(rsdp_virt as *const u8, rsdp_len) };
    let rsdp = parse_rsdp(rsdp_bytes)?;

    let (root_kind, root_address, entry_size) = if rsdp.revision >= 2 && rsdp.xsdt_address != 0 {
        (RootKind::Xsdt, rsdp.xsdt_address, 8)
    } else {
        (RootKind::Rsdt, rsdp.rsdt_address as u64, 4)
    };
    let root = read_sdt(root_address)?;
    let expected = match root_kind {
        RootKind::Rsdt => b"RSDT",
        RootKind::Xsdt => b"XSDT",
    };
    if root.get(..4) != Some(expected) {
        return Err(AcpiError::InvalidRootSignature);
    }
    let entries = root
        .len()
        .checked_sub(SDT_HEADER_LEN)
        .ok_or(AcpiError::InvalidTableLength)?
        / entry_size;
    if entries > MAX_ROOT_ENTRIES {
        return Err(AcpiError::TooManyRootEntries);
    }

    let mut madt = None;
    let mut fadt = None;
    let mut mcfg = None;
    for index in 0..entries {
        let offset = SDT_HEADER_LEN + index * entry_size;
        let address = if entry_size == 8 {
            read_u64(root, offset).ok_or(AcpiError::InvalidTableLength)?
        } else {
            read_u32(root, offset).ok_or(AcpiError::InvalidTableLength)? as u64
        };
        if address == 0 {
            continue;
        }
        let table = read_sdt(address)?;
        match table.get(..4) {
            Some(b"APIC") if madt.is_none() => madt = Some(parse_madt(table)?),
            Some(b"FACP") if fadt.is_none() => fadt = Some(parse_fadt(table)?),
            Some(b"MCFG") if mcfg.is_none() => mcfg = Some(table),
            _ => {}
        }
    }

    let madt = madt.ok_or(AcpiError::MissingMadt)?;
    let fadt = fadt.ok_or(AcpiError::MissingFadt)?;
    let dsdt = read_sdt(fadt.dsdt_address).map_err(|_| AcpiError::MissingDsdt)?;
    if dsdt.get(..4) != Some(b"DSDT") {
        return Err(AcpiError::MissingDsdt);
    }
    let (s5_type_a, s5_type_b) =
        parse_s5_aml(&dsdt[SDT_HEADER_LEN..]).ok_or(AcpiError::MissingSleepState)?;

    Ok(AcpiInfo {
        revision: rsdp.revision,
        root_kind,
        table_count: entries,
        madt,
        power: PowerInfo {
            hardware_reduced: fadt.hardware_reduced,
            reset_register: fadt.reset_register,
            reset_value: fadt.reset_value,
            pm1a_control: fadt.pm1a_control,
            pm1b_control: fadt.pm1b_control,
            sleep_control: fadt.sleep_control,
            sleep_status: fadt.sleep_status,
            s5_type_a,
            s5_type_b,
        },
        i8042_present: fadt.i8042_present,
        mcfg,
    })
}

fn parse_madt(table: &[u8]) -> Result<MadtInfo, AcpiError> {
    if table.len() < 44 {
        return Err(AcpiError::MalformedMadt);
    }
    let mut info = MadtInfo {
        local_apic_address: read_u32(table, 36).ok_or(AcpiError::MalformedMadt)? as u64,
        io_apics: [None; MAX_IO_APICS],
        overrides: [None; MAX_OVERRIDES],
    };
    let mut io_apic_count = 0;
    let mut override_count = 0;
    let mut offset = 44;
    while offset < table.len() {
        let kind = *table.get(offset).ok_or(AcpiError::MalformedMadt)?;
        let length = *table.get(offset + 1).ok_or(AcpiError::MalformedMadt)? as usize;
        if length < 2 || offset + length > table.len() {
            return Err(AcpiError::MalformedMadt);
        }
        match (kind, length) {
            (1, 12) if io_apic_count < MAX_IO_APICS => {
                info.io_apics[io_apic_count] = Some(IoApicInfo {
                    id: table[offset + 2],
                    address: read_u32(table, offset + 4).ok_or(AcpiError::MalformedMadt)?,
                    gsi_base: read_u32(table, offset + 8).ok_or(AcpiError::MalformedMadt)?,
                });
                io_apic_count += 1;
            }
            (2, 10) if table[offset + 2] == 0 && override_count < MAX_OVERRIDES => {
                info.overrides[override_count] = Some(InterruptOverride {
                    source_irq: table[offset + 3],
                    gsi: read_u32(table, offset + 4).ok_or(AcpiError::MalformedMadt)?,
                    flags: read_u16(table, offset + 8).ok_or(AcpiError::MalformedMadt)?,
                });
                override_count += 1;
            }
            (5, 12) => {
                info.local_apic_address =
                    read_u64(table, offset + 4).ok_or(AcpiError::MalformedMadt)?;
            }
            _ => {}
        }
        offset += length;
    }
    if io_apic_count == 0 {
        return Err(AcpiError::MalformedMadt);
    }
    Ok(info)
}

fn parse_fadt(table: &[u8]) -> Result<FadtInfo, AcpiError> {
    if table.len() < 116 {
        return Err(AcpiError::MalformedFadt);
    }
    let legacy_dsdt = read_u32(table, 40).ok_or(AcpiError::MalformedFadt)? as u64;
    let extended_dsdt = read_u64(table, 140).unwrap_or(0);
    let dsdt_address = if extended_dsdt != 0 {
        extended_dsdt
    } else {
        legacy_dsdt
    };
    if dsdt_address == 0 {
        return Err(AcpiError::MissingDsdt);
    }

    let flags = read_u32(table, 112).ok_or(AcpiError::MalformedFadt)?;
    let reset_register = parse_gas(table, 116).filter(|register| register.is_present());
    let reset_value = table.get(128).copied().unwrap_or(0);
    let legacy_pm1a = read_u32(table, 64).unwrap_or(0);
    let legacy_pm1b = read_u32(table, 68).unwrap_or(0);
    let extended_pm1a = parse_gas(table, 172).filter(|register| register.is_present());
    let extended_pm1b = parse_gas(table, 184).filter(|register| register.is_present());
    let pm1a_control = extended_pm1a
        .or_else(|| (legacy_pm1a != 0).then_some(GenericAddress::system_io(legacy_pm1a, 16)));
    let pm1b_control = extended_pm1b
        .or_else(|| (legacy_pm1b != 0).then_some(GenericAddress::system_io(legacy_pm1b, 16)));

    let iapc_boot_arch = read_u16(table, 109);
    Ok(FadtInfo {
        dsdt_address,
        hardware_reduced: flags & (1 << 20) != 0,
        reset_register,
        reset_value,
        pm1a_control,
        pm1b_control,
        sleep_control: parse_gas(table, 244).filter(|register| register.is_present()),
        sleep_status: parse_gas(table, 256).filter(|register| register.is_present()),
        // ACPI 1.0 tables lack this field; probing is the only available path.
        i8042_present: iapc_boot_arch.is_none_or(|arch| arch & (1 << 1) != 0),
    })
}

fn read_sdt(physical: u64) -> Result<&'static [u8], AcpiError> {
    let header = read_physical(physical, SDT_HEADER_LEN)?;
    let length = read_u32(header, 4).ok_or(AcpiError::InvalidTableLength)? as usize;
    if !(SDT_HEADER_LEN..=MAX_TABLE_LEN).contains(&length) {
        return Err(AcpiError::InvalidTableLength);
    }
    let table = read_physical(physical, length)?;
    if !checksum_valid(table) {
        return Err(AcpiError::InvalidChecksum);
    }
    Ok(table)
}

fn read_physical(physical: u64, length: usize) -> Result<&'static [u8], AcpiError> {
    if !physical_range_valid(physical, length) {
        return Err(AcpiError::InvalidAddress);
    }
    let virtual_address = PhysAddr(physical).to_virt().as_u64();
    // SAFETY: the complete range is contained in a firmware memory-map entry,
    // and Limine's HHDM maps physical memory at this virtual address.
    Ok(unsafe { slice::from_raw_parts(virtual_address as *const u8, length) })
}

fn physical_range_valid(address: u64, length: usize) -> bool {
    let Some(end) = address.checked_add(length as u64) else {
        return false;
    };
    crate::boot::memory_map()
        .iter()
        .any(|entry| address >= entry.base && end <= entry.base.saturating_add(entry.length))
}

fn parse_gas(bytes: &[u8], offset: usize) -> Option<GenericAddress> {
    let raw = bytes.get(offset..offset + 12)?;
    Some(GenericAddress {
        address_space: raw[0],
        bit_width: raw[1],
        bit_offset: raw[2],
        access_size: raw[3],
        address: read_u64(raw, 4)?,
    })
}

fn checksum_valid(bytes: &[u8]) -> bool {
    bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)) == 0
}

fn decode_package_length(bytes: &[u8]) -> Option<(usize, usize)> {
    let first = *bytes.first()?;
    let following = (first >> 6) as usize;
    if following == 0 {
        return Some(((first & 0x3f) as usize, 1));
    }
    if bytes.len() < following + 1 {
        return None;
    }
    let mut length = (first & 0x0f) as usize;
    for index in 0..following {
        length |= (bytes[index + 1] as usize) << (4 + index * 8);
    }
    Some((length, following + 1))
}

fn decode_aml_integer(bytes: &[u8]) -> Option<(u64, usize)> {
    match *bytes.first()? {
        0x00 => Some((0, 1)),
        0x01 => Some((1, 1)),
        0x0a => Some((*bytes.get(1)? as u64, 2)),
        0x0b => Some((read_u16(bytes, 1)? as u64, 3)),
        0x0c => Some((read_u32(bytes, 1)? as u64, 5)),
        0x0e => Some((read_u64(bytes, 1)?, 9)),
        _ => None,
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(offset..offset + 8)?.try_into().ok()?,
    ))
}
