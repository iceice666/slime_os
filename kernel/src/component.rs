//! Component image decoding and validation (`contracts/component/v1`).
//!
//! A component image is the only executable encoding the kernel accepts.
//! Images are produced on the host from a statically linked ELF intermediate
//! and carried as generation objects of kind `bootstrap` or `component`; the
//! generation decoder validates every image eagerly, so a generation that
//! decodes never contains a malformed executable.
//!
//! The format is deliberately structural only: integrity comes from the
//! generation object digest and authority from generation grants. An image
//! declares how to map it (entry point, per-segment offsets and R/W/X flags,
//! stack size) and nothing else — there are no relocations, no dynamic
//! linking metadata, and no capability declarations.
//!
//! Layout (all little-endian, generated from `contracts/component/v1/schema.zt`):
//!
//! ```text
//! Header (32 bytes):
//!   u64 magic         = IMAGE_MAGIC ("SLIMECMP")
//!   u32 format_version = FORMAT_VERSION
//!   u32 header_size    = HEADER_LEN
//!   u32 kernel_abi     = KERNEL_ABI_VERSION
//!   u32 entry_offset   (relative to the component base VA; must land in an
//!                       executable segment)
//!   u16 segment_count  (1..=MAX_SEGMENTS)
//!   u16 reserved
//!   u32 stack_bytes    (page multiple, 1..=MAX_STACK_BYTES)
//!
//! Segment record (20 bytes), sorted by strictly increasing vaddr_offset with
//! non-overlapping memory ranges:
//!   u32 vaddr_offset   (page-aligned, relative to the component base VA)
//!   u32 mem_len        (> 0, >= file_len; the tail beyond file_len zero-fills)
//!   u32 file_offset    (relative to the start of the image data region)
//!   u32 file_len
//!   u16 flags          (SEGMENT_FLAG_WRITE | SEGMENT_FLAG_EXEC; W and X are
//!                       never both set)
//!   u16 reserved
//! ```

#[path = "component/gen.rs"]
mod generated;

use alloc::vec::Vec;

pub use generated::{
    DEFAULT_STACK_BYTES, FORMAT_VERSION, HEADER_LEN, IMAGE_MAGIC, IMAGE_MAGIC_BYTES,
    KERNEL_ABI_VERSION, MAX_IMAGE_BYTES, MAX_SEGMENTS, MAX_STACK_BYTES, SEGMENT_FLAG_EXEC,
    SEGMENT_FLAG_WRITE, SEGMENT_LEN, WireImageHeader, WireSegmentRecord,
};

/// Why an image was rejected. Validation is total: every malformed input maps
/// to exactly one of these, never a panic or an out-of-bounds access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageError {
    /// Fewer bytes than the header or segment table requires.
    Truncated,
    BadMagic,
    /// `format_version`/`header_size` does not name this contract version.
    UnsupportedVersion,
    /// The image was built against a different syscall ABI than this kernel.
    AbiMismatch,
    /// Zero segments or more than `MAX_SEGMENTS`.
    BadSegmentCount,
    /// Unknown flag bits set, or write and execute combined on one segment.
    BadFlags,
    /// A segment is page-misaligned, empty, has `file_len > mem_len`, or its
    /// memory range is not strictly above the previous segment's.
    BadSegment,
    /// A segment's file range escapes the image data region.
    BadFileRange,
    /// The entry point does not land inside an executable segment.
    BadEntry,
    /// The summed page footprint exceeds `MAX_IMAGE_BYTES`.
    ImageTooLarge,
    /// Stack size is zero, not a page multiple, or above `MAX_STACK_BYTES`.
    BadStack,
}

/// One validated load segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub vaddr_offset: u32,
    pub mem_len: u32,
    pub file_offset: u32,
    pub file_len: u32,
    pub flags: u16,
}

impl Segment {
    pub fn writable(&self) -> bool {
        self.flags & SEGMENT_FLAG_WRITE != 0
    }

    pub fn executable(&self) -> bool {
        self.flags & SEGMENT_FLAG_EXEC != 0
    }
}

/// A validated component image, borrowed against the generation object bytes.
pub struct Image<'a> {
    pub entry_offset: u32,
    pub stack_bytes: u32,
    pub segments: Vec<Segment>,
    data: &'a [u8],
}

