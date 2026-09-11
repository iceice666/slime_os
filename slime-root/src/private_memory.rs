//! Task-private growable working memory (C10.1).
//!
//! A component's working memory is otherwise fixed at build time: its stack
//! comes from the image header and its `.data`/`.bss` from the linked ELF, so
//! every buffer must be sized for its worst case in every generation carrying
//! that component. This module is the mechanism that lifts that: one
//! task-private region, reserved as address space when the child VSpace is
//! built, backed page by page on demand, and returned in full when the task
//! dies.
//!
//! # What this is not
//!
//! It is deliberately *not* the C7 shared-buffer plane. Shared buffers exist to
//! move samples *between* components: each is a nameable, transferable,
//! loanable object the root retypes and tracks under a root-wide page ceiling.
//! Working memory is private, never transferred, never sealed, and needs no
//! physical contiguity — so there is no object here for a capability to
//! designate. What is bounded is *how many pages a task may hold*, which is a
//! budget, exactly as the stack is generation-sized and needs no capability.
//! Nothing here adds a seL4 object kind, a root-tracked object, or a right.
//!
//! # Shape
//!
//! The region is one window at a fixed base, grown only at its tail:
//!
//! * **The base never moves.** Native component images link at fixed virtual
//!   addresses and hold real machine pointers, so a growth that relocated the
//!   base would invalidate every live pointer. A growth past the reservation
//!   fails rather than moving. (WebAssembly runtimes may relocate a linear
//!   memory base precisely because Wasm code addresses by offset; native code
//!   has no such freedom.)
//! * **Address space is reserved at spawn, backing arrives on demand.** Leaf
//!   tables stay absent until a 4 KiB mapping needs them, so an aligned 2 MiB
//!   run can occupy the same slot directly. The task arena reserves the larger
//!   of the all-base-page and mixed-allocation constructions.
//! * **Growth is all-or-nothing.** A failure part way through revokes every
//!   backing extent the attempt touched while leaving the page count and every
//!   existing mapping exactly as they were.
//! * **Pages are user/read-write/execute-never, always.** No growth path can
//!   derive an executable mapping, so W^X holds by construction.
//! * **Allocation policy is userspace's.** The root tracks a page count and
//!   never an allocation; `free` is a free-list operation inside the component.

use sel4::CapTypeForObjectOfFixedSize;

use crate::child_vspace::{
    GRANULE_SIZE, LARGE_FRAME_BYTES, LARGE_FRAME_PAGES, LARGE_FRAME_TYPE, private_leaf_table_type,
};
use crate::object_allocator::{
    AllocError, ObjectAllocator, PrivateAllocation, PrivateObjectKind, TaskArenaId,
};

/// Pages one task's private region may ever hold.
///
/// This is the *reservation*, and therefore the hard structural ceiling: the
/// window's address space and its translation tables are sized for it when the
/// child VSpace is built (`child_vspace::private_window`), so no declared quota
/// can exceed it and a growth past it fails rather than relocating the base.
///
/// 512 pages is 2 MiB, exactly one large-frame span on both supported
/// architectures, so a full aligned reservation has one block-mapping shape.
pub const MAX_REGION_PAGES: usize = 512;

/// Pages every live private region may hold together.
///
/// Distinct from [`MAX_REGION_PAGES`] on purpose: that one says a single task
/// fits, this one says every task fits *at once*. Without it, `MAX_TASKS`
/// components each at their own ceiling would be admitted one at a time and
/// then exhaust the untyped pool the root also serves devices and shared
/// buffers from.
///
/// 2048 pages is 8 MiB — four tasks at the full per-task reservation, and far
/// more than any current composition's declared quotas sum to. It is checked
/// against the frame allocator's own exhaustion rather than trusted: a growth
/// that passes this ceiling and still cannot retype fails on frames instead.
pub const MAX_TOTAL_PAGES: usize = 2048;

