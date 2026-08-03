//! Component image target qualification (`contracts/component/v2`).
//!
//! The kernel owns full component-image decoding: segment bounds, entry
//! placement, W^X, and the mapped footprint are its concern because it is the
//! thing that maps them. Stage-0 never maps a component, but it does copy a
//! whole generation — executable payloads included — and hand it to a kernel
//! that will. So it needs one narrower answer before that copy: *which target
//! is this executable admitted for?*
//!
//! This module answers exactly that and nothing more. It reads the header,
//! establishes the revision, and reports the declared qualification. Keeping it
//! beside the kernel-image reader rather than duplicating a second structural
//! decoder is deliberate: two full decoders would be two chances to disagree
//! about what a byte means, and the kernel's is authoritative.
//!
//! A retained v1 image is not an unqualified image. It is an
//! `x86_64-qemu-virtio` image whose target was implied by the only builder that
//! could produce it, and it is reported as exactly that.

use crate::target_profile::{ImageTarget, TargetError, TargetProfile};

pub mod wire {
    include!("generated/component_image.rs");
}

pub use wire::{
    ELF_FORMAT_VERSION, ELF_HEADER_LEN, ELF_IMAGE_MAGIC, FORMAT_VERSION, HEADER_LEN, IMAGE_MAGIC,
    LEGACY_FORMAT_VERSION, LEGACY_HEADER_LEN, LEGACY_IMAGE_MAGIC, MAX_SEGMENTS, SEGMENT_LEN,
};

/// Why a component image's target could not be established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentTargetError {
    /// Fewer bytes than the revision's header requires.
    Truncated,
    BadMagic,
    /// The magic is known but paired with a version that revision never used.
    UnsupportedVersion,
    /// A v2 reserved field is nonzero, so the image was written by something
    /// that does not agree with this contract.
    NonZeroReserved,
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

/// The native ELF an [`Revision::Elf`] image carries, after its target has been
/// admitted for `profile`.
///
/// Admission runs first and its failure is returned unchanged, so a wrong-target
/// image never yields bytes a caller could map — the qualification check is not
/// something the caller can forget to do before reaching the payload.
pub fn admit_elf<'a>(
    blob: &'a [u8],
    profile: &TargetProfile,
) -> Result<&'a [u8], ComponentTargetError> {
    if admit(blob, profile)? != Revision::Elf {
        return Err(ComponentTargetError::UnsupportedVersion);
    }
    blob.get(ELF_HEADER_LEN..)
        .filter(|elf| !elf.is_empty())
        .ok_or(ComponentTargetError::Truncated)
}

/// Admit an image for `profile`, or report the axis that disagrees.
pub fn admit(blob: &[u8], profile: &TargetProfile) -> Result<Revision, ComponentTargetError> {
    let (revision, declared) = target(blob)?;
    profile
        .admit(&declared)
        .map_err(ComponentTargetError::Target)?;
    Ok(revision)
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

#[cfg(test)]
mod tests {
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
        bytes[wire::OFF_HEADER_TARGET_PROFILE..][..4].copy_from_slice(&profile.id.to_le_bytes());
        bytes[wire::OFF_HEADER_REQUIRED_FEATURES..][..8]
            .copy_from_slice(&profile.required_features.to_le_bytes());
        bytes
    }

    /// Bytes of the stand-in ELF body the tests below carry.
    const ELF_BODY: &[u8] = b"\x7fELF-body";

    /// An ELF-revision image: the shared qualification header under the ELF
    /// magic, followed by `body` standing in for a native executable. Returns a
    /// fixed-size buffer and its used length, because `boot-contracts` is
    /// `no_std` without `alloc`.
    fn elf_image(
        profile: &TargetProfile,
        body: &[u8],
    ) -> ([u8; ELF_HEADER_LEN + ELF_BODY.len()], usize) {
        let mut bytes = [0u8; ELF_HEADER_LEN + ELF_BODY.len()];
        bytes[..HEADER_LEN].copy_from_slice(&v2_header(profile));
        bytes[wire::OFF_HEADER_MAGIC..][..8].copy_from_slice(&ELF_IMAGE_MAGIC.to_le_bytes());
        bytes[ELF_HEADER_LEN..][..body.len()].copy_from_slice(body);
        (bytes, ELF_HEADER_LEN + body.len())
    }

    fn v1_header() -> [u8; LEGACY_HEADER_LEN] {
        let mut bytes = [0u8; LEGACY_HEADER_LEN];
        bytes[wire::OFF_HEADER_MAGIC..][..8].copy_from_slice(&LEGACY_IMAGE_MAGIC.to_le_bytes());
        bytes[wire::OFF_HEADER_FORMAT_VERSION..][..4]
            .copy_from_slice(&LEGACY_FORMAT_VERSION.to_le_bytes());
        bytes[wire::OFF_HEADER_HEADER_SIZE..][..4]
            .copy_from_slice(&(LEGACY_HEADER_LEN as u32).to_le_bytes());
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
            let (image, len) = elf_image(profile, ELF_BODY);
            let image = &image[..len];
            let (revision, declared) = target(image).expect("well-formed header");
            assert_eq!(revision, Revision::Elf);
            assert!(revision.carries_elf());
            // The distinguishing fact: identical offsets yield identical
            // qualification, so one layout describes both revisions.
            let (_, from_v2) = target(&v2_header(profile)).expect("well-formed header");
            assert_eq!(declared, from_v2);
            assert_eq!(admit(image, profile), Ok(Revision::Elf));
        }
    }

    #[test]
    fn an_elf_payload_is_only_reachable_after_its_target_is_admitted() {
        let sel4 = TargetProfile::by_name("aarch64-sel4-qemu-virt").expect("declared profile");
        let (buffer, len) = elf_image(sel4, ELF_BODY);
        let image = &buffer[..len];
        assert_eq!(admit_elf(image, sel4), Ok(ELF_BODY));

        // Offered to another profile, the bytes are never handed back: the
        // qualification failure is returned instead, so a wrong-target image
        // cannot be mapped by a caller that forgot to check first.
        let board = TargetProfile::by_name("aarch64-rpi5").expect("declared profile");
        assert_eq!(
            admit_elf(image, board),
            Err(ComponentTargetError::Target(TargetError::ProfileMismatch))
        );

        // A segment-carrying image is not an ELF image even when its target is
        // admitted, so the two loaders can never be handed each other's body.
        assert_eq!(
            admit_elf(&v2_header(sel4), sel4),
            Err(ComponentTargetError::UnsupportedVersion)
        );
        // A header with no payload has nothing to load.
        let (empty, empty_len) = elf_image(sel4, b"");
        assert_eq!(
            admit_elf(&empty[..empty_len], sel4),
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
}
