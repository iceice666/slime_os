//! x86-64 page-table mechanism: 4-level 4 KiB translation.
//!
//! The neutral virtual-memory layer ([`crate::memory::vmm`]) walks and edits
//! tables through the constants and helpers here, so the table shape, entry
//! encoding, root register, and TLB maintenance instruction stay inside the
//! architecture boundary. Neutral callers name mapping *intent*
//! ([`PTE_WRITABLE`], [`PTE_DEVICE`]) rather than an x86 bit position.

use crate::memory::{PhysAddr, VirtAddr};

/// Translation levels in the hierarchy; level 1 holds the 4 KiB leaves.
pub const PAGE_TABLE_LEVELS: u8 = 4;

/// Entries per table. Each level consumes 9 index bits.
pub const ENTRIES_PER_TABLE: usize = 512;

/// Index bits consumed per level.
const LEVEL_INDEX_BITS: u64 = 9;

/// Bit offset of the level-1 (4 KiB leaf) index.
const LEAF_SHIFT: u64 = 12;

/// Present: the entry maps something.
pub const PTE_PRESENT: u64 = 1 << 0;
/// Writable.
pub const PTE_WRITABLE: u64 = 1 << 1;
/// User-accessible (ring 3).
pub const PTE_USER: u64 = 1 << 2;
/// Write-through caching.
pub const PTE_WRITE_THROUGH: u64 = 1 << 3;
/// Cache-disable, for device MMIO.
pub const PTE_CACHE_DISABLE: u64 = 1 << 4;
/// No-execute (requires EFER.NXE, which the boot path enables).
pub const PTE_NO_EXECUTE: u64 = 1 << 63;

/// Flags for a device MMIO mapping: writable, uncached, never executable.
pub const PTE_DEVICE: u64 = PTE_WRITABLE | PTE_CACHE_DISABLE | PTE_NO_EXECUTE;

/// Block/huge mapping bit. Set at an intermediate level, the entry maps its
/// whole region directly and has no child table.
const PTE_HUGE: u64 = 1 << 7;

/// Whether an intermediate entry maps its whole region directly, so no child
/// table exists to descend into.
///
/// This is a predicate rather than an exported bit because the encoding's
/// *polarity* is architecture-specific: x86 sets a bit to mean "block", while
/// AArch64 clears one. A shared bitmask cannot express both, so neutral code
/// asks the question instead of testing a bit.
pub fn is_block(entry: u64) -> bool {
    entry & PTE_HUGE != 0
}

/// Whether a leaf entry permits writes.
pub fn is_writable(entry: u64) -> bool {
    entry & PTE_WRITABLE != 0
}

/// Downgrade a leaf entry to read-only, returning the new value.
pub fn make_read_only(entry: u64) -> u64 {
    entry & !PTE_WRITABLE
}

/// Physical-address field mask within an entry (bits 12..=51).
pub const PTE_ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;

/// Permissive flags for an intermediate (non-leaf) entry. Effective
/// permissions are decided by the leaf, so intermediates stay open.
pub const PTE_INTERMEDIATE: u64 = PTE_PRESENT | PTE_WRITABLE | PTE_USER;

/// Bits every valid leaf must carry, whatever permissions the caller asked for.
///
/// On x86 that is just the present bit. AArch64 additionally needs a page-type
/// bit and an access flag, so neutral code must not assume its intent flags are
/// a complete descriptor — it ORs this in for every leaf it installs.
pub const PTE_LEAF: u64 = PTE_PRESENT;

/// The index selecting an entry at `level` (1..=[`PAGE_TABLE_LEVELS`]).
pub fn table_index(virt: VirtAddr, level: u8) -> usize {
    let shift = LEAF_SHIFT + LEVEL_INDEX_BITS * (level as u64 - 1);
    ((virt.as_u64() >> shift) & 0x1ff) as usize
}

/// The translation root covering kernel-half addresses.
///
/// x86 has one root for the whole address space, so this is the active root.
/// AArch64 splits the halves across two registers, which is why the boundary
/// exposes this separately from [`active_root`] rather than letting neutral
/// code assume one root serves every address.
pub fn kernel_root() -> PhysAddr {
    active_root()
}

/// Physical frame the root register currently points at.
pub fn active_root() -> PhysAddr {
    let cr3: u64;
    // SAFETY: reading CR3 is a privileged but side-effect-free ring-0 read.
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
    }
    PhysAddr(cr3 & PTE_ADDR_MASK)
}

/// Install `root` as the active translation root.
///
/// # Safety
///
/// `root` must name a live top-level table whose kernel half maps the running
/// kernel; otherwise the next instruction fetch faults.
pub unsafe fn set_active_root(root: PhysAddr) {
    unsafe {
        core::arch::asm!("mov cr3, {}", in(reg) root.as_u64(), options(nostack, preserves_flags));
    }
}

/// Invalidate the TLB entry for `virt` after changing its mapping.
pub fn flush_tlb(virt: VirtAddr) {
    // SAFETY: `invlpg` only affects the TLB; always valid in ring 0.
    unsafe {
        core::arch::asm!("invlpg [{}]", in(reg) virt.as_u64(), options(nostack, preserves_flags));
    }
}
