//! Milestone 1 integration tests: physical/virtual memory, kernel heap, and
//! the APIC timer, exercised against a fully brought-up kernel.
//!
//! Unlike the default test harness (which only pulls in the Limine request
//! block), this binary runs the real init sequence — GDT, IDT, memory, timer —
//! in its own `_start` so the tests run against live subsystems, then hands off
//! to the generated `test_main`.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(slime_os_kernel::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;

use slime_os_kernel::memory::pmm::FRAME_ALLOCATOR;
use slime_os_kernel::memory::vmm::{self, PTE_NO_EXECUTE, PTE_WRITABLE};
use slime_os_kernel::memory::{PAGE_SIZE, PhysAddr, VirtAddr};
use slime_os_kernel::{gdt, interrupts, memory, time};

/// Limine entry point for this integration-test binary.
///
/// # Safety
///
/// Must only be called by the Limine bootloader, which sets up the stack,
/// paging, and GDT before jumping here.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    slime_os_kernel::limine::ensure_linked();
    gdt::init();
    interrupts::init();
    memory::init();
    time::init();
    test_main();
    slime_os_kernel::hlt_loop()
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    slime_os_kernel::test_panic_handler(info)
}

/// The PMM reports a non-empty pool and never exceeds its total.
#[test_case]
fn pmm_pool_populated() {
    let fa = FRAME_ALLOCATOR.lock();
    assert!(fa.total_frames() > 0, "PMM seeded no frames");
    assert!(fa.free_frames() <= fa.total_frames());
    // The heap consumed some frames at init, so free < total.
    assert!(fa.free_frames() < fa.total_frames());
}

/// A frame round-trips: alloc gives a page-aligned, non-zero frame; dealloc
/// returns it and the free count is restored.
#[test_case]
fn pmm_alloc_dealloc_roundtrip() {
    let before = FRAME_ALLOCATOR.lock().free_frames();

    let frame = FRAME_ALLOCATOR.lock().alloc().expect("out of frames");
    assert_eq!(frame.0 % PAGE_SIZE as u64, 0, "frame not page-aligned");
    assert_ne!(frame.0, 0, "PMM handed out frame 0");
    assert_eq!(FRAME_ALLOCATOR.lock().free_frames(), before - 1);

    // SAFETY: `frame` came from `alloc` and is otherwise unused.
    unsafe { FRAME_ALLOCATOR.lock().dealloc(frame) };
    assert_eq!(FRAME_ALLOCATOR.lock().free_frames(), before);
}

/// Two distinct allocations never hand out the same frame.
#[test_case]
fn pmm_allocs_are_distinct() {
    let a = FRAME_ALLOCATOR.lock().alloc().expect("out of frames");
    let b = FRAME_ALLOCATOR.lock().alloc().expect("out of frames");
    assert_ne!(a.0, b.0, "PMM handed out the same frame twice");
    // SAFETY: both frames came from `alloc` and are unused.
    unsafe {
        FRAME_ALLOCATOR.lock().dealloc(a);
        FRAME_ALLOCATOR.lock().dealloc(b);
    }
}

/// The VMM can map a fresh frame at a new VA, translate it back, read/write
/// through it, and refuse to double-map.
#[test_case]
fn vmm_map_translate_readwrite() {
    // A test VA well clear of the kernel image, HHDM, and heap.
    let virt = VirtAddr(0xffff_f000_dead_0000 & !0xfff);
    let frame = FRAME_ALLOCATOR.lock().alloc().expect("out of frames");

    // SAFETY: fresh frame, mapped writable/NX at an otherwise-unused VA.
    unsafe { vmm::map_page(virt, frame, PTE_WRITABLE | PTE_NO_EXECUTE).expect("map failed") };

    // Translation resolves to the physical frame we mapped.
    assert_eq!(vmm::translate(virt), Some(frame));

    // The mapping is usable: write then read back through the VA.
    let ptr = virt.as_mut_ptr::<u64>();
    // SAFETY: `virt` is now mapped writable for at least 8 bytes.
    unsafe {
        ptr.write_volatile(0x00c0_ffee_1234_5678);
        assert_eq!(ptr.read_volatile(), 0x00c0_ffee_1234_5678);
    }

    // Double-mapping the same page is reported, not silently overwritten.
    // SAFETY: same VA; expected to fail with AlreadyMapped.
    let again = unsafe { vmm::map_page(virt, frame, PTE_WRITABLE) };
    assert_eq!(again, Err(vmm::MapError::AlreadyMapped));
}

/// `translate` returns `None` for an address that was never mapped.
#[test_case]
fn vmm_translate_unmapped_is_none() {
    let unmapped = VirtAddr(0xffff_f000_beef_0000);
    assert_eq!(vmm::translate(unmapped), None);
}

/// The heap serves a growing `Vec` (forces reallocation) with correct data.
#[test_case]
fn heap_vec_grows() {
    let mut v: Vec<u64> = Vec::new();
    for i in 0..1000 {
        v.push(i);
    }
    assert_eq!(v.len(), 1000);
    assert_eq!(v.iter().sum::<u64>(), (0..1000).sum());
}

/// `Box` allocates on the heap and dereferences correctly.
#[test_case]
fn heap_box_alloc() {
    let b = Box::new([7u8; 512]);
    assert_eq!(b[0], 7);
    assert_eq!(b[511], 7);
    assert_eq!(b.len(), 512);
}

/// Freed heap memory is reusable: many alloc/drop cycles do not exhaust it.
#[test_case]
fn heap_reuse_after_free() {
    for _ in 0..2048 {
        let b = Box::new([0u8; 1024]);
        core::hint::black_box(&b);
    }
    // Reaching here without an allocation-error panic means memory was reused.
}

/// The APIC timer was calibrated and is delivering interrupts, so the tick
/// counter advances over a bounded wait.
#[test_case]
fn timer_ticks_advance() {
    assert!(time::apic::timer_count() > 0, "timer not calibrated");
    let before = time::ticks();
    time::sleep_ms(30);
    let after = time::ticks();
    assert!(
        after > before,
        "timer ticks did not advance: {before} -> {after}"
    );
}

/// `translate` of a heap address (mapped during init) resolves to a real frame,
/// confirming the VMM installed the heap mapping the allocator relies on.
#[test_case]
fn heap_backing_is_mapped() {
    let b = Box::new(0xa5u8);
    let va = VirtAddr(&*b as *const u8 as u64);
    let phys = vmm::translate(va).expect("heap VA not mapped");
    assert_eq!(phys.0 % PAGE_SIZE as u64, va.0 % PAGE_SIZE as u64);
    assert!(matches!(phys, PhysAddr(_)));
}
