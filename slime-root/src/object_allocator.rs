//! Deterministic BootInfo CSlot and untyped allocation for `slime-root`.
//!
//! Every seL4 object the root task creates is retyped out of a kernel untyped
//! named by BootInfo, into a CSlot taken from the initial thread's empty slot
//! region. Both resources are accounted explicitly: the allocator models the
//! kernel's alignment rule (`seL4_Untyped_Retype` places an object at the next
//! multiple of its own physical size) so an exhausted region is detected before
//! the invocation rather than discovered as an error code, and every failure is
//! a typed value instead of a panic.
//!
//! Slots are handed out in increasing order and never reused, so the slots an
//! individual task consumes always form one contiguous range. `slime-root`
//! records that range as the task's cleanup record.

use core::ops::Range;

/// Kernel untyped regions this allocator will track. seL4's own ceiling is
/// `CONFIG_MAX_NUM_BOOTINFO_UNTYPED_CAPS` (230 upstream, 50 on the verified
/// AArch64 configuration); a BootInfo with more kernel untypeds than this fails
/// closed instead of silently ignoring memory.
pub const MAX_KERNEL_UNTYPEDS: usize = 64;

/// Device untyped regions this allocator will track (P5.4.2a).
///
/// Separate from [`MAX_KERNEL_UNTYPEDS`] because these are never allocated
/// *from*: a device untyped names a fixed physical range, so the only thing
/// asked of it is "retype the page containing this address". They are held in
/// their own table so no ordinary allocation can ever land in MMIO, which is
/// what the `is_device()` skip below achieved by discarding them entirely.
///
/// qemu-arm-virt declares well under this; a BootInfo with more fails closed.
pub const MAX_DEVICE_UNTYPEDS: usize = 64;

/// Bytes in one AArch64 granule, the size a device frame is retyped at.
const GRANULE_BYTES: usize = 4096;

