//! Demand-backed growth of a private region, settled against the policy ledger.
//!
//! The fixed path in the parent module reserves a holder's whole quota before
//! the task is published. This one reserves nothing but what a request needs,
//! in the order that keeps a refusal harmless: resolve the mapping shape, price
//! the complete resource tuple, take it from the common pool, let the ledger
//! judge the placements the allocator actually obtained, and only then retype
//! and map. Every step before the first mapping is reversible, so a policy
//! refusal, a fragmented pool or a failed kernel operation all leave the
//! holder's existing pages, contents and guarantees exactly as they were.
//!
//! Which resources a page costs depends on where it lands, not on how many
//! pages were asked for: an aligned run with no leaf table takes a large frame,
//! a span that already has a table takes base pages, and a new span takes a
//! table first. [`Shape`] resolves that once, so the price the ledger charges
//! and the mappings the kernel performs come from the same decision.

use boot_contracts::private_memory_policy::ledger::{self, Incarnation, Ledger};

use super::{
    GrowError, GrowthPlan, Lane, NativePrivateMemoryKernel, PrivateMemoryKernel, Region, Table,
    map_growth,
};
use crate::child_vspace::{LARGE_FRAME_BYTES, LARGE_FRAME_PAGES};
use crate::object_allocator::elastic::{ElasticRefusal, ElasticRequest, MAX_ELASTIC_EXTENTS};
use crate::object_allocator::guarantee_vault::{GuaranteedAcquisition, ReservationError};
use crate::object_allocator::{AllocError, ObjectAllocator, TaskArenaId};

/// Why a demand-backed growth was refused, in the stage it was refused at.
///
/// A caller must be able to tell a policy answer from a resource answer: the
/// first will not change until the policy does, the second may change as soon
/// as another holder returns capacity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElasticGrowError {
    /// The growth would leave the reserved window. The base cannot move.
    Window {
        pages: usize,
        delta: usize,
        reservation: usize,
    },
    /// The pool cannot cover the request's complete resource tuple.
    Demand(ElasticRefusal),
    /// Acquisition failed; nothing this attempt took is still held.
    Acquire(AllocError),
    /// The ledger refused the transaction: authorization, not availability.
    Policy(ledger::Error),
    /// Retype or mapping failed after acquisition. `allocated` pages of this
    /// attempt were mapped and then unwound; previously committed pages,
    /// their contents and every guarantee are untouched.
    Frames { allocated: usize, error: AllocError },
    /// Cleanup after a failure did not complete. The holder keeps ownership of
    /// the resources still named by the transaction until a retry succeeds.
    Quarantined { error: AllocError },
    /// The entitlement's own reservation cannot back this growth. Never
    /// served from the common pool instead: a guaranteed page funded by the
    /// pool is not guaranteed.
    Reservation { error: ReservationError },
}

impl ElasticGrowError {
    /// The single resource or authority a refusal should be reported against.
    pub const fn cause(self) -> &'static str {
        match self {
            Self::Window { .. } => "reservation",
            Self::Demand(refusal) => refusal.resource(),
            Self::Acquire(_) => "acquisition",
            Self::Policy(_) => "policy",
            Self::Frames { .. } => "mapping",
            Self::Quarantined { .. } => "cleanup",
            Self::Reservation { .. } => "reservation",
        }
    }

    /// The resource that refused, with its shortfall in that resource's unit.
    ///
    /// `(resource, required, available)`. A refusal the allocator priced
    /// reports its own numbers; any other refusal names its cause with zero
    /// quantities, because it was not a count running out.
    pub const fn limit(self) -> (&'static str, u64, u64) {
        match self {
            Self::Demand(ElasticRefusal::Exhausted {
                resource,
                required,
                available,
            }) => (resource, required, available),
            Self::Demand(ElasticRefusal::Transaction { extents, limit }) => {
                ("transaction-extents", extents as u64, limit as u64)
            }
            Self::Policy(ledger::Error::Unavailable) => ("ledger-pool", 0, 0),
            Self::Policy(ledger::Error::Maximum) => ("maximum", 0, 0),
            Self::Acquire(error) => (error.resource(), 0, 0),
            other => (other.cause(), 0, 0),
        }
    }
}

