//! Pinned DMA memory lifecycle.
//!
//! M5.1 deliverable: pin DMA pages for the complete device operation and
//! reclaim them only after completion or reset. This module owns the
//! bookkeeping — which physical pages are pinned, and whether an outstanding
//! device request references them — while [`crate::capability::DmaRegion`]
//! carries the per-grant handle.
//!
//! The allocator reuses the PMM for frames and tracks a small bounded table of
//! pinned regions so the kernel can refuse reclamation while a request is in
//! flight. No policy (which driver may pin, how many pages) lives here; that
//! is enforced by capability grants at the syscall layer.

use spin::{LazyLock, Mutex};

use crate::capability::DmaRegion;
use crate::memory::{PAGE_SIZE, PhysAddr, pmm::FRAME_ALLOCATOR};
const MAX_PINNED_REGIONS: usize = 32;

/// Upper bound on pages per pinned region, matched to the `collected` trial
/// array in [`alloc_contiguous`]. Larger requests are rejected structurally.
const MAX_PIN_PAGES: usize = 64;

/// Bounded table of pinned DMA regions. Reclamation is refused while a
/// region's `outstanding` flag is set.
pub struct DmaTable {
    regions: [Option<DmaRegion>; MAX_PINNED_REGIONS],
}

impl Default for DmaTable {
    fn default() -> Self {
        Self::new()
    }
}

impl DmaTable {
    pub fn new() -> Self {
        Self {
            regions: core::array::from_fn(|_| None),
        }
    }

    /// Pin `pages` contiguous physical frames and return a [`DmaRegion`]
    /// handle. Contiguity is required because virtio descriptors carry a
    /// single physical address + length.
    pub fn pin(&mut self, pages: usize) -> Result<DmaRegion, DmaError> {
        if pages == 0 || pages > MAX_PIN_PAGES {
            return Err(DmaError::BadSize);
        }
        let base = alloc_contiguous(pages).ok_or(DmaError::OutOfFrames)?;
        let region = DmaRegion::new(base, pages);
        let slot = self
            .regions
            .iter_mut()
            .position(|r| r.is_none())
            .ok_or(DmaError::TableFull)?;
        self.regions[slot] = Some(region.clone());
        Ok(region)
    }

    /// Reclaim a pinned region. Refused while its `outstanding` flag is set.
    pub fn release(&mut self, region: &DmaRegion) -> Result<(), DmaError> {
        if region.outstanding() {
            return Err(DmaError::Outstanding);
        }
        let slot = self
            .regions
            .iter()
            .position(|r| r.as_ref().is_some_and(|r| r.ptr_eq(region)))
            .ok_or(DmaError::NotPinned)?;
        // Only the kernel-held slot actually frees the frames; the granted
        // clone is dropped by the caller separately.
        // SAFETY: the region is not outstanding and was allocated by `pin`.
        unsafe { free_contiguous(region.phys(), region.pages()) };
        self.regions[slot] = None;
        Ok(())
    }
}

pub static DMA_TABLE: LazyLock<Mutex<DmaTable>> = LazyLock::new(|| Mutex::new(DmaTable::new()));
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmaError {
    BadSize,
    OutOfFrames,
    TableFull,
    Outstanding,
    NotPinned,
}

/// Allocate `pages` physically contiguous frames. Falls back to scanning the
/// free list for a run of consecutive frame numbers; for the bounded QEMU
/// vertical slice this is sufficient and avoids a buddy allocator.
fn alloc_contiguous(pages: usize) -> Option<PhysAddr> {
    let mut alloc = FRAME_ALLOCATOR.lock();
    // Try `pages` times: each attempt pulls a candidate base, then verifies the
    // next `pages-1` frames are also free by attempting to allocate and
    // checking contiguity. On failure, return the trial frames to the list.
    for _ in 0..pages * 4 {
        let base = alloc.alloc()?;
        let mut collected = [PhysAddr(0); 64];
        collected[0] = base;
        let mut ok = true;
        for i in 1..pages {
            let next = alloc.alloc();
            match next {
                Some(p) if p.0 == base.0 + (i as u64) * PAGE_SIZE as u64 => {
                    collected[i] = p;
                }
                Some(p) => {
                    // Non-contiguous: return all collected frames and retry.
                    // SAFETY: these frames came from `alloc` and are unused.
                    unsafe { alloc.dealloc(p) };
                    for frame in collected.iter().take(i).skip(1) {
                        unsafe { alloc.dealloc(*frame) };
                    }
                    ok = false;
                    break;
                }
                None => {
                    for frame in collected.iter().take(i).skip(1) {
                        unsafe { alloc.dealloc(*frame) };
                    }
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            return Some(base);
        }
        // else: base is retained as the trial; loop and try a new base.
        // SAFETY: `base` was allocated above and is unused on this path.
        unsafe { alloc.dealloc(base) };
    }
    None
}

/// Free a contiguous run previously allocated by [`alloc_contiguous`].
///
/// # Safety
///
/// `base` must name a region of `pages` contiguous frames currently pinned by
/// this table, and the region must not be outstanding.
unsafe fn free_contiguous(base: PhysAddr, pages: usize) {
    let mut alloc = FRAME_ALLOCATOR.lock();
    for i in 0..pages {
        // SAFETY: caller guarantees these frames are owned and unused.
        unsafe {
            alloc.dealloc(PhysAddr(base.0 + (i as u64) * PAGE_SIZE as u64));
        }
    }
}
