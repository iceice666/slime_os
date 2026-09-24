//! Buddy splitting of returned common extents.
//!
//! A task's extents return to common capacity at the size they were taken.
//! Reusing them only at that size strands capacity: once ordinary tails are
//! spent, a base-page growth that needs a 1 MiB extent is refused while
//! hundreds of returned 2 MiB extents sit free. A free extent larger than a
//! request is therefore retyped into two half-size untyped children, and the
//! split repeats until one child fits.
//!
//! Children are ordinary extent records: they are assigned, released and
//! reused exactly as any other. Their parent is neither free nor active while
//! split, so no path can hand its bytes out a second time. When both children
//! are free again the parent is revoked, which deletes both child capabilities,
//! and only then is the parent free at its original size. Kernel work always
//! precedes the metadata change, so a failed retype or revoke leaves every
//! capability represented and the operation retryable.

use super::{AllocError, ExtentRecord, ObjectAllocator};

/// Reselections one request may make after its candidate was taken.
const MAX_SPLIT_RESELECTIONS: usize = 8;
/// No child: the record is not split.
pub(super) const NO_EXTENT: u32 = u32::MAX;
/// The owner an infrastructure-adopted extent records. No task arena has this
/// index, so no arena path can claim or release it.
const INFRASTRUCTURE_OWNER: u16 = u16::MAX - 1;

impl ObjectAllocator {
    /// A free common extent of exactly `size_bits`, if one exists.
    pub(super) fn reusable_extent(&self, size_bits: usize) -> Option<usize> {
        self.extents.iter().position(|entry| {
            entry.is_some_and(|extent| extent.is_common_free() && extent.size_bits == size_bits)
        })
    }

    /// Serve `size_bits` from returned capacity by splitting a larger extent.
    ///
    /// Free siblings are merged first, so a pair two holders returned becomes
    /// one larger extent before any new split is made; then the smallest free
    /// extent larger than the request is halved until a child fits.
    #[cfg(not(test))]
    pub(super) fn acquire_split_extent(&mut self, size_bits: usize) -> Result<usize, AllocError> {
        self.acquire_split_extent_with(
            size_bits,
            |parent| {
                sel4::init_thread::slot::CNODE
                    .cap()
                    .absolute_cptr(parent)
                    .revoke()
                    .map_err(|error| AllocError::ArenaCleanup {
                        slot: parent.bits() as usize,
                        error,
                    })
            },
            |parent, bits, first| {
                crate::root_cspace::retype(
                    parent.parent,
                    &sel4::ObjectBlueprint::Untyped { size_bits: bits },
                    first,
                    2,
                )
                .map_err(|error| AllocError::Retype {
                    size_bits: bits,
                    error,
                })
            },
        )
    }

    fn acquire_split_extent_with(
        &mut self,
        size_bits: usize,
        revoke: impl FnMut(sel4::cap::Untyped) -> Result<(), AllocError>,
        mut retype: impl FnMut(&ExtentRecord, usize, usize) -> Result<(), AllocError>,
    ) -> Result<usize, AllocError> {
        // A merge that cannot complete leaves its pair free and owned; the
        // request may still be served by another split.
        let _ = self.coalesce_extents_with(revoke);
        // Funding a split may adopt a free extent for infrastructure, and that
        // can be the very candidate chosen. A taken candidate is reselected;
        // every retry follows an adoption, so the bound only stops a defect
        // from looping forever.
        for _ in 0..MAX_SPLIT_RESELECTIONS {
            if let Some(index) = self.reusable_extent(size_bits) {
                self.extents_reused += 1;
                return Ok(index);
            }
            let mut index = self
                .extents
                .iter()
                .enumerate()
                .filter_map(|(index, entry)| {
                    entry
                        .filter(|extent| extent.is_common_free() && extent.size_bits > size_bits)
                        .map(|extent| (index, extent.size_bits))
                })
                .min_by_key(|(_, bits)| *bits)
                .map(|(index, _)| index)
                .ok_or(AllocError::NoKernelUntyped)?;
            loop {
                if self.extent(index).size_bits == size_bits {
                    self.extents_reused += 1;
                    return Ok(index);
                }
                match self.split_extent_with(index, &mut retype)? {
                    Some(child) => index = child,
                    None => break,
                }
            }
        }
        Err(AllocError::NoKernelUntyped)
    }

