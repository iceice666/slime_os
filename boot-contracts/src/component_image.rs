//! Component-image admission (`contracts/component/v2`).
//!
//! This module owns the byte-level contract shared by stage-0 and `slime-root`:
//! revision and target qualification, canonical header fields, declared stack
//! bounds, native-ELF wrapper shape, and the retained segment-table rules that
//! survive the custom loader's retirement.
//!
//! A retained v1 image is not architecture-neutral. It is an
//! `x86_64-qemu-virtio` image whose target was implied by the only builder that
//! could produce it, and it is reported as exactly that.

use crate::target_profile::{ImageTarget, TargetError, TargetProfile};

pub mod wire {
    include!("generated/component_image.rs");
}

pub use wire::{
    ELF_FORMAT_VERSION, ELF_HEADER_LEN, ELF_IMAGE_MAGIC, FORMAT_VERSION, HEADER_LEN, IMAGE_MAGIC,
    LEGACY_FORMAT_VERSION, LEGACY_HEADER_LEN, LEGACY_IMAGE_MAGIC, MAX_IMAGE_BYTES, MAX_SEGMENTS,
    MAX_STACK_BYTES, SEGMENT_FLAG_EXEC, SEGMENT_FLAG_WRITE, SEGMENT_LEN, WireSegmentRecord,
};

/// Why a component image's target could not be established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentTargetError {
    /// Fewer bytes than the revision's header requires.
    Truncated,
    BadMagic,
    /// The magic is known but paired with a version that revision never used.
    UnsupportedVersion,
    /// The component contract ABI does not match this decoder.
    KernelAbiMismatch,
    /// A reserved field is nonzero, so the writer disagrees with the contract.
    NonZeroReserved,
    /// The declared stack is zero, misaligned, or above the contract ceiling.
    BadStack,
    /// A revision carries fields that must be canonical for its payload shape.
    BadHeaderShape,
    /// A native ELF body is not ELF64 little-endian for the qualified target.
    BadElfIdentity,
    /// A native ELF header or load segment is malformed.
    BadElfShape,
    /// A native ELF body's file bytes or mapped footprint exceed the ceiling.
    ImageTooLarge,
    /// The image is qualified, but not for the profile it was offered to.
    Target(TargetError),
}

/// The revision an image was written at. Retained revisions are decoded with
/// their original meaning, never reinterpreted under current rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Revision {
    /// Pre-qualification images; implicitly `x86_64-qemu-virtio`.
    V1,
    /// Carries architecture, ABI, page profile, and required features.
    V2,
    /// Same qualification header as [`Revision::V2`], but the body is a
    /// complete native ELF executable rather than a segment table. Used by the
    /// seL4 target profile, where a component is an ordinary seL4 task loaded
    /// at its own link addresses.
    Elf,
}

impl Revision {
    /// Whether this revision's payload is a native ELF the caller must load
    /// with an ELF loader, rather than a Slime segment table.
    pub const fn carries_elf(self) -> bool {
        matches!(self, Self::Elf)
    }
}

/// Read the revision and declared target qualification from an image header.
pub fn target(blob: &[u8]) -> Result<(Revision, ImageTarget), ComponentTargetError> {
    if blob.len() < LEGACY_HEADER_LEN {
        return Err(ComponentTargetError::Truncated);
    }
    let magic = u64_at(blob, wire::OFF_HEADER_MAGIC)?;
    let version = u32_at(blob, wire::OFF_HEADER_FORMAT_VERSION)?;
    let header_size = u32_at(blob, wire::OFF_HEADER_HEADER_SIZE)? as usize;
    match (magic, version) {
        // The two qualified revisions share one header layout, so they are read
        // by identical offsets and differ only in which loader owns the body.
        (IMAGE_MAGIC, FORMAT_VERSION) | (ELF_IMAGE_MAGIC, ELF_FORMAT_VERSION) => {
            if header_size != HEADER_LEN || blob.len() < HEADER_LEN {
                return Err(ComponentTargetError::Truncated);
            }
            if u16_at(blob, wire::OFF_HEADER_RESERVED)? != 0 {
                return Err(ComponentTargetError::NonZeroReserved);
            }
            let revision = if magic == ELF_IMAGE_MAGIC {
                Revision::Elf
            } else {
                Revision::V2
            };
            Ok((
                revision,
                ImageTarget {
                    profile: u32_at(blob, wire::OFF_HEADER_TARGET_PROFILE)?,
                    architecture: u32_at(blob, wire::OFF_HEADER_ARCHITECTURE)?,
                    abi: u32_at(blob, wire::OFF_HEADER_ABI)?,
                    page_profile: u32_at(blob, wire::OFF_HEADER_PAGE_PROFILE)?,
                    required_features: u64_at(blob, wire::OFF_HEADER_REQUIRED_FEATURES)?,
                },
            ))
        }
        (LEGACY_IMAGE_MAGIC, LEGACY_FORMAT_VERSION) => {
            if header_size != LEGACY_HEADER_LEN {
                return Err(ComponentTargetError::Truncated);
            }
            if u16_at(blob, wire::OFF_LEGACY_HEADER_RESERVED)? != 0 {
                return Err(ComponentTargetError::NonZeroReserved);
            }
            Ok((Revision::V1, TargetProfile::legacy_image_target()))
        }
        (IMAGE_MAGIC | LEGACY_IMAGE_MAGIC | ELF_IMAGE_MAGIC, _) => {
            Err(ComponentTargetError::UnsupportedVersion)
        }
        _ => Err(ComponentTargetError::BadMagic),
    }
}

