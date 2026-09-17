//! Aligned ordinary-untyped prefixes must remain owned before a later retype.

use super::{AllocError, ObjectAllocator, UntypedRegion};

pub(super) const MAX_PRESERVED: usize = if super::LARGE_DESCRIPTOR_TABLES {
    4096
} else {
    256
};
pub(super) const MINIMUM_BITS: usize = sel4::sys::seL4_MinUntypedBits as usize;

impl ObjectAllocator {
    /// Preserved prefixes remain independently owned even if a subsequent
    /// allocation fails. Count only the unconsumed tails, never parent bytes.
    pub fn preserved_bytes_remaining(&self) -> usize {
        self.preserved[..self.preserved_len]
            .iter()
            .flatten()
            .map(UntypedRegion::remaining)
            .sum()
    }

    pub fn preserved_anchor_count(&self) -> usize {
        self.preserved_len
    }

    /// Snapshot disjoint backing owners without treating parent capabilities
    /// as additional physical memory alongside their children.
    pub fn report_backing_snapshot(&self, phase: &str) {
        sel4::debug_println!("SLIME_BACKING snapshot phase={phase} begin");
        for region in self.untypeds[..self.untyped_len].iter().flatten() {
            sel4::debug_println!(
                "SLIME_BACKING ordinary parent={} paddr={} bytes={} used={}",
                region.cap.bits(),
                region.paddr,
                region.capacity(),
                region.watermark,
            );
        }
        for region in self.preserved[..self.preserved_len].iter().flatten() {
            sel4::debug_println!(
                "SLIME_BACKING retained parent={} paddr={} bytes={} used={}",
                region.cap.bits(),
                region.paddr,
                region.capacity(),
                region.watermark,
            );
        }
        for extent in self.extents.iter().flatten() {
            sel4::debug_println!(
                "SLIME_BACKING task_extent parent={} bytes={} active={}",
                extent.parent.bits(),
                1usize << extent.size_bits,
                usize::from(extent.active),
            );
        }
        self.shared_backing.report_backing_snapshot();
        sel4::debug_println!("SLIME_BACKING snapshot phase={phase} end");
    }

    /// Consume a whole preserved leaf. Keeping its parent capability alive and
    /// never allocating its consumed tail avoids kernel cursor reset after the
    /// object's last capability is deleted.
    pub(super) fn allocate_preserved(
        &mut self,
        blueprint: sel4::ObjectBlueprint,
        destination: usize,
    ) -> Result<bool, AllocError> {
        let bits = blueprint.physical_size_bits();
        if bits < MINIMUM_BITS {
            return Ok(false);
        }
        let Some(mut index) = self.preserved[..self.preserved_len]
            .iter()
            .enumerate()
            .filter_map(|(index, region)| {
                region
                    .filter(|region| region.watermark == 0 && region.size_bits >= bits)
                    .map(|region| (index, region.size_bits))
            })
            .min_by_key(|(_, bits)| *bits)
            .map(|(index, _)| index)
        else {
            return Ok(false);
        };
        let original = self.preserved[index].unwrap();
        let splits = original.size_bits - bits;
        if 2 * splits > MAX_PRESERVED - self.preserved_len {
            return Err(AllocError::UntypedTableFull {
                limit: MAX_PRESERVED,
                declared: self.preserved_len + 2 * splits,
            });
        }
        if 2 * splits > self.free_slots() {
            return Err(AllocError::SlotsExhausted {
                allocated: self.slots_allocated,
            });
        }
        // A free-slot count does not imply the pairs required by one atomic
        // two-child retype exist. Reserve the complete split shape in a copy
        // before publishing any child capability.
        let mut planned_slots = self.slots.clone();
        for _ in 0..splits {
            planned_slots.allocate_contiguous(2, self.slots_allocated)?;
        }
        while self.preserved[index].unwrap().size_bits > bits {
            let parent = self.preserved[index].unwrap();
            let (first, reused) = self.slots.allocate_contiguous(2, self.slots_allocated)?;
            let child_bits = parent.size_bits - 1;
            // One invocation validates both destinations before creating either
            // child, so failure cannot strand an unrepresented sibling tail.
            if let Err(error) = parent.cap.untyped_retype(
                &sel4::ObjectBlueprint::Untyped {
                    size_bits: child_bits,
                },
                &sel4::init_thread::slot::CNODE
                    .cap()
                    .absolute_cptr_for_self(),
                first,
                2,
            ) {
                self.slots.release(first);
                self.slots.release(first + 1);
                return Err(AllocError::Retype {
                    size_bits: child_bits,
                    error,
                });
            }
            self.slots_allocated += 2;
            self.slots_reused += reused;
            for (side, slot) in [first, first + 1].into_iter().enumerate() {
                self.preserved[self.preserved_len] = Some(UntypedRegion {
                    cap: sel4::cap::Untyped::from_bits(slot as _),
                    paddr: parent.paddr + side * (1usize << child_bits),
                    size_bits: child_bits,
                    watermark: 0,
                });
                sel4::debug_println!(
                    "SLIME_BACKING split parent={} child={} paddr={} bytes={}",
                    parent.cap.bits(),
                    slot,
                    parent.paddr + side * (1usize << child_bits),
                    1usize << child_bits,
                );
                self.preserved_len += 1;
                self.preserved[index].as_mut().unwrap().watermark += 1usize << child_bits;
                self.objects_allocated += 1;
                self.live_objects += 1;
            }
            index = self.preserved_len - 2;
        }
        self.allocate_preserved_leaf(blueprint, destination, index)?;
        Ok(true)
    }

