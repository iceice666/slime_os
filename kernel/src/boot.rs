use core::slice;

use boot_contracts::handoff::{
    HANDOFF_MAGIC, HANDOFF_VERSION, HandoffFramebuffer, HandoffMemoryEntry, KernelHandoffV1,
    MEMORY_RESERVED, MEMORY_USABLE,
};
use spin::Once;

#[derive(Debug, Clone, Copy)]
pub struct Framebuffer {
    pub address: u64,
    pub width: u64,
    pub height: u64,
    pub pitch: u64,
    pub bpp: u16,
    pub memory_model: u8,
    pub red_mask_size: u8,
    pub red_mask_shift: u8,
    pub green_mask_size: u8,
    pub green_mask_shift: u8,
    pub blue_mask_size: u8,
    pub blue_mask_shift: u8,
}

#[derive(Clone, Copy)]
pub struct MemoryEntry {
    pub base: u64,
    pub length: u64,
    pub kind: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootStateContext {
    pub sequence: u64,
    pub known_good: [u8; 32],
    pub pending: Option<[u8; 32]>,
    pub remaining_attempts: u32,
    pub slot: u8,
    pub running_pending: bool,
    pub accepted_release_sequence: u64,
    pub running_release_sequence: u64,
    pub generation_root: [u8; 32],
    pub state_root: [u8; 32],
}

struct BootContext {
    direct_map_offset: u64,
    memory_map: &'static [MemoryEntry],
    framebuffer: Framebuffer,
    rsdp_address: u64,
    generation: &'static [u8],
    generation_identity: [u8; 32],
    bootstate: Option<BootStateContext>,
    recovery_index: &'static [u8],
}

static CONTEXT: Once<BootContext> = Once::new();
static mut TEST_MEMORY_MAP: [MemoryEntry; 512] = [MemoryEntry {
    base: 0,
    length: 0,
    kind: MEMORY_RESERVED,
}; 512];
fn find_recovery_index(generation: &'static [u8]) -> &'static [u8] {
    let Ok(decoded) = boot_contracts::generation::Generation::decode(generation) else {
        return &[];
    };
    for index in 0..decoded.object_count() {
        if let Ok(object) = decoded.object(index)
            && object.kind == boot_contracts::generation::KIND_RESOURCE
            && object.id == "recovery-index"
        {
            return object.bytes;
        }
    }
    &[]
}

/// Initialize the kernel boot context from the immutable stage-0 handoff.
///
/// # Safety
///
/// `handoff` must point to a valid, stage-0-owned [`KernelHandoffV1`]. Its
/// referenced physical ranges must remain live and mapped through the declared
/// direct-map offset for the lifetime of the kernel.
pub unsafe fn init_from_handoff(handoff: *const KernelHandoffV1) {
    assert!(!handoff.is_null(), "missing kernel handoff");
    let handoff = unsafe { &*handoff };
    assert_eq!(handoff.magic, HANDOFF_MAGIC, "bad handoff magic");
    assert_eq!(handoff.version, HANDOFF_VERSION, "bad handoff version");
    assert_eq!(
        handoff.size as usize,
        core::mem::size_of::<KernelHandoffV1>(),
        "bad handoff size"
    );
    assert!(
        (handoff.memory_map_len as usize) <= 512,
        "handoff memory map too large"
    );
    let memory_map_ptr = handoff
        .direct_map_offset
        .checked_add(handoff.memory_map_ptr as u64)
        .expect("handoff memory map address overflow")
        as *const HandoffMemoryEntry;
    let memory_map = unsafe {
        slice::from_raw_parts(
            memory_map_ptr.cast::<MemoryEntry>(),
            handoff.memory_map_len as usize,
        )
    };
    let mut framebuffer = framebuffer_from_handoff(handoff.framebuffer);
    framebuffer.address = handoff
        .direct_map_offset
        .checked_add(framebuffer.address)
        .expect("handoff framebuffer address overflow");
    let generation_ptr = handoff
        .direct_map_offset
        .checked_add(handoff.generation_ptr as u64)
        .expect("handoff generation address overflow") as *const u8;
    let generation =
        unsafe { slice::from_raw_parts(generation_ptr, handoff.generation_len as usize) };
    let recovery_index = find_recovery_index(generation);
    let pending = (handoff.pending_identity != [0; 32]).then_some(handoff.pending_identity);
    let context = BootContext {
        direct_map_offset: handoff.direct_map_offset,
        memory_map,
        framebuffer,
        rsdp_address: handoff.rsdp_address,
        generation,
        generation_identity: handoff.generation_identity,
        recovery_index,
        bootstate: Some(BootStateContext {
            sequence: handoff.bootstate_sequence,
            known_good: handoff.known_good_identity,
            pending,
            remaining_attempts: handoff.remaining_attempts,
            slot: handoff.bootstate_slot,
            running_pending: handoff.running_pending != 0,
            accepted_release_sequence: handoff.accepted_release_sequence,
            running_release_sequence: handoff.running_release_sequence,
            generation_root: handoff.generation_root,
            state_root: handoff.state_root,
        }),
    };
    CONTEXT.call_once(move || context);
}

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
    assert!(entries.len() <= 512, "limine memory map too large");
    let target = unsafe { &mut TEST_MEMORY_MAP[..entries.len()] };
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
    let memory_map = unsafe { &TEST_MEMORY_MAP[..entries.len()] };
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
    let recovery_index = find_recovery_index(generation);
    CONTEXT.call_once(|| BootContext {
        direct_map_offset: hhdm,
        memory_map,
        framebuffer,
        rsdp_address,
        generation,
        generation_identity,
        recovery_index,
        bootstate: None,
    });
}

pub fn direct_map_offset() -> u64 {
    context().direct_map_offset
}
pub fn memory_map() -> &'static [MemoryEntry] {
    context().memory_map
}
pub fn framebuffer() -> Framebuffer {
    context().framebuffer
}
pub fn rsdp_address() -> u64 {
    context().rsdp_address
}
pub fn generation() -> &'static [u8] {
    context().generation
}
pub fn generation_identity() -> [u8; 32] {
    context().generation_identity
}

fn context() -> &'static BootContext {
    CONTEXT.get().expect("boot context not initialized")
}

fn framebuffer_from_handoff(fb: HandoffFramebuffer) -> Framebuffer {
    Framebuffer {
        address: fb.address,
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
    }
}

pub fn recovery_index() -> &'static [u8] {
    context().recovery_index
}

pub fn bootstate() -> Option<BootStateContext> {
    context().bootstate
}
