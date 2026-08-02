//! Storage: partition/object formats, block transport, and the services over
//! them.
//!
//! The protocols, capability checks, on-disk formats, and services here are
//! architecture-neutral and are not re-specified per target. Only
//! [`block_device`]'s transport backends are platform-bound today (PCI virtio
//! and NVMe), so on a target without them the service reports
//! `BlockError::DeviceNotFound` rather than the syscall disappearing — the
//! syscall table stays identical across architectures.

pub mod block_device;
pub mod block_service;
pub mod gpt;
pub mod object_store;
pub mod recovery;
pub mod store_service;
pub mod transfer;
