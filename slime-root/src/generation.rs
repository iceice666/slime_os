//! Bounded generation preflight and authority derivation for `slime-root`.
//!
//! Admission is a pure decision over already-decoded generation data: it
//! re-checks the closure `slime-root` depends on, classifies each component's
//! payload so a non-ELF image can never reach the loader, and derives each
//! task's endpoint authority from declared grants only. No ambient authority is
//! synthesized here: a component with no declared inbound grant receives an
//! endpoint capability with no rights at all, which the kernel will refuse to
//! send on.

use boot_contracts::component_image::{self, ComponentTargetError};
use boot_contracts::generation::{
    DecodeError, Generation, KIND_BOOTSTRAP, KIND_COMPONENT, KIND_KERNEL, RIGHT_TRANSFER, Rights,
};
use boot_contracts::target_profile::TargetProfile;

/// Components `slime-root` will track in one generation. Matches the generation
/// format's own `MAX_COMPONENTS`; a larger graph fails closed.
pub const MAX_ADMITTED_COMPONENTS: usize = 48;

/// Logical IPC rights, numbered as in `kernel/src/capability/mod.rs`. The
/// generation format owns `RIGHT_TRANSFER`; the send/receive bits it shares
/// with the kernel are restated here because `slime-root` maps them onto seL4
/// endpoint rights.
pub const RIGHT_SEND: Rights = 1;
pub const RIGHT_RECV: Rights = 1 << 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GenerationError {
    Decode(DecodeError),
    /// The graph declares no kernel object, several, or no bootstrap/component.
    MalformedClosure {
        kernel: usize,
        bootstrap: usize,
        components: usize,
    },
    /// More components than [`MAX_ADMITTED_COMPONENTS`].
    TooManyComponents {
        declared: usize,
        limit: usize,
    },
    /// A component names an object the graph does not contain.
    DanglingComponent {
        component: usize,
    },
}

impl From<DecodeError> for GenerationError {
    fn from(error: DecodeError) -> Self {
        Self::Decode(error)
    }
}

/// How a component's payload is encoded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PayloadFormat {
    /// A target-qualified image carrying a native AArch64 ELF (P5.2), admitted
    /// for this root task's profile. Loadable.
    QualifiedElf,
    /// A bare AArch64 little-endian ELF64 image, carrying no target
    /// qualification. Loadable, and used by the native fixture the root task
    /// embeds at compile time; a generation payload is expected to be
    /// qualified.
    Aarch64Elf,
    /// A Slime component image whose payload is a segment table
    /// (`SLIMECMP`/`SLIMECM2`). These are the custom format the retired Slime
    /// kernel loaded, and `slime-root` has no loader for them.
    SlimeComponent,
    /// A qualified image that is not admitted for this profile — wrong
    /// architecture, ABI, page profile, or feature set. Never loadable, and
    /// distinguished from [`Self::Unrecognized`] so a wrong-target artifact is
    /// reported as refused rather than as malformed.
    WrongTarget,
    /// None of the above.
    Unrecognized,
}

impl PayloadFormat {
    pub const fn is_loadable(self) -> bool {
        matches!(self, Self::QualifiedElf | Self::Aarch64Elf)
    }

    /// Classify a payload, admitting a qualified image against `profile`.
    ///
    /// The magic is read through `boot_contracts::component_image`, not by
    /// prefix comparison: `SLIMECME` and `SLIMECM2` share a seven-byte prefix,
    /// so a `starts_with(b"SLIMECM")` test would call every ELF-carrying image
    /// an unloadable legacy one. The contract decoder distinguishes them by the
    /// full eight-byte magic, which is what it is for.
    ///
    /// Target admission happens *here*, before any byte reaches the loader,
    /// which is roadmap invariant 9: a wrong-target executable is refused
    /// before it is mapped, not after.
    pub fn classify(bytes: &[u8], profile: &TargetProfile) -> Self {
        const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];
        const ELF_CLASS64: u8 = 2;
        const ELF_DATA_LSB: u8 = 1;
        const ELF_MACHINE_AARCH64: u16 = 183;

        match component_image::target(bytes) {
            Ok((revision, _)) => {
                return match component_image::admit(bytes, profile) {
                    Ok(_) if revision.carries_elf() => Self::QualifiedElf,
                    // Admitted, but its body is a segment table this root task
                    // has no loader for.
                    Ok(_) => Self::SlimeComponent,
                    Err(ComponentTargetError::Target(_)) => Self::WrongTarget,
                    Err(_) => Self::Unrecognized,
                };
            }
            // Not a Slime component image at all; fall through to the bare-ELF
            // test below.
            Err(ComponentTargetError::BadMagic) => {}
            Err(_) => return Self::Unrecognized,
        }

