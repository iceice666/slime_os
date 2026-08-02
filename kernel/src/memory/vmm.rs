//! Virtual memory: multi-level page-table mapping.
//!
//! The boot path leaves the whole of physical RAM mapped at the direct-map
//! offset, so any page table can be read and written by taking its physical
//! frame address through [`PhysAddr::to_virt`]. This module walks the active
//! hierarchy, allocating intermediate tables from the PMM as needed, and
//! installs leaf mappings at [`PAGE_SIZE`].
//!
//! The table shape, entry encoding, root register, and TLB instruction come
//! from `arch::paging`; the walk, allocation discipline, permission checks, and
//! teardown here are architecture-neutral. Neutral callers select mappings by
//! intent ([`PTE_WRITABLE`], [`PTE_DEVICE`]) rather than by bit position.
//!
//! Where an encoding's *polarity* differs between architectures — x86 sets a
//! bit to mean "block" and to permit writes, AArch64 clears one for both — the
//! boundary exports a predicate (`is_block`, `is_writable`, `make_read_only`)
//! instead of a bitmask, because no single mask can express both directions.
//!
//! Design note: mapping errors are *values*, not hangs. [`map_page`] reports a
//! typed [`MapError`] when a mapping already exists or a frame runs out, which
//! is the memory-management half of the milestone's "faults are reported
//! deterministically rather than silently hanging" exit condition.

use super::pmm::FRAME_ALLOCATOR;
use super::{PAGE_SIZE, PhysAddr, VirtAddr};
use crate::arch::paging::{
    ENTRIES_PER_TABLE, PAGE_TABLE_LEVELS, PTE_ADDR_MASK, PTE_INTERMEDIATE, active_root, flush_tlb,
    is_block, make_read_only, table_index,
};

pub use crate::arch::paging::{
    PTE_CACHE_DISABLE, PTE_DEVICE, PTE_NO_EXECUTE, PTE_PRESENT, PTE_USER, PTE_WRITABLE,
    PTE_WRITE_THROUGH, is_writable,
};

/// Index of the first kernel-half entry in the top-level table. Entries below
/// it are per-address-space user mappings; entries from here up are aliases of
/// the single shared kernel hierarchy.
const KERNEL_HALF_START: usize = ENTRIES_PER_TABLE / 2;

