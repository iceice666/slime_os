//! Device drivers.
//!
//! [`frame_buffer`], [`input`], and [`serial`] are architecture-neutral: they
//! own console and event policy over a transport a platform supplies. The rest
//! are bound to the PC-class platform in [`crate::platform`] — PCI-attached
//! block transports and the inventory report over them — so they are selected
//! by target profile. An AArch64 profile supplies device-tree discovered
//! equivalents in their place (P2/P4).

pub mod device_discovery;
pub mod frame_buffer;
pub mod input;
pub mod serial;

#[cfg(target_arch = "x86_64")]
pub mod dma;
#[cfg(target_arch = "x86_64")]
pub mod hardware_inventory;
#[cfg(target_arch = "x86_64")]
pub mod nvme;
#[cfg(target_arch = "x86_64")]
pub mod virtio_blk;
