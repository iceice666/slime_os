//! Stage-0's architecture boundary.
//!
//! Generation selection, BootState handling, release authorization, and the
//! rollback flow are architecture-neutral. Page-table construction, entry-state
//! setup, and the transfer to the kernel are profile-specific and live here.
//!
//! P2 adds an `aarch64` module for the `aarch64-qemu-virt` and `aarch64-rpi5`
//! profiles behind the same signatures.

#[cfg(target_arch = "x86_64")]
pub mod x86_64;

#[cfg(target_arch = "x86_64")]
pub use x86_64 as target;