/// How one growth's pages resolve into mappings.
///
/// Pure, and computed from the region's current state rather than from a
/// request size alone: the same 512-page growth costs one large frame in a
/// fresh aligned span and 512 base pages plus a table in a span already
/// holding one.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Shape {
    pub large_frames: usize,
    pub base_pages: usize,
    pub tables: usize,
}

impl Shape {
    pub const fn pages(self) -> usize {
        self.large_frames * LARGE_FRAME_PAGES + self.base_pages
    }

    const fn request(self) -> ElasticRequest {
        ElasticRequest {
            large_frames: self.large_frames,
            base_pages: self.base_pages,
            tables: self.tables,
        }
    }
}

impl Region {
    /// Resolve the mappings `delta` further pages take, in window order.
    ///
    /// Mirrors the growth loop exactly, including that a large frame is taken
    /// only where no leaf table already occupies the span: once a span holds a
    /// table, every page in it must be a base page, and a span crossed by an
    /// earlier large frame needs no table at all.
    ///
    /// Prices the elastic lane. A guaranteed growth prices the same pages with
    /// [`Region::shape_in`] and [`Lane::BasePage`].
    pub fn shape(self, delta: usize) -> Shape {
        self.shape_in(delta, Lane::LargeFrame)
    }

    /// Resolve the mappings `delta` further pages take in one explicit lane.
    ///
    /// The lane resolved here is the lane the mapper is given, so a price and
    /// a mapping cannot disagree about whether a span took a large frame.
    pub fn shape_in(self, delta: usize, lane: Lane) -> Shape {
        self.shape_split(delta, lane, 0).0
    }

    /// [`Region::shape_in`], also counting the leaf tables opened by pages
    /// before offset `prefix`.
    ///
    /// The mapper takes a page's leaf table from that page's own source, so a
    /// guaranteed prefix owns exactly the tables its pages open and the pooled
    /// remainder owns the rest. Charging every table to either half leaves the
    /// other asking a source for a table it never acquired.
    pub fn shape_split(self, delta: usize, lane: Lane, prefix: usize) -> (Shape, usize) {
        let mut shape = Shape::default();
        let mut prefix_tables = 0;
        let previous = self.pages();
        let mut page = previous;
        let end = previous.saturating_add(delta);
        let mut span = previous / LARGE_FRAME_PAGES;
        let mut leaf_available = self.has_leaf_table(span);
        while page < end {
            if page != previous && page.is_multiple_of(LARGE_FRAME_PAGES) {
                span = page / LARGE_FRAME_PAGES;
                leaf_available = self.has_leaf_table(span);
            }
            let vaddr = self.base() + page * super::GRANULE_SIZE;
            let remaining = end - page;
            if lane.allows_large_frames()
                && !leaf_available
                && vaddr.is_multiple_of(LARGE_FRAME_BYTES)
                && remaining >= LARGE_FRAME_PAGES
            {
                shape.large_frames += 1;
                page += LARGE_FRAME_PAGES;
                continue;
            }
            if !leaf_available {
                shape.tables += 1;
                if page - previous < prefix {
                    prefix_tables += 1;
                }
                leaf_available = true;
            }
            shape.base_pages += 1;
            page += 1;
        }
        (shape, prefix_tables)
    }
}

