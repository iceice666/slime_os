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
/// Maximum BootInfo empty-slot span accepted by the reusable slot allocator.
pub const MAX_ROOT_CSLOTS: usize = 262_144;
/// Maximum simultaneously provisioned task-arena parents.
pub const MAX_TASK_ARENAS: usize = 48;
/// Root capabilities a single accepted task image can consume.
///
/// Image pages, a fixed overhead for the VSpace, tables, CNode, TCBs and the
/// per-thread page trio, and — since C10.1 — every leaf frame a task's private
/// memory quota authorizes. The private term is the *reservation* rather than
/// any one generation's declared quota, because a slot table sized to a
/// particular manifest would refuse a later one at its first growth with
/// `ArenaSlotTableFull`, which reads as an allocator defect rather than as the
/// bound it is.
pub const MAX_TASK_SLOTS: usize =
    crate::child_vspace::MAX_CHILD_IMAGE_PAGES + crate::private_memory::MAX_REGION_PAGES + 16;

const SLOT_WORD_BITS: usize = usize::BITS as usize;
const SLOT_WORDS: usize = MAX_ROOT_CSLOTS.div_ceil(SLOT_WORD_BITS);
const GRANULE_BYTES: usize = 4096;
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
}

impl ArenaPlan {
    pub const fn new() -> Self {
        Self { watermark: 0 }
    }

    pub fn add_size_bits(&mut self, size_bits: usize) -> Option<()> {
        let size = 1usize.checked_shl(u32::try_from(size_bits).ok()?)?;
        let start = self.watermark.checked_next_multiple_of(size)?;
        self.watermark = start.checked_add(size)?;
        Some(())
    }

    pub fn add(&mut self, blueprint: sel4::ObjectBlueprint) -> Option<()> {
        self.add_size_bits(blueprint.physical_size_bits())
    }

    pub const fn required_bytes(self) -> usize {
        self.watermark
    }

