//! Stage-0 to kernel handoff ABI. The `#[repr(C)]` structs are the ABI
//! surface; `contracts/handoff/v1/schema.zt` pins their layout, and the
//! assertions at the bottom of this file fail the build if the Rust layout
//! ever drifts from the generated contract constants.

include!("generated/handoff.rs");

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HandoffMemoryEntry {
    pub base: u64,
    pub length: u64,
    pub kind: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HandoffFramebuffer {
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
    pub reserved: [u8; 5],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct KernelHandoffV1 {
    pub magic: u64,
    pub version: u32,
    pub size: u32,
    pub direct_map_offset: u64,
    pub memory_map_ptr: *const HandoffMemoryEntry,
    pub memory_map_len: u32,
    pub reserved0: u32,
    pub framebuffer: HandoffFramebuffer,
    pub rsdp_address: u64,
    pub generation_ptr: *const u8,
    pub generation_len: u64,
    pub generation_identity: [u8; 32],
    pub bootstate_sequence: u64,
    pub known_good_identity: [u8; 32],
    pub pending_identity: [u8; 32],
    pub remaining_attempts: u32,
    pub bootstate_slot: u8,
    pub running_pending: u8,
    pub reserved1: [u8; 2],
    pub generation_root: [u8; 32],
    pub state_root: [u8; 32],
    pub accepted_release_sequence: u64,
    pub running_release_sequence: u64,
}

// Build-time proof that the Rust `#[repr(C)]` layout matches the
// schema-generated offsets and sizes exactly.
const _: () = {
    use core::mem::{offset_of, size_of};

    assert!(size_of::<HandoffMemoryEntry>() == HANDOFF_MEMORY_ENTRY_BYTES);
    assert!(offset_of!(HandoffMemoryEntry, base) == HANDOFF_MEMORY_ENTRY_BASE_OFFSET);
    assert!(offset_of!(HandoffMemoryEntry, length) == HANDOFF_MEMORY_ENTRY_LENGTH_OFFSET);
    assert!(offset_of!(HandoffMemoryEntry, kind) == HANDOFF_MEMORY_ENTRY_KIND_OFFSET);

    assert!(size_of::<HandoffFramebuffer>() == HANDOFF_FRAMEBUFFER_BYTES);
    assert!(offset_of!(HandoffFramebuffer, address) == HANDOFF_FRAMEBUFFER_ADDRESS_OFFSET);
    assert!(offset_of!(HandoffFramebuffer, width) == HANDOFF_FRAMEBUFFER_WIDTH_OFFSET);
    assert!(offset_of!(HandoffFramebuffer, height) == HANDOFF_FRAMEBUFFER_HEIGHT_OFFSET);
    assert!(offset_of!(HandoffFramebuffer, pitch) == HANDOFF_FRAMEBUFFER_PITCH_OFFSET);
    assert!(offset_of!(HandoffFramebuffer, bpp) == HANDOFF_FRAMEBUFFER_BPP_OFFSET);
    assert!(
        offset_of!(HandoffFramebuffer, memory_model) == HANDOFF_FRAMEBUFFER_MEMORY_MODEL_OFFSET
    );
    assert!(
        offset_of!(HandoffFramebuffer, red_mask_size) == HANDOFF_FRAMEBUFFER_RED_MASK_SIZE_OFFSET
    );
    assert!(
        offset_of!(HandoffFramebuffer, red_mask_shift) == HANDOFF_FRAMEBUFFER_RED_MASK_SHIFT_OFFSET
    );
    assert!(
        offset_of!(HandoffFramebuffer, green_mask_size)
            == HANDOFF_FRAMEBUFFER_GREEN_MASK_SIZE_OFFSET
    );
    assert!(
        offset_of!(HandoffFramebuffer, green_mask_shift)
            == HANDOFF_FRAMEBUFFER_GREEN_MASK_SHIFT_OFFSET
    );
    assert!(
        offset_of!(HandoffFramebuffer, blue_mask_size) == HANDOFF_FRAMEBUFFER_BLUE_MASK_SIZE_OFFSET
    );
    assert!(
        offset_of!(HandoffFramebuffer, blue_mask_shift)
            == HANDOFF_FRAMEBUFFER_BLUE_MASK_SHIFT_OFFSET
    );
    assert!(offset_of!(HandoffFramebuffer, reserved) == HANDOFF_FRAMEBUFFER_RESERVED_OFFSET);

    assert!(size_of::<KernelHandoffV1>() == HANDOFF_BYTES);
    assert!(offset_of!(KernelHandoffV1, magic) == HANDOFF_MAGIC_OFFSET);
    assert!(offset_of!(KernelHandoffV1, version) == HANDOFF_VERSION_OFFSET);
    assert!(offset_of!(KernelHandoffV1, size) == HANDOFF_SIZE_OFFSET);
    assert!(offset_of!(KernelHandoffV1, direct_map_offset) == HANDOFF_DIRECT_MAP_OFFSET_OFFSET);
    assert!(offset_of!(KernelHandoffV1, memory_map_ptr) == HANDOFF_MEMORY_MAP_PTR_OFFSET);
    assert!(offset_of!(KernelHandoffV1, memory_map_len) == HANDOFF_MEMORY_MAP_LEN_OFFSET);
    assert!(offset_of!(KernelHandoffV1, reserved0) == HANDOFF_RESERVED0_OFFSET);
    assert!(offset_of!(KernelHandoffV1, framebuffer) == HANDOFF_FRAMEBUFFER_OFFSET);
    assert!(offset_of!(KernelHandoffV1, rsdp_address) == HANDOFF_RSDP_ADDRESS_OFFSET);
    assert!(offset_of!(KernelHandoffV1, generation_ptr) == HANDOFF_GENERATION_PTR_OFFSET);
    assert!(offset_of!(KernelHandoffV1, generation_len) == HANDOFF_GENERATION_LEN_OFFSET);
    assert!(offset_of!(KernelHandoffV1, generation_identity) == HANDOFF_GENERATION_IDENTITY_OFFSET);
    assert!(offset_of!(KernelHandoffV1, bootstate_sequence) == HANDOFF_BOOTSTATE_SEQUENCE_OFFSET);
    assert!(offset_of!(KernelHandoffV1, known_good_identity) == HANDOFF_KNOWN_GOOD_IDENTITY_OFFSET);
    assert!(offset_of!(KernelHandoffV1, pending_identity) == HANDOFF_PENDING_IDENTITY_OFFSET);
    assert!(offset_of!(KernelHandoffV1, remaining_attempts) == HANDOFF_REMAINING_ATTEMPTS_OFFSET);
    assert!(offset_of!(KernelHandoffV1, bootstate_slot) == HANDOFF_BOOTSTATE_SLOT_OFFSET);
    assert!(offset_of!(KernelHandoffV1, running_pending) == HANDOFF_RUNNING_PENDING_OFFSET);
    assert!(offset_of!(KernelHandoffV1, reserved1) == HANDOFF_RESERVED1_OFFSET);
    assert!(offset_of!(KernelHandoffV1, generation_root) == HANDOFF_GENERATION_ROOT_OFFSET);
    assert!(offset_of!(KernelHandoffV1, state_root) == HANDOFF_STATE_ROOT_OFFSET);
    assert!(
        offset_of!(KernelHandoffV1, accepted_release_sequence)
            == HANDOFF_ACCEPTED_RELEASE_SEQUENCE_OFFSET
    );
    assert!(
        offset_of!(KernelHandoffV1, running_release_sequence)
            == HANDOFF_RUNNING_RELEASE_SEQUENCE_OFFSET
    );
};