// The contract publishes both ceilings so `build-generation.py` can refuse an
// over-declared or over-committed budget on the build side (C10.2). Pinned
// here rather than trusted: if the two ever drift, the builder would reject
// budgets this root would honour, or admit ones it would not — and the
// disagreement would surface as a runtime refusal against a quota the
// generation promised. A compile-time assert makes it a build failure instead.
const _: () = assert!(
    MAX_REGION_PAGES == boot_contracts::private_memory_budget::ROOT_REGION_PAGES,
    "private-memory reservation drifted from contracts/private-memory-budget/v1"
);
const _: () = assert!(
    MAX_TOTAL_PAGES == boot_contracts::private_memory_budget::ROOT_TOTAL_PAGES,
    "private-memory root ceiling drifted from contracts/private-memory-budget/v1"
);

/// Why a growth was refused.
///
/// Four distinct causes, because a caller must be able to tell "I asked for
/// something impossible" from "the machine is full": an allocator that cannot
/// distinguish quota exhaustion from frame exhaustion cannot decide whether
/// retrying later could help. The root's own markers name the cause; the wire
/// status the caller sees stays the coarse class
/// `contracts/syscall-abi/v1` declares.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GrowError {
    /// `pages + delta` does not fit a `usize`, or the delta itself does not fit
    /// the page-count representation. Refused before any arithmetic that could
    /// wrap into a small, plausible-looking request.
    DeltaOverflow { pages: usize, delta: usize },
    /// The growth would pass the reserved window. The base cannot move, so this
    /// is a refusal rather than a relocation.
    ReservationExceeded {
        pages: usize,
        delta: usize,
        reservation: usize,
    },
    /// The growth would pass this task's declared quota.
    QuotaExceeded {
        pages: usize,
        delta: usize,
        quota: usize,
    },
    /// The growth would pass the root-wide ceiling every live region shares.
    TotalExceeded {
        total: usize,
        delta: usize,
        ceiling: usize,
    },
    /// A frame could not be retyped or mapped. Every backing object the attempt
    /// had already taken has been returned before this is reported.
    Frames { allocated: usize, error: AllocError },
}

/// One task's private region: where it starts, how far it may grow, and how
/// much of it is currently backed.
///
/// `Copy`, because [`crate::task::Task`] is: the whole per-task record is a
/// fixed-size value the table stores inline, so the region cannot hold a
/// heap-backed structure. It does not need one — the root tracks a count, and
/// the frames themselves are owned by the task's arena.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Region {
    base: usize,
    reservation: usize,
    quota: usize,
    pages: usize,
    large_frames: usize,
    base_frames: usize,
    leaf_tables: usize,
}

impl Region {
    /// A region no task can grow: no address space, no quota.
    ///
    /// The deny-by-default state, and what the fixture paths and any task the
    /// generation gives no quota hold. Deliberately not "reserved but
    /// zero-quota": a task with no quota should also not carry a window, so
    /// nothing can later mistake the reservation for permission.
    pub const DENIED: Self = Self {
        base: 0,
        reservation: 0,
        quota: 0,
        pages: 0,
        large_frames: 0,
        base_frames: 0,
        leaf_tables: 0,
    };

    /// A region reserved at `base`, with `quota` pages of growth authorized.
    ///
    /// `quota` is clamped by the reservation rather than refused: the
    /// reservation is this root's structural bound and the quota is the
    /// generation's policy, so a policy asking for more than the mechanism can
    /// hold is honoured up to the mechanism. C10.2's admission refuses such a
    /// budget at decode, before any component launches; this clamp is the
    /// mechanism side that holds when a quota arrives from anywhere else.
    pub const fn reserved(base: usize, quota: usize) -> Self {
        let quota = if quota > MAX_REGION_PAGES {
            MAX_REGION_PAGES
        } else {
            quota
        };
        Self {
            base,
            reservation: MAX_REGION_PAGES,
            quota,
            pages: 0,
            large_frames: 0,
            base_frames: 0,
            leaf_tables: 0,
        }
    }

    pub const fn base(self) -> usize {
        self.base
    }

