#![cfg_attr(not(test), no_std)]

pub mod dango_runtime;

#[cfg(feature = "component-runtime")]
pub mod fabric_boot;
#[cfg(feature = "component-runtime")]
pub mod fabric_visibility;
#[cfg(feature = "component-runtime")]
pub mod shared_buffer_probe;
