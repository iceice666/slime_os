//! Demand-backed private acquisition.
//!
//! An authorized maximum is permission to ask, not a reservation. A holder
//! admitted with an elastic ceiling sequesters nothing at construction beyond
//! its guarantee, and every growth takes its own frames, extents, allocation
//! descriptors and root CSlots from the common pool at the moment the request
//! arrives.
//!
//! Acquisition is deliberately reversible. Extents, descriptors and slots are
//! obtained first, before any retype or mapping the caller cannot undo, so the
//! policy ledger judges a transaction against the placements it will actually
//! use and a refusal returns every byte it took. What cannot be returned is
//! stated rather than hidden: a leaf table already mapped into a span stays
//! bound to that span, remains charged to its holder, and is never published
//! to a kind-wide reusable pool where another span could skip `map_leaf`.
//!
//! Extents are sized to the request instead of to a quota. An exact
//! power-of-two decomposition means a growth's reserved bytes equal its
//! payload plus its tables, so the pool loses exactly what the holder gained
//! and a fragmented machine can still serve page-granular growth from blocks
//! no 2 MiB extent would fit.

use boot_contracts::private_memory_policy::ledger;

use super::{
    AllocError, ExtentKind, GRANULE_BYTES, MAX_PRIVATE_EXTENT_BYTES, ObjectAllocator,
    PrivateObjectKind, TaskArenaId,
};

/// Bounded, compile-time failure injection for the rollback qualification.
///
/// Only the operations no kernel-facing trait can reach are injected here:
/// acquisition of an extent or a descriptor, and the revoke a rollback
/// depends on. Retype and mapping failures need no product-code hook, because
/// the growth path already takes its kernel through a trait the scenario can
/// implement.
#[cfg(slime_private_rollback)]
pub mod inject {
    use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// Fail the acquisition of the extent at this index, once.
    pub(super) static EXTENT: AtomicUsize = AtomicUsize::new(usize::MAX);
    /// Fail descriptor provisioning of the next acquisition, once.
    pub(super) static DESCRIPTORS: AtomicBool = AtomicBool::new(false);
    /// Fail the first revoke a rollback attempts, once.
    pub(super) static REVOKE: AtomicBool = AtomicBool::new(false);

    pub fn fail_extent(index: usize) {
        EXTENT.store(index, Ordering::Relaxed);
    }

    pub fn fail_descriptors() {
        DESCRIPTORS.store(true, Ordering::Relaxed);
    }

    pub fn fail_revoke() {
        REVOKE.store(true, Ordering::Relaxed);
    }

    pub fn armed() -> bool {
        EXTENT.load(Ordering::Relaxed) != usize::MAX
            || DESCRIPTORS.load(Ordering::Relaxed)
            || REVOKE.load(Ordering::Relaxed)
    }

    pub(super) fn take_extent(index: usize) -> bool {
        EXTENT
            .compare_exchange(index, usize::MAX, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    }

    pub(super) fn take_descriptors() -> bool {
        DESCRIPTORS.swap(false, Ordering::Relaxed)
    }

    pub(super) fn take_revoke() -> bool {
        REVOKE.swap(false, Ordering::Relaxed)
    }
}

/// Fail the first arena revoke a reclamation attempts, once.
///
/// Task reclamation revokes through the root CNode directly rather than
/// through a kernel trait, so the conservation scenario's "the revoke did not
/// complete" case needs this one compile-time hook to exist at all.
#[cfg(slime_private_conservation)]
static ARENA_REVOKE_FAILURE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

#[cfg(slime_private_conservation)]
pub fn arm_arena_revoke_failure() {
    ARENA_REVOKE_FAILURE.store(true, core::sync::atomic::Ordering::Relaxed);
}

#[cfg(slime_private_conservation)]
pub(super) fn inject_arena_revoke_failure() -> bool {
    ARENA_REVOKE_FAILURE.swap(false, core::sync::atomic::Ordering::Relaxed)
}

/// Extents one growth may acquire in a single transaction.
///
/// Bounds the transaction record a rollback must be able to replay exactly,
/// and with it the placement list the ledger validates. A larger request is
/// refused by name rather than truncated, so a bulk holder loops over
/// transactions it can see the boundary of instead of relying on a partial
/// acquisition nobody recorded.
pub const MAX_ELASTIC_EXTENTS: usize = 64;

const LARGE_FRAME_BITS: usize = MAX_PRIVATE_EXTENT_BYTES.trailing_zeros() as usize;
const GRANULE_BITS: usize = GRANULE_BYTES.trailing_zeros() as usize;
const LARGE_FRAME_PAGES: usize = MAX_PRIVATE_EXTENT_BYTES / GRANULE_BYTES;

/// One growth's mapping shape, as the window arithmetic resolved it.
///
/// Expressed in mappings rather than pages because only the caller knows which
/// spans take an aligned large frame, which need base pages, and which still
/// lack a leaf table; the same page count has different resource costs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ElasticRequest {
    pub large_frames: usize,
    pub base_pages: usize,
    pub tables: usize,
}

