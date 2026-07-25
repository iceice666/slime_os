#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(slime_os_kernel::test_runner)]
#![reexport_test_harness_main = "test_main"]

//! C7.4 shared-buffer mapping and irreversible read-only sealing invariants.
//!
//! Exercises the exit condition from `roadmap/02-core-runtime.md` (C7.4): a
//! holder maps only an in-bounds region charged to its manifest quota, seals the
//! buffer read-only, and cannot recover write access; malformed ranges and
//! lifecycle misuse fail before page-table changes. These tests use real x86-64
//! page tables and shared-buffer frames under QEMU.

extern crate alloc;

use slime_os_kernel::memory::address_space::AddressSpace;
use slime_os_kernel::memory::shared_buffer::{HolderQuota, SharedBufferError, SharedBufferTable};
use slime_os_kernel::memory::{PAGE_SIZE, PhysAddr};
use slime_os_kernel::{gdt, interrupts, memory};

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    slime_os_kernel::limine::ensure_linked();
    unsafe { slime_os_kernel::boot::init_from_limine() };
    gdt::init();
    interrupts::init();
    memory::init();
    test_main();
    slime_os_kernel::hlt_loop()
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    slime_os_kernel::test_panic_handler(info)
}

const OWNER_A: u64 = 0xA4;
const OWNER_B: u64 = 0xB4;
const BASE_A: u64 = 0x0000_0001_0000_0000;
const BASE_B: u64 = 0x0000_0002_0000_0000;

fn quota(byte_pages: u32, buffers: u32, mappings: u32) -> HolderQuota {
    HolderQuota {
        byte_pages,
        buffer_count: buffers,
        mapping_count: mappings,
        loan_count: 0,
    }
}

/// An admitted subrange maps the exact buffer frame, consumes one mapping
/// charge, and disappears (with its charge) on unmap.
#[test_case]
fn in_bounds_mapping_is_exact_and_charged() {
    let mut buffers = SharedBufferTable::new();
    let space = AddressSpace::new().expect("address space");
    let q = quota(8, 2, 2);
    let region = buffers.create(OWNER_A, q, 3, true).expect("buffer");

    buffers
        .map(
            OWNER_A,
            q,
            &region,
            space.pml4(),
            BASE_A,
            PAGE_SIZE as u64,
            PAGE_SIZE as u64,
            true,
        )
        .expect("map middle page");
    assert_eq!(buffers.owner_mappings(OWNER_A), 1);
    assert!(space.user_range_mapped(BASE_A, PAGE_SIZE, true));
    assert_eq!(
        space.user_translation(BASE_A),
        Some(PhysAddr(region.phys().0 + PAGE_SIZE as u64)),
        "mapping must name the exact granted buffer page"
    );

    buffers
        .unmap(OWNER_A, &region, space.pml4(), BASE_A)
        .expect("unmap");
    assert_eq!(buffers.owner_mappings(OWNER_A), 0);
    assert!(!space.user_range_mapped(BASE_A, PAGE_SIZE, false));
    assert!(matches!(
        buffers.unmap(OWNER_A, &region, space.pml4(), BASE_A),
        Err(SharedBufferError::NotFound)
    ));
    buffers.release(&region).expect("cleanup");
}

/// Zero, misaligned, out-of-bounds, and overflowed ranges are all rejected
/// before any PTE or mapping charge changes.
#[test_case]
fn malformed_ranges_are_side_effect_free() {
    let mut buffers = SharedBufferTable::new();
    let space = AddressSpace::new().expect("address space");
    let q = quota(8, 2, 8);
    let region = buffers.create(OWNER_A, q, 2, true).expect("buffer");
    let page = PAGE_SIZE as u64;
    let bad = [
        (BASE_A, 0, 0),
        (BASE_A + 1, 0, page),
        (BASE_A, 1, page),
        (BASE_A, 0, page + 1),
        (BASE_A, page, page * 2),
        (BASE_A, u64::MAX - (page - 1), page),
    ];
    for (base, offset, length) in bad {
        assert!(matches!(
            buffers.map(
                OWNER_A,
                q,
                &region,
                space.pml4(),
                base,
                offset,
                length,
                false,
            ),
            Err(SharedBufferError::BadRange)
        ));
    }
    assert_eq!(buffers.owner_mappings(OWNER_A), 0);
    assert!(!space.user_range_mapped(BASE_A, PAGE_SIZE * 2, false));
    buffers.release(&region).expect("cleanup");
}

