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

use super::{GrowError, NativePrivateMemoryKernel, PrivateMemoryKernel, Region, Table, map_growth};
use crate::child_vspace::{LARGE_FRAME_BYTES, LARGE_FRAME_PAGES};
use crate::object_allocator::elastic::{ElasticRefusal, ElasticRequest};
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
    pub fn shape(self, delta: usize) -> Shape {
        let mut shape = Shape::default();
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
            if !leaf_available
                && vaddr.is_multiple_of(LARGE_FRAME_BYTES)
                && remaining >= LARGE_FRAME_PAGES
            {
                shape.large_frames += 1;
                page += LARGE_FRAME_PAGES;
                continue;
            }
            if !leaf_available {
                shape.tables += 1;
                leaf_available = true;
            }
            shape.base_pages += 1;
            page += 1;
        }
        shape
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

    let shape = region.shape(delta);
    let demand = shape.request().demand().map_err(ElasticGrowError::Demand)?;
    allocator
        .preflight_elastic(&demand)
        .map_err(ElasticGrowError::Demand)?;
    let acquisition = allocator
        .acquire_elastic(arena, &demand)
        .map_err(ElasticGrowError::Acquire)?;

    // The plan is built from the placements acquisition actually obtained, so
    // the ledger judges physical fit rather than a byte sum. Both refusals
    // here precede every retype, and both return the whole acquisition.
    let plan = match acquisition.plan() {
        Ok(plan) => plan,
        Err(error) => return Err(settle_refusal(allocator, &acquisition, error)),
    };
    if let Err(error) = ledger.begin(token, delta as u64, plan) {
        return Err(settle_refusal(allocator, &acquisition, error));
    }

    match map_growth(allocator, arena, vspace, region, delta, kernel) {
        Ok(outcome) => {
            if let Err(error) = allocator.commit_private_transaction(arena) {
                return Err(settle_failure(
                    allocator,
                    ledger,
                    token,
                    &acquisition,
                    ElasticGrowError::Frames {
                        allocated: outcome.pages_backed,
                        error,
                    },
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
        Err(GrowError::Frames { allocated, error }) => Err(settle_failure(
            allocator,
            ledger,
            token,
            &acquisition,
            ElasticGrowError::Frames { allocated, error },
        )),
        Err(other) => Err(settle_failure(
            allocator,
            ledger,
            token,
            &acquisition,
            ElasticGrowError::Frames {
                allocated: 0,
                error: match other {
                    GrowError::Frames { error, .. } => error,
                    _ => AllocError::NoKernelUntyped,
                },
            },
        )),
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
        &mut kernel,
    )
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
    match allocator.release_elastic(acquisition, true) {
        Ok(retained) => match ledger.abort(token, retained, true) {
            Ok(()) => failure,
            Err(error) => ElasticGrowError::Policy(error),
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
    let pages = table.reclaim(allocator, region);
    ledger.retire(token, revoked)?;
    Ok(pages)
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

    #[test]
    fn a_zero_growth_prices_nothing() {
        assert_eq!(region(1 << 36).shape(0), Shape::default());
    }
}
