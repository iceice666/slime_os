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
use crate::memory::vmm::{
    self, PTE_NO_EXECUTE, PTE_PRESENT, PTE_USER, PTE_WRITABLE, map_user_page_in,
};
use crate::memory::{PAGE_SIZE, PhysAddr, VirtAddr};

/// Maximum live shared buffers kernel-wide. Bounds the fixed table.
pub const MAX_SHARED_BUFFERS: usize = 32;

/// Maximum pages in a single shared buffer. Matched to the shared contiguous
/// allocator's per-run bound; a larger request is rejected structurally.
pub const MAX_BUFFER_PAGES: usize = CONTIG_MAX_FRAMES;

/// Fixed kernel-wide ceiling on the total pages held by all live shared
/// buffers at once. 256 pages == 1 MiB, bounded independently of the object
/// count so many small buffers cannot exceed the byte budget either.
pub const MAX_TOTAL_PAGES: usize = 256;

/// Fixed kernel-wide ceiling on live shared-buffer mappings. Bounds the
/// mapping table independently of the buffer and page ceilings; a per-holder
/// mapping quota (C7.3) is layered on top of this hard bound.
pub const MAX_MAPPINGS: usize = 64;

/// Exclusive upper bound of the canonical user half under 4-level x86-64
/// paging. Shared-buffer mappings are always task-private user mappings.
const USER_TOP: u64 = 0x0000_8000_0000_0000;

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
    /// The region was not allocated by this table (already released or forged),
    /// or a map/unmap named a region this holder does not have live.
    NotFound,
    /// Zero length, a non-page-aligned offset/length/virtual base, arithmetic
    /// overflow, or a range extending past the buffer's pages.
    BadRange,
    /// A writable mapping was requested for a region created read-only or since
    /// sealed; write access can never be widened back in.
    WriteDenied,
    /// The fixed kernel-wide mapping table is full ([`MAX_MAPPINGS`]).
    MappingsExhausted,
    /// A target virtual page is already mapped in the holder's address space.
    MapConflict,
}

/// Live per-owner charge. Keyed by the creating supervision-subtree owner
/// (a `TaskId`), so exhaustion, release, and subtree teardown are scoped to
/// one account and never disturb another owner's. An owner may carry a mapping
/// charge without a buffer charge: it mapped a region transferred to it.
#[derive(Clone, Copy)]
struct OwnerCharge {
    owner: u64,
    pages: u32,
    buffers: u32,
    mappings: u32,
}

impl OwnerCharge {
    fn is_empty(&self) -> bool {
        self.pages == 0 && self.buffers == 0 && self.mappings == 0
    }
}

struct Entry {
    region: SharedRegion,
    owner: u64,
}

/// One live shared-buffer mapping into a holder's address space. Recorded so a
/// later unmap, buffer release, or owner teardown can tear the exact user pages
/// back down and return the mapping charge. Identified by the buffer's
/// unforgeable `region_id` plus the user virtual base within one address space.
#[derive(Clone, Copy)]
struct Mapping {
    region_id: u64,
    owner: u64,
    root: PhysAddr,
    base: VirtAddr,
    pages: usize,
}

/// Upper bound on distinct owners that can hold a charge at once. An owner
/// entry is created by a buffer or a mapping, so this covers the worst case of
/// every buffer and every mapping belonging to a distinct owner; it never
/// underflows the live-task ceiling.
const MAX_CHARGE_OWNERS: usize = MAX_SHARED_BUFFERS + MAX_MAPPINGS;

/// Bounded table of live shared buffers and their mappings. Every allocation is
/// charged against both the global page total and the creating owner's
/// per-holder quota; release and owner teardown return both, and tear down
/// every page mapped from the affected buffers.
pub struct SharedBufferTable {
    regions: [Option<Entry>; MAX_SHARED_BUFFERS],
    charges: [Option<OwnerCharge>; MAX_CHARGE_OWNERS],
    mappings: [Option<Mapping>; MAX_MAPPINGS],
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
            charges: [const { None }; MAX_CHARGE_OWNERS],
            mappings: [const { None }; MAX_MAPPINGS],
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

    /// Live mappings currently charged to `owner`.
    pub fn owner_mappings(&self, owner: u64) -> u32 {
        self.charge_index(owner)
            .map(|slot| self.charges[slot].expect("charge present").mappings)
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
        self.charge_buffer(owner, pages_u32);
        Ok(region)
    }