    /// Pages currently backed by a frame.
    pub const fn pages(self) -> usize {
        self.pages
    }

    pub const fn quota(self) -> usize {
        self.quota
    }

    pub const fn large_frames(self) -> usize {
        self.large_frames
    }

    pub const fn base_frames(self) -> usize {
        self.base_frames
    }

    pub const fn leaf_tables(self) -> usize {
        self.leaf_tables
    }

    /// Bytes of address space the window spans, backed or not.
    pub const fn reserved_bytes(self) -> usize {
        self.reservation * GRANULE_SIZE
    }

    /// The window's address range, for the VSpace construction that reserves it
    /// and for the guard-placement arithmetic that keeps it clear of the image.
    pub const fn window(self) -> core::ops::Range<usize> {
        self.base..self.base + self.reservation * GRANULE_SIZE
    }

    /// Whether `range` runs into this window.
    ///
    /// The one question every *other* mapping path has to ask (C10.4). The
    /// window is reserved address space whose leaf frames arrive on demand, so
    /// before the allocator has grown into a page there is nothing mapped there
    /// and an unrelated `frame_map` at that address simply succeeds. What lands
    /// is then indistinguishable from private memory to the component holding
    /// it: its allocator hands the range out as heap and writes structured
    /// allocations over storage another component can reach, while a later
    /// growth maps a frame at an address the other table believes it owns.
    /// Neither table is wrong on its own terms, which is exactly why neither
    /// would report it.
    ///
    /// Tested against the whole reservation rather than the backed prefix,
    /// deliberately. Bounding it by the live page count would make the answer
    /// depend on how far this component's allocator happened to have grown, so
    /// the same generation would admit or refuse the same mapping depending on
    /// timing. The reservation is a fixed property of the VSpace, and refusing
    /// against it makes the two planes disjoint by construction rather than by
    /// schedule.
    ///
    /// A [`Self::DENIED`] region overlaps nothing: it carries no window, which
    /// is why a task with no quota is not a task with a hole in its address
    /// space that nothing may use.
    pub const fn overlaps(self, range: &core::ops::Range<usize>) -> bool {
        let window = self.window();
        window.start < window.end && range.start < window.end && window.start < range.end
    }

    /// Where the next grown page lands.
    const fn next_vaddr(self) -> usize {
        self.base + self.pages * GRANULE_SIZE
    }

    /// Check a growth against every bound *before* a frame is touched.
    ///
    /// Split out from [`Table::grow`] so the ordering of the four refusals is
    /// testable on the host without a kernel: which bound a caller hits decides
    /// which error it sees, and that ordering is part of the operation's
    /// contract.
    fn admit(self, delta: usize, total: usize) -> Result<(), GrowError> {
        let pages = self.pages;
        let Some(requested) = pages.checked_add(delta) else {
            return Err(GrowError::DeltaOverflow { pages, delta });
        };
        // Reservation before quota: the reservation is what the address space
        // can physically hold, so a request past it is malformed rather than
        // merely unaffordable, and reporting the affordable-but-impossible case
        // as a quota problem would tell a caller to wait for something that
        // will never happen.
        if requested > self.reservation {
            return Err(GrowError::ReservationExceeded {
                pages,
                delta,
                reservation: self.reservation,
            });
        }
        if requested > self.quota {
            return Err(GrowError::QuotaExceeded {
                pages,
                delta,
                quota: self.quota,
            });
        }
        let Some(requested_total) = total.checked_add(delta) else {
            return Err(GrowError::DeltaOverflow { pages, delta });
        };
        if requested_total > MAX_TOTAL_PAGES {
            return Err(GrowError::TotalExceeded {
                total,
                delta,
                ceiling: MAX_TOTAL_PAGES,
            });
        }
        Ok(())
    }
}

