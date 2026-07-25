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

/// Per-holder shared-buffer quota (C7.3), charged to a supervision subtree
/// owner rather than an ambient global. Every field is an absolute live
/// ceiling; a holder absent from the generation budget receives the
/// deny-by-default [`HolderQuota::DENY`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HolderQuota {
    pub byte_pages: u32,
    pub buffer_count: u32,
    pub mapping_count: u32,
    pub loan_count: u32,
}

impl HolderQuota {
    /// The quota a holder with no generation-declared budget receives: it may
    /// hold nothing. Authority is never ambient.
    pub const DENY: Self = Self {
        byte_pages: 0,
        buffer_count: 0,
        mapping_count: 0,
        loan_count: 0,
    };
}

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
    /// Allocating would exceed the creating holder's declared page or
    /// buffer-count quota.
    QuotaExceeded,
    /// The region was not allocated by this table (already released or forged).
    NotFound,
}

/// Live per-owner charge. Keyed by the creating supervision-subtree owner
/// (a `TaskId`), so exhaustion, release, and subtree teardown are scoped to
/// one account and never disturb another owner's.
#[derive(Clone, Copy)]
struct OwnerCharge {
    owner: u64,
    pages: u32,
    buffers: u32,
}

struct Entry {
    region: SharedRegion,
    owner: u64,
}

/// Bounded table of live shared buffers. Every allocation is charged against
/// both the global page total and the creating owner's per-holder quota;
/// release and owner teardown return both.
pub struct SharedBufferTable {
    regions: [Option<Entry>; MAX_SHARED_BUFFERS],
    charges: [Option<OwnerCharge>; MAX_SHARED_BUFFERS],
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
            charges: [const { None }; MAX_SHARED_BUFFERS],
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

    /// Pages currently charged to `owner`.
    pub fn owner_pages(&self, owner: u64) -> u32 {
        self.charge_index(owner)
            .map(|slot| self.charges[slot].expect("charge present").pages)
            .unwrap_or(0)
    }

    /// Live buffers currently charged to `owner`.
    pub fn owner_buffers(&self, owner: u64) -> u32 {
        self.charge_index(owner)
            .map(|slot| self.charges[slot].expect("charge present").buffers)
            .unwrap_or(0)
    }

    fn charge_index(&self, owner: u64) -> Option<usize> {
        self.charges
            .iter()
            .position(|c| c.is_some_and(|c| c.owner == owner))
    }

    /// Allocate `pages` contiguous physical frames as a new shared buffer owned
    /// by `owner` and return its kernel-identified handle. Global and per-owner
    /// bounds are checked before any frame is pulled, so a rejected request
    /// disturbs neither physical memory nor any existing holder. `quota` is the
    /// owner's declared per-holder ceiling; a holder with [`HolderQuota::DENY`]
    /// cannot allocate. `writable` records whether the creating holder may
    /// later write into the region.
    pub fn create(
        &mut self,
        owner: u64,
        quota: HolderQuota,
        pages: usize,
        writable: bool,
    ) -> Result<SharedRegion, SharedBufferError> {
        if pages == 0 || pages > MAX_BUFFER_PAGES {
            return Err(SharedBufferError::BadSize);
        }
        // Per-owner quota is checked before the global ceiling and before any
        // frame is pulled: one holder reaching its quota is a QuotaExceeded, not
        // a global-exhaustion signal another holder could observe.
        let charged_pages = self.owner_pages(owner);
        let charged_buffers = self.owner_buffers(owner);
        let pages_u32 = pages as u32;
        if charged_buffers
            .checked_add(1)
            .is_none_or(|n| n > quota.buffer_count)
            || charged_pages
                .checked_add(pages_u32)
                .is_none_or(|n| n > quota.byte_pages)
        {
            return Err(SharedBufferError::QuotaExceeded);
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
        self.regions[slot] = Some(Entry {
            region: region.clone(),
            owner,
        });
        self.total_pages += pages;
        self.charge(owner, pages_u32);
        Ok(region)
    }

    /// Reclaim a shared buffer, freeing its frames and returning its page,
    /// object, and per-owner charges. Fails with `NotFound` for a region this
    /// table never created or already released. Releasing invalidates only the
    /// kernel-held record; the caller drops its own capability separately.
    pub fn release(&mut self, region: &SharedRegion) -> Result<(), SharedBufferError> {
        let slot = self
            .regions
            .iter()
            .position(|r| r.as_ref().is_some_and(|e| e.region.ptr_eq(region)))
            .ok_or(SharedBufferError::NotFound)?;
        let held = self.regions[slot].take().expect("slot checked non-empty");
        // SAFETY: `held.region` was allocated by `create` via
        // `pmm::alloc_contiguous` and is no longer referenced by this table.
        unsafe { pmm::free_contiguous(held.region.phys(), held.region.pages()) };
        self.total_pages -= held.region.pages();
        self.uncharge(held.owner, held.region.pages() as u32);
        Ok(())
    }

    /// Reclaim every live shared buffer owned by `owner`, returning all of its
    /// pages, objects, and charges. Used on peer death, supervised restart, and
    /// explicit revocation of a supervision subtree; a buffer owned by any other
    /// subtree is untouched. Returns the number of buffers reclaimed.
    pub fn reclaim_owner(&mut self, owner: u64) -> usize {
        let mut reclaimed = 0;
        for slot in 0..self.regions.len() {
            let owned = self.regions[slot]
                .as_ref()
                .is_some_and(|e| e.owner == owner);
            if !owned {
                continue;
            }
            let held = self.regions[slot].take().expect("slot checked non-empty");
            // SAFETY: as in `release`; the region leaves the table here.
            unsafe { pmm::free_contiguous(held.region.phys(), held.region.pages()) };
            self.total_pages -= held.region.pages();
            self.uncharge(held.owner, held.region.pages() as u32);
            reclaimed += 1;
        }
        reclaimed
    }

    fn charge(&mut self, owner: u64, pages: u32) {
        if let Some(slot) = self.charge_index(owner) {
            let charge = self.charges[slot].as_mut().expect("charge present");
            charge.pages += pages;
            charge.buffers += 1;
            return;
        }
        let slot = self
            .charges
            .iter()
            .position(|c| c.is_none())
            .expect("charge slots bounded by region slots");
        self.charges[slot] = Some(OwnerCharge {
            owner,
            pages,
            buffers: 1,
        });
    }

    fn uncharge(&mut self, owner: u64, pages: u32) {
        let slot = self
            .charge_index(owner)
            .expect("released buffer was charged");
        let charge = self.charges[slot].as_mut().expect("charge present");
        charge.pages -= pages;
        charge.buffers -= 1;
        if charge.buffers == 0 {
            self.charges[slot] = None;
        }
    }
}

pub static SHARED_BUFFER_TABLE: LazyLock<Mutex<SharedBufferTable>> =
    LazyLock::new(|| Mutex::new(SharedBufferTable::new()));
