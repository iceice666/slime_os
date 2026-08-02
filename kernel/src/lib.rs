#![no_std]
#![cfg_attr(test, no_main)]
// Crate features must be declared at the crate root, so this one x86 mechanism
// cannot live inside `arch::x86_64` with the handlers that use it. It is gated
// on the target so no other architecture enables it, and it is the single
// admitted exception in `just x86_portability_check`.
#![cfg_attr(target_arch = "x86_64", feature(abi_x86_interrupt))]
#![feature(custom_test_frameworks)]
#![test_runner(crate::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

pub mod arch;
pub mod capability;
pub mod drivers;
pub mod ipc;
pub mod memory;
pub mod platform;
pub mod protocol;
pub mod runtime;
pub mod storage;
pub mod support;
pub mod syscall;
pub mod task;
pub mod time;

// Architecture mechanism, re-exported at the crate root so `crate::trap`,
// `crate::gdt`, and friends resolve to whichever architecture is being built.
// `arch::mod` selects the module; nothing outside `arch` names an ISA.
pub use arch::target::{boot, gdt, interrupts, trap};
// PC-class platform assembly for the current profile: ACPI tables, PCI ECAM,
// and ACPI power control. An AArch64 profile supplies device-tree equivalents
// instead, so these are not part of the architecture-neutral surface.
#[cfg(target_arch = "x86_64")]
pub use arch::x86_64::limine;
pub use drivers::{device_discovery, frame_buffer, input, serial};
#[cfg(target_arch = "x86_64")]
pub use drivers::{dma, hardware_inventory, nvme, virtio_blk};
#[cfg(target_arch = "x86_64")]
pub use platform::{acpi, pci};
pub use protocol::{block_proto, capability_transfer_proto, generation_proto, store_proto};
pub use runtime::{bootstrap, component, generation, generation_manager, generation_service};
pub use storage::{
    block_device, block_service, gpt, object_store, recovery, store_service, transfer,
};
pub use support::{crc32, crt, sha256};

use core::panic::PanicInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum QemuExitCode {
    Success = 0x10,
    Failed = 0x11,
    TestFailed = 0x12,
}

pub fn exit_qemu(exit_code: QemuExitCode) {
    arch::cpu::debug_exit(exit_code as u32);
}

pub fn hlt_loop() -> ! {
    loop {
        arch::cpu::wait_for_interrupt();
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
            $crate::gdt::init();
            $crate::interrupts::init();
            $crate::memory::init();
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
