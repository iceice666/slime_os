#![no_std]
#![cfg_attr(test, no_main)]
#![feature(custom_test_frameworks)]
#![test_runner(crate::test_runner)]
#![reexport_test_harness_main = "test_main"]

pub mod frame_buffer;
pub mod serial;

use core::panic::PanicInfo;

#[cfg(test)]
setup_test_entry!();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum QemuExitCode {
    Success = 0x10,
    Failed = 0x11,
    TestFailed = 0x12,
}

pub fn exit_qemu(exit_code: QemuExitCode) {
    use x86_64::instructions::port::Port;

    unsafe {
        let mut port = Port::new(0xf4);
        port.write(exit_code as u32);
    }
}

pub trait Testable {
    fn run(&self) -> ();
}

impl<T> Testable for T
where
    T: Fn(),
{
    fn run(&self) {
        serial_print!("{}...\t", core::any::type_name::<T>());
        self();
        serial_println!("[Passed]");
    }
}

pub fn test_runner(tests: &[&dyn Testable]) {
    serial_println!("Running {} test(s)", tests.len());
    for test in tests {
        test.run()
    }
    exit_qemu(QemuExitCode::Success);
}

pub fn test_panic_handler(info: &PanicInfo) -> ! {
    serial_println!("[Failed]");
    serial_println!("Panic: {}", info);
    exit_qemu(QemuExitCode::TestFailed);
    loop {}
}

#[macro_export]
macro_rules! setup_test_entry {
    () => {
        bootloader_api::entry_point!(kernel_test_main);

        pub fn kernel_test_main(_: &'static mut bootloader_api::BootInfo) -> ! {
            test_main();
            loop {}
        }
        #[panic_handler]
        fn panic(info: &core::panic::PanicInfo) -> ! {
            $crate::test_panic_handler(info)
        }
    };

    (main: $main:ident) => {
        bootloader_api::entry_point!($main);
        #[panic_handler]
        fn panic(info: &core::panic::PanicInfo) -> ! {
            $crate::test_panic_handler(info)
        }
    }
}
