//! Host ownership-census tests; no kernel allocation or cleanup is executed.

extern crate std;

use super::*;

fn on_host_stack(test: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(test)
        .unwrap()
        .join()
        .unwrap();
}

fn fixture() -> ObjectAllocator {
    let mut allocator = ObjectAllocator::empty();
    allocator.extents.provision_host(16, None);
    allocator.slots.initialize(100..200).unwrap();
    allocator
}

fn extent(index: usize, bits: usize, kind: ExtentKind) -> ExtentRecord {
    let mut extent = ExtentRecord::new(sel4::cap::Untyped::from_bits((200 + index) as _), bits);
    extent.paddr = (index + 1) << 21;
    extent.assign(index, 1, kind);
    extent
}

#[test]
fn system_census_counts_rounded_static_and_mapping_until_returned() {
    on_host_stack(|| {
        let mut allocator = fixture();
        assert_eq!(allocator.system_backing_bytes(), 0);
        assert_eq!(allocator.elastic_inventory().bytes, 0);
        allocator.extents[0] = Some(extent(0, 16, ExtentKind::Static));
        assert_eq!(allocator.system_backing_bytes(), 1 << 16);
        allocator.extents[1] = Some(extent(1, 13, ExtentKind::MappingTables));
        assert_eq!(allocator.system_backing_bytes(), (1 << 16) + (1 << 13));

        // Rounded ownership is independent of object occupancy and revoke progress.
        allocator.extents[0].as_mut().unwrap().bytes = 4096;
        allocator.extents[0].as_mut().unwrap().watermark = 4096;
        allocator.extents[0].as_mut().unwrap().objects = 1;
        allocator.extents[0].as_mut().unwrap().revoked = true;
        allocator.extents[1].as_mut().unwrap().revoked = true;
        assert_eq!(allocator.system_backing_bytes(), (1 << 16) + (1 << 13));
        assert_eq!(allocator.elastic_inventory().bytes, 0);

        allocator.extents[2] = Some(extent(2, 18, ExtentKind::PrivateData));
        allocator.extents[3] = Some(extent(3, 12, ExtentKind::PrivateTables));
        assert_eq!(allocator.system_backing_bytes(), (1 << 16) + (1 << 13));
        assert_eq!(allocator.elastic_inventory().bytes, 0);
    });
}

#[test]
fn returned_static_transfers_system_ownership_to_common_inventory_exactly_once() {
    on_host_stack(|| {
        let mut allocator = fixture();
        allocator.untypeds[0] = Some(UntypedRegion {
            cap: sel4::cap::Untyped::from_bits(1),
            paddr: 0x10000000,
            size_bits: 14,
            watermark: 4096,
        });
        allocator.untyped_len = 1;
        allocator.extents[0] = Some(extent(0, 16, ExtentKind::Static));
        let common_before = allocator.elastic_inventory().bytes;
        let baseline = common_before + allocator.system_backing_bytes();
        assert_eq!(common_before, 3 * 4096);
        assert_eq!(baseline, 3 * 4096 + (1 << 16));

        let retained = allocator.extents[0].as_mut().unwrap();
        retained.revoked = true;
        assert_eq!(allocator.elastic_inventory().bytes, common_before);
        assert_eq!(allocator.system_backing_bytes(), 1 << 16);

        // Model publication after complete cleanup, not a host kernel revoke.
        let returned = allocator.extents[0].as_mut().unwrap();
        returned.active = false;
        returned.owner = u16::MAX;
        returned.watermark = 0;
        returned.objects = 0;
        returned.bytes = 0;
        for _ in 0..2 {
            assert_eq!(allocator.system_backing_bytes(), 0);
            assert_eq!(allocator.elastic_inventory().bytes, baseline);
            assert_eq!(allocator.reusable_extent_bytes(), 1 << 16);
        }
    });
}

