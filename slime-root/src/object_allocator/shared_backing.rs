//! Bounded buddy ownership for demand-backed shared-buffer extents.
//!
//! Siblings are reusable only after their common parent is successfully revoked.
//! Kernel operations precede metadata commits; an interrupted split or merge
//! therefore keeps every capability represented until its cleanup is retried.

use super::{AllocError, ObjectAllocator};

const NONE: u16 = u16::MAX;
const MAX_ORDER: usize = crate::shared_buffer::MAX_BUFFER_PAGES.trailing_zeros() as usize;
// A fresh root is acquired only after coalescing and finding no usable free
// leaf. Every existing root of that order therefore contains a lease, so the
// live-buffer bound also bounds roots per order; smaller split leases count.
const MAX_ROOTS: usize = (MAX_ORDER + 1) * crate::shared_buffer::MAX_SHARED_BUFFERS;
// Each live lease can keep at most MAX_ORDER sibling pairs split. Free
// siblings must be coalesced before admitting additional leases; cleanup
// failures retain ownership and block further provisioning.
const MAX_NODES: usize = MAX_ROOTS + 2 * crate::shared_buffer::MAX_SHARED_BUFFERS * MAX_ORDER;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum State {
    Free,
    Split,
    Leased,
    Quarantined,
}

#[derive(Clone, Copy)]
pub(super) struct Node {
    pub slot: usize,
    pub paddr: usize,
    pub order: usize,
    parent: u16,
    children: [u16; 2],
    pub state: State,
}

#[derive(Clone, Copy)]
struct Lease {
    node: usize,
    first: usize,
    pages: usize,
    released: u64,
    committed: u64,
    aborting: bool,
}

pub(super) struct BuddyBacking {
    nodes: [Option<Node>; MAX_NODES],
    roots: usize,
    leases: [Option<Lease>; crate::shared_buffer::MAX_SHARED_BUFFERS],
}

impl BuddyBacking {
    pub const fn new() -> Self {
        Self {
            nodes: [None; MAX_NODES],
            roots: 0,
            leases: [None; crate::shared_buffer::MAX_SHARED_BUFFERS],
        }
    }

    fn exhausted() -> AllocError {
        AllocError::ArenaTableFull { limit: MAX_NODES }
    }

    fn lease_slot(&self) -> Result<usize, AllocError> {
        self.leases
            .iter()
            .position(Option::is_none)
            .ok_or_else(Self::exhausted)
    }

    fn frame_lease(&self, frame: usize) -> Option<(usize, usize)> {
        self.leases.iter().enumerate().find_map(|(index, lease)| {
            let lease = lease.as_ref()?;
            frame
                .checked_sub(lease.first)
                .filter(|offset| *offset < lease.pages)
                .map(|offset| (index, offset))
        })
    }

    pub fn vacant_root(&self) -> Result<usize, AllocError> {
        if self.roots == MAX_ROOTS {
            return Err(Self::exhausted());
        }
        self.nodes
            .iter()
            .position(Option::is_none)
            .ok_or_else(Self::exhausted)
    }

    pub fn insert_root(&mut self, index: usize, slot: usize, paddr: usize, order: usize) {
        assert!(order <= MAX_ORDER && self.nodes[index].is_none());
        self.nodes[index] = Some(Node {
            slot,
            paddr,
            order,
            parent: NONE,
            children: [NONE; 2],
            state: State::Free,
        });
        self.roots += 1;
    }

    pub fn node(&self, index: usize) -> Node {
        self.nodes[index].expect("owned buddy node")
    }

    pub fn best_fit(&self, order: usize) -> Option<usize> {
        self.nodes
            .iter()
            .enumerate()
            .filter_map(|(index, node)| {
                node.filter(|node| node.state == State::Free && node.order >= order)
                    .map(|node| (index, node.order))
            })
            .min_by_key(|(_, order)| *order)
            .map(|(index, _)| index)
    }

