//! AArch64 boot mechanism for the `aarch64-qemu-virt` profile.
//!
//! Stage-0's verified-generation selection, BootState handling, release
//! authorization, and rollback flow are architecture-neutral and stay in
//! `main.rs` and `lib.rs`. What is profile-specific lives here: stage-1
//! translation tables for the kernel's virtual layout, the MMU enable, and the
//! entry-state setup that transfers to the kernel at EL1.
//!
//! Every fallible step returns a [`BootError`]: stage-0 must never panic, since
//! a panic here bricks the boot path before any rollback machinery exists. The
//! crate denies `unwrap`, `expect`, `panic`, and slice indexing for that reason.
//!
//! # Address layout
//!
//! AArch64 splits translation between two roots. `TTBR0_EL1` covers the low
//! half and `TTBR1_EL1` the high half, selected by the top address bits. The
//! kernel is a higher-half image, so it lives in `TTBR1_EL1`; the identity map
//! stage-0 itself runs on lives in `TTBR0_EL1`. That is the structural
//! difference from x86, where one root holds both halves.

use alloc::vec::Vec;

use boot_contracts::handoff::KernelHandoffV1;
use boot_contracts::kernel_image::{SEGMENT_EXEC, SEGMENT_WRITE};
use slime_stage0::BootError;
use uefi::boot::{MemoryType, PAGE_SIZE};

use crate::{LoadedSegment, allocate_zeroed};

/// Valid descriptor.
const DESC_VALID: u64 = 1 << 0;
/// Table or page descriptor. Clear at a non-leaf level means a block.
const DESC_TABLE: u64 = 1 << 1;
/// Access flag. A leaf without it faults on first touch.
const DESC_AF: u64 = 1 << 10;
/// Shareability: inner-shareable, required for normal cacheable memory.
const DESC_INNER_SHAREABLE: u64 = 0b11 << 8;
/// AP[2]: read-only.
const DESC_READ_ONLY: u64 = 1 << 7;
/// UXN: never executable at EL0.
const DESC_UXN: u64 = 1 << 54;
/// PXN: never executable at EL1.
const DESC_PXN: u64 = 1 << 53;

/// `MAIR_EL1` attribute index 0: normal write-back cacheable memory.
const ATTR_NORMAL: u64 = 0 << 2;
/// `MAIR_EL1` attribute index 1: device-nGnRnE, for MMIO.
const ATTR_DEVICE: u64 = 1 << 2;

/// `MAIR_EL1` attribute byte for normal write-back cacheable memory.
const MAIR_ATTR_NORMAL: u64 = 0xff;
/// `MAIR_EL1` attribute byte for device-nGnRnE memory.
const MAIR_ATTR_DEVICE: u64 = 0x00;

/// `MAIR_EL1`: index 0 normal write-back, index 1 device-nGnRnE.
///
/// This register is the authority for what a descriptor's `AttrIndx` field
/// means: a descriptor names an index, not an attribute, so an inversion
/// silently maps MMIO as cacheable. These tables cover only stage-0's own
/// pre-handoff mappings; once control transfers, the kernel installs its own
/// `MAIR_EL1` and translation tables, so nothing downstream inherits these
/// indices.
const MAIR_VALUE: u64 = MAIR_ATTR_NORMAL | (MAIR_ATTR_DEVICE << 8);

/// Flags for a normal writable kernel data mapping.
const NORMAL_DATA: u64 = DESC_VALID | DESC_TABLE | DESC_AF | DESC_INNER_SHAREABLE | ATTR_NORMAL;
/// Flags for a device MMIO mapping.
const DEVICE_MMIO: u64 =
    DESC_VALID | DESC_TABLE | DESC_AF | ATTR_DEVICE | DESC_UXN | DESC_PXN | DESC_INNER_SHAREABLE;

/// Output-address field of a descriptor (bits 12..=47).
const DESC_ADDR_MASK: u64 = 0x0000_ffff_ffff_f000;

/// Base of the kernel's direct map of physical memory. Sits in the `TTBR1_EL1`
/// half, matching the offset the x86 profile publishes so the handoff contract
/// carries the same meaning on both.
pub const DIRECT_MAP_BASE: u64 = 0xffff_8000_0000_0000;

