//! The architecture-specific half of the component syscall ABI.
//!
//! Each module supplies exactly the trap instruction and the register mapping
//! for its architecture, behind one signature. Syscall numbers, argument
//! meanings, error values, and bounds stay in [`crate::syscall`], which is what
//! keeps the semantic contract identical across architectures.

#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(target_arch = "x86_64")]
mod x86_64;

#[cfg(target_arch = "aarch64")]
pub use aarch64::{raw_syscall, raw_syscall_pair};
#[cfg(target_arch = "x86_64")]
pub use x86_64::{raw_syscall, raw_syscall_pair};
