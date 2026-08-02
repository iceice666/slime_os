//! AArch64 firmware handoff and boot context.
//!
//! Decoding the stage-0 handoff is architecture-neutral and lives in
//! [`crate::arch::boot_context`]; this module re-exports it so `crate::boot::…`
//! resolves for whichever architecture is built.
//!
//! There is no second entry path here. x86 additionally supports a Limine boot
//! for the Cargo test harness; AArch64 boots only through the verified stage-0
//! loader, so the re-export is the whole module.
//!
//! The `rsdp_address` field carries whatever platform description the firmware
//! passed. On this profile that is the flattened device tree rather than an
//! ACPI RSDP; the field is named for its x86 origin and P2.5 renames it when
//! device-tree discovery lands.

pub use crate::arch::boot_context::{
    BootStateContext, Framebuffer, MemoryEntry, bootstate, direct_map_offset, framebuffer,
    generation, generation_identity, init_from_handoff, memory_map, recovery_index, rsdp_address,
};