pub(crate) trait PrivateMemoryKernel {
    fn retype(
        &mut self,
        parent: sel4::cap::Untyped,
        blueprint: &sel4::ObjectBlueprint,
        slot: usize,
    ) -> Result<(), sel4::Error>;
    fn map_frame(
        &mut self,
        frame: sel4::cap::UnspecifiedPage,
        vspace: sel4::cap::VSpace,
        vaddr: usize,
        rights: sel4::CapRights,
        attrs: sel4::VmAttributes,
    ) -> Result<(), sel4::Error>;
    fn map_leaf(
        &mut self,
        table: sel4::cap::UnspecifiedIntermediateTranslationTable,
        ty: sel4::TranslationTableObjectType,
        vspace: sel4::cap::VSpace,
        vaddr: usize,
        attrs: sel4::VmAttributes,
    ) -> Result<(), sel4::Error>;
    fn unmap_frame(&mut self, frame: sel4::cap::UnspecifiedPage) -> Result<(), sel4::Error>;
}

struct NativePrivateMemoryKernel;

impl PrivateMemoryKernel for NativePrivateMemoryKernel {
    fn retype(
        &mut self,
        parent: sel4::cap::Untyped,
        blueprint: &sel4::ObjectBlueprint,
        slot: usize,
    ) -> Result<(), sel4::Error> {
        parent.untyped_retype(
            blueprint,
            &sel4::init_thread::slot::CNODE
                .cap()
                .absolute_cptr_for_self(),
            slot,
            1,
        )
    }

    fn map_frame(
        &mut self,
        frame: sel4::cap::UnspecifiedPage,
        vspace: sel4::cap::VSpace,
        vaddr: usize,
        rights: sel4::CapRights,
        attrs: sel4::VmAttributes,
    ) -> Result<(), sel4::Error> {
        frame.frame_map(vspace, vaddr, rights, attrs)
    }

    fn map_leaf(
        &mut self,
        table: sel4::cap::UnspecifiedIntermediateTranslationTable,
        ty: sel4::TranslationTableObjectType,
        vspace: sel4::cap::VSpace,
        vaddr: usize,
        attrs: sel4::VmAttributes,
    ) -> Result<(), sel4::Error> {
        table.generic_intermediate_translation_table_map(ty, vspace, vaddr, attrs)
    }

    fn unmap_frame(&mut self, frame: sel4::cap::UnspecifiedPage) -> Result<(), sel4::Error> {
        frame.frame_unmap()
    }
}

/// Every live private region, and the root-wide page count they share.
///
/// The table is indexed by nothing: each task owns its own [`Region`] inline,
/// and this holds only the aggregate the root-wide ceiling is enforced against.
/// Keeping the total here rather than recomputing it from the task table is
/// what makes the ceiling a single-writer invariant — a growth charges it and a
/// reclamation returns it, and no third party can disagree about the sum.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Table {
    total_pages: usize,
    grants: usize,
    grown_pages: usize,
    reclaimed_pages: usize,
}

impl Table {
    pub const fn new() -> Self {
        Self {
            total_pages: 0,
            grants: 0,
            grown_pages: 0,
            reclaimed_pages: 0,
        }
    }

    /// Pages backed across every live region.
    pub const fn total_pages(&self) -> usize {
        self.total_pages
    }

    /// Growth operations served, for the root's own accounting marker.
    pub const fn grants(&self) -> usize {
        self.grants
    }

    /// Pages ever granted, and pages ever returned. Their difference is
    /// [`Self::total_pages`], which is what makes a leak observable on serial
    /// rather than inferred from a frame count.
    pub const fn grown_pages(&self) -> usize {
        self.grown_pages
    }

    pub const fn reclaimed_pages(&self) -> usize {
        self.reclaimed_pages
    }

    /// Grow `region` by `delta` pages, answering the page count *before* the
    /// growth. A failed transaction does not change logical page charges or
    /// existing committed mappings and data. Translation structure mapped by
    /// the failed attempt may remain for a retry of the same fixed region.
    pub fn grow(
        &mut self,
        allocator: &mut ObjectAllocator,
        arena: TaskArenaId,
        vspace: sel4::cap::VSpace,
        region: &mut Region,
        delta: usize,
    ) -> Result<usize, GrowError> {
        self.grow_with_kernel(
            allocator,
            arena,
            vspace,
            region,
            delta,
            &mut NativePrivateMemoryKernel,
        )
    }

