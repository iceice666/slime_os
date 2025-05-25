use crate::frame_buffer::init_framebuffer;
use crate::println;
use core::panic::PanicInfo;

#[cfg(not(feature = "kernel_test"))]
pub fn kernel_main(boot_info: &'static mut bootloader_api::BootInfo) -> ! {
    unsafe {
        core::arch::asm!("nop", options(nomem, nostack));
    }

    if let Some(framebuffer) = boot_info.framebuffer.as_mut() {
        init_framebuffer(framebuffer);
    }

    println!("Hello World{}", "!");

    unsafe {
        core::arch::asm!("nop", options(nomem, nostack));
    }

    loop {}
}

/// This function is called on panic.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("{}", info);
    loop {}
}