    /// Fund infrastructure from the smallest whole free extent that fits.
    ///
    /// Whole rather than split: a split may itself need a descriptor page,
    /// which is infrastructure's to fund, and adopting a larger extent than
    /// asked for only pre-funds infrastructure growth that already outlives
    /// every task. The record stays active under no task arena, so its bytes
    /// are reported as held and are never offered again.
    pub(super) fn adopt_returned_extent(&mut self, bytes: usize) -> Result<(), AllocError> {
        let index = self
            .extents
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                entry
                    .filter(|extent| extent.is_common_free() && 1usize << extent.size_bits >= bytes)
                    .map(|extent| (index, extent.size_bits))
            })
            .min_by_key(|(_, bits)| *bits)
            .map(|(index, _)| index)
            .ok_or(AllocError::NoKernelUntyped)?;
        let extent = self.extent(index);
        self.infrastructure
            .adopt_extent_source(super::UntypedRegion {
                cap: extent.parent,
                paddr: extent.paddr,
                size_bits: extent.size_bits,
                watermark: 0,
            })?;
        self.extents[index]
            .as_mut()
            .expect("owned extent record")
            .assign(
                usize::from(INFRASTRUCTURE_OWNER),
                0,
                super::ExtentKind::Infrastructure,
            );
        #[cfg(not(test))]
        sel4::debug_println!(
            "SLIME_BACKING infrastructure_extent parent={} paddr={} bytes={}",
            extent.parent.bits(),
            extent.paddr,
            1usize << extent.size_bits,
        );
        Ok(())
    }

    fn extent(&self, index: usize) -> ExtentRecord {
        self.extents[index].expect("owned extent record")
    }

    /// Halve one free extent; answer its lower child.
    ///
    /// Both child records and both destination slots are secured before the
    /// kernel is asked, and the kernel validates both destinations before
    /// publishing either child, so a refused retype changes nothing. Securing
    /// them can fund infrastructure from a free extent, so the candidate is
    /// read only afterwards: `None` means it was taken and nothing was split.
    fn split_extent_with(
        &mut self,
        index: usize,
        retype: impl FnOnce(&ExtentRecord, usize, usize) -> Result<(), AllocError>,
    ) -> Result<Option<usize>, AllocError> {
        self.ensure_extent_descriptors(2)?;
        self.ensure_contiguous_root_slots(2)?;
        let parent = self.extent(index);
        if !parent.is_common_free() {
            return Ok(None);
        }
        if parent.size_bits <= super::GRANULE_BYTES.trailing_zeros() as usize {
            return Err(AllocError::NoKernelUntyped);
        }
        let positions = {
            let mut vacant = self
                .extents
                .iter()
                .enumerate()
                .filter(|(_, entry)| entry.is_none())
                .map(|(index, _)| index);
            let full = AllocError::ArenaTableFull {
                limit: super::MAX_TASK_EXTENTS,
            };
            [vacant.next().ok_or(full)?, vacant.next().ok_or(full)?]
        };
        let (first, reused) = self.slots.allocate_contiguous(2, self.slots_allocated)?;
        let bits = parent.size_bits - 1;
        if let Err(error) = retype(&parent, bits, first) {
            self.slots.release(first);
            self.slots.release(first + 1);
            return Err(error);
        }
        self.slots_allocated += 2;
        self.slots_reused += reused;
        self.objects_allocated += 2;
        self.live_objects += 2;
        for (side, position) in positions.into_iter().enumerate() {
            let slot = first + side;
            let mut child = ExtentRecord::new(
                crate::root_cspace::RootSlot::<sel4::cap_type::Untyped>::from_address(slot).cap(),
                bits,
            );
            child.paddr = parent.paddr + (side << bits);
            self.extents[position] = Some(child);
            #[cfg(not(test))]
            sel4::debug_println!(
                "SLIME_BACKING extent_split parent={} child={} paddr={} bytes={}",
                parent.parent.bits(),
                slot,
                child.paddr,
                1usize << bits,
            );
        }
        let record = self.extents[index].as_mut().expect("owned extent record");
        record.split = true;
        record.children = positions.map(|position| position as u32);
        Ok(Some(positions[0]))
    }

    /// Merge every split extent whose two children are both free.
    fn coalesce_extents_with(
        &mut self,
        mut revoke: impl FnMut(sel4::cap::Untyped) -> Result<(), AllocError>,
    ) -> Result<(), AllocError> {
        while let Some(index) = self.mergeable_extent() {
            let parent = self.extent(index);
            revoke(parent.parent)?;
            // The revoke deleted both child capabilities; their slots and
            // records are released only now that it has succeeded.
            for child in parent.children {
                let child = child as usize;
                self.slots
                    .release(self.extent(child).parent.bits() as usize);
                self.extents[child] = None;
            }
            self.live_objects -= 2;
            let record = self.extents[index].as_mut().expect("owned extent record");
            record.split = false;
            record.children = [NO_EXTENT; 2];
            #[cfg(not(test))]
            sel4::debug_println!(
                "SLIME_BACKING extent_merge parent={} paddr={} bytes={}",
                parent.parent.bits(),
                parent.paddr,
                1usize << parent.size_bits,
            );
        }
        Ok(())
    }

    fn mergeable_extent(&self) -> Option<usize> {
        self.extents.iter().enumerate().find_map(|(index, entry)| {
            let extent = entry.as_ref()?;
            (extent.split
                && extent.children.iter().all(|child| {
                    self.extents[*child as usize].is_some_and(|child| child.is_common_free())
                }))
            .then_some(index)
        })
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    fn free_extent(slot: usize, paddr: usize, size_bits: usize) -> ExtentRecord {
        let mut extent = ExtentRecord::new(sel4::cap::Untyped::from_bits(slot as _), size_bits);
        extent.paddr = paddr;
        extent
    }

    fn with_allocator(test: impl FnOnce(&mut ObjectAllocator) + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(move || {
                let mut allocator = ObjectAllocator::empty();
                allocator.slots.initialize(10..64).unwrap();
                allocator.ensure_extent_descriptors(8).unwrap();
                test(&mut allocator);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn a_returned_extent_splits_for_a_smaller_request_and_merges_back_whole() {
        with_allocator(|allocator| {
            let root = allocator.take_slot().unwrap();
            allocator.extents[0] = Some(free_extent(root, 0x200000, 21));
            allocator.live_objects = 1;
            let free = allocator.free_slots();

            let child = allocator
                .split_extent_with(0, |parent, bits, first| {
                    assert_eq!((parent.parent.bits() as usize, bits), (root, 20));
                    assert!(first >= 10);
                    Ok(())
                })
                .unwrap()
                .unwrap();
            // The parent is no longer free capacity; its two halves are, and
            // together they are exactly its bytes.
            assert!(!allocator.extent(0).is_common_free());
            assert_eq!(allocator.reusable_extent_bytes(), 1 << 21);
            assert_eq!(allocator.reusable_extent_anchors(), 2);
            assert_eq!(allocator.extent(child).paddr, 0x200000);
            let sibling = allocator.extent(0).children[1] as usize;
            assert_eq!(allocator.extent(sibling).paddr, 0x300000);
            assert_eq!(allocator.free_slots(), free - 2);
            assert_eq!(allocator.live_objects, 3);

            // A live child blocks the merge; a free pair merges once.
            allocator.extents[child].as_mut().unwrap().active = true;
            allocator
                .coalesce_extents_with(|_| panic!("merged a live child"))
                .unwrap();
            allocator.extents[child].as_mut().unwrap().active = false;
            allocator
                .coalesce_extents_with(|parent| {
                    assert_eq!(parent.bits() as usize, root);
                    Ok(())
                })
                .unwrap();
            assert!(allocator.extent(0).is_common_free());
            assert_eq!(allocator.reusable_extent_bytes(), 1 << 21);
            assert_eq!(allocator.reusable_extent_anchors(), 1);
            assert_eq!(allocator.free_slots(), free);
            assert_eq!(allocator.live_objects, 1);
            assert!(allocator.extents[child].is_none());
        });
    }

    #[test]
    fn a_request_splits_the_smallest_larger_extent_down_to_its_size() {
        with_allocator(|allocator| {
            for (index, bits) in [(0, 21), (1, 14)] {
                let slot = allocator.take_slot().unwrap();
                allocator.extents[index] = Some(free_extent(slot, 0x800000 * (index + 1), bits));
            }
            let mut splits = std::vec::Vec::new();
            let child = allocator
                .acquire_split_extent_with(
                    12,
                    |_| panic!("nothing was split"),
                    |parent, bits, _| {
                        splits.push((parent.size_bits, bits));
                        Ok(())
                    },
                )
                .unwrap();
            // The 16 KiB extent is halved twice; the 2 MiB one is untouched.
            assert_eq!(splits, [(14, 13), (13, 12)]);
            assert_eq!(allocator.extent(child).size_bits, 12);
            assert_eq!(allocator.extent(child).paddr, 0x1000000);
            assert!(allocator.extent(0).is_common_free());
            assert_eq!(allocator.reusable_extent_bytes(), (1 << 21) + (1 << 14));
        });
    }

    #[test]
    fn a_candidate_taken_while_its_split_was_funded_is_left_alone() {
        with_allocator(|allocator| {
            let root = allocator.take_slot().unwrap();
            allocator.extents[0] = Some(free_extent(root, 0x200000, 21));
            // Funding infrastructure adopted it between selection and split.
            allocator.extents[0].as_mut().unwrap().active = true;
            let free = allocator.free_slots();
            assert_eq!(
                allocator.split_extent_with(0, |_, _, _| panic!("split a taken extent")),
                Ok(None)
            );
            assert!(!allocator.extent(0).split);
            assert_eq!(allocator.free_slots(), free);
        });
    }

    #[test]
    fn a_failed_split_or_merge_leaves_every_capability_owned() {
        with_allocator(|allocator| {
            let root = allocator.take_slot().unwrap();
            allocator.extents[0] = Some(free_extent(root, 0x400000, 21));
            let free = allocator.free_slots();
            assert!(
                allocator
                    .split_extent_with(0, |_, _, _| Err(AllocError::NoKernelUntyped))
                    .is_err()
            );
            assert!(allocator.extent(0).is_common_free());
            assert_eq!(allocator.free_slots(), free);
            assert_eq!(allocator.reusable_extent_anchors(), 1);

            allocator.split_extent_with(0, |_, _, _| Ok(())).unwrap();
            let children = allocator.extent(0).children;
            assert!(
                allocator
                    .coalesce_extents_with(|_| Err(AllocError::NoKernelUntyped))
                    .is_err()
            );
            // Both halves stay owned and free, and the merge stays pending.
            assert!(allocator.extent(0).split);
            for child in children {
                assert!(allocator.extent(child as usize).is_common_free());
            }
            assert_eq!(allocator.mergeable_extent(), Some(0));
            allocator.coalesce_extents_with(|_| Ok(())).unwrap();
            assert_eq!(allocator.free_slots(), free);
            assert_eq!(allocator.mergeable_extent(), None);
        });
    }
}