/// The native ELF an [`Revision::Elf`] image carries, after its complete wrapper
/// has been admitted for `profile`.
///
/// Admission runs first and its failure is returned unchanged, so malformed or
/// wrong-target bytes never reach a loader.
pub fn admit_elf<'a>(
    blob: &'a [u8],
    profile: &TargetProfile,
) -> Result<&'a [u8], ComponentTargetError> {
    if admit(blob, profile)? != Revision::Elf {
        return Err(ComponentTargetError::UnsupportedVersion);
    }
    let elf = blob
        .get(ELF_HEADER_LEN..)
        .filter(|elf| !elf.is_empty())
        .ok_or(ComponentTargetError::Truncated)?;
    validate_elf(elf, profile)?;
    Ok(elf)
}

/// Admit an image for `profile`, including the canonical fields shared by every
/// consumer of the wrapper.
pub fn admit(blob: &[u8], profile: &TargetProfile) -> Result<Revision, ComponentTargetError> {
    let (revision, declared) = target(blob)?;
    profile
        .admit(&declared)
        .map_err(ComponentTargetError::Target)?;
    validate_header(blob, revision, profile)?;
    Ok(revision)
}

fn validate_header(
    blob: &[u8],
    revision: Revision,
    profile: &TargetProfile,
) -> Result<(), ComponentTargetError> {
    if u32_at(blob, wire::OFF_HEADER_KERNEL_ABI)? != wire::KERNEL_ABI_VERSION {
        return Err(ComponentTargetError::KernelAbiMismatch);
    }
    let stack_offset = if revision == Revision::V1 {
        wire::OFF_LEGACY_HEADER_STACK_BYTES
    } else {
        wire::OFF_HEADER_STACK_BYTES
    };
    let stack_bytes = u32_at(blob, stack_offset)?;
    if stack_bytes == 0
        || u64::from(stack_bytes) % profile.page_bytes != 0
        || stack_bytes > MAX_STACK_BYTES
    {
        return Err(ComponentTargetError::BadStack);
    }
    if revision == Revision::Elf
        && (u32_at(blob, wire::OFF_HEADER_ENTRY_OFFSET)? != 0
            || u16_at(blob, wire::OFF_HEADER_SEGMENT_COUNT)? != 0)
    {
        return Err(ComponentTargetError::BadHeaderShape);
    }
    Ok(())
}

fn validate_elf(elf: &[u8], profile: &TargetProfile) -> Result<(), ComponentTargetError> {
    validate_elf_len(elf.len())?;
    let header = elf.get(..64).ok_or(ComponentTargetError::BadElfShape)?;
    if header[..4] != [0x7f, b'E', b'L', b'F']
        || header[4] != 2
        || header[5] != 1
        || header[6] != 1
        || u16::from_le_bytes([header[18], header[19]]) != profile.elf_machine as u16
    {
        return Err(ComponentTargetError::BadElfIdentity);
    }
    let phoff = usize::try_from(u64::from_le_bytes(header[32..40].try_into().unwrap()))
        .map_err(|_| ComponentTargetError::BadElfShape)?;
    let phentsize = usize::from(u16::from_le_bytes([header[54], header[55]]));
    let phnum = usize::from(u16::from_le_bytes([header[56], header[57]]));
    if phentsize != 56 || phnum == 0 {
        return Err(ComponentTargetError::BadElfShape);
    }
    let mut mapped = 0u64;
    for index in 0..phnum {
        let offset = phoff
            .checked_add(
                index
                    .checked_mul(phentsize)
                    .ok_or(ComponentTargetError::BadElfShape)?,
            )
            .ok_or(ComponentTargetError::BadElfShape)?;
        let program = elf
            .get(offset..offset + 56)
            .ok_or(ComponentTargetError::BadElfShape)?;
        if u32::from_le_bytes(program[..4].try_into().unwrap()) != 1 {
            continue;
        }
        let file_offset = u64::from_le_bytes(program[8..16].try_into().unwrap());
        let file_size = u64::from_le_bytes(program[32..40].try_into().unwrap());
        let mem_size = u64::from_le_bytes(program[40..48].try_into().unwrap());
        if file_size > mem_size
            || file_offset
                .checked_add(file_size)
                .is_none_or(|end| end > elf.len() as u64)
        {
            return Err(ComponentTargetError::BadElfShape);
        }
        let segment_mapped = mem_size
            .div_ceil(profile.page_bytes)
            .checked_mul(profile.page_bytes)
            .ok_or(ComponentTargetError::ImageTooLarge)?;
        mapped = mapped
            .checked_add(segment_mapped)
            .ok_or(ComponentTargetError::ImageTooLarge)?;
        if mapped > MAX_IMAGE_BYTES {
            return Err(ComponentTargetError::ImageTooLarge);
        }
    }
    Ok(())
}