    pub fn split_positions(&self, index: usize) -> Result<[usize; 2], AllocError> {
        let node = self.node(index);
        assert!(node.state == State::Free && node.order > 0);
        let mut empty = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node.is_none())
            .map(|(index, _)| index);
        Ok([
            empty.next().ok_or_else(Self::exhausted)?,
            empty.next().ok_or_else(Self::exhausted)?,
        ])
    }

    pub fn commit_split(&mut self, index: usize, children: [usize; 2], slots: [usize; 2]) {
        let parent = self.node(index);
        assert!(parent.state == State::Free && parent.order > 0);
        for side in 0..2 {
            assert!(self.nodes[children[side]].is_none());
            self.nodes[children[side]] = Some(Node {
                slot: slots[side],
                paddr: parent.paddr + side * (4096 << (parent.order - 1)),
                order: parent.order - 1,
                parent: index as u16,
                children: [NONE; 2],
                state: State::Free,
            });
        }
        let parent = self.nodes[index].as_mut().unwrap();
        parent.children = children.map(|child| child as u16);
        parent.state = State::Split;
    }

    pub fn lease(&mut self, index: usize) {
        let node = self.nodes[index].as_mut().unwrap();
        assert_eq!(node.state, State::Free);
        node.state = State::Leased;
    }

    pub fn quarantine(&mut self, index: usize) {
        let node = self.nodes[index].as_mut().unwrap();
        assert!(matches!(node.state, State::Leased | State::Quarantined));
        node.state = State::Quarantined;
    }

    pub fn commit_release(&mut self, index: usize) {
        let node = self.nodes[index].as_mut().unwrap();
        assert_eq!(node.state, State::Quarantined);
        node.state = State::Free;
    }

    pub fn mergeable_parent(&self, index: usize) -> Option<(usize, [usize; 2])> {
        let child = self.node(index);
        if child.parent == NONE {
            return None;
        }
        let parent_index = child.parent as usize;
        let parent = self.node(parent_index);
        let children = parent.children.map(usize::from);
        (parent.state == State::Split
            && children
                .iter()
                .all(|index| self.node(*index).state == State::Free))
        .then_some((parent_index, children))
    }

    pub fn commit_merge(&mut self, index: usize) {
        let parent = self.node(index);
        assert_eq!(parent.state, State::Split);
        for child in parent.children {
            assert_eq!(self.node(child as usize).state, State::Free);
            self.nodes[child as usize] = None;
        }
        let parent = self.nodes[index].as_mut().unwrap();
        parent.children = [NONE; 2];
        parent.state = State::Free;
    }

    pub fn reusable_bytes(&self) -> usize {
        self.nodes
            .iter()
            .flatten()
            .filter(|node| node.state == State::Free)
            .map(|node| 4096 << node.order)
            .sum()
    }

    pub(super) fn report_backing_snapshot(&self) {
        for node in self.nodes.iter().flatten() {
            let parent = if node.parent == NONE {
                0
            } else {
                self.node(node.parent as usize).slot
            };
            sel4::debug_println!(
                "SLIME_BACKING shared_node cap={} parent={} paddr={} bytes={} state={:?}",
                node.slot,
                parent,
                node.paddr,
                4096usize << node.order,
                node.state,
            );
        }
    }

    pub fn retained_bytes(&self) -> usize {
        self.nodes
            .iter()
            .flatten()
            .filter(|node| node.parent == NONE)
            .map(|node| 4096 << node.order)
            .sum()
    }

    pub fn anchors(&self) -> usize {
        self.nodes.iter().flatten().count()
    }

    pub fn pending_merge(&self) -> Option<(usize, [usize; 2])> {
        self.nodes.iter().enumerate().find_map(|(index, node)| {
            let node = node.as_ref()?;
            if node.state != State::Split {
                return None;
            }
            let children = node.children.map(usize::from);
            children
                .iter()
                .all(|child| self.node(*child).state == State::Free)
                .then_some((index, children))
        })
    }
}