        let Some(header) = bytes.get(..20) else {
            return Self::Unrecognized;
        };
        let machine = u16::from_le_bytes([header[18], header[19]]);
        if header[..4] == ELF_MAGIC
            && header[4] == ELF_CLASS64
            && header[5] == ELF_DATA_LSB
            && machine == ELF_MACHINE_AARCH64
        {
            Self::Aarch64Elf
        } else {
            Self::Unrecognized
        }
    }
}

/// One admitted component and the shape of its payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComponentPlan {
    pub component: usize,
    pub format: PayloadFormat,
    pub inbound_grants: usize,
}

/// The endpoint authority a task receives, derived from declared grants.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Authority {
    pub rights: Rights,
    pub grants: usize,
}

impl Authority {
    pub const NONE: Self = Self {
        rights: 0,
        grants: 0,
    };

    /// Map logical Slime rights onto seL4 endpoint rights.
    ///
    /// `write` is the send right. `read` is the receive right, which for a
    /// service endpoint means the holder may block on it. `grant_reply` is
    /// granted alongside send because a call must be able to hand the root task
    /// a reply authority, and because the non-MCS kernel refuses a fault
    /// handler endpoint that carries neither `grant` nor `grant_reply`. Full
    /// `grant` — the right to pass capabilities through the endpoint — is only
    /// given when the generation declares the grant transferable.
    pub fn endpoint_rights(self) -> sel4::CapRights {
        let can_send = self.rights & RIGHT_SEND != 0;
        sel4::CapRightsBuilder::none()
            .write(can_send)
            .read(self.rights & RIGHT_RECV != 0)
            .grant_reply(can_send)
            .grant(self.rights & RIGHT_TRANSFER != 0)
            .build()
    }
}

/// Accumulated inbound authority for one component: the union of the rights of
/// every grant that names it as target.
pub fn inbound_authority(
    generation: &Generation<'_>,
    component: usize,
) -> Result<Authority, GenerationError> {
    let mut authority = Authority::NONE;
    for index in 0..generation.grant_count() {
        let grant = generation.grant(index)?;
        if grant.target == component {
            authority.rights |= grant.rights;
            authority.grants += 1;
        }
    }
    Ok(authority)
}

/// The result of admitting a generation graph.
pub struct Admission {
    plans: [Option<ComponentPlan>; MAX_ADMITTED_COMPONENTS],
    len: usize,
    pub bootstrap: usize,
    pub kernel_objects: usize,
    pub bootstrap_objects: usize,
    pub component_objects: usize,
    pub grants: usize,
    pub health: usize,
    pub loadable: usize,
    pub slime_component_images: usize,
    pub unrecognized_images: usize,
    /// Qualified images refused for this profile. Non-zero means the generation
    /// carries an executable built for another target, which is refused before
    /// mapping rather than after.
    pub wrong_target_images: usize,
}

