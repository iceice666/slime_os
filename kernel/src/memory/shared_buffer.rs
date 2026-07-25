//! Shared-buffer factory allocation (C7.2).
//!
//! A `SharedBufferFactory` capability authorizes a component to allocate and
//! release kernel-identified [`SharedRegion`] objects under fixed global byte
//! and object ceilings. This module owns the bookkeeping — which physical page
//! runs back each live buffer, the running page total, and the monotonic
//! identity counter — while [`crate::capability::SharedRegion`] carries the
//! per-grant handle.
//!
//! No policy (which component may create a buffer, how large, how many) lives
//! here; that is enforced by the `RIGHT_BUFFER_CREATE` gate at the syscall
//! layer and by generation-declared factory grants. This module only enforces
//! the hard kernel-wide bounds: a component cannot exhaust physical memory or
//! the object table regardless of what its manifest permits.
//!
//! Shared-sample authority is deliberately a distinct capability kind from DMA
//! authority ([`crate::capability::DmaMemory`]) even though both draw from the
//! shared contiguous frame allocator in [`crate::memory::pmm`]. Holding one
//! never grants the other.

use spin::{LazyLock, Mutex};

use crate::capability::SharedRegion;
use crate::memory::pmm::{self, CONTIG_MAX_FRAMES};

/// Maximum live shared buffers kernel-wide. Bounds the fixed table.
pub const MAX_SHARED_BUFFERS: usize = 32;

/// Maximum pages in a single shared buffer. Matched to the shared contiguous
/// allocator's per-run bound; a larger request is rejected structurally.
pub const MAX_BUFFER_PAGES: usize = CONTIG_MAX_FRAMES;

/// Fixed kernel-wide ceiling on the total pages held by all live shared
/// buffers at once. 256 pages == 1 MiB, bounded independently of the object
/// count so many small buffers cannot exceed the byte budget either.
pub const MAX_TOTAL_PAGES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedBufferError {
    /// Zero pages, or more than [`MAX_BUFFER_PAGES`].
    BadSize,
    /// No contiguous physical run of the requested size is available.
    OutOfFrames,
    /// The live-buffer table is full ([`MAX_SHARED_BUFFERS`]).
    ObjectsExhausted,
    /// Allocating would exceed the global page ceiling ([`MAX_TOTAL_PAGES`]).
    BytesExhausted,
    /// The region was not allocated by this table (already released or forged).
    NotFound,
}

/// Bounded table of live shared buffers. Every allocation is charged against
/// both the object count and the global page total; release returns both.
pub struct SharedBufferTable {
    regions: [Option<SharedRegion>; MAX_SHARED_BUFFERS],
    total_pages: usize,
    next_id: u64,
}

impl Default for SharedBufferTable {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedBufferTable {
    pub const fn new() -> Self {
        Self {
            regions: [const { None }; MAX_SHARED_BUFFERS],
            total_pages: 0,
            next_id: 1,
        }
    }

    /// Pages currently held across all live shared buffers.
    pub fn total_pages(&self) -> usize {
        self.total_pages
    }

    /// Number of live shared buffers.
    pub fn live_count(&self) -> usize {
        self.regions.iter().filter(|r| r.is_some()).count()
    }

    /// Allocate `pages` contiguous physical frames as a new shared buffer and
    /// return its kernel-identified handle. Bounds are checked before any frame
    /// is pulled, so a rejected request disturbs neither physical memory nor an
    /// existing holder. `writable` records whether the creating holder may
    /// later write into the region.
    pub fn create(
        &mut self,
        pages: usize,
        writable: bool,
    ) -> Result<SharedRegion, SharedBufferError> {
        if pages == 0 || pages > MAX_BUFFER_PAGES {
            return Err(SharedBufferError::BadSize);
        }
        if self.total_pages + pages > MAX_TOTAL_PAGES {
            return Err(SharedBufferError::BytesExhausted);
        }
        let slot = self
            .regions
            .iter()
            .position(|r| r.is_none())
            .ok_or(SharedBufferError::ObjectsExhausted)?;
        let base = pmm::alloc_contiguous(pages).ok_or(SharedBufferError::OutOfFrames)?;
        let id = self.next_id;
        self.next_id += 1;
        let region = SharedRegion::new(id, base, pages, writable);
        self.regions[slot] = Some(region.clone());
        self.total_pages += pages;
        Ok(region)
    }

    /// Reclaim a shared buffer, freeing its frames and returning its page and
    /// object charges. Fails with `NotFound` for a region this table never
    /// created or already released. Releasing invalidates only the kernel-held
    /// record; the caller drops its own capability separately.
    pub fn release(&mut self, region: &SharedRegion) -> Result<(), SharedBufferError> {
        let slot = self
            .regions
            .iter()
            .position(|r| r.as_ref().is_some_and(|r| r.ptr_eq(region)))
            .ok_or(SharedBufferError::NotFound)?;
        let held = self.regions[slot].take().expect("slot checked non-empty");
        // SAFETY: `held` was allocated by `create` via `pmm::alloc_contiguous`
        // and is no longer referenced by this table.
        unsafe { pmm::free_contiguous(held.phys(), held.pages()) };
        self.total_pages -= held.pages();
        Ok(())
    }
}

pub static SHARED_BUFFER_TABLE: LazyLock<Mutex<SharedBufferTable>> =
    LazyLock::new(|| Mutex::new(SharedBufferTable::new()));
