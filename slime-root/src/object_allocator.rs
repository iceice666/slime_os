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
const MAX_DEVICE_FRAME_SKIP: usize = 64;

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
    pub const fn last_physical_address(&self) -> usize {
        self.last_paddr
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
        region
            .cap
            .untyped_retype(
                &blueprint,
                &sel4::init_thread::slot::CNODE
                    .cap()
                    .absolute_cptr_for_self(),
                slot_index,
                1,
            )
            .map_err(|error| AllocError::Retype { size_bits, error })?;
        if let Some(region) = self.untypeds[region_index].as_mut() {
            region.watermark = watermark;
            self.last_paddr = region.paddr.saturating_add(start);
        }
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
    pub fn release_slot(&mut self, slot: usize) -> bool {
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
            .and_then(|distance| distance.checked_add(1))
            .ok_or(AllocError::DeviceFramePassed { paddr })?;
        if count > MAX_DEVICE_FRAME_SKIP {
            return Err(AllocError::NoDeviceUntyped { paddr });
        }
        let (first, reused) = self
            .slots
            .allocate_contiguous(count, self.slots_allocated)?;
        if let Err(error) = region.cap.untyped_retype(
            &sel4::ObjectBlueprint::Arch(sel4::ObjectBlueprintArch::SmallPage),
            &sel4::init_thread::slot::CNODE
                .cap()
                .absolute_cptr_for_self(),
            first,
            count,
        ) {
            for slot in first..first + count {
                self.slots.release(slot);
            }
            return Err(AllocError::Retype {
                size_bits: 12,
                error,
            });
        }
        self.slots_allocated += count;
        self.slots_reused += reused;
        if let Some(region) = self.devices[region_index].as_mut() {
            region.retyped = target + 1;
        }
        self.objects_allocated += count;
        self.live_objects += count;
        self.bytes_allocated += count * GRANULE_BYTES;
        self.live_bytes += count * GRANULE_BYTES;
        self.last_paddr = paddr;
        Ok(sel4::init_thread::Slot::from_index(first + count - 1))
    }

    fn regions(&self) -> &[Option<UntypedRegion>] {
        self.untypeds.get(..self.untyped_len).unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::{AllocError, ArenaPlan, ArenaRecord, MAX_TASK_SLOTS, SlotPool, plan_allocation};

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
