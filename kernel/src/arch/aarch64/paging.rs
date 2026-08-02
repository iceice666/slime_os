//! AArch64 page-table mechanism: 4-level 4 KiB translation granule.
//!
//! The constants below are the real AArch64 stage-1 descriptor encodings rather
//! than reused x86 bit positions, so the neutral mapper's arithmetic is
//! exercised for this target. Two of them have the *opposite polarity* to their
//! x86 counterparts — a table is marked by a set bit, and writability by a
//! clear one — which is why the boundary exports [`is_block`], [`is_writable`],
//! and [`make_read_only`] as predicates instead of shared bitmasks. A bitmask
//! would invert both tests silently.
//!
//! **Partially validated.** P2.1 loaded these on a running EL1: the register
//! access, TLB maintenance, table walk, and the descriptor bits every kernel
//! mapping uses are exercised by `just aarch64_boot_check`, which brings up the
//! heap through this module.
//!
//! Not yet exercised: [`PTE_USER`] and [`PTE_READ_ONLY`] (no EL0 mapping exists
//! until P2.3) and [`PTE_DEVICE`] (no device is mapped through the kernel until
//! P2.5 — the UART is reached through stage-0's own mapping). The `AttrIndx`
//! values those select must agree with the `MAIR_EL1` stage-0 programs, which
//! is a correspondence no boot-path test can currently catch.

use crate::memory::{PhysAddr, VirtAddr};

/// Translation levels for a 4 KiB granule with a 48-bit address space.
pub const PAGE_TABLE_LEVELS: u8 = 4;

/// Descriptors per table at a 4 KiB granule.
pub const ENTRIES_PER_TABLE: usize = 512;

const LEVEL_INDEX_BITS: u64 = 9;
const LEAF_SHIFT: u64 = 12;

/// Bit 0: the descriptor is valid. Named `PTE_PRESENT` to match the neutral
/// mapper's vocabulary.
pub const PTE_PRESENT: u64 = 1 << 0;
/// Bit 1 at level 3 marks a page descriptor (as opposed to a block).
const PTE_PAGE: u64 = 1 << 1;
/// Writability is expressed by a *clear* AP[2], the inverse of x86's
/// set-to-permit bit, so it contributes nothing to an OR-built flag word.
/// Neutral code never reads this value: it goes through [`is_writable`] and
/// [`make_read_only`], which is why those are predicates on the boundary rather
/// than a shared bit. The structural bits a valid leaf needs come from
/// [`PTE_LEAF`], not from here.
pub const PTE_WRITABLE: u64 = 0;
/// AP[2]: read-only. Set to downgrade a writable mapping.
pub const PTE_READ_ONLY: u64 = 1 << 7;
/// AP[1] (bit 6): accessible from EL0.
pub const PTE_USER: u64 = 1 << 6;
/// `MAIR_EL1` attribute index 0: normal write-back cacheable memory.
///
/// The indices below must match the `MAIR_EL1` stage-0 programs
/// (`stage0/src/arch/aarch64.rs`), which is the authority: a descriptor selects
/// an *index*, and the meaning of that index lives entirely in the register.
/// Getting the two out of step silently maps MMIO as cacheable, which no test
/// on the boot path can catch.
const ATTR_INDEX_NORMAL: u64 = 0 << 2;
/// `MAIR_EL1` attribute index 1: device-nGnRnE memory, for MMIO.
const ATTR_INDEX_DEVICE: u64 = 1 << 2;

/// Write-through caching. AArch64's MAIR has no write-through entry in this
/// profile's two-attribute table, so this resolves to ordinary cacheable normal
/// memory — the same attribute a plain kernel data mapping gets.
pub const PTE_WRITE_THROUGH: u64 = ATTR_INDEX_NORMAL;
/// Uncached device memory, for MMIO.
pub const PTE_CACHE_DISABLE: u64 = ATTR_INDEX_DEVICE;
/// UXN (bit 54): never executable at EL0.
pub const PTE_NO_EXECUTE: u64 = 1 << 54;
/// PXN (bit 53): never executable at EL1.
pub const PTE_NO_EXECUTE_PRIVILEGED: u64 = 1 << 53;
/// AF (bit 10): access flag. A leaf without it faults on first touch.
pub const PTE_ACCESS_FLAG: u64 = 1 << 10;

/// Flags for a device MMIO mapping: uncached, never executable at either level.
/// The structural leaf bits are added by the walker through [`PTE_LEAF`].
pub const PTE_DEVICE: u64 = PTE_CACHE_DISABLE | PTE_NO_EXECUTE | PTE_NO_EXECUTE_PRIVILEGED;