    pub(crate) fn grow_with_kernel<K: PrivateMemoryKernel>(
        &mut self,
        allocator: &mut ObjectAllocator,
        arena: TaskArenaId,
        vspace: sel4::cap::VSpace,
        region: &mut Region,
        delta: usize,
        kernel: &mut K,
    ) -> Result<usize, GrowError> {
        region.admit(delta, self.total_pages)?;
        let previous = region.pages;
        if delta == 0 {
            return Ok(previous);
        }
        let mut pages_backed = 0;
        let mut large_frames = 0;
        let mut base_frames = 0;
        let mut leaf_available = region.leaf_tables != 0;
        while pages_backed < delta {
            let page = previous + pages_backed;
            let vaddr = region.base + page * GRANULE_SIZE;
            let remaining = delta - pages_backed;
            let large = !leaf_available
                && vaddr.is_multiple_of(LARGE_FRAME_BYTES)
                && remaining >= LARGE_FRAME_PAGES;
            let result = if large {
                back_large(allocator, arena, vspace, vaddr, kernel)
            } else {
                if !leaf_available {
                    match back_leaf_table(allocator, arena, vspace, region.base, kernel) {
                        Ok(table) => {
                            region.leaf_tables = 1;
                            if let Err(error) =
                                allocator.mark_private_in_flight(arena, table.allocation, true)
                            {
                                let _ = allocator.unwind_private_transaction(arena, kernel);
                                return Err(GrowError::Frames {
                                    allocated: pages_backed,
                                    error,
                                });
                            }
                            leaf_available = true;
                        }
                        Err(error) => {
                            return Err(GrowError::Frames {
                                allocated: pages_backed,
                                error: match allocator.unwind_private_transaction(arena, kernel) {
                                    Ok(()) => error,
                                    Err(cleanup) => cleanup,
                                },
                            });
                        }
                    }
                }
                back_page(allocator, arena, vspace, vaddr, kernel)
            };
            match result {
                Ok(backing) => {
                    let pages = backing.pages;
                    if let Err(error) =
                        allocator.mark_private_in_flight(arena, backing.allocation, true)
                    {
                        let _ = allocator.unwind_private_transaction(arena, kernel);
                        return Err(GrowError::Frames {
                            allocated: pages_backed,
                            error,
                        });
                    }
                    pages_backed += pages;
                    if pages == LARGE_FRAME_PAGES {
                        large_frames += 1;
                    } else {
                        base_frames += pages;
                    }
                }
                Err(error) => {
                    return Err(GrowError::Frames {
                        allocated: pages_backed,
                        error: match allocator.unwind_private_transaction(arena, kernel) {
                            Ok(()) => error,
                            Err(cleanup) => cleanup,
                        },
                    });
                }
            }
        }
        allocator
            .commit_private_transaction(arena)
            .map_err(|error| GrowError::Frames {
                allocated: pages_backed,
                error,
            })?;
        region.pages = previous + delta;
        region.large_frames += large_frames;
        region.base_frames += base_frames;
        self.total_pages += delta;
        self.grown_pages += delta;
        self.grants += 1;
        Ok(previous)
    }

    /// Return one dying task's pages to the root-wide total.
    ///
    /// The frames themselves are destroyed by the task-arena revoke every
    /// reclamation already performs — they were retyped from that arena's
    /// untyped, so nothing separate has to unmap them. What this returns is the
    /// *charge*, which is bookkeeping the revoke cannot see.
    ///
    /// Idempotent: reclaiming a region twice returns zero the second time, so a
    /// retried teardown cannot drive the total negative.
    pub fn reclaim(&mut self, region: &mut Region) -> usize {
        let pages = region.pages;
        region.pages = 0;
        region.large_frames = 0;
        region.base_frames = 0;
        region.leaf_tables = 0;
        self.total_pages = self.total_pages.saturating_sub(pages);
        self.reclaimed_pages += pages;
        pages
    }
}

