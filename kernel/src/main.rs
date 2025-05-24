#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(crate::test_runner)]
#![reexport_test_harness_main = "test_main"]

// NEED FIX: Panic on heap allocation
#[cfg(feature="uefi")]
mod frame_buffer;
#[cfg(feature="uefi")]
use frame_buffer::init_framebuffer;

#[cfg(feature="bios")]
mod vga_buffer;

use core::panic::PanicInfo;

/// This function is called on panic.
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    // Don't use println! in panic handler to avoid recursive panic
    loop {}
}

bootloader_api::entry_point!(kernel_main);

#[unsafe(no_mangle)]
fn kernel_main(_boot_info: &'static mut bootloader_api::BootInfo) -> ! {
    unsafe {
        core::arch::asm!("nop", options(nomem, nostack));
    }

    #[cfg(test)]
    test_main();

    // if let Some(framebuffer) = boot_info.framebuffer.as_mut() {
    //     init_framebuffer(framebuffer);
    // }

    println!("Hello World{}", "!");

    unsafe {
        core::arch::asm!("nop", options(nomem, nostack));
    }

    loop {}
}

#[cfg(test)]
pub fn test_runner(tests: &[&dyn Fn()]) {
    // println!("Running {} tests", tests.len());
    for test in tests {
        test();
    }
}