/// Whether an intermediate descriptor maps its whole region directly, so no
/// child table exists to descend into.
///
/// Inverted relative to x86: AArch64 marks a *table* by setting bit 1, so a
/// block is that bit **clear**. A shared bitmask would silently invert the test
/// — refusing every valid table and descending into every block — which is why
/// the boundary exports predicates here rather than a `PTE_HUGE` constant.
pub fn is_block(entry: u64) -> bool {
    entry & PTE_PAGE == 0
}

/// Whether a leaf descriptor permits writes: AP[2] clear.
pub fn is_writable(entry: u64) -> bool {
    entry & PTE_READ_ONLY == 0
}

/// Downgrade a leaf descriptor to read-only by setting AP[2].
pub fn make_read_only(entry: u64) -> u64 {
    entry | PTE_READ_ONLY
}

/// Output-address field of a descriptor (bits 12..=47).
pub const PTE_ADDR_MASK: u64 = 0x0000_ffff_ffff_f000;

/// Flags for an intermediate table descriptor: valid + table type.
pub const PTE_INTERMEDIATE: u64 = PTE_PRESENT | PTE_PAGE;

/// Bits every valid leaf must carry, whatever permissions the caller asked for.
///
/// A level-3 descriptor without [`PTE_PAGE`] is a (reserved) block encoding,
/// and one without [`PTE_ACCESS_FLAG`] faults on first touch. Neither is
/// something a caller expressing *permissions* should have to know, so the
/// walker ORs this into every leaf. x86 needs only its present bit here, which
/// is why the flags a caller passes are a complete descriptor there and not
/// here.
pub const PTE_LEAF: u64 = PTE_PRESENT | PTE_PAGE | PTE_ACCESS_FLAG;

/// The index selecting a descriptor at `level` (1..=[`PAGE_TABLE_LEVELS`]).
pub fn table_index(virt: VirtAddr, level: u8) -> usize {
    let shift = LEAF_SHIFT + LEVEL_INDEX_BITS * (level as u64 - 1);
    ((virt.as_u64() >> shift) & 0x1ff) as usize
}

/// The translation root covering kernel-half addresses: `TTBR1_EL1`.
///
/// AArch64 splits translation between two roots — `TTBR0_EL1` for the low half
/// and `TTBR1_EL1` for the high half — selected by the address itself, so a
/// kernel mapping and a user mapping do not share a root the way they do on
/// x86. Neutral code that maps into the kernel half must start its walk here.
pub fn kernel_root() -> PhysAddr {
    let ttbr: u64;
    // SAFETY: reading TTBR1_EL1 is a privileged but side-effect-free read.
    unsafe {
        core::arch::asm!("mrs {}, ttbr1_el1", out(reg) ttbr,
             options(nomem, nostack, preserves_flags));
    }
    PhysAddr(ttbr & PTE_ADDR_MASK)
}

/// Physical frame the translation-table base register points at.
///
/// Reads `TTBR0_EL1`, the user-half root. The kernel half lives in
/// `TTBR1_EL1`, which is installed once at entry and never switched — the
/// split is what makes an AArch64 address-space switch cheaper than x86's,
/// where both halves share one root.
pub fn active_root() -> PhysAddr {
    let ttbr: u64;
    // SAFETY: reading TTBR0_EL1 is a privileged but side-effect-free read.
    unsafe {
        core::arch::asm!("mrs {}, ttbr0_el1", out(reg) ttbr,
             options(nomem, nostack, preserves_flags));
    }
    PhysAddr(ttbr & PTE_ADDR_MASK)
}

/// Install `root` as the active translation root.
///
/// # Safety
///
/// `root` must name a live top-level table whose kernel half maps the running
/// kernel; otherwise the next instruction fetch faults.
pub unsafe fn set_active_root(root: PhysAddr) {
    unsafe {
        core::arch::asm!(
            "msr ttbr0_el1, {root}",
            // The barriers are required, not defensive: without them the switch
            // is not architecturally guaranteed to be visible to the next
            // translation.
            "dsb ish",
            "isb",
            root = in(reg) root.as_u64(),
            options(nostack, preserves_flags),
        );
    }
}

/// Invalidate the TLB entry for `virt` after changing its mapping.
pub fn flush_tlb(virt: VirtAddr) {
    // TLBI takes a virtual address shifted right by 12, not the address itself.
    let page = virt.as_u64() >> LEAF_SHIFT;
    // SAFETY: TLB maintenance only discards cached translations. The leading
    // `dsb ishst` orders the page-table write that preceded this against the
    // invalidate; the trailing pair makes the invalidate visible before the
    // next translation.
    unsafe {
        core::arch::asm!(
            "dsb ishst",
            "tlbi vaae1is, {page}",
            "dsb ish",
            "isb",
            page = in(reg) page,
            options(nostack, preserves_flags),
        );
    }
}
