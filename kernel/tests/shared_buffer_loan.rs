#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(slime_os_kernel::test_runner)]
#![reexport_test_harness_main = "test_main"]

//! C7.5 shared-buffer loan/return and peer-fault reclamation invariants.
//!
//! Exercises the C7.5 exit condition: a lender loans one exact sealed region,
//! release retains its pages until return, invalid returns fail closed, receiver
//! mappings remain read-only and in range, and either peer's death settles every
//! affected charge without disturbing an unrelated owner.

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

const LENDER: u64 = 0xA5;
const RECEIVER: u64 = 0xB5;
const OTHER: u64 = 0xC5;
const BASE_RECEIVER: u64 = 0x0000_0003_0000_0000;
const BASE_OTHER: u64 = 0x0000_0004_0000_0000;

fn quota(pages: u32, buffers: u32, mappings: u32, loans: u32) -> HolderQuota {
    HolderQuota {
        byte_pages: pages,
        buffer_count: buffers,
        mapping_count: mappings,
        loan_count: loans,
    }
}

#[test_case]
fn loans_require_a_sealed_exact_region() {
    let mut buffers = SharedBufferTable::new();
    let lender = quota(4, 2, 2, 1);
    let region = buffers.create(LENDER, lender, 2, true).expect("buffer");
    let page = PAGE_SIZE as u64;

    assert!(matches!(
        buffers.loan(LENDER, RECEIVER, lender, &region, 0, page),
        Err(SharedBufferError::NotSealed)
    ));
    buffers.seal(&region).expect("seal");
    for (offset, length) in [(1, page), (0, 0), (page, page * 2), (u64::MAX, page)] {
        assert!(matches!(
            buffers.loan(LENDER, RECEIVER, lender, &region, offset, length),
            Err(SharedBufferError::BadRange)
        ));
    }
    assert_eq!(buffers.owner_loans(LENDER), 0);
    buffers.release(&region).expect("cleanup");
}

#[test_case]
fn release_retains_pages_until_single_return() {
    let mut buffers = SharedBufferTable::new();
    let receiver_space = AddressSpace::new().expect("receiver address space");
    let lender = quota(4, 2, 1, 1);
    let receiver = quota(0, 0, 1, 0);
    let region = buffers.create(LENDER, lender, 2, true).expect("buffer");
    buffers.seal(&region).expect("seal");
    let loan = buffers
        .loan(
            LENDER,
            RECEIVER,
            lender,
            &region,
            PAGE_SIZE as u64,
            PAGE_SIZE as u64,
        )
        .expect("loan");

    buffers.release_by(LENDER, &region).expect("local release");
    assert_eq!(buffers.total_pages(), 2);
    assert_eq!(buffers.owner_pages(LENDER), 2);
    assert_eq!(buffers.owner_buffers(LENDER), 1);
    assert_eq!(buffers.owner_loans(LENDER), 1);
    assert!(matches!(
        buffers.map(
            LENDER,
            lender,
            &region,
            receiver_space.pml4(),
            BASE_RECEIVER,
            0,
            PAGE_SIZE as u64,
            false,
        ),
        Err(SharedBufferError::NotFound)
    ));
    buffers
        .map_loan(
            RECEIVER,
            receiver,
            loan.id(),
            loan.region(),
            receiver_space.pml4(),
            BASE_RECEIVER,
            0,
            PAGE_SIZE as u64,
        )
        .expect("receiver map");
    assert_eq!(
        receiver_space.user_translation(BASE_RECEIVER),
        Some(PhysAddr(region.phys().0 + PAGE_SIZE as u64))
    );

    buffers
        .return_loan(RECEIVER, loan.id(), loan.region())
        .expect("return");
    assert_eq!(buffers.total_pages(), 0);
    assert_eq!(buffers.live_count(), 0);
    assert_eq!(buffers.owner_pages(LENDER), 0);
    assert_eq!(buffers.owner_loans(LENDER), 0);
    assert_eq!(buffers.owner_mappings(RECEIVER), 0);
    assert!(!receiver_space.user_range_mapped(BASE_RECEIVER, PAGE_SIZE, false));
}