/// One mapping an in-flight growth has taken. Persistent transaction state is
/// held in the allocator's capacity-scaled descriptor pool.
struct Backing {
    allocation: PrivateAllocation,
    pages: usize,
}

fn back_leaf_table<K: PrivateMemoryKernel>(
    allocator: &mut ObjectAllocator,
    arena: TaskArenaId,
    vspace: sel4::cap::VSpace,
    vaddr: usize,
    kernel: &mut K,
) -> Result<Backing, AllocError> {
    let ty = private_leaf_table_type();
    let size_bits = ty.blueprint().physical_size_bits();
    let allocation = allocator.acquire_private_in(
        arena,
        PrivateObjectKind::LeafTable,
        ty.blueprint(),
        kernel,
    )?;
    let table = allocation
        .cap()
        .cast::<sel4::cap_type::UnspecifiedIntermediateTranslationTable>();
    if !allocation.mapped()
        && let Err(error) = kernel.map_leaf(table, ty, vspace, vaddr, sel4::VmAttributes::default())
    {
        allocator.reset_private_in(arena, allocation)?;
        return Err(AllocError::Retype { size_bits, error });
    }
    Ok(Backing {
        allocation,
        pages: 0,
    })
}

fn back_page<K: PrivateMemoryKernel>(
    allocator: &mut ObjectAllocator,
    arena: TaskArenaId,
    vspace: sel4::cap::VSpace,
    vaddr: usize,
    kernel: &mut K,
) -> Result<Backing, AllocError> {
    let size_bits = sel4::FrameObjectType::GRANULE.bits();
    let allocation = allocator.acquire_private_in(
        arena,
        PrivateObjectKind::Granule,
        sel4::cap_type::Granule::object_blueprint(),
        kernel,
    )?;
    let frame = allocation.cap().cast::<sel4::cap_type::UnspecifiedPage>();
    if !allocation.mapped()
        && let Err(error) = kernel.map_frame(
            frame,
            vspace,
            vaddr,
            sel4::CapRights::read_write(),
            sel4::VmAttributes::DEFAULT | sel4::VmAttributes::EXECUTE_NEVER,
        )
    {
        allocator.reset_private_in(arena, allocation)?;
        return Err(AllocError::Retype { size_bits, error });
    }
    Ok(Backing {
        allocation,
        pages: 1,
    })
}