fn validate_elf_len(len: usize) -> Result<(), ComponentTargetError> {
    if u64::try_from(len).map_or(true, |len| len > MAX_IMAGE_BYTES) {
        Err(ComponentTargetError::ImageTooLarge)
    } else {
        Ok(())
    }
}

fn u16_at(bytes: &[u8], offset: usize) -> Result<u16, ComponentTargetError> {
    bytes
        .get(offset..offset + 2)
        .and_then(|slice| slice.try_into().ok())
        .map(u16::from_le_bytes)
        .ok_or(ComponentTargetError::Truncated)
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, ComponentTargetError> {
    bytes
        .get(offset..offset + 4)
        .and_then(|slice| slice.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or(ComponentTargetError::Truncated)
}

fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, ComponentTargetError> {
    bytes
        .get(offset..offset + 8)
        .and_then(|slice| slice.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or(ComponentTargetError::Truncated)
}

/// Why a segment table is inadmissible.
///
/// The vocabulary is the retired kernel's `ImageError`, restated for the
/// subset that is a property of the *image* rather than of the loader: a
/// mapping decision the kernel makes is not here, and neither is anything
/// needing a page allocator. Every variant below is a statement about bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SegmentError {
    /// Zero segments, or more than [`MAX_SEGMENTS`].
    BadSegmentCount,
    /// A flag outside the defined set, or `WRITE | EXEC` together (W^X).
    BadFlags,
    /// A reserved field is nonzero.
    NonZeroReserved,
    /// A misaligned load offset, an empty memory range, `file_len` past
    /// `mem_len`, or ranges out of order or overlapping.
    BadSegment,
    /// A file range extending past the payload.
    BadFileRange,
    /// The entry point lands outside every executable segment.
    BadEntry,
    /// The total mapped footprint exceeds [`MAX_IMAGE_BYTES`].
    ImageTooLarge,
    /// A record shorter than [`SEGMENT_LEN`].
    Truncated,
}

/// Validate a component image's segment table (P5.4.10).
///
/// `records` is the packed segment array, `data` the payload the file ranges
/// index into, `entry_offset` the declared entry, and `page_size` the target
/// profile's page granule.
///
/// # Why this lives in `boot-contracts`
///
/// These rules were only in `kernel/src/runtime/component.rs`, exercised only
/// by `kernel/tests/component_image.rs` — eleven architecture-neutral
/// assertions in a file no Justfile target names, reachable only through
/// `just test`. P5.4.1's inventory recorded them as coverage that would vanish
/// silently when `kernel/` is deleted, with no seL4 equivalent: P5.2 observes
/// the positive path and target mismatch, and nothing exercises the malformed
/// corpus.
///
/// `slime-root` is not the right home either — it has no SLIMECM loader and
/// never will. The rules are a property of the *format*, which is what
/// `boot-contracts` is for, and P0's required check says the corpus must
/// "reject the wrong ... target-specific load layout" regardless of producer.
/// Here they are host-tested and survive the oracle's deletion.
///
/// The retired kernel's `decode` keeps its own copy for now: it is frozen, and
/// rewriting it to call this would edit the oracle. That is P5.4.final's
/// business, not this slice's.
pub fn validate_segments(
    records: &[u8],
    data: &[u8],
    count: u16,
    entry_offset: u32,
    page_size: u32,
) -> Result<(), SegmentError> {
    if page_size == 0 {
        return Err(SegmentError::BadSegment);
    }
    if count == 0 || count > MAX_SEGMENTS {
        return Err(SegmentError::BadSegmentCount);
    }
    let mut previous_end: u64 = 0;
    let mut total_pages: u64 = 0;
    let mut entry_ok = false;
    for index in 0..count as usize {
        let record = records
            .get(index * SEGMENT_LEN..)
            .and_then(WireSegmentRecord::decode)
            .ok_or(SegmentError::Truncated)?;
        if record.reserved != 0 {
            return Err(SegmentError::NonZeroReserved);
        }
        // W^X, and no undefined bits: a segment that is both writable and
        // executable is refused as an image property, before any loader has
        // the chance to map it that way.
        let flags = record.flags;
        if flags & !(SEGMENT_FLAG_WRITE | SEGMENT_FLAG_EXEC) != 0
            || flags & (SEGMENT_FLAG_WRITE | SEGMENT_FLAG_EXEC)
                == (SEGMENT_FLAG_WRITE | SEGMENT_FLAG_EXEC)
        {
            return Err(SegmentError::BadFlags);
        }
        if record.vaddr_offset % page_size != 0
            || record.mem_len == 0
            || record.file_len > record.mem_len
        {
            return Err(SegmentError::BadSegment);
        }
        // Sorted and non-overlapping, checked as one comparison: a table whose
        // ranges are out of order and one whose ranges collide are the same
        // defect seen from two sides.
        let start = u64::from(record.vaddr_offset);
        let end = start + u64::from(record.mem_len);
        if start < previous_end {
            return Err(SegmentError::BadSegment);
        }
        previous_end = end;
        let file_end = (record.file_offset as usize)
            .checked_add(record.file_len as usize)
            .ok_or(SegmentError::BadFileRange)?;
        if file_end > data.len() {
            return Err(SegmentError::BadFileRange);
        }
        total_pages += u64::from(record.mem_len).div_ceil(u64::from(page_size));
        if total_pages * u64::from(page_size) > MAX_IMAGE_BYTES {
            return Err(SegmentError::ImageTooLarge);
        }
        if flags & SEGMENT_FLAG_EXEC != 0
            && u64::from(entry_offset) >= start
            && u64::from(entry_offset) < end
        {
            entry_ok = true;
        }
    }
    // The entry must land inside an executable segment. Not merely inside the
    // image: an entry pointing at data is an image that cannot start.
    if entry_ok {
        Ok(())
    } else {
        Err(SegmentError::BadEntry)
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::*;
    use crate::target_profile::{
        ABI_SLIME_AARCH64_V1, ARCH_AARCH64, ARCH_X86_64, FEATURE_AARCH64_BASELINE,
        FEATURE_AARCH64_GENERIC_TIMER, FEATURE_AARCH64_GICV3, PAGE_PROFILE_AARCH64_4K,
    };

    fn v2_header(profile: &TargetProfile) -> [u8; HEADER_LEN] {
        let mut bytes = [0u8; HEADER_LEN];
        bytes[wire::OFF_HEADER_MAGIC..][..8].copy_from_slice(&IMAGE_MAGIC.to_le_bytes());
        bytes[wire::OFF_HEADER_FORMAT_VERSION..][..4]
            .copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes[wire::OFF_HEADER_HEADER_SIZE..][..4]
            .copy_from_slice(&(HEADER_LEN as u32).to_le_bytes());
        bytes[wire::OFF_HEADER_KERNEL_ABI..][..4]
            .copy_from_slice(&wire::KERNEL_ABI_VERSION.to_le_bytes());
        bytes[wire::OFF_HEADER_ARCHITECTURE..][..4]
            .copy_from_slice(&profile.architecture.to_le_bytes());
        bytes[wire::OFF_HEADER_ABI..][..4].copy_from_slice(&profile.abi.to_le_bytes());
        bytes[wire::OFF_HEADER_PAGE_PROFILE..][..4]
            .copy_from_slice(&profile.page_profile.to_le_bytes());
        bytes[wire::OFF_HEADER_STACK_BYTES..][..4]
            .copy_from_slice(&wire::DEFAULT_STACK_BYTES.to_le_bytes());
        bytes[wire::OFF_HEADER_TARGET_PROFILE..][..4].copy_from_slice(&profile.id.to_le_bytes());
        bytes[wire::OFF_HEADER_REQUIRED_FEATURES..][..8]
            .copy_from_slice(&profile.required_features.to_le_bytes());
        bytes
    }

    fn elf_body(profile: &TargetProfile, mem_size: u64) -> alloc::vec::Vec<u8> {
        let mut elf = alloc::vec![0u8; 120];
        elf[..4].copy_from_slice(b"\x7fELF");
        elf[4] = 2;
        elf[5] = 1;
        elf[6] = 1;
        elf[16..18].copy_from_slice(&2u16.to_le_bytes());
        elf[18..20].copy_from_slice(&(profile.elf_machine as u16).to_le_bytes());
        elf[20..24].copy_from_slice(&1u32.to_le_bytes());
        elf[32..40].copy_from_slice(&64u64.to_le_bytes());
        elf[52..54].copy_from_slice(&64u16.to_le_bytes());
        elf[54..56].copy_from_slice(&56u16.to_le_bytes());
        elf[56..58].copy_from_slice(&1u16.to_le_bytes());
        elf[64..68].copy_from_slice(&1u32.to_le_bytes());
        elf[72..80].copy_from_slice(&120u64.to_le_bytes());
        elf[96..104].copy_from_slice(&0u64.to_le_bytes());
        elf[104..112].copy_from_slice(&mem_size.to_le_bytes());
        elf
    }

    fn elf_image(profile: &TargetProfile, body: &[u8]) -> alloc::vec::Vec<u8> {
        let mut bytes = alloc::vec![0u8; ELF_HEADER_LEN + body.len()];
        bytes[..HEADER_LEN].copy_from_slice(&v2_header(profile));
        bytes[wire::OFF_HEADER_MAGIC..][..8].copy_from_slice(&ELF_IMAGE_MAGIC.to_le_bytes());
        bytes[ELF_HEADER_LEN..].copy_from_slice(body);
        bytes
    }

    fn v1_header() -> [u8; LEGACY_HEADER_LEN] {
        let mut bytes = [0u8; LEGACY_HEADER_LEN];
        bytes[wire::OFF_HEADER_MAGIC..][..8].copy_from_slice(&LEGACY_IMAGE_MAGIC.to_le_bytes());
        bytes[wire::OFF_HEADER_FORMAT_VERSION..][..4]
            .copy_from_slice(&LEGACY_FORMAT_VERSION.to_le_bytes());
        bytes[wire::OFF_HEADER_HEADER_SIZE..][..4]
            .copy_from_slice(&(LEGACY_HEADER_LEN as u32).to_le_bytes());
        bytes[wire::OFF_HEADER_KERNEL_ABI..][..4]
            .copy_from_slice(&wire::KERNEL_ABI_VERSION.to_le_bytes());
        bytes[wire::OFF_LEGACY_HEADER_STACK_BYTES..][..4]
            .copy_from_slice(&wire::DEFAULT_STACK_BYTES.to_le_bytes());
        bytes
    }

    #[test]
    fn a_v2_image_reports_the_target_it_declares() {
        for profile in crate::target_profile::PROFILES.iter() {
            let header = v2_header(profile);
            let (revision, declared) = target(&header).expect("well-formed header");
            assert_eq!(revision, Revision::V2);
            assert_eq!(declared.profile, profile.id);
            assert_eq!(declared.architecture, profile.architecture);
            assert_eq!(declared.abi, profile.abi);
            assert_eq!(declared.page_profile, profile.page_profile);
            assert_eq!(declared.required_features, profile.required_features);
            assert_eq!(admit(&header, profile), Ok(Revision::V2));
        }
    }

    #[test]
    fn an_elf_image_is_qualified_by_the_same_header_as_v2() {
        for profile in crate::target_profile::PROFILES.iter() {
            let body = elf_body(profile, profile.page_bytes);
            let image = elf_image(profile, &body);
            let (revision, declared) = target(&image).expect("well-formed header");
            assert_eq!(revision, Revision::Elf);
            assert!(revision.carries_elf());
            let (_, from_v2) = target(&v2_header(profile)).expect("well-formed header");
            assert_eq!(declared, from_v2);
            assert_eq!(admit(&image, profile), Ok(Revision::Elf));
        }
    }

    #[test]
    fn an_elf_payload_is_only_reachable_after_its_target_is_admitted() {
        let sel4 = TargetProfile::by_name("aarch64-sel4-qemu-virt").expect("declared profile");
        let body = elf_body(sel4, sel4.page_bytes);
        let image = elf_image(sel4, &body);
        assert_eq!(admit_elf(&image, sel4), Ok(body.as_slice()));

        let board = TargetProfile::by_name("aarch64-rpi5").expect("declared profile");
        assert_eq!(
            admit_elf(&image, board),
            Err(ComponentTargetError::Target(TargetError::ProfileMismatch))
        );
        assert_eq!(
            admit_elf(&v2_header(sel4), sel4),
            Err(ComponentTargetError::UnsupportedVersion)
        );
        let empty = elf_image(sel4, b"");
        assert_eq!(
            admit_elf(&empty, sel4),
            Err(ComponentTargetError::Truncated)
        );
    }

    #[test]
    fn a_retained_v1_image_means_x86_not_architecture_neutral() {
        let header = v1_header();
        let (revision, declared) = target(&header).expect("retained header");
        assert_eq!(revision, Revision::V1);
        assert_eq!(declared.architecture, ARCH_X86_64);
        assert_eq!(declared, TargetProfile::legacy_image_target());

        let x86 = TargetProfile::legacy().expect("legacy profile");
        assert_eq!(admit(&header, x86), Ok(Revision::V1));

        let board = TargetProfile::by_name("aarch64-rpi5").expect("declared profile");
        assert_eq!(
            admit(&header, board),
            Err(ComponentTargetError::Target(TargetError::ProfileMismatch))
        );
    }

    #[test]
    fn retained_v1_nonzero_reserved_fails_closed() {
        let mut header = v1_header();
        header[wire::OFF_LEGACY_HEADER_RESERVED..][..2].copy_from_slice(&1u16.to_le_bytes());
        assert_eq!(target(&header), Err(ComponentTargetError::NonZeroReserved));
    }

    #[test]
    fn retained_v1_header_size_mismatch_fails_closed() {
        let mut header = v1_header();
        header[wire::OFF_HEADER_HEADER_SIZE..][..4]
            .copy_from_slice(&(HEADER_LEN as u32).to_le_bytes());
        assert_eq!(target(&header), Err(ComponentTargetError::Truncated));
    }

    #[test]
    fn a_wrong_target_image_is_refused_for_the_profile_that_named_it() {
        let board = TargetProfile::by_name("aarch64-rpi5").expect("declared profile");
        let x86 = TargetProfile::legacy().expect("legacy profile");
        assert_eq!(
            admit(&v2_header(board), x86),
            Err(ComponentTargetError::Target(TargetError::ProfileMismatch))
        );
    }

    #[test]
    fn same_architecture_wrong_profile_is_refused() {
        let qemu = TargetProfile::by_name("aarch64-qemu-virt").expect("declared profile");
        let board = TargetProfile::by_name("aarch64-rpi5").expect("declared profile");
        assert_eq!(
            admit(&v2_header(qemu), board),
            Err(ComponentTargetError::Target(TargetError::ProfileMismatch))
        );
    }

    #[test]
    fn a_declared_feature_the_profile_lacks_is_refused() {
        let board = TargetProfile::by_name("aarch64-rpi5").expect("declared profile");
        let mut header = v2_header(board);
        let richer = FEATURE_AARCH64_BASELINE
            | FEATURE_AARCH64_GICV3
            | FEATURE_AARCH64_GENERIC_TIMER
            | (1 << 33);
        header[wire::OFF_HEADER_REQUIRED_FEATURES..][..8].copy_from_slice(&richer.to_le_bytes());
        assert_eq!(
            admit(&header, board),
            Err(ComponentTargetError::Target(TargetError::FeatureMismatch))
        );
    }

    #[test]
    fn each_qualification_axis_is_reported_separately() {
        let board = TargetProfile::by_name("aarch64-rpi5").expect("declared profile");
        let mut abi = v2_header(board);
        abi[wire::OFF_HEADER_ABI..][..4].copy_from_slice(&99u32.to_le_bytes());
        assert_eq!(
            admit(&abi, board),
            Err(ComponentTargetError::Target(TargetError::AbiMismatch))
        );

        let mut page = v2_header(board);
        page[wire::OFF_HEADER_PAGE_PROFILE..][..4].copy_from_slice(&99u32.to_le_bytes());
        assert_eq!(
            admit(&page, board),
            Err(ComponentTargetError::Target(
                TargetError::PageProfileMismatch
            ))
        );
    }

    #[test]
    fn wrong_component_abi_is_refused_before_loading() {
        let profile = TargetProfile::by_name("aarch64-sel4-qemu-virt").expect("declared profile");
        let body = elf_body(profile, profile.page_bytes);
        let mut image = elf_image(profile, &body);
        image[wire::OFF_HEADER_KERNEL_ABI..][..4]
            .copy_from_slice(&(wire::KERNEL_ABI_VERSION + 1).to_le_bytes());
        assert_eq!(
            admit_elf(&image, profile),
            Err(ComponentTargetError::KernelAbiMismatch)
        );
    }

    #[test]
    fn stack_declaration_is_bounded_and_page_aligned() {
        let profile = TargetProfile::by_name("aarch64-sel4-qemu-virt").expect("declared profile");
        let body = elf_body(profile, profile.page_bytes);
        for stack in [0, PAGE - 1, MAX_STACK_BYTES + PAGE] {
            let mut image = elf_image(profile, &body);
            image[wire::OFF_HEADER_STACK_BYTES..][..4].copy_from_slice(&stack.to_le_bytes());
            assert_eq!(
                admit_elf(&image, profile),
                Err(ComponentTargetError::BadStack)
            );
        }
        let mut image = elf_image(profile, &body);
        image[wire::OFF_HEADER_STACK_BYTES..][..4].copy_from_slice(&MAX_STACK_BYTES.to_le_bytes());
        assert_eq!(admit_elf(&image, profile), Ok(body.as_slice()));
    }

    #[test]
    fn elf_wrapper_requires_zero_segment_fields() {
        let profile = TargetProfile::by_name("aarch64-sel4-qemu-virt").expect("declared profile");
        let body = elf_body(profile, profile.page_bytes);
        for (offset, value) in [
            (wire::OFF_HEADER_ENTRY_OFFSET, 1u32),
            (wire::OFF_HEADER_SEGMENT_COUNT, 1u32),
        ] {
            let mut image = elf_image(profile, &body);
            if offset == wire::OFF_HEADER_SEGMENT_COUNT {
                image[offset..][..2].copy_from_slice(&(value as u16).to_le_bytes());
            } else {
                image[offset..][..4].copy_from_slice(&value.to_le_bytes());
            }
            assert_eq!(
                admit_elf(&image, profile),
                Err(ComponentTargetError::BadHeaderShape)
            );
        }
    }

    #[test]
    fn elf_body_size_is_bounded_without_allocating_the_body() {
        assert_eq!(validate_elf_len(MAX_IMAGE_BYTES as usize), Ok(()));
        assert_eq!(
            validate_elf_len(MAX_IMAGE_BYTES as usize + 1),
            Err(ComponentTargetError::ImageTooLarge)
        );
    }

    #[test]
    fn elf_identity_and_mapped_footprint_are_enforced() {
        let profile = TargetProfile::by_name("aarch64-sel4-qemu-virt").expect("declared profile");
        let mut wrong_machine = elf_body(profile, profile.page_bytes);
        wrong_machine[18..20].copy_from_slice(&0u16.to_le_bytes());
        assert_eq!(
            admit_elf(&elf_image(profile, &wrong_machine), profile),
            Err(ComponentTargetError::BadElfIdentity),
        );

        let oversized = elf_body(profile, MAX_IMAGE_BYTES + profile.page_bytes);
        assert_eq!(
            admit_elf(&elf_image(profile, &oversized), profile),
            Err(ComponentTargetError::ImageTooLarge),
        );
    }

    #[test]
    fn elf_program_header_size_matches_the_root_loader() {
        let profile = TargetProfile::by_name("aarch64-sel4-qemu-virt").expect("declared profile");
        let mut body = elf_body(profile, profile.page_bytes);
        body[54..56].copy_from_slice(&64u16.to_le_bytes());
        assert_eq!(
            admit_elf(&elf_image(profile, &body), profile),
            Err(ComponentTargetError::BadElfShape),
        );
    }

    #[test]
    fn an_aarch64_qualification_never_passes_as_x86() {
        let declared = ImageTarget {
            profile: crate::target_profile::PROFILE_X86_64_QEMU_VIRTIO,
            architecture: ARCH_AARCH64,
            abi: ABI_SLIME_AARCH64_V1,
            page_profile: PAGE_PROFILE_AARCH64_4K,
            required_features: FEATURE_AARCH64_BASELINE
                | FEATURE_AARCH64_GICV3
                | FEATURE_AARCH64_GENERIC_TIMER,
        };
        let x86 = TargetProfile::legacy().expect("legacy profile");
        assert_eq!(x86.admit(&declared), Err(TargetError::ArchitectureMismatch));
    }

    #[test]
    fn a_nonzero_reserved_field_fails_closed() {
        let x86 = TargetProfile::legacy().expect("legacy profile");
        let mut header = v2_header(x86);
        header[wire::OFF_HEADER_RESERVED..][..2].copy_from_slice(&1u16.to_le_bytes());
        assert_eq!(target(&header), Err(ComponentTargetError::NonZeroReserved));

        let mut header = v2_header(x86);
        header[wire::OFF_HEADER_TARGET_PROFILE..][..4].copy_from_slice(&99u32.to_le_bytes());
        assert_eq!(
            admit(&header, x86),
            Err(ComponentTargetError::Target(TargetError::ProfileMismatch))
        );
    }

    #[test]
    fn mismatched_magic_and_version_pairs_fail_closed() {
        let x86 = TargetProfile::legacy().expect("legacy profile");

        let mut wrong_magic = v2_header(x86);
        wrong_magic[wire::OFF_HEADER_MAGIC..][..8].copy_from_slice(&0u64.to_le_bytes());
        assert_eq!(target(&wrong_magic), Err(ComponentTargetError::BadMagic));

        let mut v2_magic_v1_version = v2_header(x86);
        v2_magic_v1_version[wire::OFF_HEADER_FORMAT_VERSION..][..4]
            .copy_from_slice(&LEGACY_FORMAT_VERSION.to_le_bytes());
        assert_eq!(
            target(&v2_magic_v1_version),
            Err(ComponentTargetError::UnsupportedVersion)
        );

        let mut v1_magic_v2_version = v1_header();
        v1_magic_v2_version[wire::OFF_HEADER_FORMAT_VERSION..][..4]
            .copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        assert_eq!(
            target(&v1_magic_v2_version),
            Err(ComponentTargetError::UnsupportedVersion)
        );
    }

    #[test]
    fn a_truncated_header_never_reads_past_the_blob() {
        assert_eq!(target(&[]), Err(ComponentTargetError::Truncated));
        let x86 = TargetProfile::legacy().expect("legacy profile");
        let header = v2_header(x86);
        for length in 0..HEADER_LEN {
            assert_eq!(
                target(&header[..length]),
                Err(ComponentTargetError::Truncated),
                "a {length}-byte prefix must not decode"
            );
        }
    }

    #[test]
    fn a_v2_header_size_that_disagrees_with_the_contract_fails_closed() {
        let x86 = TargetProfile::legacy().expect("legacy profile");
        let mut header = v2_header(x86);
        header[wire::OFF_HEADER_HEADER_SIZE..][..4]
            .copy_from_slice(&(LEGACY_HEADER_LEN as u32).to_le_bytes());
        assert_eq!(target(&header), Err(ComponentTargetError::Truncated));
    }

    /// A 4 KiB page granule, which every profile this format admits uses.
    const PAGE: u32 = 4096;

    /// One segment record, encoded.
    fn segment(
        vaddr: u32,
        mem_len: u32,
        file_offset: u32,
        file_len: u32,
        flags: u16,
    ) -> [u8; SEGMENT_LEN] {
        WireSegmentRecord {
            vaddr_offset: vaddr,
            mem_len,
            file_offset,
            file_len,
            flags,
            reserved: 0,
        }
        .encode()
    }

    /// A one-segment executable table and a payload long enough for it.
    fn one_executable_segment() -> ([u8; SEGMENT_LEN], [u8; 64]) {
        (segment(0, PAGE, 0, 32, SEGMENT_FLAG_EXEC), [0u8; 64])
    }

    #[test]
    fn a_well_formed_segment_table_is_admitted() {
        let (records, data) = one_executable_segment();
        assert_eq!(validate_segments(&records, &data, 1, 0, PAGE), Ok(()));
    }

    /// A `.bss` tail: `mem_len` beyond `file_len` is the zero-fill the loader
    /// owes, not a malformed range.
    #[test]
    fn a_zero_fill_tail_is_not_a_malformed_range() {
        let records = segment(0, PAGE * 2, 0, 16, SEGMENT_FLAG_EXEC);
        assert_eq!(validate_segments(&records, &[0u8; 64], 1, 0, PAGE), Ok(()));
    }

    #[test]
    fn zero_and_excess_segment_counts_are_refused() {
        let (records, data) = one_executable_segment();
        for count in [0, MAX_SEGMENTS + 1] {
            assert_eq!(
                validate_segments(&records, &data, count, 0, PAGE),
                Err(SegmentError::BadSegmentCount),
            );
        }
    }

    #[test]
    fn zero_page_size_is_refused_without_panicking() {
        let (records, data) = one_executable_segment();
        assert_eq!(
            validate_segments(&records, &data, 1, 0, 0),
            Err(SegmentError::BadSegment),
        );
    }

    /// W^X as an image property. A segment claiming both is refused before any
    /// loader can map it that way, which is the invariant the roadmap states.
    #[test]
    fn a_writable_executable_segment_is_refused() {
        let records = segment(0, PAGE, 0, 32, SEGMENT_FLAG_WRITE | SEGMENT_FLAG_EXEC);
        assert_eq!(
            validate_segments(&records, &[0u8; 64], 1, 0, PAGE),
            Err(SegmentError::BadFlags),
        );
    }

    #[test]
    fn undefined_segment_flags_are_refused() {
        for flags in [1 << 2, 1 << 15] {
            let records = segment(0, PAGE, 0, 32, SEGMENT_FLAG_EXEC | flags);
            assert_eq!(
                validate_segments(&records, &[0u8; 64], 1, 0, PAGE),
                Err(SegmentError::BadFlags),
            );
        }
    }

    #[test]
    fn nonzero_segment_reserved_field_is_refused() {
        let mut records = segment(0, PAGE, 0, 32, SEGMENT_FLAG_EXEC);
        records[wire::OFF_SEGMENT_RESERVED..][..2].copy_from_slice(&1u16.to_le_bytes());
        assert_eq!(
            validate_segments(&records, &[0u8; 64], 1, 0, PAGE),
            Err(SegmentError::NonZeroReserved),
        );
    }

    /// Misaligned, empty, and file-longer-than-memory, which are the three
    /// shapes a single record can be wrong in.
    #[test]
    fn a_malformed_single_segment_is_refused() {
        for records in [
            segment(1, PAGE, 0, 32, SEGMENT_FLAG_EXEC),
            segment(0, 0, 0, 0, SEGMENT_FLAG_EXEC),
            segment(0, 16, 0, 32, SEGMENT_FLAG_EXEC),
        ] {
            assert_eq!(
                validate_segments(&records, &[0u8; 64], 1, 0, PAGE),
                Err(SegmentError::BadSegment),
            );
        }
    }

    /// Out of order and overlapping are one check seen from two sides: a table
    /// whose ranges are unsorted cannot be proven non-overlapping in one pass.
    #[test]
    fn unsorted_and_overlapping_ranges_are_refused() {
        let mut unsorted = [0u8; SEGMENT_LEN * 2];
        unsorted[..SEGMENT_LEN].copy_from_slice(&segment(PAGE, PAGE, 0, 16, SEGMENT_FLAG_EXEC));
        unsorted[SEGMENT_LEN..].copy_from_slice(&segment(0, PAGE, 16, 16, SEGMENT_FLAG_WRITE));

        let mut overlapping = [0u8; SEGMENT_LEN * 2];
        overlapping[..SEGMENT_LEN].copy_from_slice(&segment(0, PAGE * 2, 0, 16, SEGMENT_FLAG_EXEC));
        overlapping[SEGMENT_LEN..].copy_from_slice(&segment(
            PAGE,
            PAGE,
            16,
            16,
            SEGMENT_FLAG_WRITE,
        ));

        for records in [unsorted, overlapping] {
            assert_eq!(
                validate_segments(&records, &[0u8; 64], 2, 0, PAGE),
                Err(SegmentError::BadSegment),
            );
        }
    }

    #[test]
    fn a_file_range_past_the_payload_is_refused() {
        let records = segment(0, PAGE, 0, 65, SEGMENT_FLAG_EXEC);
        assert_eq!(
            validate_segments(&records, &[0u8; 64], 1, 0, PAGE),
            Err(SegmentError::BadFileRange),
        );
    }

    /// The entry must land inside an *executable* segment, not merely inside
    /// the image: an entry pointing at data is an image that cannot start.
    #[test]
    fn an_entry_outside_every_executable_segment_is_refused() {
        let (records, data) = one_executable_segment();
        assert_eq!(
            validate_segments(&records, &data, 1, PAGE, PAGE),
            Err(SegmentError::BadEntry),
        );

        let writable = segment(0, PAGE, 0, 32, SEGMENT_FLAG_WRITE);
        assert_eq!(
            validate_segments(&writable, &[0u8; 64], 1, 0, PAGE),
            Err(SegmentError::BadEntry),
        );
    }

    #[test]
    fn a_footprint_past_the_image_ceiling_is_refused() {
        // One page past the ceiling: exactly `MAX_IMAGE_BYTES` is admissible,
        // so the fixture has to exceed it rather than reach it.
        let over = u32::try_from(MAX_IMAGE_BYTES + u64::from(PAGE)).unwrap_or(u32::MAX);
        let records = segment(0, over, 0, 0, SEGMENT_FLAG_EXEC);
        assert_eq!(
            validate_segments(&records, &[0u8; 64], 1, 0, PAGE),
            Err(SegmentError::ImageTooLarge),
        );
    }

    #[test]
    fn a_record_shorter_than_the_contract_is_refused() {
        let (records, data) = one_executable_segment();
        assert_eq!(
            validate_segments(&records[..SEGMENT_LEN - 1], &data, 1, 0, PAGE),
            Err(SegmentError::Truncated),
        );
    }
}
