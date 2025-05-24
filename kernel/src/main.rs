#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(crate::test_runner)]
#![reexport_test_harness_main = "test_main"]

mod frame_buffer;
use frame_buffer::init_framebuffer;

use core::panic::PanicInfo;

/// This function is called on panic.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("{}", info);
    loop {}
}

bootloader_api::entry_point!(kernel_main);

#[unsafe(no_mangle)]
fn kernel_main(boot_info: &'static mut bootloader_api::BootInfo) -> ! {
    unsafe {
        core::arch::asm!("nop", options(nomem, nostack));
    }

    #[cfg(test)]
    test_main();

    if let Some(framebuffer) = boot_info.framebuffer.as_mut() {
        init_framebuffer(framebuffer);
    }

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