impl Admission {
    /// Re-check the closure and classify every component payload against
    /// `profile`, so a wrong-target executable is refused before the loader can
    /// be offered it.
    pub fn admit(
        generation: &Generation<'_>,
        profile: &TargetProfile,
    ) -> Result<Self, GenerationError> {
        let declared = generation.component_count();
        if declared > MAX_ADMITTED_COMPONENTS {
            return Err(GenerationError::TooManyComponents {
                declared,
                limit: MAX_ADMITTED_COMPONENTS,
            });
        }

        let mut kernel_objects = 0;
        let mut bootstrap_objects = 0;
        let mut component_objects = 0;
        for index in 0..generation.object_count() {
            match generation.object(index)?.kind {
                KIND_KERNEL => kernel_objects += 1,
                KIND_BOOTSTRAP => bootstrap_objects += 1,
                KIND_COMPONENT => component_objects += 1,
                _ => {}
            }
        }
        if kernel_objects != 1 || bootstrap_objects == 0 || component_objects == 0 {
            return Err(GenerationError::MalformedClosure {
                kernel: kernel_objects,
                bootstrap: bootstrap_objects,
                components: component_objects,
            });
        }

        let mut plans = [None; MAX_ADMITTED_COMPONENTS];
        let mut len = 0;
        let mut loadable = 0;
        let mut slime_component_images = 0;
        let mut unrecognized_images = 0;
        let mut wrong_target_images = 0;
        for component in 0..declared {
            let record = generation.component(component)?;
            let object = generation
                .object(record.object)
                .map_err(|_| GenerationError::DanglingComponent { component })?;
            let format = PayloadFormat::classify(object.bytes, profile);
            match format {
                PayloadFormat::QualifiedElf | PayloadFormat::Aarch64Elf => loadable += 1,
                PayloadFormat::SlimeComponent => slime_component_images += 1,
                PayloadFormat::WrongTarget => wrong_target_images += 1,
                PayloadFormat::Unrecognized => unrecognized_images += 1,
            }
            let Some(slot) = plans.get_mut(len) else {
                return Err(GenerationError::TooManyComponents {
                    declared,
                    limit: MAX_ADMITTED_COMPONENTS,
                });
            };
            *slot = Some(ComponentPlan {
                component,
                format,
                inbound_grants: inbound_authority(generation, component)?.grants,
            });
            len += 1;
        }

        for index in 0..generation.grant_count() {
            generation.grant(index)?;
        }
        for index in 0..generation.health_count() {
            generation.health_component(index)?;
        }

        Ok(Self {
            plans,
            len,
            bootstrap: generation.bootstrap,
            kernel_objects,
            bootstrap_objects,
            component_objects,
            grants: generation.grant_count(),
            health: generation.health_count(),
            loadable,
            slime_component_images,
            unrecognized_images,
            wrong_target_images,
        })
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn plans(&self) -> impl Iterator<Item = &ComponentPlan> {
        self.plans.iter().take(self.len).flatten()
    }

    /// Every admitted component whose payload this root task could load.
    pub fn loadable_plans(&self) -> impl Iterator<Item = &ComponentPlan> {
        self.plans().filter(|plan| plan.format.is_loadable())
    }
}

#[cfg(test)]
mod tests {
    use super::{Authority, PayloadFormat, RIGHT_RECV, RIGHT_SEND};
    use boot_contracts::component_image::wire;
    use boot_contracts::generation::RIGHT_TRANSFER;
    use boot_contracts::target_profile::TargetProfile;

    fn sel4_profile() -> &'static TargetProfile {
        TargetProfile::by_name("aarch64-sel4-qemu-virt").expect("declared profile")
    }