/// Per-holder mapping quota exhaustion does not disturb another holder's
/// mapping, and unmap returns quota to the exhausted holder.
#[test_case]
fn mapping_quota_is_per_holder_and_reusable() {
    let mut buffers = SharedBufferTable::new();
    let space_a = AddressSpace::new().expect("A address space");
    let space_b = AddressSpace::new().expect("B address space");
    let qa = quota(4, 2, 1);
    let qb = quota(4, 2, 2);
    let a = buffers.create(OWNER_A, qa, 2, true).expect("A buffer");
    let b = buffers.create(OWNER_B, qb, 2, true).expect("B buffer");
    let page = PAGE_SIZE as u64;

    buffers
        .map(OWNER_A, qa, &a, space_a.pml4(), BASE_A, 0, page, true)
        .expect("A mapping 1");
    assert!(matches!(
        buffers.map(
            OWNER_A,
            qa,
            &a,
            space_a.pml4(),
            BASE_A + page,
            page,
            page,
            true,
        ),
        Err(SharedBufferError::QuotaExceeded)
    ));
    buffers
        .map(OWNER_B, qb, &b, space_b.pml4(), BASE_B, 0, page, true)
        .expect("B unaffected");
    assert!(space_b.user_range_mapped(BASE_B, PAGE_SIZE, true));

    buffers
        .unmap(OWNER_A, &a, space_a.pml4(), BASE_A)
        .expect("return A charge");
    buffers
        .map(
            OWNER_A,
            qa,
            &a,
            space_a.pml4(),
            BASE_A + page,
            page,
            page,
            true,
        )
        .expect("A reuses charge");
    assert!(space_b.user_range_mapped(BASE_B, PAGE_SIZE, true));

    buffers.release(&a).expect("cleanup A");
    buffers.release(&b).expect("cleanup B");
}

/// Sealing downgrades every existing writable PTE before publishing the sealed
/// state. New read-only maps remain possible; no later writable map succeeds.
#[test_case]
fn seal_is_irreversible_and_downgrades_live_mappings() {
    let mut buffers = SharedBufferTable::new();
    let space = AddressSpace::new().expect("address space");
    let q = quota(4, 2, 4);
    let region = buffers.create(OWNER_A, q, 2, true).expect("buffer");
    let page = PAGE_SIZE as u64;

    buffers
        .map(OWNER_A, q, &region, space.pml4(), BASE_A, 0, page, true)
        .expect("writable map");
    assert!(space.user_range_mapped(BASE_A, PAGE_SIZE, true));

    buffers.seal(&region).expect("seal");
    assert!(region.sealed());
    assert!(space.user_range_mapped(BASE_A, PAGE_SIZE, false));
    assert!(!space.user_range_mapped(BASE_A, PAGE_SIZE, true));
    assert!(matches!(
        buffers.map(
            OWNER_A,
            q,
            &region,
            space.pml4(),
            BASE_A + page,
            page,
            page,
            true,
        ),
        Err(SharedBufferError::WriteDenied)
    ));
    buffers
        .map(
            OWNER_A,
            q,
            &region,
            space.pml4(),
            BASE_A + page,
            page,
            page,
            false,
        )
        .expect("read-only map after seal");
    assert!(space.user_range_mapped(BASE_A + page, PAGE_SIZE, false));
    assert!(!space.user_range_mapped(BASE_A + page, PAGE_SIZE, true));
    buffers.seal(&region).expect("idempotent reseal");
    buffers.release(&region).expect("cleanup");
}

/// A buffer created read-only cannot be widened to writable even before a seal.
#[test_case]
fn created_read_only_region_cannot_be_widened() {
    let mut buffers = SharedBufferTable::new();
    let space = AddressSpace::new().expect("address space");
    let q = quota(2, 1, 1);
    let region = buffers.create(OWNER_A, q, 1, false).expect("buffer");
    let page = PAGE_SIZE as u64;

    assert!(matches!(
        buffers.map(OWNER_A, q, &region, space.pml4(), BASE_A, 0, page, true,),
        Err(SharedBufferError::WriteDenied)
    ));
    buffers
        .map(OWNER_A, q, &region, space.pml4(), BASE_A, 0, page, false)
        .expect("read-only map");
    assert!(!space.user_range_mapped(BASE_A, PAGE_SIZE, true));
    buffers.release(&region).expect("cleanup");
}

