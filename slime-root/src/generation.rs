//! Bounded generation preflight and authority derivation for `slime-root`.
//!
//! Admission is a pure decision over decoded v4 generation data: it classifies
//! executable payloads and preserves the explicit instance launch model.
//! Initial authority comes only from per-instance bindings; executables are
//! never inferred to be instances.

use boot_contracts::component_image::{self, ComponentTargetError};
use boot_contracts::fabric_graph::{self, FabricGraph};
use boot_contracts::generation::{
    DecodeError, Generation, Instance, InstanceBinding, KIND_BOOTSTRAP, KIND_COMPONENT,
    KIND_RESOURCE, RIGHT_TRANSFER, ResourceQuota, Rights,
};
use boot_contracts::target_profile::TargetProfile;

pub const MAX_ADMITTED_EXECUTABLES: usize = 48;
pub const MAX_ADMITTED_INSTANCES: usize = 48;

/// Logical IPC rights, numbered by the generation contract
/// (`contracts/generation/v1/schema.zt`, rendered by
/// `scripts/build/build-generation.py`). The generation format owns
/// `RIGHT_TRANSFER`; the send/receive bits are restated here because
/// `slime-root` maps them onto seL4 endpoint rights.
pub const RIGHT_SEND: Rights = 1;
pub const RIGHT_RECV: Rights = 1 << 1;
/// Authority to execute an object: what makes a grant name a spawnable
/// executable rather than a channel or a factory.
pub const RIGHT_EXEC: Rights = 1 << 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GenerationError {
    Decode(DecodeError),
    UnsupportedGenerationVersion,
    MalformedClosure {
        bootstrap: usize,
        executables: usize,
    },
    /// A declared resource quota exceeds what the root can actually place
    /// (B49). Named per class so a refusal says which ceiling was hit.
    /// The plan's object counts, summed across every process, exceed the root
    /// CSlots available to hold capabilities to them (B49).
    PlanExceedsRootSlots {
        required: usize,
        available: usize,
    },
    QuotaExceedsCeiling {
        instance: usize,
        kind: &'static str,
        declared: u32,
        limit: u32,
    },
    TooManyExecutables {
        declared: usize,
        limit: usize,
    },
    TooManyInstances {
        declared: usize,
        limit: usize,
    },
    DanglingExecutable {
        executable: usize,
    },
    BareElfPayload {
        executable: usize,
    },
    /// The generation carries a fabric graph whose declared limits this root
    /// cannot satisfy, or which contradicts itself (C8.2).
    ///
    /// Distinct from [`Self::Decode`] because the bytes are well formed: the
    /// graph says what it needs and the answer is no. The whole generation
    /// fails closed, before the fabric or any participant launches.
    UnsatisfiableFabricGraph,
    /// The fabric graph names a participant this generation does not declare
    /// as a component (C8.3).
    ///
    /// Distinct from [`Self::UnsatisfiableFabricGraph`]: the graph is
    /// internally consistent and fits every ceiling, but it promises an edge
    /// to a component that will never exist, so the promise cannot be kept by
    /// any mechanism.
    UndeclaredFabricParticipant,
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
    /// A bare AArch64 little-endian ELF64 image carrying no target
    /// qualification. Recognized for diagnostics, but never loadable from a
    /// generation: only target-qualified component images cross admission.
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
        matches!(self, Self::QualifiedElf)
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutablePlan {
    pub executable: usize,
    pub format: PayloadFormat,
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

pub fn bound_authority(
    generation: &Generation<'_>,
    instance: Instance<'_>,
) -> Result<Authority, GenerationError> {
    let instance_index = (0..generation.instance_count())
        .find(|index| {
            generation
                .instance(*index)
                .is_ok_and(|candidate| candidate.name == instance.name)
        })
        .ok_or(GenerationError::Decode(DecodeError::BadIndex))?;
    let mut authority = Authority::NONE;
    for index in 0..instance.binding_count() {
        let InstanceBinding { grant, .. } = generation.binding(instance, index)?;
        let grant = generation.grant(grant)?;
        if generation.grant_applies_to_instance(grant, instance_index) {
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
            u32::MAX,
        )
        .map_err(|_| GenerationError::UnsatisfiableFabricGraph)?;
    // Every matched pair's offered QoS must satisfy the requested one
    // (P5.4.10). `validate_against` bounds the graph's *aggregate* demand;
    // this is the per-pair question, and the two are independent — a graph can
    // fit every ceiling and still promise a reader more than its writer offers.
    //
    // Queue depth is deliberately not a root ceiling after B46. Buffered
    // streams use the generation-provisioned v2 shared ring, while native
    // Endpoint rendezvous has no root-owned queue. The graph may still declare
    // its policy depth; only the component provisioning path must honour it.
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
/// fixture but the stream plane's — though *not* the retained x86 generation
/// the P5.1 fixture variant boots, which carries a larger graph than any
/// `sel4-*` fixture and the only interposition hop of the set. That is
/// deliberate rather than incidental: the check must be silent on graphs that
/// make no fabric promise, and refuse exactly those whose promises this
/// mechanism cannot keep.
///
/// `None` versus `Some(shape)` is what lets a boot marker distinguish
/// "checked and satisfiable" from "nothing to check", which is the difference
/// a gate needs to see; the shape itself is C8.4's structural arm.
fn fabric_graph_admission(
    generation: &Generation<'_>,
) -> Result<Option<FabricShape>, GenerationError> {
    let Some(graph) = fabric_graph_object(generation) else {
        return Ok(None);
    };
    // A graph that will not decode is refused here rather than surfaced as a
    // decode error, because the generation is otherwise well formed: it is the
    // graph that is wrong, and the caller's marker should say so.
    let graph = graph.map_err(|_| GenerationError::UnsatisfiableFabricGraph)?;
    fabric_graph_is_satisfiable(&graph)?;
    fabric_graph_participants_are_declared(generation, &graph)?;
    Ok(Some(FabricShape {
        schemas: graph.schema_count(),
        routes: graph.route_count(),
        participants: graph.participant_count(),
        interpositions: graph.interposition_count(),
        capability_slots: graph.limits().capability_slots,
    }))
}

/// Every participant in the graph must name a component this generation
/// declares (C8.3).
///
/// The graph identifies participants by a hash of the component name, and the
/// fabric service answers requests from a *build-time* table generated from the
/// same manifest. Nothing checked that the two agreed: a graph naming a
/// component the generation had since dropped would decode, satisfy every
/// ceiling, and launch — and the mismatch would surface only as a participant
/// whose control endpoint never arrives, which reads as a hang rather than as
/// the provenance failure it is.
///
/// Checked in the direction that can actually be wrong. A component with no
/// participant is ordinary (most components are not on the fabric); a
/// participant with no component is a graph promising an edge to nobody.
fn fabric_graph_participants_are_declared(
    generation: &Generation<'_>,
    graph: &FabricGraph<'_>,
) -> Result<(), GenerationError> {
    let declared = generation.instance_count();
    if declared > MAX_ADMITTED_INSTANCES {
        return Err(GenerationError::TooManyInstances {
            declared,
            limit: MAX_ADMITTED_INSTANCES,
        });
    }
    let mut names = [None; MAX_ADMITTED_INSTANCES];
    for (slot, name) in names.iter_mut().enumerate().take(declared) {
        *name = Some(generation.instance(slot)?.name);
    }
    participants_are_declared(&names[..declared], graph)
}

/// The set half of [`fabric_graph_participants_are_declared`], over the
/// component names rather than the generation that carries them.
///
/// Split out so the property is testable: building a whole `Generation` by
/// hand would make the fixture the thing under test.
fn participants_are_declared(
    names: &[Option<&str>],
    graph: &FabricGraph<'_>,
) -> Result<(), GenerationError> {
    let declares = |identity: [u8; 32]| {
        names
            .iter()
            .flatten()
            .any(|name| fabric_graph::component_identity(name) == identity)
    };
    // The fabric host itself. `decode` only rejects an all-zero value, and a
    // graph naming a host the manifest dropped fits every ceiling — but no
    // participant would receive anything, because the host that mints every
    // route half does not exist. Checked first: it is the failure that
    // disables the whole graph rather than one edge of it.
    if !declares(graph.fabric_component_identity()) {
        return Err(GenerationError::UndeclaredFabricParticipant);
    }
    for index in 0..graph.participant_count() {
        let participant = graph
            .participant(index)
            .ok_or(GenerationError::UnsatisfiableFabricGraph)?;
        if !declares(participant.component_identity) {
            return Err(GenerationError::UndeclaredFabricParticipant);
        }
    }
    // Interposition hops. `validate_interposition` checks chain termination,
    // revisits, and self-bypass, never membership — and a hop is a *mandatory*
    // proxy on its route, so a dropped one silently breaks the route it was
    // added to mediate. The retained generation this root boots by default
    // carries one, so this arm is exercised on every fixture-variant boot.
    for index in 0..graph.interposition_count() {
        let hop = graph
            .interposition(index)
            .ok_or(GenerationError::UnsatisfiableFabricGraph)?;
        if !declares(hop.component_identity) {
            return Err(GenerationError::UndeclaredFabricParticipant);
        }
    }
    Ok(())
}

/// The shape a declared fabric graph fixes. See [`Admission::fabric_schemas`].
#[derive(Clone, Copy)]
struct FabricShape {
    schemas: usize,
    routes: usize,
    participants: usize,
    interpositions: usize,
    /// The graph's declared per-child capability-slot ceiling (C8.13.3).
    capability_slots: u32,
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

/// The result of admitting a v5 generation graph.
pub struct Admission {
    executable_plans: [Option<ExecutablePlan>; MAX_ADMITTED_EXECUTABLES],
    executable_len: usize,
    instance_indices: [usize; MAX_ADMITTED_INSTANCES],
    instance_len: usize,
    pub bootstrap_instance: usize,
    pub bootstrap_objects: usize,
    pub component_objects: usize,
    pub grants: usize,
    pub health: usize,
    pub loadable: usize,
    pub slime_component_images: usize,
    pub unrecognized_images: usize,
    pub wrong_target_images: usize,
    pub fabric_graph_admitted: bool,
    pub fabric_schemas: usize,
    pub fabric_routes: usize,
    pub fabric_participants: usize,
    pub fabric_interpositions: usize,
    /// The declared `capabilitySlots` ceiling, or 0 when this generation
    /// declares no fabric graph (C8.13.3).
    ///
    /// Zero always means "no ceiling to report against". A graph that declares
    /// the field may itself declare zero — the decoder bounds it only from
    /// above — so `crate::cspace::breaches_ceiling` treats both cases alike
    /// rather than distinguishing an absent graph from a permissive one.
    pub fabric_capability_slots: u32,
}

impl Admission {
    pub fn admit(
        generation: &Generation<'_>,
        profile: &TargetProfile,
    ) -> Result<Self, GenerationError> {
        if !generation.is_v5() {
            return Err(GenerationError::UnsupportedGenerationVersion);
        }
        let executable_len = generation.executable_count();
        if executable_len > MAX_ADMITTED_EXECUTABLES {
            return Err(GenerationError::TooManyExecutables {
                declared: executable_len,
                limit: MAX_ADMITTED_EXECUTABLES,
            });
        }
        let instance_len = generation.instance_count();
        if instance_len > MAX_ADMITTED_INSTANCES {
            return Err(GenerationError::TooManyInstances {
                declared: instance_len,
                limit: MAX_ADMITTED_INSTANCES,
            });
        }
        // B49: the object plan is proven to fit before anything activates.
        //
        // The quota record says how many CNodes, TCBs, endpoints, frames, and
        // CSlots each process needs. Checking it here rather than discovering
        // it during construction is the difference between a graph refused
        // whole and one that half-activates and then fails to place a
        // capability -- at which point some children are already running.
        //
        // Per class, not as a total: a plan that needs one CSlot too many is a
        // different defect from one that needs an extra TCB, and a single
        // "too big" would say neither.
        for index in 0..generation.resource_quota_count() {
            admit_resource_quota(&generation.resource_quota(index)?)?;
        }
        let fabric = fabric_graph_admission(generation)?;
        let mut bootstrap_objects = 0;
        let mut component_objects = 0;
        for index in 0..generation.object_count() {
            match generation.object(index)?.kind {
                KIND_BOOTSTRAP => bootstrap_objects += 1,
                KIND_COMPONENT => component_objects += 1,
                _ => {}
            }
        }
        if bootstrap_objects == 0 || executable_len == 0 {
            return Err(GenerationError::MalformedClosure {
                bootstrap: bootstrap_objects,
                executables: executable_len,
            });
        }

        let mut executable_plans = [None; MAX_ADMITTED_EXECUTABLES];
        let mut loadable = 0;
        let mut slime_component_images = 0;
        let mut unrecognized_images = 0;
        let mut wrong_target_images = 0;
        for (executable, plan) in executable_plans.iter_mut().enumerate().take(executable_len) {
            let record = generation.executable(executable)?;
            let object = generation
                .object(record.object)
                .map_err(|_| GenerationError::DanglingExecutable { executable })?;
            let format = PayloadFormat::classify(object.bytes, profile);
            match format {
                PayloadFormat::QualifiedElf => loadable += 1,
                PayloadFormat::Aarch64Elf => {
                    return Err(GenerationError::BareElfPayload { executable });
                }
                PayloadFormat::SlimeComponent => slime_component_images += 1,
                PayloadFormat::WrongTarget => wrong_target_images += 1,
                PayloadFormat::Unrecognized => unrecognized_images += 1,
            }
            *plan = Some(ExecutablePlan { executable, format });
        }
        let mut instance_indices = [0; MAX_ADMITTED_INSTANCES];
        for (index, slot) in instance_indices.iter_mut().enumerate().take(instance_len) {
            generation.instance(index)?;
            *slot = index;
        }
        for index in 0..generation.grant_count() {
            generation.grant(index)?;
        }
        for index in 0..generation.health_count() {
            generation.health_instance(index)?;
        }

        Ok(Self {
            executable_plans,
            executable_len,
            instance_indices,
            instance_len,
            bootstrap_instance: generation.bootstrap_instance,
            bootstrap_objects,
            component_objects,
            grants: generation.grant_count(),
            health: generation.health_count(),
            loadable,
            slime_component_images,
            unrecognized_images,
            wrong_target_images,
            fabric_graph_admitted: fabric.is_some(),
            fabric_schemas: fabric.map_or(0, |shape| shape.schemas),
            fabric_routes: fabric.map_or(0, |shape| shape.routes),
            fabric_participants: fabric.map_or(0, |shape| shape.participants),
            fabric_interpositions: fabric.map_or(0, |shape| shape.interpositions),
            fabric_capability_slots: fabric.map_or(0, |shape| shape.capability_slots),
        })
    }

    pub const fn executable_len(&self) -> usize {
        self.executable_len
    }
    pub const fn instance_len(&self) -> usize {
        self.instance_len
    }
    pub const fn is_empty(&self) -> bool {
        self.instance_len == 0
    }
    pub fn executable_plans(&self) -> impl Iterator<Item = &ExecutablePlan> {
        self.executable_plans
            .iter()
            .take(self.executable_len)
            .flatten()
    }
    pub fn loadable_executables(&self) -> impl Iterator<Item = &ExecutablePlan> {
        self.executable_plans()
            .filter(|plan| plan.format.is_loadable())
    }
    pub fn executable_plan(&self, executable: usize) -> Option<&ExecutablePlan> {
        self.executable_plans
            .get(executable)
            .and_then(Option::as_ref)
    }
    pub fn instances<'a>(
        &'a self,
        generation: &'a Generation<'a>,
    ) -> impl Iterator<Item = Instance<'a>> + 'a {
        self.instance_indices[..self.instance_len]
            .iter()
            .filter_map(|index| generation.instance(*index).ok())
    }
    pub fn root_autostart_instances<'a>(
        &'a self,
        generation: &'a Generation<'a>,
    ) -> impl Iterator<Item = Instance<'a>> + 'a {
        self.instances(generation)
            .filter(|instance| instance.is_root_autostart())
    }
}

/// Root CSlots consumed per object a process declares.
///
/// Measured, not derived: the 48-instance stress plane consumed 3186 slots
/// constructing 39 instances whose quotas declared 33 objects each. The excess
/// is intermediate page tables, window aliases, and arena parent untypeds --
/// root-side costs that belong to no child's plan.
const ROOT_SLOTS_PER_DECLARED_OBJECT: usize = 3;

/// Refuse a plan whose quotas, summed, exceed the root CSlots available
/// (B49).
///
/// The per-instance ceilings say each process fits on its own; this says they
/// all fit together. Without it a 48-instance graph admits and then dies
/// mid-construction with children already running, which is exactly the
/// failure admission exists to prevent — observed as
/// `VSpace(Alloc(SlotsExhausted))` at instance 39 of the stress plane.
///
/// The root holds a capability per object it creates for a child, so a
/// process's root-side cost is its own object count; the CSlots inside the
/// child's CNode are the child's, carved from the arena rather than the
/// root's pool.
pub fn admit_total_slots(
    generation: &Generation<'_>,
    available: usize,
) -> Result<usize, GenerationError> {
    let mut required = 0usize;
    for index in 0..generation.resource_quota_count() {
        let quota = generation.resource_quota(index)?;
        let per_process = (quota.cnode_count
            + quota.tcb_count
            + quota.endpoint_count
            + quota.notification_count
            + quota.frame_count
            + quota.page_table_count) as usize;
        required = required.saturating_add(per_process);
    }
    // Each declared object costs at least one root CSlot, and in practice
    // more: intermediate page tables the loader creates, the window alias, and
    // the arena's parent untyped are root-side costs no per-process quota
    // names. Measured on the 48-instance stress plane, construction consumed
    // 81 slots per instance against 33 declared objects.
    //
    // The factor is deliberately a measured constant rather than a model of
    // every source: a model that claimed precision it does not have would
    // admit graphs that then die mid-construction, which is the failure this
    // check exists to prevent. Refusing a graph that would have fit is
    // recoverable; admitting one that does not is not.
    let required = required.saturating_mul(ROOT_SLOTS_PER_DECLARED_OBJECT);
    if required > available {
        return Err(GenerationError::PlanExceedsRootSlots {
            required,
            available,
        });
    }
    Ok(required)
}

/// Refuse a declared quota that exceeds what the root can actually place
/// (B49).
///
/// Per class, not as a total: a plan needing one CSlot too many is a different
/// defect from one needing an extra TCB, and a single "too big" would say
/// neither. Checked at admission rather than during construction, which is the
/// difference between a graph refused whole and one that half-activates and
/// then cannot place a capability, with children already running.
fn admit_resource_quota(quota: &ResourceQuota<'_>) -> Result<(), GenerationError> {
    for (kind, declared, limit) in [
        (
            "cslot",
            quota.cslot_count,
            (1u32 << crate::task::CHILD_CNODE_SIZE_BITS),
        ),
        // One TCB and one IPC-buffer frame per thread, bounded by what
        // the child VSpace maps buffer/window pairs for (B47). A plan
        // asking for more would have a thread with no buffer.
        (
            "tcb",
            quota.tcb_count,
            crate::child_vspace::MAX_CHILD_THREADS as u32,
        ),
        (
            // One IPC-buffer/window pair per thread plus the image's own
            // pages, which the loader maps from root CSlots (B49).
            "frame",
            quota.frame_count,
            (crate::child_vspace::MAX_CHILD_THREADS + crate::child_vspace::MAX_CHILD_IMAGE_PAGES)
                as u32,
        ),
        ("cnode", quota.cnode_count, 1),
        // The child's VSpace root. One per process by definition -- a
        // second would be a second address space, which is a second
        // process.
        ("vspace", quota.page_table_count, 1),
        // Fault + console + one native endpoint per declared relative slot.
        // The plan still bounds the total by the child CSpace capacity above;
        // refusing at two would reject every process that has a real peer.
        (
            "endpoint",
            quota.endpoint_count,
            2 + crate::task::CHILD_NATIVE_REGION_SLOTS as u32,
        ),
    ] {
        if declared > limit {
            return Err(GenerationError::QuotaExceedsCeiling {
                instance: quota.owner_process,
                kind,
                declared,
                limit,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        Authority, FabricGraph, GenerationError, PayloadFormat, RIGHT_RECV, RIGHT_SEND,
        fabric_graph_is_satisfiable, participants_are_declared,
    };
    use boot_contracts::component_image::wire;
    use boot_contracts::generation::{RIGHT_TRANSFER, ResourceQuota};

    use super::admit_resource_quota;
    use crate::child_vspace::{MAX_CHILD_IMAGE_PAGES, MAX_CHILD_THREADS};

    /// The quota a single-threaded process declares: one CNode, one VSpace,
    /// one TCB, one IPC-buffer frame, two endpoints, and a full CNode of
    /// slots. Matches what `build-generation.py` packs.
    fn single_threaded_quota() -> ResourceQuota<'static> {
        ResourceQuota {
            name: "fixture:quota",
            owner_process: 0,
            cnode_count: 1,
            tcb_count: 1,
            endpoint_count: 2,
            notification_count: 0,
            frame_count: 1,
            page_table_count: 1,
            mapping_count: 0,
            irq_count: 0,
            cslot_count: 1u32 << crate::task::CHILD_CNODE_SIZE_BITS,
            untyped_bytes: 0,
            dynamic_reserve_bytes: 0,
            flags: 0,
        }
    }

    #[test]
    fn the_quota_a_single_threaded_process_declares_is_admitted() {
        admit_resource_quota(&single_threaded_quota())
            .expect("the plan the builder emits must be placeable");
    }

    #[test]
    fn a_two_thread_process_may_declare_a_tcb_and_frame_per_thread() {
        // B47: threads own a TCB and an IPC buffer each, so the ceiling is per
        // thread rather than one. A ceiling of 1 here would refuse every
        // multi-threaded process.
        let mut quota = single_threaded_quota();
        quota.tcb_count = 2;
        quota.frame_count = 2;
        admit_resource_quota(&quota).expect("two threads declare two of each");
    }

    #[test]
    fn one_object_over_any_ceiling_is_refused_naming_its_class() {
        // Every class the root places, each raised by exactly one. A ceiling
        // that admitted one over would let a graph activate and then fail to
        // place a capability, with children already running -- which is the
        // failure admission exists to prevent.
        let cases: [(&str, fn(&mut ResourceQuota<'_>), u32, u32); 6] = [
            ("cnode", |q| q.cnode_count = 2, 2, 1),
            ("tcb", |q| q.tcb_count = 3, 3, 2),
            ("endpoint", |q| q.endpoint_count = 34, 34, 33),
            (
                "frame",
                |q| q.frame_count = (MAX_CHILD_THREADS + MAX_CHILD_IMAGE_PAGES + 1) as u32,
                (MAX_CHILD_THREADS + MAX_CHILD_IMAGE_PAGES + 1) as u32,
                (MAX_CHILD_THREADS + MAX_CHILD_IMAGE_PAGES) as u32,
            ),
            ("vspace", |q| q.page_table_count = 2, 2, 1),
            ("cslot", |q| q.cslot_count = 129, 129, 128),
        ];
        for (class, mutate, declared, limit) in cases {
            let mut quota = single_threaded_quota();
            mutate(&mut quota);
            match admit_resource_quota(&quota) {
                Err(GenerationError::QuotaExceedsCeiling {
                    instance,
                    kind,
                    declared: reported,
                    limit: reported_limit,
                }) => {
                    assert_eq!(kind, class, "the refusal must name the class it hit");
                    assert_eq!(instance, 0, "and the process whose plan it was");
                    assert_eq!(reported, declared);
                    assert_eq!(reported_limit, limit);
                }
                other => panic!("{class}: one over its ceiling was not refused: {other:?}"),
            }
        }
    }
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
    /// The helper writes every canonical admission field so mutations below
    /// isolate the field they intend to test.
    fn qualified(magic: u64, profile: &TargetProfile) -> [u8; wire::HEADER_LEN + ELF_TAIL] {
        let mut bytes = [0u8; wire::HEADER_LEN + ELF_TAIL];
        bytes[wire::OFF_HEADER_MAGIC..][..8].copy_from_slice(&magic.to_le_bytes());
        bytes[wire::OFF_HEADER_FORMAT_VERSION..][..4]
            .copy_from_slice(&wire::FORMAT_VERSION.to_le_bytes());
        bytes[wire::OFF_HEADER_HEADER_SIZE..][..4]
            .copy_from_slice(&(wire::HEADER_LEN as u32).to_le_bytes());
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

    /// The components `qos_graph` names: the fabric host and its two
    /// participants. Derived identities, not raw bytes, so a provenance test
    /// can declare the same components by name.
    const QOS_FABRIC: &str = "fabric-service";
    const QOS_PUBLISHER: &str = "fabric-publisher";
    const QOS_SUBSCRIBER: &str = "fabric-subscriber";

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
        // The fabric host, named rather than arbitrary: admission now checks
        // this identity against the generation too, so a raw constant would
        // make every graph built here unadmittable.
        bytes[48..80].copy_from_slice(&boot_contracts::fabric_graph::component_identity(
            QOS_FABRIC,
        ));
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
            (
                boot_contracts::fabric_graph::component_identity(QOS_PUBLISHER),
                DIRECTION_PUBLISH,
                offered,
            ),
            (
                boot_contracts::fabric_graph::component_identity(QOS_SUBSCRIBER),
                DIRECTION_SUBSCRIBE,
                requested,
            ),
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

    /// A two-schema graph whose schemas carry distinct full identities and the
    /// tags given (C8.1). No route or participant: `validate_schemas` runs
    /// before anything references a schema, so a collision is refusable on the
    /// schema table alone.
    ///
    /// The generation-local tag is a *lookup key*. Two distinct interfaces
    /// sharing one tag makes every later resolution ambiguous, and the wrong
    /// answer is a message decoded against the wrong schema rather than an
    /// error, which is why this is refused at decode rather than reported.
    fn two_schema_graph(first_tag: u64, second_tag: u64) -> alloc::vec::Vec<u8> {
        use boot_contracts::fabric_graph::{
            CONTRACT_KIND_STREAM, FORMAT_VERSION, HEADER_BYTES, MAGIC, SCHEMA_ENTRY_BYTES,
        };
        let total = HEADER_BYTES + 2 * SCHEMA_ENTRY_BYTES;
        let mut bytes = alloc::vec![0u8; total];
        bytes[..8].copy_from_slice(&MAGIC);
        bytes[8..12].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes[12..16].copy_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        bytes[24..28].copy_from_slice(&(total as u32).to_le_bytes());
        bytes[28..32].copy_from_slice(&2u32.to_le_bytes()); // schemas
        bytes[48..80].copy_from_slice(&[0xab; 32]);
        for (index, value) in SATISFIABLE.iter().enumerate() {
            bytes[80 + index * 4..][..4].copy_from_slice(&value.to_le_bytes());
        }
        // Schema identities must ascend: the decoder checks the ordering
        // separately from the tag collision, so a descending pair would fail
        // for the wrong reason and the test would prove nothing.
        for (slot, (identity, tag)) in [([0x11u8; 32], first_tag), ([0x22u8; 32], second_tag)]
            .into_iter()
            .enumerate()
        {
            let at = HEADER_BYTES + slot * SCHEMA_ENTRY_BYTES;
            bytes[at..at + 32].copy_from_slice(&identity);
            bytes[at + 32..at + 40].copy_from_slice(&tag.to_le_bytes());
            bytes[at + 40..at + 44].copy_from_slice(&CONTRACT_KIND_STREAM.to_le_bytes());
            bytes[at + 44..at + 48].copy_from_slice(&64u32.to_le_bytes());
        }
        bytes
    }

    /// Limits every ceiling admits. Field order is the wire order:
    /// routes, ingress_sources, publishers, subscribers, clients, servers,
    /// sample_bytes, queue_depth, history_depth, event_depth, retained_samples,
    /// retries, in_flight_calls, in_flight_operations, buffer_pages, buffers,
    /// mappings, loans, capability_slots.
    const SATISFIABLE: [u32; 19] = [1, 4, 1, 1, 0, 0, 64, 8, 4, 4, 2, 2, 0, 0, 8, 2, 4, 4, 16];

    /// C8.3 (P5.4.10): a graph may only name participants the generation
    /// declares as components.
    ///
    /// The fabric answers requests from a build-time table generated from the
    /// same manifest, and nothing checked the two still agreed. A graph naming
    /// a dropped component decodes, fits every ceiling, and launches; the
    /// mismatch then surfaces as a control endpoint that never arrives, which
    /// reads as a hang rather than as a provenance failure.
    #[test]
    fn a_graph_may_not_name_a_component_the_generation_lacks() {
        use boot_contracts::fabric_graph::RELIABILITY_RELIABLE;
        let bytes = qos_graph(RELIABILITY_RELIABLE as u8, RELIABILITY_RELIABLE as u8);
        let graph = FabricGraph::decode(&bytes).expect("well-formed graph");

        // Host and both participants declared: admitted. Without this half the
        // test would pass on a check that refused every graph.
        let complete = [
            Some(QOS_FABRIC),
            Some(QOS_PUBLISHER),
            Some(QOS_SUBSCRIBER),
            Some("init"),
        ];
        assert_eq!(participants_are_declared(&complete, &graph), Ok(()));

        // The subscriber dropped from the generation, its participant left in
        // the graph: refused before anything launches.
        let missing = [Some(QOS_FABRIC), Some(QOS_PUBLISHER), Some("init")];
        assert_eq!(
            participants_are_declared(&missing, &graph),
            Err(GenerationError::UndeclaredFabricParticipant)
        );

        // The fabric host itself dropped. Every participant is still declared,
        // so only the host arm can catch this — and nothing would receive a
        // route half if it booted.
        let hostless = [Some(QOS_PUBLISHER), Some(QOS_SUBSCRIBER), Some("init")];
        assert_eq!(
            participants_are_declared(&hostless, &graph),
            Err(GenerationError::UndeclaredFabricParticipant)
        );

        // A component no participant names is ordinary, not an error: most
        // components are not on the fabric at all.
        let extra = [
            Some(QOS_FABRIC),
            Some(QOS_PUBLISHER),
            Some(QOS_SUBSCRIBER),
            Some("console"),
            Some("sysinfo"),
        ];
        assert_eq!(participants_are_declared(&extra, &graph), Ok(()));
    }

    /// C8.1 (P5.4.10): two distinct interfaces may not share one
    /// generation-local type tag.
    ///
    /// The positive half is load-bearing. Without it this test would pass on a
    /// decoder that refused every two-schema graph, which is the failure mode
    /// a negative-only test cannot see.
    #[test]
    fn distinct_schemas_may_share_no_type_tag() {
        let distinct = two_schema_graph(0xAAAA, 0xBBBB);
        assert!(
            FabricGraph::decode(&distinct).is_ok(),
            "two schemas with distinct tags are a legal graph"
        );

        let collided = two_schema_graph(0xAAAA, 0xAAAA);
        assert!(
            matches!(
                FabricGraph::decode(&collided),
                Err(boot_contracts::fabric_graph::DecodeError::IdentityMismatch)
            ),
            "a shared type tag must be refused at decode, not resolved later"
        );
    }

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
    /// Queue depth is absent deliberately: after B46 it is a generation-owned
    /// ring property, not a root queue ceiling.
    #[test]
    fn no_graph_exceeding_a_ceiling_is_admitted() {
        // (index into the limits block, a value past this root's ceiling)
        let over = [
            (1, crate::ipc::MAX_WAIT_SOURCES as u32 + 1),
            // index 7 is queue depth: native Endpoint/ring provisioning owns it.
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
    fn malformed_qualified_elf_headers_are_not_loadable() {
        let profile = sel4_profile();
        let mut wrong_abi = qualified(wire::ELF_IMAGE_MAGIC, profile);
        wrong_abi[wire::OFF_HEADER_KERNEL_ABI..][..4]
            .copy_from_slice(&(wire::KERNEL_ABI_VERSION + 1).to_le_bytes());
        let mut bad_stack = qualified(wire::ELF_IMAGE_MAGIC, profile);
        bad_stack[wire::OFF_HEADER_STACK_BYTES..][..4].copy_from_slice(&0u32.to_le_bytes());
        let mut nonzero_entry = qualified(wire::ELF_IMAGE_MAGIC, profile);
        nonzero_entry[wire::OFF_HEADER_ENTRY_OFFSET..][..4].copy_from_slice(&1u32.to_le_bytes());
        let mut nonzero_segments = qualified(wire::ELF_IMAGE_MAGIC, profile);
        nonzero_segments[wire::OFF_HEADER_SEGMENT_COUNT..][..2]
            .copy_from_slice(&1u16.to_le_bytes());

        for bytes in [wrong_abi, bad_stack, nonzero_entry, nonzero_segments] {
            let format = PayloadFormat::classify(&bytes, profile);
            assert_eq!(format, PayloadFormat::Unrecognized);
            assert!(!format.is_loadable());
        }
    }

    #[test]
    fn bare_aarch64_elf64_is_recognized_but_refused_for_loading() {
        let bytes = elf_header(2, 1, 183);
        let format = PayloadFormat::classify(&bytes, sel4_profile());
        assert_eq!(format, PayloadFormat::Aarch64Elf);
        assert!(!format.is_loadable());
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