/// Grow `region` by `delta` pages against the common pool.
///
/// Answers the page count *before* the growth, exactly as the fixed path does.
/// The ledger is the authority on whether this holder may hold the pages; the
/// allocator is the authority on whether the machine can back them. Both must
/// agree before a single frame is retyped, and a success is returned only
/// after every page has real backing.
#[allow(clippy::too_many_arguments)]
pub(crate) fn grow<K: PrivateMemoryKernel>(
    table: &mut Table,
    allocator: &mut ObjectAllocator,
    ledger: &mut Ledger<'_>,
    token: Incarnation,
    arena: TaskArenaId,
    vspace: sel4::cap::VSpace,
    region: &mut Region,
    delta: usize,
    growth: GrowthPlan,
    kernel: &mut K,
) -> Result<usize, ElasticGrowError> {
    let previous = region.pages();
    let requested = previous
        .checked_add(delta)
        .ok_or(ElasticGrowError::Window {
            pages: previous,
            delta,
            reservation: region.reservation(),
        })?;
    if requested > region.reservation() {
        return Err(ElasticGrowError::Window {
            pages: previous,
            delta,
            reservation: region.reservation(),
        });
    }
    if delta == 0 {
        return Ok(previous);
    }

    // One resolution, consumed three times: the demand below prices exactly
    // this shape, the ledger judges exactly that demand's placements, and the
    // mapper below is given the same plan that produced it.
    // The guaranteed prefix is served from the entitlement's own reservation
    // and the remainder from the pool, in one transaction the ledger settles
    // as a whole. Tables follow their pages: a guaranteed page's leaf table
    // is guaranteed too, because a table the pool could refuse would make the
    // page it maps unreachable, and a pooled page's table is pooled.
    let (shape, _) = region.shape_split(delta, growth.lane, 0);
    let guaranteed_pages = growth.guaranteed_pages.min(shape.base_pages);
    let (_, guaranteed_tables) = region.shape_split(delta, growth.lane, guaranteed_pages);
    let guaranteed = match growth.reservation {
        Some(id) if guaranteed_pages != 0 => {
            match allocator.acquire_guaranteed(id, arena, guaranteed_pages, guaranteed_tables) {
                Ok(acquired) => Some(acquired),
                Err(error) => return Err(ElasticGrowError::Reservation { error }),
            }
        }
        _ => None,
    };
    let pooled = Shape {
        large_frames: shape.large_frames,
        base_pages: shape.base_pages - guaranteed_pages,
        tables: shape.tables - guaranteed_tables,
    };
    let demand = pooled.request().demand().map_err(|error| {
        settle_guaranteed(allocator, guaranteed.as_ref());
        ElasticGrowError::Demand(error)
    })?;
    if let Err(error) = allocator.preflight_elastic(&demand) {
        settle_guaranteed(allocator, guaranteed.as_ref());
        return Err(ElasticGrowError::Demand(error));
    }
    let acquisition = match allocator.acquire_elastic(arena, &demand) {
        Ok(acquisition) => acquisition,
        Err(error) => {
            settle_guaranteed(allocator, guaranteed.as_ref());
            return Err(ElasticGrowError::Acquire(error));
        }
    };
    // The guaranteed half needs a descriptor and a CSlot for every frame and
    // leaf table it is about to retype, exactly as the pooled half does. Its
    // funding comes from the entitlement's own withheld counts rather than
    // from the pool, so a guaranteed page stays fundable after the pool is
    // exhausted — which is the whole promise.
    let lent = guaranteed
        .as_ref()
        .map_or(0, |acquired| acquired.pages() + acquired.tables());
    if let Some(id) = growth.reservation.filter(|_| lent != 0)
        && let Err(error) = allocator
            .lend_reserved_resources(id, lent, lent)
            .and_then(|()| {
                allocator
                    .provision_private_slots(arena, lent)
                    .map_err(ReservationError::Backing)
            })
    {
        allocator.restore_reserved_resources(id, lent, lent);
        let _ = allocator.release_elastic(&acquisition, false);
        settle_guaranteed(allocator, guaranteed.as_ref());
        return Err(ElasticGrowError::Reservation { error });
    }

    // The plan is built from the placements acquisition actually obtained, so
    // the ledger judges physical fit rather than a byte sum. Both refusals
    // here precede every retype, and both return the whole acquisition.
    let plan = match certify(&acquisition, guaranteed.as_ref()) {
        Ok(plan) => plan,
        Err(error) => {
            unwind_guaranteed(allocator, growth.reservation, guaranteed.as_ref(), lent);
            return Err(settle_refusal(allocator, &acquisition, error));
        }
    };
    if let Err(error) = ledger.begin(token, delta as u64, plan) {
        unwind_guaranteed(allocator, growth.reservation, guaranteed.as_ref(), lent);
        return Err(settle_refusal(allocator, &acquisition, error));
    }

    let growth = GrowthPlan {
        guaranteed_pages,
        ..growth
    };
    match map_growth(allocator, arena, vspace, region, delta, growth, kernel) {
        Ok(outcome) => {
            if let Err(error) = allocator.commit_private_transaction(arena) {
                let failure = settle_failure(
                    allocator,
                    ledger,
                    token,
                    &acquisition,
                    ElasticGrowError::Frames {
                        allocated: outcome.pages_backed,
                        error,
                    },
                );
                return Err(settle_lent(
                    allocator,
                    growth.reservation,
                    guaranteed.as_ref(),
                    lent,
                    failure,
                ));
            }
            if let Err(error) = ledger.commit(token) {
                return Err(ElasticGrowError::Policy(error));
            }
            // Descriptors the plan priced but the mapping did not consume are
            // returned immediately: an idle holder must not keep management
            // storage another holder could use.
            if let Err(error) = allocator.drain_private_empty_records(arena) {
                return Err(ElasticGrowError::Quarantined { error });
            }
            table.record_growth(region, delta, outcome.large_frames, outcome.base_frames);
            Ok(previous)
        }
        Err(other) => {
            let failure = match other {
                GrowError::Frames { allocated, error } => {
                    ElasticGrowError::Frames { allocated, error }
                }
                _ => ElasticGrowError::Frames {
                    allocated: 0,
                    error: AllocError::NoKernelUntyped,
                },
            };
            let failure = settle_failure(allocator, ledger, token, &acquisition, failure);
            Err(settle_lent(
                allocator,
                growth.reservation,
                guaranteed.as_ref(),
                lent,
                failure,
            ))
        }
    }
}

