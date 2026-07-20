pub const MAGIC: [u8; 8] = *b"SLIMEKRN";
pub const FORMAT_VERSION: u32 = 1;
pub const KERNEL_ABI_VERSION: u32 = 2;
pub const HEADER_LEN: usize = 64;
pub const SEGMENT_LEN: usize = 40;
pub const RELOCATION_LEN: usize = 16;
pub const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_SEGMENTS: usize = 8;
pub const MAX_RELOCATIONS: usize = 16_384;
pub const LOAD_BASE: u64 = 0xffff_ffff_9000_0000;
pub const PREFERRED_BASE: u64 = 0xffff_ffff_8000_0000;
pub const SEGMENT_WRITE: u32 = 1;
pub const SEGMENT_EXEC: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageError {
    Truncated,
    BadMagic,
    UnsupportedVersion,
    BadHeader,
    BadAbi,
    BadBounds,
    BadSegment,
    BadEntry,
    BadRelocation,
    UnknownFlags,
}

#[derive(Debug, Clone, Copy)]
pub struct Segment<'a> {
    pub vaddr_offset: u64,
    pub mem_len: u64,
    pub flags: u32,
    pub bytes: &'a [u8],
}

#[derive(Debug, Clone, Copy)]
pub struct Relocation {
    pub target_offset: u64,
    pub addend: i64,
}

pub struct KernelImage<'a> {
    pub preferred_base: u64,
    pub entry_offset: u64,
    bytes: &'a [u8],
    segment_count: usize,
    relocation_count: usize,
}

impl<'a> KernelImage<'a> {
    pub fn decode(bytes: &'a [u8]) -> Result<Self, ImageError> {
        if bytes.len() < HEADER_LEN {
            return Err(ImageError::Truncated);
        }
        if bytes[..8] != MAGIC {
            return Err(ImageError::BadMagic);
        }
        if u32_at(bytes, 8)? != FORMAT_VERSION {
            return Err(ImageError::UnsupportedVersion);
        }
        if u32_at(bytes, 12)? as usize != HEADER_LEN {
            return Err(ImageError::BadHeader);
        }
        if u32_at(bytes, 16)? != KERNEL_ABI_VERSION {
            return Err(ImageError::BadAbi);
        }
        if u32_at(bytes, 20)? != 0 {
            return Err(ImageError::UnknownFlags);
        }
        let preferred_base = u64_at(bytes, 24)?;
        let entry_offset = u64_at(bytes, 32)?;
        let segment_count = u32_at(bytes, 40)? as usize;
        let relocation_count = u32_at(bytes, 44)? as usize;
        let payload_offset = u64_at(bytes, 48)? as usize;
        let image_len = u64_at(bytes, 56)? as usize;
        if preferred_base != PREFERRED_BASE
            || image_len != bytes.len()
            || bytes.len() as u64 > MAX_IMAGE_BYTES
        {
            return Err(ImageError::BadBounds);
        }
        if !(1..=MAX_SEGMENTS).contains(&segment_count) || relocation_count > MAX_RELOCATIONS {
            return Err(ImageError::BadBounds);
        }
        let segment_start = HEADER_LEN;
        let relocation_start = segment_start
            .checked_add(
                segment_count
                    .checked_mul(SEGMENT_LEN)
                    .ok_or(ImageError::BadBounds)?,
            )
            .ok_or(ImageError::BadBounds)?;
        let tables_end = relocation_start
            .checked_add(
                relocation_count
                    .checked_mul(RELOCATION_LEN)
                    .ok_or(ImageError::BadBounds)?,
            )
            .ok_or(ImageError::BadBounds)?;
        if payload_offset != tables_end || payload_offset > bytes.len() {
            return Err(ImageError::BadBounds);
        }
        let image = Self {
            preferred_base,
            entry_offset,
            bytes,
            segment_count,
            relocation_count,
        };
        let mut previous_end = 0u64;
        let mut entry_ok = false;
        for index in 0..segment_count {
            let segment = image.segment(index)?;
            if segment.vaddr_offset % 4096 != 0
                || segment.mem_len == 0
                || segment.vaddr_offset < previous_end
            {
                return Err(ImageError::BadSegment);
            }
            if segment.flags & !(SEGMENT_WRITE | SEGMENT_EXEC) != 0
                || segment.flags == SEGMENT_WRITE | SEGMENT_EXEC
            {
                return Err(ImageError::UnknownFlags);
            }
            previous_end = segment
                .vaddr_offset
                .checked_add(segment.mem_len)
                .ok_or(ImageError::BadSegment)?;
            if previous_end > MAX_IMAGE_BYTES {
                return Err(ImageError::BadBounds);
            }
            if segment.flags & SEGMENT_EXEC != 0
                && segment.vaddr_offset <= entry_offset
                && entry_offset < previous_end
            {
                entry_ok = true;
            }
        }
        if !entry_ok {
            return Err(ImageError::BadEntry);
        }
        for index in 0..relocation_count {
            let relocation = image.relocation(index)?;
            if relocation.target_offset % 8 != 0
                || !image.range_in_writable(relocation.target_offset, 8)
            {
                return Err(ImageError::BadRelocation);
            }
            let addend = relocation.addend as u64;
            let end = preferred_base
                .checked_add(previous_end.next_multiple_of(4096))
                .ok_or(ImageError::BadRelocation)?;
            if addend < preferred_base || addend > end {
                return Err(ImageError::BadRelocation);
            }
        }
        Ok(image)
    }