impl ObjectAllocator {
    pub fn allocate_shared_granules(&mut self, pages: usize) -> Result<(usize, usize), AllocError> {
        self.settle_shared_releases()?;
        let lease_slot = self.shared_backing.lease_slot()?;
        self.ensure_contiguous_root_slots(pages)?;
        let index = self.acquire_shared_leaf(pages)?;
        let node = self.shared_backing.node(index);
        let (first, reused) = match self.slots.allocate_contiguous(pages, self.slots_allocated) {
            Ok(value) => value,
            Err(error) => {
                self.shared_backing.quarantine(index);
                self.shared_backing.commit_release(index);
                return Err(error);
            }
        };
        for offset in 0..pages {
            if let Err(error) = self
                .physical
                .insert(first + offset, node.paddr + offset * 4096)
            {
                for rollback in 0..offset {
                    self.physical.remove(first + rollback);
                }
                for slot in first..first + pages {
                    self.slots.release(slot);
                }
                self.shared_backing.quarantine(index);
                self.shared_backing.commit_release(index);
                return Err(error);
            }
        }
        let blueprint =
            <sel4::cap_type::Granule as sel4::CapTypeForObjectOfFixedSize>::object_blueprint();
        if let Err(error) = crate::root_cspace::retype(
            sel4::cap::Untyped::from_bits(node.slot as _),
            &blueprint,
            first,
            pages,
        ) {
            for slot in first..first + pages {
                self.physical.remove(slot);
                self.slots.release(slot);
            }
            self.shared_backing.quarantine(index);
            self.shared_backing.commit_release(index);
            return Err(AllocError::Retype {
                size_bits: 12,
                error,
            });
        }
        self.slots_allocated += pages;
        self.slots_reused += reused;
        self.objects_allocated += pages;
        self.live_objects += pages;
        // Parent extent bytes already charge this physical backing.
        self.shared_backing.leases[lease_slot] = Some(Lease {
            node: index,
            first,
            pages,
            released: 0,
            committed: 0,
            aborting: false,
        });
        Ok((first, node.paddr))
    }

    /// Delete an anchor but keep its CSlot and backing quarantined until the
    /// state machine commits the entire teardown batch.
    pub fn release_shared_frame(&mut self, frame: usize) -> Result<bool, AllocError> {
        self.release_shared_frame_with(frame, |slot| {
            sel4::init_thread::slot::CNODE
                .cap()
                .absolute_cptr(sel4::CPtr::from_bits(slot as _))
                .delete()
                .map_err(|error| AllocError::ArenaCleanup { slot, error })
        })
    }

    fn release_shared_frame_with(
        &mut self,
        frame: usize,
        delete: impl FnOnce(usize) -> Result<(), AllocError>,
    ) -> Result<bool, AllocError> {
        let Some((index, offset)) = self.shared_backing.frame_lease(frame) else {
            return Ok(false);
        };
        let lease = self.shared_backing.leases[index].unwrap();
        let bit = 1u64 << offset;
        if lease.released & bit != 0 {
            return Ok(true);
        }
        delete(frame)?;
        self.physical.remove(frame);
        self.live_objects -= 1;
        self.shared_backing.leases[index].as_mut().unwrap().released |= bit;
        self.shared_backing.quarantine(lease.node);
        Ok(true)
    }

    pub fn shared_frame_released(&self, frame: usize) -> bool {
        self.shared_backing
            .frame_lease(frame)
            .is_some_and(|(index, offset)| {
                self.shared_backing.leases[index].unwrap().released & (1u64 << offset) != 0
            })
    }

    pub fn commit_shared_frame_release(&mut self, frame: usize) {
        let Some((index, offset)) = self.shared_backing.frame_lease(frame) else {
            return;
        };
        let lease = self.shared_backing.leases[index].as_mut().unwrap();
        let bit = 1u64 << offset;
        assert_ne!(lease.released & bit, 0);
        lease.committed |= bit;
    }

    /// Mark an unpublished allocation for retryable cleanup. No logical
    /// buffer can refer to these anchors, so successful deletes may commit.
    pub fn abort_shared_allocation(&mut self, first: usize) -> Result<(), AllocError> {
        let Some((index, offset)) = self.shared_backing.frame_lease(first) else {
            return Err(BuddyBacking::exhausted());
        };
        if offset != 0 {
            return Err(BuddyBacking::exhausted());
        }
        self.shared_backing.leases[index].as_mut().unwrap().aborting = true;
        self.settle_shared_releases()
    }

    /// Finish previously committed leases before admitting a new allocation.
    /// A failed revoke leaves the lease and all destination slots quarantined.
    pub fn settle_shared_releases(&mut self) -> Result<(), AllocError> {
        self.settle_shared_leases_with(
            |slot| {
                sel4::init_thread::slot::CNODE
                    .cap()
                    .absolute_cptr(sel4::CPtr::from_bits(slot as _))
                    .delete()
                    .map_err(|error| AllocError::ArenaCleanup { slot, error })
            },
            |slot| {
                sel4::init_thread::slot::CNODE
                    .cap()
                    .absolute_cptr(sel4::CPtr::from_bits(slot as _))
                    .revoke()
                    .map_err(|error| AllocError::ArenaCleanup { slot, error })
            },
        )?;
        self.coalesce_shared_leaves()
    }