    pub(super) fn allocate_preserved_leaf(
        &mut self,
        blueprint: sel4::ObjectBlueprint,
        destination: usize,
        index: usize,
    ) -> Result<(), AllocError> {
        let bits = blueprint.physical_size_bits();
        let leaf = self.preserved[index].ok_or(AllocError::NoKernelUntyped)?;
        if leaf.watermark != 0 || leaf.size_bits != bits {
            return Err(AllocError::NoKernelUntyped);
        }
        let recorded = super::records_provenance(blueprint);
        if recorded {
            self.physical.insert(destination, leaf.paddr)?;
        }
        if let Err(error) = leaf.cap.untyped_retype(
            &blueprint,
            &sel4::init_thread::slot::CNODE
                .cap()
                .absolute_cptr_for_self(),
            destination,
            1,
        ) {
            if recorded {
                self.physical.remove(destination);
            }
            return Err(AllocError::Retype {
                size_bits: bits,
                error,
            });
        }
        self.preserved[index].as_mut().unwrap().watermark = 1usize << bits;
        self.last_paddr = leaf.paddr;
        self.objects_allocated += 1;
        self.live_objects += 1;
        self.bytes_allocated += 1usize << bits;
        self.live_bytes += 1usize << bits;
        sel4::debug_println!(
            "SLIME_BACKING consume source=preserved parent={} slot={} paddr={} bytes={} count=1",
            leaf.cap.bits(),
            destination,
            leaf.paddr,
            1usize << bits,
        );
        sel4::debug_println!(
            "SLIME_ALLOC preserved parent={} slot={} paddr={} bytes={}",
            leaf.cap.bits(),
            destination,
            leaf.paddr,
            1usize << bits
        );
        Ok(())
    }

    pub(super) fn preserve_global_prefix(
        &mut self,
        index: usize,
        end: usize,
    ) -> Result<(), AllocError> {
        self.preserve_global_prefix_with(index, end, |parent, bits, slot| {
            parent
                .untyped_retype(
                    &sel4::ObjectBlueprint::Untyped { size_bits: bits },
                    &sel4::init_thread::slot::CNODE
                        .cap()
                        .absolute_cptr_for_self(),
                    slot,
                    1,
                )
                .map_err(|error| AllocError::Retype {
                    size_bits: bits,
                    error,
                })
        })
    }

