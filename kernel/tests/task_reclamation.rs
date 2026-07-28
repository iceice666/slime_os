#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(slime_os_kernel::test_runner)]
#![reexport_test_harness_main = "test_main"]

//! B9: a task that goes away returns every frame it consumed.
//!
//! Before this, `spawn_with_caps_for` mapped image and stack frames that no
//! path ever released: `terminate` left the `Task` in the scheduler forever, so
//! `AddressSpace::drop` never ran, and even when it did it freed only the PML4
//! and deliberately leaked the user-half page tables. Every spawn permanently
//! consumed its pages, so a repeated spawn/exit workload drained the allocator
//! monotonically.
//!
//! The exit condition is conservation: a spawn/release cycle must return the
//! free-frame count to its starting value, with no drift across iterations.
//! That is what these tests measure. They exercise the release path directly
//! (`release_unscheduled`) rather than running a task to termination, because
//! the harness has no user tasks — the live counterpart is
//! `just spawn_service_check`, where real components spawn and exit through
//! `terminate` and the scheduler's reaper.
//!
//! Drift, not absolute equality, is the property under test wherever the kernel
//! may legitimately allocate alongside: the first cycle can consume a heap page
//! that later cycles reuse, so a single before/after comparison would be flaky
//! in exactly the way that hides a real leak. Comparing successive deltas
//! catches a per-cycle leak of even one frame.

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

use slime_os_kernel::capability::{Capability, KernelObject, RIGHT_SEND};
use slime_os_kernel::memory::pmm::FRAME_ALLOCATOR;
use slime_os_kernel::task::{self, SpawnError};
use slime_os_kernel::{gdt, interrupts, ipc, memory};

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

fn free_frames() -> usize {
    FRAME_ALLOCATOR.lock().free_frames()
}

/// A single-segment executable component image (`contracts/component/v1`) whose
/// segment spans `pages` pages, so the spawn's frame cost is a known quantity.
fn image_spanning(pages: usize) -> Vec<u8> {
    use slime_os_kernel::component::*;
    let mem_len = pages * slime_os_kernel::memory::PAGE_SIZE;
    let mut image = Vec::new();
    image.extend_from_slice(
        &WireImageHeader {
            magic: IMAGE_MAGIC,
            format_version: FORMAT_VERSION,
            header_size: HEADER_LEN as u32,
            kernel_abi: KERNEL_ABI_VERSION,
            entry_offset: 0,
            segment_count: 1,
            reserved: 0,
            stack_bytes: DEFAULT_STACK_BYTES,
        }
        .encode(),
    );
    image.extend_from_slice(
        &WireSegmentRecord {
            vaddr_offset: 0,
            mem_len: mem_len as u32,
            file_offset: 0,
            // One byte of code; the rest of `mem_len` is `.bss` the kernel
            // zero-fills, which still costs a frame per page.
            file_len: 1,
            flags: SEGMENT_FLAG_EXEC,
            reserved: 0,
        }
        .encode(),
    );
    image.push(0x90);
    image
}

/// A spawn/release cycle is frame-neutral, and stays so across iterations.
///
/// The first cycle is excluded from the comparison: it may grow the kernel heap
/// for the task's `Vec` and boxed kernel stack, which later cycles reuse. Every
/// cycle after that must be exactly zero — a leak of one frame per spawn shows
/// up immediately, and a leak that only appears later still shows up as drift.
#[test_case]
fn spawn_and_release_conserves_frames() {
    let image = image_spanning(2);

    // Warm-up cycle: absorbs any one-time heap growth.
    let id = task::spawn_with_caps(&image, vec![]).expect("warm-up spawn");
    assert!(
        task::release_unscheduled(id),
        "warm-up task was not present"
    );

    for iteration in 0..8 {
        let before = free_frames();
        let id = task::spawn_with_caps(&image, vec![]).expect("spawn");
        assert!(
            free_frames() < before,
            "a spawn must consume frames, or this test proves nothing"
        );
        assert!(
            task::release_unscheduled(id),
            "spawned task was not present"
        );
        let after = free_frames();
        assert_eq!(
            after,
            before,
            "spawn/release leaked {} frame(s) on iteration {iteration}",
            before.saturating_sub(after)
        );
    }
}

/// A larger image costs more frames and returns all of them, so the release
/// path scales with what the spawn actually mapped rather than freeing a fixed
/// prefix.
#[test_case]
fn release_returns_every_mapped_frame() {
    let small = image_spanning(1);
    let large = image_spanning(16);

    // Warm-up: both sizes, so neither measurement below pays heap growth.
    for image in [&small, &large] {
        let id = task::spawn_with_caps(image, vec![]).expect("warm-up spawn");
        assert!(task::release_unscheduled(id));
    }

    let baseline = free_frames();
    let id = task::spawn_with_caps(&small, vec![]).expect("small spawn");
    let small_cost = baseline - free_frames();
    assert!(task::release_unscheduled(id));
    assert_eq!(free_frames(), baseline, "small spawn leaked");

    let id = task::spawn_with_caps(&large, vec![]).expect("large spawn");
    let large_cost = baseline - free_frames();
    assert!(task::release_unscheduled(id));
    assert_eq!(free_frames(), baseline, "large spawn leaked");

    // 15 extra segment pages, and both spawns pay the same stack and page-table
    // cost, so the difference is exactly the extra image pages.
    assert_eq!(
        large_cost - small_cost,
        15,
        "release did not scale with the mapped image"
    );
}

