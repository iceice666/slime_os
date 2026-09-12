//! Deterministic BootInfo CSlot and untyped allocation for `slime-root`.
//!
//! Global objects (devices and shared buffers included) remain monotonic. Child
//! tasks are different: each is built below a derived untyped whose capability
//! is the task's lifetime anchor. Revoking that anchor removes every task object
//! and every alias derived from one of them. The anchor is then deleted and its
//! dedicated parent untyped can retype the same physical region for a later
//! task. Root CSlots are managed by a bounded bitmap and are returned only after
//! the corresponding capability is known to be gone.

use core::ops::Range;
#[cfg(slime_b38_force_unwind)]
use core::sync::atomic::{AtomicBool, Ordering};
#[cfg(slime_private_fail_large_map)]
use core::sync::atomic::{AtomicBool as PrivateMapAtomicBool, Ordering as PrivateMapOrdering};
#[cfg(slime_private_fail_second_allocation)]
use core::sync::atomic::{
    AtomicBool as PrivateAllocationAtomicBool, Ordering as PrivateAllocationOrdering,
};

#[cfg(slime_b38_force_unwind)]
static FORCE_UNWIND_ONCE: AtomicBool = AtomicBool::new(true);

#[cfg(slime_b38_force_unwind)]
pub(crate) fn take_forced_unwind() -> bool {
    FORCE_UNWIND_ONCE.swap(false, Ordering::Relaxed)
}

#[cfg(slime_private_fail_second_allocation)]
static FORCE_PRIVATE_SECOND_ALLOCATION_FAILURE: PrivateAllocationAtomicBool =
    PrivateAllocationAtomicBool::new(false);

#[cfg(slime_private_fail_large_map)]
static FORCE_PRIVATE_LARGE_MAP_FAILURE: PrivateMapAtomicBool = PrivateMapAtomicBool::new(true);

/// Arm the one injected private-growth retype failure for the next transaction.
/// Set by the fixture service loop so the fault lands on a specific request
/// rather than on whichever growth in the boot happens to be first.
#[cfg(slime_private_fail_second_allocation)]
pub fn arm_private_second_allocation_failure() {
    FORCE_PRIVATE_SECOND_ALLOCATION_FAILURE.store(true, PrivateAllocationOrdering::Relaxed);
}

#[cfg(slime_private_fail_second_allocation)]
fn fail_private_allocation(kind: PrivateObjectKind, in_flight_granules: usize) -> bool {
    kind == PrivateObjectKind::Granule
        && in_flight_granules == 1
        && FORCE_PRIVATE_SECOND_ALLOCATION_FAILURE.swap(false, PrivateAllocationOrdering::Relaxed)
}

#[cfg(slime_private_fail_large_map)]
pub(crate) fn take_forced_private_large_map_failure() -> bool {
    FORCE_PRIVATE_LARGE_MAP_FAILURE.swap(false, PrivateMapOrdering::Relaxed)
}

pub const MAX_KERNEL_UNTYPEDS: usize = 64;
pub const MAX_DEVICE_UNTYPEDS: usize = 64;
/// Maximum root CSpace width admitted by the software slot bitmap.
///
/// This is the widest CNode the bitmap and [`ArenaAllocation`]'s packed slot
/// field can describe, not the width any platform provides:
/// `SlotPool::initialize` still accepts only the BootInfo span the selected
/// kernel actually exposes.
pub const MAX_ROOT_CSLOTS: usize = 524_288;
/// Maximum simultaneously owned task-backing records.
pub const MAX_TASK_ARENAS: usize = 48;
/// Root CSlots the kernel this root links against actually provides.
///
/// Read from the installed kernel configuration rather than declared here, so
/// a platform whose CNode width changes cannot leave a hand-written constant
/// describing the previous kernel.
const KERNEL_ROOT_CNODE_SLOTS: usize = 1 << sel4::sel4_cfg_usize!(ROOT_CNODE_SIZE_BITS);
/// Whether this image can afford descriptor tables sized for
/// [`MAX_PLANNED_PRIVATE_PAGES`] holders.
///
/// The tables are `.bss`, and in this root `.bss` is capacity, not just
/// memory: the seL4 loader creates one root CSlot per page of the root image
/// before the root runs, so the ~4 MiB `AllocationRecord` array alone spends
/// ~1026 root CSlots. A kernel narrower than [`MAX_ROOT_CSLOTS`] — every
/// physical board's 12-bit default — has 4096 slots in total, so those tables
/// would consume the CSpace before `admit_total_slots` ever evaluates the
/// product graph. Such an image keeps the 4096-record envelope, which bounds
/// private backing well above the 512-page runtime ceiling
/// [`crate::private_memory::MAX_REGION_PAGES`] enforces.
const LARGE_DESCRIPTOR_TABLES: bool = KERNEL_ROOT_CNODE_SLOTS >= MAX_ROOT_CSLOTS;
/// Root-owned task allocation descriptors.
///
/// One descriptor per object or retained capability the root owns for a task.
/// Private planning counts every 4 KiB fallback frame, every leaf table, and a
/// retained large frame per 2 MiB span. The static envelope covers the largest
/// admitted image, translation tables, per-thread pages, TCBs, aliases, VSpace,
/// and CNode. A plan's fit is still decided by [`TaskBackingCapacity::fits`]
/// against live availability; this bound only ensures the table can represent
/// the widest four-holder qualification on kernels that can afford it.
const MAX_PLANNED_PRIVATE_SPANS: usize =
    MAX_PLANNED_PRIVATE_PAGES.div_ceil(MAX_PRIVATE_EXTENT_PAGES);
const MAX_PLANNED_PRIVATE_ALLOCATIONS: usize =
    MAX_PLANNED_PRIVATE_PAGES + 2 * MAX_PLANNED_PRIVATE_SPANS;
const MAX_PLANNED_STATIC_ALLOCATIONS: usize = 2
    + 2 * (sel4::vspace_levels::NUM_LEVELS - 1)
    + (crate::child_vspace::MAX_CHILD_IMAGE_PAGES - 2 + 2 * crate::child_vspace::MAX_CHILD_THREADS)
    + 2 * crate::child_vspace::MAX_CHILD_THREADS;
pub const MAX_TASK_ALLOCATIONS: usize = if LARGE_DESCRIPTOR_TABLES {
    4 * (MAX_PLANNED_PRIVATE_ALLOCATIONS + MAX_PLANNED_STATIC_ALLOCATIONS)
} else {
    4096
};
/// Widest private-extent population any budget this root admits can demand.
///
/// A record's `size_bits` is fixed at creation and an inactive record is reused
/// only for its own size, so a position is never re-sized: the table must hold
/// every extent that is live at once, and a generation's holder set is fixed at
/// admission. Derived from the ceilings that decide admissibility rather than
/// stated, because this bound is *not* checked during admission — a budget
/// exceeding it would pass `admit_total_slots` and then fail partway through
/// [`ObjectAllocator::provision_extent`] with `ArenaTableFull`.
/// Each nonzero runtime quota uses both entries of `PrivateBackingLayout::extents`:
/// one data extent and one table extent, independent of its growth shape.
const fn max_admissible_private_extents() -> usize {
    let holders = boot_contracts::private_memory_budget::MAX_HOLDERS;
    let total = crate::private_memory::MAX_TOTAL_PAGES;
    2 * if holders < total { holders } else { total }
}

/// Every task consumes one static extent. A planned private holder additionally
/// consumes one data extent and one page-table extent per 2 MiB span.
pub const MAX_TASK_EXTENTS: usize = if LARGE_DESCRIPTOR_TABLES {
    MAX_TASK_ARENAS + 4 * (2 * MAX_PLANNED_PRIVATE_SPANS)
} else {
    3 * MAX_TASK_ARENAS
};
const _: () = assert!(
    MAX_TASK_EXTENTS >= MAX_TASK_ARENAS + max_admissible_private_extents(),
    "extent table cannot hold one static extent per task plus the widest \
     private-extent population contracts/private-memory-budget/v1 admits"
);

const SLOT_WORD_BITS: usize = usize::BITS as usize;
const SLOT_WORDS: usize = MAX_ROOT_CSLOTS.div_ceil(SLOT_WORD_BITS);
const GRANULE_BYTES: usize = 4096;
/// Largest independently reclaimable private-data extent.
const MAX_PRIVATE_EXTENT_BYTES: usize = 2 * 1024 * 1024;
const MAX_PRIVATE_EXTENT_PAGES: usize = MAX_PRIVATE_EXTENT_BYTES / GRANULE_BYTES;
/// Largest internal sizing case MEM-ARENAS promises the host planner can
/// represent without changing the public runtime ceiling.
pub const MAX_PLANNED_PRIVATE_PAGES: usize = 256 * 1024 * 1024 / GRANULE_BYTES;
/// Maximum queue pages allocated as one physically contiguous run.
const MAX_CONTIGUOUS_GRANULES: usize = 64;
/// Maximum objects in one seL4 untyped retype invocation.
const MAX_RETYPE_FAN_OUT: usize = 256;

fn device_retype_plan(
    retyped: usize,
    target: usize,
    free_slots: usize,
    holding_anchor: bool,
) -> Option<(usize, usize)> {
    let remaining = target.checked_sub(retyped)?.checked_add(1)?;
    let available = free_slots.saturating_sub(usize::from(holding_anchor));
    let batch = core::cmp::min(remaining, core::cmp::min(available, MAX_RETYPE_FAN_OUT));
    (batch != 0).then_some((remaining.div_ceil(MAX_RETYPE_FAN_OUT), batch))
}

/// Root CSlots whose allocation-time physical base is retained at once.
///
/// This bounds the **DMA-participating frame population, not the CSpace**.
/// Only a granule retyped from ordinary RAM, or an MMIO page retyped from a
/// device untyped, can be handed to a device, so only those need a physical
/// base remembered. The root holds at most: one frame-cap anchor per page of
/// every live shared buffer, IO1's contiguous device-queue pages, one frame per
/// declared MMIO region the probe reaches and per mapping IO1 hands out, the
/// userspace IO service's declared DMA and MMIO ceilings, and — only in the
/// immutable selector image — two bootstrap DMA pages per admitted boot device.
/// Every term is another module's own declared ceiling rather than a number
/// chosen here, so a plane that raises its bound raises this with it instead of
/// silently overrunning it.
///
/// The live bound is 448 entries, or 452 in the immutable selector image,
/// whose two bootstrap DMA pages per admitted boot device are the only terms
/// this product path no longer contributes. [`PROVENANCE_SLOTS`] rounds either
/// to a 1024-position open table of two-word records: 16 KiB of `.bss`, or four
/// root-image pages and their corresponding root CSlots.
#[cfg(slime_boot_selector)]
const SELECTOR_PHYSICAL_PROVENANCE: usize = 2 * crate::device::MAX_BLOCK_DEVICES;
#[cfg(not(slime_boot_selector))]
const SELECTOR_PHYSICAL_PROVENANCE: usize = 0;

const MAX_PHYSICAL_PROVENANCE: usize = crate::shared_buffer::MAX_FRAME_ANCHORS
    + crate::io_resource::MAX_DMA_MAPPINGS
    + crate::io_resource::MAX_MMIO_REGIONS
    + crate::io_resource::MAX_MMIO_MAPPINGS
    + SELECTOR_PHYSICAL_PROVENANCE;

