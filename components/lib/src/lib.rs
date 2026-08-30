#![cfg_attr(not(test), no_std)]

#[cfg(feature = "component-runtime")]
pub mod block_io;
#[cfg(feature = "component-runtime")]
pub mod fabric_boot;
#[cfg(feature = "component-runtime")]
pub mod fabric_matrix;
#[cfg(feature = "component-runtime")]
pub mod fabric_self_view;
#[cfg(feature = "component-runtime")]
pub mod fabric_visibility;
#[cfg(feature = "component-runtime")]
pub mod generation_composition;
#[cfg(feature = "component-runtime")]
pub mod shared_buffer_probe;
pub mod virtio_mmio;