impl<'a> Image<'a> {
    /// File bytes backing `segment` (shorter than `mem_len` when the segment
    /// carries zero-filled `.bss`).
    pub fn segment_bytes(&self, segment: &Segment) -> &'a [u8] {
        let start = segment.file_offset as usize;
        &self.data[start..start + segment.file_len as usize]
    }
}

/// Decode and fully validate an image. Bounded in every dimension: at most
/// `MAX_SEGMENTS` records, `MAX_IMAGE_BYTES` of mapped footprint, and file
/// ranges proven inside `blob` before any byte is exposed.
pub fn decode(blob: &[u8]) -> Result<Image<'_>, ImageError> {
    let header = WireImageHeader::decode(blob).ok_or(ImageError::Truncated)?;
    if header.magic != IMAGE_MAGIC {
        return Err(ImageError::BadMagic);
    }
    if header.format_version != FORMAT_VERSION || header.header_size as usize != HEADER_LEN {
        return Err(ImageError::UnsupportedVersion);
    }
    if header.kernel_abi != KERNEL_ABI_VERSION {
        return Err(ImageError::AbiMismatch);
    }
    let count = header.segment_count;
    if count == 0 || count > MAX_SEGMENTS {
        return Err(ImageError::BadSegmentCount);
    }
    if header.stack_bytes == 0
        || header.stack_bytes % crate::memory::PAGE_SIZE as u32 != 0
        || header.stack_bytes > MAX_STACK_BYTES
    {
        return Err(ImageError::BadStack);
    }
    let records_end = HEADER_LEN
        .checked_add(count as usize * SEGMENT_LEN)
        .ok_or(ImageError::Truncated)?;
    if records_end > blob.len() {
        return Err(ImageError::Truncated);
    }
    let data = &blob[records_end..];

    let mut segments = Vec::with_capacity(count as usize);
    let mut previous_end: u64 = 0;
    let mut total_pages: u64 = 0;
    let mut entry_ok = false;
    for index in 0..count as usize {
        let record = WireSegmentRecord::decode(&blob[HEADER_LEN + index * SEGMENT_LEN..])
            .ok_or(ImageError::Truncated)?;
        let flags = record.flags;
        if flags & !(SEGMENT_FLAG_WRITE | SEGMENT_FLAG_EXEC) != 0
            || flags & (SEGMENT_FLAG_WRITE | SEGMENT_FLAG_EXEC)
                == (SEGMENT_FLAG_WRITE | SEGMENT_FLAG_EXEC)
        {
            return Err(ImageError::BadFlags);
        }
        if record.vaddr_offset % crate::memory::PAGE_SIZE as u32 != 0
            || record.mem_len == 0
            || record.file_len > record.mem_len
        {
            return Err(ImageError::BadSegment);
        }
        let start = u64::from(record.vaddr_offset);
        let end = start + u64::from(record.mem_len);
        if start < previous_end {
            return Err(ImageError::BadSegment);
        }
        previous_end = end;
        let file_end = (record.file_offset as usize)
            .checked_add(record.file_len as usize)
            .ok_or(ImageError::BadFileRange)?;
        if file_end > data.len() {
            return Err(ImageError::BadFileRange);
        }
        total_pages += u64::from(record.mem_len).div_ceil(crate::memory::PAGE_SIZE as u64);
        if total_pages * crate::memory::PAGE_SIZE as u64 > MAX_IMAGE_BYTES {
            return Err(ImageError::ImageTooLarge);
        }
        if record.flags & SEGMENT_FLAG_EXEC != 0
            && u64::from(header.entry_offset) >= start
            && u64::from(header.entry_offset) < end
        {
            entry_ok = true;
        }
        segments.push(Segment {
            vaddr_offset: record.vaddr_offset,
            mem_len: record.mem_len,
            file_offset: record.file_offset,
            file_len: record.file_len,
            flags: record.flags,
        });
    }
    if !entry_ok {
        return Err(ImageError::BadEntry);
    }

    Ok(Image {
        entry_offset: header.entry_offset,
        stack_bytes: header.stack_bytes,
        segments,
        data,
    })
}
