#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(slime_os_kernel::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use core::panic::PanicInfo;

use slime_os_kernel::frame_buffer::init_framebuffer;
use slime_os_kernel::{gdt, interrupts, memory, println, serial_println, time};
#[cfg(test)]
mod testing;

/// Limine-compatible entry point.
///
/// On x86-64 Limine enters with a valid stack and following the SysV AMD64
/// ABI. We mark it `extern "C"`, `no_mangle`, and `-> !` so the linker can
/// use it as the ELF entry (see `ENTRY(_start)` in `linker.ld`) and so the
/// compiler emits no prologue that depends on a frame we do not have.
/// `unsafe`: the body touches machine state (framebuffer memory, the test
/// harness) that Rust cannot prove is safe.
///
/// # Safety
///
/// Must only be called by the Limine bootloader, which sets up the stack,
/// paging, and GDT before jumping here. Calling it from any other context
/// is undefined behavior.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    kernel_main();

    slime_os_kernel::hlt_loop()
}

fn kernel_main() {
    // Console first: every subsequent diagnostic is visible.
    init_framebuffer();
    println!("Slime OS Framework bring-up image");
    serial_println!("[serial] Hello World{}!", "");

    // GDT/TSS before the IDT: the IDT gates read our code selector, and the
    // Double Fault gate needs the TSS's IST stack to exist.
    gdt::init();
    serial_println!("[serial] GDT+TSS loaded");
    println!("[bringup] GDT+TSS loaded");

    // Load the IDT so exceptions route to our handlers instead of
    // triple-faulting QEMU into a silent reset.
    interrupts::init();
    serial_println!("[serial] IDT loaded");
    println!("[bringup] IDT loaded");

    // Physical + virtual memory and the kernel heap. After this, `alloc`
    // works and page faults are reported deterministically.
    memory::init();
    {
        let fa = memory::pmm::FRAME_ALLOCATOR.lock();
        serial_println!(
            "[serial] PMM: {} / {} frames free",
            fa.free_frames(),
            fa.total_frames(),
        );
    }
    serial_println!("[serial] heap online");
    println!("[bringup] heap online");

    // Prove the heap really works before relying on it.
    {
        use alloc::vec::Vec;
        let mut v = Vec::new();
        for i in 0..256 {
            v.push(i * i);
        }
        serial_println!("[serial] heap check: sum={}", v.iter().sum::<u64>());
    }

    // APIC timer: interrupt-driven monotonic clock. Enables interrupts.
    time::init();
    serial_println!(
        "[serial] APIC timer online (count={})",
        time::apic::timer_count()
    );
    println!("[bringup] APIC timer online");

    #[cfg(not(test))]
    {
        // #BP: trigger a breakpoint and prove we come back.
        // SAFETY: `int3` is a trap; our #BP handler returns normally.
        unsafe {
            core::arch::asm!("int3", options(nostack, preserves_flags));
        }
        serial_println!("[serial] survived int3");
        println!("[bringup] survived int3");

        // Prove the timer is ticking: wait ~50 ms and confirm uptime advanced.
        let before = time::ticks();
        time::sleep_ms(50);
        serial_println!(
            "[serial] timer ticks: {} -> {} (uptime {} ms)",
            before,
            time::ticks(),
            time::uptime_ms(),
        );
        println!("[bringup] timer ticks {} -> {}", before, time::ticks(),);

        slime_os_kernel::bootstrap::start();
    }

    #[cfg(test)]
    test_main();
}

/// This function is called on panic.
#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("{}", info);
    slime_os_kernel::hlt_loop()
}

#[cfg(test)]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    slime_os_kernel::test_panic_handler(info)
}