/// Positions in the open-addressed provenance table.
///
/// A power of two so the probe start is a mask rather than a division, and
/// twice the live bound so the table never runs at a load factor that turns
/// linear probing into a scan. Never equal to the live bound: [`insert`]'s
/// walk to a free position relies on at least one entry always being empty.
///
/// [`insert`]: ProvenanceTable::insert
const PROVENANCE_SLOTS: usize = (2 * MAX_PHYSICAL_PROVENANCE).next_power_of_two();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AllocError {
    NoKernelUntyped,
    UntypedTableFull {
        limit: usize,
        declared: usize,
    },
    SlotsExhausted {
        allocated: usize,
    },
    SlotRangeTooLarge {
        declared: usize,
        limit: usize,
    },
    UntypedExhausted {
        size_bits: usize,
        remaining: usize,
    },
    Retype {
        size_bits: usize,
        error: sel4::Error,
    },
    DeviceTableFull {
        limit: usize,
        declared: usize,
    },
    NoDeviceUntyped {
        paddr: usize,
    },
    UnalignedDeviceFrame {
        paddr: usize,
    },
    DeviceFramePassed {
        paddr: usize,
    },
    DeviceCleanup {
        slot: usize,
        error: sel4::Error,
    },
    ArenaTableFull {
        limit: usize,
    },
    ArenaTooSmall {
        size_bits: usize,
        required: usize,
    },
    ArenaSlotTableFull {
        limit: usize,
    },
    UnknownArena(TaskArenaId),
    ArenaCleanup {
        slot: usize,
        error: sel4::Error,
    },
    /// The physical-provenance table has no free position for a frame this
    /// allocation just retyped.
    ///
    /// Fails the allocation rather than losing the record. A frame whose
    /// physical base the root cannot recover would be mapped into a device's
    /// descriptor with a *wrong* address, which is worse than not existing:
    /// the device would read or write memory nothing granted it.
    ProvenanceTableFull {
        limit: usize,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct UntypedRegion {
    cap: sel4::cap::Untyped,
    paddr: usize,
    size_bits: usize,
    watermark: usize,
}

impl UntypedRegion {
    fn capacity(&self) -> usize {
        1usize << self.size_bits
    }
    fn remaining(&self) -> usize {
        self.capacity().saturating_sub(self.watermark)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DeviceRegion {
    cap: sel4::cap::Untyped,
    paddr: usize,
    size_bits: usize,
    retyped: usize,
}

impl DeviceRegion {
    fn contains(&self, paddr: usize, len: usize) -> bool {
        let Some(end) = paddr.checked_add(len) else {
            return false;
        };
        let Some(region_end) = self.paddr.checked_add(1usize << self.size_bits) else {
            return false;
        };
        paddr >= self.paddr && end <= region_end
    }
}

/// Where an object lands under seL4's object-size alignment rule.
pub(crate) fn plan_allocation(
    watermark: usize,
    capacity: usize,
    size_bits: usize,
) -> Option<(usize, usize)> {
    let size = 1usize.checked_shl(u32::try_from(size_bits).ok()?)?;
    let start = watermark.checked_next_multiple_of(size)?;
    let end = start.checked_add(size)?;
    (end <= capacity).then_some((start, end))
}

fn plan_extent_in_regions(regions: &mut [Option<UntypedRegion>], size_bits: usize) -> bool {
    let Some((index, watermark)) = regions.iter().enumerate().find_map(|(index, region)| {
        let region = region.as_ref()?;
        plan_allocation(region.watermark, region.capacity(), size_bits).map(|(_, end)| (index, end))
    }) else {
        return false;
    };
    regions[index]
        .as_mut()
        .expect("selected untyped region exists")
        .watermark = watermark;
    true
}

fn task_backing_extents_fit_in(
    regions: &mut [Option<UntypedRegion>],
    plan: TaskBackingPlan,
    static_backing: TaskStaticBacking,
    holders: usize,
) -> bool {
    let Some(static_size_bits) = static_backing
        .reserved_bytes
        .is_power_of_two()
        .then(|| static_backing.reserved_bytes.trailing_zeros() as usize)
    else {
        return holders == 0;
    };
    let Some(data_bytes) = plan.reserved_bytes.checked_sub(plan.page_table_bytes) else {
        return false;
    };
    if !plan.page_table_bytes.is_multiple_of(GRANULE_BYTES) {
        return false;
    }
    let table_extents = plan.page_table_bytes / GRANULE_BYTES;
    let data_size_bits = if plan.data_extents == 0 {
        if data_bytes != 0 {
            return false;
        }
        None
    } else {
        if !data_bytes.is_multiple_of(plan.data_extents) {
            return false;
        }
        let bytes = data_bytes / plan.data_extents;
        if !bytes.is_power_of_two() {
            return false;
        }
        Some(bytes.trailing_zeros() as usize)
    };
    if 1usize
        .checked_add(table_extents)
        .and_then(|count| count.checked_add(plan.data_extents))
        != Some(plan.extent_descriptors)
    {
        return false;
    }
    for _ in 0..holders {
        if !plan_extent_in_regions(regions, static_size_bits) {
            return false;
        }
        for _ in 0..table_extents {
            if !plan_extent_in_regions(regions, GRANULE_BYTES.trailing_zeros() as usize) {
                return false;
            }
        }
        if let Some(size_bits) = data_size_bits {
            for _ in 0..plan.data_extents {
                if !plan_extent_in_regions(regions, size_bits) {
                    return false;
                }
            }
        }
    }
    true
}

/// Pure task-arena sizing model, shared by admission and host tests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArenaPlan {
    watermark: usize,
    allocations: usize,
}

impl ArenaPlan {
    pub const fn new() -> Self {
        Self {
            watermark: 0,
            allocations: 0,
        }
    }

    pub fn add_size_bits(&mut self, size_bits: usize) -> Option<()> {
        let size = 1usize.checked_shl(u32::try_from(size_bits).ok()?)?;
        let start = self.watermark.checked_next_multiple_of(size)?;
        self.watermark = start.checked_add(size)?;
        self.allocations = self.allocations.checked_add(1)?;
        Some(())
    }

    pub fn add(&mut self, blueprint: sel4::ObjectBlueprint) -> Option<()> {
        self.add_size_bits(blueprint.physical_size_bits())
    }

    pub const fn required_bytes(self) -> usize {
        self.watermark
    }
    pub const fn allocation_count(self) -> usize {
        self.allocations
    }

    /// Extend this plan to an already computed worst-case watermark.
    ///
    /// Alternative allocation sequences cannot both be replayed into one bump
    /// plan. Their maximum end offset is the exact reservation the arena must
    /// cover, so callers model each sequence on a copy and retain the larger.
    pub fn reserve_to(&mut self, watermark: usize) {
        self.watermark = self.watermark.max(watermark);
    }
    pub fn required_size_bits(self) -> Option<usize> {
        let bytes = self.watermark.max(1);
        Some(usize::BITS as usize - bytes.saturating_sub(1).leading_zeros() as usize)
    }
}
/// Capacity report for a quota-backed task without allocating kernel objects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskBackingPlan {
    pub private_pages: usize,
    pub data_extents: usize,
    pub extent_descriptors: usize,
    pub allocation_descriptors: usize,
    pub required_cslots: usize,
    pub reserved_bytes: usize,
    pub payload_bytes: usize,
    pub page_table_bytes: usize,
    pub alignment_waste: usize,
}

/// Static arena cost already materialized for one exemplar task.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskStaticBacking {
    pub allocation_descriptors: usize,
    pub reserved_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskBackingRequirements {
    pub cslots: usize,
    pub allocation_descriptors: usize,
    pub extent_descriptors: usize,
    pub reserved_bytes: usize,
}

/// Hypothetical holder capacity against the root's current live resource state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskBackingCapacity {
    pub plan: TaskBackingPlan,
    pub static_backing: TaskStaticBacking,
    pub holders: usize,
    pub cslots_available: usize,
    pub allocation_descriptors_available: usize,
    pub extent_descriptors_available: usize,
    pub ordinary_bytes_available: usize,
    pub ordinary_layout_fits: bool,
    pub root_image_bytes: usize,
    pub root_stack_bytes: usize,
    pub root_heap_bytes: usize,
}

impl TaskBackingCapacity {
    pub fn requirements(self) -> Option<TaskBackingRequirements> {
        if self.holders == 0 {
            return Some(TaskBackingRequirements {
                cslots: 0,
                allocation_descriptors: 0,
                extent_descriptors: 0,
                reserved_bytes: 0,
            });
        }
        Some(TaskBackingRequirements {
            cslots: self
                .plan
                .required_cslots
                .checked_add(self.static_backing.allocation_descriptors)?
                .checked_mul(self.holders)?,
            allocation_descriptors: self
                .plan
                .allocation_descriptors
                .checked_add(self.static_backing.allocation_descriptors)?
                .checked_mul(self.holders)?,
            extent_descriptors: self.plan.extent_descriptors.checked_mul(self.holders)?,
            reserved_bytes: self
                .plan
                .reserved_bytes
                .checked_add(self.static_backing.reserved_bytes)?
                .checked_mul(self.holders)?,
        })
    }

    pub fn fits(self) -> bool {
        self.requirements().is_some_and(|required| {
            required.cslots <= self.cslots_available
                && required.allocation_descriptors <= self.allocation_descriptors_available
                && required.extent_descriptors <= self.extent_descriptors_available
                && required.reserved_bytes <= self.ordinary_bytes_available
                && self.ordinary_layout_fits
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PrivateExtentSpec {
    kind: ExtentKind,
    size_bits: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PrivateBackingLayout {
    private_pages: usize,
    pub(crate) allocation_descriptors: usize,
    extents: [Option<PrivateExtentSpec>; 2],
}

impl PrivateBackingLayout {
    /// Root CSlots this layout consumes beyond its allocation descriptors.
    ///
    /// Each extent retains its parent untyped in a root CSlot
    /// ([`ObjectAllocator::provision_extent`]), so admission that counted only
    /// descriptors would pass a plan that later fails staging with
    /// `SlotsExhausted` by exactly this margin.
    pub(crate) fn extent_parent_slots(&self) -> usize {
        self.extents.iter().flatten().count()
    }

    pub(crate) fn for_quota(quota: usize) -> Self {
        let private_pages = quota.min(crate::private_memory::MAX_REGION_PAGES);
        if private_pages == 0 {
            return Self {
                private_pages,
                allocation_descriptors: 0,
                extents: [None; 2],
            };
        }
        let data_bytes = private_pages * GRANULE_BYTES;
        let data_bits =
            usize::BITS as usize - data_bytes.saturating_sub(1).leading_zeros() as usize;
        let data = Some(PrivateExtentSpec {
            kind: ExtentKind::PrivateData,
            size_bits: data_bits,
        });
        Self {
            private_pages,
            allocation_descriptors: if private_pages == MAX_PRIVATE_EXTENT_PAGES {
                private_pages + 2
            } else {
                private_pages + 1
            },
            extents: [
                Some(PrivateExtentSpec {
                    kind: ExtentKind::PrivateTables,
                    size_bits: GRANULE_BYTES.trailing_zeros() as usize,
                }),
                data,
            ],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PrivateBackingRequest {
    Extent { kind: ExtentKind, size_bits: usize },
    Slots { count: usize },
}

fn provision_private_backing_with(
    quota: usize,
    mut apply: impl FnMut(PrivateBackingRequest) -> Result<(), AllocError>,
) -> Result<(), AllocError> {
    let layout = PrivateBackingLayout::for_quota(quota);
    for extent in layout.extents.into_iter().flatten() {
        apply(PrivateBackingRequest::Extent {
            kind: extent.kind,
            size_bits: extent.size_bits,
        })?;
    }
    apply(PrivateBackingRequest::Slots {
        count: layout.allocation_descriptors,
    })
}

/// Plan segmented private backing through [`MAX_PLANNED_PRIVATE_PAGES`],
/// independently of the lower public runtime quota ceiling.
///
/// Data is split into independently reclaimable 2 MiB extents; the one-page
/// allocation shape owns one CSlot/allocation descriptor per page plus one leaf
/// table per 2 MiB span.
pub fn plan_task_backing(private_pages: usize) -> Option<TaskBackingPlan> {
    if private_pages > MAX_PLANNED_PRIVATE_PAGES {
        return None;
    }
    if private_pages <= crate::private_memory::MAX_REGION_PAGES {
        let layout = PrivateBackingLayout::for_quota(private_pages);
        let mut data_extents = 0;
        let mut reserved_data = 0usize;
        let mut page_table_bytes = 0usize;
        let mut private_extents = 0;
        for extent in layout.extents.into_iter().flatten() {
            private_extents += 1;
            let bytes = 1usize.checked_shl(u32::try_from(extent.size_bits).ok()?)?;
            match extent.kind {
                ExtentKind::PrivateData => {
                    data_extents += 1;
                    reserved_data = reserved_data.checked_add(bytes)?;
                }
                ExtentKind::PrivateTables => {
                    page_table_bytes = page_table_bytes.checked_add(bytes)?;
                }
                ExtentKind::Static => unreachable!(),
            }
        }
        let payload_bytes = layout.private_pages.checked_mul(GRANULE_BYTES)?;
        let extent_descriptors = 1 + private_extents;
        return Some(TaskBackingPlan {
            private_pages,
            data_extents,
            extent_descriptors,
            allocation_descriptors: layout.allocation_descriptors,
            required_cslots: layout
                .allocation_descriptors
                .checked_add(extent_descriptors)?,
            reserved_bytes: reserved_data.checked_add(page_table_bytes)?,
            payload_bytes,
            page_table_bytes,
            alignment_waste: reserved_data.checked_sub(payload_bytes)?,
        });
    }
    let payload_extents = private_pages.div_ceil(MAX_PRIVATE_EXTENT_PAGES);
    // An unmapped, reusable large frame is revoked before its dedicated extent
    // is reused for base pages; both shapes consume the same reserved RAM.
    let data_extents = payload_extents;
    let leaf_tables = payload_extents;
    let large_frames = payload_extents;
    let allocation_descriptors = private_pages
        .checked_add(leaf_tables)?
        .checked_add(large_frames)?;
    let extent_descriptors = 1usize.checked_add(data_extents)?.checked_add(leaf_tables)?;
    let payload_bytes = private_pages.checked_mul(GRANULE_BYTES)?;
    let page_table_bytes = leaf_tables.checked_mul(GRANULE_BYTES)?;
    let reserved_data = data_extents.checked_mul(MAX_PRIVATE_EXTENT_BYTES)?;
    let reserved_bytes = reserved_data.checked_add(page_table_bytes)?;
    Some(TaskBackingPlan {
        private_pages,
        data_extents,
        extent_descriptors,
        allocation_descriptors,
        required_cslots: allocation_descriptors.checked_add(extent_descriptors)?,
        reserved_bytes,
        payload_bytes,
        page_table_bytes,
        alignment_waste: reserved_data.checked_sub(payload_bytes)?,
    })
}

impl Default for ArenaPlan {
    fn default() -> Self {
        Self::new()
    }
}

/// One live root CSlot's allocation-time physical base.
///
/// `slot` doubles as the occupancy flag. [`ProvenanceTable::EMPTY`] is
/// `usize::MAX`, which no root CSlot index can be — [`SlotPool::new`] refuses a
/// BootInfo span wider than [`MAX_ROOT_CSLOTS`] — so the sentinel costs no
/// discriminant word, and the record stays two words rather than the three an
/// `Option` would take.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProvenanceEntry {
    slot: usize,
    paddr: usize,
}

/// Allocation-time physical provenance for the frames a device may be handed.
///
/// Open-addressed on the root CSlot index with linear probing, sized by
/// [`MAX_PHYSICAL_PROVENANCE`] rather than by the CSpace. Deletion shifts the
/// probe chain back rather than leaving a tombstone, so a root that allocates
/// and releases DMA frames indefinitely never degrades: the table's cost is its
/// live population, not its history.
struct ProvenanceTable {
    entries: [ProvenanceEntry; PROVENANCE_SLOTS],
    len: usize,
}

impl ProvenanceTable {
    const EMPTY: usize = usize::MAX;
    const MASK: usize = PROVENANCE_SLOTS - 1;

    const fn new() -> Self {
        Self {
            entries: [ProvenanceEntry {
                slot: Self::EMPTY,
                paddr: 0,
            }; PROVENANCE_SLOTS],
            len: 0,
        }
    }

    const fn home(slot: usize) -> usize {
        slot & Self::MASK
    }

    const fn step(probe: usize) -> usize {
        (probe + 1) & Self::MASK
    }

    /// Cyclic probe distance from a chain's start to one of its positions.
    const fn distance(home: usize, probe: usize) -> usize {
        probe.wrapping_sub(home) & Self::MASK
    }

    fn position(&self, slot: usize) -> Option<usize> {
        let mut probe = Self::home(slot);
        // Bounded by the table rather than by reaching an empty position, so a
        // corrupted chain is a miss instead of a hang.
        for _ in 0..PROVENANCE_SLOTS {
            let entry = self.entries[probe];
            if entry.slot == Self::EMPTY {
                return None;
            }
            if entry.slot == slot {
                return Some(probe);
            }
            probe = Self::step(probe);
        }
        None
    }

    fn get(&self, slot: usize) -> Option<usize> {
        self.position(slot).map(|probe| self.entries[probe].paddr)
    }

    /// Record `slot`'s physical base, or refuse when the table is at its bound.
    ///
    /// Refusing is the whole point: a frame whose physical address the root
    /// cannot recover must fail its allocation, never reach a device descriptor
    /// carrying some other frame's address. Re-recording a slot already present
    /// overwrites in place and cannot fail, so a slot the pool reissued after a
    /// release that did not reach here still ends up with its current base.
    fn insert(&mut self, slot: usize, paddr: usize) -> Result<(), AllocError> {
        let mut probe = Self::home(slot);
        // Terminates: `len <= MAX_PHYSICAL_PROVENANCE < PROVENANCE_SLOTS`, so
        // some position is always empty.
        loop {
            let entry = self.entries[probe];
            if entry.slot == Self::EMPTY {
                break;
            }
            if entry.slot == slot {
                self.entries[probe].paddr = paddr;
                return Ok(());
            }
            probe = Self::step(probe);
        }
        if self.len >= MAX_PHYSICAL_PROVENANCE {
            return Err(AllocError::ProvenanceTableFull {
                limit: MAX_PHYSICAL_PROVENANCE,
            });
        }
        self.entries[probe] = ProvenanceEntry { slot, paddr };
        self.len += 1;
        Ok(())
    }

    /// Forget `slot`'s physical base, reporting whether one was held.
    ///
    /// The vacated position is closed by pulling back every following entry
    /// whose own chain runs through it, which keeps [`Self::position`]'s
    /// stop-at-empty walk correct without tombstones.
    fn remove(&mut self, slot: usize) -> bool {
        let Some(mut hole) = self.position(slot) else {
            return false;
        };
        self.entries[hole].slot = Self::EMPTY;
        self.len -= 1;
        let mut probe = Self::step(hole);
        loop {
            let entry = self.entries[probe];
            if entry.slot == Self::EMPTY {
                return true;
            }
            let home = Self::home(entry.slot);
            // Movable exactly when a walk from this entry's own chain start
            // reaches the hole before it reaches the entry: then filling the
            // hole keeps the entry findable, and leaving it would strand it
            // behind the empty position.
            if Self::distance(home, hole) < Self::distance(home, probe) {
                self.entries[hole] = entry;
                self.entries[probe].slot = Self::EMPTY;
                hole = probe;
            }
            probe = Self::step(probe);
        }
    }
}

/// Whether an object of this shape can be handed to a device, and therefore
/// needs its physical base retained.
///
/// A base page of ordinary RAM and nothing else: a TCB, endpoint, CNode or
/// translation table has no physical identity any caller of
/// [`ObjectAllocator::physical_address_of`] can use, and admitting them would
/// spend the bounded table on records no one reads — which would then refuse a
/// frame that genuinely needed one.
fn records_provenance(blueprint: sel4::ObjectBlueprint) -> bool {
    blueprint == <sel4::cap_type::Granule as sel4::CapTypeForObjectOfFixedSize>::object_blueprint()
}
struct SlotPool {
    base: usize,
    len: usize,
    used: [usize; SLOT_WORDS],
    issued: [usize; SLOT_WORDS],
    live: usize,
}

impl SlotPool {
    const EMPTY: Self = Self {
        base: 0,
        len: 0,
        used: [0; SLOT_WORDS],
        issued: [0; SLOT_WORDS],
        live: 0,
    };

    fn initialize(&mut self, range: Range<usize>) -> Result<(), AllocError> {
        let len = range.end.saturating_sub(range.start);
        if len > MAX_ROOT_CSLOTS {
            return Err(AllocError::SlotRangeTooLarge {
                declared: len,
                limit: MAX_ROOT_CSLOTS,
            });
        }
        self.base = range.start;
        self.len = len;
        Ok(())
    }

    fn new(range: Range<usize>) -> Result<Self, AllocError> {
        let mut pool = Self::EMPTY;
        pool.initialize(range)?;
        Ok(pool)
    }

    /// Slots this pool can still issue.
    fn free(&self) -> usize {
        self.len - self.live
    }

    fn allocate(&mut self, total_allocated: usize) -> Result<(usize, bool), AllocError> {
        for offset in 0..self.len {
            let word = offset / SLOT_WORD_BITS;
            let mask = 1usize << (offset % SLOT_WORD_BITS);
            if self.used[word] & mask == 0 {
                let reused = self.issued[word] & mask != 0;
                self.used[word] |= mask;
                self.issued[word] |= mask;
                self.live += 1;
                return Ok((self.base + offset, reused));
            }
        }
        Err(AllocError::SlotsExhausted {
            allocated: total_allocated,
        })
    }

    fn first_contiguous(&self, count: usize, extra_used: Option<usize>) -> Option<usize> {
        if count == 0 || count > self.len {
            return None;
        }
        (0..=self.len - count).find_map(|start| {
            let clear = (start..start + count).all(|offset| {
                self.used[offset / SLOT_WORD_BITS] & (1usize << (offset % SLOT_WORD_BITS)) == 0
                    && extra_used != Some(self.base + offset)
            });
            clear.then_some(self.base + start)
        })
    }

    fn allocate_contiguous(
        &mut self,
        count: usize,
        total_allocated: usize,
    ) -> Result<(usize, usize), AllocError> {
        if count == 0 || count > self.len {
            return Err(AllocError::SlotsExhausted {
                allocated: total_allocated,
            });
        }
        for start in 0..=self.len - count {
            if (start..start + count).any(|offset| {
                self.used[offset / SLOT_WORD_BITS] & (1usize << (offset % SLOT_WORD_BITS)) != 0
            }) {
                continue;
            }
            let mut reused = 0;
            for offset in start..start + count {
                let word = offset / SLOT_WORD_BITS;
                let mask = 1usize << (offset % SLOT_WORD_BITS);
                reused += usize::from(self.issued[word] & mask != 0);
                self.used[word] |= mask;
                self.issued[word] |= mask;
            }
            self.live += count;
            return Ok((self.base + start, reused));
        }
        Err(AllocError::SlotsExhausted {
            allocated: total_allocated,
        })
    }

    fn release(&mut self, slot: usize) -> bool {
        let Some(offset) = slot
            .checked_sub(self.base)
            .filter(|offset| *offset < self.len)
        else {
            return false;
        };
        let word = offset / SLOT_WORD_BITS;
        let mask = 1usize << (offset % SLOT_WORD_BITS);
        if self.used[word] & mask == 0 {
            return false;
        }
        self.used[word] &= !mask;
        self.live -= 1;
        true
    }

    const fn remaining(&self) -> usize {
        self.len - self.live
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskArenaId {
    index: u16,
    serial: u32,
}

impl TaskArenaId {
    pub const fn index(self) -> usize {
        self.index as usize
    }
    pub(crate) const fn from_raw(index: u16, serial: u32) -> Self {
        Self { index, serial }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrivateObjectKind {
    Empty = 0,
    Granule = 1,
    LargeFrame = 2,
    LeafTable = 3,
}

const PRIVATE_STATE_NONE: u32 = u32::MAX;
const PRIVATE_EXTENT_NONE: u16 = u16::MAX;

pub(crate) struct PrivateAllocation {
    arena: TaskArenaId,
    position: u32,
    cap: sel4::cap::Unspecified,
    mapped: bool,
}

impl PrivateAllocation {
    pub(crate) const fn cap(&self) -> sel4::cap::Unspecified {
        self.cap
    }

    pub(crate) const fn mapped(&self) -> bool {
        self.mapped
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ArenaAllocation(u32);

impl ArenaAllocation {
    const EMPTY: Self = Self(u32::MAX);
    const SLOT_BITS: u32 = MAX_ROOT_CSLOTS.trailing_zeros();
    const SLOT_MASK: u32 = (1 << Self::SLOT_BITS) - 1;
    const SIZE_SHIFT: u32 = Self::SLOT_BITS;
    const SIZE_MASK: u32 = 0x3f << Self::SIZE_SHIFT;
    const PRIVATE: u32 = 1 << 25;
    const REUSABLE: u32 = 1 << 26;
    const KIND_SHIFT: u32 = 27;
    const KIND_MASK: u32 = 0x3 << Self::KIND_SHIFT;
    const MAPPED: u32 = 1 << 29;
    const IN_FLIGHT: u32 = 1 << 30;

    fn new(slot: usize, size_bits: usize, private: bool, reusable: bool) -> Self {
        debug_assert!(slot < MAX_ROOT_CSLOTS);
        debug_assert!(size_bits < 64);
        Self(
            slot as u32
                | (size_bits as u32) << Self::SIZE_SHIFT
                | if private { Self::PRIVATE } else { 0 }
                | if reusable { Self::REUSABLE } else { 0 },
        )
    }

    const fn slot(self) -> usize {
        (self.0 & Self::SLOT_MASK) as usize
    }

    const fn size_bits(self) -> usize {
        ((self.0 & Self::SIZE_MASK) >> Self::SIZE_SHIFT) as usize
    }

    const fn is_private(self) -> bool {
        self.0 & Self::PRIVATE != 0
    }

    const fn is_reusable(self) -> bool {
        self.0 & Self::REUSABLE != 0
    }

    const fn private_kind(self) -> PrivateObjectKind {
        match (self.0 & Self::KIND_MASK) >> Self::KIND_SHIFT {
            1 => PrivateObjectKind::Granule,
            2 => PrivateObjectKind::LargeFrame,
            3 => PrivateObjectKind::LeafTable,
            _ => PrivateObjectKind::Empty,
        }
    }

    const fn is_mapped(self) -> bool {
        self.0 & Self::MAPPED != 0
    }

    const fn is_in_flight(self) -> bool {
        self.0 & Self::IN_FLIGHT != 0
    }

    fn set_in_flight(&mut self, in_flight: bool) {
        if in_flight {
            self.0 |= Self::IN_FLIGHT;
        } else {
            self.0 &= !Self::IN_FLIGHT;
        }
    }

    fn set_private_state(
        &mut self,
        kind: PrivateObjectKind,
        size_bits: usize,
        reusable: bool,
        mapped: bool,
    ) {
        self.0 = (self.0
            & !(Self::SIZE_MASK
                | Self::KIND_MASK
                | Self::REUSABLE
                | Self::MAPPED
                | Self::IN_FLIGHT))
            | (size_bits as u32) << Self::SIZE_SHIFT
            | (kind as u32) << Self::KIND_SHIFT
            | if reusable { Self::REUSABLE } else { 0 }
            | if mapped { Self::MAPPED } else { 0 };
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExtentKind {
    Static,
    PrivateData,
    PrivateTables,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ExtentRecord {
    parent: sel4::cap::Untyped,
    size_bits: usize,
    owner: u16,
    serial: u32,
    kind: ExtentKind,
    active: bool,
    revoked: bool,
    watermark: usize,
    objects: usize,
    bytes: usize,
}

impl ExtentRecord {
    fn new(parent: sel4::cap::Untyped, size_bits: usize) -> Self {
        Self {
            parent,
            size_bits,
            owner: u16::MAX,
            serial: 0,
            kind: ExtentKind::Static,
            active: false,
            revoked: false,
            watermark: 0,
            objects: 0,
            bytes: 0,
        }
    }

    fn assign(&mut self, owner: usize, serial: u32, kind: ExtentKind) {
        self.owner = owner as u16;
        self.serial = serial;
        self.kind = kind;
        self.active = true;
        self.revoked = false;
        self.watermark = 0;
        self.objects = 0;
        self.bytes = 0;
    }

    const fn belongs_to(&self, id: TaskArenaId) -> bool {
        self.active && self.owner as usize == id.index() && self.serial == id.serial
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AllocationRecord {
    owner: u16,
    extent: u16,
    serial: u32,
    allocation: ArenaAllocation,
    next_state: u32,
}

impl AllocationRecord {
    const EMPTY: Self = Self {
        owner: u16::MAX,
        extent: PRIVATE_EXTENT_NONE,
        serial: 0,
        allocation: ArenaAllocation::EMPTY,
        next_state: PRIVATE_STATE_NONE,
    };

    const fn belongs_to(&self, id: TaskArenaId) -> bool {
        self.owner as usize == id.index() && self.serial == id.serial
    }
}

fn task_static_backing_from_records(
    id: TaskArenaId,
    allocations: &[AllocationRecord],
    extents: &[Option<ExtentRecord>],
) -> Option<TaskStaticBacking> {
    let allocation_descriptors = allocations
        .iter()
        .filter(|record| record.belongs_to(id) && !record.allocation.is_private())
        .count();
    let mut matching = extents.iter().flatten().filter(|extent| {
        extent.belongs_to(id) && extent.kind == ExtentKind::Static && !extent.revoked
    });
    let extent = matching.next()?;
    if matching.next().is_some() {
        return None;
    }
    Some(TaskStaticBacking {
        allocation_descriptors,
        reserved_bytes: 1usize.checked_shl(u32::try_from(extent.size_bits).ok()?)?,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ArenaRecord {
    serial: u32,
    active: bool,
    slot_len: usize,
    reusable_heads: [u32; 4],
    in_flight_head: u32,
    in_flight_granules: usize,
    releasing: bool,
}

impl ArenaRecord {
    const fn empty() -> Self {
        Self {
            serial: 0,
            active: false,
            slot_len: 0,
            reusable_heads: [PRIVATE_STATE_NONE; 4],
            in_flight_head: PRIVATE_STATE_NONE,
            in_flight_granules: 0,
            releasing: false,
        }
    }

    const fn id(&self, index: usize) -> TaskArenaId {
        TaskArenaId {
            index: index as u16,
            serial: self.serial,
        }
    }
}

const _: () = assert!(
    MAX_ROOT_CSLOTS.is_power_of_two()
        && ArenaAllocation::SLOT_BITS + 6 <= 25
        && MAX_TASK_ALLOCATIONS < u32::MAX as usize
        && MAX_TASK_EXTENTS < u16::MAX as usize
);
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PrivateRecordVisits {
    reusable: usize,
    mark: usize,
    reset: usize,
    commit: usize,
    unwind: usize,
}

pub struct ObjectAllocator {
    slots: SlotPool,
    untypeds: [Option<UntypedRegion>; MAX_KERNEL_UNTYPEDS],
    untyped_len: usize,
    devices: [Option<DeviceRegion>; MAX_DEVICE_UNTYPEDS],
    device_len: usize,
    arenas: [ArenaRecord; MAX_TASK_ARENAS],
    extents: [Option<ExtentRecord>; MAX_TASK_EXTENTS],
    allocations: [AllocationRecord; MAX_TASK_ALLOCATIONS],
    next_arena_serial: u32,
    slots_allocated: usize,
    objects_allocated: usize,
    bytes_allocated: usize,
    live_objects: usize,
    live_bytes: usize,
    slots_reused: usize,
    extents_reused: usize,
    last_paddr: usize,
    physical: ProvenanceTable,
    #[cfg(test)]
    private_visits: PrivateRecordVisits,
}

impl ObjectAllocator {
    pub const fn empty() -> Self {
        Self {
            slots: SlotPool::EMPTY,
            untypeds: [None; MAX_KERNEL_UNTYPEDS],
            untyped_len: 0,
            devices: [None; MAX_DEVICE_UNTYPEDS],
            device_len: 0,
            arenas: [ArenaRecord::empty(); MAX_TASK_ARENAS],
            extents: [None; MAX_TASK_EXTENTS],
            allocations: [AllocationRecord::EMPTY; MAX_TASK_ALLOCATIONS],
            next_arena_serial: 1,
            slots_allocated: 0,
            objects_allocated: 0,
            bytes_allocated: 0,
            live_objects: 0,
            live_bytes: 0,
            slots_reused: 0,
            extents_reused: 0,
            last_paddr: 0,
            physical: ProvenanceTable::new(),
            #[cfg(test)]
            private_visits: PrivateRecordVisits {
                reusable: 0,
                mark: 0,
                reset: 0,
                commit: 0,
                unwind: 0,
            },
        }
    }

    pub fn initialize(&mut self, bootinfo: &sel4::BootInfo) -> Result<(), AllocError> {
        let kernel_untypeds = bootinfo.kernel_untyped_range();
        let declared = kernel_untypeds.len();
        if declared > MAX_KERNEL_UNTYPEDS {
            return Err(AllocError::UntypedTableFull {
                limit: MAX_KERNEL_UNTYPEDS,
                declared,
            });
        }
        self.slots.initialize(bootinfo.empty().range())?;
        let descriptors = bootinfo.untyped_list();
        for index in kernel_untypeds {
            let Some(descriptor) = descriptors.get(index) else {
                continue;
            };
            if descriptor.is_device() {
                continue;
            }
            self.untypeds[self.untyped_len] = Some(UntypedRegion {
                cap: bootinfo.untyped().index(index).cap(),
                paddr: descriptor.paddr(),
                size_bits: descriptor.size_bits(),
                watermark: 0,
            });
            self.untyped_len += 1;
        }
        if self.untyped_len == 0 {
            return Err(AllocError::NoKernelUntyped);
        }

        let device_untypeds = bootinfo.device_untyped_range();
        let device_declared = device_untypeds.len();
        for index in device_untypeds {
            let Some(descriptor) = descriptors.get(index) else {
                continue;
            };
            let Some(dst) = self.devices.get_mut(self.device_len) else {
                return Err(AllocError::DeviceTableFull {
                    limit: MAX_DEVICE_UNTYPEDS,
                    declared: device_declared,
                });
            };
            *dst = Some(DeviceRegion {
                cap: bootinfo.untyped().index(index).cap(),
                paddr: descriptor.paddr(),
                size_bits: descriptor.size_bits(),
                retyped: 0,
            });
            self.device_len += 1;
        }
        Ok(())
    }

    pub const fn slots_remaining(&self) -> usize {
        self.slots.remaining()
    }
    pub const fn slots_allocated(&self) -> usize {
        self.slots_allocated
    }
    pub const fn live_slots(&self) -> usize {
        self.slots.live
    }
    pub const fn slots_reused(&self) -> usize {
        self.slots_reused
    }
    pub const fn objects_allocated(&self) -> usize {
        self.objects_allocated
    }
    pub const fn live_objects(&self) -> usize {
        self.live_objects
    }
    pub const fn bytes_allocated(&self) -> usize {
        self.bytes_allocated
    }
    pub const fn live_bytes(&self) -> usize {
        self.live_bytes
    }
    pub const fn extents_reused(&self) -> usize {
        self.extents_reused
    }

    pub fn reusable_extent_bytes(&self) -> usize {
        self.extents
            .iter()
            .flatten()
            .filter(|extent| !extent.active)
            .map(|extent| 1usize << extent.size_bits)
            .sum()
    }

    /// [`Self::reusable_extent_bytes`], restricted to private-backing extents.
    ///
    /// The unrestricted total is satisfied by any inactive extent, including
    /// the one every task carries for its static construction; a private
    /// holder whose data or table extents never returned to the free list
    /// would still read as nonzero reclamation. This is the kind-scoped
    /// evidence that a private extent specifically was reclaimed rather than
    /// leaked.
    pub fn reusable_private_extent_bytes(&self) -> usize {
        self.extents
            .iter()
            .flatten()
            .filter(|extent| {
                !extent.active
                    && matches!(
                        extent.kind,
                        ExtentKind::PrivateData | ExtentKind::PrivateTables
                    )
            })
            .map(|extent| 1usize << extent.size_bits)
            .sum()
    }
    pub const fn untyped_count(&self) -> usize {
        self.untyped_len
    }
    pub const fn device_untyped_count(&self) -> usize {
        self.device_len
    }
    /// Physical base retained for a live root CSlot allocated from ordinary RAM.
    /// This root-only mechanism seam is public because the binary links the
    /// allocator through the library crate; no component ABI exposes it.
    ///
    /// `None` for a slot that never held a DMA-capable frame, and for one whose
    /// frame has been released: a caller reaching for an address the root no
    /// longer owns must be refused, not answered from a stale record.
    pub fn physical_address_of(&self, slot: usize) -> Option<usize> {
        self.physical.get(slot)
    }

    /// Live entries in the bounded physical-provenance table.
    pub const fn physical_provenance_len(&self) -> usize {
        self.physical.len
    }

    pub fn untyped_bytes_remaining(&self) -> usize {
        self.regions()
            .iter()
            .flatten()
            .map(UntypedRegion::remaining)
            .sum()
    }

    /// Whether the current ordinary untyped regions can place every planned
    /// extent in provisioning order under seL4's object-size alignment rule.
    pub fn task_backing_extents_fit(
        &self,
        plan: TaskBackingPlan,
        static_backing: TaskStaticBacking,
        holders: usize,
    ) -> bool {
        let mut regions = self.untypeds;
        task_backing_extents_fit_in(
            &mut regions[..self.untyped_len],
            plan,
            static_backing,
            holders,
        )
    }

    fn take_slot(&mut self) -> Result<usize, AllocError> {
        let (slot, reused) = self.slots.allocate(self.slots_allocated)?;
        self.slots_reused += usize::from(reused);
        self.slots_allocated += 1;
        Ok(slot)
    }

    fn allocate_from_global(
        &mut self,
        blueprint: sel4::ObjectBlueprint,
        slot_index: usize,
    ) -> Result<(), AllocError> {
        let size_bits = blueprint.physical_size_bits();
        let (region_index, start, watermark) = self
            .untypeds
            .iter()
            .take(self.untyped_len)
            .enumerate()
            .find_map(|(index, region)| {
                let region = region.as_ref()?;
                plan_allocation(region.watermark, region.capacity(), size_bits)
                    .map(|(start, end)| (index, start, end))
            })
            .ok_or(AllocError::UntypedExhausted {
                size_bits,
                remaining: self.untyped_bytes_remaining(),
            })?;
        let region = self.untypeds[region_index].ok_or(AllocError::UntypedExhausted {
            size_bits,
            remaining: 0,
        })?;
        let paddr = region.paddr.saturating_add(start);
        // Recorded *before* the retype, so an exhausted table refuses the
        // allocation while it is still free to refuse: the slot is not yet
        // occupied, so releasing it back to the pool leaves nothing behind. The
        // retype's own failure path undoes the record below. Recording
        // afterwards would mean either a live capability with no recoverable
        // physical base, or a slot released while still holding one — the
        // `DeleteFirst` hazard [`Self::release_slot`] documents.
        let recorded = records_provenance(blueprint);
        if recorded {
            self.physical.insert(slot_index, paddr)?;
        }
        if let Err(error) = region.cap.untyped_retype(
            &blueprint,
            &sel4::init_thread::slot::CNODE
                .cap()
                .absolute_cptr_for_self(),
            slot_index,
            1,
        ) {
            if recorded {
                self.physical.remove(slot_index);
            }
            return Err(AllocError::Retype { size_bits, error });
        }
        if let Some(region) = self.untypeds[region_index].as_mut() {
            region.watermark = watermark;
        }
        self.last_paddr = paddr;
        self.objects_allocated += 1;
        self.live_objects += 1;
        self.bytes_allocated += 1usize << size_bits;
        self.live_bytes += 1usize << size_bits;
        Ok(())
    }

    pub fn allocate(
        &mut self,
        blueprint: sel4::ObjectBlueprint,
    ) -> Result<sel4::init_thread::Slot<sel4::cap_type::Unspecified>, AllocError> {
        let slot = self.take_slot()?;
        if let Err(error) = self.allocate_from_global(blueprint, slot) {
            self.slots.release(slot);
            return Err(error);
        }
        Ok(sel4::init_thread::Slot::from_index(slot))
    }

    pub fn allocate_fixed<T: sel4::CapTypeForObjectOfFixedSize>(
        &mut self,
    ) -> Result<sel4::init_thread::Slot<T>, AllocError> {
        Ok(self.allocate(T::object_blueprint())?.cast())
    }

    /// Retype `count` adjacent base pages from one ordinary untyped in one
    /// kernel operation. This is the provenance required by legacy virtqueues,
    /// whose PFN names one physically contiguous queue area.
    pub fn allocate_contiguous_granules(
        &mut self,
        count: usize,
    ) -> Result<(usize, usize), AllocError> {
        if count == 0 || count > MAX_CONTIGUOUS_GRANULES {
            return Err(AllocError::SlotsExhausted {
                allocated: self.slots_allocated,
            });
        }
        let blueprint =
            <sel4::cap_type::Granule as sel4::CapTypeForObjectOfFixedSize>::object_blueprint();
        let size_bits = blueprint.physical_size_bits();
        let page_bytes = 1usize << size_bits;
        let total = page_bytes
            .checked_mul(count)
            .ok_or(AllocError::UntypedExhausted {
                size_bits,
                remaining: self.untyped_bytes_remaining(),
            })?;
        let (region_index, start, watermark) = self
            .untypeds
            .iter()
            .take(self.untyped_len)
            .enumerate()
            .find_map(|(index, region)| {
                let region = region.as_ref()?;
                let start = region.watermark.checked_next_multiple_of(page_bytes)?;
                let end = start.checked_add(total)?;
                (end <= region.capacity()).then_some((index, start, end))
            })
            .ok_or(AllocError::UntypedExhausted {
                size_bits,
                remaining: self.untyped_bytes_remaining(),
            })?;
        let region = self.untypeds[region_index].ok_or(AllocError::UntypedExhausted {
            size_bits,
            remaining: 0,
        })?;
        let (first, reused) = self
            .slots
            .allocate_contiguous(count, self.slots_allocated)?;
        let paddr = region.paddr.saturating_add(start);
        for index in 0..count {
            if let Err(error) = self
                .physical
                .insert(first + index, paddr + index * page_bytes)
            {
                for done in 0..index {
                    self.physical.remove(first + done);
                }
                for slot in first..first + count {
                    self.slots.release(slot);
                }
                return Err(error);
            }
        }
        if let Err(error) = region.cap.untyped_retype(
            &blueprint,
            &sel4::init_thread::slot::CNODE
                .cap()
                .absolute_cptr_for_self(),
            first,
            count,
        ) {
            for slot in first..first + count {
                self.physical.remove(slot);
                self.slots.release(slot);
            }
            return Err(AllocError::Retype { size_bits, error });
        }
        self.untypeds[region_index].as_mut().unwrap().watermark = watermark;
        self.slots_allocated += count;
        self.slots_reused += reused;
        self.objects_allocated += count;
        self.live_objects += count;
        self.bytes_allocated += total;
        self.live_bytes += total;
        self.last_paddr = paddr + (count - 1) * page_bytes;
        Ok((first, paddr))
    }

    pub fn allocate_variable<T: sel4::CapTypeForObjectOfVariableSize>(
        &mut self,
        size_bits: usize,
    ) -> Result<sel4::init_thread::Slot<T>, AllocError> {
        Ok(self.allocate(T::object_blueprint(size_bits))?.cast())
    }

    pub fn reserve_slot<T: sel4::CapType>(
        &mut self,
    ) -> Result<sel4::init_thread::Slot<T>, AllocError> {
        Ok(sel4::init_thread::Slot::from_index(self.take_slot()?))
    }

    /// Begin one task lifetime with an independently reclaimable static extent.
    /// Private quota backing is provisioned separately before construction.
    pub fn begin_task_arena(&mut self, size_bits: usize) -> Result<TaskArenaId, AllocError> {
        let index = self.arenas.iter().position(|arena| !arena.active).ok_or(
            AllocError::ArenaTableFull {
                limit: MAX_TASK_ARENAS,
            },
        )?;
        let serial = self.next_arena_serial;
        self.next_arena_serial = self.next_arena_serial.wrapping_add(1).max(1);
        self.arenas[index] = ArenaRecord {
            serial,
            active: true,
            ..ArenaRecord::empty()
        };
        let id = self.arenas[index].id(index);
        if let Err(error) = self.provision_extent(id, size_bits, ExtentKind::Static) {
            self.arenas[index] = ArenaRecord::empty();
            return Err(error);
        }
        Ok(id)
    }

    fn arena(&self, id: TaskArenaId) -> Result<&ArenaRecord, AllocError> {
        self.arenas
            .get(id.index())
            .filter(|arena| arena.serial == id.serial && arena.active)
            .ok_or(AllocError::UnknownArena(id))
    }

    fn private_arena(&self, id: TaskArenaId) -> Result<&ArenaRecord, AllocError> {
        let arena = self.arena(id)?;
        if arena.releasing {
            Err(AllocError::UnknownArena(id))
        } else {
            Ok(arena)
        }
    }

    pub fn task_static_backing(&self, id: TaskArenaId) -> Option<TaskStaticBacking> {
        self.arena(id).ok()?;
        task_static_backing_from_records(id, &self.allocations, &self.extents)
    }

    fn arena_mut(&mut self, id: TaskArenaId) -> Result<&mut ArenaRecord, AllocError> {
        self.arenas
            .get_mut(id.index())
            .filter(|arena| arena.serial == id.serial && arena.active)
            .ok_or(AllocError::UnknownArena(id))
    }

    fn private_record_error() -> AllocError {
        AllocError::ArenaSlotTableFull {
            limit: MAX_TASK_ALLOCATIONS,
        }
    }

    #[cfg(test)]
    fn reset_private_record_visits(&mut self) {
        self.private_visits = PrivateRecordVisits::default();
    }

    #[cfg(test)]
    fn private_record_visits(&self) -> PrivateRecordVisits {
        self.private_visits
    }

    fn push_reusable(&mut self, id: TaskArenaId, position: usize) -> Result<(), AllocError> {
        let kind = self
            .allocations
            .get(position)
            .filter(|record| {
                record.belongs_to(id)
                    && record.allocation.is_private()
                    && record.allocation.is_reusable()
                    && !record.allocation.is_in_flight()
                    && record.next_state == PRIVATE_STATE_NONE
            })
            .ok_or_else(Self::private_record_error)?
            .allocation
            .private_kind();
        let head = self.private_arena(id)?.reusable_heads[kind as usize];
        self.allocations[position].next_state = head;
        self.arena_mut(id)?.reusable_heads[kind as usize] = position as u32;
        Ok(())
    }

    fn pop_reusable(
        &mut self,
        id: TaskArenaId,
        kind: PrivateObjectKind,
        size_bits: Option<usize>,
    ) -> Result<Option<usize>, AllocError> {
        let mut previous = PRIVATE_STATE_NONE;
        let mut current = self.private_arena(id)?.reusable_heads[kind as usize];
        let limit = self.private_arena(id)?.slot_len;
        for _ in 0..limit {
            if current == PRIVATE_STATE_NONE {
                return Ok(None);
            }
            let position = current as usize;
            #[cfg(test)]
            {
                self.private_visits.reusable += 1;
            }
            let record = self
                .allocations
                .get(position)
                .filter(|record| {
                    record.belongs_to(id)
                        && record.allocation.is_private()
                        && record.allocation.is_reusable()
                        && !record.allocation.is_in_flight()
                        && record.allocation.private_kind() == kind
                })
                .ok_or_else(Self::private_record_error)?;
            let next = record.next_state;
            if size_bits.is_none_or(|expected| record.allocation.size_bits() == expected) {
                if previous == PRIVATE_STATE_NONE {
                    self.arena_mut(id)?.reusable_heads[kind as usize] = next;
                } else {
                    self.allocations[previous as usize].next_state = next;
                }
                self.allocations[position].next_state = PRIVATE_STATE_NONE;
                return Ok(Some(position));
            }
            previous = current;
            current = next;
        }
        Err(Self::private_record_error())
    }

    fn private_record_mut(
        &mut self,
        id: TaskArenaId,
        allocation: &PrivateAllocation,
    ) -> Result<&mut AllocationRecord, AllocError> {
        self.private_arena(id)?;
        if allocation.arena != id {
            return Err(Self::private_record_error());
        }
        self.allocations
            .get_mut(allocation.position as usize)
            .filter(|record| {
                record.belongs_to(id)
                    && record.allocation.is_private()
                    && !record.allocation.is_reusable()
                    && !record.allocation.is_in_flight()
                    && record.allocation.private_kind() != PrivateObjectKind::Empty
                    && record.allocation.slot() == allocation.cap.bits() as usize
                    && record.next_state == PRIVATE_STATE_NONE
            })
            .ok_or_else(Self::private_record_error)
    }

    fn provision_extent(
        &mut self,
        id: TaskArenaId,
        size_bits: usize,
        kind: ExtentKind,
    ) -> Result<usize, AllocError> {
        self.arena(id)?;
        let reusable = self.extents.iter().position(|entry| {
            entry.is_some_and(|extent| !extent.active && extent.size_bits == size_bits)
        });
        let index = if let Some(index) = reusable {
            self.extents_reused += 1;
            index
        } else {
            let index = self.extents.iter().position(Option::is_none).ok_or(
                AllocError::ArenaTableFull {
                    limit: MAX_TASK_EXTENTS,
                },
            )?;
            let parent_slot = self.take_slot()?;
            let blueprint = sel4::ObjectBlueprint::Untyped { size_bits };
            if let Err(error) = self.allocate_from_global(blueprint, parent_slot) {
                self.slots.release(parent_slot);
                return Err(error);
            }
            let parent =
                sel4::init_thread::Slot::<sel4::cap_type::Untyped>::from_index(parent_slot).cap();
            self.extents[index] = Some(ExtentRecord::new(parent, size_bits));
            index
        };
        self.extents[index]
            .as_mut()
            .expect("extent position is provisioned")
            .assign(id.index(), id.serial, kind);
        Ok(index)
    }

    fn allocation_position(&self) -> Result<usize, AllocError> {
        self.allocations
            .iter()
            .position(|record| record.owner == u16::MAX)
            .ok_or(AllocError::ArenaSlotTableFull {
                limit: MAX_TASK_ALLOCATIONS,
            })
    }

    fn push_allocation(
        &mut self,
        id: TaskArenaId,
        extent: Option<usize>,
        allocation: ArenaAllocation,
    ) -> Result<usize, AllocError> {
        let position = self.allocation_position()?;
        self.allocations[position] = AllocationRecord {
            owner: id.index() as u16,
            extent: extent.map_or(PRIVATE_EXTENT_NONE, |index| index as u16),
            serial: id.serial,
            allocation,
            next_state: PRIVATE_STATE_NONE,
        };
        self.arena_mut(id)?.slot_len += 1;
        Ok(position)
    }

    fn extent_for_allocation(
        &self,
        id: TaskArenaId,
        kind: ExtentKind,
        size_bits: usize,
    ) -> Result<(usize, usize), AllocError> {
        self.extents
            .iter()
            .enumerate()
            .filter_map(|(index, extent)| extent.as_ref().map(|extent| (index, extent)))
            .filter(|(_, extent)| extent.belongs_to(id) && extent.kind == kind && !extent.revoked)
            .find_map(|(index, extent)| {
                plan_allocation(extent.watermark, 1usize << extent.size_bits, size_bits)
                    .map(|(_, watermark)| (index, watermark))
            })
            .ok_or(AllocError::ArenaTooSmall {
                size_bits,
                required: 1usize << size_bits,
            })
    }

    pub fn allocate_in(
        &mut self,
        id: TaskArenaId,
        blueprint: sel4::ObjectBlueprint,
    ) -> Result<sel4::init_thread::Slot<sel4::cap_type::Unspecified>, AllocError> {
        self.arena(id)?;
        self.allocation_position()?;
        let size_bits = blueprint.physical_size_bits();
        let (extent_index, watermark) =
            self.extent_for_allocation(id, ExtentKind::Static, size_bits)?;
        let slot = self.take_slot()?;
        let parent = self.extents[extent_index]
            .expect("selected extent exists")
            .parent;
        if let Err(error) = parent.untyped_retype(
            &blueprint,
            &sel4::init_thread::slot::CNODE
                .cap()
                .absolute_cptr_for_self(),
            slot,
            1,
        ) {
            self.slots.release(slot);
            return Err(AllocError::Retype { size_bits, error });
        }
        if let Err(error) = self.push_allocation(
            id,
            Some(extent_index),
            ArenaAllocation::new(slot, size_bits, false, false),
        ) {
            let root = sel4::init_thread::slot::CNODE.cap();
            let _ = root
                .absolute_cptr(sel4::CPtr::from_bits(slot as _))
                .delete();
            self.slots.release(slot);
            return Err(error);
        }
        let bytes = 1usize << size_bits;
        let extent = self.extents[extent_index]
            .as_mut()
            .expect("selected extent exists");
        extent.watermark = watermark;
        extent.objects += 1;
        extent.bytes += bytes;
        self.objects_allocated += 1;
        self.live_objects += 1;
        self.bytes_allocated += bytes;
        self.live_bytes += bytes;
        Ok(sel4::init_thread::Slot::from_index(slot))
    }

    pub fn allocate_fixed_in<T: sel4::CapTypeForObjectOfFixedSize>(
        &mut self,
        id: TaskArenaId,
    ) -> Result<sel4::init_thread::Slot<T>, AllocError> {
        Ok(self.allocate_in(id, T::object_blueprint())?.cast())
    }

    pub fn allocate_variable_in<T: sel4::CapTypeForObjectOfVariableSize>(
        &mut self,
        id: TaskArenaId,
        size_bits: usize,
    ) -> Result<sel4::init_thread::Slot<T>, AllocError> {
        Ok(self.allocate_in(id, T::object_blueprint(size_bits))?.cast())
    }

    pub fn reserve_slot_in<T: sel4::CapType>(
        &mut self,
        id: TaskArenaId,
    ) -> Result<sel4::init_thread::Slot<T>, AllocError> {
        self.arena(id)?;
        self.allocation_position()?;
        let slot = self.take_slot()?;
        if let Err(error) =
            self.push_allocation(id, None, ArenaAllocation::new(slot, 0, false, false))
        {
            self.slots.release(slot);
            return Err(error);
        }
        Ok(sel4::init_thread::Slot::from_index(slot))
    }

    pub fn free_slots(&self) -> usize {
        self.slots.free()
    }

    pub fn allocation_descriptors_free(&self) -> usize {
        MAX_TASK_ALLOCATIONS
            - self
                .allocations
                .iter()
                .filter(|entry| entry.owner != u16::MAX)
                .count()
    }

    pub fn extent_descriptors_free(&self) -> usize {
        self.extents.iter().filter(|entry| entry.is_none()).count()
    }

    pub fn release_slot(&mut self, slot: usize) -> bool {
        self.physical.remove(slot);
        self.slots.release(slot)
    }

    pub fn arena_slot_count(&self, id: TaskArenaId) -> Result<usize, AllocError> {
        Ok(self.arena(id)?.slot_len)
    }

    /// Register an arena already owning `slots` root CSlots, for host tests.
    ///
    /// `begin_task_arena` retypes its static extent, which needs a live
    /// kernel; this records the same arena identity and slot ownership without
    /// one. The arena holds no allocation or extent record, so
    /// `release_task_arena` reports its slot count without invoking the
    /// kernel.
    #[cfg(test)]
    pub(crate) fn arena_owning_slots_for_test(&mut self, slots: usize) -> TaskArenaId {
        let index = self
            .arenas
            .iter()
            .position(|arena| !arena.active)
            .expect("test arena table has room");
        let serial = self.next_arena_serial;
        self.next_arena_serial = self.next_arena_serial.wrapping_add(1).max(1);
        self.arenas[index] = ArenaRecord {
            serial,
            active: true,
            slot_len: slots,
            ..ArenaRecord::empty()
        };
        self.arenas[index].id(index)
    }

    pub fn provision_private_backing(
        &mut self,
        id: TaskArenaId,
        quota: usize,
    ) -> Result<(), AllocError> {
        self.private_arena(id)?;
        provision_private_backing_with(quota, |request| match request {
            PrivateBackingRequest::Extent { kind, size_bits } => {
                self.provision_extent(id, size_bits, kind).map(|_| ())
            }
            PrivateBackingRequest::Slots { count } => self.provision_private_slots(id, count),
        })
    }

    pub fn provision_private_slots(
        &mut self,
        id: TaskArenaId,
        count: usize,
    ) -> Result<(), AllocError> {
        self.private_arena(id)?;
        if count > self.allocation_descriptors_free() || count > self.free_slots() {
            return Err(Self::private_record_error());
        }
        for _ in 0..count {
            let slot = self.take_slot()?;
            let position =
                match self.push_allocation(id, None, ArenaAllocation::new(slot, 0, true, true)) {
                    Ok(position) => position,
                    Err(error) => {
                        self.slots.release(slot);
                        return Err(error);
                    }
                };
            self.push_reusable(id, position)?;
        }
        Ok(())
    }

    fn take_private_slot(
        &mut self,
        id: TaskArenaId,
        kind: PrivateObjectKind,
        size_bits: usize,
    ) -> Result<(usize, usize, usize, bool, bool), AllocError> {
        if let Some(position) = self.pop_reusable(id, kind, Some(size_bits))? {
            let record = &mut self.allocations[position];
            let mapped = record.allocation.is_mapped();
            let extent = record.extent as usize;
            record
                .allocation
                .set_private_state(kind, size_bits, false, mapped);
            return Ok((position, extent, 0, true, mapped));
        }
        let extent_kind = if kind == PrivateObjectKind::LeafTable {
            ExtentKind::PrivateTables
        } else {
            ExtentKind::PrivateData
        };
        let (extent, watermark) = self.extent_for_allocation(id, extent_kind, size_bits)?;
        let position = self
            .pop_reusable(id, PrivateObjectKind::Empty, None)?
            .ok_or_else(Self::private_record_error)?;
        let record = &mut self.allocations[position];
        record.extent = extent as u16;
        record
            .allocation
            .set_private_state(kind, size_bits, false, false);
        Ok((position, extent, watermark, false, false))
    }

    /// A large frame occupies its entire data extent. Only a reusable, unmapped
    /// frame can be revoked here: committed and in-flight mappings remain owned.
    fn recycle_private_large_frame<K: crate::private_memory::PrivateMemoryKernel>(
        &mut self,
        id: TaskArenaId,
        kernel: &mut K,
    ) -> Result<(), AllocError> {
        let Some(position) = self.pop_reusable(id, PrivateObjectKind::LargeFrame, None)? else {
            return Ok(());
        };
        let record = self.allocations[position];
        let extent_index = record.extent as usize;
        let result = (|| {
            let extent = self
                .extents
                .get(extent_index)
                .and_then(Option::as_ref)
                .filter(|extent| {
                    extent.belongs_to(id)
                        && !extent.revoked
                        && extent.kind == ExtentKind::PrivateData
                        && extent.size_bits == record.allocation.size_bits()
                        && extent.objects == 1
                        && extent.bytes == 1usize << extent.size_bits
                        && !record.allocation.is_mapped()
                })
                .ok_or_else(Self::private_record_error)?;
            kernel
                .revoke(extent.parent)
                .map_err(|error| AllocError::ArenaCleanup {
                    slot: extent.parent.bits() as usize,
                    error,
                })
        })();
        if let Err(error) = result {
            self.push_reusable(id, position)?;
            return Err(error);
        }
        let extent = self.extents[extent_index]
            .as_mut()
            .expect("validated private extent");
        self.live_objects -= extent.objects;
        self.live_bytes -= extent.bytes;
        extent.watermark = 0;
        extent.objects = 0;
        extent.bytes = 0;
        let record = &mut self.allocations[position];
        record.extent = PRIVATE_EXTENT_NONE;
        record
            .allocation
            .set_private_state(PrivateObjectKind::Empty, 0, true, false);
        self.push_reusable(id, position)
    }

    pub(crate) fn acquire_private_in<K: crate::private_memory::PrivateMemoryKernel>(
        &mut self,
        id: TaskArenaId,
        kind: PrivateObjectKind,
        blueprint: sel4::ObjectBlueprint,
        kernel: &mut K,
    ) -> Result<PrivateAllocation, AllocError> {
        debug_assert!(kind != PrivateObjectKind::Empty);
        self.private_arena(id)?;
        if kind == PrivateObjectKind::Granule {
            self.recycle_private_large_frame(id, kernel)?;
        }
        #[cfg(slime_private_fail_second_allocation)]
        if fail_private_allocation(kind, self.private_arena(id)?.in_flight_granules) {
            return Err(AllocError::Retype {
                size_bits: blueprint.physical_size_bits(),
                error: sel4::Error::NotEnoughMemory,
            });
        }
        let size_bits = blueprint.physical_size_bits();
        let (position, extent_index, watermark, reused, mapped) =
            self.take_private_slot(id, kind, size_bits)?;
        let slot = self.allocations[position].allocation.slot();
        if !reused {
            let parent = self.extents[extent_index]
                .expect("private extent exists")
                .parent;
            if let Err(error) = kernel.retype(parent, &blueprint, slot) {
                self.allocations[position].extent = PRIVATE_EXTENT_NONE;
                self.allocations[position].allocation.set_private_state(
                    PrivateObjectKind::Empty,
                    0,
                    true,
                    false,
                );
                self.push_reusable(id, position)?;
                return Err(AllocError::Retype { size_bits, error });
            }
            let bytes = 1usize << size_bits;
            let extent = self.extents[extent_index]
                .as_mut()
                .expect("private extent exists");
            extent.watermark = watermark;
            extent.objects += 1;
            extent.bytes += bytes;
            self.objects_allocated += 1;
            self.live_objects += 1;
            self.bytes_allocated += bytes;
            self.live_bytes += bytes;
        }
        Ok(PrivateAllocation {
            arena: id,
            position: position as u32,
            cap: sel4::cap::Unspecified::from_bits(slot as _),
            mapped,
        })
    }

    pub(crate) fn mark_private_in_flight(
        &mut self,
        id: TaskArenaId,
        allocation: PrivateAllocation,
        mapped: bool,
    ) -> Result<(), AllocError> {
        #[cfg(test)]
        {
            self.private_visits.mark += 1;
        }
        let position = allocation.position as usize;
        let kind = self
            .private_record_mut(id, &allocation)?
            .allocation
            .private_kind();
        let size_bits = self.allocations[position].allocation.size_bits();
        let head = self.private_arena(id)?.in_flight_head;
        let record = &mut self.allocations[position];
        record
            .allocation
            .set_private_state(kind, size_bits, false, mapped);
        record.allocation.set_in_flight(true);
        record.next_state = head;
        let arena = self.arena_mut(id)?;
        arena.in_flight_head = position as u32;
        if kind == PrivateObjectKind::Granule {
            arena.in_flight_granules += 1;
        }
        Ok(())
    }

    pub fn commit_private_transaction(&mut self, id: TaskArenaId) -> Result<(), AllocError> {
        self.private_arena(id)?;
        loop {
            let head = self.private_arena(id)?.in_flight_head;
            if head == PRIVATE_STATE_NONE {
                return Ok(());
            }
            let position = head as usize;
            #[cfg(test)]
            {
                self.private_visits.commit += 1;
            }
            let record = self
                .allocations
                .get(position)
                .filter(|record| {
                    record.belongs_to(id)
                        && record.allocation.is_private()
                        && record.allocation.is_in_flight()
                        && !record.allocation.is_reusable()
                })
                .ok_or_else(Self::private_record_error)?;
            let next = record.next_state;
            let kind = record.allocation.private_kind();
            self.arenas[id.index()].in_flight_head = next;
            self.allocations[position].next_state = PRIVATE_STATE_NONE;
            self.allocations[position].allocation.set_in_flight(false);
            if kind == PrivateObjectKind::Granule {
                self.arenas[id.index()].in_flight_granules -= 1;
            }
        }
    }

    pub(crate) fn unwind_private_transaction<K: crate::private_memory::PrivateMemoryKernel>(
        &mut self,
        id: TaskArenaId,
        kernel: &mut K,
    ) -> Result<(), AllocError> {
        self.private_arena(id)?;
        loop {
            let head = self.private_arena(id)?.in_flight_head;
            if head == PRIVATE_STATE_NONE {
                return Ok(());
            }
            let position = head as usize;
            #[cfg(test)]
            {
                self.private_visits.unwind += 1;
            }
            let record = self
                .allocations
                .get(position)
                .filter(|record| {
                    record.belongs_to(id)
                        && record.allocation.is_private()
                        && record.allocation.is_in_flight()
                        && !record.allocation.is_reusable()
                })
                .ok_or_else(Self::private_record_error)?;
            let next = record.next_state;
            let slot = record.allocation.slot();
            let kind = record.allocation.private_kind();
            let size_bits = record.allocation.size_bits();
            let mapped = if kind == PrivateObjectKind::LeafTable {
                true
            } else {
                kernel
                    .unmap_frame(sel4::cap::UnspecifiedPage::from_bits(slot as _))
                    .map_err(|error| AllocError::ArenaCleanup { slot, error })?;
                false
            };
            self.arenas[id.index()].in_flight_head = next;
            if kind == PrivateObjectKind::Granule {
                self.arenas[id.index()].in_flight_granules -= 1;
            }
            let record = &mut self.allocations[position];
            record.next_state = PRIVATE_STATE_NONE;
            record
                .allocation
                .set_private_state(kind, size_bits, true, mapped);
            self.push_reusable(id, position)?;
        }
    }

    pub(crate) fn reset_private_in(
        &mut self,
        id: TaskArenaId,
        allocation: PrivateAllocation,
    ) -> Result<(), AllocError> {
        #[cfg(test)]
        {
            self.private_visits.reset += 1;
        }
        let position = allocation.position as usize;
        let record = self.private_record_mut(id, &allocation)?;
        let kind = record.allocation.private_kind();
        let size_bits = record.allocation.size_bits();
        record
            .allocation
            .set_private_state(kind, size_bits, true, false);
        self.push_reusable(id, position)
    }

    /// Revoke every backing extent before returning any task-owned CSlot.
    /// Completed revokes are recorded so a later failure is retryable exactly once.
    pub fn release_task_arena(&mut self, id: TaskArenaId) -> Result<usize, AllocError> {
        let released = self.arena(id)?.slot_len;
        self.arenas[id.index()].releasing = true;
        let root = sel4::init_thread::slot::CNODE.cap();
        for index in 0..self.extents.len() {
            let Some(extent) = self.extents[index] else {
                continue;
            };
            if !extent.belongs_to(id) || extent.revoked {
                continue;
            }
            let parent_slot = extent.parent.bits() as usize;
            root.absolute_cptr(sel4::CPtr::from_bits(parent_slot as _))
                .revoke()
                .map_err(|error| AllocError::ArenaCleanup {
                    slot: parent_slot,
                    error,
                })?;
            self.live_objects = self.live_objects.saturating_sub(extent.objects);
            self.live_bytes = self.live_bytes.saturating_sub(extent.bytes);
            let record = self.extents[index].as_mut().expect("extent exists");
            record.revoked = true;
            record.objects = 0;
            record.bytes = 0;
        }
        self.arenas[id.index()].reusable_heads = [PRIVATE_STATE_NONE; 4];
        self.arenas[id.index()].in_flight_head = PRIVATE_STATE_NONE;
        self.arenas[id.index()].in_flight_granules = 0;
        for index in 0..self.allocations.len() {
            let record = self.allocations[index];
            if !record.belongs_to(id) {
                continue;
            }
            let slot = record.allocation.slot();
            root.absolute_cptr(sel4::CPtr::from_bits(slot as _))
                .delete()
                .map_err(|error| AllocError::ArenaCleanup { slot, error })?;
            self.slots.release(slot);
            self.physical.remove(slot);
            self.allocations[index] = AllocationRecord::EMPTY;
        }
        for extent in self.extents.iter_mut().flatten() {
            if extent.belongs_to(id) {
                extent.active = false;
                extent.revoked = false;
                extent.owner = u16::MAX;
                extent.serial = 0;
                extent.watermark = 0;
            }
        }
        self.arenas[id.index()] = ArenaRecord::empty();
        Ok(released)
    }

    pub fn allocate_device_frame(
        &mut self,
        paddr: usize,
    ) -> Result<sel4::init_thread::Slot<sel4::cap_type::Granule>, AllocError> {
        if !paddr.is_multiple_of(GRANULE_BYTES) {
            return Err(AllocError::UnalignedDeviceFrame { paddr });
        }
        let (region_index, region) = self
            .devices
            .iter()
            .take(self.device_len)
            .enumerate()
            .find_map(|(index, region)| {
                let region = region.as_ref()?;
                region
                    .contains(paddr, GRANULE_BYTES)
                    .then_some((index, *region))
            })
            .ok_or(AllocError::NoDeviceUntyped { paddr })?;
        let target = (paddr - region.paddr) / GRANULE_BYTES;
        let count = target
            .checked_sub(region.retyped)
            .and_then(|count| count.checked_add(1))
            .ok_or(AllocError::DeviceFramePassed { paddr })?;

        // Preflight the exact sparse walk before advancing the kernel untyped's
        // watermark. Each completed chunk leaves only its last slot live, so
        // the next allocation sees the original pool plus one virtual anchor.
        // A fragmented or exhausted pool is therefore refused before the first
        // retype rather than leaving an unreachable consumed prefix behind.
        let mut planned = 0;
        let mut planned_anchor = None;
        while planned < count {
            let chunk_len = device_retype_plan(
                region.retyped + planned,
                target,
                self.slots.free(),
                planned_anchor.is_some(),
            )
            .map(|(_, batch)| batch)
            .ok_or(AllocError::SlotsExhausted {
                allocated: self.slots_allocated,
            })?;
            let first = self
                .slots
                .first_contiguous(chunk_len, planned_anchor)
                .ok_or(AllocError::SlotsExhausted {
                    allocated: self.slots_allocated,
                })?;
            planned += chunk_len;
            planned_anchor = Some(first + chunk_len - 1);
        }
        if self.physical.len >= MAX_PHYSICAL_PROVENANCE {
            return Err(AllocError::ProvenanceTableFull {
                limit: MAX_PHYSICAL_PROVENANCE,
            });
        }

        let mut completed = 0;
        let mut anchor = None;
        let root = sel4::init_thread::slot::CNODE.cap();

        while completed < count {
            let chunk_len = device_retype_plan(
                region.retyped + completed,
                target,
                self.slots.free(),
                anchor.is_some(),
            )
            .map(|(_, batch)| batch)
            .ok_or(AllocError::SlotsExhausted {
                allocated: self.slots_allocated,
            })?;
            let (first, reused) = self
                .slots
                .allocate_contiguous(chunk_len, self.slots_allocated)?;
            let last = first + chunk_len - 1;
            let final_chunk = completed + chunk_len == count;
            if final_chunk && let Err(error) = self.physical.insert(last, paddr) {
                for slot in first..first + chunk_len {
                    self.slots.release(slot);
                }
                return Err(error);
            }
            if let Err(error) = region.cap.untyped_retype(
                &sel4::FrameObjectType::GRANULE.blueprint(),
                &root.absolute_cptr_for_self(),
                first,
                chunk_len,
            ) {
                if final_chunk {
                    self.physical.remove(last);
                }
                for slot in first..first + chunk_len {
                    self.slots.release(slot);
                }
                return Err(AllocError::Retype {
                    size_bits: sel4::FrameObjectType::GRANULE.bits(),
                    error,
                });
            }
            self.slots_allocated += chunk_len;
            self.slots_reused += reused;
            self.objects_allocated += chunk_len;
            self.live_objects += chunk_len;
            self.bytes_allocated += chunk_len * GRANULE_BYTES;
            self.live_bytes += chunk_len * GRANULE_BYTES;

            // Keep one child while advancing to the next chunk. If every cap
            // disappeared, seL4 would reset the device untyped's free index to
            // zero and the next retype would recreate the prefix instead of
            // continuing toward the requested physical page.
            if let Some(slot) = anchor.take() {
                root.absolute_cptr(sel4::CPtr::from_bits(slot as sel4::CPtrBits))
                    .delete()
                    .map_err(|error| AllocError::DeviceCleanup { slot, error })?;
                self.slots.release(slot);
                self.live_objects -= 1;
                self.live_bytes -= GRANULE_BYTES;
            }
            for slot in first..last {
                root.absolute_cptr(sel4::CPtr::from_bits(slot as sel4::CPtrBits))
                    .delete()
                    .map_err(|error| AllocError::DeviceCleanup { slot, error })?;
                self.slots.release(slot);
                self.live_objects -= 1;
                self.live_bytes -= GRANULE_BYTES;
            }
            anchor = Some(last);
            completed += chunk_len;
        }
        if let Some(region) = self.devices[region_index].as_mut() {
            region.retyped += count;
        }

        self.last_paddr = paddr;
        Ok(sel4::init_thread::Slot::from_index(anchor.unwrap()))
    }

    fn regions(&self) -> &[Option<UntypedRegion>] {
        self.untypeds.get(..self.untyped_len).unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AllocError, AllocationRecord, ArenaAllocation, ArenaPlan, ArenaRecord, ExtentKind,
        ExtentRecord, GRANULE_BYTES, KERNEL_ROOT_CNODE_SLOTS, LARGE_DESCRIPTOR_TABLES,
        MAX_PHYSICAL_PROVENANCE, MAX_PLANNED_PRIVATE_ALLOCATIONS, MAX_PLANNED_PRIVATE_PAGES,
        MAX_PLANNED_PRIVATE_SPANS, MAX_PLANNED_STATIC_ALLOCATIONS, MAX_PRIVATE_EXTENT_BYTES,
        MAX_PRIVATE_EXTENT_PAGES, MAX_ROOT_CSLOTS, MAX_TASK_ALLOCATIONS, MAX_TASK_ARENAS,
        MAX_TASK_EXTENTS, ObjectAllocator, PRIVATE_EXTENT_NONE, PRIVATE_STATE_NONE,
        PROVENANCE_SLOTS, PrivateBackingLayout, PrivateBackingRequest, PrivateObjectKind,
        PrivateRecordVisits, ProvenanceTable, SlotPool, TaskArenaId, TaskBackingCapacity,
        TaskStaticBacking, UntypedRegion, device_retype_plan, plan_allocation, plan_task_backing,
        provision_private_backing_with, task_backing_extents_fit_in,
        task_static_backing_from_records,
    };
    use crate::private_memory::{PrivateMemoryKernel, Region, Table};
    use sel4::CapTypeForObjectOfFixedSize;
    extern crate std;
    use std::vec::Vec;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum KernelRequest {
        Retype { slot: usize, size_bits: usize },
        MapFrame { slot: usize, vaddr: usize },
        MapLeaf { slot: usize, vaddr: usize },
        UnmapFrame { slot: usize },
        Revoke { slot: usize },
    }

    #[derive(Default)]
    struct RecordingPrivateKernel {
        requests: Vec<KernelRequest>,
        fail_retype_at: Option<usize>,
        fail_map_frame_at: Option<usize>,
        fail_unmap_at: Option<usize>,
        fail_revoke: bool,
        retypes: usize,
        frame_maps: usize,
        unmaps: usize,
    }

    impl PrivateMemoryKernel for RecordingPrivateKernel {
        fn revoke(&mut self, parent: sel4::cap::Untyped) -> Result<(), sel4::Error> {
            self.requests.push(KernelRequest::Revoke {
                slot: parent.bits() as usize,
            });
            if self.fail_revoke {
                Err(sel4::Error::NotEnoughMemory)
            } else {
                Ok(())
            }
        }

        fn retype(
            &mut self,
            _parent: sel4::cap::Untyped,
            blueprint: &sel4::ObjectBlueprint,
            slot: usize,
        ) -> Result<(), sel4::Error> {
            self.retypes += 1;
            self.requests.push(KernelRequest::Retype {
                slot,
                size_bits: blueprint.physical_size_bits(),
            });
            if self.fail_retype_at == Some(self.retypes) {
                Err(sel4::Error::NotEnoughMemory)
            } else {
                Ok(())
            }
        }

        fn map_frame(
            &mut self,
            frame: sel4::cap::UnspecifiedPage,
            _vspace: sel4::cap::VSpace,
            vaddr: usize,
            _rights: sel4::CapRights,
            _attrs: sel4::VmAttributes,
        ) -> Result<(), sel4::Error> {
            self.frame_maps += 1;
            self.requests.push(KernelRequest::MapFrame {
                slot: frame.bits() as usize,
                vaddr,
            });
            if self.fail_map_frame_at == Some(self.frame_maps) {
                Err(sel4::Error::NotEnoughMemory)
            } else {
                Ok(())
            }
        }

        fn map_leaf(
            &mut self,
            table: sel4::cap::UnspecifiedIntermediateTranslationTable,
            _ty: sel4::TranslationTableObjectType,
            _vspace: sel4::cap::VSpace,
            vaddr: usize,
            _attrs: sel4::VmAttributes,
        ) -> Result<(), sel4::Error> {
            self.requests.push(KernelRequest::MapLeaf {
                slot: table.bits() as usize,
                vaddr,
            });
            Ok(())
        }

        fn unmap_frame(&mut self, frame: sel4::cap::UnspecifiedPage) -> Result<(), sel4::Error> {
            self.unmaps += 1;
            self.requests.push(KernelRequest::UnmapFrame {
                slot: frame.bits() as usize,
            });
            if self.fail_unmap_at == Some(self.unmaps) {
                Err(sel4::Error::NotEnoughMemory)
            } else {
                Ok(())
            }
        }
    }

    fn setup_private_growth_fixture() -> (ObjectAllocator, TaskArenaId) {
        let mut allocator = ObjectAllocator::empty();
        allocator.slots = SlotPool::new(1024..MAX_ROOT_CSLOTS).unwrap();
        allocator.arenas[0] = ArenaRecord {
            serial: 1,
            active: true,
            ..ArenaRecord::empty()
        };
        let id = allocator.arenas[0].id(0);
        let mut extent_position = 0;
        provision_private_backing_with(512, |request| match request {
            PrivateBackingRequest::Extent { kind, size_bits } => {
                let slot = allocator.take_slot()?;
                let mut extent =
                    ExtentRecord::new(sel4::cap::Untyped::from_bits(slot as _), size_bits);
                extent.assign(id.index(), id.serial, kind);
                allocator.extents[extent_position] = Some(extent);
                extent_position += 1;
                Ok(())
            }
            PrivateBackingRequest::Slots { count } => allocator.provision_private_slots(id, count),
        })
        .unwrap();
        allocator.reset_private_record_visits();
        (allocator, id)
    }

    /// Physical provenance is retained for a live frame, dropped when the frame
    /// is released, and refused rather than lost when the table is full.
    ///
    /// The third arm is why this exists. This table replaced a
    /// `[usize; MAX_ROOT_CSLOTS]` array whose 2 MB of `.bss` spent 512 root
    /// CSlots and made an admissible generation unbootable, and the array's one
    /// virtue was that it could not fill. A bounded table can, and the *only*
    /// acceptable behaviour there is to fail closed: a frame whose physical base
    /// the root cannot recover, silently mapped from a stale or absent record,
    /// would put some other frame's address in a device descriptor and let the
    /// device touch memory nothing granted it.
    ///
    /// Driven through the table rather than through `ObjectAllocator`, whose
    /// every allocation path needs a live kernel to retype against; the seL4
    /// gates cover the join. The reuse arm is the one that would catch a
    /// `release_slot` that forgot to free its entry: a root allocating and
    /// releasing DMA frames in a loop must not exhaust a table sized to the
    /// live population.
    #[test]
    fn physical_provenance_is_freed_on_release_and_fails_closed_when_full() {
        const PAGE: usize = 4096;
        let mut table = ProvenanceTable::new();

        // An allocated frame's address is retrievable; an unrecorded slot's is
        // not, so `None` means "the root does not own this" rather than zero.
        assert_eq!(table.insert(7, 0x4000_0000), Ok(()));
        assert_eq!(table.get(7), Some(0x4000_0000));
        assert_eq!(table.get(8), None);

        // A released frame's address is gone. Answering from a stale record
        // would hand a device an address the root no longer owns.
        assert!(table.remove(7));
        assert_eq!(table.get(7), None);
        assert!(!table.remove(7));
        assert_eq!(table.len, 0);

        // Colliding slots stay individually findable and individually
        // removable: `PROVENANCE_SLOTS` apart is the same probe start, so
        // removing the first must not strand the second behind an empty
        // position. This is the property that lets deletion avoid tombstones.
        let (a, b, c) = (5, 5 + PROVENANCE_SLOTS, 5 + 2 * PROVENANCE_SLOTS);
        for (index, slot) in [a, b, c].into_iter().enumerate() {
            assert_eq!(table.insert(slot, index * PAGE), Ok(()));
        }
        assert!(table.remove(a));
        assert_eq!(table.get(b), Some(PAGE));
        assert_eq!(table.get(c), Some(2 * PAGE));
        assert!(table.remove(b));
        assert_eq!(table.get(c), Some(2 * PAGE));
        assert!(table.remove(c));
        assert_eq!(table.len, 0);

        // Full to its declared bound, then refused. Not dropped, not
        // overwritten: the record the caller would have read is still the one
        // it gets, and the new frame's allocation fails.
        for slot in 0..MAX_PHYSICAL_PROVENANCE {
            assert_eq!(table.insert(slot, slot * PAGE), Ok(()), "slot {slot}");
        }
        assert_eq!(table.len, MAX_PHYSICAL_PROVENANCE);
        assert_eq!(
            table.insert(MAX_PHYSICAL_PROVENANCE, 0xdead_0000),
            Err(AllocError::ProvenanceTableFull {
                limit: MAX_PHYSICAL_PROVENANCE
            })
        );
        assert_eq!(table.get(MAX_PHYSICAL_PROVENANCE), None);
        // And every prior record survived the refusal.
        for slot in 0..MAX_PHYSICAL_PROVENANCE {
            assert_eq!(table.get(slot), Some(slot * PAGE), "slot {slot}");
        }

        // A full table that releases one frame accepts one more, which is what
        // makes long-running reuse bounded by the live population rather than
        // by the number of frames the root has ever allocated.
        assert!(table.remove(0));
        assert_eq!(table.insert(MAX_PHYSICAL_PROVENANCE, 0x9000_0000), Ok(()));
        assert_eq!(table.get(MAX_PHYSICAL_PROVENANCE), Some(0x9000_0000));

        // Re-recording a live slot overwrites in place rather than consuming a
        // second position, so a pool that reissued an index still answers with
        // that index's current frame.
        assert_eq!(table.len, MAX_PHYSICAL_PROVENANCE);
        assert_eq!(table.insert(MAX_PHYSICAL_PROVENANCE, 0xa000_0000), Ok(()));
        assert_eq!(table.len, MAX_PHYSICAL_PROVENANCE);
        assert_eq!(table.get(MAX_PHYSICAL_PROVENANCE), Some(0xa000_0000));
    }

    #[test]
    fn sparse_device_retype_is_chunked_without_spending_the_root_cspace() {
        assert_eq!(device_retype_plan(0, 0, 3_000, false), Some((1, 1)));
        assert_eq!(device_retype_plan(0, 255, 3_000, false), Some((1, 256)));
        assert_eq!(device_retype_plan(0, 256, 3_000, false), Some((2, 256)));
        assert_eq!(device_retype_plan(0, 0x101, 3_000, false), Some((2, 256)));
        assert_eq!(device_retype_plan(7, 6, 3_000, false), None);

        assert_eq!(device_retype_plan(0, 20_517, 3_000, false), Some((81, 256)));
        assert_eq!(device_retype_plan(0, 261, 255, false), Some((2, 255)));
        assert_eq!(device_retype_plan(255, 261, 255, true), Some((1, 7)));
        assert_eq!(device_retype_plan(0, 0, 0, false), None);
        assert_eq!(device_retype_plan(0, 1, 1, true), None);
    }
    #[test]
    fn sparse_device_preflight_models_the_live_anchor_without_mutating_slots() {
        let mut slots = SlotPool::new(100..108).unwrap();
        for expected in 100..106 {
            let (allocated, _) = slots.allocate(0).unwrap();
            assert_eq!(allocated, expected);
        }
        assert!(slots.release(100));
        assert!(slots.release(102));
        assert!(slots.release(104));

        assert_eq!(slots.first_contiguous(1, None), Some(100));
        assert_eq!(slots.first_contiguous(1, Some(100)), Some(102));
        assert_eq!(slots.first_contiguous(2, None), Some(106));
        assert_eq!(slots.first_contiguous(2, Some(106)), None);
        assert_eq!(slots.live, 3);
    }

    #[test]
    fn allocation_is_aligned_to_object_size() {
        assert_eq!(plan_allocation(0, 1 << 20, 12), Some((0, 4096)));
        assert_eq!(plan_allocation(4096, 1 << 20, 14), Some((16384, 32768)));
    }

    #[test]
    fn alignment_loss_can_exhaust_a_region() {
        assert_eq!(plan_allocation(4096, 1 << 14, 14), None);
        assert_eq!(plan_allocation(4096, 1 << 14, 12), Some((4096, 8192)));
    }

    #[test]
    fn exact_fit_is_allowed_and_full_region_is_not() {
        assert_eq!(plan_allocation(0, 1 << 12, 12), Some((0, 4096)));
        assert_eq!(plan_allocation(4096, 1 << 12, 12), None);
    }

    #[test]
    fn runtime_plan_matches_provisioning_trace() {
        for quota in 0..=crate::private_memory::MAX_REGION_PAGES {
            let mut trace = [None; 4];
            let mut len = 0;
            provision_private_backing_with(quota, |request| {
                trace[len] = Some(request);
                len += 1;
                Ok(())
            })
            .unwrap();
            let plan = plan_task_backing(quota).unwrap();
            let mut reserved = 0;
            let mut table = 0;
            let mut data = 0;
            let mut allocations = 0;
            let mut extents = 1;
            for request in trace.into_iter().take(len).flatten() {
                match request {
                    PrivateBackingRequest::Extent { kind, size_bits } => {
                        let bytes = 1usize << size_bits;
                        reserved += bytes;
                        extents += 1;
                        if kind == ExtentKind::PrivateTables {
                            table += bytes;
                        } else {
                            data += 1;
                        }
                    }
                    PrivateBackingRequest::Slots { count } => allocations = count,
                }
            }
            assert_eq!(plan.data_extents, data);
            assert_eq!(plan.extent_descriptors, extents);
            assert_eq!(plan.allocation_descriptors, allocations);
            assert_eq!(plan.required_cslots, allocations + extents);
            assert_eq!(plan.reserved_bytes, reserved);
            assert_eq!(plan.page_table_bytes, table);
        }
        let requests = |quota| {
            let mut trace = [None; 4];
            let mut len = 0;
            provision_private_backing_with(quota, |request| {
                trace[len] = Some(request);
                len += 1;
                Ok(())
            })
            .unwrap();
            (trace, len)
        };
        let (zero, zero_len) = requests(0);
        assert_eq!(zero_len, 1);
        assert_eq!(zero[0], Some(PrivateBackingRequest::Slots { count: 0 }));
        let (full, full_len) = requests(512);
        assert_eq!(full_len, 3);
        assert_eq!(
            full,
            [
                Some(PrivateBackingRequest::Extent {
                    kind: ExtentKind::PrivateTables,
                    size_bits: 12,
                }),
                Some(PrivateBackingRequest::Extent {
                    kind: ExtentKind::PrivateData,
                    size_bits: 21,
                }),
                Some(PrivateBackingRequest::Slots { count: 514 }),
                None,
            ]
        );
        assert_eq!(
            PrivateBackingLayout::for_quota(513),
            PrivateBackingLayout::for_quota(512)
        );
        assert_eq!(
            PrivateBackingLayout::for_quota(usize::MAX),
            PrivateBackingLayout::for_quota(512)
        );
        assert_ne!(
            plan_task_backing(513).unwrap(),
            plan_task_backing(512).unwrap()
        );
        assert_eq!(
            plan_task_backing(512),
            Some(super::TaskBackingPlan {
                private_pages: 512,
                data_extents: 1,
                extent_descriptors: 3,
                allocation_descriptors: 514,
                required_cslots: 517,
                reserved_bytes: 2_101_248,
                payload_bytes: 2_097_152,
                page_table_bytes: 4096,
                alignment_waste: 0,
            })
        );
    }

    #[test]
    fn private_provisioning_stops_at_first_failure() {
        for quota in [0, 3, 512] {
            let mut expected = [None; 4];
            let mut request_count = 0;
            provision_private_backing_with(quota, |request| {
                expected[request_count] = Some(request);
                request_count += 1;
                Ok(())
            })
            .unwrap();
            for failing in 0..request_count {
                let mut actual = [None; 4];
                let mut called = 0;
                let error = provision_private_backing_with(quota, |request| {
                    actual[called] = Some(request);
                    let current = called;
                    called += 1;
                    if current == failing {
                        Err(AllocError::NoKernelUntyped)
                    } else {
                        Ok(())
                    }
                })
                .unwrap_err();
                assert_eq!(error, AllocError::NoKernelUntyped);
                assert_eq!(called, failing + 1);
                assert_eq!(actual[..called], expected[..called]);
            }
        }
    }

    #[test]
    fn runtime_capacity_rejects_each_reservation_shortfall() {
        let plan = plan_task_backing(512).unwrap();
        let exact = TaskBackingCapacity {
            plan,
            static_backing: TaskStaticBacking {
                allocation_descriptors: 8,
                reserved_bytes: 16_384,
            },
            holders: 1,
            cslots_available: 525,
            allocation_descriptors_available: 522,
            extent_descriptors_available: 3,
            ordinary_bytes_available: 2_117_632,
            ordinary_layout_fits: true,
            root_image_bytes: 0,
            root_stack_bytes: 0,
            root_heap_bytes: 0,
        };
        assert_eq!(exact.requirements().unwrap().cslots, 525);
        assert!(exact.fits());
        assert!(
            !TaskBackingCapacity {
                cslots_available: 524,
                ..exact
            }
            .fits()
        );
        assert!(
            !TaskBackingCapacity {
                allocation_descriptors_available: 521,
                ..exact
            }
            .fits()
        );
        assert!(
            !TaskBackingCapacity {
                extent_descriptors_available: 2,
                ..exact
            }
            .fits()
        );
        assert!(
            !TaskBackingCapacity {
                ordinary_bytes_available: 2_117_631,
                ..exact
            }
            .fits()
        );
    }

    #[test]
    fn backing_slots_scale_with_quota_and_cover_both_full_window_shapes() {
        assert_eq!(PrivateBackingLayout::for_quota(0).allocation_descriptors, 0);
        assert_eq!(PrivateBackingLayout::for_quota(8).allocation_descriptors, 9);
        assert_eq!(
            PrivateBackingLayout::for_quota(512).allocation_descriptors,
            514
        );
        assert_eq!(
            PrivateBackingLayout::for_quota(2048).allocation_descriptors,
            514
        );
    }

    /// A quota's root-CSlot cost is its descriptors *plus* one slot per
    /// extent, measured against what provisioning actually takes from the
    /// pool rather than restated from the layout.
    ///
    /// The two must agree because generation admission adds this cost before
    /// any child starts.
    #[test]
    fn quota_root_slot_cost_counts_every_extent_parent() {
        // On its own large stack, as every allocator-holding test here is:
        // the record tables are megabytes and a default test stack holds no
        // `ObjectAllocator` at all.
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(|| {
                // One allocator across every quota, each with its own arena,
                // measured as a delta -- which is what the per-holder
                // admission term is anyway.
                let mut allocator = ObjectAllocator::empty();
                allocator.slots = SlotPool::new(1024..MAX_ROOT_CSLOTS).unwrap();
                let mut extent_position = 0;
                for (index, quota) in [0, 1, 8, 511, 512, 513, usize::MAX].into_iter().enumerate() {
                    let layout = PrivateBackingLayout::for_quota(quota);
                    allocator.arenas[index] = ArenaRecord {
                        serial: index as u32 + 1,
                        active: true,
                        ..ArenaRecord::empty()
                    };
                    let id = allocator.arenas[index].id(index);
                    let before = allocator.free_slots();
                    let extents_before = extent_position;
                    // `provision_extent`'s retype needs a live kernel, so the
                    // extent arm takes its slot directly and records the
                    // extent; the slot accounting under test is unchanged.
                    provision_private_backing_with(quota, |request| match request {
                        PrivateBackingRequest::Extent { kind, size_bits } => {
                            let slot = allocator.take_slot()?;
                            let mut extent = ExtentRecord::new(
                                sel4::cap::Untyped::from_bits(slot as _),
                                size_bits,
                            );
                            extent.assign(id.index(), id.serial, kind);
                            allocator.extents[extent_position] = Some(extent);
                            extent_position += 1;
                            Ok(())
                        }
                        PrivateBackingRequest::Slots { count } => {
                            allocator.provision_private_slots(id, count)
                        }
                    })
                    .unwrap();
                    let consumed = before - allocator.free_slots();
                    assert_eq!(
                        consumed,
                        layout.allocation_descriptors + layout.extent_parent_slots(),
                        "quota {quota} consumed {consumed} root CSlots"
                    );
                    assert_eq!(
                        extent_position - extents_before,
                        layout.extent_parent_slots()
                    );
                }
                // Nonzero quotas retain one data extent and one table extent.
                assert_eq!(PrivateBackingLayout::for_quota(0).extent_parent_slots(), 0);
                assert_eq!(PrivateBackingLayout::for_quota(1).extent_parent_slots(), 2);
                assert_eq!(
                    PrivateBackingLayout::for_quota(511).extent_parent_slots(),
                    2
                );
                assert_eq!(
                    PrivateBackingLayout::for_quota(512).extent_parent_slots(),
                    2
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn freed_root_slot_is_the_next_slot_reused() {
        let mut slots = SlotPool::new(100..104).unwrap();
        assert_eq!(slots.allocate(0), Ok((100, false)));
        assert_eq!(slots.allocate(1), Ok((101, false)));
        assert!(slots.release(100));
        assert_eq!(slots.allocate(2), Ok((100, true)));
        assert_eq!(slots.live, 2);
    }

    #[test]
    fn bounded_live_slots_cross_the_old_lifetime_watermark() {
        let mut slots = SlotPool::new(10..14).unwrap();
        for issued in 0..80 {
            let (slot, reused) = slots.allocate(issued).unwrap();
            assert_eq!(slot, 10);
            assert_eq!(reused, issued != 0);
            assert!(slots.release(slot));
        }
        assert_eq!(slots.live, 0);
    }

    #[test]
    fn arena_plan_accounts_for_alignment() {
        let mut plan = ArenaPlan::new();
        plan.add_size_bits(12).unwrap();
        plan.add_size_bits(14).unwrap();
        assert_eq!(plan.required_bytes(), 32 * 1024);
        assert_eq!(plan.required_size_bits(), Some(15));
    }

    #[test]
    fn arena_release_model_returns_same_parent_space() {
        let mut parent_available = true;
        let first_parent = if core::mem::replace(&mut parent_available, false) {
            7
        } else {
            0
        };
        parent_available = true;
        let second_parent = if core::mem::replace(&mut parent_available, false) {
            7
        } else {
            0
        };
        assert_eq!(first_parent, second_parent);
    }

    #[test]
    fn large_capacity_plans_are_segmented_and_honest() {
        const PAGES_64_MIB: usize = 64 * 1024 * 1024 / 4096;
        const PAGES_256_MIB: usize = 256 * 1024 * 1024 / 4096;
        let sixty_four = plan_task_backing(PAGES_64_MIB).unwrap();
        assert_eq!(sixty_four.data_extents, 32);
        assert_eq!(sixty_four.payload_bytes, 64 * 1024 * 1024);
        assert_eq!(sixty_four.alignment_waste, 0);
        assert_eq!(sixty_four.page_table_bytes, 32 * 4096);

        let holder = plan_task_backing(PAGES_256_MIB).unwrap();
        assert_eq!(holder.data_extents, 128);
        assert_eq!(holder.reserved_bytes, 256 * 1024 * 1024 + 128 * 4096);
        let four_reserved = holder.reserved_bytes * 4;
        assert_eq!(four_reserved, 1_075_838_976);

        let current = plan_task_backing(MAX_PRIVATE_EXTENT_PAGES).unwrap();
        assert_eq!(current.data_extents, 1);
        assert_eq!(current.reserved_bytes, MAX_PRIVATE_EXTENT_BYTES + 4096);
        assert!(plan_task_backing(MAX_PLANNED_PRIVATE_PAGES + 1).is_none());
        let mut regions = [Some(UntypedRegion {
            cap: sel4::cap::Untyped::from_bits(1),
            paddr: 0x8000_0000,
            size_bits: 31,
            watermark: 128 * 1024 * 1024,
        })];
        assert!(task_backing_extents_fit_in(
            &mut regions,
            holder,
            TaskStaticBacking {
                allocation_descriptors: 520,
                reserved_bytes: 4 * 1024 * 1024,
            },
            4
        ));
    }

    #[test]
    fn four_holder_capacity_refuses_every_real_resource_shortfall() {
        const PAGES_256_MIB: usize = 256 * 1024 * 1024 / 4096;
        let plan = plan_task_backing(PAGES_256_MIB).unwrap();
        let static_backing = TaskStaticBacking {
            allocation_descriptors: 8,
            reserved_bytes: 16_384,
        };
        let mut capacity = TaskBackingCapacity {
            plan,
            static_backing,
            holders: 4,
            cslots_available: usize::MAX,
            allocation_descriptors_available: usize::MAX,
            extent_descriptors_available: usize::MAX,
            ordinary_bytes_available: usize::MAX,
            ordinary_layout_fits: true,
            root_image_bytes: 0,
            root_stack_bytes: 1024 * 1024,
            root_heap_bytes: 512 * 1024,
        };
        let required = capacity.requirements().unwrap();
        assert_eq!(required.extent_descriptors, 257 * 4);
        capacity.cslots_available = required.cslots;
        capacity.allocation_descriptors_available = required.allocation_descriptors;
        capacity.extent_descriptors_available = required.extent_descriptors;
        capacity.ordinary_bytes_available = required.reserved_bytes;
        assert!(capacity.fits());
        assert!(
            !TaskBackingCapacity {
                cslots_available: required.cslots - 1,
                ..capacity
            }
            .fits()
        );
        assert!(
            !TaskBackingCapacity {
                allocation_descriptors_available: required.allocation_descriptors - 1,
                ..capacity
            }
            .fits()
        );
        assert!(
            !TaskBackingCapacity {
                extent_descriptors_available: required.extent_descriptors - 1,
                ..capacity
            }
            .fits()
        );
        assert!(
            !TaskBackingCapacity {
                ordinary_bytes_available: required.reserved_bytes - 1,
                ..capacity
            }
            .fits()
        );
        capacity.ordinary_layout_fits = false;
        capacity.ordinary_bytes_available = required.reserved_bytes;
        assert!(!capacity.fits());
    }

    #[test]
    fn capacity_rejects_fragmented_untyped_alignment_false_positive() {
        let plan = plan_task_backing(512).unwrap();
        let static_backing = TaskStaticBacking {
            allocation_descriptors: 8,
            reserved_bytes: 16_384,
        };
        let region = |slot: usize, size_bits: usize, watermark: usize| {
            Some(UntypedRegion {
                cap: sel4::cap::Untyped::from_bits(slot as _),
                paddr: (slot - 1) << 21,
                size_bits,
                watermark,
            })
        };
        let mut fragmented = [
            region(1, 21, GRANULE_BYTES),
            region(2, 21, GRANULE_BYTES),
            region(3, 21, GRANULE_BYTES),
        ];
        let ordinary_bytes_available: usize = fragmented
            .iter()
            .flatten()
            .map(UntypedRegion::remaining)
            .sum();
        assert!(ordinary_bytes_available >= plan.reserved_bytes + static_backing.reserved_bytes);
        assert!(!task_backing_extents_fit_in(
            &mut fragmented,
            plan,
            static_backing,
            1,
        ));

        let mut contiguous = [region(1, 23, 0)];
        assert!(task_backing_extents_fit_in(
            &mut contiguous,
            plan,
            static_backing,
            1,
        ));
    }
    #[test]
    fn static_backing_counts_aliases_and_excludes_private_records() {
        let id = TaskArenaId::from_raw(2, 7);
        let other = TaskArenaId::from_raw(3, 7);
        let stale = TaskArenaId::from_raw(2, 8);
        let record =
            |owner: TaskArenaId, extent: u16, allocation: ArenaAllocation| AllocationRecord {
                owner: owner.index as u16,
                extent,
                serial: owner.serial,
                allocation,
                next_state: PRIVATE_STATE_NONE,
            };
        let mut private_empty = ArenaAllocation::new(12, 0, true, true);
        private_empty.set_private_state(PrivateObjectKind::Empty, 0, true, false);
        let mut private_frame = ArenaAllocation::new(13, 12, true, true);
        private_frame.set_private_state(PrivateObjectKind::Granule, 12, true, false);
        let mut private_leaf = ArenaAllocation::new(14, 12, true, true);
        private_leaf.set_private_state(PrivateObjectKind::LeafTable, 12, true, true);
        let allocations = [
            record(id, 0, ArenaAllocation::new(10, 12, false, false)),
            record(
                id,
                PRIVATE_EXTENT_NONE,
                ArenaAllocation::new(11, 0, false, false),
            ),
            record(id, PRIVATE_EXTENT_NONE, private_empty),
            record(id, 1, private_frame),
            record(id, 2, private_leaf),
            record(other, 0, ArenaAllocation::new(15, 12, false, false)),
            record(stale, 0, ArenaAllocation::new(16, 12, false, false)),
        ];
        let static_extent = |owner: TaskArenaId, revoked: bool| ExtentRecord {
            parent: sel4::cap::Untyped::from_bits(1),
            size_bits: 16,
            owner: owner.index as u16,
            serial: owner.serial,
            kind: ExtentKind::Static,
            active: true,
            revoked,
            watermark: 4096,
            objects: 1,
            bytes: 4096,
        };
        assert_eq!(
            task_static_backing_from_records(id, &allocations, &[Some(static_extent(id, false))]),
            Some(TaskStaticBacking {
                allocation_descriptors: 2,
                reserved_bytes: 65_536,
            })
        );
        assert!(task_static_backing_from_records(id, &allocations, &[]).is_none());
        assert!(
            task_static_backing_from_records(id, &allocations, &[Some(static_extent(id, true))])
                .is_none()
        );
        assert!(
            task_static_backing_from_records(
                id,
                &allocations,
                &[
                    Some(static_extent(id, false)),
                    Some(static_extent(id, false))
                ]
            )
            .is_none()
        );
        let mut overflow = static_extent(id, false);
        overflow.size_bits = usize::BITS as usize;
        assert!(task_static_backing_from_records(id, &allocations, &[Some(overflow)]).is_none());
        let mut private_extent = static_extent(id, false);
        private_extent.kind = ExtentKind::PrivateData;
        assert!(
            task_static_backing_from_records(id, &allocations, &[Some(private_extent)]).is_none()
        );
    }

    #[test]
    fn four_holders_include_static_descriptors_at_default_pool_boundary() {
        let plan = plan_task_backing(65_536).unwrap();
        let capacity = TaskBackingCapacity {
            plan,
            static_backing: TaskStaticBacking {
                allocation_descriptors: 1,
                reserved_bytes: 4096,
            },
            holders: 4,
            cslots_available: usize::MAX,
            allocation_descriptors_available: 262_657,
            extent_descriptors_available: plan.extent_descriptors * 4,
            ordinary_bytes_available: usize::MAX,
            ordinary_layout_fits: true,
            root_image_bytes: 0,
            root_stack_bytes: 0,
            root_heap_bytes: 0,
        };
        let required = capacity.requirements().unwrap();
        assert_eq!(required.allocation_descriptors, 263_172);
        assert_eq!(
            required.allocation_descriptors - capacity.allocation_descriptors_available,
            515
        );
        assert!(!capacity.fits());
    }

    #[test]
    fn capacity_overflow_is_refused_and_zero_holders_are_empty() {
        let plan = plan_task_backing(1).unwrap();
        let capacity = TaskBackingCapacity {
            plan,
            static_backing: TaskStaticBacking {
                allocation_descriptors: usize::MAX,
                reserved_bytes: usize::MAX,
            },
            holders: 1,
            cslots_available: usize::MAX,
            allocation_descriptors_available: usize::MAX,
            extent_descriptors_available: usize::MAX,
            ordinary_bytes_available: usize::MAX,
            ordinary_layout_fits: true,
            root_image_bytes: 0,
            root_stack_bytes: 0,
            root_heap_bytes: 0,
        };
        assert_eq!(capacity.requirements(), None);
        assert!(!capacity.fits());
        let multiplied = TaskBackingCapacity {
            static_backing: TaskStaticBacking {
                allocation_descriptors: 0,
                reserved_bytes: 0,
            },
            holders: usize::MAX,
            ..capacity
        };
        assert_eq!(multiplied.requirements(), None);
        assert!(!multiplied.fits());
        let zero = TaskBackingCapacity {
            holders: 0,
            cslots_available: 0,
            allocation_descriptors_available: 0,
            extent_descriptors_available: 0,
            ordinary_bytes_available: 0,
            ..capacity
        };
        assert_eq!(
            zero.requirements(),
            Some(super::TaskBackingRequirements {
                cslots: 0,
                allocation_descriptors: 0,
                extent_descriptors: 0,
                reserved_bytes: 0,
            })
        );
        assert!(zero.fits());
    }

    #[test]
    fn single_page_growth_visits_only_selected_records() {
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(|| {
                let (mut allocator, arena) = setup_private_growth_fixture();
                let mut table = Table::new();
                let mut region = Region::reserved(0x1000_0000, 512);
                let vspace = sel4::cap::VSpace::from_bits(7);
                let mut kernel = RecordingPrivateKernel::default();
                for page in 0..512 {
                    assert_eq!(
                        table.grow_with_kernel(
                            &mut allocator,
                            arena,
                            vspace,
                            &mut region,
                            1,
                            &mut kernel,
                        ),
                        Ok(page)
                    );
                }
                assert_eq!(region.base(), 0x1000_0000);
                assert_eq!(region.pages(), 512);
                assert_eq!(region.base_frames(), 512);
                assert_eq!(region.large_frames(), 0);
                assert_eq!(region.leaf_tables(), 1);
                assert_eq!(table.total_pages(), 512);
                assert_eq!(table.grants(), 512);
                assert_eq!(table.grown_pages(), 512);
                assert_eq!(
                    allocator.private_record_visits(),
                    PrivateRecordVisits {
                        reusable: 513,
                        mark: 513,
                        reset: 0,
                        commit: 513,
                        unwind: 0,
                    }
                );
                assert_eq!(
                    kernel
                        .requests
                        .iter()
                        .filter(|request| matches!(request, KernelRequest::Retype { .. }))
                        .count(),
                    513
                );
                assert_eq!(
                    kernel
                        .requests
                        .iter()
                        .filter(|request| matches!(request, KernelRequest::MapLeaf { .. }))
                        .count(),
                    1
                );
                assert_eq!(
                    kernel
                        .requests
                        .iter()
                        .filter(|request| matches!(request, KernelRequest::MapFrame { .. }))
                        .count(),
                    512
                );
                assert!(
                    !kernel
                        .requests
                        .iter()
                        .any(|request| matches!(request, KernelRequest::UnmapFrame { .. }))
                );
                let records: Vec<_> = allocator
                    .allocations
                    .iter()
                    .filter(|record| record.belongs_to(arena) && record.allocation.is_private())
                    .collect();
                assert_eq!(records.len(), 514);
                assert_eq!(
                    records
                        .iter()
                        .filter(
                            |record| record.allocation.private_kind() == PrivateObjectKind::Empty
                        )
                        .count(),
                    1
                );
                assert!(
                    records
                        .iter()
                        .filter(|record| {
                            record.allocation.private_kind() != PrivateObjectKind::Empty
                        })
                        .all(|record| record.allocation.is_mapped()
                            && !record.allocation.is_reusable())
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn private_state_indices_reject_wrong_ownership() {
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(|| {
                let (mut allocator, arena) = setup_private_growth_fixture();
                let valid = allocator.arenas[arena.index()].reusable_heads
                    [PrivateObjectKind::Empty as usize];
                let snapshot = allocator.allocations[valid as usize];
                allocator.allocations[valid as usize].owner = 1;
                let mut kernel = RecordingPrivateKernel::default();
                assert!(matches!(
                    allocator.acquire_private_in(
                        arena,
                        PrivateObjectKind::Granule,
                        sel4::cap_type::Granule::object_blueprint(),
                        &mut kernel,
                    ),
                    Err(AllocError::ArenaSlotTableFull {
                        limit: MAX_TASK_ALLOCATIONS,
                    })
                ));
                allocator.allocations[valid as usize] = snapshot;
                assert_eq!(
                    allocator.arenas[arena.index()].reusable_heads
                        [PrivateObjectKind::Empty as usize],
                    valid
                );
                allocator.arenas[arena.index()].releasing = true;
                assert_eq!(
                    allocator.provision_private_backing(arena, 1),
                    Err(AllocError::UnknownArena(arena))
                );
                assert_eq!(
                    allocator.commit_private_transaction(arena),
                    Err(AllocError::UnknownArena(arena))
                );
                assert_eq!(
                    allocator.unwind_private_transaction(arena, &mut kernel),
                    Err(AllocError::UnknownArena(arena))
                );
                assert!(kernel.requests.is_empty());
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn large_descriptor_tables_cover_retry_safe_plans_and_static_tasks() {
        if LARGE_DESCRIPTOR_TABLES {
            assert_eq!(MAX_PLANNED_PRIVATE_SPANS, 128);
            assert_eq!(MAX_PLANNED_PRIVATE_ALLOCATIONS, 65_792);
            assert!(MAX_PLANNED_STATIC_ALLOCATIONS >= 520);
            assert_eq!(
                MAX_TASK_ALLOCATIONS,
                4 * (MAX_PLANNED_PRIVATE_ALLOCATIONS + MAX_PLANNED_STATIC_ALLOCATIONS)
            );
            assert_eq!(MAX_TASK_EXTENTS, MAX_TASK_ARENAS + 4 * 2 * 128);
        }
    }

    #[test]
    fn failed_large_extent_revoke_retains_ownership_then_reaches_base_page_quota() {
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(|| {
                let (mut allocator, arena) = setup_private_growth_fixture();
                let mut table = Table::new();
                let mut region = Region::reserved(0x1000_0000, 512);
                let vspace = sel4::cap::VSpace::from_bits(7);
                let mut kernel = RecordingPrivateKernel {
                    fail_map_frame_at: Some(1),
                    ..Default::default()
                };
                assert!(
                    table
                        .grow_with_kernel(
                            &mut allocator,
                            arena,
                            vspace,
                            &mut region,
                            512,
                            &mut kernel
                        )
                        .is_err()
                );
                let position = allocator.arenas[arena.index()].reusable_heads
                    [PrivateObjectKind::LargeFrame as usize]
                    as usize;
                let extent_index = allocator.allocations[position].extent as usize;
                let parent = allocator.extents[extent_index].unwrap().parent;
                kernel.fail_map_frame_at = None;
                kernel.fail_revoke = true;
                for _ in 0..2 {
                    assert!(
                        table
                            .grow_with_kernel(
                                &mut allocator,
                                arena,
                                vspace,
                                &mut region,
                                1,
                                &mut kernel
                            )
                            .is_err()
                    );
                    assert_eq!(region.pages(), 0);
                    assert_eq!(table.total_pages(), 0);
                    assert_eq!(
                        allocator.allocations[position].allocation.private_kind(),
                        PrivateObjectKind::LargeFrame
                    );
                    assert!(allocator.allocations[position].allocation.is_reusable());
                    let extent = allocator.extents[extent_index].unwrap();
                    assert!(extent.belongs_to(arena));
                    assert_eq!(extent.objects, 1);
                    assert_eq!(extent.bytes, MAX_PRIVATE_EXTENT_BYTES);
                    assert_eq!(extent.watermark, MAX_PRIVATE_EXTENT_BYTES);
                    assert_eq!(extent.parent, parent);
                }
                kernel.fail_revoke = false;
                for page in 0..512 {
                    assert_eq!(
                        table.grow_with_kernel(
                            &mut allocator,
                            arena,
                            vspace,
                            &mut region,
                            1,
                            &mut kernel
                        ),
                        Ok(page)
                    );
                }
                assert_eq!(region.base_frames(), 512);
                assert_eq!(region.large_frames(), 0);
                assert_eq!(allocator.live_objects(), 513);
                assert_eq!(
                    allocator.live_bytes(),
                    MAX_PRIVATE_EXTENT_BYTES + GRANULE_BYTES
                );
                assert_eq!(allocator.extents[extent_index].unwrap().objects, 512);
                assert_eq!(allocator.extents[extent_index].unwrap().parent, parent);
                assert_eq!(
                    kernel
                        .requests
                        .iter()
                        .filter(|request| matches!(request, KernelRequest::Revoke { .. }))
                        .count(),
                    3
                );
                assert!(
                    kernel
                        .requests
                        .iter()
                        .filter_map(|request| match request {
                            KernelRequest::Revoke { slot } => Some(*slot),
                            _ => None,
                        })
                        .all(|slot| slot == parent.bits() as usize)
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn one_page_growth_and_failed_large_frames_are_both_counted() {
        const PAGES_256_MIB: usize = 256 * 1024 * 1024 / 4096;
        let holder = plan_task_backing(PAGES_256_MIB).unwrap();
        assert_eq!(holder.allocation_descriptors, PAGES_256_MIB + 256);
        assert_eq!(holder.required_cslots, holder.allocation_descriptors + 257);
        assert!(holder.required_cslots > 4096);
    }

    #[test]
    fn allocation_metadata_is_root_wide_and_compact() {
        assert_eq!(core::mem::size_of::<ArenaAllocation>(), 4);
        assert!(core::mem::size_of::<AllocationRecord>() <= 16);
        assert!(
            core::mem::size_of::<ArenaRecord>()
                < core::mem::size_of::<[ArenaAllocation; MAX_TASK_ALLOCATIONS]>()
        );
        let mut allocation = ArenaAllocation::new(40, 21, true, false);
        allocation.set_private_state(PrivateObjectKind::LargeFrame, 21, true, true);
        assert_eq!(allocation.slot(), 40);
        assert_eq!(allocation.private_kind(), PrivateObjectKind::LargeFrame);
        assert!(allocation.is_reusable());
        assert!(allocation.is_mapped());
    }

    /// The allocator's tables are `.bss`, and the seL4 loader spends one root
    /// CSlot per page of the root image before the root runs. Tables the
    /// booting kernel's own CNode cannot pay for make the image unbootable
    /// before admission is ever evaluated, so they must be sized against the
    /// kernel actually linked, not against the widest one supported. A
    /// quarter of the CSpace is the budget: the rest carries the product
    /// graph's own objects, which `admit_total_slots` then counts.
    #[test]
    fn allocator_state_fits_the_linked_kernels_root_cspace() {
        let image_slots = core::mem::size_of::<ObjectAllocator>().div_ceil(GRANULE_BYTES);
        assert!(
            image_slots * 4 <= KERNEL_ROOT_CNODE_SLOTS,
            "allocator state spends {image_slots} of {KERNEL_ROOT_CNODE_SLOTS} root CSlots"
        );
        assert_eq!(
            LARGE_DESCRIPTOR_TABLES,
            MAX_TASK_ALLOCATIONS > 4096,
            "table size must follow the kernel width, not a hand-set flag"
        );
        assert_eq!(
            MAX_TASK_EXTENTS > 3 * MAX_TASK_ARENAS,
            LARGE_DESCRIPTOR_TABLES
        );
    }

    #[test]
    fn only_in_flight_records_are_the_failed_transactions_own_objects() {
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(|| {
                let (mut allocator, arena) = setup_private_growth_fixture();
                let mut table = Table::new();
                let mut region = Region::reserved(0x1000_0000, 512);
                let vspace = sel4::cap::VSpace::from_bits(7);
                let mut kernel = RecordingPrivateKernel::default();
                assert_eq!(
                    table.grow_with_kernel(
                        &mut allocator,
                        arena,
                        vspace,
                        &mut region,
                        1,
                        &mut kernel,
                    ),
                    Ok(0)
                );
                kernel.fail_retype_at = Some(kernel.retypes + 2);
                let error = table
                    .grow_with_kernel(&mut allocator, arena, vspace, &mut region, 2, &mut kernel)
                    .unwrap_err();
                assert!(matches!(
                    error,
                    crate::private_memory::GrowError::Frames { allocated: 1, .. }
                ));
                assert_eq!(region.pages(), 1);
                assert_eq!(region.base_frames(), 1);
                assert_eq!(region.leaf_tables(), 1);
                assert_eq!(table.total_pages(), 1);
                assert_eq!(table.grants(), 1);
                assert_eq!(
                    allocator.arenas[arena.index()].in_flight_head,
                    PRIVATE_STATE_NONE
                );
                assert_eq!(allocator.arenas[arena.index()].in_flight_granules, 0);
                assert_eq!(
                    kernel
                        .requests
                        .iter()
                        .filter(|request| matches!(request, KernelRequest::UnmapFrame { .. }))
                        .count(),
                    1
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn an_unwound_frame_stays_owned_and_typed_for_the_retry() {
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(|| {
                let (mut allocator, arena) = setup_private_growth_fixture();
                let mut table = Table::new();
                let mut region = Region::reserved(0x1000_0000, 512);
                let vspace = sel4::cap::VSpace::from_bits(7);
                let mut kernel = RecordingPrivateKernel {
                    fail_retype_at: Some(3),
                    ..RecordingPrivateKernel::default()
                };
                assert!(
                    table
                        .grow_with_kernel(
                            &mut allocator,
                            arena,
                            vspace,
                            &mut region,
                            2,
                            &mut kernel,
                        )
                        .is_err()
                );
                let retypes_after_failure = kernel.retypes;
                kernel.fail_retype_at = None;
                assert_eq!(
                    table.grow_with_kernel(
                        &mut allocator,
                        arena,
                        vspace,
                        &mut region,
                        2,
                        &mut kernel,
                    ),
                    Ok(0)
                );
                assert_eq!(kernel.retypes, retypes_after_failure + 1);
                assert_eq!(region.pages(), 2);
                assert_eq!(region.leaf_tables(), 1);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn an_unwound_leaf_table_is_retained_mapped() {
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(|| {
                let (mut allocator, arena) = setup_private_growth_fixture();
                let mut table = Table::new();
                let mut region = Region::reserved(0x1000_0000, 512);
                let vspace = sel4::cap::VSpace::from_bits(7);
                let mut kernel = RecordingPrivateKernel {
                    fail_retype_at: Some(3),
                    ..RecordingPrivateKernel::default()
                };
                assert!(
                    table
                        .grow_with_kernel(
                            &mut allocator,
                            arena,
                            vspace,
                            &mut region,
                            2,
                            &mut kernel,
                        )
                        .is_err()
                );
                assert_eq!(region.pages(), 0);
                assert_eq!(region.leaf_tables(), 1);
                assert_eq!(
                    kernel
                        .requests
                        .iter()
                        .filter(|request| matches!(request, KernelRequest::MapLeaf { .. }))
                        .count(),
                    1
                );
                kernel.fail_retype_at = None;
                assert_eq!(
                    table.grow_with_kernel(
                        &mut allocator,
                        arena,
                        vspace,
                        &mut region,
                        2,
                        &mut kernel,
                    ),
                    Ok(0)
                );
                assert_eq!(
                    kernel
                        .requests
                        .iter()
                        .filter(|request| matches!(request, KernelRequest::MapLeaf { .. }))
                        .count(),
                    1
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
