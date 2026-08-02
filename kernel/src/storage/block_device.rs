//! Common block-service backend selection.
//!
//! Clients see one capability-gated block protocol. Transport identity remains
//! inside the trusted service: deterministic QEMU prefers virtio, while a
//! Framework NVMe controller is admitted only through the read-only backend.
//!
//! [`BlockError`] and the operations on [`BlockDevice`] are
//! architecture-neutral — the block protocol, its error model, and the services
//! above it are the same on every target. Only the transports are
//! platform-bound: on a target with no admitted transport, every operation
//! reports [`BlockError::DeviceNotFound`] instead of the type disappearing, so
//! the syscall surface does not vary by architecture.

use crate::capability::PciFunctionInfo;
#[cfg(target_arch = "x86_64")]
use crate::nvme::{NvmeBlock, NvmeError};
#[cfg(target_arch = "x86_64")]
use crate::virtio_blk::{VirtioBlkError, VirtioBlock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    DeviceNotFound,
    OutOfRange,
    BufferSize,
    Timeout,
    InjectedTimeout,
    ReadOnly,
    Device,
    InjectedFailure,
}

impl BlockError {
    pub fn requires_reinitialize(self) -> bool {
        matches!(self, Self::Timeout | Self::Device)
    }
}

#[cfg(target_arch = "x86_64")]
impl From<VirtioBlkError> for BlockError {
    fn from(value: VirtioBlkError) -> Self {
        match value {
            VirtioBlkError::DeviceNotFound => Self::DeviceNotFound,
            VirtioBlkError::OutOfRange => Self::OutOfRange,
            VirtioBlkError::BufferSize => Self::BufferSize,
            VirtioBlkError::Timeout | VirtioBlkError::ResetTimeout => Self::Timeout,
            VirtioBlkError::InjectedFailure => Self::InjectedFailure,
            _ => Self::Device,
        }
    }
}

#[cfg(target_arch = "x86_64")]
impl From<NvmeError> for BlockError {
    fn from(value: NvmeError) -> Self {
        match value {
            NvmeError::DeviceNotFound => Self::DeviceNotFound,
            NvmeError::OutOfRange => Self::OutOfRange,
            NvmeError::BufferSize => Self::BufferSize,
            NvmeError::Timeout => Self::Timeout,
            NvmeError::ReadOnly => Self::ReadOnly,
            _ => Self::Device,
        }
    }
}

#[cfg(target_arch = "x86_64")]
pub enum BlockDevice {
    Virtio(VirtioBlock),
    Nvme(NvmeBlock),
}

#[cfg(target_arch = "x86_64")]
impl BlockDevice {
    pub fn find_and_init() -> Result<(PciFunctionInfo, Self), BlockError> {
        let functions = crate::device_discovery::functions();
        let function = functions
            .iter()
            .find(|function| {
                function.vendor_id == 0x1af4 && matches!(function.device_id, 0x1001 | 0x1042)
            })
            .or_else(|| {
                functions
                    .iter()
                    .find(|function| function.class_code & 0x00ff_ffff == 0x010802)
            })
            .copied()
            .ok_or(BlockError::DeviceNotFound)?;
        Self::init(function).map(|device| (function, device))
    }

    pub fn init(function: PciFunctionInfo) -> Result<Self, BlockError> {
        if function.vendor_id == 0x1af4 && matches!(function.device_id, 0x1001 | 0x1042) {
            return VirtioBlock::init(function)
                .map(Self::Virtio)
                .map_err(Into::into);
        }
        if function.class_code & 0x00ff_ffff == 0x010802 {
            return NvmeBlock::init(function)
                .map(Self::Nvme)
                .map_err(Into::into);
        }
        Err(BlockError::DeviceNotFound)
    }

    pub fn capacity_sectors(&self) -> u64 {
        match self {
            Self::Virtio(device) => device.capacity_sectors(),
            Self::Nvme(device) => device.capacity_sectors(),
        }
    }