    /// Map one page-aligned subrange of `region` into `root` at `base`, charged
    /// as one live mapping to `owner`. Bounds, write state, quota, and the fixed
    /// mapping table are checked before any PTE changes. A page-table failure is
    /// fully rolled back before returning.
    #[allow(clippy::too_many_arguments)]
    pub fn map(
        &mut self,
        owner: u64,
        quota: HolderQuota,
        region: &SharedRegion,
        root: PhysAddr,
        base: u64,
        offset: u64,
        length: u64,
        writable: bool,
    ) -> Result<(), SharedBufferError> {
        let page = PAGE_SIZE as u64;
        let region_bytes = (region.pages() as u64)
            .checked_mul(page)
            .ok_or(SharedBufferError::BadRange)?;
        let Some(end) = offset.checked_add(length) else {
            return Err(SharedBufferError::BadRange);
        };
        let Some(virtual_end) = base.checked_add(length) else {
            return Err(SharedBufferError::BadRange);
        };
        if length == 0
            || !base.is_multiple_of(page)
            || !offset.is_multiple_of(page)
            || !length.is_multiple_of(page)
            || base >= USER_TOP
            || virtual_end > USER_TOP
            || end > region_bytes
        {
            return Err(SharedBufferError::BadRange);
        }
        if !self.regions.iter().any(|entry| {
            entry
                .as_ref()
                .is_some_and(|entry| entry.region.ptr_eq(region))
        }) {
            return Err(SharedBufferError::NotFound);
        }
        if writable && (!region.writable() || region.sealed()) {
            return Err(SharedBufferError::WriteDenied);
        }
        if self
            .owner_mappings(owner)
            .checked_add(1)
            .is_none_or(|count| count > quota.mapping_count)
        {
            return Err(SharedBufferError::QuotaExceeded);
        }
        let mapping_slot = self
            .mappings
            .iter()
            .position(Option::is_none)
            .ok_or(SharedBufferError::MappingsExhausted)?;

        let pages = (length / page) as usize;
        let mut flags = PTE_USER | PTE_PRESENT | PTE_NO_EXECUTE;
        if writable {
            flags |= PTE_WRITABLE;
        }
        for index in 0..pages {
            let delta = (index * PAGE_SIZE) as u64;
            let virt = VirtAddr(base + delta);
            let phys = PhysAddr(region.phys().0 + offset + delta);
            // SAFETY: range validation proves `phys` is one of `region`'s
            // frames, and `virt` is the caller-supplied user page in `root`.
            let result = unsafe { map_user_page_in(root, virt, phys, flags) };
            if let Err(error) = result {
                for rollback in 0..index {
                    // SAFETY: these are exactly the pages this operation
                    // installed above; no mapping record has been published.
                    unsafe {
                        vmm::unmap_user_page_in(
                            root,
                            VirtAddr(base + (rollback * PAGE_SIZE) as u64),
                        );
                    }
                }
                return Err(match error {
                    vmm::MapError::AlreadyMapped => SharedBufferError::MapConflict,
                    vmm::MapError::OutOfFrames => SharedBufferError::OutOfFrames,
                });
            }
        }

        self.mappings[mapping_slot] = Some(Mapping {
            region_id: region.id(),
            owner,
            root,
            base: VirtAddr(base),
            pages,
        });
        self.charge_mapping(owner);
        Ok(())
    }

    /// Remove the exact mapping of `region` at `base` from `owner`'s address
    /// space and return its mapping charge. A stale or mismatched tuple returns
    /// `NotFound` without touching any PTE.
    pub fn unmap(
        &mut self,
        owner: u64,
        region: &SharedRegion,
        root: PhysAddr,
        base: u64,
    ) -> Result<(), SharedBufferError> {
        let slot = self
            .mappings
            .iter()
            .position(|mapping| {
                mapping.is_some_and(|mapping| {
                    mapping.owner == owner
                        && mapping.region_id == region.id()
                        && mapping.root == root
                        && mapping.base == VirtAddr(base)
                })
            })
            .ok_or(SharedBufferError::NotFound)?;
        self.teardown_mapping(slot);
        Ok(())
    }

    /// Irreversibly seal `region` read-only. Every present PTE in every live
    /// mapping is downgraded before the Arc-shared seal flag is published, so a
    /// holder cannot retain or later recover writable access.
    pub fn seal(&mut self, region: &SharedRegion) -> Result<(), SharedBufferError> {
        if !self.regions.iter().any(|entry| {
            entry
                .as_ref()
                .is_some_and(|entry| entry.region.ptr_eq(region))
        }) {
            return Err(SharedBufferError::NotFound);
        }
        if region.sealed() {
            return Ok(());
        }
        for mapping in self.mappings.iter().flatten() {
            if mapping.region_id != region.id() {
                continue;
            }
            for page in 0..mapping.pages {
                let _ = vmm::set_user_page_readonly_in(
                    mapping.root,
                    VirtAddr(mapping.base.0 + (page * PAGE_SIZE) as u64),
                );
            }
        }
        region.seal();
        Ok(())
    }

