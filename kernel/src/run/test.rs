use crate::{QemuExitCode, exit_qemu};
use crate::{println, serial_println};
use core::panic::PanicInfo;
use linkme::distributed_slice;

pub type TestResult = Result<(), &'static str>;

#[distributed_slice]
pub static KERNEL_TESTS: [fn() -> TestResult];

#[unsafe(no_mangle)]
pub fn kernel_main(boot_info: &'static mut bootloader_api::BootInfo) -> ! {
    serial_println!("Running {} test(s)", KERNEL_TESTS.len());
    let mut success_counter = 0;
    let mut fail_counter = 0;

    for test in KERNEL_TESTS {
        match test() {
            Ok(()) => {
                serial_println!("Test passed");
                success_counter += 1;
            }
            Err(msg) => {
                serial_println!("Test failed: {}", msg);
                fail_counter += 1;
            }
        }
    }

    serial_println!("{} tests passed", success_counter);
    serial_println!("{} tests failed", fail_counter);

    exit_qemu(QemuExitCode::Success);
    loop {}
}

/// This function is called on panic.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial_println!("{}", info);
    loop {}
}