/// Grow through the real kernel, for callers outside this module.
#[allow(clippy::too_many_arguments)]
pub fn grow_native(
    table: &mut Table,
    allocator: &mut ObjectAllocator,
    ledger: &mut Ledger<'_>,
    token: Incarnation,
    arena: TaskArenaId,
    vspace: sel4::cap::VSpace,
    region: &mut Region,
    delta: usize,
    growth: GrowthPlan,
) -> Result<usize, ElasticGrowError> {
    let mut kernel = NativePrivateMemoryKernel::for_growth(region, delta);
    grow(
        table,
        allocator,
        ledger,
        token,
        arena,
        vspace,
        region,
        delta,
        growth,
        &mut kernel,
    )
}

/// Certify one transaction's complete placement, protected half and all.
///
/// Both halves are expressed as runs: a pooled extent is one block, and a
/// guaranteed run is the consecutive pages taken from one borrowed extent at
/// its recorded base and watermark. The witness states what the reservation
/// funded, and certification refuses unless exactly that much landed in
/// protected ranges.
fn certify(
    acquisition: &crate::object_allocator::elastic::ElasticAcquisition,
    guaranteed: Option<&GuaranteedAcquisition>,
) -> Result<ledger::Plan, ledger::Error> {
    let Some(guaranteed) = guaranteed else {
        return acquisition.plan();
    };
    let mut sources = [ledger::Range {
        start: 0,
        bytes: 0,
        class: ledger::Class::OrdinaryTail,
    }; MAX_TRANSACTION_RUNS];
    let mut runs = [ledger::Run {
        start: 0,
        size_bits: 0,
        count: 0,
    }; MAX_TRANSACTION_RUNS];
    let mut len = 0;
    let mut push = |source: ledger::Range, run: ledger::Run| -> Result<(), ledger::Error> {
        if len >= MAX_TRANSACTION_RUNS {
            return Err(ledger::Error::Placement);
        }
        sources[len] = source;
        runs[len] = run;
        len += 1;
        Ok(())
    };
    for (source, run) in guaranteed.sources().iter().zip(guaranteed.runs()) {
        push(*source, *run)?;
    }
    for (source, placement) in acquisition.sources().iter().zip(acquisition.placements()) {
        push(
            *source,
            ledger::Run {
                start: placement.start,
                size_bits: placement.size_bits,
                count: 1,
            },
        )?;
    }
    let resources = guaranteed
        .resources()
        .checked_add(acquisition.resources())?;
    ledger::Plan::validate_runs(
        &sources[..len],
        &runs[..len],
        resources,
        guaranteed.witness(),
    )
}

