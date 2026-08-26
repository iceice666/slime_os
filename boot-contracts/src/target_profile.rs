//! Target profiles (`contracts/target-profile/v1`).
//!
//! A profile is a complete executable and platform contract: architecture,
//! architecture-qualified ABI, page profile, and an exact required-feature
//! mask. Two artifacts belong to the same profile only when all four agree.
//!
//! The table itself is generated from the contract, so the host builder that
//! stamps these identifiers into image headers and the admission path that
//! checks them cannot drift apart. This module adds only the lookup and
//! comparison logic; it holds no policy of its own.
//!
//! Feature matching is equality, not containment. An image that declares a
//! feature the profile does not name is rejected here rather than faulting on a
//! missing instruction after it has been mapped and entered.

include!("generated/target_profile.rs");

/// One admitted target profile. Field values come from the contract table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetProfile {
    pub id: u32,
    pub name: &'static str,
    pub architecture: u32,
    pub abi: u32,
    pub page_profile: u32,
    pub required_features: u64,
    /// `e_machine` a host ELF intermediate must carry for this profile.
    pub elf_machine: u32,
    pub page_bytes: u64,
    pub kernel_preferred_base: u64,
    pub kernel_load_base: u64,
    pub component_base: u64,
}

/// The target qualification an executable image declares in its header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageTarget {
    pub profile: u32,
    pub architecture: u32,
    pub abi: u32,
    pub page_profile: u32,
    pub required_features: u64,
}

/// Why an artifact was refused admission for a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetError {
    /// The generation or release names a profile this build does not admit.
    UnknownProfile,
    /// The generation and the executable name different exact profiles.
    ProfileMismatch,
    /// The image was built for a different instruction set.
    ArchitectureMismatch,
    /// The image declares a different calling convention for this ISA.
    AbiMismatch,
    /// The image needs a different page granule or translation profile.
    PageProfileMismatch,
    /// The image's required-feature mask is not exactly the profile's.
    FeatureMismatch,
    /// The image is linked for a base address this profile does not use.
    LoadLayoutMismatch,
}

impl TargetProfile {
    /// Resolve a profile by its exact generation/release `target` string.
    /// Unknown names fail rather than resolving to a nearby profile.
    pub fn by_name(name: &str) -> Result<&'static Self, TargetError> {
        let mut index = 0;
        while index < PROFILE_COUNT {
            if str_eq(PROFILES[index].name, name) {
                return Ok(&PROFILES[index]);
            }
            index += 1;
        }
        Err(TargetError::UnknownProfile)
    }

    /// Resolve a profile by its stable numeric id.
    pub fn by_id(id: u32) -> Result<&'static Self, TargetError> {
        let mut index = 0;
        while index < PROFILE_COUNT {
            if PROFILES[index].id == id {
                return Ok(&PROFILES[index]);
            }
            index += 1;
        }
        Err(TargetError::UnknownProfile)
    }
    /// Resolve the exact profile this binary was built to execute.
    ///
    /// AArch64 has multiple profiles on one Cargo target, so its build must set
    /// `SLIME_TARGET_PROFILE`. Single-profile architectures have a fail-closed
    /// default, while an explicit name must still match the compiled ISA.
    pub fn current() -> Result<&'static Self, TargetError> {
        #[cfg(target_arch = "x86_64")]
        let default_name = "x86_64-qemu-virtio";
        #[cfg(target_arch = "aarch64")]
        let default_name = "";
        #[cfg(target_arch = "riscv64")]
        let default_name = "riscv64-qemu-virt";
        #[cfg(not(any(
            target_arch = "x86_64",
            target_arch = "aarch64",
            target_arch = "riscv64"
        )))]
        let default_name = "";

        let name = match option_env!("SLIME_TARGET_PROFILE") {
            Some(name) => name,
            None => default_name,
        };
        let profile = Self::by_name(name)?;
        #[cfg(target_arch = "x86_64")]
        if profile.architecture != ARCH_X86_64 {
            return Err(TargetError::ArchitectureMismatch);
        }
        #[cfg(target_arch = "aarch64")]
        if profile.architecture != ARCH_AARCH64 {
            return Err(TargetError::ArchitectureMismatch);
        }
        #[cfg(target_arch = "riscv64")]
        if profile.architecture != ARCH_RISCV64 {
            return Err(TargetError::ArchitectureMismatch);
        }
        Ok(profile)
    }

    /// Admit an executable image's declared qualification against this profile.
    /// Each axis reports separately so a failure names what actually differs.
    pub fn admit(&self, image: &ImageTarget) -> Result<(), TargetError> {
        if image.profile != self.id {
            return Err(TargetError::ProfileMismatch);
        }
        if image.architecture != self.architecture {
            return Err(TargetError::ArchitectureMismatch);
        }
        if image.abi != self.abi {
            return Err(TargetError::AbiMismatch);
        }
        if image.page_profile != self.page_profile {
            return Err(TargetError::PageProfileMismatch);
        }
        if image.required_features != self.required_features {
            return Err(TargetError::FeatureMismatch);
        }
        Ok(())
    }

    /// The qualification a retained pre-P0 (v1) image carries implicitly. A v1
    /// image is not architecture-neutral: it is an `x86_64-qemu-virtio` image
    /// whose target was implied by the only builder that could emit it.
    pub fn legacy() -> Result<&'static Self, TargetError> {
        Self::by_id(PROFILE_X86_64_QEMU_VIRTIO)
    }

    /// The declared qualification of a retained v1 image, for admission against
    /// whatever profile the generation actually names.
    pub fn legacy_image_target() -> ImageTarget {
        ImageTarget {
            profile: PROFILE_X86_64_QEMU_VIRTIO,
            architecture: ARCH_X86_64,
            abi: ABI_SLIME_X86_64_V1,
            page_profile: PAGE_PROFILE_X86_64_4K,
            required_features: FEATURE_X86_64_BASELINE,
        }
    }
}