/// A task holding capabilities releases them with its frames, so a spawn that
/// carried a grant costs no more at rest than one that did not.
#[test_case]
fn release_conserves_frames_for_a_task_holding_capabilities() {
    let image = image_spanning(1);
    let granted = || {
        let (endpoint, _peer) = ipc::channel();
        vec![Capability {
            object: KernelObject::Endpoint(endpoint),
            rights: RIGHT_SEND,
        }]
    };

    let id = task::spawn_with_caps(&image, granted()).expect("warm-up spawn");
    assert!(task::release_unscheduled(id));

    for iteration in 0..4 {
        let before = free_frames();
        let id = task::spawn_with_caps(&image, granted()).expect("spawn");
        assert!(task::release_unscheduled(id));
        assert_eq!(
            free_frames(),
            before,
            "spawn/release with a capability leaked on iteration {iteration}"
        );
    }
}

/// A rejected spawn costs nothing.
///
/// `spawn_with_caps_for` builds the address space before it validates the
/// capability set, so a bad grant leaves a fully mapped image behind unless the
/// failure path releases it. This drives that path through the public API and
/// asserts conservation.
#[test_case]
fn a_rejected_spawn_leaks_nothing() {
    let image = image_spanning(2);
    // `RIGHT_SEND` is meaningless for an executable, so the capability table
    // rejects it after the address space is already built.
    let bad = || {
        vec![Capability {
            object: KernelObject::Executable {
                name: None,
                bytes: &[0x90],
                spawn_budget: 0,
            },
            rights: RIGHT_SEND,
        }]
    };

    // Warm-up, then measure: the failure path must be neutral too.
    assert!(matches!(
        task::spawn_with_caps(&image, bad()),
        Err(SpawnError::BadCapability)
    ));

    for iteration in 0..4 {
        let before = free_frames();
        assert!(matches!(
            task::spawn_with_caps(&image, bad()),
            Err(SpawnError::BadCapability)
        ));
        assert_eq!(
            free_frames(),
            before,
            "a rejected spawn leaked on iteration {iteration}"
        );
    }
}

/// A shared-buffer frame mapped into a task's user half is not freed twice when
/// that task goes away.
///
/// This is the sharpest hazard the teardown introduces. `free_user_half` frees
/// every present user leaf it finds, and a mapped shared buffer *is* such a
/// leaf — but the frame belongs to `SharedBufferTable`, not to the address
/// space. Freeing it here would hand the allocator a frame the buffer table
/// still owns and later frees again.
///
/// What makes it safe is ordering: `terminate` calls `reclaim_owner` first,
/// which tears down the holder's mappings through `unmap_user_page_in` and
/// clears those leaves before any reap walks the table. This test pins that
/// ordering by driving it directly — map a buffer into a task's address space,
/// reclaim as the buffer table would, then release the task — and asserting the
/// allocator ends where it started rather than gaining a frame it does not own.
#[test_case]
fn releasing_a_task_does_not_double_free_shared_buffer_frames() {
    use slime_os_kernel::memory::shared_buffer::{HolderQuota, SHARED_BUFFER_TABLE};

    const BASE: u64 = 0x0000_0011_0000_0000;
    let image = image_spanning(1);
    let owner: u64 = 0xB9;
    let quota = HolderQuota {
        byte_pages: 4,
        buffer_count: 1,
        mapping_count: 2,
        loan_count: 0,
    };

    // Warm-up so neither the heap nor the buffer table grows during the
    // measured cycle.
    let id = task::spawn_with_caps(&image, vec![]).expect("warm-up spawn");
    assert!(task::release_unscheduled(id));

    let baseline = free_frames();
    let id = task::spawn_with_caps(&image, vec![]).expect("spawn");
    let root = task::address_space_root(id).expect("spawned task has an address space");

    let region = SHARED_BUFFER_TABLE
        .lock()
        .create(owner, quota, 1, true)
        .expect("create shared buffer");
    SHARED_BUFFER_TABLE
        .lock()
        .map(
            owner,
            quota,
            &region,
            root,
            BASE,
            0,
            slime_os_kernel::memory::PAGE_SIZE as u64,
            true,
        )
        .expect("map shared buffer into the task");

    // Termination order: the buffer table reclaims its own frames and clears
    // the leaves it installed, and only then does the task go away.
    SHARED_BUFFER_TABLE.lock().reclaim_owner(owner);
    assert!(task::release_unscheduled(id));

    assert_eq!(
        free_frames(),
        baseline,
        "a mapped shared buffer was double-freed or leaked across task release"
    );
}