/// Runs one transaction may certify: the pooled extents it acquired plus the
/// borrowed extents its guaranteed half draws from.
const MAX_TRANSACTION_RUNS: usize =
    MAX_ELASTIC_EXTENTS + crate::object_allocator::guarantee_vault::MAX_GUARANTEED_RUNS;

/// Return a guaranteed half whose transaction never reached the ledger.
fn settle_guaranteed(allocator: &mut ObjectAllocator, guaranteed: Option<&GuaranteedAcquisition>) {
    if let Some(guaranteed) = guaranteed {
        allocator.release_guaranteed(guaranteed);
    }
}

/// Return both halves of a guaranteed attempt: its borrowed extents and the
/// funding it drew from its entitlement.
fn unwind_guaranteed(
    allocator: &mut ObjectAllocator,
    reservation: Option<crate::object_allocator::guarantee_vault::ReservationId>,
    guaranteed: Option<&GuaranteedAcquisition>,
    lent: usize,
) {
    if let Some(id) = reservation
        && lent != 0
    {
        let _ = allocator.drain_private_empty_records(guaranteed.map_or_else(
            || unreachable!("a lend implies a guaranteed acquisition"),
            GuaranteedAcquisition::arena,
        ));
        allocator.restore_reserved_resources(id, lent, lent);
    }
    settle_guaranteed(allocator, guaranteed);
}

/// Return a failed guaranteed growth's lent funding once the ledger has settled.
///
/// A completed abort charged the entitlement nothing, so the lend returns here;
/// retirement restores only what the ledger holds. A quarantined or unsettled
/// abort still holds the guaranteed charge, and returning the lend as well would
/// restore it twice when that incarnation retires.
fn settle_lent(
    allocator: &mut ObjectAllocator,
    reservation: Option<crate::object_allocator::guarantee_vault::ReservationId>,
    guaranteed: Option<&GuaranteedAcquisition>,
    lent: usize,
    failure: ElasticGrowError,
) -> ElasticGrowError {
    if matches!(failure, ElasticGrowError::Frames { .. }) {
        unwind_guaranteed(allocator, reservation, guaranteed, lent);
    }
    failure
}

/// Return an acquisition the ledger refused, before any mapping existed.
///
/// No transaction was opened, so nothing is settled: the holder is exactly
/// where it was, and the pool has every byte back.
fn settle_refusal(
    allocator: &mut ObjectAllocator,
    acquisition: &crate::object_allocator::elastic::ElasticAcquisition,
    error: ledger::Error,
) -> ElasticGrowError {
    match allocator.release_elastic(acquisition, false) {
        Ok(_) => ElasticGrowError::Policy(error),
        Err(error) => ElasticGrowError::Quarantined { error },
    }
}

/// Settle a failed growth: return what can be returned, keep what cannot.
///
/// A successful cleanup retains only the leaf tables already bound to a span —
/// they name one fixed address and may never be handed to another span — and
/// the ledger keeps exactly those charged to this holder. A failed cleanup
/// quarantines the member instead: its resources stay owned and its next
/// request is refused until a retry completes, so nothing is charged twice and
/// nothing is advertised as free while it is still held.
fn settle_failure(
    allocator: &mut ObjectAllocator,
    ledger: &mut Ledger<'_>,
    token: Incarnation,
    acquisition: &crate::object_allocator::elastic::ElasticAcquisition,
    failure: ElasticGrowError,
) -> ElasticGrowError {
    let released = allocator.release_elastic(acquisition, true);
    settle_abort(ledger, token, released, failure)
}