    pub fn segment_count(&self) -> usize {
        self.segment_count
    }
    pub fn relocation_count(&self) -> usize {
        self.relocation_count
    }

    pub fn segment(&self, index: usize) -> Result<Segment<'a>, ImageError> {
        if index >= self.segment_count {
            return Err(ImageError::BadBounds);
        }
        let offset = HEADER_LEN + index * SEGMENT_LEN;
        let vaddr_offset = u64_at(self.bytes, offset)?;
        let mem_len = u64_at(self.bytes, offset + 8)?;
        let file_offset = u64_at(self.bytes, offset + 16)? as usize;
        let file_len = u64_at(self.bytes, offset + 24)? as usize;
        let flags = u32_at(self.bytes, offset + 32)?;
        if u32_at(self.bytes, offset + 36)? != 0 || file_len as u64 > mem_len {
            return Err(ImageError::BadSegment);
        }
        let start = file_offset;
        let end = start.checked_add(file_len).ok_or(ImageError::BadBounds)?;
        if start < self.payload_offset() || end > self.bytes.len() {
            return Err(ImageError::BadBounds);
        }
        Ok(Segment {
            vaddr_offset,
            mem_len,
            flags,
            bytes: &self.bytes[start..end],
        })
    }

    pub fn relocation(&self, index: usize) -> Result<Relocation, ImageError> {
        if index >= self.relocation_count {
            return Err(ImageError::BadBounds);
        }
        let offset = HEADER_LEN + self.segment_count * SEGMENT_LEN + index * RELOCATION_LEN;
        Ok(Relocation {
            target_offset: u64_at(self.bytes, offset)?,
            addend: i64_at(self.bytes, offset + 8)?,
        })
    }

    fn payload_offset(&self) -> usize {
        HEADER_LEN + self.segment_count * SEGMENT_LEN + self.relocation_count * RELOCATION_LEN
    }

    fn range_in_writable(&self, start: u64, len: u64) -> bool {
        (0..self.segment_count).any(|index| {
            self.segment(index).is_ok_and(|segment| {
                segment.flags & SEGMENT_WRITE != 0
                    && start >= segment.vaddr_offset
                    && start
                        .checked_add(len)
                        .is_some_and(|end| end <= segment.vaddr_offset + segment.mem_len)
            })
        })
    }
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, ImageError> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(ImageError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}
fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, ImageError> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(ImageError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}
fn i64_at(bytes: &[u8], offset: usize) -> Result<i64, ImageError> {
    Ok(i64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(ImageError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}