    pub fn required_size_bits(self) -> Option<usize> {
        let bytes = self.watermark.max(1);
        Some(usize::BITS as usize - bytes.saturating_sub(1).leading_zeros() as usize)
    }
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
    fn new(range: Range<usize>) -> Result<Self, AllocError> {
        let len = range.len();
        if len > MAX_ROOT_CSLOTS {
            return Err(AllocError::SlotRangeTooLarge {
                declared: len,
                limit: MAX_ROOT_CSLOTS,
            });
        }
        Ok(Self {
            base: range.start,
            len,
            used: [0; SLOT_WORDS],
            issued: [0; SLOT_WORDS],
            live: 0,
        })
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
struct ArenaRecord {
    serial: u32,
    parent: sel4::cap::Untyped,
    size_bits: usize,
    active: bool,
    watermark: usize,
    slots: [usize; MAX_TASK_SLOTS],
    slot_len: usize,
    objects: usize,
    bytes: usize,
}

impl ArenaRecord {
    const EMPTY_SLOT: usize = usize::MAX;

    fn new(serial: u32, parent: sel4::cap::Untyped, size_bits: usize) -> Self {
        Self {
            serial,
            parent,
            size_bits,
            active: false,
            watermark: 0,
            slots: [Self::EMPTY_SLOT; MAX_TASK_SLOTS],
            slot_len: 0,
            objects: 0,
            bytes: 0,
        }
    }

    fn id(&self, index: usize) -> TaskArenaId {
        TaskArenaId {
            index: index as u16,
            serial: self.serial,
        }
    }

    fn push_slot(&mut self, slot: usize) -> Result<(), AllocError> {
        let Some(dst) = self.slots.get_mut(self.slot_len) else {
            return Err(AllocError::ArenaSlotTableFull {
                limit: MAX_TASK_SLOTS,
            });
        };
        *dst = slot;
        self.slot_len += 1;
        Ok(())
    }

    /// Inverse of [`Self::push_slot`] for the most recent entry only: forget
    /// `slot` and rewind this arena's watermark and tallies by `size`.
    ///
    /// Refuses unless `slot` is the arena's top recorded entry. An arena is a
    /// bump allocator, so only the object at the watermark can be rewound;
    /// releasing from the middle would either strand the bytes or later hand
    /// out an overlapping region. Making that a refusal rather than a
    /// precondition is what keeps a caller unwinding in the wrong order from
    /// silently mis-accounting the arena.
    ///
    /// Pure bookkeeping: the caller is responsible for having emptied the slot
    /// and for returning the pool index, which this cannot reach.
    fn release_last(&mut self, slot: usize, size: usize) -> Result<(), AllocError> {
        let last = self
            .slot_len
            .checked_sub(1)
            .ok_or(AllocError::ArenaSlotTableFull {
                limit: MAX_TASK_SLOTS,
            })?;
        let entry = self
            .slots
            .get_mut(last)
            .filter(|recorded| **recorded == slot)
            .ok_or(AllocError::ArenaSlotTableFull {
                limit: MAX_TASK_SLOTS,
            })?;
        *entry = Self::EMPTY_SLOT;
        self.slot_len = last;
        self.watermark = self.watermark.saturating_sub(size);
        self.objects = self.objects.saturating_sub(1);
        self.bytes = self.bytes.saturating_sub(size);
        Ok(())
    }
}

pub struct ObjectAllocator {
    slots: SlotPool,
    untypeds: [Option<UntypedRegion>; MAX_KERNEL_UNTYPEDS],
    untyped_len: usize,
    devices: [Option<DeviceRegion>; MAX_DEVICE_UNTYPEDS],
    device_len: usize,
    arenas: [Option<ArenaRecord>; MAX_TASK_ARENAS],
    next_arena_serial: u32,
    slots_allocated: usize,
    objects_allocated: usize,
    bytes_allocated: usize,
    live_objects: usize,
    live_bytes: usize,
    slots_reused: usize,
    arena_reuses: usize,
    last_paddr: usize,
    physical: ProvenanceTable,
}

impl ObjectAllocator {
    pub fn new(bootinfo: &sel4::BootInfo) -> Result<Self, AllocError> {
        let kernel_untypeds = bootinfo.kernel_untyped_range();
        let declared = kernel_untypeds.len();
        if declared > MAX_KERNEL_UNTYPEDS {
            return Err(AllocError::UntypedTableFull {
                limit: MAX_KERNEL_UNTYPEDS,
                declared,
            });
        }
        let mut untypeds = [None; MAX_KERNEL_UNTYPEDS];
        let mut untyped_len = 0;
        let descriptors = bootinfo.untyped_list();
        for index in kernel_untypeds {
            let Some(descriptor) = descriptors.get(index) else {
                continue;
            };
            if descriptor.is_device() {
                continue;
            }
            untypeds[untyped_len] = Some(UntypedRegion {
                cap: bootinfo.untyped().index(index).cap(),
                paddr: descriptor.paddr(),
                size_bits: descriptor.size_bits(),
                watermark: 0,
            });
            untyped_len += 1;
        }
        if untyped_len == 0 {
            return Err(AllocError::NoKernelUntyped);
        }

        let device_untypeds = bootinfo.device_untyped_range();
        let device_declared = device_untypeds.len();
        let mut devices = [None; MAX_DEVICE_UNTYPEDS];
        let mut device_len = 0;
        for index in device_untypeds {
            let Some(descriptor) = descriptors.get(index) else {
                continue;
            };
            let Some(dst) = devices.get_mut(device_len) else {
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
            device_len += 1;
        }
        Ok(Self {
            slots: SlotPool::new(bootinfo.empty().range())?,
            untypeds,
            untyped_len,
            devices,
            device_len,
            arenas: [None; MAX_TASK_ARENAS],
            next_arena_serial: 1,
            slots_allocated: 0,
            objects_allocated: 0,
            bytes_allocated: 0,
            live_objects: 0,
            live_bytes: 0,
            slots_reused: 0,
            arena_reuses: 0,
            last_paddr: 0,
            physical: ProvenanceTable::new(),
        })
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
    pub const fn arena_reuses(&self) -> usize {
        self.arena_reuses
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

    /// Begin one task lifetime. A reusable dedicated parent is provisioned on
    /// first use; subsequent lifetimes retype the arena from that same parent.
    pub fn begin_task_arena(&mut self, size_bits: usize) -> Result<TaskArenaId, AllocError> {
        let reusable = self.arenas.iter().position(|entry| {
            entry.is_some_and(|arena| !arena.active && arena.size_bits == size_bits)
        });
        let index = if let Some(index) = reusable {
            self.arena_reuses += 1;
            index
        } else {
            let index =
                self.arenas
                    .iter()
                    .position(Option::is_none)
                    .ok_or(AllocError::ArenaTableFull {
                        limit: MAX_TASK_ARENAS,
                    })?;
            let parent_slot = self.take_slot()?;
            let blueprint = sel4::ObjectBlueprint::Untyped { size_bits };
            if let Err(error) = self.allocate_from_global(blueprint, parent_slot) {
                self.slots.release(parent_slot);
                return Err(error);
            }
            let parent =
                sel4::init_thread::Slot::<sel4::cap_type::Untyped>::from_index(parent_slot).cap();
            self.arenas[index] = Some(ArenaRecord::new(0, parent, size_bits));
            index
        };

        let serial = self.next_arena_serial;
        self.next_arena_serial = self.next_arena_serial.wrapping_add(1).max(1);
        let arena = self.arenas[index]
            .as_mut()
            .ok_or(AllocError::ArenaTableFull {
                limit: MAX_TASK_ARENAS,
            })?;
        arena.serial = serial;
        arena.active = true;
        arena.watermark = 0;
        arena.slot_len = 0;
        arena.objects = 0;
        arena.bytes = 0;
        Ok(arena.id(index))
    }

    fn arena_mut(&mut self, id: TaskArenaId) -> Result<&mut ArenaRecord, AllocError> {
        self.arenas
            .get_mut(id.index())
            .and_then(Option::as_mut)
            .filter(|arena| arena.serial == id.serial && arena.active)
            .ok_or(AllocError::UnknownArena(id))
    }

    pub fn allocate_in(
        &mut self,
        id: TaskArenaId,
        blueprint: sel4::ObjectBlueprint,
    ) -> Result<sel4::init_thread::Slot<sel4::cap_type::Unspecified>, AllocError> {
        let size_bits = blueprint.physical_size_bits();
        if self.arena_mut(id)?.slot_len >= MAX_TASK_SLOTS {
            return Err(AllocError::ArenaSlotTableFull {
                limit: MAX_TASK_SLOTS,
            });
        }
        let slot = self.take_slot()?;
        let result = (|| {
            let arena = self.arena_mut(id)?;
            let (_, watermark) =
                plan_allocation(arena.watermark, 1usize << arena.size_bits, size_bits).ok_or(
                    AllocError::ArenaTooSmall {
                        size_bits: arena.size_bits,
                        required: arena.watermark.saturating_add(1usize << size_bits),
                    },
                )?;
            let anchor = arena.parent;
            anchor
                .untyped_retype(
                    &blueprint,
                    &sel4::init_thread::slot::CNODE
                        .cap()
                        .absolute_cptr_for_self(),
                    slot,
                    1,
                )
                .map_err(|error| AllocError::Retype { size_bits, error })?;
            arena.push_slot(slot)?;
            arena.watermark = watermark;
            arena.objects += 1;
            arena.bytes += 1usize << size_bits;
            Ok(())
        })();
        if let Err(error) = result {
            self.slots.release(slot);
            return Err(error);
        }
        self.objects_allocated += 1;
        self.live_objects += 1;
        self.bytes_allocated += 1usize << size_bits;
        self.live_bytes += 1usize << size_bits;
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
        if self.arena_mut(id)?.slot_len >= MAX_TASK_SLOTS {
            return Err(AllocError::ArenaSlotTableFull {
                limit: MAX_TASK_SLOTS,
            });
        }
        let slot = self.take_slot()?;
        if let Err(error) = self.arena_mut(id)?.push_slot(slot) {
            self.slots.release(slot);
            return Err(error);
        }
        Ok(sel4::init_thread::Slot::from_index(slot))
    }

    /// Root CSlots still available to issue.
    ///
    /// Admission compares the plan's total against this before any component
    /// starts (B49): the per-instance ceilings say each process fits, and this
    /// says they all fit together.
    pub fn free_slots(&self) -> usize {
        self.slots.free()
    }

    /// Return one root CSlot to the free bitmap.
    ///
    /// The caller must have emptied the slot first: the pool tracks
    /// availability, not occupancy, so releasing a slot that still holds a
    /// capability makes the next allocation of that index fail `DeleteFirst`.
    /// Used by the shared-buffer adapter for frame aliases, which are the one
    /// root capability minted outside the arena path.
    ///
    /// Any physical-provenance record the slot held is dropped here, which is
    /// what keeps a root that allocates and releases DMA frames indefinitely
    /// from exhausting a table sized to the *live* population.
    pub fn release_slot(&mut self, slot: usize) -> bool {
        self.physical.remove(slot);
        self.slots.release(slot)
    }

    pub fn arena_slot_count(&self, id: TaskArenaId) -> Result<usize, AllocError> {
        self.arenas
            .get(id.index())
            .and_then(Option::as_ref)
            .filter(|arena| arena.serial == id.serial && arena.active)
            .map(|arena| arena.slot_len)
            .ok_or(AllocError::UnknownArena(id))
    }

    /// Undo the most recent [`Self::allocate_in`] on `arena`: forget the object,
    /// return its CSlot to the pool, and rewind the arena's watermark.
    ///
    /// The caller must already have emptied the slot — deleted the capability
    /// and, for a frame, unmapped it — on exactly the terms
    /// [`Self::release_slot`] states: the pool tracks availability, not
    /// occupancy, so returning an index that still holds a capability makes the
    /// next allocation there fail `DeleteFirst`.
    ///
    /// **Last-allocated only, and checked.** An arena is a bump allocator, so a
    /// watermark can only be rewound over the object at its top; releasing from
    /// the middle would either strand the bytes or hand out an overlapping
    /// region. Passing anything but the arena's last recorded slot is refused
    /// rather than silently mis-accounted, which makes the precondition a
    /// property the allocator enforces instead of one the caller must remember.
    /// An unwinding caller therefore returns its objects in reverse order.
    ///
    /// **The rewind is conservative by construction.** [`plan_allocation`]
    /// aligns each object's start *up* to its own size before adding it, so the
    /// watermark it produced is `aligned_start + size` and subtracting `size`
    /// yields exactly `aligned_start` — which is at or above the watermark the
    /// allocation began from. Any alignment padding therefore stays consumed
    /// rather than being handed out again, so the error can only ever be
    /// stranded bytes and never an overlapping region. For a run of same-sized
    /// objects, which is what a growth allocates, there is no padding and the
    /// rewind is exact.
    ///
    /// `slot` naming a foreign arena, not naming its top slot, and a
    /// `size_bits` that does not fit a `usize` all answer
    /// [`AllocError::UnknownArena`]. They are collapsed deliberately: each
    /// means the caller does not hold the allocation it claims to be returning,
    /// which is one bug with one correct response — leave the arena untouched.
    ///
    /// This exists because `release_task_arena` — the only other path that
    /// returns arena slots — runs at task death. Without it, a caller that
    /// allocates from an arena *while the task runs* and then fails part way
    /// has no way to give the slots back, and every retry leaks one `slot_len`
    /// against `MAX_TASK_SLOTS` for every allocation kind the arena serves
    /// (C10.1's growth unwind is the first such caller).
    pub fn release_last_in(
        &mut self,
        id: TaskArenaId,
        slot: usize,
        size_bits: usize,
    ) -> Result<(), AllocError> {
        let size = 1usize
            .checked_shl(u32::try_from(size_bits).map_err(|_| AllocError::UnknownArena(id))?)
            .ok_or(AllocError::UnknownArena(id))?;
        // The arena's own bookkeeping first, because it is the half that can
        // refuse: if `slot` is not its top entry nothing has been touched, so
        // returning early leaves the pool index alone rather than freeing an
        // index the arena still records.
        self.arena_mut(id)?.release_last(slot, size)?;
        self.slots.release(slot);
        self.live_objects = self.live_objects.saturating_sub(1);
        self.live_bytes = self.live_bytes.saturating_sub(size);
        Ok(())
    }

    /// Revoke the retained arena cap, empty every recorded CSlot, and only then
    /// return those indices to the bitmap. If revoke fails, the arena remains
    /// live and no slot is reused.
    ///
    /// The revoke alone is not sufficient. It drops what was *derived from the
    /// arena's own untyped*, which covers every object retyped by
    /// [`Self::allocate_in`]. But [`Self::reserve_slot_in`] charges a bare
    /// CSlot to the arena for lifetime purposes while its occupant is minted
    /// from an object the arena never owned — a globally allocated Endpoint or
    /// Notification (`peer_endpoint`/`notification` `install_instance`). No
    /// revoke of the arena parent can reach such a capability, so the slot
    /// survives teardown still occupied. Releasing it hands the pool an index
    /// the kernel still considers full, and the next `reserve_slot` there is
    /// refused `DeleteFirst` even though the bitmap is correct about
    /// availability — the pool tracks availability, never occupancy.
    ///
    /// Deleting is unconditional and idempotent: a slot the revoke already
    /// emptied deletes successfully as a no-op.
    pub fn release_task_arena(&mut self, id: TaskArenaId) -> Result<usize, AllocError> {
        let arena = *self
            .arenas
            .get(id.index())
            .and_then(Option::as_ref)
            .filter(|arena| arena.serial == id.serial && arena.active)
            .ok_or(AllocError::UnknownArena(id))?;
        let root_cnode = sel4::init_thread::slot::CNODE.cap();
        let parent_slot = arena.parent.bits() as usize;
        let cptr = root_cnode.absolute_cptr(sel4::CPtr::from_bits(parent_slot as sel4::CPtrBits));
        cptr.revoke().map_err(|error| AllocError::ArenaCleanup {
            slot: parent_slot,
            error,
        })?;

        for slot in arena.slots.iter().take(arena.slot_len).copied() {
            root_cnode
                .absolute_cptr(sel4::CPtr::from_bits(slot as sel4::CPtrBits))
                .delete()
                .map_err(|error| AllocError::ArenaCleanup { slot, error })?;
            self.slots.release(slot);
            self.physical.remove(slot);
        }
        self.live_objects = self.live_objects.saturating_sub(arena.objects);
        self.live_bytes = self.live_bytes.saturating_sub(arena.bytes);
        if let Some(record) = self.arenas[id.index()].as_mut() {
            record.active = false;
            record.watermark = 0;
            record.slot_len = 0;
            record.objects = 0;
            record.bytes = 0;
        }
        Ok(arena.slot_len)
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
        AllocError, ArenaPlan, ArenaRecord, MAX_PHYSICAL_PROVENANCE, MAX_TASK_SLOTS,
        PROVENANCE_SLOTS, ProvenanceTable, SlotPool, device_retype_plan, plan_allocation,
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

    /// `release_last` is the inverse `push_slot` lacked, and it shrinks the
    /// table only from its top: the released entry is cleared, the watermark and
    /// tallies rewind by exactly the object's size, and the freed position is
    /// reusable.
    ///
    /// Driven through the real function rather than by inlining its mutations,
    /// because the assertion that matters is the *guard*: a caller unwinding in
    /// the wrong order must be refused, not silently mis-accounted. The pool and
    /// kernel halves (`slots.release`, deleting the capability) need a live
    /// allocator and stay the seL4 gates' job.
    #[test]
    fn an_arena_slot_table_shrinks_only_from_its_top() {
        const PAGE: usize = 4096;
        let mut arena = ArenaRecord::new(1, sel4::cap::Untyped::from_bits(0), 20);
        assert_eq!(arena.push_slot(40), Ok(()));
        assert_eq!(arena.push_slot(41), Ok(()));
        arena.watermark = 2 * PAGE;
        arena.objects = 2;
        arena.bytes = 2 * PAGE;

        // Not the top entry: refused, with nothing touched. This is what stops
        // an out-of-order unwind from rewinding a watermark over an object that
        // is still live.
        assert_eq!(
            arena.release_last(40, PAGE),
            Err(AllocError::ArenaSlotTableFull {
                limit: MAX_TASK_SLOTS
            })
        );
        assert_eq!(arena.slot_len, 2);
        assert_eq!(arena.watermark, 2 * PAGE);
        assert_eq!(arena.objects, 2);
        assert_eq!(arena.bytes, 2 * PAGE);

        // The top entry: accepted, and every tally rewinds by exactly one page.
        assert_eq!(arena.release_last(41, PAGE), Ok(()));
        assert_eq!(arena.slot_len, 1);
        assert_eq!(arena.slots[1], ArenaRecord::EMPTY_SLOT);
        assert_eq!(arena.watermark, PAGE);
        assert_eq!(arena.objects, 1);
        assert_eq!(arena.bytes, PAGE);

        // And the freed position is genuinely reusable, so a retried growth does
        // not walk the table forward past its own returned slots — which is the
        // leak this inverse exists to prevent.
        assert_eq!(arena.push_slot(42), Ok(()));
        assert_eq!(arena.slot_len, 2);
        assert_eq!(arena.slots[1], 42);

        // An empty table has no top entry to name.
        assert_eq!(arena.release_last(42, PAGE), Ok(()));
        assert_eq!(arena.release_last(40, PAGE), Ok(()));
        assert_eq!(arena.slot_len, 0);
        assert_eq!(
            arena.release_last(40, PAGE),
            Err(AllocError::ArenaSlotTableFull {
                limit: MAX_TASK_SLOTS
            })
        );
    }

    /// Without an inverse for `push_slot`, a caller that allocates from a live
    /// arena and fails part way exhausts the table by retrying. This pins that
    /// the ceiling is reached at exactly `MAX_TASK_SLOTS` pushes, which is the
    /// bound the growth unwind must not walk into.
    #[test]
    fn an_arena_slot_table_refuses_past_its_declared_bound() {
        let mut arena = ArenaRecord::new(1, sel4::cap::Untyped::from_bits(0), 20);
        for slot in 0..super::MAX_TASK_SLOTS {
            assert_eq!(arena.push_slot(slot), Ok(()), "slot {slot}");
        }
        assert_eq!(
            arena.push_slot(super::MAX_TASK_SLOTS),
            Err(AllocError::ArenaSlotTableFull {
                limit: super::MAX_TASK_SLOTS
            })
        );
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
            assert_eq!(slots.live, 0);
        }
    }

    #[test]
    fn arena_plan_accounts_for_alignment_and_rounds_to_power_of_two() {
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
}