    fn preserve_global_prefix_with(
        &mut self,
        index: usize,
        end: usize,
        mut retype: impl FnMut(sel4::cap::Untyped, usize, usize) -> Result<(), AllocError>,
    ) -> Result<(), AllocError> {
        let region = self.untypeds[index].ok_or(AllocError::NoKernelUntyped)?;
        let count = prefix_blocks(region.watermark, end, MINIMUM_BITS)
            .filter(|_| end <= region.capacity())
            .ok_or(AllocError::NoKernelUntyped)?;
        if count > MAX_PRESERVED - self.preserved_len {
            return Err(AllocError::UntypedTableFull {
                limit: MAX_PRESERVED,
                declared: self.preserved_len + count,
            });
        }
        if count > self.free_slots() {
            return Err(AllocError::SlotsExhausted {
                allocated: self.slots_allocated,
            });
        }
        let mut cursor = region.watermark;
        while cursor < end {
            let bits =
                prefix_block(cursor, end, MINIMUM_BITS).ok_or(AllocError::NoKernelUntyped)?;
            let slot = self.take_slot()?;
            if let Err(error) = retype(region.cap, bits, slot) {
                self.slots.release(slot);
                return Err(error);
            }
            self.preserved[self.preserved_len] = Some(UntypedRegion {
                cap: sel4::cap::Untyped::from_bits(slot as _),
                paddr: region.paddr + cursor,
                size_bits: bits,
                watermark: 0,
            });
            #[cfg(not(test))]
            sel4::debug_println!(
                "SLIME_BACKING preserve parent={} child={} paddr={} bytes={}",
                region.cap.bits(),
                slot,
                region.paddr + cursor,
                1usize << bits,
            );
            self.preserved_len += 1;
            self.objects_allocated += 1;
            self.live_objects += 1;
            cursor += 1usize << bits;
            self.untypeds[index].as_mut().unwrap().watermark = cursor;
        }
        Ok(())
    }
}

/// Simulate the same preserved-leaf selection and ordinary-prefix retention
/// used by live extent provisioning, including all additional anchor slots.
pub(super) fn plan_extent(
    ordinary: &mut [Option<UntypedRegion>],
    preserved: &mut [Option<UntypedRegion>],
    preserved_len: &mut usize,
    slots: &mut usize,
    bits: usize,
) -> bool {
    plan_extent_with_slots(ordinary, preserved, preserved_len, slots, bits, None)
}

pub(super) fn plan_extent_with_slots(
    ordinary: &mut [Option<UntypedRegion>],
    preserved: &mut [Option<UntypedRegion>],
    preserved_len: &mut usize,
    slots: &mut usize,
    bits: usize,
    mut slot_pool: Option<&mut super::SlotPool>,
) -> bool {
    let Some(bytes) = u32::try_from(bits)
        .ok()
        .and_then(|bits| 1usize.checked_shl(bits))
    else {
        return false;
    };
    if *slots == 0 || bits < MINIMUM_BITS {
        return false;
    }
    if let Some((mut index, size_bits)) = preserved[..*preserved_len]
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            entry
                .filter(|entry| entry.watermark == 0 && entry.size_bits >= bits)
                .map(|entry| (index, entry.size_bits))
        })
        .min_by_key(|(_, size)| *size)
    {
        let extra = 2 * (size_bits - bits);
        if extra > preserved.len() - *preserved_len || extra + 1 > *slots {
            return false;
        }
        if let Some(pool) = slot_pool.as_mut() {
            if pool.allocate(0).is_err() {
                return false;
            }
            for _ in 0..size_bits - bits {
                if pool.allocate_contiguous(2, 0).is_err() {
                    return false;
                }
            }
        }
        while preserved[index].unwrap().size_bits > bits {
            let parent = preserved[index].unwrap();
            let child_bits = parent.size_bits - 1;
            let first = *preserved_len;
            for side in 0..2 {
                preserved[*preserved_len] = Some(UntypedRegion {
                    cap: parent.cap,
                    paddr: parent.paddr + side * (1usize << child_bits),
                    size_bits: child_bits,
                    watermark: 0,
                });
                *preserved_len += 1;
            }
            preserved[index].as_mut().unwrap().watermark = parent.capacity();
            index = first;
        }
        preserved[index].as_mut().unwrap().watermark = bytes;
        *slots -= extra + 1;
        return true;
    }
    let Some((index, start, end)) = ordinary.iter().enumerate().find_map(|(index, entry)| {
        let entry = entry.as_ref()?;
        super::plan_allocation(entry.watermark, entry.capacity(), bits)
            .map(|(start, end)| (index, start, end))
    }) else {
        return false;
    };
    let parent = ordinary[index].unwrap();
    let Some(extra) = prefix_blocks(parent.watermark, start, MINIMUM_BITS) else {
        return false;
    };
    if extra > preserved.len() - *preserved_len || extra + 1 > *slots {
        return false;
    }
    if let Some(pool) = slot_pool.as_mut() {
        for _ in 0..extra + 1 {
            if pool.allocate(0).is_err() {
                return false;
            }
        }
    }
    let mut cursor = parent.watermark;
    while cursor < start {
        let gap_bits = prefix_block(cursor, start, MINIMUM_BITS).unwrap();
        preserved[*preserved_len] = Some(UntypedRegion {
            cap: parent.cap,
            paddr: parent.paddr + cursor,
            size_bits: gap_bits,
            watermark: 0,
        });
        *preserved_len += 1;
        cursor += 1usize << gap_bits;
    }
    ordinary[index].as_mut().unwrap().watermark = end;
    *slots -= extra + 1;
    true
}

