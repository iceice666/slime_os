//! Allocator-owned reservation identities for backing extents.
//!
//! A reservation is an allocator identity, independent of any task arena or
//! incarnation, that owns whole backing extents. While a reservation owns an
//! extent record, that record is not active and not common capacity: every
//! ordinary and elastic selection, reuse, rebuild and inventory path reads
//! [`super::ExtentRecord::is_reserved`] (directly, or through
//! `belongs_to`/`is_common_free`, which are defined in terms of it) and skips
//! it. That single predicate — not the `active` flag, whose meaning stays
//! "held by a live task arena" — is what keeps reserved backing out of reuse.
//!
//! A task arena may **borrow** a reserved extent. A borrowed extent is active
//! and owned by that arena for every allocation purpose, but it keeps its
//! origin, so the existing release paths — which clear `active` only after
//! their revoke succeeded — return it to the reservation it came from rather
//! than to common capacity. A revoke that did not complete leaves the extent
//! active and owned: it is not returned, not reusable, and not reported free.
//!
//! What this module does **not** establish. It reserves backing extents only.
//! Allocation descriptors, root CSlots, metadata pages, mapping tables and
//! capability hierarchy for a future borrow are still taken from the common
//! pool when they are used, so a reservation here is not a funded promise that
//! a later growth can be served. The names in this module are deliberately
//! neutral for that reason, and stay neutral until the whole resource tuple is
//! reserved and tested.
//!
//! Extents are the unit because an extent record is what owns a parent untyped
//! capability; reserving one removes real ordinary or preserved capacity from
//! the pool, rather than recording a byte count against it.

use boot_contracts::private_memory_policy::ledger;

use super::{AllocError, ExtentKind, ExtentSource, ObjectAllocator, TaskArenaId, global_backing};

/// Distinct backing extents one guaranteed transaction may draw from.
///
/// A run covers every page taken from one extent, so this bounds the number
/// of extents a single growth touches rather than its page count: at one span
/// per data extent a transaction may still place thousands of pages.
pub const MAX_GUARANTEED_RUNS: usize = 32;

/// What one guaranteed transaction took from a reservation.
///
/// Placements are runs read from each extent's recorded physical base and
/// watermark — never predicted from the request — so the ledger certifies
/// where the pages will actually land. `borrowed` is what this call took out
/// of the reservation's idle backing, which is what a failure must return.
#[derive(Clone, Copy)]
pub struct GuaranteedAcquisition {
    arena: TaskArenaId,
    reservation: ReservationId,
    runs: [ledger::Run; MAX_GUARANTEED_RUNS],
    sources: [ledger::Range; MAX_GUARANTEED_RUNS],
    len: usize,
    borrowed: [u32; MAX_GUARANTEED_RUNS],
    borrowed_len: usize,
    pages: usize,
    tables: usize,
    resources: ledger::Resources,
}

impl GuaranteedAcquisition {
    const EMPTY_RUN: ledger::Run = ledger::Run {
        start: 0,
        size_bits: 0,
        count: 0,
    };
    const EMPTY_RANGE: ledger::Range = ledger::Range {
        start: 0,
        bytes: 0,
        class: ledger::Class::Guaranteed,
    };

    fn new(arena: TaskArenaId, reservation: ReservationId) -> Self {
        Self {
            arena,
            reservation,
            runs: [Self::EMPTY_RUN; MAX_GUARANTEED_RUNS],
            sources: [Self::EMPTY_RANGE; MAX_GUARANTEED_RUNS],
            len: 0,
            borrowed: [0; MAX_GUARANTEED_RUNS],
            borrowed_len: 0,
            pages: 0,
            tables: 0,
            resources: ledger::Resources::ZERO,
        }
    }

    pub fn runs(&self) -> &[ledger::Run] {
        &self.runs[..self.len]
    }

    pub fn sources(&self) -> &[ledger::Range] {
        &self.sources[..self.len]
    }

    pub const fn resources(&self) -> ledger::Resources {
        self.resources
    }

    pub const fn pages(&self) -> usize {
        self.pages
    }

    pub const fn tables(&self) -> usize {
        self.tables
    }

    pub const fn arena(&self) -> TaskArenaId {
        self.arena
    }

    /// The authorization this transaction's protected half is certified with.
    pub const fn witness(&self) -> ledger::Witness {
        ledger::Witness {
            guaranteed: self.resources,
            guarantee_pages: self.pages as u64,
        }
    }