/// Why a demand was refused before anything was acquired.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElasticRefusal {
    /// Page, byte or resource arithmetic left the representable range.
    Overflow,
    /// More extents than one transaction may own.
    Transaction { extents: usize, limit: usize },
    /// The named resource is short. `required` and `available` are reported in
    /// that resource's own unit, never converted into bytes.
    Exhausted {
        resource: &'static str,
        required: u64,
        available: u64,
    },
}

impl ElasticRefusal {
    pub const fn resource(self) -> &'static str {
        match self {
            Self::Overflow => "arithmetic",
            Self::Transaction { .. } => "transaction-extents",
            Self::Exhausted { resource, .. } => resource,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ElasticExtentSpec {
    kind: ExtentKind,
    size_bits: usize,
}

/// The complete, checked resource tuple one growth needs.
///
/// Computed before any effect. `resources` is what the ledger charges: bytes
/// are the extents' exact size, so payload and table bytes are counted once
/// each and no alignment residue is charged to a holder that cannot use it.
#[derive(Clone, Copy, Debug)]
pub struct ElasticDemand {
    request: ElasticRequest,
    extents: [ElasticExtentSpec; MAX_ELASTIC_EXTENTS],
    extent_len: usize,
    descriptors: usize,
    slots: usize,
    resources: ledger::Resources,
}

impl ElasticDemand {
    pub const fn resources(&self) -> ledger::Resources {
        self.resources
    }

    pub const fn extents(&self) -> usize {
        self.extent_len
    }

    pub const fn descriptors(&self) -> usize {
        self.descriptors
    }

    pub const fn slots(&self) -> usize {
        self.slots
    }

    pub const fn pages(&self) -> usize {
        self.request.large_frames * LARGE_FRAME_PAGES + self.request.base_pages
    }
}

impl ElasticRequest {
    pub const fn pages(self) -> Option<usize> {
        let Some(large) = self.large_frames.checked_mul(LARGE_FRAME_PAGES) else {
            return None;
        };
        large.checked_add(self.base_pages)
    }

    /// Resolve this request into the exact extents, descriptors and slots it
    /// needs, refusing every overflow before a single byte is taken.
    ///
    /// Large frames each take their own aligned 2 MiB extent, because a large
    /// frame is retyped whole. Base pages take an exact power-of-two
    /// decomposition capped at 2 MiB: any page count decomposes exactly, so
    /// the reserved bytes equal the payload rather than rounding every partial
    /// span up to a full one. Each new leaf table takes its own granule extent.
    pub fn demand(self) -> Result<ElasticDemand, ElasticRefusal> {
        let mut extents = [ElasticExtentSpec {
            kind: ExtentKind::PrivateData,
            size_bits: LARGE_FRAME_BITS,
        }; MAX_ELASTIC_EXTENTS];
        let mut extent_len = 0usize;
        let mut push = |kind, size_bits, extent_len: &mut usize| -> Result<(), ElasticRefusal> {
            if *extent_len >= MAX_ELASTIC_EXTENTS {
                return Err(ElasticRefusal::Transaction {
                    extents: *extent_len + 1,
                    limit: MAX_ELASTIC_EXTENTS,
                });
            }
            extents[*extent_len] = ElasticExtentSpec { kind, size_bits };
            *extent_len += 1;
            Ok(())
        };
        for _ in 0..self.large_frames {
            push(ExtentKind::PrivateData, LARGE_FRAME_BITS, &mut extent_len)?;
        }
        let mut remaining = self
            .base_pages
            .checked_mul(GRANULE_BYTES)
            .ok_or(ElasticRefusal::Overflow)?;
        while remaining >= MAX_PRIVATE_EXTENT_BYTES {
            push(ExtentKind::PrivateData, LARGE_FRAME_BITS, &mut extent_len)?;
            remaining -= MAX_PRIVATE_EXTENT_BYTES;
        }
        for bits in (GRANULE_BITS..LARGE_FRAME_BITS).rev() {
            if remaining & (1usize << bits) != 0 {
                push(ExtentKind::PrivateData, bits, &mut extent_len)?;
                remaining -= 1usize << bits;
            }
        }
        debug_assert_eq!(remaining, 0);
        for _ in 0..self.tables {
            push(ExtentKind::PrivateTables, GRANULE_BITS, &mut extent_len)?;
        }

        let pages = self.pages().ok_or(ElasticRefusal::Overflow)?;
        let objects = self
            .large_frames
            .checked_add(self.base_pages)
            .and_then(|objects| objects.checked_add(self.tables))
            .ok_or(ElasticRefusal::Overflow)?;
        let bytes = pages
            .checked_add(self.tables)
            .and_then(|units| units.checked_mul(GRANULE_BYTES))
            .ok_or(ElasticRefusal::Overflow)?;
        let slots = objects
            .checked_add(extent_len)
            .ok_or(ElasticRefusal::Overflow)?;
        let resources = ledger::Resources {
            bytes: bytes as u64,
            slots: slots as u64,
            descriptors: objects as u64,
            extents: extent_len as u64,
            tables: self.tables as u64,
        };
        Ok(ElasticDemand {
            request: self,
            extents,
            extent_len,
            descriptors: objects,
            slots,
            resources,
        })
    }
}

/// Resources one growth actually took, in the order a rollback must undo them.
///
/// Holds the live extent indices rather than capabilities: the extent record
/// is what owns the parent untyped, and returning ownership means marking that
/// record reusable after its revoke succeeded, not deleting an anchor the
/// pool still needs.
#[derive(Clone, Copy)]
pub struct ElasticAcquisition {
    arena: TaskArenaId,
    extents: [u32; MAX_ELASTIC_EXTENTS],
    placements: [ledger::Placement; MAX_ELASTIC_EXTENTS],
    sources: [ledger::Range; MAX_ELASTIC_EXTENTS],
    len: usize,
    descriptors: usize,
    resources: ledger::Resources,
}

/// Extents a failed rollback could not return, named by their arena.
///
/// Every failure adds to the arena's record rather than replacing it, so a
/// second failure cannot orphan the first.
#[derive(Clone, Copy)]
pub struct Quarantine {
    serial: u32,
    failed: bool,
    extents: [u32; MAX_ELASTIC_EXTENTS],
    len: usize,
}

impl Quarantine {
    pub const EMPTY: Self = Self {
        serial: 0,
        failed: false,
        extents: [0; MAX_ELASTIC_EXTENTS],
        len: 0,
    };
}

impl ElasticAcquisition {
    const fn empty(arena: TaskArenaId) -> Self {
        Self {
            arena,
            extents: [0; MAX_ELASTIC_EXTENTS],
            placements: [ledger::Placement {
                start: 0,
                size_bits: 0,
            }; MAX_ELASTIC_EXTENTS],
            sources: [ledger::Range {
                start: 0,
                bytes: 0,
                class: ledger::Class::OrdinaryTail,
            }; MAX_ELASTIC_EXTENTS],
            len: 0,
            descriptors: 0,
            resources: ledger::Resources::ZERO,
        }
    }

    pub const fn resources(&self) -> ledger::Resources {
        self.resources
    }

    pub const fn extents(&self) -> usize {
        self.len
    }

    pub fn placements(&self) -> &[ledger::Placement] {
        &self.placements[..self.len]
    }

    pub fn sources(&self) -> &[ledger::Range] {
        &self.sources[..self.len]
    }

    /// Certify this acquisition against the policy ledger's placement rules.
    ///
    /// Every extent must sit inside the free range it was taken from, no two
    /// may overlap, and their sizes must sum to exactly the charged bytes. The
    /// plan is built from what the allocator *did*, before any retype, so a
    /// transaction the ledger refuses is one that can still be returned whole.
    pub fn plan(&self) -> Result<ledger::Plan, ledger::Error> {
        ledger::Plan::validate(self.sources(), self.placements(), self.resources)
    }
}

impl ObjectAllocator {
    /// Ordinary capacity no live holder owns, in the ledger's resource tuple.
    ///
    /// Bytes are the admitted ordinary tails, the retained alignment prefixes
    /// and the extents earlier holders returned — three disjoint owners of the
    /// same class, counted once each. Boot exclusions are already absent from
    /// BootInfo and are deliberately not subtracted a second time. Descriptor,
    /// slot and extent counts are today's free capacity, not a ceiling: both
    /// grow from this same byte pool, which is why a caller must never add
    /// them together. Root CSlots are the same: the root CSpace grows by
    /// leaves funded from this byte pool, so today's free slots are a
    /// watermark and the leaves the pool could still fund are capacity.
    pub fn elastic_inventory(&self) -> ledger::Resources {
        let bytes = self.untyped_bytes_remaining()
            + self.preserved_bytes_remaining()
            + self.reusable_extent_bytes();
        // Tails a fragmentation scenario withholds are still owned by no
        // holder: they are unplaceable, not consumed, so funding keeps them.
        #[cfg(slime_private_fragmentation)]
        let bytes = bytes + self.physically_withheld_ordinary.unwrap_or(0) as usize;
        ledger::Resources {
            bytes: bytes as u64,
            slots: (self.free_slots() + self.fundable_slots(bytes)) as u64,
            descriptors: (self.allocation_descriptors_free() + self.fundable_descriptors()) as u64,
            extents: (self.extent_descriptors_free() + self.fundable_extents()) as u64,
            tables: (self.untyped_bytes_remaining() / GRANULE_BYTES) as u64,
        }
    }

    /// Root CSlots the expandable root CSpace could still add from `bytes`.
    ///
    /// Each leaf is one CNode of `LEAF_SLOTS` slots, retyped from ordinary
    /// memory. Like the descriptor counts below it is an alternative use of
    /// the same bytes, never an addition to them.
    fn fundable_slots(&self, bytes: usize) -> usize {
        let leaf = 1usize << crate::root_cspace::leaf_blueprint().physical_size_bits();
        (bytes / leaf).saturating_mul(crate::root_cspace::LEAF_SLOTS)
    }

    /// Allocation descriptors the metadata window could still fund.
    ///
    /// Descriptor storage is page-backed and grows on demand, so today's free
    /// count is a watermark rather than a capacity: reporting it as the
    /// inventory would refuse a policy on a machine that can fund it.
    fn fundable_descriptors(&self) -> usize {
        self.metadata_pages_available()
            * super::segmented::Segmented::<super::AllocationRecord>::entries_per_page()
    }

    fn fundable_extents(&self) -> usize {
        self.metadata_pages_available()
            * super::segmented::Segmented::<Option<super::ExtentRecord>>::entries_per_page()
    }

    /// Pages the metadata window has not yet handed to a descriptor segment.
    ///
    /// One window funds every kind of record, so these counts are alternatives
    /// rather than a sum; a caller that added them would be pricing the same
    /// pages twice.
    fn metadata_pages_available(&self) -> usize {
        super::infrastructure::METADATA_END.saturating_sub(self.metadata_next) / GRANULE_BYTES
    }

    /// Mark one arena's private backing as demand-served.
    ///
    /// Elastic arenas hold extents of mixed sizes, so they select backing
    /// best-fit rather than first-fit: a base page must not consume the
    /// aligned 2 MiB extent a large frame in the same transaction was planned
    /// to occupy. Fixed arenas keep their original selection unchanged.
    pub fn mark_arena_elastic(&mut self, id: TaskArenaId) -> Result<(), AllocError> {
        self.arena_mut(id)?.elastic = true;
        Ok(())
    }

    /// Refuse a demand the pool cannot cover, before any resource moves.
    ///
    /// Bytes and extent records are checked here because neither can be grown
    /// by this path: the byte pool is the platform's, and an extent record is
    /// storage the metadata window funds. Descriptors and root CSlots are not
    /// refused here — both grow from the same admitted ordinary memory, so
    /// their shortfall is a funding attempt during acquisition, and its
    /// failure is reported against the resource that actually ran out.
    pub fn preflight_elastic(&self, demand: &ElasticDemand) -> Result<(), ElasticRefusal> {
        let inventory = self.elastic_inventory();
        if demand.resources.bytes > inventory.bytes {
            return Err(ElasticRefusal::Exhausted {
                resource: "ordinary-bytes",
                required: demand.resources.bytes,
                available: inventory.bytes,
            });
        }
        if demand.resources.extents > inventory.extents {
            return Err(ElasticRefusal::Exhausted {
                resource: "extent-records",
                required: demand.resources.extents,
                available: inventory.extents,
            });
        }
        Ok(())
    }

    /// Take exactly the demanded extents, descriptors and slots.
    ///
    /// Failure attempts to return every acquired resource. If cleanup fails,
    /// `elastic_quarantine_resources` reports ownership the caller must charge
    /// before returning the error. Nothing here retypes a frame or touches a
    /// page table.
    pub fn acquire_elastic(
        &mut self,
        id: TaskArenaId,
        demand: &ElasticDemand,
    ) -> Result<ElasticAcquisition, AllocError> {
        self.private_arena(id)?;
        let mut acquisition = ElasticAcquisition {
            resources: demand.resources,
            ..ElasticAcquisition::empty(id)
        };
        for spec in &demand.extents[..demand.extent_len] {
            #[cfg(slime_private_rollback)]
            if inject::take_extent(acquisition.len) {
                let _ = self.release_elastic(&acquisition, false);
                return Err(AllocError::UntypedExhausted {
                    size_bits: spec.size_bits,
                    remaining: self.untyped_bytes_remaining(),
                });
            }
            match self.provision_elastic_extent(id, spec.size_bits, spec.kind) {
                Ok((index, placement, source)) => {
                    acquisition.extents[acquisition.len] = index as u32;
                    acquisition.placements[acquisition.len] = placement;
                    acquisition.sources[acquisition.len] = source;
                    acquisition.len += 1;
                }
                Err(error) => {
                    let _ = self.release_elastic(&acquisition, false);
                    return Err(error);
                }
            }
        }
        #[cfg(slime_private_rollback)]
        let injected = inject::take_descriptors();
        #[cfg(not(slime_private_rollback))]
        let injected = false;
        let provisioned = if injected {
            Err(AllocError::ArenaSlotTableFull {
                limit: demand.descriptors,
            })
        } else {
            self.provision_private_slots(id, demand.descriptors)
        };
        if let Err(error) = provisioned {
            let _ = self.release_elastic(&acquisition, false);
            return Err(error);
        }
        acquisition.descriptors = demand.descriptors;
        Ok(acquisition)
    }

    /// Provision one extent and report where it physically landed.
    ///
    /// The placement is the allocator's own record of the retype it just
    /// performed, not a prediction: a reused extent reports the base it has
    /// held since it was first taken, and a fresh one reports the base the
    /// global backing path consumed.
    fn provision_elastic_extent(
        &mut self,
        id: TaskArenaId,
        size_bits: usize,
        kind: ExtentKind,
    ) -> Result<(usize, ledger::Placement, ledger::Range), AllocError> {
        let reused_before = self.extents_reused;
        let retained_before = self.preserved_bytes_remaining();
        let index = self.provision_extent(id, size_bits, kind)?;
        let paddr = self.extents[index].expect("provisioned extent").paddr;
        // Which free owner served the extent is part of the evidence: a
        // request served from a retained alignment prefix or a returned extent
        // is one that needed no fresh aligned block from an ordinary tail.
        let class = if self.extents_reused != reused_before {
            ledger::Class::Reusable
        } else if self.preserved_bytes_remaining() != retained_before {
            ledger::Class::PreservedLeaf
        } else {
            ledger::Class::OrdinaryTail
        };
        Ok((
            index,
            ledger::Placement {
                start: paddr as u64,
                size_bits: size_bits as u8,
            },
            ledger::Range {
                start: paddr as u64,
                bytes: 1u64 << size_bits,
                class,
            },
        ))
    }

    /// Return an acquisition's still-unused resources to the common pool.
    ///
    /// `keep_mapped_tables` retains exactly those extents whose leaf table is
    /// still mapped into a span: that table cannot be handed to another span,
    /// so its bytes stay charged to this holder and are reported as retained
    /// rather than quietly dropped. Everything else is revoked, its
    /// descriptors released, and its extent record published as reusable
    /// capacity another holder may take.
    ///
    /// A failed revoke stops the return of that extent and is reported: the
    /// resources stay owned by this holder until a retry succeeds, which is
    /// what makes a quarantine retryable instead of a leak.
    pub fn release_elastic(
        &mut self,
        acquisition: &ElasticAcquisition,
        keep_mapped_tables: bool,
    ) -> Result<ledger::Resources, AllocError> {
        let id = acquisition.arena;
        let mut retained = ledger::Resources::ZERO;
        let root = sel4::init_thread::slot::CNODE.cap();
        let mut failure = None;
        for position in 0..acquisition.len {
            let index = acquisition.extents[position] as usize;
            let Some(extent) = self.extents[index] else {
                continue;
            };
            if !extent.belongs_to(id) || extent.revoked {
                continue;
            }
            if keep_mapped_tables && self.holds_mapped_leaf_table(id, index) {
                retained.bytes += 1u64 << extent.size_bits;
                retained.tables += 1;
                retained.slots += 1;
                retained.descriptors += 1;
                continue;
            }
            let parent_slot = extent.parent.bits() as usize;
            #[cfg(slime_private_rollback)]
            if inject::take_revoke() {
                failure = Some(AllocError::ArenaCleanup {
                    slot: parent_slot,
                    error: sel4::Error::IllegalOperation,
                });
                continue;
            }
            if let Err(error) = root
                .absolute_cptr(sel4::CPtr::from_bits(parent_slot as _))
                .revoke()
            {
                failure = Some(AllocError::ArenaCleanup {
                    slot: parent_slot,
                    error,
                });
                continue;
            }
            self.live_objects = self.live_objects.saturating_sub(extent.objects);
            self.live_bytes = self.live_bytes.saturating_sub(extent.bytes);
            self.release_extent_allocations(id, index);
            // Backing borrowed from a reservation returns to that reservation
            // here, and only here: the revoke above has completed, so the
            // entitlement regains capacity the machine has actually recovered.
            self.settle_returned_extent(index);
        }
        // One rebuild after every release: a reusable chain may name any of
        // the positions this call cleared, and walking a stale head is how a
        // later growth would acquire a capability the kernel has destroyed.
        self.rebuild_reusable_chains(id);
        if let Err(error) = self.drain_private_empty_records(id) {
            failure = Some(error);
        }
        match failure {
            Some(error) => {
                // Ownership stays with this holder and the extents stay named
                // in its arena's record, so a retry — or the arena's revoke at
                // retirement — can still return them.
                self.quarantine_extents(id, &acquisition.extents[..acquisition.len]);
                Err(error)
            }
            None => Ok(retained),
        }
    }

    /// Add unreturned extents to their arena's quarantine record.
    fn quarantine_extents(&mut self, id: TaskArenaId, extents: &[u32]) {
        let record = &mut self.elastic_quarantine[id.index()];
        if record.serial != id.serial {
            *record = Quarantine::EMPTY;
            record.serial = id.serial;
        }
        record.failed = true;
        for &index in extents {
            let owned = self.extents[index as usize]
                .is_some_and(|extent| extent.belongs_to(id) && !extent.revoked);
            if owned && !record.extents[..record.len].contains(&index) {
                // Bounded by the arena's live extents; an extent past the
                // record's capacity is still owned and returned by the arena
                // revoke.
                if let Some(slot) = record.extents.get_mut(record.len) {
                    *slot = index;
                    record.len += 1;
                }
            }
        }
    }

    /// Whether a rollback left resources owned but unreturned in `id`.
    pub fn elastic_quarantined(&self, id: TaskArenaId) -> bool {
        let record = &self.elastic_quarantine[id.index()];
        record.serial == id.serial && record.failed
    }

    /// Actual named ownership left by failed acquisition cleanup, not its demand.
    /// Extent parents cost one slot each; surviving object and empty records
    /// cost one descriptor and slot each. Empty records have no extent yet and
    /// must remain charged even when every extent was successfully returned.
    /// Before `begin` none of these resources authorizes mapped payload pages.
    pub fn elastic_quarantine_resources(&self, id: TaskArenaId) -> ledger::Resources {
        if !self.elastic_quarantined(id) {
            return ledger::Resources::ZERO;
        }
        let quarantine = &self.elastic_quarantine[id.index()];
        let named = &quarantine.extents[..quarantine.len];
        let mut resources = ledger::Resources::ZERO;
        for &index in named {
            if let Some(extent) = self.extents[index as usize]
                .filter(|extent| extent.belongs_to(id) && !extent.revoked)
            {
                resources.bytes += 1u64 << extent.size_bits;
                resources.extents += 1;
                resources.slots += 1;
                if extent.kind == ExtentKind::PrivateTables {
                    resources.tables += (1u64 << extent.size_bits) / GRANULE_BYTES as u64;
                }
            }
        }
        for record in self.allocations.iter() {
            if record.belongs_to(id)
                && record.allocation.is_private()
                && (record.allocation.private_kind() == PrivateObjectKind::Empty
                    || named.contains(&record.extent))
            {
                resources.descriptors += 1;
                resources.slots += 1;
            }
        }
        resources
    }

    /// Retry the return a failed cleanup could not complete in `id`.
    ///
    /// The record is consumed before the retry and whatever fails again is
    /// recorded anew, and an extent already returned is skipped because it no
    /// longer belongs to the holder.
    pub fn retry_elastic_quarantine(
        &mut self,
        id: TaskArenaId,
    ) -> Result<ledger::Resources, AllocError> {
        if !self.elastic_quarantined(id) {
            return Ok(ledger::Resources::ZERO);
        }
        let record =
            core::mem::replace(&mut self.elastic_quarantine[id.index()], Quarantine::EMPTY);
        let mut pending = ElasticAcquisition::empty(id);
        pending.extents[..record.len].copy_from_slice(&record.extents[..record.len]);
        pending.len = record.len;
        self.release_elastic(&pending, true)
    }

    /// Whether this extent still holds a leaf table bound to a live span.
    fn holds_mapped_leaf_table(&self, id: TaskArenaId, extent: usize) -> bool {
        self.allocations.iter().any(|record| {
            record.belongs_to(id)
                && record.extent as usize == extent
                && record.allocation.is_private()
                && record.allocation.private_kind() == PrivateObjectKind::LeafTable
                && record.allocation.is_mapped()
        })
    }

    /// Drop every allocation record naming a revoked extent.
    ///
    /// The revoke destroyed the children, so their CSlots are already empty:
    /// releasing them needs no delete, and leaving the records would name
    /// capabilities that no longer exist.
    fn release_extent_allocations(&mut self, id: TaskArenaId, extent: usize) {
        for index in 0..self.allocations.len() {
            let record = self.allocations[index];
            if !record.belongs_to(id) || record.extent as usize != extent {
                continue;
            }
            self.slots.release(record.allocation.slot());
            self.physical.remove(record.allocation.slot());
            self.allocations[index] = super::AllocationRecord::EMPTY;
            self.allocation_search_start = self.allocation_search_start.min(index);
            if let Ok(arena) = self.arena_mut(id) {
                arena.slot_len = arena.slot_len.saturating_sub(1);
            }
        }
    }

    /// Return every unconsumed descriptor and its CSlot to the pool.
    ///
    /// An elastic arena holds no spare descriptors between transactions: each
    /// growth provisions exactly what its plan priced and returns the rest, so
    /// an idle holder's authorized maximum costs the pool nothing.
    pub fn drain_private_empty_records(&mut self, id: TaskArenaId) -> Result<(), AllocError> {
        let bound = self.private_arena(id)?.slot_len;
        for _ in 0..bound {
            let Some(position) = self.pop_reusable(id, PrivateObjectKind::Empty, None)? else {
                return Ok(());
            };
            let slot = self.allocations[position].allocation.slot();
            self.slots.release(slot);
            self.allocations[position] = super::AllocationRecord::EMPTY;
            self.allocation_search_start = self.allocation_search_start.min(position);
            let arena = self.arena_mut(id)?;
            arena.slot_len = arena.slot_len.saturating_sub(1);
        }
        Ok(())
    }

    /// Rebuild one arena's reusable chains from its surviving records.
    fn rebuild_reusable_chains(&mut self, id: TaskArenaId) {
        let Ok(arena) = self.arena_mut(id) else {
            return;
        };
        arena.reusable_heads = [super::PRIVATE_STATE_NONE; 4];
        for index in 0..self.allocations.len() {
            let record = self.allocations[index];
            if !record.belongs_to(id)
                || !record.allocation.is_private()
                || !record.allocation.is_reusable()
                || record.allocation.is_in_flight()
            {
                continue;
            }
            let kind = record.allocation.private_kind() as usize;
            let head = self.arenas[id.index()].reusable_heads[kind];
            self.allocations[index].next_state = head;
            self.arenas[id.index()].reusable_heads[kind] = index as u32;
        }
    }

    /// Consume every admitted ordinary tail, leaving only retained prefixes
    /// and returned extents to serve the next request.
    ///
    /// Nothing is retyped, so the watermarks this returns restore exact
    /// ownership. The point is a machine whose remaining capacity is real but
    /// no longer holds one aligned 2 MiB block.
    #[cfg(slime_private_fragmentation)]
    pub fn hold_ordinary_tails(&mut self) -> [usize; super::MAX_KERNEL_UNTYPEDS] {
        let mut watermarks = [0; super::MAX_KERNEL_UNTYPEDS];
        let withheld = self.untyped_bytes_remaining() as u64;
        for (index, region) in self.untypeds[..self.untyped_len].iter_mut().enumerate() {
            if let Some(region) = region.as_mut() {
                watermarks[index] = region.watermark;
                region.watermark = region.capacity();
            }
        }
        self.physically_withheld_ordinary = Some(withheld);
        watermarks
    }

    #[cfg(slime_private_fragmentation)]
    pub fn restore_ordinary_tails(&mut self, watermarks: &[usize; super::MAX_KERNEL_UNTYPEDS]) {
        self.physically_withheld_ordinary = None;
        for (index, region) in self.untypeds[..self.untyped_len].iter_mut().enumerate() {
            if let Some(region) = region.as_mut() {
                region.watermark = watermarks[index];
            }
        }
    }

    /// Close the metadata window, so descriptor storage cannot grow.
    ///
    /// Reserving the free descriptors alone proves nothing: they are funded
    /// from admitted memory on demand, so a near-limit case has to remove the
    /// funding as well as the stock. Returns the watermark to restore.
    #[cfg(slime_private_rollback)]
    pub fn hold_metadata_window(&mut self) -> usize {
        core::mem::replace(&mut self.metadata_next, super::infrastructure::METADATA_END)
    }

    #[cfg(slime_private_rollback)]
    pub fn restore_metadata_window(&mut self, next: usize) {
        self.metadata_next = next;
    }

    /// Largest aligned block the admitted ordinary tails can still place.
    ///
    /// Reported rather than inferred: "the pool has enough bytes" and "the
    /// pool can place a 2 MiB extent" are different claims, and the second is
    /// the one a large frame depends on.
    pub fn largest_aligned_ordinary_block(&self) -> usize {
        let mut largest = 0;
        for bits in (GRANULE_BITS..=LARGE_FRAME_BITS).rev() {
            let placeable = self.untypeds[..self.untyped_len]
                .iter()
                .flatten()
                .any(|region| {
                    super::plan_allocation(region.watermark, region.capacity(), bits).is_some()
                });
            if placeable {
                largest = 1usize << bits;
                break;
            }
        }
        largest
    }

    /// One reconcilable line of elastic accounting.
    ///
    /// Payload, the backing that holds it, management storage and reusable
    /// capacity are separate quantities: a report that summed them could not
    /// distinguish a holder's live bytes from the pool's, and the point of
    /// this plane's evidence is that the two move in opposite directions by
    /// the same amount.
    pub fn report_elastic_census(&self, phase: &str) {
        let inventory = self.elastic_inventory();
        sel4::debug_println!(
            "SLIME_MEM elastic census phase={} ordinary={} retained={} reusable={} live_bytes={} extents_active={} extents_anchored={} slots_free={} descriptors_free={} extent_records_free={} metadata={}",
            phase,
            self.untyped_bytes_remaining(),
            self.preserved_bytes_remaining(),
            self.reusable_extent_bytes(),
            self.live_bytes(),
            self.active_extent_bytes(),
            self.reusable_extent_anchors(),
            self.free_slots(),
            inventory.descriptors,
            inventory.extents,
            self.infrastructure_owned_bytes(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A second failed rollback adds to its arena's record instead of
    /// replacing it, peers keep their own records, and a reused arena index
    /// never inherits a retired arena's record.
    #[test]
    fn failed_rollbacks_accumulate_per_arena() {
        extern crate std;
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(|| {
                let mut allocator = ObjectAllocator::empty();
                allocator.slots.initialize(10..64).unwrap();
                allocator.ensure_extent_descriptors(8).unwrap();
                let a = TaskArenaId::from_raw(2, 7);
                let b = TaskArenaId::from_raw(3, 7);
                let owned = |owner: TaskArenaId| {
                    let mut extent =
                        super::super::ExtentRecord::new(sel4::cap::Untyped::from_bits(1), 12);
                    extent.assign(owner.index(), owner.serial, ExtentKind::PrivateData);
                    Some(extent)
                };
                allocator.extents[0] = owned(a);
                allocator.extents[1] = owned(a);
                allocator.extents[2] = owned(b);
                allocator.quarantine_extents(a, &[0]);
                allocator.quarantine_extents(a, &[1, 0]);
                allocator.quarantine_extents(b, &[2]);
                let record = allocator.elastic_quarantine[a.index()];
                assert_eq!(&record.extents[..record.len], &[0, 1]);
                assert!(allocator.elastic_quarantined(a));
                assert!(allocator.elastic_quarantined(b));
                assert!(!allocator.elastic_quarantined(TaskArenaId::from_raw(2, 8)));
                assert_eq!(
                    allocator.elastic_quarantine_resources(a),
                    ledger::Resources {
                        bytes: 2 * GRANULE_BYTES as u64,
                        slots: 2,
                        extents: 2,
                        ..ledger::Resources::ZERO
                    }
                );
                allocator.extents[1] = None;
                assert_eq!(
                    allocator.elastic_quarantine_resources(a).bytes,
                    GRANULE_BYTES as u64
                );
                assert_eq!(allocator.elastic_quarantine_resources(b).extents, 1);
                assert_eq!(
                    allocator.elastic_quarantine_resources(TaskArenaId::from_raw(2, 8)),
                    ledger::Resources::ZERO
                );
                // Drain failure with no live extents must still mark quarantine.
                let invalid = TaskArenaId::from_raw(4, 7);
                assert!(
                    allocator
                        .release_elastic(&ElasticAcquisition::empty(invalid), false)
                        .is_err()
                );
                assert!(allocator.elastic_quarantined(invalid));
                assert_eq!(
                    allocator.elastic_quarantine_resources(invalid),
                    ledger::Resources::ZERO
                );
                // An extent that no longer belongs to the arena is not named.
                allocator.quarantine_extents(b, &[0]);
                let record = allocator.elastic_quarantine[b.index()];
                assert_eq!(&record.extents[..record.len], &[2]);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    /// The extent sizes a demand resolved to, in acquisition order.
    fn sizes<const N: usize>(demand: &ElasticDemand) -> [usize; N] {
        assert_eq!(demand.extent_len, N);
        let mut sizes = [0; N];
        for (slot, spec) in sizes.iter_mut().zip(&demand.extents[..N]) {
            *slot = spec.size_bits;
        }
        sizes
    }

    #[test]
    fn a_request_reserves_its_payload_exactly_not_a_rounded_up_span() {
        // One page must cost one page of backing plus its table, not the
        // 2 MiB span a quota-shaped reservation would take.
        let demand = ElasticRequest {
            large_frames: 0,
            base_pages: 1,
            tables: 1,
        }
        .demand()
        .unwrap();
        assert_eq!(demand.resources().bytes, 2 * GRANULE_BYTES as u64);
        assert_eq!(demand.extents(), 2);
        assert_eq!(demand.resources().tables, 1);
        assert_eq!(demand.descriptors(), 2);
        // Two extent parents and two objects.
        assert_eq!(demand.slots(), 4);
    }

    #[test]
    fn base_pages_decompose_into_exact_power_of_two_blocks() {
        // 513 pages is one full span plus one page: a decomposition that
        // rounded up would charge a second 2 MiB extent for that page.
        let demand = ElasticRequest {
            large_frames: 0,
            base_pages: 513,
            tables: 2,
        }
        .demand()
        .unwrap();
        assert_eq!(demand.resources().bytes, 515 * GRANULE_BYTES as u64);
        // One 2 MiB block, one granule block, two table granules.
        assert_eq!(demand.extents(), 4);
        assert_eq!(demand.resources().extents, 4);
        assert_eq!(sizes(&demand), [21, 12, 12, 12]);
    }

    #[test]
    fn a_mixed_request_keeps_large_frames_on_their_own_aligned_extents() {
        let demand = ElasticRequest {
            large_frames: 2,
            base_pages: 3,
            tables: 1,
        }
        .demand()
        .unwrap();
        // Two aligned large extents, then 3 pages as 2 + 1, then the table.
        assert_eq!(sizes(&demand), [21, 21, 13, 12, 12]);
        assert_eq!(demand.pages(), 2 * LARGE_FRAME_PAGES + 3);
        assert_eq!(
            demand.resources().bytes,
            (demand.pages() as u64 + 1) * GRANULE_BYTES as u64
        );
    }

    #[test]
    fn a_transaction_wider_than_the_record_is_refused_by_name() {
        let refusal = ElasticRequest {
            large_frames: MAX_ELASTIC_EXTENTS + 1,
            base_pages: 0,
            tables: 0,
        }
        .demand()
        .unwrap_err();
        assert_eq!(refusal.resource(), "transaction-extents");
        assert!(matches!(
            refusal,
            ElasticRefusal::Transaction {
                limit: MAX_ELASTIC_EXTENTS,
                ..
            }
        ));
    }

    #[test]
    fn page_arithmetic_that_cannot_be_represented_is_refused_before_any_extent() {
        assert_eq!(
            ElasticRequest {
                large_frames: usize::MAX / 2,
                base_pages: 0,
                tables: 0,
            }
            .pages(),
            None
        );
        assert_eq!(
            ElasticRequest {
                large_frames: 0,
                base_pages: usize::MAX,
                tables: 0,
            }
            .demand()
            .unwrap_err(),
            ElasticRefusal::Overflow
        );
    }

    #[test]
    fn an_empty_request_charges_nothing() {
        let demand = ElasticRequest::default().demand().unwrap();
        assert_eq!(demand.resources(), ledger::Resources::ZERO);
        assert_eq!(demand.extents(), 0);
        assert_eq!(demand.pages(), 0);
    }
}