/// Return the largest aligned power-of-two block at `start` inside the prefix.
/// Invalid or sub-minimum ranges are refused rather than silently discarded.
fn prefix_block(start: usize, end: usize, minimum_bits: usize) -> Option<usize> {
    let minimum = 1usize.checked_shl(minimum_bits.try_into().ok()?)?;
    if start >= end || !start.is_multiple_of(minimum) || !end.is_multiple_of(minimum) {
        return None;
    }
    let remaining = end.checked_sub(start)?;
    let fitting = usize::BITS as usize - 1 - remaining.leading_zeros() as usize;
    let alignment = if start == 0 {
        usize::BITS as usize - 1
    } else {
        start.trailing_zeros() as usize
    };
    let bits = fitting.min(alignment);
    (bits >= minimum_bits).then_some(bits)
}

/// Validate the complete decomposition before reserving capabilities or
/// changing kernel state. The count bounds all destination slots needed.
fn prefix_blocks(start: usize, end: usize, minimum_bits: usize) -> Option<usize> {
    let minimum = 1usize.checked_shl(minimum_bits.try_into().ok()?)?;
    if start > end || !start.is_multiple_of(minimum) || !end.is_multiple_of(minimum) {
        return None;
    }
    let mut cursor = start;
    let mut count = 0usize;
    while cursor < end {
        let bits = prefix_block(cursor, end, minimum_bits)?;
        cursor = cursor.checked_add(1usize.checked_shl(bits.try_into().ok()?)?)?;
        count = count.checked_add(1)?;
    }
    Some(count)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn fragmented_slots_refuse_split_before_changing_backing() {
        let mut pool = super::super::SlotPool::new(100..108).unwrap();
        for _ in 0..8 {
            pool.allocate(0).unwrap();
        }
        for slot in [100, 102, 104, 106] {
            pool.release(slot);
        }
        let mut preserved = [None; 8];
        preserved[0] = Some(UntypedRegion {
            cap: sel4::cap::Untyped::from_bits(1),
            paddr: 0x800000,
            size_bits: 13,
            watermark: 0,
        });
        let before = preserved;
        let mut len = 1;
        let mut available = pool.free();
        assert!(!plan_extent_with_slots(
            &mut [],
            &mut preserved,
            &mut len,
            &mut available,
            12,
            Some(&mut pool)
        ));
        assert_eq!(preserved, before);
        assert_eq!(len, 1);
        assert_eq!(available, 4);
    }

    #[test]
    fn planner_reuses_prefixes_and_refuses_anchor_shortfalls() {
        let region = Some(UntypedRegion {
            cap: sel4::cap::Untyped::from_bits(1),
            paddr: 0x800000,
            size_bits: 13,
            watermark: 16,
        });
        let mut ordinary = [region];
        let mut preserved = [None; 32];
        let mut len = 0;
        let mut slots = 32;
        assert!(plan_extent(
            &mut ordinary,
            &mut preserved,
            &mut len,
            &mut slots,
            12
        ));
        assert_eq!(ordinary[0].unwrap().watermark, 8192);
        assert_eq!(len, 8);
        assert_eq!(slots, 23);
        assert!(plan_extent(
            &mut ordinary,
            &mut preserved,
            &mut len,
            &mut slots,
            4
        ));
        assert_eq!(ordinary[0].unwrap().watermark, 8192);
        assert_eq!(slots, 22);
        assert_eq!(preserved[0].unwrap().watermark, 16);
        let mut ordinary = [region];
        let mut insufficient = [None; 7];
        let mut len = 0;
        let mut slots = 32;
        assert!(!plan_extent(
            &mut ordinary,
            &mut insufficient,
            &mut len,
            &mut slots,
            12
        ));
        assert_eq!(ordinary[0], region);
        assert_eq!(len, 0);
        let mut preserved = [None; 32];
        slots = 8;
        assert!(!plan_extent(
            &mut ordinary,
            &mut preserved,
            &mut len,
            &mut slots,
            12
        ));
        assert_eq!(ordinary[0], region);
        assert_eq!(slots, 8);
    }

    #[test]
    fn partial_prefix_preservation_retains_success_and_retries_only_the_tail() {
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(|| {
                let mut allocator = ObjectAllocator::empty();
                allocator.slots.initialize(100..150).unwrap();
                allocator.untypeds[0] = Some(UntypedRegion {
                    cap: sel4::cap::Untyped::from_bits(1),
                    paddr: 0x800000,
                    size_bits: 12,
                    watermark: 16,
                });
                allocator.untyped_len = 1;
                let before = allocator.untyped_bytes_remaining();
                let mut calls = 0;
                assert!(
                    allocator
                        .preserve_global_prefix_with(0, 4096, |_, _, _| {
                            calls += 1;
                            if calls == 3 {
                                Err(AllocError::NoKernelUntyped)
                            } else {
                                Ok(())
                            }
                        })
                        .is_err()
                );
                assert_eq!(allocator.untypeds[0].unwrap().watermark, 64);
                assert_eq!(allocator.preserved_len, 2);
                assert_eq!(allocator.preserved_bytes_remaining(), 48);
                assert_eq!(
                    allocator.untyped_bytes_remaining() + allocator.preserved_bytes_remaining(),
                    before
                );
                let mut bits_seen = std::vec::Vec::new();
                allocator
                    .preserve_global_prefix_with(0, 4096, |_, bits, _| {
                        bits_seen.push(bits);
                        Ok(())
                    })
                    .unwrap();
                assert_eq!(bits_seen, [6, 7, 8, 9, 10, 11]);
                assert_eq!(allocator.preserved_len, 8);
                assert_eq!(allocator.untyped_bytes_remaining(), 0);
                assert_eq!(allocator.preserved_bytes_remaining(), before);
                assert_eq!(allocator.free_slots(), 42);
                allocator
                    .preserve_global_prefix_with(0, 4096, |_, _, _| {
                        panic!("replayed preserved range")
                    })
                    .unwrap();
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn preservation_refuses_insufficient_slots_or_descriptors_before_retyping() {
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(|| {
                let mut allocator = ObjectAllocator::empty();
                allocator.slots.initialize(100..101).unwrap();
                allocator.untypeds[0] = Some(UntypedRegion {
                    cap: sel4::cap::Untyped::from_bits(1),
                    paddr: 0x800000,
                    size_bits: 12,
                    watermark: 16,
                });
                assert!(
                    allocator
                        .preserve_global_prefix_with(0, 4096, |_, _, _| panic!(
                            "preflight must refuse"
                        ))
                        .is_err()
                );
                assert_eq!(allocator.untypeds[0].unwrap().watermark, 16);
                assert_eq!(allocator.preserved_len, 0);
                allocator.slots.initialize(100..150).unwrap();
                allocator.preserved_len = MAX_PRESERVED;
                assert!(
                    allocator
                        .preserve_global_prefix_with(0, 4096, |_, _, _| panic!(
                            "descriptor preflight must refuse"
                        ))
                        .is_err()
                );
                assert_eq!(allocator.free_slots(), 50);
                assert_eq!(allocator.untypeds[0].unwrap().watermark, 16);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn prefixes_are_partitioned_without_alignment_loss_or_overlap() {
        for start in (0..4096).step_by(16) {
            for end in (start..8192).step_by(16) {
                let count = prefix_blocks(start, end, 4).unwrap();
                let mut cursor = start;
                let mut observed = 0;
                while cursor < end {
                    let bits = prefix_block(cursor, end, 4).unwrap();
                    let bytes = 1usize << bits;
                    assert!(cursor.is_multiple_of(bytes));
                    assert!(bytes <= end - cursor);
                    cursor += bytes;
                    observed += 1;
                }
                assert_eq!(cursor, end);
                assert_eq!(observed, count);
            }
        }
        assert_eq!(prefix_blocks(16, 1 << 21, 4), Some(17));
        assert_eq!(prefix_blocks(1, 4096, 4), None);
        assert_eq!(prefix_blocks(16, 4095, 4), None);
        assert_eq!(prefix_blocks(32, 16, 4), None);
        assert_eq!(prefix_blocks(0, 0, usize::BITS as usize), None);
        assert_eq!(prefix_blocks(16, 16, 4), Some(0));
    }
}
