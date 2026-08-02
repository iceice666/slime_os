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
//! **Unvalidated.** These encodings are written from the architecture reference
//! and have never been loaded by hardware. P2 implements the register access and
//! TLB maintenance instructions and must check them against a running EL1 before
//! any mapping claim.

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
/// Writability is expressed by a *clear* AP[2], the inverse of x86's set-to-
/// permit bit, so it contributes nothing to an OR-built flag word. Neutral code
/// never reads this: it goes through [`is_writable`] and [`make_read_only`],
/// which is why those are predicates on the boundary rather than a shared bit.
///
/// The value a writable leaf does carry is the page-descriptor and access-flag
/// bits, without which the entry is invalid or faults on first touch.
pub const PTE_WRITABLE: u64 = PTE_PAGE | PTE_ACCESS_FLAG;
/// AP[2]: read-only. Set to downgrade a writable mapping.
pub const PTE_READ_ONLY: u64 = 1 << 7;
/// AP[1] (bit 6): accessible from EL0.
pub const PTE_USER: u64 = 1 << 6;
/// Attribute index 1: normal write-through memory.
pub const PTE_WRITE_THROUGH: u64 = 1 << 2;
/// Attribute index 0: device-nGnRnE memory, for MMIO.
pub const PTE_CACHE_DISABLE: u64 = 0;
/// UXN (bit 54): never executable at EL0.
pub const PTE_NO_EXECUTE: u64 = 1 << 54;
/// PXN (bit 53): never executable at EL1.
pub const PTE_NO_EXECUTE_PRIVILEGED: u64 = 1 << 53;
/// AF (bit 10): access flag. A leaf without it faults on first touch.
pub const PTE_ACCESS_FLAG: u64 = 1 << 10;

/// Flags for a device MMIO mapping: uncached, never executable at either level.
pub const PTE_DEVICE: u64 =
    PTE_PAGE | PTE_ACCESS_FLAG | PTE_CACHE_DISABLE | PTE_NO_EXECUTE | PTE_NO_EXECUTE_PRIVILEGED;

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

/// The index selecting a descriptor at `level` (1..=[`PAGE_TABLE_LEVELS`]).
pub fn table_index(virt: VirtAddr, level: u8) -> usize {
    let shift = LEAF_SHIFT + LEVEL_INDEX_BITS * (level as u64 - 1);
    ((virt.as_u64() >> shift) & 0x1ff) as usize
}

/// Physical frame the translation-table base register points at.
pub fn active_root() -> PhysAddr {
    unimplemented!("aarch64 TTBR read: implemented by P2")
}

/// Install `root` as the active translation root.
///
/// # Safety
///
/// `root` must name a live top-level table whose kernel half maps the running
/// kernel.
pub unsafe fn set_active_root(_root: PhysAddr) {
    unimplemented!("aarch64 TTBR write: implemented by P2")
}

/// Invalidate the TLB entry for `virt` after changing its mapping.
pub fn flush_tlb(_virt: VirtAddr) {
    unimplemented!("aarch64 TLBI: implemented by P2")
}