#[test_case]
fn stale_duplicate_and_wrong_buffer_returns_are_side_effect_free() {
    let mut buffers = SharedBufferTable::new();
    let lender = quota(4, 2, 1, 1);
    let region = buffers.create(LENDER, lender, 1, true).expect("buffer");
    let wrong = buffers
        .create(LENDER, lender, 1, true)
        .expect("wrong buffer");
    buffers.seal(&region).expect("seal buffer");
    buffers.seal(&wrong).expect("seal wrong buffer");
    let loan = buffers
        .loan(LENDER, RECEIVER, lender, &region, 0, PAGE_SIZE as u64)
        .expect("loan");

    assert!(matches!(
        buffers.return_loan(RECEIVER, loan.id(), &wrong),
        Err(SharedBufferError::NotFound)
    ));
    assert_eq!(buffers.owner_loans(LENDER), 1);
    buffers
        .return_loan(RECEIVER, loan.id(), loan.region())
        .expect("return");
    assert!(matches!(
        buffers.return_loan(RECEIVER, loan.id(), loan.region()),
        Err(SharedBufferError::NotFound)
    ));
    assert!(matches!(
        buffers.revoke_loan(LENDER, loan.id(), loan.region()),
        Err(SharedBufferError::NotFound)
    ));
    assert_eq!(buffers.owner_loans(LENDER), 0);
    buffers.release(&region).expect("cleanup region");
    buffers.release(&wrong).expect("cleanup wrong");
}

#[test_case]
fn receiver_mapping_cannot_escape_or_gain_write_access() {
    let mut buffers = SharedBufferTable::new();
    let receiver_space = AddressSpace::new().expect("receiver address space");
    let lender = quota(4, 1, 1, 1);
    let receiver = quota(0, 0, 1, 0);
    let region = buffers.create(LENDER, lender, 2, true).expect("buffer");
    buffers.seal(&region).expect("seal");
    let page = PAGE_SIZE as u64;
    let loan = buffers
        .loan(LENDER, RECEIVER, lender, &region, page, page)
        .expect("loan");

    assert!(matches!(
        buffers.map_loan(
            RECEIVER,
            receiver,
            loan.id(),
            loan.region(),
            receiver_space.pml4(),
            BASE_RECEIVER,
            page,
            page,
        ),
        Err(SharedBufferError::BadRange)
    ));
    buffers
        .map_loan(
            RECEIVER,
            receiver,
            loan.id(),
            loan.region(),
            receiver_space.pml4(),
            BASE_RECEIVER,
            0,
            page,
        )
        .expect("exact loan map");
    assert!(receiver_space.user_range_mapped(BASE_RECEIVER, PAGE_SIZE, false));
    assert!(!receiver_space.user_range_mapped(BASE_RECEIVER, PAGE_SIZE, true));
    assert_eq!(
        receiver_space.user_translation(BASE_RECEIVER),
        Some(PhysAddr(region.phys().0 + page))
    );
    buffers
        .return_loan(RECEIVER, loan.id(), loan.region())
        .expect("cleanup loan");
    buffers.release(&region).expect("cleanup buffer");
}

#[test_case]
fn loan_quota_is_per_lender_and_reusable_after_revocation() {
    let mut buffers = SharedBufferTable::new();
    let lender = quota(2, 1, 1, 1);
    let region = buffers.create(LENDER, lender, 1, true).expect("buffer");
    buffers.seal(&region).expect("seal");
    let first = buffers
        .loan(LENDER, RECEIVER, lender, &region, 0, PAGE_SIZE as u64)
        .expect("first loan");
    assert!(matches!(
        buffers.loan(LENDER, OTHER, lender, &region, 0, PAGE_SIZE as u64,),
        Err(SharedBufferError::QuotaExceeded)
    ));
    buffers
        .revoke_loan(LENDER, first.id(), first.region())
        .expect("revoke first");
    let second = buffers
        .loan(LENDER, OTHER, lender, &region, 0, PAGE_SIZE as u64)
        .expect("quota reused");
    buffers
        .return_loan(OTHER, second.id(), second.region())
        .expect("return second");
    buffers.release(&region).expect("cleanup");
}