    /// Reclaim a shared buffer, first removing every mapping of its frames,
    /// then freeing them and returning page, object, and mapping charges.
    pub fn release(&mut self, region: &SharedRegion) -> Result<(), SharedBufferError> {
        let slot = self
            .regions
            .iter()
            .position(|r| r.as_ref().is_some_and(|e| e.region.ptr_eq(region)))
            .ok_or(SharedBufferError::NotFound)?;
        self.teardown_region_mappings(region.id());
        let held = self.regions[slot].take().expect("slot checked non-empty");
        // SAFETY: every mapping was torn down above and `held.region` was
        // allocated by `create` via `pmm::alloc_contiguous`.
        unsafe { pmm::free_contiguous(held.region.phys(), held.region.pages()) };
        self.total_pages -= held.region.pages();
        self.uncharge_buffer(held.owner, held.region.pages() as u32);
        Ok(())
    }

    /// Reclaim every mapping charged to `owner`, then every live buffer owned
    /// by it. Releasing each owned buffer additionally removes any mappings of
    /// that buffer held by another owner, preventing stale PTEs to freed frames.
    /// Unrelated owners' buffers and mappings remain untouched.
    pub fn reclaim_owner(&mut self, owner: u64) -> usize {
        for slot in 0..self.mappings.len() {
            if self.mappings[slot].is_some_and(|mapping| mapping.owner == owner) {
                self.teardown_mapping(slot);
            }
        }

        let mut reclaimed = 0;
        for slot in 0..self.regions.len() {
            let owned = self.regions[slot]
                .as_ref()
                .is_some_and(|entry| entry.owner == owner);
            if !owned {
                continue;
            }
            let region = self.regions[slot]
                .as_ref()
                .expect("owned region present")
                .region
                .clone();
            self.release(&region)
                .expect("owned region remains live during reclaim");
            reclaimed += 1;
        }
        reclaimed
    }

    fn teardown_region_mappings(&mut self, region_id: u64) {
        for slot in 0..self.mappings.len() {
            if self.mappings[slot].is_some_and(|mapping| mapping.region_id == region_id) {
                self.teardown_mapping(slot);
            }
        }
    }

    fn teardown_mapping(&mut self, slot: usize) {
        let mapping = self.mappings[slot]
            .take()
            .expect("mapping slot checked present");
        for page in 0..mapping.pages {
            // SAFETY: the mapping record names exactly the PTEs installed by
            // `map`; the buffer frames remain live until release completes.
            unsafe {
                vmm::unmap_user_page_in(
                    mapping.root,
                    VirtAddr(mapping.base.0 + (page * PAGE_SIZE) as u64),
                );
            }
        }
        self.uncharge_mapping(mapping.owner);
    }

    fn charge_slot(&mut self, owner: u64) -> usize {
        if let Some(slot) = self.charge_index(owner) {
            return slot;
        }
        let slot = self
            .charges
            .iter()
            .position(Option::is_none)
            .expect("charge owners bounded by buffers plus mappings");
        self.charges[slot] = Some(OwnerCharge {
            owner,
            pages: 0,
            buffers: 0,
            mappings: 0,
        });
        slot
    }

    fn remove_empty_charge(&mut self, slot: usize) {
        if self.charges[slot].is_some_and(|charge| charge.is_empty()) {
            self.charges[slot] = None;
        }
    }

    fn charge_buffer(&mut self, owner: u64, pages: u32) {
        let slot = self.charge_slot(owner);
        let charge = self.charges[slot].as_mut().expect("charge present");
        charge.pages += pages;
        charge.buffers += 1;
    }

    fn uncharge_buffer(&mut self, owner: u64, pages: u32) {
        let slot = self
            .charge_index(owner)
            .expect("released buffer was charged");
        let charge = self.charges[slot].as_mut().expect("charge present");
        charge.pages -= pages;
        charge.buffers -= 1;
        self.remove_empty_charge(slot);
    }

    fn charge_mapping(&mut self, owner: u64) {
        let slot = self.charge_slot(owner);
        self.charges[slot]
            .as_mut()
            .expect("charge present")
            .mappings += 1;
    }

    fn uncharge_mapping(&mut self, owner: u64) {
        let slot = self.charge_index(owner).expect("live mapping was charged");
        self.charges[slot]
            .as_mut()
            .expect("charge present")
            .mappings -= 1;
        self.remove_empty_charge(slot);
    }
}

pub static SHARED_BUFFER_TABLE: LazyLock<Mutex<SharedBufferTable>> =
    LazyLock::new(|| Mutex::new(SharedBufferTable::new()));
