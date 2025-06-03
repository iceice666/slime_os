#![no_std]
#![cfg_attr(test, no_main)]
#![feature(custom_test_frameworks)]
#![test_runner(slime_os_kernel::test_runner)]
#![reexport_test_harness_main = "test_main"]

slime_os_kernel::setup_test_entry!();

use slime_os_kernel::serial_println;

#[test_case]
fn test_println() {
    serial_println!("test_println output");
}
