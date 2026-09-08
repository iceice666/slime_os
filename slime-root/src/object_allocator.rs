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

#[cfg(slime_b38_force_unwind)]
static FORCE_UNWIND_ONCE: AtomicBool = AtomicBool::new(true);

#[cfg(slime_b38_force_unwind)]
pub(crate) fn take_forced_unwind() -> bool {
    FORCE_UNWIND_ONCE.swap(false, Ordering::Relaxed)
}

pub const MAX_KERNEL_UNTYPEDS: usize = 64;
pub const MAX_DEVICE_UNTYPEDS: usize = 64;
/// Maximum root CSpace width admitted by the software slot bitmap.
///
/// The bitmap can describe the 19-bit QEMU CNodes MEM-ARENAS sizes below, but
/// `SlotPool::initialize` still accepts only the BootInfo span the selected
/// platform actually exposes. The Duo kernel keeps its 12-bit CNode and its
/// three-extents-per-task descriptor table.
pub const MAX_ROOT_CSLOTS: usize = 524_288;
/// Maximum simultaneously owned task-backing records.
pub const MAX_TASK_ARENAS: usize = 48;
/// Root-owned task allocation descriptors.
///
/// One-page-at-a-time growth needs one frame descriptor per page, plus one leaf
/// table per 2 MiB span. QEMU profiles reserve enough for four 256 MiB holders;
/// the Duo keeps the previous 4096-record envelope.
#[cfg(not(slime_cv1800b_duo))]
pub const MAX_TASK_ALLOCATIONS: usize = 4 * (MAX_PLANNED_PRIVATE_PAGES + 128) + 1;
#[cfg(slime_cv1800b_duo)]
pub const MAX_TASK_ALLOCATIONS: usize = 4096;
/// Every task consumes one static extent. A quota-bearing task additionally
/// consumes independently reclaimable data and page-table extents.
#[cfg(not(slime_cv1800b_duo))]
pub const MAX_TASK_EXTENTS: usize = MAX_TASK_ARENAS + 4 * (1 + 128 + 128);
#[cfg(slime_cv1800b_duo)]
pub const MAX_TASK_EXTENTS: usize = 3 * MAX_TASK_ARENAS;

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
/// Sizing this by root CSlot instead cost a boot, and the reason is the same
/// one recorded at length above `boot_selector::SELECTOR_GENERATION_BYTES`: in
/// this root a large static is not merely memory, it is *capacity*. The seL4
/// loader creates one root CSlot per page of the root image's `.bss` before the
/// root runs, so the `[usize; MAX_ROOT_CSLOTS]` this replaced — 2 MB of `.bss`
/// for 262_144 conceivable slots — spent 512 root CSlots and made a previously
/// admissible generation unbootable, refused with
/// `PlanExceedsRootSlots { required: 2313, available: 2185 }`.
///
/// The live bound below is 448 entries, or 452 in the immutable selector image,
/// whose two bootstrap DMA pages per admitted boot device are the only terms
/// this product path no longer contributes. [`PROVENANCE_SLOTS`] rounds either
/// to a 1024-position open table of two-word records: 16 KiB of `.bss`, four
/// root CSlots, against the 512 the array spent.
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

/// Four-holder capacity against the QEMU root's actual live resource state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskBackingCapacity {
    pub plan: TaskBackingPlan,
    pub holders: usize,
    pub graph_cslots: usize,
    pub cslots_available: usize,
    pub allocation_descriptors_available: usize,
    pub extent_descriptors_available: usize,
    pub ordinary_bytes_available: usize,
    pub root_image_bytes: usize,
    pub root_stack_bytes: usize,
    pub root_heap_bytes: usize,
}

impl TaskBackingCapacity {
    pub fn fits(self) -> bool {
        let holder_cslots = self.plan.required_cslots.saturating_mul(self.holders);
        let holder_allocations = self
            .plan
            .allocation_descriptors
            .saturating_mul(self.holders);
        let holder_extents = self.plan.extent_descriptors.saturating_mul(self.holders);
        let holder_bytes = self.plan.reserved_bytes.saturating_mul(self.holders);
        holder_cslots.saturating_add(self.graph_cslots) <= self.cslots_available
            && holder_allocations <= self.allocation_descriptors_available
            && holder_extents <= self.extent_descriptors_available
            && holder_bytes <= self.ordinary_bytes_available
    }
}