    pub fn read_sector(&mut self, lba: u64, output: &mut [u8]) -> Result<(), BlockError> {
        match self {
            Self::Virtio(device) => device.read_sector(lba, output).map_err(Into::into),
            Self::Nvme(device) => device.read_sector(lba, output).map_err(Into::into),
        }
    }

    pub fn write_sector(&mut self, lba: u64, input: &[u8]) -> Result<(), BlockError> {
        match self {
            Self::Virtio(device) => device.write_sector(lba, input).map_err(Into::into),
            Self::Nvme(device) => device.write_sector(lba, input).map_err(Into::into),
        }
    }

    pub fn flush(&mut self) -> Result<(), BlockError> {
        match self {
            Self::Virtio(device) => device.flush().map_err(Into::into),
            Self::Nvme(device) => device.flush().map_err(Into::into),
        }
    }

    pub fn inject_failure(&mut self) -> Result<(), BlockError> {
        match self {
            Self::Virtio(device) => device.inject_failure().map_err(Into::into),
            Self::Nvme(_) => Err(BlockError::Device),
        }
    }

    pub fn inject_timeout(&mut self) -> Result<(), BlockError> {
        match self {
            Self::Virtio(device) => device.inject_timeout().map_err(|error| match error {
                VirtioBlkError::Timeout => BlockError::InjectedTimeout,
                other => other.into(),
            }),
            Self::Nvme(_) => Err(BlockError::Timeout),
        }
    }

    pub fn inject_reset(&mut self) -> Result<(), BlockError> {
        match self {
            Self::Virtio(device) => device.inject_reset().map_err(Into::into),
            Self::Nvme(device) => {
                device.reset()?;
                Err(BlockError::Device)
            }
        }
    }

    pub fn inject_flush_failure(&mut self) -> Result<(), BlockError> {
        match self {
            Self::Virtio(device) => device.inject_flush_failure().map_err(Into::into),
            Self::Nvme(_) => Err(BlockError::ReadOnly),
        }
    }

    pub fn inject_interrupted_write(&mut self, lba: u64, input: &[u8]) -> Result<(), BlockError> {
        match self {
            Self::Virtio(device) => device
                .inject_interrupted_write(lba, input)
                .map_err(Into::into),
            Self::Nvme(_) => Err(BlockError::ReadOnly),
        }
    }
}

/// The admitted block transports for this target profile.
///
/// No block transport is admitted yet on this architecture: P2 brings up
/// virtio-mmio for `aarch64-qemu-virt` and P4 qualifies the Raspberry Pi 5
/// storage path. Until then every operation reports
/// [`BlockError::DeviceNotFound`], which the services above already handle as
/// an ordinary absent-device outcome.
#[cfg(not(target_arch = "x86_64"))]
pub enum BlockDevice {}

#[cfg(not(target_arch = "x86_64"))]
impl BlockDevice {
    pub fn find_and_init() -> Result<(PciFunctionInfo, Self), BlockError> {
        Err(BlockError::DeviceNotFound)
    }

    pub fn init(_function: PciFunctionInfo) -> Result<Self, BlockError> {
        Err(BlockError::DeviceNotFound)
    }

    pub fn capacity_sectors(&self) -> u64 {
        match *self {}
    }

    pub fn read_sector(&mut self, _lba: u64, _output: &mut [u8]) -> Result<(), BlockError> {
        match *self {}
    }

    pub fn write_sector(&mut self, _lba: u64, _input: &[u8]) -> Result<(), BlockError> {
        match *self {}
    }

    pub fn flush(&mut self) -> Result<(), BlockError> {
        match *self {}
    }

    pub fn inject_failure(&mut self) -> Result<(), BlockError> {
        match *self {}
    }

    pub fn inject_timeout(&mut self) -> Result<(), BlockError> {
        match *self {}
    }

    pub fn inject_reset(&mut self) -> Result<(), BlockError> {
        match *self {}
    }

    pub fn inject_flush_failure(&mut self) -> Result<(), BlockError> {
        match *self {}
    }

    pub fn inject_interrupted_write(&mut self, _lba: u64, _input: &[u8]) -> Result<(), BlockError> {
        match *self {}
    }
}
