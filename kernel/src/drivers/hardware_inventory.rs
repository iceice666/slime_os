//! Bounded, versioned hardware inventory for Framework evidence boots.
//!
//! The report is line-oriented and framed so the host harness can extract the
//! same canonical bytes from serial on every boot. Every line is also emitted
//! to the framebuffer. Raw firmware memory, storage payloads, and descriptor
//! dumps are deliberately excluded.

use alloc::vec::Vec;

use crate::acpi::{AcpiInfo, InterruptRoute};
use crate::boot::Framebuffer;
use crate::capability::PciFunctionInfo;
use crate::input::InputInitReport;
use crate::nvme::NvmeBlock;
use crate::pci::{self, BarInfo};
use crate::{println, serial_println};

pub const REPORT_VERSION: u32 = 1;
pub const MAX_REPORTED_FUNCTIONS: usize = 256;
pub const MAX_REPORTED_BARS: usize = MAX_REPORTED_FUNCTIONS * 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryError {
    InvalidFramebuffer,
    TooManyFunctions,
    TooManyBars,
    PciDiscovery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FramebufferGeometry {
    pub width: u64,
    pub height: u64,
    pub pitch: u64,
    pub bpp: u16,
    pub stride: u64,
    pub byte_len: u64,
    pub memory_model: u8,
}

pub fn framebuffer_geometry(fb: Framebuffer) -> Result<FramebufferGeometry, InventoryError> {
    if fb.width == 0 || fb.height == 0 || fb.bpp == 0 || !fb.bpp.is_multiple_of(8) {
        return Err(InventoryError::InvalidFramebuffer);
    }
    let bytes_per_pixel = u64::from(fb.bpp / 8);
    if bytes_per_pixel == 0 || !fb.pitch.is_multiple_of(bytes_per_pixel) {
        return Err(InventoryError::InvalidFramebuffer);
    }
    let stride = fb.pitch / bytes_per_pixel;
    let byte_len = fb
        .pitch
        .checked_mul(fb.height)
        .ok_or(InventoryError::InvalidFramebuffer)?;
    if stride < fb.width || byte_len == 0 {
        return Err(InventoryError::InvalidFramebuffer);
    }
    Ok(FramebufferGeometry {
        width: fb.width,
        height: fb.height,
        pitch: fb.pitch,
        bpp: fb.bpp,
        stride,
        byte_len,
        memory_model: fb.memory_model,
    })
}

pub fn normalize_functions(
    functions: &[PciFunctionInfo],
) -> Result<Vec<PciFunctionInfo>, InventoryError> {
    if functions.len() > MAX_REPORTED_FUNCTIONS {
        return Err(InventoryError::TooManyFunctions);
    }
    let mut normalized = functions.to_vec();
    normalized.sort_unstable_by_key(|function| {
        (
            function.segment,
            function.bus,
            function.device,
            function.function,
        )
    });
    Ok(normalized)
}

fn emit_begin() {
    serial_println!("[hw-report] begin version={}", REPORT_VERSION);
    println!("[hw-report] begin version={}", REPORT_VERSION);
}

fn emit_end() {
    serial_println!("[hw-report] end version={}", REPORT_VERSION);
    println!("[hw-report] end version={}", REPORT_VERSION);
}

fn emit_acpi(platform: &AcpiInfo) {
    serial_println!(
        "[hw-report] acpi revision={} root={:?} table_count={} ivrs={} dmar={}",
        platform.revision,
        platform.root_kind,
        platform.table_count,
        platform.iommu_ivrs_present,
        platform.iommu_dmar_present,
    );
    println!(
        "[hw-report] acpi revision={} root={:?} table_count={} ivrs={} dmar={}",
        platform.revision,
        platform.root_kind,
        platform.table_count,
        platform.iommu_ivrs_present,
        platform.iommu_dmar_present,
    );
    for (index, table) in platform.tables.iter().flatten().enumerate() {
        serial_println!(
            "[hw-report] acpi_table index={} sig={:02x}{:02x}{:02x}{:02x} len={} revision={} checksum={:02x} oem={:02x}{:02x}{:02x}{:02x}{:02x}{:02x} table={:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            index,
            table.signature[0],
            table.signature[1],
            table.signature[2],
            table.signature[3],
            table.length,
            table.revision,
            table.checksum,
            table.oem_id[0],
            table.oem_id[1],
            table.oem_id[2],
            table.oem_id[3],
            table.oem_id[4],
            table.oem_id[5],
            table.oem_table_id[0],
            table.oem_table_id[1],
            table.oem_table_id[2],
            table.oem_table_id[3],
            table.oem_table_id[4],
            table.oem_table_id[5],
            table.oem_table_id[6],
            table.oem_table_id[7],
        );
        println!(
            "[hw-report] acpi_table index={} sig={:02x}{:02x}{:02x}{:02x} len={} revision={} checksum={:02x} oem={:02x}{:02x}{:02x}{:02x}{:02x}{:02x} table={:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            index,
            table.signature[0],
            table.signature[1],
            table.signature[2],
            table.signature[3],
            table.length,
            table.revision,
            table.checksum,
            table.oem_id[0],
            table.oem_id[1],
            table.oem_id[2],
            table.oem_id[3],
            table.oem_id[4],
            table.oem_id[5],
            table.oem_table_id[0],
            table.oem_table_id[1],
            table.oem_table_id[2],
            table.oem_table_id[3],
            table.oem_table_id[4],
            table.oem_table_id[5],
            table.oem_table_id[6],
            table.oem_table_id[7],
        );
    }
}

fn emit_interrupts(platform: &AcpiInfo) {
    for io_apic in platform.madt.io_apics.iter().flatten() {
        serial_println!(
            "[hw-report] ioapic id={} address={:#010x} gsi_base={}",
            io_apic.id,
            io_apic.address,
            io_apic.gsi_base,
        );
        println!(
            "[hw-report] ioapic id={} address={:#010x} gsi_base={}",
            io_apic.id, io_apic.address, io_apic.gsi_base,
        );
    }
    for route in platform.madt.overrides.iter().flatten() {
        let normalized = platform.madt.route_for_isa_irq(route.source_irq);
        emit_route(route.source_irq, normalized);
    }
    if !platform
        .madt
        .overrides
        .iter()
        .flatten()
        .any(|route| route.source_irq == 1)
    {
        emit_route(1, platform.madt.route_for_isa_irq(1));
    }
}

fn emit_route(source_irq: u8, route: InterruptRoute) {
    serial_println!(
        "[hw-report] interrupt source_irq={} gsi={} active_low={} level={}",
        source_irq,
        route.gsi,
        route.active_low,
        route.level_triggered,
    );
    println!(
        "[hw-report] interrupt source_irq={} gsi={} active_low={} level={}",
        source_irq, route.gsi, route.active_low, route.level_triggered,
    );
}

fn emit_framebuffer() -> Result<(), InventoryError> {
    let geometry = framebuffer_geometry(crate::boot::framebuffer())?;
    serial_println!(
        "[hw-report] framebuffer width={} height={} pitch={} bpp={} stride={} bytes={} model={}",
        geometry.width,
        geometry.height,
        geometry.pitch,
        geometry.bpp,
        geometry.stride,
        geometry.byte_len,
        geometry.memory_model,
    );
    println!(
        "[hw-report] framebuffer width={} height={} pitch={} bpp={} stride={} bytes={} model={}",
        geometry.width,
        geometry.height,
        geometry.pitch,
        geometry.bpp,
        geometry.stride,
        geometry.byte_len,
        geometry.memory_model,
    );
    Ok(())
}

fn emit_bar(function: &PciFunctionInfo, bar: &BarInfo) {
    serial_println!(
        "[hw-report] bar bdf={}:{:02x}:{:02x}.{} index={} kind={:?} prefetch={} base={:#018x} size=unknown",
        function.segment,
        function.bus,
        function.device,
        function.function,
        bar.index,
        bar.kind,
        bar.prefetchable,
        bar.base,
    );
    println!(
        "[hw-report] bar bdf={}:{:02x}:{:02x}.{} index={} kind={:?} prefetch={} base={:#018x} size=unknown",
        function.segment,
        function.bus,
        function.device,
        function.function,
        bar.index,
        bar.kind,
        bar.prefetchable,
        bar.base,
    );
}

fn emit_pci(functions: &[PciFunctionInfo]) -> Result<(), InventoryError> {
    let normalized = normalize_functions(functions)?;
    let mut reported_bars = 0usize;
    for function in normalized {
        serial_println!(
            "[hw-report] pci bdf={}:{:02x}:{:02x}.{} vendor={:#06x} device={:#06x} class={:#08x}",
            function.segment,
            function.bus,
            function.device,
            function.function,
            function.vendor_id,
            function.device_id,
            function.class_code,
        );
        println!(
            "[hw-report] pci bdf={}:{:02x}:{:02x}.{} vendor={:#06x} device={:#06x} class={:#08x}",
            function.segment,
            function.bus,
            function.device,
            function.function,
            function.vendor_id,
            function.device_id,
            function.class_code,
        );
        match pci::assigned_bars(&function) {
            Ok(bars) => {
                for bar in bars.iter().filter(|bar| bar.base != 0) {
                    reported_bars += 1;
                    if reported_bars > MAX_REPORTED_BARS {
                        return Err(InventoryError::TooManyBars);
                    }
                    emit_bar(&function, bar);
                }
            }
            Err(error) => {
                serial_println!(
                    "[hw-report] bar_error bdf={}:{:02x}:{:02x}.{} error={:?}",
                    function.segment,
                    function.bus,
                    function.device,
                    function.function,
                    error,
                );
                println!(
                    "[hw-report] bar_error bdf={}:{:02x}:{:02x}.{} error={:?}",
                    function.segment, function.bus, function.device, function.function, error,
                );
            }
        }
    }
    Ok(())
}

fn emit_input(report: &InputInitReport) {
    serial_println!(
        "[hw-report] input path={:?} stage_count={}",
        report.path,
        report.len
    );
    println!(
        "[hw-report] input path={:?} stage_count={}",
        report.path, report.len
    );
    for (index, record) in report.stages[..report.len].iter().flatten().enumerate() {
        serial_println!(
            "[hw-report] input_stage index={} stage={:?} error={:?}",
            index,
            record.stage,
            record.error,
        );
        println!(
            "[hw-report] input_stage index={} stage={:?} error={:?}",
            index, record.stage, record.error,
        );
    }
}

fn emit_nvme(functions: &[PciFunctionInfo]) {
    let Some(function) = functions
        .iter()
        .find(|function| function.class_code & 0x00ff_ffff == 0x01_08_02)
        .copied()
    else {
        serial_println!("[hw-report] nvme status=absent");
        println!("[hw-report] nvme status=absent");
        return;
    };
    match NvmeBlock::init(function) {
        Ok(device) => {
            let identity = device.identity();
            serial_println!(
                "[hw-report] nvme status=ready bdf={}:{:02x}:{:02x}.{} serial={:?} model={:?} firmware={:?} namespaces={} sectors={}",
                function.segment,
                function.bus,
                function.device,
                function.function,
                identity.serial(),
                identity.model(),
                identity.firmware(),
                identity.namespace_count,
                device.capacity_sectors(),
            );
            println!(
                "[hw-report] nvme status=ready bdf={}:{:02x}:{:02x}.{} serial={:?} model={:?} firmware={:?} namespaces={} sectors={}",
                function.segment,
                function.bus,
                function.device,
                function.function,
                identity.serial(),
                identity.model(),
                identity.firmware(),
                identity.namespace_count,
                device.capacity_sectors(),
            );
        }
        Err(error) => {
            serial_println!("[hw-report] nvme status=error error={:?}", error);
            println!("[hw-report] nvme status=error error={:?}", error);
        }
    }
}

pub fn emit(
    platform: &AcpiInfo,
    functions: Result<&[PciFunctionInfo], crate::pci::PciError>,
    input: &InputInitReport,
) -> Result<(), InventoryError> {
    emit_begin();
    serial_println!(
        "[hw-report] generation={:02x?}",
        crate::boot::generation_identity()
    );
    println!(
        "[hw-report] generation={:02x?}",
        crate::boot::generation_identity()
    );
    emit_acpi(platform);
    emit_interrupts(platform);
    emit_framebuffer()?;
    let functions = match functions {
        Ok(functions) => functions,
        Err(error) => {
            serial_println!("[hw-report] pci status=error error={:?}", error);
            println!("[hw-report] pci status=error error={:?}", error);
            emit_input(input);
            emit_end();
            return Err(InventoryError::PciDiscovery);
        }
    };
    emit_pci(functions)?;
    emit_nvme(functions);
    emit_input(input);
    emit_end();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framebuffer() -> Framebuffer {
        Framebuffer {
            address: 0x1000,
            width: 1920,
            height: 1080,
            pitch: 7680,
            bpp: 32,
            memory_model: 1,
            red_mask_size: 8,
            red_mask_shift: 16,
            green_mask_size: 8,
            green_mask_shift: 8,
            blue_mask_size: 8,
            blue_mask_shift: 0,
        }
    }

    #[test_case]
    fn framebuffer_geometry_is_checked() {
        let geometry = framebuffer_geometry(framebuffer()).unwrap();
        assert_eq!(geometry.stride, 1920);
        assert_eq!(geometry.byte_len, 8_294_400);
        let mut invalid = framebuffer();
        invalid.bpp = 0;
        assert_eq!(
            framebuffer_geometry(invalid),
            Err(InventoryError::InvalidFramebuffer)
        );
        invalid = framebuffer();
        invalid.pitch = 1;
        assert_eq!(
            framebuffer_geometry(invalid),
            Err(InventoryError::InvalidFramebuffer)
        );
    }

    #[test_case]
    fn pci_normalization_is_canonical_and_bounded() {
        let second = PciFunctionInfo {
            segment: 0,
            bus: 2,
            device: 0,
            function: 0,
            vendor_id: 2,
            device_id: 2,
            class_code: 2,
        };
        let first = PciFunctionInfo {
            segment: 0,
            bus: 1,
            device: 0,
            function: 0,
            vendor_id: 1,
            device_id: 1,
            class_code: 1,
        };
        assert_eq!(
            normalize_functions(&[second, first]).unwrap(),
            [first, second]
        );
        let oversized = [first; MAX_REPORTED_FUNCTIONS + 1];
        assert_eq!(
            normalize_functions(&oversized),
            Err(InventoryError::TooManyFunctions)
        );
    }
}