    fn elf_header(class: u8, data: u8, machine: u16) -> [u8; 20] {
        let mut bytes = [0u8; 20];
        bytes[..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        bytes[4] = class;
        bytes[5] = data;
        bytes[18..20].copy_from_slice(&machine.to_le_bytes());
        bytes
    }

    /// A qualified component image for `profile`, under `magic`.
    fn qualified(magic: u64, profile: &TargetProfile) -> [u8; wire::HEADER_LEN + 20] {
        let mut bytes = [0u8; wire::HEADER_LEN + 20];
        bytes[wire::OFF_HEADER_MAGIC..][..8].copy_from_slice(&magic.to_le_bytes());
        bytes[wire::OFF_HEADER_FORMAT_VERSION..][..4]
            .copy_from_slice(&wire::FORMAT_VERSION.to_le_bytes());
        bytes[wire::OFF_HEADER_HEADER_SIZE..][..4]
            .copy_from_slice(&(wire::HEADER_LEN as u32).to_le_bytes());
        bytes[wire::OFF_HEADER_ARCHITECTURE..][..4]
            .copy_from_slice(&profile.architecture.to_le_bytes());
        bytes[wire::OFF_HEADER_ABI..][..4].copy_from_slice(&profile.abi.to_le_bytes());
        bytes[wire::OFF_HEADER_PAGE_PROFILE..][..4]
            .copy_from_slice(&profile.page_profile.to_le_bytes());
        bytes[wire::OFF_HEADER_TARGET_PROFILE..][..4].copy_from_slice(&profile.id.to_le_bytes());
        bytes[wire::OFF_HEADER_REQUIRED_FEATURES..][..8]
            .copy_from_slice(&profile.required_features.to_le_bytes());
        bytes[wire::HEADER_LEN..].copy_from_slice(&elf_header(2, 1, 183));
        bytes
    }

    #[test]
    fn a_qualified_elf_image_is_loadable() {
        let profile = sel4_profile();
        let image = qualified(wire::ELF_IMAGE_MAGIC, profile);
        let format = PayloadFormat::classify(&image, profile);
        assert_eq!(format, PayloadFormat::QualifiedElf);
        assert!(format.is_loadable());
    }

    /// `SLIMECME` and `SLIMECM2` share a seven-byte prefix, so a `starts_with`
    /// test would call every ELF-carrying image an unloadable legacy one and
    /// the graph would silently never launch. This is that regression.
    #[test]
    fn the_elf_and_segment_magics_are_not_confused_by_their_shared_prefix() {
        let profile = sel4_profile();
        assert!(wire::ELF_IMAGE_MAGIC_BYTES.starts_with(b"SLIMECM"));
        assert!(wire::IMAGE_MAGIC_BYTES.starts_with(b"SLIMECM"));
        assert_ne!(wire::ELF_IMAGE_MAGIC, wire::IMAGE_MAGIC);
        assert_eq!(
            PayloadFormat::classify(&qualified(wire::ELF_IMAGE_MAGIC, profile), profile),
            PayloadFormat::QualifiedElf
        );
        assert_eq!(
            PayloadFormat::classify(&qualified(wire::IMAGE_MAGIC, profile), profile),
            PayloadFormat::SlimeComponent
        );
    }

    /// Invariant 9: a wrong-target executable is refused before mapping, and is
    /// reported as refused rather than as malformed.
    #[test]
    fn an_image_for_another_profile_is_refused_not_loaded() {
        let board = TargetProfile::by_name("aarch64-rpi5").expect("declared profile");
        let image = qualified(wire::ELF_IMAGE_MAGIC, board);
        let format = PayloadFormat::classify(&image, sel4_profile());
        assert_eq!(format, PayloadFormat::WrongTarget);
        assert!(!format.is_loadable());
    }

    #[test]
    fn aarch64_elf64_is_loadable() {
        let bytes = elf_header(2, 1, 183);
        assert_eq!(
            PayloadFormat::classify(&bytes, sel4_profile()),
            PayloadFormat::Aarch64Elf
        );
        assert!(PayloadFormat::classify(&bytes, sel4_profile()).is_loadable());
    }

    #[test]
    fn wrong_class_machine_or_endianness_is_not_loadable() {
        for bytes in [
            elf_header(1, 1, 183),
            elf_header(2, 2, 183),
            elf_header(2, 1, 62),
        ] {
            assert_eq!(
                PayloadFormat::classify(&bytes, sel4_profile()),
                PayloadFormat::Unrecognized
            );
        }
    }

    #[test]
    fn segment_carrying_images_are_recognized_but_not_loadable() {
        let x86 = TargetProfile::legacy().expect("legacy profile");
        // A retained v1 image, admitted for the profile it implies.
        let mut v1 = [0u8; wire::LEGACY_HEADER_LEN];
        v1[wire::OFF_HEADER_MAGIC..][..8].copy_from_slice(&wire::LEGACY_IMAGE_MAGIC.to_le_bytes());
        v1[wire::OFF_HEADER_FORMAT_VERSION..][..4]
            .copy_from_slice(&wire::LEGACY_FORMAT_VERSION.to_le_bytes());
        v1[wire::OFF_HEADER_HEADER_SIZE..][..4]
            .copy_from_slice(&(wire::LEGACY_HEADER_LEN as u32).to_le_bytes());
        let format = PayloadFormat::classify(&v1, x86);
        assert_eq!(format, PayloadFormat::SlimeComponent);
        assert!(!format.is_loadable());
    }

    #[test]
    fn short_payloads_are_unrecognized() {
        assert_eq!(
            PayloadFormat::classify(&[0x7f, b'E', b'L', b'F'], sel4_profile()),
            PayloadFormat::Unrecognized
        );
        assert_eq!(
            PayloadFormat::classify(&[], sel4_profile()),
            PayloadFormat::Unrecognized
        );
    }

    #[test]
    fn no_declared_grant_yields_no_endpoint_authority() {
        assert_eq!(
            Authority::NONE.endpoint_rights(),
            sel4::CapRights::none(),
            "an ungranted component must not be able to invoke the root endpoint"
        );
    }

    #[test]
    fn send_implies_reply_authority_but_not_capability_transfer() {
        let send_only = Authority {
            rights: RIGHT_SEND | RIGHT_RECV,
            grants: 1,
        };
        assert_eq!(
            send_only.endpoint_rights(),
            sel4::CapRights::new(true, false, true, true)
        );
        let transferable = Authority {
            rights: RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
            grants: 1,
        };
        assert_eq!(
            transferable.endpoint_rights(),
            sel4::CapRights::new(true, true, true, true)
        );
    }
}