fn back_large<K: PrivateMemoryKernel>(
    allocator: &mut ObjectAllocator,
    arena: TaskArenaId,
    vspace: sel4::cap::VSpace,
    vaddr: usize,
    kernel: &mut K,
) -> Result<Backing, AllocError> {
    let size_bits = LARGE_FRAME_TYPE.bits();
    let allocation = allocator.acquire_private_in(
        arena,
        PrivateObjectKind::LargeFrame,
        LARGE_FRAME_TYPE.blueprint(),
        kernel,
    )?;
    let frame = allocation.cap().cast::<sel4::cap_type::UnspecifiedPage>();
    #[cfg(slime_private_fail_large_map)]
    if !allocation.mapped() && crate::object_allocator::take_forced_private_large_map_failure() {
        allocator.reset_private_in(arena, allocation)?;
        return Err(AllocError::Retype {
            size_bits,
            error: sel4::Error::NotEnoughMemory,
        });
    }
    if !allocation.mapped()
        && let Err(error) = kernel.map_frame(
            frame,
            vspace,
            vaddr,
            sel4::CapRights::read_write(),
            sel4::VmAttributes::DEFAULT | sel4::VmAttributes::EXECUTE_NEVER,
        )
    {
        allocator.reset_private_in(arena, allocation)?;
        return Err(AllocError::Retype { size_bits, error });
    }
    Ok(Backing {
        allocation,
        pages: LARGE_FRAME_PAGES,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region() -> Region {
        Region::reserved(0x1000_0000, 8)
    }

    #[test]
    fn a_denied_region_has_no_window_and_no_quota() {
        let denied = Region::DENIED;
        assert_eq!(denied.quota(), 0);
        assert_eq!(denied.pages(), 0);
        assert_eq!(denied.reserved_bytes(), 0);
        assert!(denied.window().is_empty());
        // Deny-by-default is the whole point: a task the generation does not
        // name grows nothing, and it fails on the reservation rather than
        // reaching a quota comparison against zero.
        assert_eq!(
            denied.admit(1, 0),
            Err(GrowError::ReservationExceeded {
                pages: 0,
                delta: 1,
                reservation: 0,
            })
        );
    }

    #[test]
    fn a_zero_delta_is_admitted_even_on_a_denied_region() {
        // A size query allocates nothing, so refusing it would force an
        // allocator with no quota to guess its own extent.
        assert_eq!(Region::DENIED.admit(0, 0), Ok(()));
        assert_eq!(Region::DENIED.admit(0, MAX_TOTAL_PAGES), Ok(()));
    }

    #[test]
    fn a_quota_is_clamped_to_the_reservation_never_raised_past_it() {
        let wide = Region::reserved(0x1000_0000, MAX_REGION_PAGES * 4);
        assert_eq!(wide.quota(), MAX_REGION_PAGES);
        assert_eq!(wide.reserved_bytes(), MAX_REGION_PAGES * GRANULE_SIZE);
    }

    #[test]
    fn the_window_starts_at_the_base_and_spans_the_whole_reservation() {
        let region = region();
        assert_eq!(region.base(), 0x1000_0000);
        assert_eq!(region.window().start, 0x1000_0000);
        assert_eq!(
            region.window().end,
            0x1000_0000 + MAX_REGION_PAGES * GRANULE_SIZE
        );
        assert_eq!(region.next_vaddr(), 0x1000_0000);
    }

    #[test]
    fn a_mapping_anywhere_in_the_reservation_overlaps_even_where_no_page_is_backed() {
        let region = region();
        let window = region.window();
        // Nothing is backed here at all: `pages()` is zero. The whole point is
        // that an unbacked address inside the window is still not available to
        // another mapping, because whether a page has been grown into is a
        // property of when the check runs rather than of the address space.
        assert_eq!(region.pages(), 0);
        assert!(region.overlaps(&(window.start..window.start + GRANULE_SIZE)));
        assert!(region.overlaps(&(window.end - GRANULE_SIZE..window.end)));
        // A range straddling either boundary overlaps: the first byte inside is
        // enough, and a mapping is refused whole rather than clipped.
        assert!(region.overlaps(&(window.start - GRANULE_SIZE..window.start + GRANULE_SIZE)));
        assert!(region.overlaps(&(window.end - GRANULE_SIZE..window.end + GRANULE_SIZE)));
        // And one spanning it entirely, which a naive containment test misses.
        assert!(region.overlaps(&(window.start - GRANULE_SIZE..window.end + GRANULE_SIZE)));
    }

    #[test]
    fn a_mapping_touching_either_boundary_from_outside_does_not_overlap() {
        let region = region();
        let window = region.window();
        // Half-open on both ends: the page ending exactly at the base and the
        // one starting exactly at the end are both legal. Off-by-one here would
        // refuse a component the page immediately below its own window, which
        // `child_vspace` deliberately leaves as the guard granule.
        assert!(!region.overlaps(&(window.start - GRANULE_SIZE..window.start)));
        assert!(!region.overlaps(&(window.end..window.end + GRANULE_SIZE)));
        // An empty range at the base names no byte and so takes nothing.
        assert!(!region.overlaps(&(window.start..window.start)));
    }

    #[test]
    fn a_denied_region_overlaps_nothing_it_has_no_window_to_defend() {
        // A task with no quota carries no reservation, so there is no hole in
        // its address space and no address another mapping may not use. Were
        // this to answer `true` for the zero range, every mapping by every
        // component with no declared quota would be refused.
        let denied = Region::DENIED;
        assert!(!denied.overlaps(&(0..GRANULE_SIZE)));
        assert!(!denied.overlaps(&(0..usize::MAX)));
    }

    #[test]
    fn growth_within_the_quota_is_admitted_and_the_quota_edge_is_inclusive() {
        let region = region();
        assert_eq!(region.admit(1, 0), Ok(()));
        assert_eq!(region.admit(8, 0), Ok(()));
        assert_eq!(
            region.admit(9, 0),
            Err(GrowError::QuotaExceeded {
                pages: 0,
                delta: 9,
                quota: 8,
            })
        );
    }

    #[test]
    fn a_delta_that_would_overflow_the_page_count_is_refused_before_any_bound() {
        let mut region = region();
        region.pages = 3;
        assert_eq!(
            region.admit(usize::MAX, 0),
            Err(GrowError::DeltaOverflow {
                pages: 3,
                delta: usize::MAX,
            })
        );
    }

    #[test]
    fn the_reservation_is_reported_before_the_quota() {
        let mut region = Region::reserved(0x1000_0000, MAX_REGION_PAGES * 2);
        region.pages = MAX_REGION_PAGES;
        assert!(matches!(
            region.admit(1, 0),
            Err(GrowError::ReservationExceeded { .. })
        ));
    }

    #[test]
    fn the_root_wide_ceiling_is_checked_after_the_per_task_bounds() {
        let region = region();
        assert_eq!(
            region.admit(4, MAX_TOTAL_PAGES - 2),
            Err(GrowError::TotalExceeded {
                total: MAX_TOTAL_PAGES - 2,
                delta: 4,
                ceiling: MAX_TOTAL_PAGES,
            })
        );
        assert_eq!(region.admit(2, MAX_TOTAL_PAGES - 2), Ok(()));
    }

    #[test]
    fn reclaiming_a_region_returns_its_pages_and_is_idempotent() {
        let mut table = Table::new();
        let mut region = region();
        // The commit half of `grow` without a kernel: the host cannot map a
        // frame, so the accounting is driven directly and the mapping path
        // stays the seL4 gate's to prove.
        region.pages = 5;
        table.total_pages = 5;
        table.grown_pages = 5;
        assert_eq!(table.reclaim(&mut region), 5);
        assert_eq!(table.total_pages(), 0);
        assert_eq!(table.reclaimed_pages(), 5);
        assert_eq!(region.pages(), 0);
        // A failed initial growth may retain its mapped leaf without charging a
        // page. Teardown must clear that translation state without inventing a
        // grant or reclamation.
        region.leaf_tables = 1;
        assert_eq!(table.reclaim(&mut region), 0);
        assert_eq!(region.leaf_tables(), 0);
        assert_eq!(table.total_pages(), 0);
        assert_eq!(table.grown_pages(), 5);
        assert_eq!(table.reclaimed_pages(), 5);
        assert_eq!(table.grants(), 0);
        // A retried teardown must not drive the total negative.
        assert_eq!(table.reclaim(&mut region), 0);
        assert_eq!(table.total_pages(), 0);
        assert_eq!(table.reclaimed_pages(), 5);
    }

    #[test]
    fn one_tasks_exhaustion_leaves_another_tasks_ceiling_intact() {
        // The root-wide total is shared; the per-task quota is not. A task at
        // its own ceiling must not lower anyone else's.
        let mut exhausted = Region::reserved(0x1000_0000, 4);
        exhausted.pages = 4;
        let untouched = Region::reserved(0x2000_0000, 4);
        assert_eq!(
            exhausted.admit(1, 4),
            Err(GrowError::QuotaExceeded {
                pages: 4,
                delta: 1,
                quota: 4,
            })
        );
        assert_eq!(untouched.admit(4, 4), Ok(()));
    }
}
