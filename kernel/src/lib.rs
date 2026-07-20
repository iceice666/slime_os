#![no_std]
#![cfg_attr(test, no_main)]
#![feature(abi_x86_interrupt, custom_test_frameworks)]
#![test_runner(crate::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

pub mod acpi;
pub mod block_proto;
pub mod block_service;
pub mod boot;
pub mod bootstrap;
pub mod capability;
pub mod component;
pub mod crc32;
pub mod crt;
pub mod dma;
pub mod frame_buffer;
pub mod gdt;
pub mod generation;
pub mod generation_manager;
pub mod gpt;
pub mod input;
pub mod interrupts;
pub mod ipc;
pub mod limine;
pub mod memory;
pub mod object_store;
pub mod pci;
pub mod platform;
pub mod serial;
pub mod sha256;
pub mod store_proto;
pub mod store_service;
pub mod syscall;
pub mod task;
pub mod time;
pub mod trap;
pub mod virtio_blk;

use core::panic::PanicInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum QemuExitCode {
    Success = 0x10,
    Failed = 0x11,
    TestFailed = 0x12,
}

pub fn exit_qemu(exit_code: QemuExitCode) {
    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx") 0xf4_u16,
            in("eax") exit_code as u32,
            options(nomem, nostack, preserves_flags),
        );
    }
}

pub fn hlt_loop() -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
        }
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
    hlt_loop()
}

pub fn test_expected_panic_handler(info: &PanicInfo) -> ! {
    serial_println!("[Passed]");
    serial_println!("Expected panic: {}", info);
    exit_qemu(QemuExitCode::Success);
    hlt_loop()
}
#[macro_export]
macro_rules! setup_test_entry {
    () => {
        // Limine entry point for the default test harness. We do not touch
        // the framebuffer in tests (keeps output deterministic over serial
        // only), but we must still pull in the Limine request block so the
        // bootloader honors it.
        ///
        /// # Safety
        ///
        /// Must only be called by the Limine bootloader.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn _start() -> ! {
            $crate::limine::ensure_linked();
            unsafe { $crate::boot::init_from_limine() };
            test_main();
            $crate::hlt_loop()
        }
        #[panic_handler]
        fn panic(info: &core::panic::PanicInfo) -> ! {
            $crate::test_panic_handler(info)
        }
    };
    (expected_panic: $main:ident) => {
        #[allow(unreachable_code)]
        // Variant for `should_panic`-style tests: the user supplies the
        // main function that is expected to panic; we just provide the
        // Limine entry shell around it and a panic handler that treats
        // the panic as success.
        ///
        /// # Safety
        ///
        /// Must only be called by the Limine bootloader.
        #[allow(unreachable_code)]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn _start() -> ! {
            $crate::limine::ensure_linked();
            unsafe { $crate::boot::init_from_limine() };
            $main(());
            $crate::hlt_loop()
        }
        #[panic_handler]
        fn panic(info: &core::panic::PanicInfo) -> ! {
            $crate::test_expected_panic_handler(info)
        }
    };
}

#[cfg(test)]
setup_test_entry!();
