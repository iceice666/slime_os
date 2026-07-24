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

/// Page-size bit (bit 7). At the PDPT/PD levels a set bit means the entry maps a
/// huge page directly rather than pointing at a lower-level table.
const PTE_HUGE: u64 = 1 << 7;

/// Physical-address field mask within a page-table entry (bits 12..=51).
const ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;

/// Follow an intermediate (PML4/PDPT/PD) entry to the physical frame of its
/// child table, or `None` if the entry cannot be safely descended.
///
/// Rejects entries that are absent, missing any bit in `required` (e.g.
/// [`PTE_USER`]), map a huge page (no 4 KiB child table exists), or point
/// outside physical RAM. The last case makes a corrupted table produce a typed
/// `None` here instead of a wild HHDM dereference that faults deep inside the
/// walker and misattributes the failure.
fn child_table(entry: u64, required: u64) -> Option<PhysAddr> {
    if entry & (PTE_PRESENT | required) != PTE_PRESENT | required || entry & PTE_HUGE != 0 {
        return None;
    }
    let phys = PhysAddr(entry & ADDR_MASK);
    let max = crate::memory::max_phys_addr();
    // `max == 0` means the bound is not yet known (pre-`memory::init`); accept.
    if max != 0 && phys.0 >= max {
        return None;
    }
    Some(phys)
}

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
pub(crate) fn active_pml4() -> PhysAddr {
    let cr3: u64;
    // SAFETY: reading CR3 is a privileged but side-effect-free ring-0 read.
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
    }
    PhysAddr(cr3 & ADDR_MASK)
}
pub(crate) fn copy_kernel_half(source: PhysAddr, destination: PhysAddr) {
    unsafe {
        let src = source.to_virt().as_mut_ptr::<u64>();
        let dst = destination.to_virt().as_mut_ptr::<u64>();
        core::ptr::copy_nonoverlapping(src.add(256), dst.add(256), 256);
    }
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
    // A huge-page entry at this level already maps the whole region, so there is
    // no lower table to descend into and no 4 KiB leaf can be installed here.
    if entry & PTE_PRESENT != 0 && entry & PTE_HUGE != 0 {
        return Err(MapError::AlreadyMapped);
    }
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
/// Must not be called while holding the scheduler lock: successful mappings
/// propagate the kernel half to all task address spaces under that lock.
///
/// # Safety
///
/// Installing a mapping aliases physical memory into the address space; the
/// caller must ensure `phys` is safe to expose at `virt` with `flags`.
pub(crate) unsafe fn map_page_in(
    root: PhysAddr,
    virt: VirtAddr,
    phys: PhysAddr,
    flags: u64,
) -> Result<(), MapError> {
    // SAFETY: `root` names a live PML4, reachable through the HHDM.
    let pml4 = unsafe { PageTable::at(root) };
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
    crate::task::synchronize_kernel_mappings(root);
    Ok(())
}

/// Remap 4 KiB virtual page `virt` to a new physical frame `phys`, overwriting
/// any existing leaf mapping. Used only for the single PCI ECAM scratch page,
/// which is reused across functions during single-threaded boot enumeration.
///
/// # Safety
///
/// The caller must ensure the old mapping (if any) is safe to invalidate and
/// the new `phys` is safe to expose at `virt` with `flags`.
///
/// Must not be called while holding the scheduler lock: successful remaps
/// propagate the kernel half to all task address spaces under that lock.
pub(crate) unsafe fn remap_page_in(
    root: PhysAddr,
    virt: VirtAddr,
    phys: PhysAddr,
    flags: u64,
) -> Result<(), MapError> {
    // SAFETY: same HHDM-walk discipline as `map_page_in`.
    let pml4 = unsafe { PageTable::at(root) };
    let pdpt = unsafe { next_table(pml4, index(virt, 4))? };
    let pd = unsafe { next_table(pdpt, index(virt, 3))? };
    let pt = unsafe { next_table(pd, index(virt, 2))? };
    let i = index(virt, 1);
    pt.entries[i] = (phys.0 & ADDR_MASK) | flags | PTE_PRESENT;
    flush(virt);
    crate::task::synchronize_kernel_mappings(root);
    Ok(())
}

/// Map 4 KiB virtual page `virt` in the active address space.
///
/// # Safety
///
/// Installing a mapping aliases physical memory into the address space; the
/// caller must ensure `phys` is safe to expose at `virt` with `flags`.
pub unsafe fn map_page(virt: VirtAddr, phys: PhysAddr, flags: u64) -> Result<(), MapError> {
    // SAFETY: CR3 names the live PML4, reachable through the HHDM.
    unsafe { map_page_in(active_pml4(), virt, phys, flags) }
}

/// Return the leaf page-table flags for `virt` in `root`, or `None` if unmapped.
pub(crate) fn page_flags_in(root: PhysAddr, virt: VirtAddr) -> Option<u64> {
    // SAFETY: `root` names a live PML4; every descent stops at a present entry.
    let pml4 = unsafe { PageTable::at(root) };
    let mut table: &PageTable = pml4;
    for level in (2..=4).rev() {
        let entry = table.entries[index(virt, level)];
        let child = child_table(entry, PTE_USER)?;
        // SAFETY: `child_table` proved the entry present, non-huge, and within
        // RAM, so it names a live lower-level table reachable via HHDM.
        table = unsafe { PageTable::at(child) };
    }
    let leaf = table.entries[index(virt, 1)];
    (leaf & PTE_PRESENT != 0 && leaf & PTE_USER != 0).then_some(leaf)
}

/// Like [`page_flags_in`] but does not require the `PTE_USER` bit. Used for
/// kernel-space mappings such as the PCI ECAM scratch page, where intermediate
/// entries are still created with `PTE_USER` (per `next_table`) but the leaf
/// intentionally omits it.
pub(crate) fn leaf_flags_in(root: PhysAddr, virt: VirtAddr) -> Option<u64> {
    // SAFETY: same HHDM-walk discipline as `page_flags_in`.
    let pml4 = unsafe { PageTable::at(root) };
    let mut table: &PageTable = pml4;
    for level in (2..=4).rev() {
        let entry = table.entries[index(virt, level)];
        let child = child_table(entry, 0)?;
        // SAFETY: `child_table` proved the entry present, non-huge, and within
        // RAM, so it names a live lower-level table reachable via HHDM.
        table = unsafe { PageTable::at(child) };
    }
    let leaf = table.entries[index(virt, 1)];
    (leaf & PTE_PRESENT != 0).then_some(leaf)
}

/// Translate a virtual address to its physical address, or `None` if unmapped.
pub(crate) fn translate_in(root: PhysAddr, virt: VirtAddr) -> Option<PhysAddr> {
    // SAFETY: `root` names a live PML4; every descent stops at a present entry.
    let pml4 = unsafe { PageTable::at(root) };
    let mut table: &PageTable = pml4;
    for level in (2..=4).rev() {
        let entry = table.entries[index(virt, level)];
        let child = child_table(entry, 0)?;
        // SAFETY: `child_table` proved the entry present, non-huge, and within
        // RAM, so it names a live lower-level table reachable via HHDM.
        table = unsafe { PageTable::at(child) };
    }
    let leaf = table.entries[index(virt, 1)];
    if leaf & PTE_PRESENT == 0 {
        return None;
    }
    let page = leaf & ADDR_MASK;
    Some(PhysAddr(page + (virt.0 & 0xfff)))
}

pub fn translate(virt: VirtAddr) -> Option<PhysAddr> {
    translate_in(active_pml4(), virt)
}