/// Close a failed growth's ledger transaction whatever its cleanup reported.
///
/// The transaction never stays pending: a pending transaction refuses every
/// later growth of every subject. A settlement the ledger refuses, like a
/// cleanup that did not complete, closes as a quarantine that keeps the whole
/// charge until the holder retires.
fn settle_abort(
    ledger: &mut Ledger<'_>,
    token: Incarnation,
    released: Result<ledger::Resources, AllocError>,
    failure: ElasticGrowError,
) -> ElasticGrowError {
    match released {
        Ok(retained) => match ledger.abort(token, retained, true) {
            Ok(()) => failure,
            Err(error) => {
                let _ = ledger.abort(token, ledger::Resources::ZERO, false);
                ElasticGrowError::Policy(error)
            }
        },
        Err(error) => {
            let _ = ledger.abort(token, ledger::Resources::ZERO, false);
            ElasticGrowError::Quarantined { error }
        }
    }
}

/// Return one dying elastic holder's capacity to the pool.
///
/// The arena revoke destroys the frames; this returns the charge and the
/// entitlement. A revoke that did not complete keeps both: the ledger
/// quarantines the member rather than advertising capacity the machine has
/// not actually recovered.
pub fn retire(
    table: &mut Table,
    allocator: &mut ObjectAllocator,
    ledger: &mut Ledger<'_>,
    token: Incarnation,
    region: &mut Region,
    revoked: bool,
) -> Result<usize, ledger::Error> {
    // Policy ownership is released first. In particular, `revoked = false`
    // quarantines the incarnation and returns before either the aggregate page
    // charge or the region's span record is advertised as free. Once the arena
    // revoke has succeeded, ledger retirement preflights all refund arithmetic
    // before mutating its member, so reclamation below cannot strand a retired
    // incarnation with live region accounting.
    retire_after_policy(ledger.retire(token, revoked), || {
        table.reclaim(allocator, region)
    })
}

