//! Common block-service backend selection.
//!
//! Clients see one capability-gated block protocol. Transport identity remains
//! inside the trusted service: deterministic QEMU prefers virtio, while a
//! Framework NVMe controller is admitted only through the read-only backend.

use crate::capability::PciFunctionInfo;
use crate::nvme::{NvmeBlock, NvmeError};
use crate::virtio_blk::{VirtioBlkError, VirtioBlock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    DeviceNotFound,
    OutOfRange,
    BufferSize,
    Timeout,
    ReadOnly,
    Device,
}

impl BlockError {
    pub fn requires_reinitialize(self) -> bool {
        matches!(self, Self::Timeout | Self::Device)
    }
}

impl From<VirtioBlkError> for BlockError {
    fn from(value: VirtioBlkError) -> Self {
        match value {
            VirtioBlkError::DeviceNotFound => Self::DeviceNotFound,
            VirtioBlkError::OutOfRange => Self::OutOfRange,
            VirtioBlkError::BufferSize => Self::BufferSize,
            VirtioBlkError::Timeout | VirtioBlkError::ResetTimeout => Self::Timeout,
            _ => Self::Device,
        }
    }
}

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

pub enum BlockDevice {
    Virtio(VirtioBlock),
    Nvme(NvmeBlock),
}

impl BlockDevice {
    pub fn find_and_init() -> Result<(PciFunctionInfo, Self), BlockError> {
        let functions = crate::pci::enumerate().map_err(|_| BlockError::DeviceNotFound)?;
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
            Self::Virtio(device) => device.inject_timeout().map_err(Into::into),
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