/// Top (exclusive) of the kernel boot stack's dedicated virtual window.
///
/// Mapped with 4 KiB pages into an otherwise-unused high slot, with the page
/// directly below its base left unmapped as a guard, so an overflow past the
/// base faults deterministically instead of corrupting whatever physical RAM
/// sits below the stack.
const KERNEL_STACK_TOP_VA: u64 = 0xffff_ff00_0010_0000;

/// Base of RAM on the `virt` machine. Everything below is memory-mapped I/O —
/// the PL011 UART, the GIC, and the virtio-mmio transports — and must be mapped
/// with device attributes rather than as cacheable normal memory. Part of the
/// pinned machine profile, like the UART base itself; P2.5 discovers it from
/// the device tree.
const RAM_PHYS_BASE: u64 = 0x4000_0000;

/// Index bits consumed per translation level.
const LEVEL_INDEX_BITS: u64 = 9;
/// Bit offset of the level-3 (4 KiB leaf) index.
const LEAF_SHIFT: u64 = 12;
/// Descriptors per table at a 4 KiB granule.
const ENTRIES_PER_TABLE: usize = 512;
/// Bytes a level-2 block descriptor maps.
const BLOCK_BYTES: u64 = 2 << 20;

/// Smallest data cache line size this CPU reports, in bytes.
///
/// Read from `CTR_EL0.DminLine` rather than assumed: the field exists precisely
/// because the value is implementation-defined, and stepping by more than the
/// true line size skips lines, leaving stale page-table bytes behind when the
/// cache is disabled. `aarch64-qemu-virt` and `aarch64-rpi5` are different CPUs,
/// so a constant that happens to hold for one is not evidence for the other.
fn cache_line_bytes() -> u64 {
    let ctr: u64;
    // SAFETY: `CTR_EL0` is a side-effect-free identification register read.
    unsafe {
        core::arch::asm!("mrs {}, ctr_el0", out(reg) ctr,
             options(nomem, nostack, preserves_flags));
    }
    // DminLine (bits 16..20) is log2 of the line size in 32-bit words. The
    // architecture bounds the field to 4 bits, so the product is at least 4 and
    // the clean loop below always advances — a zero stride would spin forever
    // with the MMU half-configured and no diagnostic.
    let words = 1u64 << ((ctr >> 16) & 0xf);
    words * 4
}

/// One page-table's worth of descriptors.
#[repr(align(4096))]
struct Page([u64; ENTRIES_PER_TABLE]);

/// `TCR_EL1`: 48-bit address space in both halves, 4 KiB granule, inner/outer
/// write-back cacheable table walks, inner-shareable.
const TCR_VALUE: u64 = {
    // T0SZ/T1SZ = 16 gives a 48-bit address space per half.
    let t0sz = 16;
    let t1sz = 16 << 16;
    // IRGN/ORGN = write-back write-allocate, SH = inner-shareable, for both.
    let ttbr0_cacheable = (0b01 << 8) | (0b01 << 10) | (0b11 << 12);
    let ttbr1_cacheable = (0b01 << 24) | (0b01 << 26) | (0b11 << 28);
    // TG0 = 4 KiB (0b00), TG1 = 4 KiB (0b10). The encodings differ per half.
    let granules = 0b10 << 30;
    // IPS = 40-bit intermediate physical address, ample for this machine.
    let ips = 0b010 << 32;
    t0sz | t1sz | ttbr0_cacheable | ttbr1_cacheable | granules | ips
};

/// `SCTLR_EL1` bits: MMU enable, data cache, instruction cache.
const SCTLR_MMU: u64 = 1 << 0;
const SCTLR_DCACHE: u64 = 1 << 2;
const SCTLR_ICACHE: u64 = 1 << 12;

/// The kernel's stage-1 translation tables, built by stage-0.
///
/// Holds both roots: `low` becomes `TTBR0_EL1` (the identity map stage-0 runs
/// on and the kernel's user half), `high` becomes `TTBR1_EL1` (the kernel image
/// and direct map).
pub struct PageTables {
    low: u64,
    high: u64,
    pages: Vec<u64>,
}

impl PageTables {
    pub fn new() -> Result<Self, BootError> {
        let low = allocate_zeroed(PAGE_SIZE, MemoryType::LOADER_DATA)? as u64;
        let high = allocate_zeroed(PAGE_SIZE, MemoryType::LOADER_DATA)? as u64;
        Ok(Self {
            low,
            high,
            pages: alloc::vec![low, high],
        })
    }