fn retire_after_policy(
    policy: Result<(), ledger::Error>,
    reclaim: impl FnOnce() -> usize,
) -> Result<usize, ledger::Error> {
    policy?;
    Ok(reclaim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::child_vspace::GRANULE_SIZE;

    /// A window of the compiled per-region capacity, for shape tests that are
    /// about mapping selection rather than about how wide a window may be.
    fn region(base: usize) -> Region {
        region_of(base, super::super::MAX_REGION_PAGES)
    }

    fn region_of(base: usize, reservation: usize) -> Region {
        Region::reserve_for_test(base, reservation, reservation, true).expect("host test window")
    }

    /// A settlement the ledger refuses still closes the transaction, as a
    /// quarantine: other subjects keep growing and the holder can retire.
    #[test]
    fn a_refused_settlement_never_leaves_a_transaction_pending() {
        use boot_contracts::private_memory_policy::{
            self as policy, Entitlement, Header, Instance, Subject,
        };
        let entitlement = Entitlement {
            identity: policy::entitlement_identity("shared"),
            subtree_root: [0; 32],
            guarantee_pages: 0,
            maximum_pages: 0,
            maximum_mode: policy::POOL,
            reserved: 0,
        };
        let mut subjects =
            [policy::subject_identity("a"), policy::subject_identity("b")].map(|identity| {
                Subject {
                    identity,
                    entitlement: entitlement.identity,
                    maximum_pages: 1,
                    maximum_mode: policy::FIXED,
                    reserved: 0,
                }
            });
        subjects.sort_by_key(|subject| subject.identity);
        let header = Header {
            magic: policy::MAGIC,
            format_version: policy::FORMAT_VERSION,
            header_size: policy::HEADER_BYTES as u32,
            required_flags: 0,
            entitlement_count: 1,
            subject_count: 2,
            total_len: (policy::HEADER_BYTES
                + policy::ENTITLEMENT_BYTES
                + 2 * policy::SUBJECT_BYTES) as u32,
            reserved: 0,
            reserve_bytes: 0,
            reserve_slots: 0,
            reserve_descriptors: 0,
            reserve_extents: 0,
            reserve_tables: 0,
        };
        let mut bytes = header.encode().to_vec();
        bytes.extend(entitlement.encode());
        for subject in subjects {
            bytes.extend(subject.encode());
        }
        let decoded = policy::Policy::decode(&bytes).unwrap();
        let instances = subjects.map(|subject| Instance {
            identity: subject.identity,
            owner: None,
        });
        let page = ledger::Resources {
            bytes: policy::PAGE_BYTES,
            slots: 1,
            descriptors: 1,
            extents: 1,
            tables: 1,
        };
        let plan = || {
            ledger::Plan::validate(
                &[ledger::Range {
                    start: 0,
                    bytes: policy::PAGE_BYTES,
                    class: ledger::Class::OrdinaryTail,
                }],
                &[ledger::Placement {
                    start: 0,
                    size_bits: 12,
                }],
                page,
            )
            .unwrap()
        };
        let pool = ledger::Resources {
            bytes: 16 * policy::PAGE_BYTES,
            slots: 16,
            descriptors: 16,
            extents: 16,
            tables: 16,
        };
        let mut ledger =
            Ledger::admit(decoded, &instances, pool, &[ledger::Resources::ZERO]).unwrap();
        let a = ledger.bind(&policy::subject_identity("a")).unwrap();
        let b = ledger.bind(&policy::subject_identity("b")).unwrap();
        let failure = ElasticGrowError::Frames {
            allocated: 0,
            error: AllocError::NoKernelUntyped,
        };
        // A one-page window has one span: retaining two tables is refused.
        let two_tables = ledger::Resources {
            bytes: 2 * policy::PAGE_BYTES,
            slots: 2,
            descriptors: 2,
            extents: 0,
            tables: 2,
        };
        ledger.begin(a, 1, plan()).unwrap();
        assert!(matches!(
            settle_abort(&mut ledger, a, Ok(two_tables), failure),
            ElasticGrowError::Policy(ledger::Error::Cleanup)
        ));
        // Nothing is pending: a peer grows, and the holder stays quarantined.
        ledger.begin(b, 1, plan()).unwrap();
        ledger.commit(b).unwrap();
        assert_eq!(ledger.begin(a, 1, plan()), Err(ledger::Error::Cleanup));
        // A cleanup that did not complete closes the same way.
        ledger.retire(b, true).unwrap();
        let b = ledger.bind(&policy::subject_identity("b")).unwrap();
        ledger.begin(b, 1, plan()).unwrap();
        assert!(matches!(
            settle_abort(&mut ledger, b, Err(AllocError::NoKernelUntyped), failure),
            ElasticGrowError::Quarantined { .. }
        ));
        assert_eq!(ledger.begin(b, 1, plan()), Err(ledger::Error::Cleanup));
    }

    #[test]
    fn retirement_refusal_never_runs_accounting_reclamation() {
        for error in [
            ledger::Error::Cleanup,
            ledger::Error::Transaction,
            ledger::Error::Incarnation,
        ] {
            let mut reclaimed = false;
            assert_eq!(
                retire_after_policy(Err(error), || {
                    reclaimed = true;
                    7
                }),
                Err(error)
            );
            assert!(!reclaimed);
        }
        assert_eq!(retire_after_policy(Ok(()), || 7), Ok(7));
    }

    #[test]
    fn a_fresh_aligned_window_takes_large_frames_and_no_tables() {
        let shape = region(1 << 36).shape(LARGE_FRAME_PAGES);
        assert_eq!(
            shape,
            Shape {
                large_frames: 1,
                base_pages: 0,
                tables: 0,
            }
        );
        assert_eq!(shape.pages(), LARGE_FRAME_PAGES);
    }

    #[test]
    fn a_partial_span_takes_one_table_and_base_pages_only() {
        // The page a caller actually asked for must not drag a 2 MiB frame in
        // behind it: a one-page request is one page, one table.
        let shape = region(1 << 36).shape(1);
        assert_eq!(
            shape,
            Shape {
                large_frames: 0,
                base_pages: 1,
                tables: 1,
            }
        );
    }

    #[test]
    fn a_span_that_already_has_a_table_never_takes_a_large_frame() {
        let mut region = region(1 << 36);
        let first = region.shape(1);
        assert_eq!(first.tables, 1);
        region.commit_shape_for_test(first);
        // The rest of the span is base pages, and the table is not paid twice.
        let rest = region.shape(LARGE_FRAME_PAGES - 1);
        assert_eq!(
            rest,
            Shape {
                large_frames: 0,
                base_pages: LARGE_FRAME_PAGES - 1,
                tables: 0,
            }
        );
    }

    #[test]
    fn a_growth_crossing_a_span_boundary_prices_each_span_separately() {
        let mut region = region(1 << 36);
        let first = region.shape(1);
        region.commit_shape_for_test(first);
        // One page in the first span, then a whole aligned span: the second
        // span is a large frame even though the first one is paged.
        let crossing = region.shape(LARGE_FRAME_PAGES - 1 + LARGE_FRAME_PAGES);
        assert_eq!(
            crossing,
            Shape {
                large_frames: 1,
                base_pages: LARGE_FRAME_PAGES - 1,
                tables: 0,
            }
        );
    }

    #[test]
    fn a_misaligned_window_is_refused_at_construction_not_mispriced() {
        // Span ownership is indexed from the base, so a base one granule past
        // a 2 MiB boundary would put the first span's pages under two leaf
        // tables and price only one. Such a window is refused at construction
        // rather than mispriced at the mapping that would need the second.
        let base = (1 << 36) + GRANULE_SIZE;
        assert!(
            Region::reserve_for_test(base, LARGE_FRAME_PAGES, LARGE_FRAME_PAGES, true).is_err()
        );
        // A window that ends past the last address is refused for the same
        // reason: every page address below derives from that sum unchecked.
        let last_block = usize::MAX - LARGE_FRAME_BYTES + 1;
        assert!(
            Region::reserve_for_test(last_block, 2 * LARGE_FRAME_PAGES, 1, true).is_err(),
            "a window ending past the last address must be refused"
        );
    }

    #[test]
    fn a_window_wider_than_the_compiled_row_prices_its_own_spans() {
        // The window is policy-derived, so span ownership must extend past the
        // per-region page count this image was compiled for; pricing a span
        // beyond it as absent would take a second table over one address.
        let reservation = super::super::MAX_REGION_PAGES + LARGE_FRAME_PAGES;
        let mut region = region_of(1 << 36, reservation);
        assert_eq!(region.reservation(), reservation);
        let first = region.shape(super::super::MAX_REGION_PAGES);
        region.commit_shape_for_test(first);
        assert_eq!(
            region.shape(LARGE_FRAME_PAGES),
            Shape {
                large_frames: 1,
                base_pages: 0,
                tables: 0,
            },
            "the span past the compiled row is still tracked as its own"
        );
    }

    /// The guaranteed lane prices what it maps: base pages and their tables,
    /// never a large frame. The same pages in the elastic lane are one 2 MiB
    /// frame, which is the selection a guarantee must not depend on.
    #[test]
    fn the_guaranteed_lane_prices_base_pages_and_never_a_large_frame() {
        let guaranteed = region(1 << 36).shape_in(LARGE_FRAME_PAGES, Lane::BasePage);
        assert_eq!(
            guaranteed,
            Shape {
                large_frames: 0,
                base_pages: LARGE_FRAME_PAGES,
                tables: 1,
            }
        );
        assert_eq!(
            region(1 << 36).shape_in(LARGE_FRAME_PAGES, Lane::LargeFrame),
            Shape {
                large_frames: 1,
                base_pages: 0,
                tables: 0,
            }
        );
        // Backing a whole aligned span of base pages still takes one aligned
        // 2 MiB extent and one granule extent: the lane changes the mapping,
        // not the block the payload is retyped from.
        let demand = guaranteed.request().demand().unwrap();
        assert_eq!(
            demand.resources().bytes,
            (LARGE_FRAME_PAGES as u64 + 1) * GRANULE_SIZE as u64
        );
        assert_eq!(demand.extents(), 2);
        assert_eq!(demand.resources().tables, 1);
        assert_eq!(demand.descriptors(), LARGE_FRAME_PAGES + 1);
    }

    #[test]
    fn a_zero_growth_prices_nothing() {
        assert_eq!(region(1 << 36).shape(0), Shape::default());
    }
}
