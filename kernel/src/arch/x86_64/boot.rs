//! x86-64 firmware handoff and boot context.
//!
//! Decoding the stage-0 handoff is architecture-neutral and lives in
//! [`crate::arch::boot_context`], re-exported here so `crate::boot::…` resolves
//! for whichever architecture is built. What this module adds is the x86-only
//! entry path: the Limine boot protocol the Cargo test harness uses instead of
//! stage-0.

use boot_contracts::handoff::{MAX_MEMORY_ENTRIES, MEMORY_RESERVED, MEMORY_USABLE};

use crate::arch::boot_context::publish_bootloader_context;
pub use crate::arch::boot_context::{
    BootStateContext, Framebuffer, MemoryEntry, bootstate, direct_map_offset, framebuffer,
    generation, generation_identity, init_from_handoff, memory_map, recovery_index, rsdp_address,
};

/// Initialize the kernel boot context from Limine responses for test boots.
///
/// # Safety
///
/// Limine must have transferred control with all requested responses populated,
/// and this function must run once before memory management reclaims boot data.
pub unsafe fn init_from_limine() {
    let hhdm = crate::limine::HHDM
        .response()
        .expect("limine: no HHDM response")
        .offset;
    let entries = crate::limine::MEMMAP
        .response()
        .expect("limine: no memory map")
        .entries();
    assert!(
        entries.len() <= MAX_MEMORY_ENTRIES,
        "limine memory map too large"
    );
    // SAFETY: single-threaded boot; this runs once before any other reader.
    let target = unsafe { &mut crate::arch::boot_context::TEST_MEMORY_MAP[..entries.len()] };
    for (dst, src) in target.iter_mut().zip(entries) {
        *dst = MemoryEntry {
            base: src.base,
            length: src.length,
            kind: if src.type_ == limine::memmap::MEMMAP_USABLE {
                MEMORY_USABLE
            } else {
                MEMORY_RESERVED
            },
        };
    }
    let memory_map = unsafe { &crate::arch::boot_context::TEST_MEMORY_MAP[..entries.len()] };
    let fb = crate::limine::FRAMEBUFFER
        .response()
        .expect("limine: no framebuffer")
        .framebuffers()
        .first()
        .copied()
        .expect("limine: empty framebuffer");
    let framebuffer = Framebuffer {
        address: fb.address() as u64,
        width: fb.width,
        height: fb.height,
        pitch: fb.pitch,
        bpp: fb.bpp,
        memory_model: fb.memory_model,
        red_mask_size: fb.red_mask_size,
        red_mask_shift: fb.red_mask_shift,
        green_mask_size: fb.green_mask_size,
        green_mask_shift: fb.green_mask_shift,
        blue_mask_size: fb.blue_mask_size,
        blue_mask_shift: fb.blue_mask_shift,
    };
    let rsdp_address = crate::limine::RSDP.response().map_or(0, |response| {
        let address = response.address as u64;
        let base_revision = if crate::limine::BASE_REVISION.is_supported() {
            limine::BaseRevision::MAX_SUPPORTED
        } else {
            crate::limine::BASE_REVISION.actual_revision().unwrap_or(0)
        };
        if base_revision == 3 {
            address
        } else {
            address.wrapping_sub(hhdm)
        }
    });
    let generation = crate::limine::generation_module_optional().unwrap_or(&[]);
    let generation_identity = if generation.is_empty() {
        [0; 32]
    } else {
        boot_contracts::generation::generation_identity(generation)
    };
    publish_bootloader_context(
        hhdm,
        memory_map,
        framebuffer,
        rsdp_address,
        generation,
        generation_identity,
    );
}
