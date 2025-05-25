#[cfg(not(feature = "kernel_test"))]
mod main;
#[cfg(not(feature = "kernel_test"))]
pub use main::kernel_main;

#[cfg(feature = "kernel_test")]
mod test;
#[cfg(feature = "kernel_test")]
pub use test::{KERNEL_TESTS, TestResult, kernel_main};
