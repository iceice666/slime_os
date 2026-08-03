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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct UntypedRegion {
    cap: sel4::cap::Untyped,
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
    slots_allocated: usize,
    objects_allocated: usize,
    bytes_allocated: usize,
}

impl ObjectAllocator {
    /// Record every non-device untyped region and the empty CSlot range.
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
            let Some(slot) = untypeds.get_mut(untyped_len) else {
                return Err(AllocError::UntypedTableFull {
                    limit: MAX_KERNEL_UNTYPEDS,
                    declared,
                });
            };
            *slot = Some(UntypedRegion {
                cap: bootinfo.untyped().index(index).cap(),
                size_bits: descriptor.size_bits(),
                watermark: 0,
            });
            untyped_len += 1;
        }
        if untyped_len == 0 {
            return Err(AllocError::NoKernelUntyped);
        }
        Ok(Self {
            empty: bootinfo.empty().range(),
            untypeds,
            untyped_len,
            slots_allocated: 0,
            objects_allocated: 0,
            bytes_allocated: 0,
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
        let (region_index, watermark) = self
            .untypeds
            .iter()
            .take(self.untyped_len)
            .enumerate()
            .find_map(|(index, region)| {
                let region = region.as_ref()?;
                plan_allocation(region.watermark, region.capacity(), size_bits)
                    .map(|(_, end)| (index, end))
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
