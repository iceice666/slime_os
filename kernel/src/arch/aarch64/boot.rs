//! AArch64 firmware handoff and the boot context it establishes.
//!
//! The handoff contract itself is architecture-neutral and already decoded by
//! `boot-contracts`; what differs per architecture is the entry state, the
//! direct-map establishment, and how platform device information is
//! discovered — device tree here rather than ACPI. P2 implements those and
//! fills this context.
//!
//! The context types come from `arch::boot_context`: the framebuffer
//! description, memory-map entry, and BootState context are handoff-contract
//! shapes, not ISA mechanisms, so both architectures decode the same bytes into
//! the same structures.

use boot_contracts::handoff::KernelHandoffV1;

pub use crate::arch::boot_context::{BootStateContext, Framebuffer, MemoryEntry};

/// Initialize the kernel boot context from the immutable stage-0 handoff.
///
/// # Safety
///
/// `handoff` must point to a valid, stage-0-owned [`KernelHandoffV1`] whose
/// referenced physical ranges stay live and mapped through the declared
/// direct-map offset for the lifetime of the kernel.
pub unsafe fn init_from_handoff(_handoff: *const KernelHandoffV1) {
    unimplemented!("aarch64 boot context: implemented by P2")
}

pub fn direct_map_offset() -> u64 {
    unimplemented!("aarch64 boot context: implemented by P2")
}

pub fn memory_map() -> &'static [MemoryEntry] {
    unimplemented!("aarch64 boot context: implemented by P2")
}

pub fn framebuffer() -> Framebuffer {
    unimplemented!("aarch64 boot context: implemented by P2")
}

/// Address of the platform description table root. AArch64 uses a device tree
/// rather than an ACPI RSDP; P2 fixes how that is carried.
pub fn rsdp_address() -> u64 {
    unimplemented!("aarch64 platform description: implemented by P2")
}

pub fn generation() -> &'static [u8] {
    unimplemented!("aarch64 boot context: implemented by P2")
}

pub fn generation_identity() -> [u8; 32] {
    unimplemented!("aarch64 boot context: implemented by P2")
}

pub fn recovery_index() -> &'static [u8] {
    unimplemented!("aarch64 boot context: implemented by P2")
}

pub fn bootstate() -> Option<BootStateContext> {
    unimplemented!("aarch64 boot context: implemented by P2")
}
