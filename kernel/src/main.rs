#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(crate::test_runner)]
#![reexport_test_harness_main = "test_main"]

mod frame_buffer;

extern crate alloc;
use core::panic::PanicInfo;
use frame_buffer::init_framebuffer;
use linked_list_allocator::LockedHeap;

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

/// This function is called on panic.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    loop {}
}

bootloader_api::entry_point!(kernel_main);

#[unsafe(no_mangle)]
fn kernel_main(boot_info: &'static mut bootloader_api::BootInfo) -> ! {
    #[cfg(test)]
    test_main();

    let  bootloader_api::BootInfo { framebuffer, .. } = boot_info;
    if let Some(framebuffer) = boot_info.framebuffer.as_mut() {
        init_framebuffer(framebuffer);
    }

    println!("Hello World{}", "!");

   

    loop {}
}

#[cfg(test)]
pub fn test_runner(tests: &[&dyn Fn()]) {
    // println!("Running {} tests", tests.len());
    for test in tests {
        test();
    }
}