/// Granules [`ObjectAllocator::allocate_device_frame`] will retype to reach a
/// target page.
///
/// seL4 has no "retype at this offset" invocation: the page at index `n` inside
/// a device untyped is reached by retyping `n + 1` of them and keeping the last,
/// so a target deep inside a large region costs that many CSlots. qemu-arm-virt
/// puts the virtio-mmio transports at `0x0a00_0000 + n * 0x200`, all inside one
/// granule, so a correct target is at a small offset from its region's base.
/// A large one means the address is wrong, and consuming hundreds of CSlots to
/// discover that is worse than refusing.
const MAX_DEVICE_FRAME_SKIP: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AllocError {
    /// BootInfo declared no non-device untyped memory.
    NoKernelUntyped,
    /// More kernel untypeds than [`MAX_KERNEL_UNTYPEDS`].
    UntypedTableFull { limit: usize, declared: usize },
    /// The initial thread's empty CSlot region is exhausted.
    SlotsExhausted { allocated: usize },
    /// No tracked untyped region can still hold an object of this size.
    UntypedExhausted { size_bits: usize, remaining: usize },
    /// The retype invocation itself failed.
    Retype {
        size_bits: usize,
        error: sel4::Error,
    },
    /// More device untypeds than [`MAX_DEVICE_UNTYPEDS`].
    DeviceTableFull { limit: usize, declared: usize },
    /// No device untyped covers the requested physical address.
    NoDeviceUntyped { paddr: usize },
    /// The requested address is not granule-aligned.
    UnalignedDeviceFrame { paddr: usize },
    /// The device untyped's watermark is already past this page. seL4's retype
    /// only advances, so the page can no longer be reached this boot.
    DeviceFramePassed { paddr: usize },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct UntypedRegion {
    cap: sel4::cap::Untyped,
    /// Physical base, so an allocation can report the guest-physical address of
    /// what it retyped. Discarded before P5.4.2a, which is why the root could
    /// map a frame but never name it to a device.
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

/// One device untyped, named by the physical range it covers.
///
/// No watermark: this is never allocated from in order, only asked for the page
/// containing a specific address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DeviceRegion {
    cap: sel4::cap::Untyped,
    paddr: usize,
    size_bits: usize,
    /// Granules already retyped out of this region, mirroring the kernel's own
    /// monotonic watermark. Without it, a second `allocate_device_frame` on one
    /// region computes its count from the base and lands past its target.
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

/// Where an object of `size_bits` would land in a region, and the watermark
/// that follows it, or `None` when the region cannot hold it.
///
/// The kernel aligns each retyped object to its own physical size, so a region
/// whose free bytes are merely large enough is not necessarily usable.
fn plan_allocation(watermark: usize, capacity: usize, size_bits: usize) -> Option<(usize, usize)> {
    let size = 1usize.checked_shl(u32::try_from(size_bits).ok()?)?;
    let start = watermark.checked_next_multiple_of(size)?;
    let end = start.checked_add(size)?;
    (end <= capacity).then_some((start, end))
}

pub struct ObjectAllocator {
    empty: Range<usize>,
    untypeds: [Option<UntypedRegion>; MAX_KERNEL_UNTYPEDS],
    untyped_len: usize,
    devices: [Option<DeviceRegion>; MAX_DEVICE_UNTYPEDS],
    device_len: usize,
    slots_allocated: usize,
    objects_allocated: usize,
    bytes_allocated: usize,
    /// Physical base of the most recent [`ObjectAllocator::allocate`], for the
    /// DMA case: a virtqueue descriptor must carry the guest-physical address of
    /// a frame the root just retyped.
    last_paddr: usize,
}

impl ObjectAllocator {
    /// Record every untyped region BootInfo names and the empty CSlot range.
    ///
    /// Two tables, because the two kinds are used differently: ordinary
    /// untypeds are allocated *from* in watermark order, while a device untyped
    /// names a fixed physical range and is only ever asked for the page
    /// containing a given address. Keeping them apart is what makes it
    /// impossible for an ordinary allocation to land in MMIO — the property the
    /// old `is_device()` skip achieved by discarding device untypeds outright,
    /// which also made the root unable to reach a device at all (P5.4.2a).
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
            // Belt and braces: `kernel_untyped_range` already excludes these,
            // and a device untyped reaching the general pool would be silent.
            if descriptor.is_device() {
                continue;
            }
            let Some(slot) = untypeds.get_mut(untyped_len) else {
                return Err(AllocError::UntypedTableFull {
                    limit: MAX_KERNEL_UNTYPEDS,
                    declared,
                });
            };
            *slot = Some(UntypedRegion {
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
            let Some(slot) = devices.get_mut(device_len) else {
                return Err(AllocError::DeviceTableFull {
                    limit: MAX_DEVICE_UNTYPEDS,
                    declared: device_declared,
                });
            };
            *slot = Some(DeviceRegion {
                cap: bootinfo.untyped().index(index).cap(),
                paddr: descriptor.paddr(),
                size_bits: descriptor.size_bits(),
                retyped: 0,
            });
            device_len += 1;
        }
        // No `NoDeviceUntyped` at construction: a graph that never touches a
        // device must still boot on a machine that declares none.
        Ok(Self {
            empty: bootinfo.empty().range(),
            untypeds,
            untyped_len,
            devices,
            device_len,
            slots_allocated: 0,
            objects_allocated: 0,
            bytes_allocated: 0,
            last_paddr: 0,
        })
    }

    /// The next CSlot index this allocator will hand out. Callers bracket a
    /// task's allocations with this value to derive its cleanup record.
    pub fn next_slot_index(&self) -> usize {
        self.empty.start
    }

    pub fn slots_remaining(&self) -> usize {
        self.empty.len()
    }

    pub fn slots_allocated(&self) -> usize {
        self.slots_allocated
    }

    pub fn objects_allocated(&self) -> usize {
        self.objects_allocated
    }

    pub fn bytes_allocated(&self) -> usize {
        self.bytes_allocated
    }

    pub fn untyped_count(&self) -> usize {
        self.untyped_len
    }

    /// Untyped bytes still reachable, ignoring per-object alignment loss.
    pub fn untyped_bytes_remaining(&self) -> usize {
        self.regions()
            .iter()
            .flatten()
            .map(UntypedRegion::remaining)
            .sum()
    }

    /// Retype one object into a fresh CSlot.
    pub fn allocate(
        &mut self,
        blueprint: sel4::ObjectBlueprint,
    ) -> Result<sel4::init_thread::Slot<sel4::cap_type::Unspecified>, AllocError> {
        let size_bits = blueprint.physical_size_bits();
        let slot_index = self.empty.start;
        if slot_index >= self.empty.end {
            return Err(AllocError::SlotsExhausted {
                allocated: self.slots_allocated,
            });
        }
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
        let Some(Some(region)) = self.untypeds.get(region_index) else {
            return Err(AllocError::UntypedExhausted {
                size_bits,
                remaining: 0,
            });
        };
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
        if let Some(Some(region)) = self.untypeds.get_mut(region_index) {
            region.watermark = watermark;
            // `plan_allocation` already models the kernel's alignment rule, so
            // this is where the object actually lands. Recorded rather than
            // discarded because a DMA buffer must be nameable to a device by
            // guest-physical address, and nothing else in the root can derive
            // it (P5.4.2a).
            self.last_paddr = region.paddr.saturating_add(start);
        }
        self.empty.start = slot_index + 1;
        self.slots_allocated += 1;
        self.objects_allocated += 1;
        self.bytes_allocated += 1usize << size_bits;
        Ok(sel4::init_thread::Slot::from_index(slot_index))
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

    /// Physical base of the most recent [`Self::allocate`].
    ///
    /// Only meaningful immediately after one succeeds. A DMA buffer's whole
    /// purpose is to be named to a device by guest-physical address, and seL4
    /// exposes no way to ask a frame cap where it lives — the allocator is the
    /// only place that knows.
    pub fn last_physical_address(&self) -> usize {
        self.last_paddr
    }

    pub fn device_untyped_count(&self) -> usize {
        self.device_len
    }

    /// Retype the device granule containing `paddr` into a fresh CSlot.
    ///
    /// Not an allocation in the watermark sense the ordinary path means, but it
    /// does keep a watermark, and that is the subtle part.
    ///
    /// **`seL4_Untyped_Retype` has no offset argument.** Each call places its
    /// objects at the untyped's own internal watermark, which only ever
    /// advances. So the granule at index `n` is reached by retyping enough
    /// pages to arrive there and keeping the last — the same trimming the
    /// upstream `serial-device` example performs — and a *second* call on the
    /// same region starts where the first stopped rather than at the base.
    /// Computing the count from the region base every time therefore works
    /// exactly once; the second call silently lands past its target, which is a
    /// mapping that faults rather than an error.
    ///
    /// The per-region `retyped` count is what makes repeated calls correct: it
    /// tracks where the kernel's watermark actually is, so the count asked for
    /// is the distance from there. Going backwards is impossible and is
    /// refused.
    ///
    /// The skipped caps are left in their slots rather than deleted: they name
    /// real MMIO pages, and deleting them would return the space to an untyped
    /// this root has no second use for.
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
        // The kernel's watermark only advances, so a page already passed is
        // unreachable. Refused rather than retyped past.
        let count = target
            .checked_sub(region.retyped)
            .and_then(|distance| distance.checked_add(1))
            .ok_or(AllocError::DeviceFramePassed { paddr })?;
        if count > MAX_DEVICE_FRAME_SKIP {
            return Err(AllocError::NoDeviceUntyped { paddr });
        }
        let slot_index = self.empty.start;
        if slot_index + count > self.empty.end {
            return Err(AllocError::SlotsExhausted {
                allocated: self.slots_allocated,
            });
        }
        region
            .cap
            .untyped_retype(
                &sel4::ObjectBlueprint::Arch(sel4::ObjectBlueprintArch::SmallPage),
                &sel4::init_thread::slot::CNODE
                    .cap()
                    .absolute_cptr_for_self(),
                slot_index,
                count,
            )
            .map_err(|error| AllocError::Retype {
                size_bits: 12,
                error,
            })?;
        if let Some(Some(region)) = self.devices.get_mut(region_index) {
            region.retyped = target + 1;
        }
        self.empty.start = slot_index + count;
        self.slots_allocated += count;
        self.objects_allocated += count;
        self.last_paddr = paddr;
        Ok(sel4::init_thread::Slot::from_index(slot_index + count - 1))
    }

    /// Reserve one empty root CSlot without retyping any untyped memory.
    ///
    /// For invocations that hand the kernel a destination CSlot directly
    /// (`seL4_IRQControl_Get` and its variants), rather than retyping an
    /// object into one. Shares the same slot cursor and exhaustion error as
    /// [`Self::allocate`], so a task's recorded cleanup range still covers
    /// slots this call hands out.
    pub fn reserve_slot<T: sel4::CapType>(
        &mut self,
    ) -> Result<sel4::init_thread::Slot<T>, AllocError> {
        let slot_index = self.empty.start;
        if slot_index >= self.empty.end {
            return Err(AllocError::SlotsExhausted {
                allocated: self.slots_allocated,
            });
        }
        self.empty.start = slot_index + 1;
        self.slots_allocated += 1;
        Ok(sel4::init_thread::Slot::from_index(slot_index))
    }

    fn regions(&self) -> &[Option<UntypedRegion>] {
        self.untypeds.get(..self.untyped_len).unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::plan_allocation;

    #[test]
    fn allocation_is_aligned_to_object_size() {
        // A 4 KiB object leaves the watermark unaligned for a 16 KiB object,
        // which the kernel would place at the next 16 KiB boundary.
        assert_eq!(plan_allocation(0, 1 << 20, 12), Some((0, 4096)));
        assert_eq!(plan_allocation(4096, 1 << 20, 14), Some((16384, 32768)));
    }

    #[test]
    fn alignment_loss_can_exhaust_a_region() {
        // 16 KiB region with 4 KiB already taken cannot hold a 16 KiB object,
        // even though 12 KiB remain unused.
        assert_eq!(plan_allocation(4096, 1 << 14, 14), None);
        assert_eq!(plan_allocation(4096, 1 << 14, 12), Some((4096, 8192)));
    }

    #[test]
    fn exact_fit_is_allowed_and_full_region_is_not() {
        assert_eq!(plan_allocation(0, 1 << 12, 12), Some((0, 4096)));
        assert_eq!(plan_allocation(4096, 1 << 12, 12), None);
    }
}
