#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(slime_os_kernel::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;

use slime_os_kernel::frame_buffer::init_framebuffer;
use slime_os_kernel::println;
use slime_os_kernel::serial_println;
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
    println!("Hello World{}", "!");
    serial_println!("[serial] Hello World{}!", "");

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