#[test]
fn system_census_keeps_global_root_guarantee_and_split_owners_disjoint() {
    on_host_stack(|| {
        let mut allocator = fixture();
        // Seed an independently observable counter value; this does not test
        // publication by allocate(), contiguous allocation, or the high probe.
        allocator.global_object_backing_bytes = 2096;
        allocator.extents[0] = Some(extent(0, 16, ExtentKind::Static));
        allocator.extents[1] = Some(extent(1, 13, ExtentKind::MappingTables));
        let system = 2096 + (1 << 16) + (1 << 13);
        assert_eq!(allocator.system_backing_bytes(), system);

        let root = extent(2, 17, ExtentKind::Infrastructure);
        allocator.infrastructure.pin_for_test(UntypedRegion {
            cap: root.parent,
            paddr: root.paddr,
            size_bits: root.size_bits,
            watermark: 4096,
        });
        allocator.extents[2] = Some(root);
        assert_eq!(allocator.infrastructure_owned_bytes(), 1 << 17);
        assert_eq!(allocator.system_backing_bytes(), system);

        let mut guaranteed = extent(3, 15, ExtentKind::Static);
        guaranteed.reserve(0, ExtentKind::Static);
        allocator.extents[3] = Some(guaranteed);
        assert_eq!(allocator.reserved_extent_bytes(), 1 << 15);
        assert_eq!(allocator.system_backing_bytes(), system);
        allocator.extents[3]
            .as_mut()
            .unwrap()
            .assign(3, 2, ExtentKind::MappingTables);
        assert_eq!(allocator.reserved_extent_bytes(), 0);
        assert_eq!(allocator.borrowed_extent_bytes(), 1 << 15);
        assert_eq!(allocator.system_backing_bytes(), system);

        // The parent describes the same bytes as these two leaves, not extra RAM.
        let mut parent = extent(4, 15, ExtentKind::Static);
        parent.split = true;
        parent.children = [5, 6];
        let mut left = extent(5, 14, ExtentKind::Static);
        left.paddr = parent.paddr;
        let mut right = extent(6, 14, ExtentKind::MappingTables);
        right.paddr = parent.paddr + (1 << 14);
        allocator.extents[4] = Some(parent);
        allocator.extents[5] = Some(left);
        allocator.extents[6] = Some(right);
        assert_eq!(allocator.system_backing_bytes(), system + (1 << 15));
        assert_eq!(allocator.elastic_inventory().bytes, 0);
        let baseline = allocator.system_backing_bytes()
            + allocator.infrastructure_owned_bytes() as u64
            + allocator.reserved_extent_bytes() as u64
            + allocator.borrowed_extent_bytes() as u64
            + allocator.elastic_inventory().bytes;
        assert_eq!(baseline, system + (1 << 17) + 2 * (1 << 15));
    });
}

#[test]
fn shared_system_census_retains_whole_roots_across_split_lease_and_release() {
    on_host_stack(|| {
        let mut allocator = fixture();
        allocator.global_object_backing_bytes = 2096;
        let root = allocator.shared_backing.vacant_root().unwrap();
        allocator.shared_backing.insert_root(root, 300, 0x200000, 2);
        let second = allocator.shared_backing.vacant_root().unwrap();
        allocator
            .shared_backing
            .insert_root(second, 301, 0x400000, 0);
        let shared_bytes = 5 * 4096;
        let baseline = 2096 + shared_bytes as u64;
        assert_eq!(allocator.retained_shared_bytes(), shared_bytes);
        assert_eq!(allocator.system_backing_bytes(), baseline);
        assert_eq!(allocator.elastic_inventory().bytes, 0);

        let children = allocator.shared_backing.split_positions(root).unwrap();
        allocator
            .shared_backing
            .commit_split(root, children, [302, 303]);
        assert_eq!(allocator.system_backing_bytes(), baseline);
        allocator.shared_backing.lease(children[0]);
        assert_eq!(allocator.shared_backing.reusable_bytes(), 3 * 4096);
        assert_eq!(allocator.retained_shared_bytes(), shared_bytes);
        assert_eq!(allocator.system_backing_bytes(), baseline);
        allocator.shared_backing.quarantine(children[0]);
        assert_eq!(allocator.system_backing_bytes(), baseline);
        allocator.shared_backing.commit_release(children[0]);
        assert_eq!(allocator.shared_backing.reusable_bytes(), shared_bytes);
        assert_eq!(allocator.system_backing_bytes(), baseline);
        allocator.shared_backing.commit_merge(root);
        assert_eq!(allocator.retained_shared_bytes(), shared_bytes);
        assert_eq!(allocator.system_backing_bytes(), baseline);
        assert_eq!(allocator.elastic_inventory().bytes, 0);
    });
}
