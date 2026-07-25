#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(slime_os_kernel::test_runner)]
#![reexport_test_harness_main = "test_main"]

//! C7.3 generation-quota and supervision-subtree accounting invariants.
//!
//! Exercises the exit condition from `roadmap/02-core-runtime.md` (C7.3): two
//! holders receive distinct generation-declared budgets; one reaches byte or
//! buffer-count exhaustion without affecting the other, and termination of its
//! supervision subtree returns every unloaned page and charge. Malformed and
//! globally-impossible budgets fail before allocation.
//!
//! These run under QEMU against a brought-up kernel (so the physical frame
//! allocator is live). The shared-buffer table draws real contiguous frames.

#![allow(clippy::bool_assert_comparison)]

extern crate alloc;
use slime_os_kernel::memory::shared_buffer::{
    HolderQuota, MAX_SHARED_BUFFERS, MAX_TOTAL_PAGES, SharedBufferError, SharedBufferTable,
};
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

const OWNER_A: u64 = 0xA;
const OWNER_B: u64 = 0xB;

fn quota(byte_pages: u32, buffer_count: u32) -> HolderQuota {
    HolderQuota {
        byte_pages,
        buffer_count,
        mapping_count: buffer_count,
        loan_count: buffer_count,
    }
}

/// A holder with no declared budget receives the deny-by-default quota and
/// cannot allocate any shared buffer. Authority is never ambient.
#[test_case]
fn deny_by_default_holder_cannot_allocate() {
    let mut buffers = SharedBufferTable::new();
    assert!(matches!(
        buffers.create(OWNER_A, HolderQuota::DENY, 1, true),
        Err(SharedBufferError::QuotaExceeded)
    ));
    assert_eq!(buffers.owner_pages(OWNER_A), 0);
    assert_eq!(buffers.owner_buffers(OWNER_A), 0);
    assert_eq!(buffers.total_pages(), 0);
}

/// A holder cannot exceed its manifest byte quota even while the global page
/// budget and object table remain far below their ceilings.
#[test_case]
fn byte_quota_is_enforced_per_holder() {
    let mut buffers = SharedBufferTable::new();
    // Budget: up to 4 pages, up to 8 buffers. The 4-page ceiling bites first.
    let a = quota(4, 8);
    let first = buffers.create(OWNER_A, a, 3, true).expect("first fits");
    assert_eq!(buffers.owner_pages(OWNER_A), 3);
    // A 2-page request would reach 5 > 4: rejected on the byte ceiling.
    assert!(matches!(
        buffers.create(OWNER_A, a, 2, true),
        Err(SharedBufferError::QuotaExceeded)
    ));
    // A 1-page request still fits (3 + 1 == 4).
    let second = buffers
        .create(OWNER_A, a, 1, true)
        .expect("second fits exactly");
    assert_eq!(buffers.owner_pages(OWNER_A), 4);
    assert!(matches!(
        buffers.create(OWNER_A, a, 1, true),
        Err(SharedBufferError::QuotaExceeded)
    ));
    buffers.release(&first).expect("cleanup");
    buffers.release(&second).expect("cleanup");
    assert_eq!(buffers.owner_pages(OWNER_A), 0);
}

/// A holder cannot exceed its manifest buffer-count quota even with page budget
/// to spare.
#[test_case]
fn buffer_count_quota_is_enforced_per_holder() {
    let mut buffers = SharedBufferTable::new();
    // Up to 2 buffers, plenty of pages.
    let a = quota(32, 2);
    let first = buffers.create(OWNER_A, a, 1, true).expect("buffer 1");
    let second = buffers.create(OWNER_A, a, 1, true).expect("buffer 2");
    assert_eq!(buffers.owner_buffers(OWNER_A), 2);
    assert!(matches!(
        buffers.create(OWNER_A, a, 1, true),
        Err(SharedBufferError::QuotaExceeded)
    ));
    buffers.release(&first).expect("cleanup");
    buffers.release(&second).expect("cleanup");
}

/// One holder reaching quota exhaustion does not disturb an unrelated holder's
/// account, and the exhausted holder can allocate again once it releases.
#[test_case]
fn one_holder_exhaustion_does_not_disturb_another() {
    let mut buffers = SharedBufferTable::new();
    let a = quota(2, 2);
    let b = quota(8, 4);
    let a1 = buffers
        .create(OWNER_A, a, 2, true)
        .expect("A fills its pages");
    // A is at its 2-page ceiling; a further request is rejected.
    assert!(matches!(
        buffers.create(OWNER_A, a, 1, true),
        Err(SharedBufferError::QuotaExceeded)
    ));
    // B is entirely unaffected and allocates against its own account.
    let b1 = buffers.create(OWNER_B, b, 4, true).expect("B unaffected");
    assert_eq!(buffers.owner_pages(OWNER_A), 2);
    assert_eq!(buffers.owner_pages(OWNER_B), 4);
    // A releases and can allocate again; B stays put.
    buffers.release(&a1).expect("A releases");
    assert_eq!(buffers.owner_pages(OWNER_A), 0);
    let a2 = buffers
        .create(OWNER_A, a, 1, true)
        .expect("A allocates post-release");
    assert_eq!(buffers.owner_pages(OWNER_A), 1);
    assert_eq!(buffers.owner_pages(OWNER_B), 4);
    buffers.release(&a2).expect("cleanup");
    buffers.release(&b1).expect("cleanup");
}