/// Plan segmented private backing for host admission and capacity reports.
///
/// This deliberately accepts quotas above the current public runtime ceiling:
/// MEM-ARENAS must prove that 64 MiB and 256 MiB plans are representable before
/// a later milestone changes the authenticated ceiling. Data is split into
/// independently reclaimable 2 MiB extents; the adversarial one-page path owns
/// one CSlot/allocation descriptor per page plus one leaf table per 2 MiB span.
pub fn plan_task_backing(private_pages: usize) -> Option<TaskBackingPlan> {
    if private_pages > MAX_PLANNED_PRIVATE_PAGES {
        return None;
    }
    let data_extents = private_pages.div_ceil(MAX_PRIVATE_EXTENT_PAGES);
    let leaf_tables = data_extents;
    let allocation_descriptors = if private_pages == 0 {
        0
    } else {
        private_pages + leaf_tables
    };
    let extent_descriptors = if private_pages == 0 {
        1
    } else {
        1 + data_extents + leaf_tables
    };
    let payload_bytes = private_pages.checked_mul(GRANULE_BYTES)?;
    let page_table_bytes = leaf_tables.checked_mul(GRANULE_BYTES)?;
    let reserved_data = data_extents.checked_mul(MAX_PRIVATE_EXTENT_BYTES)?;
    let reserved_bytes = reserved_data.checked_add(page_table_bytes)?;
    Some(TaskBackingPlan {
        private_pages,
        data_extents,
        extent_descriptors,
        allocation_descriptors,
        required_cslots: allocation_descriptors + extent_descriptors,
        reserved_bytes,
        payload_bytes,
        page_table_bytes,
        alignment_waste: reserved_data - payload_bytes,
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
    serial: u32,
    extent: u32,
    allocation: ArenaAllocation,
}

impl AllocationRecord {
    const EMPTY: Self = Self {
        owner: u16::MAX,
        serial: 0,
        extent: u32::MAX,
        allocation: ArenaAllocation::EMPTY,
    };

    const fn belongs_to(&self, id: TaskArenaId) -> bool {
        self.owner as usize == id.index() && self.serial == id.serial
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ArenaRecord {
    serial: u32,
    active: bool,
    slot_len: usize,
}

impl ArenaRecord {
    const fn empty() -> Self {
        Self {
            serial: 0,
            active: false,
            slot_len: 0,
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
        && MAX_TASK_ALLOCATIONS <= u32::MAX as usize
        && MAX_TASK_EXTENTS <= u32::MAX as usize
);
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
        // Every page's provenance, recorded before the retype for the reason
        // `allocate_from_global` states, and unwound as a unit. A queue whose
        // pages are only partly recoverable is not a usable queue: the caller
        // maps all `count` of them into one contiguous device area, so a hole
        // would surface later as a mapping at the wrong address.
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

    fn arena_mut(&mut self, id: TaskArenaId) -> Result<&mut ArenaRecord, AllocError> {
        self.arenas
            .get_mut(id.index())
            .filter(|arena| arena.serial == id.serial && arena.active)
            .ok_or(AllocError::UnknownArena(id))
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
            serial: id.serial,
            extent: extent.map_or(u32::MAX, |index| index as u32),
            allocation,
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

    /// Root CSlots still available to issue.
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

    /// Return one root CSlot to the free bitmap after its capability is gone.
    pub fn release_slot(&mut self, slot: usize) -> bool {
        self.physical.remove(slot);
        self.slots.release(slot)
    }

    pub fn arena_slot_count(&self, id: TaskArenaId) -> Result<usize, AllocError> {
        Ok(self.arena(id)?.slot_len)
    }

    /// Provision every private backing extent and CSlot before task publication.
    pub fn provision_private_backing(
        &mut self,
        id: TaskArenaId,
        quota: usize,
        slot_count: usize,
    ) -> Result<(), AllocError> {
        let quota = quota.min(crate::private_memory::MAX_REGION_PAGES);
        if quota != 0 {
            self.provision_extent(
                id,
                GRANULE_BYTES.trailing_zeros() as usize,
                ExtentKind::PrivateTables,
            )?;
            let data_bytes = quota
                .checked_mul(GRANULE_BYTES)
                .ok_or(AllocError::ArenaTooSmall {
                    size_bits: usize::BITS as usize,
                    required: usize::MAX,
                })?;
            let data_bits =
                usize::BITS as usize - data_bytes.max(1).saturating_sub(1).leading_zeros() as usize;
            self.provision_extent(id, data_bits, ExtentKind::PrivateData)?;
            if quota == MAX_PRIVATE_EXTENT_PAGES {
                self.provision_extent(id, data_bits, ExtentKind::PrivateData)?;
            }
        }
        self.provision_private_slots(id, slot_count)
    }

    pub fn provision_private_slots(
        &mut self,
        id: TaskArenaId,
        count: usize,
    ) -> Result<(), AllocError> {
        self.arena(id)?;
        if count > self.allocation_descriptors_free() || count > self.free_slots() {
            return Err(AllocError::ArenaSlotTableFull {
                limit: MAX_TASK_ALLOCATIONS,
            });
        }
        for _ in 0..count {
            let slot = self.take_slot()?;
            if let Err(error) =
                self.push_allocation(id, None, ArenaAllocation::new(slot, 0, true, true))
            {
                self.slots.release(slot);
                return Err(error);
            }
        }
        Ok(())
    }

    fn take_private_slot(
        &mut self,
        id: TaskArenaId,
        kind: PrivateObjectKind,
        size_bits: usize,
    ) -> Result<(usize, usize, bool, bool), AllocError> {
        if let Some((position, record)) =
            self.allocations.iter_mut().enumerate().find(|(_, record)| {
                record.belongs_to(id)
                    && record.allocation.is_private()
                    && record.allocation.is_reusable()
                    && record.allocation.private_kind() == kind
                    && record.allocation.size_bits() == size_bits
            })
        {
            let mapped = record.allocation.is_mapped();
            record
                .allocation
                .set_private_state(kind, size_bits, false, mapped);
            return Ok((position, record.extent as usize, true, mapped));
        }
        let position = self
            .allocations
            .iter()
            .position(|record| {
                record.belongs_to(id)
                    && record.allocation.is_private()
                    && record.allocation.is_reusable()
                    && record.allocation.private_kind() == PrivateObjectKind::Empty
            })
            .ok_or(AllocError::ArenaSlotTableFull {
                limit: MAX_TASK_ALLOCATIONS,
            })?;
        let extent_kind = if kind == PrivateObjectKind::LeafTable {
            ExtentKind::PrivateTables
        } else {
            ExtentKind::PrivateData
        };
        let (extent, _) = self.extent_for_allocation(id, extent_kind, size_bits)?;
        let record = &mut self.allocations[position];
        record.extent = extent as u32;
        record
            .allocation
            .set_private_state(kind, size_bits, false, false);
        Ok((position, extent, false, false))
    }

    pub fn acquire_private_in(
        &mut self,
        id: TaskArenaId,
        kind: PrivateObjectKind,
        blueprint: sel4::ObjectBlueprint,
    ) -> Result<(sel4::cap::Unspecified, bool, bool), AllocError> {
        debug_assert!(kind != PrivateObjectKind::Empty);
        let size_bits = blueprint.physical_size_bits();
        let (position, extent_index, reused, mapped) =
            self.take_private_slot(id, kind, size_bits)?;
        let slot = self.allocations[position].allocation.slot();
        if !reused {
            let (_, watermark) = self.extent_for_allocation(
                id,
                if kind == PrivateObjectKind::LeafTable {
                    ExtentKind::PrivateTables
                } else {
                    ExtentKind::PrivateData
                },
                size_bits,
            )?;
            let parent = self.extents[extent_index]
                .expect("private extent exists")
                .parent;
            if let Err(error) = parent.untyped_retype(
                &blueprint,
                &sel4::init_thread::slot::CNODE
                    .cap()
                    .absolute_cptr_for_self(),
                slot,
                1,
            ) {
                self.allocations[position].extent = u32::MAX;
                self.allocations[position].allocation.set_private_state(
                    PrivateObjectKind::Empty,
                    0,
                    true,
                    false,
                );
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
        Ok((sel4::cap::Unspecified::from_bits(slot as _), reused, mapped))
    }

    pub fn mark_private_in_flight(
        &mut self,
        id: TaskArenaId,
        cap: sel4::cap::Unspecified,
    ) -> Result<(), AllocError> {
        let slot = cap.bits() as usize;
        let record = self
            .allocations
            .iter_mut()
            .find(|record| record.belongs_to(id) && record.allocation.slot() == slot)
            .ok_or(AllocError::ArenaSlotTableFull {
                limit: MAX_TASK_ALLOCATIONS,
            })?;
        record.allocation.set_in_flight(true);
        Ok(())
    }

    pub fn commit_private_transaction(&mut self, id: TaskArenaId) -> Result<(), AllocError> {
        self.arena(id)?;
        for record in &mut self.allocations {
            if record.belongs_to(id) && record.allocation.is_in_flight() {
                record.allocation.set_in_flight(false);
            }
        }
        Ok(())
    }

    /// Revoke every extent touched by a failed growth transaction.
    ///
    /// Private objects are provisioned into independent parents, so revoking
    /// each touched parent destroys the failed attempt before its slots return
    /// to the task's pre-provisioned pool. A retry therefore uses the original
    /// reservation; it never needs a second payload-sized set of extents.
    pub fn unwind_private_transaction(&mut self, id: TaskArenaId) -> Result<(), AllocError> {
        self.arena(id)?;
        let root = sel4::init_thread::slot::CNODE.cap();
        for index in 0..self.extents.len() {
            let touched = self.allocations.iter().any(|record| {
                record.belongs_to(id)
                    && record.allocation.is_in_flight()
                    && record.extent as usize == index
            });
            if !touched {
                continue;
            }
            let extent = self.extents[index].expect("in-flight extent exists");
            let parent_slot = extent.parent.bits() as usize;
            root.absolute_cptr(sel4::CPtr::from_bits(parent_slot as _))
                .revoke()
                .map_err(|error| AllocError::ArenaCleanup {
                    slot: parent_slot,
                    error,
                })?;
            self.live_objects = self.live_objects.saturating_sub(extent.objects);
            self.live_bytes = self.live_bytes.saturating_sub(extent.bytes);
            let extent = self.extents[index].as_mut().expect("extent exists");
            extent.watermark = 0;
            extent.objects = 0;
            extent.bytes = 0;
            for record in &mut self.allocations {
                if record.belongs_to(id) && record.extent as usize == index {
                    record.extent = u32::MAX;
                    record
                        .allocation
                        .set_private_state(PrivateObjectKind::Empty, 0, true, false);
                }
            }
        }
        Ok(())
    }

    pub fn reset_private_in(
        &mut self,
        id: TaskArenaId,
        cap: sel4::cap::Unspecified,
        kind: PrivateObjectKind,
        size_bits: usize,
    ) -> Result<(), AllocError> {
        let slot = cap.bits() as usize;
        let position = self
            .allocations
            .iter()
            .position(|record| {
                record.belongs_to(id)
                    && !record.allocation.is_reusable()
                    && record.allocation.slot() == slot
                    && record.allocation.private_kind() == kind
                    && record.allocation.size_bits() == size_bits
            })
            .ok_or(AllocError::ArenaSlotTableFull {
                limit: MAX_TASK_ALLOCATIONS,
            })?;
        let extent_index = self.allocations[position].extent as usize;
        self.allocations[position].extent = u32::MAX;
        self.allocations[position].allocation.set_private_state(
            PrivateObjectKind::Empty,
            0,
            true,
            false,
        );
        let bytes = 1usize << size_bits;
        if let Some(extent) = self.extents.get_mut(extent_index).and_then(Option::as_mut) {
            extent.objects = extent.objects.saturating_sub(1);
            extent.bytes = extent.bytes.saturating_sub(bytes);
        }
        self.live_objects = self.live_objects.saturating_sub(1);
        self.live_bytes = self.live_bytes.saturating_sub(bytes);
        Ok(())
    }

    /// Revoke every backing extent before returning any task-owned CSlot.
    /// Completed revokes are recorded so a later failure is retryable exactly once.
    pub fn release_task_arena(&mut self, id: TaskArenaId) -> Result<usize, AllocError> {
        let released = self.arena(id)?.slot_len;
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
                    size_bits: 12,
                    error,
                });
            }

            completed += chunk_len;
            self.devices[region_index].as_mut().unwrap().retyped += chunk_len;
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
        AllocError, AllocationRecord, ArenaAllocation, ArenaPlan, ArenaRecord,
        MAX_PHYSICAL_PROVENANCE, MAX_PLANNED_PRIVATE_PAGES, MAX_PRIVATE_EXTENT_BYTES,
        MAX_PRIVATE_EXTENT_PAGES, MAX_TASK_ALLOCATIONS, PROVENANCE_SLOTS, PrivateObjectKind,
        ProvenanceTable, SlotPool, TaskBackingCapacity, device_retype_plan, plan_allocation,
        plan_task_backing,
    };
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
        assert!(holder.reserved_bytes < 512 * 1024 * 1024);
        let four_reserved = holder.reserved_bytes * 4;
        assert!(four_reserved < 2 * 1024 * 1024 * 1024usize);

        let current = plan_task_backing(MAX_PRIVATE_EXTENT_PAGES).unwrap();
        assert_eq!(current.data_extents, 1);
        assert_eq!(current.reserved_bytes, MAX_PRIVATE_EXTENT_BYTES + 4096);
        assert!(plan_task_backing(MAX_PLANNED_PRIVATE_PAGES + 1).is_none());
    }

    #[test]
    fn four_holder_capacity_refuses_every_real_resource_shortfall() {
        const PAGES_256_MIB: usize = 256 * 1024 * 1024 / 4096;
        let plan = plan_task_backing(PAGES_256_MIB).unwrap();
        let capacity = TaskBackingCapacity {
            plan,
            holders: 4,
            graph_cslots: 1,
            cslots_available: plan.required_cslots * 4 + 1,
            allocation_descriptors_available: plan.allocation_descriptors * 4,
            extent_descriptors_available: plan.extent_descriptors * 4,
            ordinary_bytes_available: 2 * 1024 * 1024 * 1024usize,
            root_image_bytes: 0,
            root_stack_bytes: 1024 * 1024,
            root_heap_bytes: 512 * 1024,
        };
        assert!(capacity.fits());
        assert!(
            !TaskBackingCapacity {
                cslots_available: capacity.cslots_available - 1,
                ..capacity
            }
            .fits()
        );
        assert!(
            !TaskBackingCapacity {
                allocation_descriptors_available: capacity.allocation_descriptors_available - 1,
                ..capacity
            }
            .fits()
        );
        assert!(
            !TaskBackingCapacity {
                extent_descriptors_available: capacity.extent_descriptors_available - 1,
                ..capacity
            }
            .fits()
        );
        assert!(
            !TaskBackingCapacity {
                ordinary_bytes_available: plan.reserved_bytes * 4 - 1,
                ..capacity
            }
            .fits()
        );
    }

    #[test]
    fn one_page_growth_cost_is_not_hidden_by_large_frames() {
        const PAGES_256_MIB: usize = 256 * 1024 * 1024 / 4096;
        let holder = plan_task_backing(PAGES_256_MIB).unwrap();
        assert_eq!(holder.allocation_descriptors, PAGES_256_MIB + 128);
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
}
