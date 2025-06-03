#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(slime_os_kernel::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use slime_os_kernel::frame_buffer::init_framebuffer;
use slime_os_kernel::println;

mod testing;

/////////////////////////////////////////////////////

bootloader_api::entry_point!(kernel_main);

pub fn kernel_main(boot_info: &'static mut bootloader_api::BootInfo) -> ! {
    if let Some(framebuffer) = boot_info.framebuffer.as_mut() {
        init_framebuffer(framebuffer);
    }

    println!("Hello World{}", "!");

    #[cfg(test)]
    test_main();


    loop {}
}

/// This function is called on panic.
#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("{}", info);
    loop {}
}

#[cfg(test)]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    slime_os_kernel::test_panic_handler(info)
}

