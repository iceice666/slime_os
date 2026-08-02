//! Bus-neutral device discovery.
//!
//! The trusted boot graph and block service ask "which block devices exist?"
//! rather than "walk PCI ECAM": the answer is a list of
//! [`PciFunctionInfo`](crate::capability::PciFunctionInfo) capability
//! descriptors, which is already a neutral capability type. How that list is
//! produced is platform mechanism — PCI ECAM on the PC-class profile, device
//! tree on `aarch64-qemu-virt` and `aarch64-rpi5` (P2/P4).
//!
//! A target with no admitted discovery mechanism returns an empty list, which
//! every caller already handles as "no such device" — the same outcome as a
//! machine that genuinely has none. It never fabricates a device.

use alloc::vec::Vec;

use crate::capability::PciFunctionInfo;

/// Every device function this platform admits, or an empty list when discovery
/// is unavailable or finds nothing.
pub fn functions() -> Vec<PciFunctionInfo> {
    #[cfg(target_arch = "x86_64")]
    {
        crate::pci::enumerate().unwrap_or_default()
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        // P2 supplies device-tree discovery for the AArch64 profiles.
        Vec::new()
    }
}