/// A conflict discovered after an earlier page was installed rolls that page
/// back, leaving the pre-existing mapping and all charges unchanged.
#[test_case]
fn map_conflict_rolls_back_partial_page_table_changes() {
    let mut buffers = SharedBufferTable::new();
    let space = AddressSpace::new().expect("address space");
    let q = quota(8, 4, 4);
    let blocker = buffers.create(OWNER_A, q, 1, true).expect("blocker");
    let candidate = buffers.create(OWNER_A, q, 2, true).expect("candidate");
    let page = PAGE_SIZE as u64;

    buffers
        .map(
            OWNER_A,
            q,
            &blocker,
            space.pml4(),
            BASE_A + page,
            0,
            page,
            true,
        )
        .expect("pre-existing second page");
    assert!(matches!(
        buffers.map(
            OWNER_A,
            q,
            &candidate,
            space.pml4(),
            BASE_A,
            0,
            page * 2,
            true,
        ),
        Err(SharedBufferError::MapConflict)
    ));
    assert!(!space.user_range_mapped(BASE_A, PAGE_SIZE, false));
    assert!(space.user_range_mapped(BASE_A + page, PAGE_SIZE, true));
    assert_eq!(buffers.owner_mappings(OWNER_A), 1);
    buffers.release(&candidate).expect("candidate cleanup");
    buffers.release(&blocker).expect("blocker cleanup");
}

/// Releasing a live buffer removes every mapping before returning its frames;
/// stale map/unmap attempts fail structurally against the released identity.
#[test_case]
fn release_unmaps_before_free_and_rejects_stale_region() {
    let mut buffers = SharedBufferTable::new();
    let space = AddressSpace::new().expect("address space");
    let q = quota(2, 1, 2);
    let region = buffers.create(OWNER_A, q, 1, true).expect("buffer");
    let page = PAGE_SIZE as u64;
    buffers
        .map(OWNER_A, q, &region, space.pml4(), BASE_A, 0, page, true)
        .expect("map");

    buffers.release(&region).expect("release mapped buffer");
    assert!(!space.user_range_mapped(BASE_A, PAGE_SIZE, false));
    assert_eq!(buffers.owner_mappings(OWNER_A), 0);
    assert!(matches!(
        buffers.map(OWNER_A, q, &region, space.pml4(), BASE_A, 0, page, false,),
        Err(SharedBufferError::NotFound)
    ));
    assert!(matches!(
        buffers.unmap(OWNER_A, &region, space.pml4(), BASE_A),
        Err(SharedBufferError::NotFound)
    ));
}

/// Supervision-subtree reclamation removes only the target owner's mappings
/// and buffers; an unrelated holder's mapping remains present and writable.
#[test_case]
fn subtree_cleanup_does_not_disturb_unrelated_mapping() {
    let mut buffers = SharedBufferTable::new();
    let space_a = AddressSpace::new().expect("A address space");
    let space_b = AddressSpace::new().expect("B address space");
    let q = quota(4, 2, 2);
    let a = buffers.create(OWNER_A, q, 1, true).expect("A buffer");
    let b = buffers.create(OWNER_B, q, 1, true).expect("B buffer");
    let page = PAGE_SIZE as u64;
    buffers
        .map(OWNER_A, q, &a, space_a.pml4(), BASE_A, 0, page, true)
        .expect("A map");
    buffers
        .map(OWNER_B, q, &b, space_b.pml4(), BASE_B, 0, page, true)
        .expect("B map");

    assert_eq!(buffers.reclaim_owner(OWNER_A), 1);
    assert!(!space_a.user_range_mapped(BASE_A, PAGE_SIZE, false));
    assert_eq!(buffers.owner_mappings(OWNER_A), 0);
    assert!(space_b.user_range_mapped(BASE_B, PAGE_SIZE, true));
    assert_eq!(buffers.owner_mappings(OWNER_B), 1);
    buffers.release(&b).expect("cleanup B");
}
