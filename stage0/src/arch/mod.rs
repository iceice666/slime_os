//! Stage-0's architecture boundary.
//!
//! Generation selection, BootState handling, release authorization, and the
//! rollback flow are architecture-neutral. Page-table construction, entry-state
//! setup, and the transfer to the kernel are profile-specific and live here.
//!
//! Each module implements the same signatures, so `main.rs` names one flow.

#[cfg(target_arch = "aarch64")]
pub mod aarch64;
#[cfg(target_arch = "x86_64")]
pub mod x86_64;

#[cfg(target_arch = "aarch64")]
pub use aarch64 as target;
#[cfg(target_arch = "x86_64")]
pub use x86_64 as target;
