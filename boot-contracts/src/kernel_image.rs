//! Kernel image decoding and target admission (`contracts/kernel-image/v2`).
//!
//! `decode` validates both the current and retained wire revisions without
//! choosing a machine profile. In particular, the linked preferred base is
//! structural image data rather than a global constant. `decode_for_profile`
//! owns the exact base cross-check because the selected profile is the source
//! of truth for the admitted load layout.
//!
//! Retained v1 images carry the implicit qualification of the sole producer:
//! `x86_64-qemu-virtio`. They are never treated as architecture-neutral.

use crate::target_profile::{ImageTarget, TargetError, TargetProfile};

pub const MAGIC: [u8; 8] = *b"SLIMEKR2";
pub const LEGACY_MAGIC: [u8; 8] = *b"SLIMEKRN";
include!("generated/kernel_image.rs");

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
    Target(TargetError),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Revision {
    Current,
    Legacy,
}

pub struct KernelImage<'a> {
    pub preferred_base: u64,
    pub entry_offset: u64,
    target: ImageTarget,
    header_len: usize,
    bytes: &'a [u8],
    segment_count: usize,
    relocation_count: usize,
}

impl<'a> KernelImage<'a> {
    pub fn decode(bytes: &'a [u8]) -> Result<Self, ImageError> {
        if bytes.len() < LEGACY_HEADER_LEN {
            return Err(ImageError::Truncated);
        }
        let revision = match (
            bytes
                .get(OFF_HEADER_MAGIC..OFF_HEADER_MAGIC + MAGIC.len())
                .ok_or(ImageError::Truncated)?,
            u32_at(bytes, OFF_HEADER_FORMAT_VERSION)?,
        ) {
            (magic, FORMAT_VERSION) if magic == MAGIC => Revision::Current,
            (magic, LEGACY_FORMAT_VERSION) if magic == LEGACY_MAGIC => Revision::Legacy,
            (magic, _) if magic == MAGIC || magic == LEGACY_MAGIC => {
                return Err(ImageError::UnsupportedVersion);
            }
            _ => return Err(ImageError::BadMagic),
        };
        let header_len = match revision {
            Revision::Current => HEADER_LEN,
            Revision::Legacy => LEGACY_HEADER_LEN,
        };
        if bytes.len() < header_len {
            return Err(ImageError::Truncated);
        }
        if u32_at(bytes, OFF_HEADER_HEADER_SIZE)? as usize != header_len {
            return Err(ImageError::BadHeader);
        }
        if u32_at(bytes, OFF_HEADER_KERNEL_ABI)? != KERNEL_ABI_VERSION {
            return Err(ImageError::BadAbi);
        }
        if u32_at(bytes, OFF_HEADER_REQUIRED_FLAGS)? != 0 {
            return Err(ImageError::UnknownFlags);
        }
        let (
            target,
            preferred_base,
            entry_offset,
            segment_count,
            relocation_count,
            payload_offset,
            image_len,
        ) = match revision {
            Revision::Current => (
                ImageTarget {
                    profile: u32_at(bytes, OFF_HEADER_TARGET_PROFILE)?,
                    architecture: u32_at(bytes, OFF_HEADER_ARCHITECTURE)?,
                    abi: u32_at(bytes, OFF_HEADER_ABI)?,
                    page_profile: u32_at(bytes, OFF_HEADER_PAGE_PROFILE)?,
                    required_features: u64_at(bytes, OFF_HEADER_REQUIRED_FEATURES)?,
                },
                u64_at(bytes, OFF_HEADER_PREFERRED_BASE)?,
                u64_at(bytes, OFF_HEADER_ENTRY_OFFSET)?,
                u32_at(bytes, OFF_HEADER_SEGMENT_COUNT)? as usize,
                u32_at(bytes, OFF_HEADER_RELOCATION_COUNT)? as usize,
                u64_at(bytes, OFF_HEADER_PAYLOAD_OFFSET)? as usize,
                u64_at(bytes, OFF_HEADER_TOTAL_LEN)? as usize,
            ),
            Revision::Legacy => (
                TargetProfile::legacy_image_target(),
                u64_at(bytes, OFF_LEGACY_HEADER_PREFERRED_BASE)?,
                u64_at(bytes, OFF_LEGACY_HEADER_ENTRY_OFFSET)?,
                u32_at(bytes, OFF_LEGACY_HEADER_SEGMENT_COUNT)? as usize,
                u32_at(bytes, OFF_LEGACY_HEADER_RELOCATION_COUNT)? as usize,
                u64_at(bytes, OFF_LEGACY_HEADER_PAYLOAD_OFFSET)? as usize,
                u64_at(bytes, OFF_LEGACY_HEADER_TOTAL_LEN)? as usize,
            ),
        };
        if image_len != bytes.len() || bytes.len() as u64 > MAX_IMAGE_BYTES {
            return Err(ImageError::BadBounds);
        }
        if !(1..=MAX_SEGMENTS).contains(&segment_count) || relocation_count > MAX_RELOCATIONS {
            return Err(ImageError::BadBounds);
        }
        let segment_start = header_len;
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
            target,
            header_len,
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

    pub fn decode_for_profile(
        bytes: &'a [u8],
        profile: &TargetProfile,
    ) -> Result<Self, ImageError> {
        let image = Self::decode(bytes)?;
        profile.admit(&image.target()).map_err(ImageError::Target)?;
        if image.preferred_base != profile.kernel_preferred_base {
            return Err(ImageError::Target(TargetError::LoadLayoutMismatch));
        }
        Ok(image)
    }

    pub fn target(&self) -> ImageTarget {
        self.target
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
        let offset = self.header_len + index * SEGMENT_LEN;
        let vaddr_offset = u64_at(self.bytes, offset + OFF_SEGMENT_VADDR_OFFSET)?;
        let mem_len = u64_at(self.bytes, offset + OFF_SEGMENT_MEM_LEN)?;
        let file_offset = u64_at(self.bytes, offset + OFF_SEGMENT_FILE_OFFSET)? as usize;
        let file_len = u64_at(self.bytes, offset + OFF_SEGMENT_FILE_LEN)? as usize;
        let flags = u32_at(self.bytes, offset + OFF_SEGMENT_FLAGS)?;
        if u32_at(self.bytes, offset + OFF_SEGMENT_RESERVED)? != 0 || file_len as u64 > mem_len {
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
        let offset = self.header_len + self.segment_count * SEGMENT_LEN + index * RELOCATION_LEN;
        Ok(Relocation {
            target_offset: u64_at(self.bytes, offset + OFF_RELOCATION_TARGET_OFFSET)?,
            addend: i64_at(self.bytes, offset + OFF_RELOCATION_ADDEND)?,
        })
    }

    fn payload_offset(&self) -> usize {
        self.header_len + self.segment_count * SEGMENT_LEN + self.relocation_count * RELOCATION_LEN
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

#[cfg(test)]
mod tests {
    extern crate alloc;

    use super::*;
    use crate::target_profile::{ABI_SLIME_AARCH64_V1, ARCH_AARCH64, PAGE_PROFILE_AARCH64_4K};

    #[derive(Debug, Clone, Copy)]
    struct FixtureTarget {
        profile: u32,
        architecture: u32,
        abi: u32,
        page_profile: u32,
        required_features: u64,
    }

    fn aarch64_target() -> FixtureTarget {
        let profile = TargetProfile::by_name("aarch64-rpi5").expect("declared profile");
        FixtureTarget {
            profile: profile.id,
            architecture: ARCH_AARCH64,
            abi: ABI_SLIME_AARCH64_V1,
            page_profile: PAGE_PROFILE_AARCH64_4K,
            required_features: profile.required_features,
        }
    }

    fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn build_v2(target: FixtureTarget) -> alloc::vec::Vec<u8> {
        let payload_offset = HEADER_LEN + SEGMENT_LEN;
        let total_len = payload_offset + 8;
        let mut bytes = alloc::vec![0u8; total_len];
        bytes[OFF_HEADER_MAGIC..OFF_HEADER_MAGIC + MAGIC.len()].copy_from_slice(&MAGIC);
        put_u32(&mut bytes, OFF_HEADER_FORMAT_VERSION, FORMAT_VERSION);
        put_u32(&mut bytes, OFF_HEADER_HEADER_SIZE, HEADER_LEN as u32);
        put_u32(&mut bytes, OFF_HEADER_KERNEL_ABI, KERNEL_ABI_VERSION);
        put_u32(&mut bytes, OFF_HEADER_ARCHITECTURE, target.architecture);
        put_u32(&mut bytes, OFF_HEADER_ABI, target.abi);
        put_u32(&mut bytes, OFF_HEADER_PAGE_PROFILE, target.page_profile);
        put_u32(&mut bytes, OFF_HEADER_TARGET_PROFILE, target.profile);
        put_u64(
            &mut bytes,
            OFF_HEADER_REQUIRED_FEATURES,
            target.required_features,
        );
        put_u64(&mut bytes, OFF_HEADER_PREFERRED_BASE, PREFERRED_BASE);
        put_u64(&mut bytes, OFF_HEADER_ENTRY_OFFSET, 0);
        put_u32(&mut bytes, OFF_HEADER_SEGMENT_COUNT, 1);
        put_u32(&mut bytes, OFF_HEADER_RELOCATION_COUNT, 0);
        put_u64(&mut bytes, OFF_HEADER_PAYLOAD_OFFSET, payload_offset as u64);
        put_u64(&mut bytes, OFF_HEADER_TOTAL_LEN, total_len as u64);
        let segment = HEADER_LEN;
        put_u64(&mut bytes, segment + OFF_SEGMENT_VADDR_OFFSET, 0);
        put_u64(&mut bytes, segment + OFF_SEGMENT_MEM_LEN, 8);
        put_u64(
            &mut bytes,
            segment + OFF_SEGMENT_FILE_OFFSET,
            payload_offset as u64,
        );
        put_u64(&mut bytes, segment + OFF_SEGMENT_FILE_LEN, 8);
        put_u32(&mut bytes, segment + OFF_SEGMENT_FLAGS, SEGMENT_EXEC);
        bytes[payload_offset..].copy_from_slice(&[0x90; 8]);
        bytes
    }

    fn build_v1() -> alloc::vec::Vec<u8> {
        let payload_offset = LEGACY_HEADER_LEN + SEGMENT_LEN;
        let total_len = payload_offset + 8;
        let mut bytes = alloc::vec![0u8; total_len];
        bytes[OFF_HEADER_MAGIC..OFF_HEADER_MAGIC + LEGACY_MAGIC.len()]
            .copy_from_slice(&LEGACY_MAGIC);
        put_u32(&mut bytes, OFF_HEADER_FORMAT_VERSION, LEGACY_FORMAT_VERSION);
        put_u32(&mut bytes, OFF_HEADER_HEADER_SIZE, LEGACY_HEADER_LEN as u32);
        put_u32(&mut bytes, OFF_HEADER_KERNEL_ABI, KERNEL_ABI_VERSION);
        put_u64(&mut bytes, OFF_LEGACY_HEADER_PREFERRED_BASE, PREFERRED_BASE);
        put_u64(&mut bytes, OFF_LEGACY_HEADER_ENTRY_OFFSET, 0);
        put_u32(&mut bytes, OFF_LEGACY_HEADER_SEGMENT_COUNT, 1);
        put_u32(&mut bytes, OFF_LEGACY_HEADER_RELOCATION_COUNT, 0);
        put_u64(
            &mut bytes,
            OFF_LEGACY_HEADER_PAYLOAD_OFFSET,
            payload_offset as u64,
        );
        put_u64(&mut bytes, OFF_LEGACY_HEADER_TOTAL_LEN, total_len as u64);
        let segment = LEGACY_HEADER_LEN;
        put_u64(&mut bytes, segment + OFF_SEGMENT_VADDR_OFFSET, 0);
        put_u64(&mut bytes, segment + OFF_SEGMENT_MEM_LEN, 8);
        put_u64(
            &mut bytes,
            segment + OFF_SEGMENT_FILE_OFFSET,
            payload_offset as u64,
        );
        put_u64(&mut bytes, segment + OFF_SEGMENT_FILE_LEN, 8);
        put_u32(&mut bytes, segment + OFF_SEGMENT_FLAGS, SEGMENT_EXEC);
        bytes[payload_offset..].copy_from_slice(&[0x90; 8]);
        bytes
    }

    #[test]
    fn v2_image_round_trips_and_reports_declared_target() {
        let target = aarch64_target();
        let bytes = build_v2(target);
        let image = KernelImage::decode(&bytes).expect("v2 image decodes");
        assert_eq!(image.segment_count(), 1);
        assert_eq!(image.segment(0).expect("segment").bytes, &[0x90; 8]);
        assert_eq!(
            image.target(),
            ImageTarget {
                profile: target.profile,
                architecture: target.architecture,
                abi: target.abi,
                page_profile: target.page_profile,
                required_features: target.required_features,
            }
        );
    }

    #[test]
    fn retained_v1_image_reports_legacy_x86_target() {
        let bytes = build_v1();
        let image = KernelImage::decode(&bytes).expect("v1 image decodes");
        assert_eq!(image.target(), TargetProfile::legacy_image_target());
    }

    #[test]
    fn decode_for_profile_admits_matching_profile() {
        let profile = TargetProfile::by_name("aarch64-rpi5").expect("declared profile");
        let bytes = build_v2(aarch64_target());
        assert!(KernelImage::decode_for_profile(&bytes, profile).is_ok());
    }

    #[test]
    fn decode_for_profile_reports_same_architecture_profile_mismatch() {
        let profile = TargetProfile::by_name("aarch64-rpi5").expect("declared profile");
        let mut target = aarch64_target();
        target.profile = crate::target_profile::PROFILE_AARCH64_QEMU_VIRT;
        let bytes = build_v2(target);
        assert!(matches!(
            KernelImage::decode_for_profile(&bytes, profile),
            Err(ImageError::Target(TargetError::ProfileMismatch))
        ));
    }

    #[test]
    fn decode_for_profile_reports_architecture_mismatch() {
        let profile = TargetProfile::by_name("aarch64-rpi5").expect("declared profile");
        let mut target = aarch64_target();
        target.architecture = crate::target_profile::ARCH_X86_64;
        let bytes = build_v2(target);
        assert!(matches!(
            KernelImage::decode_for_profile(&bytes, profile),
            Err(ImageError::Target(TargetError::ArchitectureMismatch))
        ));
    }

    #[test]
    fn decode_for_profile_reports_abi_mismatch() {
        let profile = TargetProfile::by_name("aarch64-rpi5").expect("declared profile");
        let mut target = aarch64_target();
        target.abi = crate::target_profile::ABI_SLIME_X86_64_V1;
        let bytes = build_v2(target);
        assert!(matches!(
            KernelImage::decode_for_profile(&bytes, profile),
            Err(ImageError::Target(TargetError::AbiMismatch))
        ));
    }

    #[test]
    fn decode_for_profile_reports_page_profile_mismatch() {
        let profile = TargetProfile::by_name("aarch64-rpi5").expect("declared profile");
        let mut target = aarch64_target();
        target.page_profile = crate::target_profile::PAGE_PROFILE_X86_64_4K;
        let bytes = build_v2(target);
        assert!(matches!(
            KernelImage::decode_for_profile(&bytes, profile),
            Err(ImageError::Target(TargetError::PageProfileMismatch))
        ));
    }

    #[test]
    fn decode_for_profile_reports_feature_mismatch() {
        let profile = TargetProfile::by_name("aarch64-rpi5").expect("declared profile");
        let mut target = aarch64_target();
        target.required_features = 0;
        let bytes = build_v2(target);
        assert!(matches!(
            KernelImage::decode_for_profile(&bytes, profile),
            Err(ImageError::Target(TargetError::FeatureMismatch))
        ));
    }

    #[test]
    fn retained_v1_image_is_not_admitted_for_aarch64() {
        let profile = TargetProfile::by_name("aarch64-rpi5").expect("declared profile");
        let bytes = build_v1();
        assert!(matches!(
            KernelImage::decode_for_profile(&bytes, profile),
            Err(ImageError::Target(TargetError::ProfileMismatch))
        ));
    }

    #[test]
    fn decode_for_profile_rejects_wrong_preferred_base() {
        let profile = TargetProfile::by_name("aarch64-rpi5").expect("declared profile");
        let mut bytes = build_v2(aarch64_target());
        put_u64(
            &mut bytes,
            OFF_HEADER_PREFERRED_BASE,
            profile.kernel_preferred_base + 0x1000,
        );
        assert!(matches!(
            KernelImage::decode_for_profile(&bytes, profile),
            Err(ImageError::Target(TargetError::LoadLayoutMismatch))
        ));
    }

    #[test]
    fn unknown_magic_fails_closed() {
        let mut bytes = build_v2(aarch64_target());
        bytes[OFF_HEADER_MAGIC] ^= 0xff;
        assert!(matches!(
            KernelImage::decode(&bytes),
            Err(ImageError::BadMagic)
        ));
    }

    #[test]
    fn current_magic_with_legacy_version_is_unsupported() {
        let mut bytes = build_v2(aarch64_target());
        put_u32(&mut bytes, OFF_HEADER_FORMAT_VERSION, LEGACY_FORMAT_VERSION);
        assert!(matches!(
            KernelImage::decode(&bytes),
            Err(ImageError::UnsupportedVersion)
        ));
    }

    #[test]
    fn legacy_magic_with_current_version_is_unsupported() {
        let mut bytes = build_v1();
        put_u32(&mut bytes, OFF_HEADER_FORMAT_VERSION, FORMAT_VERSION);
        assert!(matches!(
            KernelImage::decode(&bytes),
            Err(ImageError::UnsupportedVersion)
        ));
    }

    #[test]
    fn v2_unknown_target_profile_fails_closed() {
        let profile = TargetProfile::by_name("aarch64-rpi5").expect("declared profile");
        let mut bytes = build_v2(aarch64_target());
        put_u32(&mut bytes, OFF_HEADER_TARGET_PROFILE, u32::MAX);
        assert!(matches!(
            KernelImage::decode_for_profile(&bytes, profile),
            Err(ImageError::Target(TargetError::ProfileMismatch))
        ));
    }

    #[test]
    fn v2_nonzero_required_flags_fails_closed() {
        let mut bytes = build_v2(aarch64_target());
        put_u32(&mut bytes, OFF_HEADER_REQUIRED_FLAGS, 1);
        assert!(matches!(
            KernelImage::decode(&bytes),
            Err(ImageError::UnknownFlags)
        ));
    }
}
