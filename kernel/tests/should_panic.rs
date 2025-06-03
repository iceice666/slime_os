#![no_std]
#![no_main]

slime_os_kernel::setup_test_entry!(main: _main);

use slime_os_kernel::QemuExitCode;
use slime_os_kernel::exit_qemu;
use slime_os_kernel::serial_println;

pub fn _main(_: &'static mut bootloader_api::BootInfo) -> ! {
    should_fail();
    serial_println!("[test did not panic]");
    exit_qemu(QemuExitCode::Failed);
    loop {}
}

fn should_fail() {
    serial_println!("should_panic::should_fail...");
    assert_eq!(0, 1);
}
