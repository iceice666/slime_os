pub const HANDOFF_MAGIC: u64 = u64::from_le_bytes(*b"SLIMEHND");
pub const HANDOFF_VERSION: u32 = 1;
pub const MEMORY_USABLE: u64 = 0;
pub const MEMORY_RESERVED: u64 = 1;
pub const MAX_MEMORY_ENTRIES: usize = 512;

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
}