    fn settle_shared_leases_with(
        &mut self,
        mut delete: impl FnMut(usize) -> Result<(), AllocError>,
        mut revoke: impl FnMut(usize) -> Result<(), AllocError>,
    ) -> Result<(), AllocError> {
        for index in 0..self.shared_backing.leases.len() {
            let Some(mut lease) = self.shared_backing.leases[index] else {
                continue;
            };
            if lease.aborting {
                for frame in lease.first..lease.first + lease.pages {
                    self.release_shared_frame_with(frame, &mut delete)?;
                    self.commit_shared_frame_release(frame);
                }
                lease = self.shared_backing.leases[index].unwrap();
            }
            let all = if lease.pages == 64 {
                u64::MAX
            } else {
                (1u64 << lease.pages) - 1
            };
            if lease.committed != all {
                continue;
            }
            let node = self.shared_backing.node(lease.node);
            revoke(node.slot)?;
            for frame in lease.first..lease.first + lease.pages {
                self.slots.release(frame);
            }
            self.shared_backing.commit_release(lease.node);
            self.shared_backing.leases[index] = None;
        }
        Ok(())
    }

    pub fn reusable_shared_bytes(&self) -> usize {
        self.shared_backing.reusable_bytes()
    }
    pub fn retained_shared_bytes(&self) -> usize {
        self.shared_backing.retained_bytes()
    }
    pub fn retained_shared_anchors(&self) -> usize {
        self.shared_backing.anchors()
    }

    /// Obtain an untyped leaf without publishing frame ownership.
    pub(super) fn acquire_shared_leaf(&mut self, pages: usize) -> Result<usize, AllocError> {
        if pages == 0 || pages > crate::shared_buffer::MAX_BUFFER_PAGES {
            return Err(BuddyBacking::exhausted());
        }
        self.coalesce_shared_leaves()?;
        let order = pages.next_power_of_two().trailing_zeros() as usize;
        let mut index = if let Some(index) = self.shared_backing.best_fit(order) {
            index
        } else {
            let index = self.shared_backing.vacant_root()?;
            let slot = self.take_slot()?;
            if let Err(error) = self.allocate_from_global(
                sel4::ObjectBlueprint::Untyped {
                    size_bits: 12 + order,
                },
                slot,
            ) {
                self.slots.release(slot);
                return Err(error);
            }
            self.shared_backing
                .insert_root(index, slot, self.last_paddr, order);
            index
        };
        while self.shared_backing.node(index).order > order {
            index = self.split_shared_leaf_with(index, |parent, bits, first| {
                crate::root_cspace::retype(
                    sel4::cap::Untyped::from_bits(parent as _),
                    &sel4::ObjectBlueprint::Untyped { size_bits: bits },
                    first,
                    2,
                )
                .map_err(|error| AllocError::Retype {
                    size_bits: bits,
                    error,
                })
            })?;
        }
        self.shared_backing.lease(index);
        Ok(index)
    }

    fn split_shared_leaf_with(
        &mut self,
        index: usize,
        retype: impl FnOnce(usize, usize, usize) -> Result<(), AllocError>,
    ) -> Result<usize, AllocError> {
        let positions = self.shared_backing.split_positions(index)?;
        let node = self.shared_backing.node(index);
        self.ensure_contiguous_root_slots(2)?;
        let (first, reused) = self.slots.allocate_contiguous(2, self.slots_allocated)?;
        // The kernel validates both destination slots before publishing either
        // child. A refused retype leaves the parent and metadata untouched.
        if let Err(error) = retype(node.slot, 11 + node.order, first) {
            self.slots.release(first);
            self.slots.release(first + 1);
            return Err(error);
        }
        self.slots_allocated += 2;
        self.slots_reused += reused;
        self.objects_allocated += 2;
        self.live_objects += 2;
        self.shared_backing
            .commit_split(index, positions, [first, first + 1]);
        Ok(positions[0])
    }

    pub(super) fn coalesce_shared_leaves(&mut self) -> Result<(), AllocError> {
        self.coalesce_shared_leaves_with(|slot| {
            sel4::init_thread::slot::CNODE
                .cap()
                .absolute_cptr(sel4::CPtr::from_bits(slot as _))
                .revoke()
                .map_err(|error| AllocError::ArenaCleanup { slot, error })
        })
    }