    /// The index selecting a descriptor at `level` (0..=3, 0 being the root).
    fn index(virtual_address: u64, level: u64) -> usize {
        let shift = LEAF_SHIFT + LEVEL_INDEX_BITS * (3 - level);
        ((virtual_address >> shift) & 0x1ff) as usize
    }

    /// Descend into the next-level table a descriptor points at, allocating a
    /// zeroed table when the descriptor is absent.
    fn table(&mut self, parent: u64, index: usize) -> Result<u64, BootError> {
        // SAFETY: `parent` is a table this builder allocated and identity-mapped.
        let entries = unsafe { &mut *(parent as *mut Page) };
        let entry = *entries.0.get(index).ok_or(BootError::PageTableExhausted)?;
        if entry & DESC_VALID != 0 {
            // A block descriptor already maps this whole region, so there is no
            // child table to descend into.
            if entry & DESC_TABLE == 0 {
                return Err(BootError::PageTableExhausted);
            }
            return Ok(entry & DESC_ADDR_MASK);
        }
        let child = allocate_zeroed(PAGE_SIZE, MemoryType::LOADER_DATA)? as u64;
        self.pages.push(child);
        *entries
            .0
            .get_mut(index)
            .ok_or(BootError::PageTableExhausted)? = child | DESC_VALID | DESC_TABLE;
        Ok(child)
    }

    /// Whether `virtual_address` belongs to the high (`TTBR1_EL1`) half.
    fn is_high_half(virtual_address: u64) -> bool {
        virtual_address >= 0xffff_0000_0000_0000
    }

    /// The root covering `virtual_address`.
    fn root_for(&self, virtual_address: u64) -> u64 {
        if Self::is_high_half(virtual_address) {
            self.high
        } else {
            self.low
        }
    }

    fn map_4k(
        &mut self,
        virtual_address: u64,
        physical_address: u64,
        flags: u64,
    ) -> Result<(), BootError> {
        let root = self.root_for(virtual_address);
        let level1 = self.table(root, Self::index(virtual_address, 0))?;
        let level2 = self.table(level1, Self::index(virtual_address, 1))?;
        let level3 = self.table(level2, Self::index(virtual_address, 2))?;
        // SAFETY: `level3` is a table this builder allocated and identity-mapped.
        let entries = unsafe { &mut *(level3 as *mut Page) };
        let index = Self::index(virtual_address, 3);
        *entries
            .0
            .get_mut(index)
            .ok_or(BootError::PageTableExhausted)? = (physical_address & DESC_ADDR_MASK) | flags;
        Ok(())
    }

    /// Install one 2 MiB block descriptor at level 2.
    fn map_block(
        &mut self,
        virtual_address: u64,
        physical_address: u64,
        flags: u64,
    ) -> Result<(), BootError> {
        let root = self.root_for(virtual_address);
        let level1 = self.table(root, Self::index(virtual_address, 0))?;
        let level2 = self.table(level1, Self::index(virtual_address, 1))?;
        // SAFETY: `level2` is a table this builder allocated and identity-mapped.
        let entries = unsafe { &mut *(level2 as *mut Page) };
        let index = Self::index(virtual_address, 2);
        // A block descriptor is the leaf flags with the table bit cleared.
        *entries
            .0
            .get_mut(index)
            .ok_or(BootError::PageTableExhausted)? =
            (physical_address & DESC_ADDR_MASK) | (flags & !DESC_TABLE);
        Ok(())
    }

    /// Map the first `bytes` of physical address space twice: identically in
    /// the low half (so stage-0 keeps running after the MMU is enabled) and at
    /// [`DIRECT_MAP_BASE`] in the high half (the kernel's direct map).
    ///
    /// Blocks below [`RAM_PHYS_BASE`] carry device attributes: that window is
    /// MMIO on this machine, and mapping the PL011 or GIC as cacheable normal
    /// memory would let the CPU cache, reorder, or speculatively read device
    /// registers. Nothing in either window is executable.
    pub fn map_identity(&mut self, bytes: u64) -> Result<(), BootError> {
        let blocks = bytes.div_ceil(BLOCK_BYTES);
        for block in 0..blocks {
            let physical = block
                .checked_mul(BLOCK_BYTES)
                .ok_or(BootError::AddressOverflow)?;
            let device = physical < RAM_PHYS_BASE;
            // The identity window must stay executable at EL1: stage-0 is
            // running from it, and the instruction that re-enables the MMU
            // fetches its successor through these very descriptors. Marking it
            // PXN kills the CPU at that instruction with no diagnostic.
            let identity = if device {
                DEVICE_MMIO
            } else {
                NORMAL_DATA | DESC_UXN
            };
            // The direct map is data only — the kernel executes from its own
            // image mapping, never from here — so it is never executable.
            let direct_flags = if device {
                DEVICE_MMIO
            } else {
                NORMAL_DATA | DESC_UXN | DESC_PXN
            };
            self.map_block(physical, physical, identity)?;
            let direct = DIRECT_MAP_BASE
                .checked_add(physical)
                .ok_or(BootError::AddressOverflow)?;
            self.map_block(direct, physical, direct_flags)?;
        }
        Ok(())
    }

