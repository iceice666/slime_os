//! Component image decoding and validation (`contracts/component/v2`).
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
//! Layout (all little-endian, generated from `contracts/component/v2/schema.zt`):
//!
//! ```text
//! Header (56 bytes):
//!   u64 magic          = IMAGE_MAGIC ("SLIMECM2")
//!   u32 format_version = FORMAT_VERSION
//!   u32 header_size    = HEADER_LEN
//!   u32 kernel_abi     = KERNEL_ABI_VERSION
//!   u32 architecture
//!   u32 abi
//!   u32 page_profile
//!   u32 entry_offset   (relative to the component base VA; must land in an
//!                       executable segment)
//!   u16 segment_count  (1..=MAX_SEGMENTS)
//!   u16 reserved       = 0
//!   u32 stack_bytes    (page multiple, 1..=MAX_STACK_BYTES)
//!   u32 target_profile
//!   u64 required_features
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
//!
//! Retained v1 images use `SLIMECMP` and a 32-byte header. Their target was
//! implicit in the sole producer and therefore means exactly
//! `x86_64-qemu-virtio`, never an architecture-neutral executable.

use slime_proto::component as generated;

use alloc::vec::Vec;
use boot_contracts::target_profile::{ImageTarget, TargetError, TargetProfile};

pub use generated::{
    DEFAULT_STACK_BYTES, FORMAT_VERSION, HEADER_LEN, IMAGE_MAGIC, IMAGE_MAGIC_BYTES,
    KERNEL_ABI_VERSION, LEGACY_FORMAT_VERSION, LEGACY_HEADER_LEN, LEGACY_IMAGE_MAGIC,
    LEGACY_IMAGE_MAGIC_BYTES, MAX_IMAGE_BYTES, MAX_SEGMENTS, MAX_STACK_BYTES, SEGMENT_FLAG_EXEC,
    SEGMENT_FLAG_WRITE, SEGMENT_LEN, WireImageHeader, WireSegmentRecord,
};

/// Why an image was rejected. Validation is total: every malformed input maps
/// to exactly one of these, never a panic or an out-of-bounds access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageError {
    /// Fewer bytes than the header or segment table requires.
    Truncated,
    BadMagic,
    /// The magic names a supported revision but its version does not match.
    UnsupportedVersion,
    /// The header length does not match the selected revision.
    BadHeader,
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
    /// The image's declared target does not match the admitted profile.
    Target(TargetError),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Revision {
    Current(ImageTarget),
    Legacy,
}

/// A validated component image, borrowed against the generation object bytes.
pub struct Image<'a> {
    pub entry_offset: u32,
    pub stack_bytes: u32,
    pub segments: Vec<Segment>,
    revision: Revision,
    data: &'a [u8],
}

impl<'a> Image<'a> {
    /// File bytes backing `segment` (shorter than `mem_len` when the segment
    /// carries zero-filled `.bss`).
    pub fn segment_bytes(&self, segment: &Segment) -> &'a [u8] {
        let start = segment.file_offset as usize;
        &self.data[start..start + segment.file_len as usize]
    }

    pub fn target(&self) -> ImageTarget {
        match self.revision {
            Revision::Current(target) => target,
            Revision::Legacy => TargetProfile::legacy_image_target(),
        }
    }
}