    pub const fn reservation(&self) -> ReservationId {
        self.reservation
    }
}

/// Reservation identities this allocator can hold simultaneously.
///
/// A small table of plain records kept in the allocator itself: it holds no
/// capability and no backing, so it costs `.bss` words rather than metadata
/// pages. The extents a reservation owns come from the existing extent
/// descriptor table, which the metadata window funds on demand as before.
pub const MAX_RESERVATIONS: usize = 8;

/// Extents one `reserve_backing` call may take.
///
/// Bounds the rollback record the atomicity contract below must be able to
/// replay exactly. A wider request is refused by name rather than partially
/// served, so a caller loops over calls whose boundary it can see.
pub const MAX_RESERVATION_BATCH: usize = 64;

/// Sentinel for an extent record no reservation owns.
pub(super) const RESERVATION_NONE: u32 = u32::MAX;

/// Extent size one guaranteed span reserves, for payload and tables alike.
pub const GUARANTEE_SPAN_BITS: usize = super::MAX_PRIVATE_EXTENT_BYTES.trailing_zeros() as usize;
/// Base pages one reserved span can back.
pub const GUARANTEE_SPAN_PAGES: usize = super::MAX_PRIVATE_EXTENT_PAGES;
const REQUESTS_PER_SPAN: usize = 2;

/// The requests one guaranteed span's backing resolves to.
///
/// Pure, so the shape a policy reserves can be checked without a machine.
/// Table extents are grouped first for the reason
/// [`super::PrivateBackingLayout::extent`] groups them: alternating a 2 MiB
/// and a 4 KiB retype strands almost a whole span at each boundary. Here both
/// are 2 MiB, so the order costs nothing and stays consistent.
pub fn span_requests(spans: usize, out: &mut [ReservationRequest]) -> usize {
    let mut len = 0;
    for (index, slot) in out.iter_mut().enumerate().take(spans * REQUESTS_PER_SPAN) {
        *slot = ReservationRequest {
            kind: if index < spans {
                ReservedKind::Tables
            } else {
                ReservedKind::Data
            },
            size_bits: GUARANTEE_SPAN_BITS,
        };
        len = index + 1;
    }
    len
}

/// Spans a guarantee of `pages` base pages reserves.
pub const fn spans_for_pages(pages: u64) -> u64 {
    pages.div_ceil(GUARANTEE_SPAN_PAGES as u64)
}

/// A reservation identity. The serial makes a closed reservation's identity
/// permanently unusable, even after its table position is reissued.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReservationId {
    index: u16,
    serial: u32,
}

impl ReservationId {
    pub const fn index(self) -> usize {
        self.index as usize
    }

    pub const fn serial(self) -> u32 {
        self.serial
    }
}

/// The backing kinds a reservation may hold.
///
/// Exactly the two private-backing kinds: payload data extents and leaf-table
/// extents. A reservation never owns a task's static construction extent or
/// the root's mapping-table extents, which belong to their own owners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReservedKind {
    Data,
    Tables,
}

impl ReservedKind {
    const fn extent_kind(self) -> ExtentKind {
        match self {
            Self::Data => ExtentKind::PrivateData,
            Self::Tables => ExtentKind::PrivateTables,
        }
    }
}

/// One extent a caller asks a reservation to own.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReservationRequest {
    pub kind: ReservedKind,
    pub size_bits: usize,
}

/// What a reservation currently owns, counted from the extent records
/// themselves rather than from a cached total.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReservationBacking {
    /// Everything the reservation owns, idle or borrowed.
    pub extents: usize,
    pub bytes: usize,
    pub data_extents: usize,
    pub table_extents: usize,
    /// The subset a task arena currently holds.
    pub borrowed_extents: usize,
    pub borrowed_bytes: usize,
}

impl ReservationBacking {
    /// What a further borrow may still take from this reservation.
    pub const fn available_extents(&self) -> usize {
        self.extents - self.borrowed_extents
    }

