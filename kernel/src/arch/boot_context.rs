//! Boot-context shapes shared by every architecture.
//!
//! These are decoded forms of the architecture-neutral stage-0 handoff
//! contract, not ISA mechanisms: both architectures read the same handoff bytes
//! and must produce the same framebuffer description, memory map, and BootState
//! view. Each `arch::<target>::boot` fills them from its own firmware entry
//! path and re-exports them.

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
