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
use boot_contracts::fabric_graph::{self, FabricGraph};
use boot_contracts::generation::{
    DecodeError, Generation, KIND_BOOTSTRAP, KIND_COMPONENT, KIND_KERNEL, KIND_RESOURCE,
    RIGHT_TRANSFER, Rights,
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
/// Authority to execute an object: what makes a grant name a spawnable
/// executable rather than a channel or a factory.
pub const RIGHT_EXEC: Rights = 1 << 3;

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
    /// The generation carries a fabric graph whose declared limits this root
    /// cannot satisfy, or which contradicts itself (C8.2).
    ///
    /// Distinct from [`Self::Decode`] because the bytes are well formed: the
    /// graph says what it needs and the answer is no. The whole generation
    /// fails closed, before the fabric or any participant launches.
    UnsatisfiableFabricGraph,
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
            // A segment-carrying image is unloadable here whatever it targets:
            // `slime-root` has no loader for the retired kernel's format. Its
            // target is therefore not consulted, so a retained x86 generation
            // is still reported as the Slime component images it holds rather
            // than as a pile of wrong-target artifacts.
            Ok((revision, _)) if !revision.carries_elf() => return Self::SlimeComponent,
            // An ELF-carrying image is one this root task could load, so the
            // target is exactly what decides whether it may.
            Ok(_) => {
                return match component_image::admit(bytes, profile) {
                    Ok(_) => Self::QualifiedElf,
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

/// Whether this root can satisfy a declared fabric graph (C8.2).
///
/// Separated from the generation walk so it is reachable without constructing
/// a generation blob: the interesting content is *which ceilings* are passed,
/// and those are this implementation's rather than the retired kernel's. The
/// predicate itself is `boot_contracts`, shared byte-for-byte with the oracle,
/// so the two can disagree only where their mechanisms genuinely differ —
/// which is the reason validating in both places is not redundant.
pub fn fabric_graph_is_satisfiable(graph: &FabricGraph<'_>) -> Result<(), GenerationError> {
    graph
        .validate_against(
            crate::ipc::MAX_WAIT_SOURCES as u32,
            crate::graph::MAX_TASK_CAPS as u32,
            crate::shared_buffer::MAX_TOTAL_PAGES as u32,
            crate::shared_buffer::MAX_SHARED_BUFFERS as u32,
            crate::shared_buffer::MAX_MAPPINGS as u32,
            crate::shared_buffer::MAX_LOANS as u32,
            crate::ipc::MAX_MESSAGE_BYTES as u32,
            crate::ipc::CHANNEL_CAPACITY as u32,
        )
        .map_err(|_| GenerationError::UnsatisfiableFabricGraph)?;
    // Every matched pair's offered QoS must satisfy the requested one
    // (P5.4.10). `validate_against` bounds the graph's *aggregate* demand;
    // this is the per-pair question, and the two are independent — a graph can
    // fit every ceiling and still promise a reader more than its writer offers.
    //
    // Refused at admission rather than reported. C8.5 treats an incompatible
    // pair as a runtime event a live fabric surfaces, which is why
    // `boot_contracts` makes it a query rather than a decode error — but this
    // root has no C8.5 plane to surface it on, so a graph it cannot honour
    // fails closed instead of launching participants that will never match.
    // When P5.4.5 brings the QoS plane, this becomes the wrong answer and
    // moves; recorded here so that is a decision rather than a discovery.
    //
    // The structural siblings need no call: `FabricGraph::decode` already runs
    // `validate_participants` and `validate_interposition`, so route-membership
    // counts, chain termination, hop revisits, and self-bypass are enforced
    // before this function is reached.
    if graph.all_pairs_qos_compatible() {
        Ok(())
    } else {
        Err(GenerationError::UnsatisfiableFabricGraph)
    }
}

/// Validate a declared fabric graph against this root's own ceilings (C8.2),
/// reporting whether one was present.
///
/// A no-op for a generation that declares no graph, which is every `sel4-*`
/// fixture but the stream plane's. That is deliberate rather than incidental:
/// the check must be silent on graphs that make no fabric promise, and refuse
/// exactly those whose promises this mechanism cannot keep. The `bool` is what
/// lets a boot marker distinguish "checked and satisfiable" from "nothing to
/// check", which is the difference a gate needs to see.
fn fabric_graph_admission(generation: &Generation<'_>) -> Result<bool, GenerationError> {
    let Some(graph) = fabric_graph_object(generation) else {
        return Ok(false);
    };
    // A graph that will not decode is refused here rather than surfaced as a
    // decode error, because the generation is otherwise well formed: it is the
    // graph that is wrong, and the caller's marker should say so.
    let graph = graph.map_err(|_| GenerationError::UnsatisfiableFabricGraph)?;
    fabric_graph_is_satisfiable(&graph)?;
    Ok(true)
}

/// Locate the fabric-graph resource object, if the generation declares one.
///
/// The same shape as the retired kernel's: a `KIND_RESOURCE` object whose
/// payload carries the fabric-graph magic, first match wins.
fn fabric_graph_object<'a>(
    generation: &Generation<'a>,
) -> Option<Result<FabricGraph<'a>, fabric_graph::DecodeError>> {
    for index in 0..generation.object_count() {
        let object = generation.object(index).ok()?;
        if object.kind == KIND_RESOURCE
            && object.bytes.len() >= fabric_graph::MAGIC.len()
            && object.bytes[..fabric_graph::MAGIC.len()] == fabric_graph::MAGIC
        {
            return Some(FabricGraph::decode(object.bytes));
        }
    }
    None
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
    /// Whether the generation declared a fabric graph, and this root checked
    /// it against its own ceilings (C8.2).
    ///
    /// Reported so the *wiring* is observable, not only the predicate. The
    /// predicate has unit tests; that the admission path consults it at all is
    /// what a boot marker can show and a unit test over a hand-built graph
    /// cannot.
    pub fabric_graph_admitted: bool,
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

        // C8.2: a declared fabric graph is validated against *this* root's own
        // ceilings before any component launches, so a graph promising more
        // than the mechanism can deliver fails the whole generation closed
        // rather than failing a participant halfway through a boot.
        //
        // The retired kernel does this in `kernel/src/runtime/generation.rs`
        // and `slime-root` did not, which P5.4.1's inventory recorded as C8.2
        // having no seL4 equivalent at all rather than a partial one. The
        // resource already rode along in every generation the builder emits;
        // nothing read it.
        //
        // The predicate itself is `boot_contracts`, shared byte-for-byte with
        // the oracle — only the ceilings differ, because they are this
        // implementation's rather than that one's. That is the whole point of
        // validating here as well: identical bytes can be satisfiable for one
        // mechanism and impossible for another.
        let fabric_graph_admitted = fabric_graph_admission(generation)?;

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
            fabric_graph_admitted,
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
    use super::{
        Authority, FabricGraph, GenerationError, PayloadFormat, RIGHT_RECV, RIGHT_SEND,
        fabric_graph_is_satisfiable,
    };
    use boot_contracts::component_image::wire;
    use boot_contracts::generation::RIGHT_TRANSFER;
    use boot_contracts::target_profile::TargetProfile;

    fn sel4_profile() -> &'static TargetProfile {
        TargetProfile::by_name("aarch64-sel4-qemu-virt").expect("declared profile")
    }

    /// Bytes an [`elf_header`] fixture occupies. Named so [`qualified`] sizes
    /// its tail from the same constant rather than from a literal.
    const ELF_TAIL: usize = wire::LEGACY_HEADER_LEN + 8;

    /// A bare ELF64 header prefix, padded past `LEGACY_HEADER_LEN`.
    ///
    /// The padding is load-bearing, and it is what B23's first host run
    /// surfaced. `classify` reaches its bare-ELF test only when
    /// `component_image::target` answers `BadMagic`; a blob shorter than
    /// `LEGACY_HEADER_LEN` (32) answers `Truncated` instead and falls straight
    /// to `Unrecognized`. These fixtures were 20 bytes and had been asserting
    /// against the wrong arm since `target` gained that guard — invisibly,
    /// because nothing compiled them.
    ///
    /// No production path is affected: every real ELF is far longer than 32
    /// bytes, and a payload that short is not one this root could load anyway.
    fn elf_header(class: u8, data: u8, machine: u16) -> [u8; ELF_TAIL] {
        let mut bytes = [0u8; ELF_TAIL];
        bytes[..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        bytes[4] = class;
        bytes[5] = data;
        bytes[18..20].copy_from_slice(&machine.to_le_bytes());
        bytes
    }

    /// A qualified component image for `profile`, under `magic`.
    ///
    /// The tail is sized from [`elf_header`] rather than by a literal, so
    /// widening that fixture cannot silently truncate this one. It was a
    /// literal `20`, and the mismatch was invisible while nothing compiled
    /// these tests (B23).
    fn qualified(magic: u64, profile: &TargetProfile) -> [u8; wire::HEADER_LEN + ELF_TAIL] {
        let mut bytes = [0u8; wire::HEADER_LEN + ELF_TAIL];
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

    /// A minimal well-formed fabric graph carrying `limits`, with no schemas,
    /// routes, participants, or interposition hops.
    ///
    /// Hand-built rather than borrowed from `boot_contracts`, whose encoder is
    /// `#[cfg(test)]` and so not reachable from here. The empty tables are what
    /// make it minimal: `validate_against` reads only the limits block, so an
    /// empty graph isolates the ceiling comparison from everything else the
    /// decoder checks.
    fn graph_with(limits: [u32; 19]) -> alloc::vec::Vec<u8> {
        use boot_contracts::fabric_graph::{FORMAT_VERSION, HEADER_BYTES, MAGIC};
        let mut bytes = alloc::vec![0u8; HEADER_BYTES];
        bytes[..8].copy_from_slice(&MAGIC);
        bytes[8..12].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes[12..16].copy_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        bytes[24..28].copy_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        // The fabric component's identity. Non-zero or the decoder answers
        // `MissingReference`: a graph naming no fabric describes no plane.
        bytes[48..80].copy_from_slice(&[0xab; 32]);
        for (index, value) in limits.iter().enumerate() {
            bytes[80 + index * 4..][..4].copy_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    /// A one-route stream graph with a publisher and a subscriber, each
    /// carrying the reliability the caller names.
    ///
    /// Enough structure to reach `all_pairs_qos_compatible`, which
    /// [`graph_with`]'s empty participant table cannot: with no pairs the
    /// predicate is vacuously true, so a graph built there proves nothing about
    /// QoS either way.
    fn qos_graph(offered: u8, requested: u8) -> alloc::vec::Vec<u8> {
        use boot_contracts::fabric_graph::{
            CONTRACT_KIND_STREAM, DIRECTION_PUBLISH, DIRECTION_SUBSCRIBE, DURABILITY_VOLATILE,
            FORMAT_VERSION, HEADER_BYTES, INTERPOSITION_NONE, LIVELINESS_AUTOMATIC, MAGIC,
            PARTICIPANT_ENTRY_BYTES, ROUTE_ENTRY_BYTES, SCHEMA_ENTRY_BYTES, VISIBILITY_GRAPH,
            grant_identity, route_identity,
        };
        // Route and grant identities are derived hashes, not arbitrary bytes:
        // the decoder recomputes and compares them, so a hand-built graph must
        // fold them the same way. That check is what makes an identity name one
        // exact (route, component, direction) tuple rather than a label.
        const SCHEMA_IDENTITY: [u8; 32] = [0x11; 32];
        let route_id = route_identity("telemetry", &SCHEMA_IDENTITY, CONTRACT_KIND_STREAM);
        let total =
            HEADER_BYTES + SCHEMA_ENTRY_BYTES + ROUTE_ENTRY_BYTES + 2 * PARTICIPANT_ENTRY_BYTES;
        let mut bytes = alloc::vec![0u8; total];
        bytes[..8].copy_from_slice(&MAGIC);
        bytes[8..12].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes[12..16].copy_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        bytes[24..28].copy_from_slice(&(total as u32).to_le_bytes());
        bytes[28..32].copy_from_slice(&1u32.to_le_bytes()); // schemas
        bytes[32..36].copy_from_slice(&1u32.to_le_bytes()); // routes
        bytes[36..40].copy_from_slice(&2u32.to_le_bytes()); // participants
        bytes[48..80].copy_from_slice(&[0xab; 32]);
        for (index, value) in SATISFIABLE.iter().enumerate() {
            bytes[80 + index * 4..][..4].copy_from_slice(&value.to_le_bytes());
        }

        let schema = HEADER_BYTES;
        bytes[schema..schema + 32].copy_from_slice(&SCHEMA_IDENTITY);
        bytes[schema + 32..schema + 40].copy_from_slice(&0xAAAAu64.to_le_bytes());
        bytes[schema + 40..schema + 44].copy_from_slice(&CONTRACT_KIND_STREAM.to_le_bytes());
        bytes[schema + 44..schema + 48].copy_from_slice(&64u32.to_le_bytes());

        let route = schema + SCHEMA_ENTRY_BYTES;
        bytes[route..route + 32].copy_from_slice(&route_id);
        bytes[route + 32..route + 36].copy_from_slice(&0u32.to_le_bytes()); // schema index
        bytes[route + 36..route + 40].copy_from_slice(&CONTRACT_KIND_STREAM.to_le_bytes());
        bytes[route + 40..route + 44].copy_from_slice(&2u32.to_le_bytes()); // participants

        // Grant identities must be distinct and sorted: the decoder rejects a
        // duplicate, and `encode` sorts, so a hand-built graph must too.
        let mut placed = [([0u8; 32], [0u8; 32], 0u32, 0u8); 2];
        for (slot, (component, direction, reliability)) in [
            ([0x41u8; 32], DIRECTION_PUBLISH, offered),
            ([0x42u8; 32], DIRECTION_SUBSCRIBE, requested),
        ]
        .into_iter()
        .enumerate()
        {
            placed[slot] = (
                grant_identity(&route_id, &component, direction),
                component,
                direction,
                reliability,
            );
        }
        // The decoder requires grant identities in ascending order, as the
        // builder's own encoder sorts them.
        placed.sort_by_key(|entry| entry.0);
        for (slot, (identity, component, direction, reliability)) in placed.into_iter().enumerate()
        {
            let at = route + ROUTE_ENTRY_BYTES + slot * PARTICIPANT_ENTRY_BYTES;
            bytes[at..at + 32].copy_from_slice(&identity);
            bytes[at + 32..at + 64].copy_from_slice(&component);
            bytes[at + 64..at + 68].copy_from_slice(&0u32.to_le_bytes()); // route
            bytes[at + 68..at + 72].copy_from_slice(&direction.to_le_bytes());
            bytes[at + 72..at + 76].copy_from_slice(&VISIBILITY_GRAPH.to_le_bytes());
            bytes[at + 76..at + 80].copy_from_slice(&INTERPOSITION_NONE.to_le_bytes());
            bytes[at + 104..at + 108].copy_from_slice(&1u32.to_le_bytes()); // history
            bytes[at + 112] = reliability;
            bytes[at + 113] = DURABILITY_VOLATILE as u8;
            bytes[at + 114] = LIVELINESS_AUTOMATIC as u8;
        }
        bytes
    }

    /// Limits every ceiling admits. Field order is the wire order:
    /// routes, ingress_sources, publishers, subscribers, clients, servers,
    /// sample_bytes, queue_depth, history_depth, event_depth, retained_samples,
    /// retries, in_flight_calls, in_flight_operations, buffer_pages, buffers,
    /// mappings, loans, capability_slots.
    const SATISFIABLE: [u32; 19] = [1, 4, 1, 1, 0, 0, 64, 8, 4, 4, 2, 2, 0, 0, 8, 2, 4, 4, 16];

    /// C8.2's exit condition on this side: a graph the root can satisfy is
    /// admitted, so the check is not refusing everything.
    #[test]
    fn a_satisfiable_fabric_graph_is_admitted() {
        let bytes = graph_with(SATISFIABLE);
        let graph = FabricGraph::decode(&bytes).expect("well-formed graph");
        assert_eq!(fabric_graph_is_satisfiable(&graph), Ok(()));
    }

    /// A matched pair whose offer satisfies the request is admitted, so the
    /// QoS half of the check is not refusing every graph that has pairs at all.
    #[test]
    fn a_compatible_qos_pair_is_admitted() {
        use boot_contracts::fabric_graph::RELIABILITY_RELIABLE;
        let bytes = qos_graph(RELIABILITY_RELIABLE as u8, RELIABILITY_RELIABLE as u8);
        let graph = FabricGraph::decode(&bytes).expect("well-formed graph");
        assert_eq!(fabric_graph_is_satisfiable(&graph), Ok(()));
    }

    /// P5.4.10: a graph within every ceiling but promising a reader more than
    /// its writer offers is refused. This is the assertion
    /// `kernel/tests/fabric_manifest.rs` makes over the booted graph and no
    /// seL4 gate made — `validate_against` bounds aggregate demand and says
    /// nothing about per-pair compatibility, so the two are independent.
    ///
    /// A BEST_EFFORT writer against a RELIABLE reader: the reader asks for
    /// delivery the writer never promised.
    #[test]
    fn an_incompatible_qos_pair_is_refused_within_every_ceiling() {
        use boot_contracts::fabric_graph::{RELIABILITY_BEST_EFFORT, RELIABILITY_RELIABLE};
        let bytes = qos_graph(RELIABILITY_BEST_EFFORT as u8, RELIABILITY_RELIABLE as u8);
        let graph = FabricGraph::decode(&bytes).expect("well-formed graph");
        assert!(
            !graph.all_pairs_qos_compatible(),
            "the fixture must actually declare an incompatible pair",
        );
        assert_eq!(
            fabric_graph_is_satisfiable(&graph),
            Err(GenerationError::UnsatisfiableFabricGraph),
        );
    }

    /// The other half, and the one that matters: no graph exceeding a ceiling
    /// this root owns is ever admitted. Exercised one field at a time so a
    /// ceiling wired to the wrong constant is visible rather than masked by a
    /// sibling that happens to refuse.
    ///
    /// Either refusal point counts. Some of these values are also structurally
    /// impossible against the format's *own* maxima, so the decoder rejects
    /// them before `validate_against` is reached — `ingress_sources` is one:
    /// `MAX_WAIT_SOURCES + 1` exceeds the wire format's ceiling too. What must
    /// hold is that no such graph reaches a running fabric, not which of the
    /// two guards catches it, and asserting on the guard would make this test
    /// fail if the format's maxima ever moved independently.
    #[test]
    fn no_graph_exceeding_a_ceiling_is_admitted() {
        // (index into the limits block, a value past this root's ceiling)
        let over = [
            (1, crate::ipc::MAX_WAIT_SOURCES as u32 + 1),
            (7, crate::ipc::CHANNEL_CAPACITY as u32 + 1),
            (14, crate::shared_buffer::MAX_TOTAL_PAGES as u32 + 1),
            (15, crate::shared_buffer::MAX_SHARED_BUFFERS as u32 + 1),
            (16, crate::shared_buffer::MAX_MAPPINGS as u32 + 1),
            (17, crate::shared_buffer::MAX_LOANS as u32 + 1),
            (18, crate::graph::MAX_TASK_CAPS as u32 + 1),
        ];
        for (index, value) in over {
            let mut limits = SATISFIABLE;
            limits[index] = value;
            let bytes = graph_with(limits);
            let admitted = FabricGraph::decode(&bytes)
                .is_ok_and(|graph| fabric_graph_is_satisfiable(&graph).is_ok());
            assert!(
                !admitted,
                "limit {index} at {value} exceeds this root's ceiling and was admitted",
            );
        }
    }

    /// A graph is refused for contradicting itself, not only for exceeding a
    /// ceiling: the fabric brokers one loan and one mapping per matched
    /// subscriber, so promising more subscribers than either cannot be
    /// delivered however small the numbers are.
    #[test]
    fn a_self_contradicting_graph_is_refused_within_every_ceiling() {
        let mut limits = SATISFIABLE;
        limits[3] = 8; // subscribers
        limits[16] = 4; // mappings
        limits[17] = 4; // loans
        let bytes = graph_with(limits);
        let graph = FabricGraph::decode(&bytes).expect("well-formed graph");
        assert_eq!(
            fabric_graph_is_satisfiable(&graph),
            Err(GenerationError::UnsatisfiableFabricGraph),
        );
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

    /// A segment-carrying image is unloadable here whatever it targets, so its
    /// target is not consulted. This is what keeps a retained x86 generation
    /// reported as the Slime component images it holds — P5.1's
    /// `slimecm=[1-9]\d*` evidence — rather than as wrong-target artifacts.
    #[test]
    fn segment_carrying_images_are_recognized_without_consulting_their_target() {
        let x86 = TargetProfile::legacy().expect("legacy profile");
        let mut v1 = [0u8; wire::LEGACY_HEADER_LEN];
        v1[wire::OFF_HEADER_MAGIC..][..8].copy_from_slice(&wire::LEGACY_IMAGE_MAGIC.to_le_bytes());
        v1[wire::OFF_HEADER_FORMAT_VERSION..][..4]
            .copy_from_slice(&wire::LEGACY_FORMAT_VERSION.to_le_bytes());
        v1[wire::OFF_HEADER_HEADER_SIZE..][..4]
            .copy_from_slice(&(wire::LEGACY_HEADER_LEN as u32).to_le_bytes());
        for profile in [x86, sel4_profile()] {
            let format = PayloadFormat::classify(&v1, profile);
            assert_eq!(format, PayloadFormat::SlimeComponent);
            assert!(!format.is_loadable());
        }
        // The same holds for a v2 image built for another architecture.
        let board = TargetProfile::by_name("aarch64-rpi5").expect("declared profile");
        assert_eq!(
            PayloadFormat::classify(&qualified(wire::IMAGE_MAGIC, board), sel4_profile()),
            PayloadFormat::SlimeComponent
        );
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
        // The band between `classify`'s own 20-byte discriminator and
        // `LEGACY_HEADER_LEN` (32): long enough to answer every class, data,
        // and machine test, short enough that `component_image::target`
        // answers `Truncated` and never yields `BadMagic`. A well-formed
        // AArch64 ELF *prefix* in that band must still be unrecognized.
        //
        // Pinned explicitly because widening `elf_header` past the guard is
        // what made the two tests above it meaningful, and would otherwise
        // have vacated this band entirely — removing the guard or moving
        // `LEGACY_HEADER_LEN` would then leave the suite green.
        let elf = elf_header(2, 1, 183);
        assert_eq!(
            PayloadFormat::classify(&elf[..wire::LEGACY_HEADER_LEN - 1], sel4_profile()),
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