    pub const fn available_bytes(&self) -> usize {
        self.bytes - self.borrowed_bytes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReservationError {
    TableFull {
        limit: usize,
    },
    /// The identity names no live reservation: it was never opened, it was
    /// closed, or its serial belongs to an earlier reservation at the same
    /// position.
    UnknownReservation(ReservationId),
    Batch {
        requested: usize,
        limit: usize,
    },
    /// An extent size no untyped can represent, or one below seL4's minimum.
    Size {
        size_bits: usize,
    },
    /// The reservation still owns an extent a task arena holds or objects were
    /// retyped from, so closing it would drop ownership of live children.
    Occupied {
        extent: usize,
    },
    /// No idle reservation-owned extent of the requested kind and size.
    ///
    /// A refusal, never a fallback to common capacity: a borrow that silently
    /// took pool backing would report a guarantee it did not redeem.
    Unavailable {
        size_bits: usize,
    },
    Backing(AllocError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ReservationRecord {
    serial: u32,
    active: bool,
    /// Allocation descriptors and root CSlots held out of common capacity for
    /// this reservation. Counts rather than identities: both are fungible and
    /// carry no placement or alignment, so withholding a count from every
    /// funding predicate is the same ownership a named record would give,
    /// provided the storage behind it is materialized first.
    descriptors: usize,
    slots: usize,
}

impl ReservationRecord {
    const fn empty() -> Self {
        Self {
            serial: 0,
            active: false,
            descriptors: 0,
            slots: 0,
        }
    }
}

pub(super) struct ReservationTable {
    records: [ReservationRecord; MAX_RESERVATIONS],
    next_serial: u32,
    /// Totals across live reservations, withheld from every funding predicate.
    pub(super) descriptors: usize,
    pub(super) slots: usize,
}

impl ReservationTable {
    pub(super) const fn new() -> Self {
        Self {
            records: [ReservationRecord::empty(); MAX_RESERVATIONS],
            next_serial: 1,
            descriptors: 0,
            slots: 0,
        }
    }
}

impl ObjectAllocator {
    /// Open an empty reservation identity.
    ///
    /// Opening reserves no backing. Serials never repeat for a live table
    /// position, so an identity from a closed reservation cannot address the
    /// reservation that replaced it.
    pub fn open_reservation(&mut self) -> Result<ReservationId, ReservationError> {
        let index = self
            .reservations
            .records
            .iter()
            .position(|record| !record.active)
            .ok_or(ReservationError::TableFull {
                limit: MAX_RESERVATIONS,
            })?;
        let serial = self.reservations.next_serial;
        self.reservations.next_serial = serial.wrapping_add(1).max(1);
        self.reservations.records[index] = ReservationRecord {
            serial,
            active: true,
            ..ReservationRecord::empty()
        };
        Ok(ReservationId {
            index: index as u16,
            serial,
        })
    }

    /// Resolve an identity to its live table position, refusing a stale serial.
    fn reservation_index(&self, id: ReservationId) -> Result<usize, ReservationError> {
        self.reservations
            .records
            .get(id.index())
            .filter(|record| record.active && record.serial == id.serial())
            .map(|_| id.index())
            .ok_or(ReservationError::UnknownReservation(id))
    }

    /// Take `specs` as reservation-owned backing extents.
    ///
    /// Atomicity contract: **this call rolls back its own prefix.** If any
    /// extent cannot be taken, every extent this call already took is returned
    /// to common capacity before the error is reported, and the reservation
    /// owns exactly what it owned before the call. Returning means the extent
    /// record becomes a reusable anchor again, the same state an elastic
    /// release leaves behind — the parent untyped stays owned by the root and
    /// its bytes are not handed back to an ordinary watermark.
    ///
    /// Backing taken by an earlier successful call is never rolled back here.
    pub fn reserve_backing(
        &mut self,
        id: ReservationId,
        specs: &[ReservationRequest],
    ) -> Result<(), ReservationError> {
        let reservation = self.reservation_index(id)?;
        if specs.len() > MAX_RESERVATION_BATCH {
            return Err(ReservationError::Batch {
                requested: specs.len(),
                limit: MAX_RESERVATION_BATCH,
            });
        }
        for spec in specs {
            if spec.size_bits < global_backing::MINIMUM_BITS
                || spec.size_bits >= usize::BITS as usize
            {
                return Err(ReservationError::Size {
                    size_bits: spec.size_bits,
                });
            }
        }
        let mut taken = [0u32; MAX_RESERVATION_BATCH];
        let mut len = 0;
        for spec in specs {
            match self.take_reserved_extent(reservation, spec.kind.extent_kind(), spec.size_bits) {
                Ok(extent) => {
                    taken[len] = extent as u32;
                    len += 1;
                }
                Err(error) => {
                    for extent in &taken[..len] {
                        self.return_reserved_extent(*extent as usize);
                    }
                    return Err(ReservationError::Backing(error));
                }
            }
        }
        Ok(())
    }

    /// Reserve `spans` aligned spans of guaranteed backing.
    ///
    /// One 2 MiB data extent and one 2 MiB table extent per span: the data
    /// extent backs up to a span of base pages and the table extent backs up
    /// to a span's worth of leaf tables, which is the conservative one-table-
    /// per-page bound a cohort scattered across address spaces can demand.
    /// Coarse extents are deliberate — a guarantee of one 4 KiB extent per
    /// page would exhaust one transaction's extent record after 32 pages.
    ///
    /// Atomic per call, like [`Self::reserve_backing`] and by way of it: a
    /// span this call cannot reserve returns every span this call already
    /// took, and the reservation keeps exactly what it had. Backing reserved
    /// by an earlier call is not rolled back here; a caller abandoning a
    /// partly built reservation closes it.
    pub fn reserve_guarantee_spans(
        &mut self,
        id: ReservationId,
        spans: usize,
    ) -> Result<(), ReservationError> {
        self.reservation_index(id)?;
        let mut taken = 0;
        while taken < spans {
            let batch = (spans - taken).min(MAX_RESERVATION_BATCH / REQUESTS_PER_SPAN);
            let mut requests = [ReservationRequest {
                kind: ReservedKind::Data,
                size_bits: GUARANTEE_SPAN_BITS,
            }; MAX_RESERVATION_BATCH];
            let len = span_requests(batch, &mut requests);
            if let Err(error) = self.reserve_backing(id, &requests[..len]) {
                // Everything this method took across its own batches, so a
                // caller sees one boundary rather than a partly built span
                // list it did not ask for.
                if taken != 0 {
                    self.release_reserved_spans(id, taken);
                }
                return Err(error);
            }
            taken += batch;
        }
        Ok(())
    }

    /// Return `spans` spans' worth of idle backing to common capacity.
    ///
    /// Only idle extents are touched: a borrowed span stays with its holder.
    fn release_reserved_spans(&mut self, id: ReservationId, spans: usize) {
        let Ok(reservation) = self.reservation_index(id) else {
            return;
        };
        let mut returned = 0;
        for index in 0..self.extents.len() {
            if returned == spans * REQUESTS_PER_SPAN {
                break;
            }
            if self.extents[index].is_some_and(|extent| {
                extent.reserved_by(reservation) && extent.size_bits == GUARANTEE_SPAN_BITS
            }) {
                self.return_reserved_extent(index);
                returned += 1;
            }
        }
    }

    /// Acquire one extent record and mark it reservation-owned.
    fn take_reserved_extent(
        &mut self,
        reservation: usize,
        kind: ExtentKind,
        size_bits: usize,
    ) -> Result<usize, AllocError> {
        let extent = self.acquire_extent_record(size_bits)?;
        self.extents[extent]
            .as_mut()
            .expect("acquired extent record")
            .reserve(reservation, kind);
        Ok(extent)
    }

    /// Publish one reservation-owned extent record back as common capacity.
    fn return_reserved_extent(&mut self, extent: usize) {
        if let Some(record) = self.extents.get_mut(extent).and_then(Option::as_mut) {
            record.unreserve();
        }
    }

    /// Release every extent a reservation owns and retire its identity.
    ///
    /// Refused while any owned extent holds objects: dropping ownership of a
    /// parent whose children are live would leave those children unreachable
    /// and their bytes counted as free. This slice never retypes from reserved
    /// backing, so the refusal is a guard on that invariant rather than a path
    /// the product takes.
    pub fn close_reservation(&mut self, id: ReservationId) -> Result<usize, ReservationError> {
        let reservation = self.reservation_index(id)?;
        for index in 0..self.extents.len() {
            let Some(extent) = self.extents[index] else {
                continue;
            };
            if extent.owned_by_reservation(reservation)
                && (extent.is_borrowed() || extent.objects != 0)
            {
                return Err(ReservationError::Occupied { extent: index });
            }
        }
        let mut released = 0;
        for index in 0..self.extents.len() {
            if self.extents[index].is_some_and(|extent| extent.owned_by_reservation(reservation)) {
                self.return_reserved_extent(index);
                released += 1;
            }
        }
        let record = self.reservations.records[reservation];
        self.reservations.descriptors -= record.descriptors;
        self.reservations.slots -= record.slots;
        self.reservations.records[reservation] = ReservationRecord::empty();
        Ok(released)
    }

    /// What one live reservation owns right now.
    pub fn reservation_backing(
        &self,
        id: ReservationId,
    ) -> Result<ReservationBacking, ReservationError> {
        let reservation = self.reservation_index(id)?;
        let mut backing = ReservationBacking::default();
        for extent in self
            .extents
            .iter()
            .flatten()
            .filter(|extent| extent.owned_by_reservation(reservation))
        {
            let bytes = 1usize << extent.size_bits;
            backing.extents += 1;
            backing.bytes += bytes;
            match extent.kind {
                ExtentKind::PrivateTables => backing.table_extents += 1,
                _ => backing.data_extents += 1,
            }
            if extent.is_borrowed() {
                backing.borrowed_extents += 1;
                backing.borrowed_bytes += bytes;
            }
        }
        Ok(backing)
    }

    /// Borrow one idle reservation-owned extent into a task arena.
    ///
    /// The extent becomes ordinary arena-owned backing for allocation and
    /// revoke, and keeps its origin. It therefore returns to this reservation
    /// when the arena's release path revokes it, and to nowhere at all if that
    /// revoke fails. Size and kind must match exactly: a reservation funds a
    /// specific shape, and serving a 4 KiB request from a 2 MiB reserved
    /// extent would silently spend 511 pages of a guarantee.
    pub fn borrow_reserved(
        &mut self,
        id: ReservationId,
        arena: TaskArenaId,
        kind: ReservedKind,
        size_bits: usize,
    ) -> Result<usize, ReservationError> {
        let reservation = self.reservation_index(id)?;
        let serial = self.arena(arena).map_err(ReservationError::Backing)?.serial;
        let extent_kind = kind.extent_kind();
        let index = self
            .extents
            .iter()
            .position(|entry| {
                entry.is_some_and(|extent| {
                    extent.reserved_by(reservation)
                        && extent.kind == extent_kind
                        && extent.size_bits == size_bits
                        && !extent.revoked
                })
            })
            .ok_or(ReservationError::Unavailable { size_bits })?;
        self.extents[index]
            .as_mut()
            .expect("selected reserved extent")
            .assign(arena.index(), serial, extent_kind);
        Ok(index)
    }

    /// Hold allocation descriptors and root CSlots for one reservation.
    ///
    /// Materialize first, then withhold: the storage behind both is grown from
    /// admitted ordinary memory on demand, so reserving a count over capacity
    /// that does not exist yet would be the unmaterialized estimate a
    /// guarantee must not be. Growth failure refuses the reservation and
    /// changes nothing.
    ///
    /// Unlike backing extents these are counts, which is sound only because a
    /// CSlot and a descriptor record are fungible and carry no placement: any
    /// one of them satisfies any request for one. Bytes are not, which is why
    /// they are reserved as concrete extents instead.
    pub fn reserve_guarantee_resources(
        &mut self,
        id: ReservationId,
        descriptors: usize,
        slots: usize,
    ) -> Result<(), ReservationError> {
        let reservation = self.reservation_index(id)?;
        self.ensure_allocation_descriptors(descriptors)
            .map_err(ReservationError::Backing)?;
        self.ensure_root_slots(slots)
            .map_err(ReservationError::Backing)?;
        if descriptors > self.allocation_descriptors_free() || slots > self.free_slots() {
            return Err(ReservationError::Backing(AllocError::ArenaSlotTableFull {
                limit: self.allocation_descriptors_free(),
            }));
        }
        self.reservations.records[reservation].descriptors += descriptors;
        self.reservations.records[reservation].slots += slots;
        self.reservations.descriptors += descriptors;
        self.reservations.slots += slots;
        Ok(())
    }

    /// Hand one reservation's withheld descriptors and CSlots to its holder.
    ///
    /// Withholding funds a guarantee; lending is how that funding is actually
    /// spent. The counts leave the floor here so the ordinary provisioning
    /// path can take them, which is the only way a guaranteed page's frame
    /// and leaf table get a descriptor and a CSlot at all.
    pub fn lend_reserved_resources(
        &mut self,
        id: ReservationId,
        descriptors: usize,
        slots: usize,
    ) -> Result<(), ReservationError> {
        let reservation = self.reservation_index(id)?;
        let record = self.reservations.records[reservation];
        if record.descriptors < descriptors || record.slots < slots {
            return Err(ReservationError::Unavailable {
                size_bits: descriptors.max(slots),
            });
        }
        self.reservations.records[reservation].descriptors -= descriptors;
        self.reservations.records[reservation].slots -= slots;
        self.reservations.descriptors -= descriptors;
        self.reservations.slots -= slots;
        Ok(())
    }

    /// Put lent funding back, for a transaction that did not complete.
    pub fn restore_reserved_resources(
        &mut self,
        id: ReservationId,
        descriptors: usize,
        slots: usize,
    ) {
        let Ok(reservation) = self.reservation_index(id) else {
            return;
        };
        self.reservations.records[reservation].descriptors += descriptors;
        self.reservations.records[reservation].slots += slots;
        self.reservations.descriptors += descriptors;
        self.reservations.slots += slots;
    }

    /// Allocation descriptors live reservations hold out of common capacity.
    pub const fn reserved_descriptors(&self) -> usize {
        self.reservations.descriptors
    }

    /// Root CSlots live reservations hold out of common capacity.
    pub const fn reserved_slots(&self) -> usize {
        self.reservations.slots
    }

    /// Plan one guaranteed transaction's page-granular backing.
    ///
    /// Draws `pages` payload granules and `tables` leaf-table granules from
    /// extents this arena has already borrowed from `id`, borrowing one more
    /// span whenever the current ones are full. Nothing is retyped: this
    /// records where each page will land, from the extent's own recorded base
    /// and watermark, so the ledger certifies the placement rather than
    /// trusting the request.
    ///
    /// A reservation with no room left refuses. It never falls back to the
    /// common pool: a guarantee served from the pool is not a guarantee, and
    /// the refusal names the reservation so the caller reports the right
    /// cause.
    pub fn acquire_guaranteed(
        &mut self,
        id: ReservationId,
        arena: TaskArenaId,
        pages: usize,
        tables: usize,
    ) -> Result<GuaranteedAcquisition, ReservationError> {
        self.reservation_index(id)?;
        let mut acquisition = GuaranteedAcquisition::new(arena, id);
        let granule_bits = super::GRANULE_BYTES.trailing_zeros() as usize;
        for (kind, mut remaining) in [
            (ExtentKind::PrivateData, pages),
            (ExtentKind::PrivateTables, tables),
        ] {
            while remaining != 0 {
                let placement = self
                    .extent_placement_excluding(
                        arena,
                        kind,
                        granule_bits,
                        ExtentSource::Guaranteed(id),
                        &acquisition.borrowed[..acquisition.borrowed_len],
                        &acquisition.runs[..acquisition.len],
                    )
                    .filter(|(_, _, room)| *room != 0);
                let (index, paddr, room) = match placement {
                    Some(placement) => placement,
                    None => {
                        let reserved_kind = match kind {
                            ExtentKind::PrivateTables => ReservedKind::Tables,
                            _ => ReservedKind::Data,
                        };
                        match self.borrow_reserved(id, arena, reserved_kind, GUARANTEE_SPAN_BITS) {
                            Ok(extent) => {
                                if acquisition.borrowed_len >= MAX_GUARANTEED_RUNS {
                                    self.release_guaranteed(&acquisition);
                                    return Err(ReservationError::Batch {
                                        requested: acquisition.borrowed_len + 1,
                                        limit: MAX_GUARANTEED_RUNS,
                                    });
                                }
                                acquisition.borrowed[acquisition.borrowed_len] = extent as u32;
                                acquisition.borrowed_len += 1;
                                continue;
                            }
                            Err(error) => {
                                self.release_guaranteed(&acquisition);
                                return Err(error);
                            }
                        }
                    }
                };
                if acquisition.len >= MAX_GUARANTEED_RUNS {
                    self.release_guaranteed(&acquisition);
                    return Err(ReservationError::Batch {
                        requested: acquisition.len + 1,
                        limit: MAX_GUARANTEED_RUNS,
                    });
                }
                let take = room.min(remaining);
                let extent = self.extents[index].expect("selected guaranteed extent");
                acquisition.runs[acquisition.len] = ledger::Run {
                    start: paddr as u64,
                    size_bits: granule_bits as u8,
                    count: take as u64,
                };
                acquisition.sources[acquisition.len] = ledger::Range {
                    start: extent.paddr as u64,
                    bytes: 1u64 << extent.size_bits,
                    class: ledger::Class::Guaranteed,
                };
                acquisition.len += 1;
                remaining -= take;
                if kind == ExtentKind::PrivateTables {
                    acquisition.tables += take;
                } else {
                    acquisition.pages += take;
                }
            }
        }
        let objects = (pages + tables) as u64;
        acquisition.resources = ledger::Resources {
            bytes: objects * super::GRANULE_BYTES as u64,
            slots: objects,
            descriptors: objects,
            // The extents were charged to this entitlement when the
            // reservation took them; a page retyped inside one adds none.
            extents: 0,
            tables: tables as u64,
        };
        Ok(acquisition)
    }

    /// Return a guaranteed transaction's freshly borrowed extents.
    ///
    /// Only the extents this call borrowed, and only while nothing has been
    /// retyped from them: an extent the arena already held keeps its pages.
    pub fn release_guaranteed(&mut self, acquisition: &GuaranteedAcquisition) {
        for index in 0..acquisition.borrowed_len {
            let extent = acquisition.borrowed[index] as usize;
            if self.extents[extent].is_some_and(|record| record.objects == 0) {
                self.settle_returned_extent(extent);
            }
        }
    }

    /// [`Self::extent_placement`], skipping extents this transaction has
    /// already planned against, so a second run does not restate the first
    /// one's room.
    fn extent_placement_excluding(
        &self,
        id: TaskArenaId,
        kind: ExtentKind,
        size_bits: usize,
        source: ExtentSource,
        borrowed: &[u32],
        planned: &[ledger::Run],
    ) -> Option<(usize, usize, usize)> {
        let _ = borrowed;
        let mut best: Option<(usize, usize, usize)> = None;
        for (index, extent) in self
            .extents
            .iter()
            .enumerate()
            .filter_map(|(index, extent)| extent.as_ref().map(|extent| (index, extent)))
        {
            if !extent.belongs_to(id)
                || extent.kind != kind
                || extent.revoked
                || !self.extent_matches_source(index, source)
            {
                continue;
            }
            let size = 1usize << size_bits;
            let Some(start) = extent.watermark.checked_next_multiple_of(size) else {
                continue;
            };
            let capacity = 1usize << extent.size_bits;
            let Some(room) = capacity.checked_sub(start).map(|room| room / size) else {
                continue;
            };
            let base = extent.paddr + start;
            // An extent a previous run in this same transaction already drew
            // from is exhausted for planning purposes: its watermark has not
            // moved yet, and counting its room twice would place two runs at
            // one address.
            if room == 0 || planned.iter().any(|run| run.start == base as u64) {
                continue;
            }
            if best.is_none_or(|(_, _, best_room)| room < best_room) {
                best = Some((index, base, room));
            }
        }
        best
    }

    /// Bytes every live reservation has lent to a task arena.
    pub fn borrowed_extent_bytes(&self) -> usize {
        self.extents
            .iter()
            .flatten()
            .filter(|extent| extent.is_borrowed())
            .map(|extent| 1usize << extent.size_bits)
            .sum()
    }

    /// Bytes every live reservation holds out of common capacity.
    ///
    /// Reported on its own line rather than folded into the active or reusable
    /// totals: reserved backing is neither held by a live task arena nor
    /// available to the next request, and counting it in either would misstate
    /// one of them.
    pub fn reserved_extent_bytes(&self) -> usize {
        self.extents
            .iter()
            .flatten()
            .filter(|extent| extent.is_reserved())
            .map(|extent| 1usize << extent.size_bits)
            .sum()
    }

    /// Extent anchors every live reservation holds out of common capacity.
    pub fn reserved_extent_anchors(&self) -> usize {
        self.extents
            .iter()
            .flatten()
            .filter(|extent| extent.is_reserved())
            .count()
    }

    /// Live reservation identities.
    pub fn live_reservations(&self) -> usize {
        self.reservations
            .records
            .iter()
            .filter(|record| record.active)
            .count()
    }
}