    fn coalesce_shared_leaves_with(
        &mut self,
        mut revoke: impl FnMut(usize) -> Result<(), AllocError>,
    ) -> Result<(), AllocError> {
        while let Some((index, children)) = self.shared_backing.pending_merge() {
            let parent = self.shared_backing.node(index);
            revoke(parent.slot)?;
            // Revoke removes both child untyped capabilities. Their slots stay
            // reserved until this ownership transaction commits.
            for child in children {
                self.slots.release(self.shared_backing.node(child).slot);
            }
            self.live_objects -= 2;
            self.shared_backing.commit_merge(index);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn failed_buddy_split_returns_slots_without_publishing_children() {
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(|| {
                let mut allocator = ObjectAllocator::empty();
                allocator.slots.initialize(10..20).unwrap();
                allocator.take_slot().unwrap();
                allocator.shared_backing.insert_root(0, 10, 0x800000, 2);
                allocator.live_objects = 1;
                let free = allocator.free_slots();
                assert!(
                    allocator
                        .split_shared_leaf_with(0, |parent, bits, first| {
                            assert_eq!((parent, bits, first), (10, 13, 11));
                            Err(AllocError::NoKernelUntyped)
                        })
                        .is_err()
                );
                assert_eq!(allocator.free_slots(), free);
                assert_eq!(allocator.live_objects(), 1);
                assert_eq!(allocator.shared_backing.anchors(), 1);
                assert_eq!(allocator.shared_backing.best_fit(2), Some(0));
                let child = allocator
                    .split_shared_leaf_with(0, |_, _, _| Ok(()))
                    .unwrap();
                assert_eq!(allocator.free_slots(), free - 2);
                assert_eq!(allocator.shared_backing.node(child).paddr, 0x800000);
                assert_eq!(allocator.shared_backing.node(child).order, 1);
                assert_eq!(allocator.shared_backing.anchors(), 3);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn failed_buddy_revoke_preserves_children_and_reserved_slots() {
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(|| {
                let mut allocator = ObjectAllocator::empty();
                allocator.slots.initialize(10..20).unwrap();
                for _ in 0..3 {
                    allocator.take_slot().unwrap();
                }
                allocator.shared_backing.insert_root(0, 10, 0x800000, 1);
                let children = allocator.shared_backing.split_positions(0).unwrap();
                allocator.shared_backing.commit_split(0, children, [11, 12]);
                allocator.live_objects = 3;
                let before = allocator.free_slots();
                assert!(
                    allocator
                        .coalesce_shared_leaves_with(|_| Err(AllocError::NoKernelUntyped))
                        .is_err()
                );
                assert_eq!(allocator.free_slots(), before);
                assert_eq!(allocator.shared_backing.anchors(), 3);
                assert_eq!(allocator.live_objects(), 3);
                assert_eq!(allocator.shared_backing.best_fit(1), None);
                allocator
                    .coalesce_shared_leaves_with(|slot| {
                        assert_eq!(slot, 10);
                        Ok(())
                    })
                    .unwrap();
                assert_eq!(allocator.free_slots(), before + 2);
                assert_eq!(allocator.shared_backing.anchors(), 1);
                assert_eq!(allocator.live_objects(), 1);
                allocator
                    .coalesce_shared_leaves_with(|_| panic!("completed revoke repeated"))
                    .unwrap();
                assert_eq!(allocator.shared_backing.best_fit(1), Some(0));
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn failed_anchor_delete_retains_provenance_and_retry_commits_once() {
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(|| {
                let mut allocator = ObjectAllocator::empty();
                allocator.shared_backing.insert_root(0, 10, 0x800000, 0);
                allocator.shared_backing.lease(0);
                allocator.shared_backing.leases[0] = Some(Lease {
                    node: 0,
                    first: 20,
                    pages: 1,
                    released: 0,
                    committed: 0,
                    aborting: true,
                });
                allocator.physical.insert(20, 0x800000).unwrap();
                allocator.live_objects = 2;
                assert!(
                    allocator
                        .release_shared_frame_with(20, |_| Err(AllocError::NoKernelUntyped))
                        .is_err()
                );
                assert_eq!(allocator.physical_address_of(20), Some(0x800000));
                assert_eq!(allocator.live_objects, 2);
                assert!(!allocator.shared_frame_released(20));
                assert_eq!(
                    allocator.release_shared_frame_with(20, |_| Ok(())),
                    Ok(true)
                );
                assert_eq!(allocator.physical_address_of(20), None);
                assert_eq!(allocator.live_objects, 1);
                assert_eq!(
                    allocator.release_shared_frame_with(20, |_| panic!("duplicate deletion")),
                    Ok(true)
                );
                assert_eq!(allocator.shared_backing.best_fit(0), None);
                allocator.commit_shared_frame_release(20);
                assert_eq!(allocator.shared_backing.leases[0].unwrap().committed, 1);
                assert_eq!(allocator.shared_backing.best_fit(0), None);
                assert!(
                    allocator
                        .settle_shared_leases_with(
                            |_| panic!("completed delete replayed"),
                            |_| Err(AllocError::NoKernelUntyped),
                        )
                        .is_err()
                );
                assert!(allocator.shared_backing.leases[0].is_some());
                assert_eq!(allocator.shared_backing.best_fit(0), None);
                allocator
                    .settle_shared_leases_with(|_| panic!("completed delete replayed"), |_| Ok(()))
                    .unwrap();
                assert!(allocator.shared_backing.leases[0].is_none());
                assert_eq!(allocator.shared_backing.best_fit(0), Some(0));
                assert_eq!(allocator.live_objects, 1);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn mixed_orders_reuse_one_parent_without_merging_live_siblings() {
        std::thread::Builder::new()
            .stack_size(4 * 1024 * 1024)
            .spawn(|| {
                let mut pool = BuddyBacking::new();
                let root = pool.vacant_root().unwrap();
                pool.insert_root(root, 100, 0x800000, MAX_ORDER);
                for order in 0..=MAX_ORDER {
                    let mut leased = std::vec::Vec::new();
                    while let Some(mut index) = pool.best_fit(order) {
                        while pool.node(index).order > order {
                            let children = pool.split_positions(index).unwrap();
                            pool.commit_split(
                                index,
                                children,
                                [1000 + children[0], 1000 + children[1]],
                            );
                            index = children[0];
                        }
                        pool.lease(index);
                        leased.push(index);
                        if leased.len() == crate::shared_buffer::MAX_SHARED_BUFFERS {
                            break;
                        }
                    }
                    for pair in leased.iter().enumerate() {
                        let (done, index) = pair;
                        pool.quarantine(*index);
                        assert_ne!(pool.best_fit(order), Some(*index));
                        pool.commit_release(*index);
                        while let Some((parent, _)) = pool.pending_merge() {
                            pool.commit_merge(parent);
                        }
                        for live in &leased[done + 1..] {
                            assert_eq!(pool.node(*live).state, State::Leased);
                        }
                    }
                    assert_eq!(pool.best_fit(MAX_ORDER), Some(root));
                    assert_eq!(pool.anchors(), 1);
                    assert_eq!(pool.reusable_bytes(), 4096 << MAX_ORDER);
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn split_release_and_coalesce_preserve_capacity_and_quarantine() {
        std::thread::Builder::new()
            .stack_size(4 * 1024 * 1024)
            .spawn(|| {
                let mut pool = BuddyBacking::new();
                let root = pool.vacant_root().unwrap();
                pool.insert_root(root, 10, 0x800000, 6);
                let children = pool.split_positions(root).unwrap();
                // Uncommitted kernel work cannot change the ownership graph.
                assert_eq!(pool.reusable_bytes(), 64 * 4096);
                pool.commit_split(root, children, [11, 12]);
                assert_eq!(pool.node(children[1]).paddr, 0x820000);
                pool.lease(children[0]);
                pool.quarantine(children[0]);
                assert!(pool.mergeable_parent(children[1]).is_none());
                assert_eq!(pool.best_fit(6), None);
                assert_eq!(pool.reusable_bytes(), 32 * 4096);
                pool.commit_release(children[0]);
                assert_eq!(pool.mergeable_parent(children[0]), Some((root, children)));
                assert_eq!(pool.pending_merge(), Some((root, children)));
                // Failed revoke/delete leaves the same merge pending and cannot
                // make the parent available to a new large request.
                assert_eq!(pool.best_fit(6), None);
                assert_eq!(pool.pending_merge(), Some((root, children)));
                pool.commit_merge(root);
                assert_eq!(pool.best_fit(6), Some(root));
                assert_eq!(pool.reusable_bytes(), pool.retained_bytes());
                assert_eq!(pool.anchors(), 1);
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
