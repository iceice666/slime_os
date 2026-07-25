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
use crate::memory::pmm::{self, CONTIG_MAX_FRAMES};
const MAX_PINNED_REGIONS: usize = 32;

/// Upper bound on pages per pinned region. Larger requests are rejected
/// structurally. Matched to the shared contiguous-allocator bound.
const MAX_PIN_PAGES: usize = CONTIG_MAX_FRAMES;

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
        let base = pmm::alloc_contiguous(pages).ok_or(DmaError::OutOfFrames)?;
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
        unsafe { pmm::free_contiguous(region.phys(), region.pages()) };
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