/// `str::eq` is not const-friendly across all toolchains here and this runs in
/// allocation-free `no_std` admission code; compare bytes directly.
fn str_eq(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_profile_name_resolves_to_itself() {
        for profile in PROFILES.iter() {
            let resolved = TargetProfile::by_name(profile.name).expect("declared profile");
            assert_eq!(resolved.id, profile.id);
            assert_eq!(
                TargetProfile::by_id(profile.id).expect("declared id").name,
                profile.name
            );
        }
    }

    #[test]
    fn profile_ids_and_names_are_unique() {
        for (index, profile) in PROFILES.iter().enumerate() {
            for other in PROFILES.iter().skip(index + 1) {
                assert_ne!(profile.id, other.id, "duplicate profile id");
                assert_ne!(profile.name, other.name, "duplicate profile name");
            }
        }
    }

    #[test]
    fn unknown_target_names_do_not_resolve_to_a_nearby_profile() {
        for name in [
            "",
            "aarch64",
            "aarch64-rpi4",
            "x86_64-qemu-virtio ",
            "AARCH64-RPI5",
        ] {
            assert_eq!(
                TargetProfile::by_name(name),
                Err(TargetError::UnknownProfile)
            );
        }
        assert_eq!(TargetProfile::by_id(0), Err(TargetError::UnknownProfile));
        assert_eq!(
            TargetProfile::by_id(u32::MAX),
            Err(TargetError::UnknownProfile)
        );
    }

    #[test]
    fn a_profile_admits_exactly_its_own_qualification() {
        for profile in PROFILES.iter() {
            let exact = ImageTarget {
                profile: profile.id,
                architecture: profile.architecture,
                abi: profile.abi,
                page_profile: profile.page_profile,
                required_features: profile.required_features,
            };
            assert_eq!(profile.admit(&exact), Ok(()));
        }
    }

    #[test]
    fn each_mismatched_axis_is_reported_separately() {
        let profile = TargetProfile::by_name("aarch64-rpi5").expect("declared profile");
        let exact = ImageTarget {
            profile: profile.id,
            architecture: profile.architecture,
            abi: profile.abi,
            page_profile: profile.page_profile,
            required_features: profile.required_features,
        };
        assert_eq!(
            profile.admit(&ImageTarget {
                profile: PROFILE_AARCH64_QEMU_VIRT,
                ..exact
            }),
            Err(TargetError::ProfileMismatch)
        );
        assert_eq!(
            profile.admit(&ImageTarget {
                architecture: ARCH_X86_64,
                ..exact
            }),
            Err(TargetError::ArchitectureMismatch)
        );
        assert_eq!(
            profile.admit(&ImageTarget {
                abi: ABI_SLIME_X86_64_V1,
                ..exact
            }),
            Err(TargetError::AbiMismatch)
        );
        assert_eq!(
            profile.admit(&ImageTarget {
                page_profile: PAGE_PROFILE_X86_64_4K,
                ..exact
            }),
            Err(TargetError::PageProfileMismatch)
        );
        assert_eq!(
            profile.admit(&ImageTarget {
                required_features: 0,
                ..exact
            }),
            Err(TargetError::FeatureMismatch)
        );
    }

    #[test]
    fn a_richer_feature_set_is_rejected_rather_than_downgraded() {
        let profile = TargetProfile::by_name("aarch64-qemu-virt").expect("declared profile");
        let superset = ImageTarget {
            profile: profile.id,
            architecture: profile.architecture,
            abi: profile.abi,
            page_profile: profile.page_profile,
            required_features: profile.required_features | (1 << 40),
        };
        assert_eq!(profile.admit(&superset), Err(TargetError::FeatureMismatch));
    }

    #[test]
    fn an_x86_image_is_never_admitted_for_an_arm_profile() {
        let legacy = TargetProfile::legacy_image_target();
        for name in ["aarch64-qemu-virt", "aarch64-rpi5", "riscv64-qemu-virt"] {
            let profile = TargetProfile::by_name(name).expect("declared profile");
            assert_eq!(profile.admit(&legacy), Err(TargetError::ProfileMismatch));
        }
        let x86 = TargetProfile::legacy().expect("legacy profile");
        assert_eq!(x86.admit(&legacy), Ok(()));
    }

    #[test]
    fn the_two_aarch64_profiles_reject_each_others_images() {
        let qemu = TargetProfile::by_name("aarch64-qemu-virt").expect("declared profile");
        let board = TargetProfile::by_name("aarch64-rpi5").expect("declared profile");
        let qemu_image = ImageTarget {
            profile: qemu.id,
            architecture: qemu.architecture,
            abi: qemu.abi,
            page_profile: qemu.page_profile,
            required_features: qemu.required_features,
        };
        assert_eq!(board.admit(&qemu_image), Err(TargetError::ProfileMismatch));
    }

    #[test]
    fn profile_names_fit_the_signed_release_target_area() {
        for profile in PROFILES.iter() {
            assert!(!profile.name.is_empty(), "profile name must be non-empty");
            assert!(
                profile.name.len() <= MAX_NAME_BYTES,
                "profile name exceeds the release target area"
            );
        }
    }
}