/// Decode and fully validate an image. Bounded in every dimension: at most
/// `MAX_SEGMENTS` records, `MAX_IMAGE_BYTES` of mapped footprint, and file
/// ranges proven inside `blob` before any byte is exposed.
pub fn decode(blob: &[u8]) -> Result<Image<'_>, ImageError> {
    if blob.len() < LEGACY_HEADER_LEN {
        return Err(ImageError::Truncated);
    }
    let magic = u64_at(blob, generated::OFF_HEADER_MAGIC)?;
    let version = u32_at(blob, generated::OFF_HEADER_FORMAT_VERSION)?;
    let revision = match (magic, version) {
        (IMAGE_MAGIC, FORMAT_VERSION) => Revision::Current(ImageTarget {
            profile: u32_at(blob, generated::OFF_HEADER_TARGET_PROFILE)?,
            architecture: u32_at(blob, generated::OFF_HEADER_ARCHITECTURE)?,
            abi: u32_at(blob, generated::OFF_HEADER_ABI)?,
            page_profile: u32_at(blob, generated::OFF_HEADER_PAGE_PROFILE)?,
            required_features: u64_at(blob, generated::OFF_HEADER_REQUIRED_FEATURES)?,
        }),
        (LEGACY_IMAGE_MAGIC, LEGACY_FORMAT_VERSION) => Revision::Legacy,
        (IMAGE_MAGIC | LEGACY_IMAGE_MAGIC, _) => return Err(ImageError::UnsupportedVersion),
        _ => return Err(ImageError::BadMagic),
    };
    let header_len = match revision {
        Revision::Current(_) => HEADER_LEN,
        Revision::Legacy => LEGACY_HEADER_LEN,
    };
    if blob.len() < header_len {
        return Err(ImageError::Truncated);
    }
    if u32_at(blob, generated::OFF_HEADER_HEADER_SIZE)? as usize != header_len {
        return Err(ImageError::BadHeader);
    }
    if u32_at(blob, generated::OFF_HEADER_KERNEL_ABI)? != KERNEL_ABI_VERSION {
        return Err(ImageError::AbiMismatch);
    }
    let (entry_offset, count, stack_bytes) = match revision {
        Revision::Current(_) => {
            let header = WireImageHeader::decode(blob).ok_or(ImageError::Truncated)?;
            if header.reserved != 0 {
                return Err(ImageError::BadFlags);
            }
            (
                header.entry_offset,
                header.segment_count,
                header.stack_bytes,
            )
        }
        Revision::Legacy => {
            if u16_at(blob, generated::OFF_LEGACY_HEADER_RESERVED)? != 0 {
                return Err(ImageError::BadFlags);
            }
            (
                u32_at(blob, generated::OFF_LEGACY_HEADER_ENTRY_OFFSET)?,
                u16_at(blob, generated::OFF_LEGACY_HEADER_SEGMENT_COUNT)?,
                u32_at(blob, generated::OFF_LEGACY_HEADER_STACK_BYTES)?,
            )
        }
    };
    if count == 0 || count > MAX_SEGMENTS {
        return Err(ImageError::BadSegmentCount);
    }
    if stack_bytes == 0
        || stack_bytes % crate::memory::PAGE_SIZE as u32 != 0
        || stack_bytes > MAX_STACK_BYTES
    {
        return Err(ImageError::BadStack);
    }
    let records_end = header_len
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
        let record = WireSegmentRecord::decode(&blob[header_len + index * SEGMENT_LEN..])
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
            && u64::from(entry_offset) >= start
            && u64::from(entry_offset) < end
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
        entry_offset,
        stack_bytes,
        segments,
        revision,
        data,
    })
}

pub fn decode_for_profile<'a>(
    blob: &'a [u8],
    profile: &TargetProfile,
) -> Result<Image<'a>, ImageError> {
    let image = decode(blob)?;
    profile.admit(&image.target()).map_err(ImageError::Target)?;
    Ok(image)
}

fn u16_at(blob: &[u8], offset: usize) -> Result<u16, ImageError> {
    let bytes = blob.get(offset..offset + 2).ok_or(ImageError::Truncated)?;
    Ok(u16::from_le_bytes(
        bytes.try_into().map_err(|_| ImageError::Truncated)?,
    ))
}

fn u32_at(blob: &[u8], offset: usize) -> Result<u32, ImageError> {
    let bytes = blob.get(offset..offset + 4).ok_or(ImageError::Truncated)?;
    Ok(u32::from_le_bytes(
        bytes.try_into().map_err(|_| ImageError::Truncated)?,
    ))
}

fn u64_at(blob: &[u8], offset: usize) -> Result<u64, ImageError> {
    let bytes = blob.get(offset..offset + 8).ok_or(ImageError::Truncated)?;
    Ok(u64::from_le_bytes(
        bytes.try_into().map_err(|_| ImageError::Truncated)?,
    ))
}
