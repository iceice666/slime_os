#![no_std]
#![no_main]
// Suppress compiler complain about "can't find crate for `test`"
// We will build our own test process
#![feature(custom_test_frameworks)]
#![test_runner(crate::test_runner)]
#[cfg(test)]
pub fn test_runner(_: &[&dyn Fn()]) {}
/////////////////////////////////////////////
mod frame_buffer;
mod serial;

mod run;
#[cfg(feature = "kernel_test")]
mod testing;

use run::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum QemuExitCode {
    Success = 0x10,
    Failed = 0x11,
}

pub fn exit_qemu(exit_code: QemuExitCode) {
    use x86_64::instructions::port::Port;

    unsafe {
        let mut port = Port::new(0xf4);
        port.write(exit_code as u32);
    }
}

bootloader_api::entry_point!(kernel_main);
