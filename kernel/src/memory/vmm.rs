//! Virtual memory: 4-level x86-64 page-table mapping.
//!
//! Limine leaves us in 4-level paging with the whole of physical RAM mapped at
//! the HHDM offset, so we can read and write any page table by taking its
//! physical frame address through [`PhysAddr::to_virt`]. This module walks the
//! active hierarchy (rooted at CR3), allocating intermediate tables from the
//! PMM as needed, and installs 4 KiB leaf mappings.
//!
//! Design note: mapping errors are *values*, not hangs. [`map_page`] reports a
//! typed [`MapError`] when a mapping already exists or a frame runs out, which
//! is the memory-management half of the milestone's "faults are reported
//! deterministically rather than silently hanging" exit condition.

use super::pmm::FRAME_ALLOCATOR;
use super::{PAGE_SIZE, PhysAddr, VirtAddr};

/// Present: the entry maps something.
pub const PTE_PRESENT: u64 = 1 << 0;
/// Writable.
pub const PTE_WRITABLE: u64 = 1 << 1;
/// User-accessible (ring 3).
pub const PTE_USER: u64 = 1 << 2;
/// Write-through caching.
pub const PTE_WRITE_THROUGH: u64 = 1 << 3;
/// Cache-disable (for MMIO like the Local APIC).
pub const PTE_CACHE_DISABLE: u64 = 1 << 4;
/// No-execute (requires EFER.NXE, which Limine enables).
pub const PTE_NO_EXECUTE: u64 = 1 << 63;

/// Physical-address field mask within a page-table entry (bits 12..=51).
const ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;

/// Why a mapping request failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    /// The virtual page is already mapped.
    AlreadyMapped,
    /// The frame allocator is out of physical frames.
    OutOfFrames,
}

/// A hardware page table: 512 64-bit entries, one 4 KiB frame.
#[repr(C, align(4096))]
struct PageTable {
    entries: [u64; 512],
}

impl PageTable {
    /// Borrow the table living at physical frame `phys`, via the HHDM.
    ///
    /// # Safety
    ///
    /// `phys` must point at a live page-table frame reachable through the HHDM,
    /// and the borrow must not alias another live `&mut` to the same table.
    unsafe fn at(phys: PhysAddr) -> &'static mut PageTable {
        unsafe { &mut *phys.to_virt().as_mut_ptr::<PageTable>() }
    }
}

/// The nine index bits selecting an entry at page-table `level` (1..=4).
fn index(virt: VirtAddr, level: u8) -> usize {
    let shift = 12 + 9 * (level as u64 - 1);
    ((virt.0 >> shift) & 0x1ff) as usize
}

/// Physical address of the active top-level (PML4) table, read from CR3.
fn active_pml4() -> PhysAddr {
    let cr3: u64;
    // SAFETY: reading CR3 is a privileged but side-effect-free ring-0 read.
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
    }
    PhysAddr(cr3 & ADDR_MASK)
}

/// Invalidate the TLB entry for `virt` after changing its mapping.
fn flush(virt: VirtAddr) {
    // SAFETY: `invlpg` only affects the TLB; always valid in ring 0.
    unsafe {
        core::arch::asm!("invlpg [{}]", in(reg) virt.0, options(nostack, preserves_flags));
    }
}

/// Descend into the next-level table an entry points at, allocating and
/// zeroing a fresh table when the entry is absent.
///
/// # Safety
///
/// `table` must be a live table borrowed through the HHDM.
unsafe fn next_table(table: &mut PageTable, i: usize) -> Result<&'static mut PageTable, MapError> {
    let entry = table.entries[i];
    let phys = if entry & PTE_PRESENT == 0 {
        let frame = FRAME_ALLOCATOR
            .lock()
            .alloc()
            .ok_or(MapError::OutOfFrames)?;
        // Zero the new table before linking it in.
        // SAFETY: `frame` is a fresh, exclusively owned frame reached via HHDM.
        unsafe {
            core::ptr::write_bytes(frame.to_virt().as_mut_ptr::<u8>(), 0, PAGE_SIZE);
        }
        // Intermediate entries are permissive (present+writable+user); the leaf
        // entry's flags decide the effective permissions.
        table.entries[i] = frame.0 | PTE_PRESENT | PTE_WRITABLE | PTE_USER;
        frame
    } else {
        PhysAddr(entry & ADDR_MASK)
    };
    // SAFETY: `phys` now names a live, zeroed-or-existing page-table frame.
    Ok(unsafe { PageTable::at(phys) })
}

/// Map 4 KiB virtual page `virt` to physical frame `phys` with `flags`.
///
/// `flags` should carry at least [`PTE_PRESENT`]. Returns [`MapError`] if the
/// page is already mapped or the allocator is exhausted, never overwriting an
/// existing mapping.
///
/// # Safety
///
/// Installing a mapping aliases physical memory into the address space; the
/// caller must ensure `phys` is safe to expose at `virt` with `flags`.
pub unsafe fn map_page(virt: VirtAddr, phys: PhysAddr, flags: u64) -> Result<(), MapError> {
    // SAFETY: CR3 names the live PML4, reachable through the HHDM.
    let pml4 = unsafe { PageTable::at(active_pml4()) };
    // SAFETY: each descent borrows a live table reached through the HHDM.
    let pdpt = unsafe { next_table(pml4, index(virt, 4))? };
    let pd = unsafe { next_table(pdpt, index(virt, 3))? };
    let pt = unsafe { next_table(pd, index(virt, 2))? };

    let i = index(virt, 1);
    if pt.entries[i] & PTE_PRESENT != 0 {
        return Err(MapError::AlreadyMapped);
    }
    pt.entries[i] = (phys.0 & ADDR_MASK) | flags | PTE_PRESENT;
    flush(virt);
    Ok(())
}

/// Translate a virtual address to its physical address, or `None` if unmapped.
pub fn translate(virt: VirtAddr) -> Option<PhysAddr> {
    // SAFETY: CR3 names the live PML4; every descent stops at a present entry.
    let pml4 = unsafe { PageTable::at(active_pml4()) };
    let mut table: &PageTable = pml4;
    for level in (2..=4).rev() {
        let entry = table.entries[index(virt, level)];
        if entry & PTE_PRESENT == 0 {
            return None;
        }
        // SAFETY: present entry points at a live lower-level table via HHDM.
        table = unsafe { PageTable::at(PhysAddr(entry & ADDR_MASK)) };
    }
    let leaf = table.entries[index(virt, 1)];
    if leaf & PTE_PRESENT == 0 {
        return None;
    }
    let page = leaf & ADDR_MASK;
    Some(PhysAddr(page + (virt.0 & 0xfff)))
}