    pub fn map_segments(&mut self, segments: &[LoadedSegment]) -> Result<(), BootError> {
        for segment in segments {
            let writable = segment.flags & SEGMENT_WRITE != 0;
            let executable = segment.flags & SEGMENT_EXEC != 0;
            // AArch64 expresses read-only by *setting* AP[2], the inverse of
            // x86's set-to-permit write bit.
            let mut flags = NORMAL_DATA | DESC_UXN;
            if !writable {
                flags |= DESC_READ_ONLY;
            }
            if !executable {
                flags |= DESC_PXN;
            }
            for page in 0..segment.page_count as u64 {
                let offset = page
                    .checked_mul(PAGE_SIZE as u64)
                    .ok_or(BootError::AddressOverflow)?;
                self.map_4k(
                    segment
                        .virtual_address
                        .checked_add(offset)
                        .ok_or(BootError::AddressOverflow)?,
                    segment
                        .physical_address
                        .checked_add(offset)
                        .ok_or(BootError::AddressOverflow)?,
                    flags,
                )?;
            }
        }
        Ok(())
    }

    /// Map `bytes` of kernel boot stack, whose frames start at physical
    /// `stack_phys`, into the dedicated virtual window ending at
    /// [`KERNEL_STACK_TOP_VA`], leaving one unmapped guard page below the base.
    /// Returns the top-of-stack virtual address to load into `SP`.
    pub fn map_stack(&mut self, stack_phys: u64, bytes: usize) -> Result<u64, BootError> {
        let page = PAGE_SIZE as u64;
        let bytes = bytes as u64;
        let base_va = KERNEL_STACK_TOP_VA
            .checked_sub(bytes)
            .ok_or(BootError::AddressOverflow)?;
        // The guard page sits at base_va - PAGE_SIZE and is intentionally never
        // mapped, so a downward overflow past the stack base faults.
        let pages = bytes / page;
        for index in 0..pages {
            let offset = index.checked_mul(page).ok_or(BootError::AddressOverflow)?;
            self.map_4k(
                base_va
                    .checked_add(offset)
                    .ok_or(BootError::AddressOverflow)?,
                stack_phys
                    .checked_add(offset)
                    .ok_or(BootError::AddressOverflow)?,
                NORMAL_DATA | DESC_UXN | DESC_PXN,
            )?;
        }
        Ok(KERNEL_STACK_TOP_VA)
    }