#[test_case]
fn receiver_death_settles_only_its_loan() {
    let mut buffers = SharedBufferTable::new();
    let receiver_space = AddressSpace::new().expect("receiver address space");
    let other_space = AddressSpace::new().expect("other address space");
    let lender = quota(2, 1, 1, 1);
    let receiver = quota(0, 0, 1, 0);
    let other = quota(2, 1, 1, 0);
    let region = buffers.create(LENDER, lender, 1, true).expect("buffer");
    let unrelated = buffers.create(OTHER, other, 1, true).expect("unrelated");
    buffers.seal(&region).expect("seal");
    let loan = buffers
        .loan(LENDER, RECEIVER, lender, &region, 0, PAGE_SIZE as u64)
        .expect("loan");
    buffers
        .map_loan(
            RECEIVER,
            receiver,
            loan.id(),
            loan.region(),
            receiver_space.pml4(),
            BASE_RECEIVER,
            0,
            PAGE_SIZE as u64,
        )
        .expect("receiver map");
    buffers
        .map(
            OTHER,
            other,
            &unrelated,
            other_space.pml4(),
            BASE_OTHER,
            0,
            PAGE_SIZE as u64,
            true,
        )
        .expect("unrelated map");

    assert_eq!(buffers.reclaim_owner(RECEIVER), 0);
    assert_eq!(buffers.owner_loans(LENDER), 0);
    assert_eq!(buffers.owner_mappings(RECEIVER), 0);
    assert_eq!(buffers.owner_pages(LENDER), 1);
    assert!(other_space.user_range_mapped(BASE_OTHER, PAGE_SIZE, true));
    assert_eq!(buffers.owner_mappings(OTHER), 1);
    buffers.release(&region).expect("cleanup lender");
    buffers.release(&unrelated).expect("cleanup unrelated");
}

#[test_case]
fn lender_death_revokes_receiver_and_preserves_unrelated_owner() {
    let mut buffers = SharedBufferTable::new();
    let receiver_space = AddressSpace::new().expect("receiver address space");
    let other_space = AddressSpace::new().expect("other address space");
    let lender = quota(2, 1, 1, 1);
    let receiver = quota(0, 0, 1, 0);
    let other = quota(2, 1, 1, 0);
    let region = buffers.create(LENDER, lender, 1, true).expect("buffer");
    let unrelated = buffers.create(OTHER, other, 1, true).expect("unrelated");
    buffers.seal(&region).expect("seal");
    let loan = buffers
        .loan(LENDER, RECEIVER, lender, &region, 0, PAGE_SIZE as u64)
        .expect("loan");
    buffers
        .map_loan(
            RECEIVER,
            receiver,
            loan.id(),
            loan.region(),
            receiver_space.pml4(),
            BASE_RECEIVER,
            0,
            PAGE_SIZE as u64,
        )
        .expect("receiver map");
    buffers
        .map(
            OTHER,
            other,
            &unrelated,
            other_space.pml4(),
            BASE_OTHER,
            0,
            PAGE_SIZE as u64,
            true,
        )
        .expect("unrelated map");

    assert_eq!(buffers.reclaim_owner(LENDER), 1);
    assert_eq!(buffers.owner_loans(LENDER), 0);
    assert_eq!(buffers.owner_pages(LENDER), 0);
    assert_eq!(buffers.owner_mappings(RECEIVER), 0);
    assert!(!receiver_space.user_range_mapped(BASE_RECEIVER, PAGE_SIZE, false));
    assert!(matches!(
        buffers.map_loan(
            RECEIVER,
            receiver,
            loan.id(),
            loan.region(),
            receiver_space.pml4(),
            BASE_RECEIVER,
            0,
            PAGE_SIZE as u64,
        ),
        Err(SharedBufferError::NotFound)
    ));
    assert!(other_space.user_range_mapped(BASE_OTHER, PAGE_SIZE, true));
    assert_eq!(buffers.owner_pages(OTHER), 1);
    buffers.release(&unrelated).expect("cleanup unrelated");
}