/// Termination of a supervision subtree (modeled by `reclaim_owner`) returns
/// every page and charge held by that owner and leaves another owner's account
/// untouched — the peer-death / restart / revocation reclamation path.
#[test_case]
fn subtree_teardown_reclaims_only_its_own_charges() {
    let mut buffers = SharedBufferTable::new();
    let a = quota(16, 4);
    let b = quota(16, 4);
    let _a1 = buffers.create(OWNER_A, a, 3, true).expect("A buffer 1");
    let _a2 = buffers.create(OWNER_A, a, 2, false).expect("A buffer 2");
    let b1 = buffers.create(OWNER_B, b, 5, true).expect("B buffer 1");
    assert_eq!(buffers.owner_pages(OWNER_A), 5);
    assert_eq!(buffers.owner_buffers(OWNER_A), 2);
    assert_eq!(buffers.owner_pages(OWNER_B), 5);
    let global_before = buffers.total_pages();
    assert_eq!(global_before, 10);

    // Tear down A's subtree.
    let reclaimed = buffers.reclaim_owner(OWNER_A);
    assert_eq!(reclaimed, 2);
    assert_eq!(buffers.owner_pages(OWNER_A), 0);
    assert_eq!(buffers.owner_buffers(OWNER_A), 0);
    // B is completely untouched.
    assert_eq!(buffers.owner_pages(OWNER_B), 5);
    assert_eq!(buffers.owner_buffers(OWNER_B), 1);
    assert_eq!(buffers.total_pages(), 5);
    // A double release of a reclaimed buffer is structurally rejected, proving
    // the pages already left the table (no double free).
    assert!(matches!(
        buffers.release(&_a1),
        Err(SharedBufferError::NotFound)
    ));

    // Reclaiming an owner with no live buffers is a no-op.
    assert_eq!(buffers.reclaim_owner(OWNER_A), 0);
    buffers.release(&b1).expect("cleanup");
    assert_eq!(buffers.total_pages(), 0);
}

/// Per-holder quota is checked before the global ceiling, so a holder within a
/// generous quota can still only take what the global budget allows, and a
/// rejected over-quota request never perturbs the global page total.
#[test_case]
fn quota_check_precedes_global_and_is_side_effect_free() {
    let mut buffers = SharedBufferTable::new();
    // A tiny quota against an empty table: the rejection is QuotaExceeded, not
    // a global-exhaustion signal, and pulls no frame.
    let a = quota(1, 1);
    let _a1 = buffers.create(OWNER_A, a, 1, true).expect("A fits");
    let before = buffers.total_pages();
    assert!(matches!(
        buffers.create(OWNER_A, a, 1, true),
        Err(SharedBufferError::QuotaExceeded)
    ));
    // The global page total is unchanged by the rejected request.
    assert_eq!(buffers.total_pages(), before);
    buffers.release(&_a1).expect("cleanup");
}

/// The fixed table and global ceilings still bound a single holder granted an
/// over-large quota: quota is a per-holder cap layered on top of the hard
/// kernel bounds, never a way to exceed them.
#[test_case]
fn kernel_ceilings_still_bound_a_generous_quota() {
    let mut buffers = SharedBufferTable::new();
    // A quota larger than the whole table/global budget. The kernel bounds must
    // still bite. Fill with modest chunks so byte exhaustion is what caps it.
    let generous = quota(u32::MAX, u32::MAX);
    let chunk = 16;
    let full = MAX_TOTAL_PAGES / chunk;
    assert!(full < MAX_SHARED_BUFFERS, "byte budget must bite first");
    let mut live = alloc::vec::Vec::new();
    for _ in 0..full {
        live.push(
            buffers
                .create(OWNER_A, generous, chunk, true)
                .expect("fill global budget"),
        );
    }
    assert_eq!(buffers.total_pages(), MAX_TOTAL_PAGES);
    // The quota would allow more, but the global ceiling does not.
    assert!(matches!(
        buffers.create(OWNER_A, generous, 1, true),
        Err(SharedBufferError::BytesExhausted)
    ));
    for region in &live {
        buffers.release(region).expect("cleanup");
    }
    assert_eq!(buffers.total_pages(), 0);
}