    /// Install both roots and enable the MMU and caches.
    ///
    /// UEFI on AArch64 hands off with the MMU *already enabled* under its own
    /// translation tables — unlike x86 UEFI, where stage-0 simply replaces CR3.
    /// `TCR_EL1`, `TTBR*_EL1`, and `MAIR_EL1` cannot be reconfigured while
    /// translation is live, so this disables the MMU and caches first, switches
    /// the configuration while running on flat physical addresses, and turns it
    /// back on. Stage-0's own code and stack must be identity-mapped in the new
    /// tables for the re-enable to be survivable, which is what
    /// [`Self::map_identity`] guarantees.
    ///
    /// # Safety
    ///
    /// The tables must map every address the caller executes from and touches
    /// after this returns, including stage-0's own code and stack.
    pub unsafe fn activate(&self) {
        // Clean every table to the point of coherency before translation is
        // disabled. The tables were written through a cacheable mapping, so
        // those lines are still dirty in the data cache; once the cache is off,
        // the table walker reads memory directly and would see stale bytes.
        // `self.pages` is exactly the set of frames that need it.
        let stride = cache_line_bytes();
        for page in &self.pages {
            let mut line = *page;
            let end = page.wrapping_add(PAGE_SIZE as u64);
            while line < end {
                // SAFETY: `dc cvac` only writes back a cache line for an
                // address this builder owns; it cannot fault on a mapped page.
                unsafe {
                    core::arch::asm!("dc cvac, {line}", line = in(reg) line,
                         options(nostack, preserves_flags));
                }
                line = line.wrapping_add(stride);
            }
        }
        // SAFETY: the clean above is complete before the barrier.
        unsafe {
            core::arch::asm!("dsb sy", options(nostack, preserves_flags));
        }
        unsafe {
            core::arch::asm!(
                // Disable MMU, data cache, and instruction cache. From here to
                // the re-enable the CPU runs on flat physical addresses, so
                // every address in play must already be an identity mapping.
                "mrs {tmp}, sctlr_el1",
                "bic {tmp}, {tmp}, {disable}",
                "msr sctlr_el1, {tmp}",
                "isb",
                // Clean and invalidate: with the data cache off, stale lines
                // from the previous configuration would otherwise shadow the
                // table writes below.
                "dsb sy",
                "tlbi vmalle1",
                "ic iallu",
                "dsb sy",
                "isb",
                // Install the new configuration while translation is off.
                "msr mair_el1, {mair}",
                "msr tcr_el1, {tcr}",
                "msr ttbr0_el1, {low}",
                "msr ttbr1_el1, {high}",
                "dsb ish",
                "tlbi vmalle1",
                "dsb ish",
                "isb",
                // Re-enable. The next instruction fetch is translated by the
                // tables just installed.
                "mrs {tmp}, sctlr_el1",
                "orr {tmp}, {tmp}, {enable}",
                "msr sctlr_el1, {tmp}",
                "isb",
                mair = in(reg) MAIR_VALUE,
                tcr = in(reg) TCR_VALUE,
                low = in(reg) self.low,
                high = in(reg) self.high,
                disable = in(reg) SCTLR_MMU | SCTLR_DCACHE | SCTLR_ICACHE,
                enable = in(reg) SCTLR_MMU | SCTLR_DCACHE | SCTLR_ICACHE,
                tmp = out(reg) _,
                options(nostack),
            );
        }
    }
}

/// Confirm the CPU supports the profile's required translation configuration.
///
/// Checks the 4 KiB granule and a physical address range large enough for the
/// machine. An unsupported configuration returns a structured error rather than
/// producing tables the MMU will refuse.
pub fn check_translation_support() -> Result<(), BootError> {
    let mmfr0: u64;
    // SAFETY: reading a feature-identification register is side-effect free.
    unsafe {
        core::arch::asm!("mrs {}, id_aa64mmfr0_el1", out(reg) mmfr0,
             options(nomem, nostack, preserves_flags));
    }
    // TGran4 (bits 28..32): 0b0000 means the 4 KiB granule is supported.
    if (mmfr0 >> 28) & 0xf != 0b0000 {
        return Err(BootError::UnsupportedTranslation);
    }
    // PARange (bits 0..4): 0b0010 is 40-bit, which TCR_EL1.IPS above requests.
    if mmfr0 & 0xf < 0b0010 {
        return Err(BootError::UnsupportedTranslation);
    }
    Ok(())
}

/// Establish the execute-permission baseline.
///
/// x86 must enable `EFER.NXE` before a no-execute page-table bit has any
/// effect. AArch64 has no such global enable — `UXN`/`PXN` are always live — so
/// this only verifies the translation configuration the profile requires.
pub fn enable_nxe() -> Result<(), BootError> {
    check_translation_support()
}

/// Transfer to the kernel at EL1.
///
/// Installs the kernel's boot stack, passes the handoff pointer in `x0` per the
/// AAPCS64 first-argument register, and branches. Does not return.
///
/// # Safety
///
/// `entry` must be the kernel's mapped entry address, `handoff` a live
/// `KernelHandoffV1` reachable through the direct map, and `stack_top` a mapped
/// stack whose guard page is below it.
pub unsafe fn jump(entry: u64, handoff: *const KernelHandoffV1, stack_top: u64) -> ! {
    unsafe {
        core::arch::asm!(
            "mov sp, {stack}",
            "br {entry}",
            stack = in(reg) stack_top,
            entry = in(reg) entry,
            in("x0") handoff,
            options(noreturn),
        )
    }
}