/// Follow an intermediate (PML4/PDPT/PD) entry to the physical frame of its
/// child table, or `None` if the entry cannot be safely descended.
///
/// Rejects entries that are absent, missing any bit in `required` (e.g.
/// [`PTE_USER`]), map a huge page (no 4 KiB child table exists), or point
/// outside physical RAM. The last case makes a corrupted table produce a typed
/// `None` here instead of a wild HHDM dereference that faults deep inside the
/// walker and misattributes the failure.
fn child_table(entry: u64, required: u64) -> Option<PhysAddr> {
    if entry & (PTE_PRESENT | required) != PTE_PRESENT | required || is_block(entry) {
        return None;
    }
    let phys = PhysAddr(entry & PTE_ADDR_MASK);
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

/// A hardware page table: [`ENTRIES_PER_TABLE`] 64-bit entries, one page.
#[repr(C, align(4096))]
struct PageTable {
    entries: [u64; ENTRIES_PER_TABLE],
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

/// Physical address of the active top-level table.
pub(crate) fn active_pml4() -> PhysAddr {
    active_root()
}

/// Alias the shared kernel half of `source` into `destination`, so a new
/// address space sees the one kernel hierarchy.
///
/// # Safety
///
/// Both frames must be live top-level tables reachable through the direct map,
/// and `destination` must not be the active root of a running task.
pub(crate) unsafe fn copy_kernel_half(source: PhysAddr, destination: PhysAddr) {
    unsafe {
        let src = source.to_virt().as_mut_ptr::<u64>();
        let dst = destination.to_virt().as_mut_ptr::<u64>();
        core::ptr::copy_nonoverlapping(
            src.add(KERNEL_HALF_START),
            dst.add(KERNEL_HALF_START),
            ENTRIES_PER_TABLE - KERNEL_HALF_START,
        );
    }
}

/// Invalidate the TLB entry for `virt` after changing its mapping.
fn flush(virt: VirtAddr) {
    flush_tlb(virt);
}

/// Descend into the next-level table an entry points at, allocating and
/// zeroing a fresh table when the entry is absent.
///
/// # Safety
///
/// `table` must be a live table borrowed through the HHDM.
unsafe fn next_table(table: &mut PageTable, i: usize) -> Result<&'static mut PageTable, MapError> {
    let entry = table.entries[i];
    // A block entry at this level already maps the whole region, so there is
    // no lower table to descend into and no leaf can be installed here.
    if entry & PTE_PRESENT != 0 && is_block(entry) {
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
        // Intermediate entries are permissive; the leaf entry's flags decide
        // the effective permissions.
        table.entries[i] = frame.0 | PTE_INTERMEDIATE;
        frame
    } else {
        PhysAddr(entry & PTE_ADDR_MASK)
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
    let mut table = pml4;
    for level in (2..=PAGE_TABLE_LEVELS).rev() {
        // SAFETY: each descent borrows a live table reached through the direct map.
        table = unsafe { next_table(table, table_index(virt, level))? };
    }
    let pt = table;

    let i = table_index(virt, 1);
    if pt.entries[i] & PTE_PRESENT != 0 {
        return Err(MapError::AlreadyMapped);
    }
    pt.entries[i] = (phys.0 & PTE_ADDR_MASK) | flags | PTE_PRESENT;
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
    let mut table = pml4;
    for level in (2..=PAGE_TABLE_LEVELS).rev() {
        // SAFETY: each descent borrows a live table reached through the direct map.
        table = unsafe { next_table(table, table_index(virt, level))? };
    }
    let pt = table;
    let i = table_index(virt, 1);
    pt.entries[i] = (phys.0 & PTE_ADDR_MASK) | flags | PTE_PRESENT;
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

/// Map 4 KiB user page `virt` to `phys` with `flags` in `root`'s user half.
///
/// Unlike [`map_page_in`], this does **not** propagate the kernel half to other
/// address spaces, so it never takes the scheduler lock and is safe to call
/// while another kernel lock (e.g. the shared-buffer table) is held. It is for
/// user-half mappings only; the caller must pass [`PTE_USER`] in `flags`.
/// Returns [`MapError::AlreadyMapped`] without overwriting an existing leaf.
///
/// # Safety
///
/// Installing a mapping aliases physical memory into the address space; the
/// caller must ensure `phys` is safe to expose at `virt` with `flags`.
pub(crate) unsafe fn map_user_page_in(
    root: PhysAddr,
    virt: VirtAddr,
    phys: PhysAddr,
    flags: u64,
) -> Result<(), MapError> {
    // SAFETY: `root` names a live PML4, reachable through the HHDM.
    let pml4 = unsafe { PageTable::at(root) };
    // SAFETY: each descent borrows a live table reached through the HHDM.
    let mut table = pml4;
    for level in (2..=PAGE_TABLE_LEVELS).rev() {
        // SAFETY: each descent borrows a live table reached through the direct map.
        table = unsafe { next_table(table, table_index(virt, level))? };
    }
    let pt = table;
    let i = table_index(virt, 1);
    if pt.entries[i] & PTE_PRESENT != 0 {
        return Err(MapError::AlreadyMapped);
    }
    pt.entries[i] = (phys.0 & PTE_ADDR_MASK) | flags | PTE_PRESENT;
    flush(virt);
    Ok(())
}

/// Borrow the leaf entry for user page `virt` in `root`, descending only
/// through present, non-huge, user tables. Returns `None` if any level is
/// absent (the page is unmapped) — it never allocates a table.
fn user_leaf_mut(root: PhysAddr, virt: VirtAddr) -> Option<&'static mut u64> {
    // SAFETY: `root` names a live PML4; every descent stops at a present entry.
    let mut table: &mut PageTable = unsafe { PageTable::at(root) };
    for level in (2..=PAGE_TABLE_LEVELS).rev() {
        let entry = table.entries[table_index(virt, level)];
        let child = child_table(entry, PTE_USER)?;
        // SAFETY: `child_table` proved the entry present, non-huge, in RAM.
        table = unsafe { PageTable::at(child) };
    }
    Some(&mut table.entries[table_index(virt, 1)])
}

/// Remove the user leaf mapping for `virt` in `root`. Returns `true` if a
/// present user leaf was cleared, `false` if the page was already unmapped.
/// Only the leaf entry is touched; intermediate tables intentionally remain.
///
/// # Safety
///
/// The caller must ensure the physical frame behind `virt` is safe to
/// invalidate — no live borrow must outlast the unmap.
pub(crate) unsafe fn unmap_user_page_in(root: PhysAddr, virt: VirtAddr) -> bool {
    let Some(leaf) = user_leaf_mut(root, virt) else {
        return false;
    };
    if *leaf & (PTE_PRESENT | PTE_USER) != PTE_PRESENT | PTE_USER {
        return false;
    }
    *leaf = 0;
    flush(virt);
    true
}

/// Downgrade the user leaf mapping for `virt` in `root` to read-only by
/// applying the architecture's read-only encoding. Returns `true` if a present
/// user leaf was found
/// (already read-only counts as success), `false` if the page is unmapped.
pub(crate) fn set_user_page_readonly_in(root: PhysAddr, virt: VirtAddr) -> bool {
    let Some(leaf) = user_leaf_mut(root, virt) else {
        return false;
    };
    if *leaf & (PTE_PRESENT | PTE_USER) != PTE_PRESENT | PTE_USER {
        return false;
    }
    *leaf = make_read_only(*leaf);
    flush(virt);
    true
}

/// Return the leaf page-table flags for `virt` in `root`, or `None` if unmapped.
pub(crate) fn page_flags_in(root: PhysAddr, virt: VirtAddr) -> Option<u64> {
    // SAFETY: `root` names a live PML4; every descent stops at a present entry.
    let pml4 = unsafe { PageTable::at(root) };
    let mut table: &PageTable = pml4;
    for level in (2..=PAGE_TABLE_LEVELS).rev() {
        let entry = table.entries[table_index(virt, level)];
        let child = child_table(entry, PTE_USER)?;
        // SAFETY: `child_table` proved the entry present, non-huge, and within
        // RAM, so it names a live lower-level table reachable via HHDM.
        table = unsafe { PageTable::at(child) };
    }
    let leaf = table.entries[table_index(virt, 1)];
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
    for level in (2..=PAGE_TABLE_LEVELS).rev() {
        let entry = table.entries[table_index(virt, level)];
        let child = child_table(entry, 0)?;
        // SAFETY: `child_table` proved the entry present, non-huge, and within
        // RAM, so it names a live lower-level table reachable via HHDM.
        table = unsafe { PageTable::at(child) };
    }
    let leaf = table.entries[table_index(virt, 1)];
    (leaf & PTE_PRESENT != 0).then_some(leaf)
}

/// Translate a virtual address to its physical address, or `None` if unmapped.
pub(crate) fn translate_in(root: PhysAddr, virt: VirtAddr) -> Option<PhysAddr> {
    // SAFETY: `root` names a live PML4; every descent stops at a present entry.
    let pml4 = unsafe { PageTable::at(root) };
    let mut table: &PageTable = pml4;
    for level in (2..=PAGE_TABLE_LEVELS).rev() {
        let entry = table.entries[table_index(virt, level)];
        let child = child_table(entry, 0)?;
        // SAFETY: `child_table` proved the entry present, non-huge, and within
        // RAM, so it names a live lower-level table reachable via HHDM.
        table = unsafe { PageTable::at(child) };
    }
    let leaf = table.entries[table_index(virt, 1)];
    if leaf & PTE_PRESENT == 0 {
        return None;
    }
    let page = leaf & PTE_ADDR_MASK;
    Some(PhysAddr(page + (virt.0 & 0xfff)))
}

pub fn translate(virt: VirtAddr) -> Option<PhysAddr> {
    translate_in(active_pml4(), virt)
}

/// Free every user-half frame reachable from `root`: leaf pages first, then the
/// intermediate tables that held them.
///
/// This is the release path for an address space that is going away (B9). The
/// kernel half is deliberately untouched — entries from [`KERNEL_HALF_START`]
/// up are shared aliases of the one kernel hierarchy, copied in by
/// [`AddressSpace::new`](super::address_space::AddressSpace::new), so freeing
/// them would tear down the kernel's own mappings for every task.
///
/// Only entries carrying [`PTE_USER`] are followed, and huge entries are left
/// alone: nothing in this kernel installs a user huge page, so one appearing
/// here means the table is not ours to walk. Both rules make a corrupted table
/// leak rather than free a frame the allocator may hand out again.
///
/// Returns the number of frames released. Nothing consumes it today — the
/// conservation gates measure the allocator's own free count instead, which is
/// the stronger check because it also catches a frame freed twice. It is
/// returned for the caller that wants to attribute a teardown.
///
/// # Safety
///
/// `root` must name a live top-level table whose user half no longer has any
/// live borrow — no task may be running in this address space, and no kernel
/// mapping may still alias one of its user frames.
pub(crate) unsafe fn free_user_half(root: PhysAddr) -> usize {
    // SAFETY: the caller guarantees `root` is a live top-level table reached
    // through the direct map. Only the user half is walked.
    unsafe { free_table(root, PAGE_TABLE_LEVELS, 0..KERNEL_HALF_START) }
}

/// Clear and free every user entry of the table at `frame`, which sits at
/// `level` in the hierarchy, restricted to the entry indices in `range`.
/// The table at `frame` itself is *not* freed; its owner does that.
///
/// Recursion depth is bounded by [`PAGE_TABLE_LEVELS`].
///
/// # Safety
///
/// `frame` must name a live table at `level` with no outstanding borrow, and
/// the address space must be dead.
unsafe fn free_table(frame: PhysAddr, level: u8, range: core::ops::Range<usize>) -> usize {
    let mut freed = 0;
    // SAFETY: the caller guarantees `frame` names a live table.
    let table = unsafe { PageTable::at(frame) };
    for index in range {
        let entry = table.entries[index];
        if level == 1 {
            // Leaf level: the entry names a user data frame.
            if entry & (PTE_PRESENT | PTE_USER) != PTE_PRESENT | PTE_USER {
                continue;
            }
            table.entries[index] = 0;
            // SAFETY: the leaf is cleared, the address space is dead, and the
            // present check kept the frame inside a mapped user page.
            unsafe {
                FRAME_ALLOCATOR
                    .lock()
                    .dealloc(PhysAddr(entry & PTE_ADDR_MASK))
            };
            freed += 1;
            continue;
        }
        let Some(child) = child_table(entry, PTE_USER) else {
            continue;
        };
        // SAFETY: `child_table` validated the entry as a present, non-huge,
        // in-range table frame one level down.
        freed += unsafe { free_table(child, level - 1, 0..ENTRIES_PER_TABLE) };
        table.entries[index] = 0;
        // SAFETY: every entry the child held is cleared and freed.
        unsafe { FRAME_ALLOCATOR.lock().dealloc(child) };
        freed += 1;
    }
    freed
}
