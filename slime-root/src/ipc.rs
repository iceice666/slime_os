//! Bounded root-side service IPC.
//!
//! Component-to-component messages use declared seL4 Endpoints directly. This
//! module decodes only the bounded wire envelope received by root services;
//! each service owns the meaning of its request labels.

/// Native messages carry at most one capability, and at most this many payload
/// bytes. Both are generated from `contracts/syscall-abi/v1/schema.zt`, so the
/// bound the root enforces is the one `components/runtime` encodes against
/// (B59).
pub use slime_proto::syscall_abi::{
    MAX_CAPS_PER_MSG as MAX_MESSAGE_CAPS, MAX_MSG as MAX_MESSAGE_BYTES,
};

/// The wake sources one fabric participant may register, re-exported from the
/// contract that declares it (B66).
///
/// This was a local `= 9` in this module, described as a "compatibility value"
/// for a root wait set B46 deleted — a number `ipc.rs` had no business owning,
/// duplicating `fabric_graph`'s own `maxIngressSources`. Admission now reads the
/// one declaration, and `build-generation.py` emits the same value as
/// `FABRIC_MAX_INGRESS_SOURCES` for the workers to size themselves against.
pub use boot_contracts::fabric_graph::MAX_INGRESS_SOURCES as MAX_WAIT_SOURCES;

/// Which root mechanism owns `label`, or `None` when no surviving mechanism
/// does.
///
/// The envelope decode below bounds a request's *shape*; this assigns its
/// *meaning*. Both belong to this module, and B61 moved this here from the
/// binary: it is a pure total function over two generated tables — the operation
/// labels and the service kinds — so leaving it in `main.rs` made it unreachable
/// from `just test_sel4_root` for no benefit.
///
/// A label with no surviving mechanism is refused rather than defaulted. B46
/// deleted the logical channel, wait-set, and endpoint-create operations, and a
/// component image built before that cutover still invokes their labels; each
/// must answer `UnsupportedOperation`, never fall through to a mechanism that now
/// happens to occupy a nearby number.
pub const fn service_for_root_label(label: sel4::Word) -> Option<u32> {
    use boot_contracts::generation::{
        SERVICE_CAPABILITY_TRANSFER, SERVICE_DIRECTORY, SERVICE_LIFECYCLE, SERVICE_SHARED_BUFFER,
        SERVICE_SPAWN, SERVICE_SUPERVISION,
    };
    use slime_proto::syscall_abi::{
        capability_table_labels, capability_transfer_labels, directory_labels, lifecycle_labels,
        shared_buffer_labels, spawn_labels, supervision_labels,
    };
    match label {
        lifecycle_labels::EXIT | lifecycle_labels::UNHEALTHY => Some(SERVICE_LIFECYCLE),
        spawn_labels::SPAWN => Some(SERVICE_SPAWN),
        supervision_labels::STATUS | supervision_labels::DERIVE => Some(SERVICE_SUPERVISION),
        capability_table_labels::DROP
        | capability_table_labels::OCCUPANCY
        | capability_table_labels::RESOLVE_BINDING
        | capability_transfer_labels::EXPORT
        | capability_transfer_labels::IMPORT
        | capability_transfer_labels::EXPORT_CANCEL
        | capability_transfer_labels::EXPORT_FINALIZE => Some(SERVICE_CAPABILITY_TRANSFER),
        shared_buffer_labels::CREATE
        | shared_buffer_labels::RELEASE
        | shared_buffer_labels::MAP
        | shared_buffer_labels::UNMAP
        | shared_buffer_labels::SEAL
        | shared_buffer_labels::LOAN
        | shared_buffer_labels::LOAN_MAP
        | shared_buffer_labels::RETURN
        | shared_buffer_labels::REVOKE
        | shared_buffer_labels::OCCUPANCY => Some(SERVICE_SHARED_BUFFER),
        directory_labels::DERIVE => Some(SERVICE_DIRECTORY),
        _ => None,
    }
}
/// Message registers the AArch64 fast path carries in architectural registers.
pub const FAST_MESSAGE_REGISTERS: usize = sel4::NUM_FAST_MESSAGE_REGISTERS;

// The four-MR fast path and the four-capability logical bound are independent
// facts that happen to agree on AArch64. Pin the transport side so a profile
// with fewer fast registers fails here instead of silently truncating.
const _: () = assert!(FAST_MESSAGE_REGISTERS == 4);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IpcError {
    InvalidOperation,
    UnsupportedOperation,
    InvalidLength,
    UnsupportedCapabilityTransfer,
    QueueFull,
    WouldBlock,
    PeerDead,
    DestinationSlotsExhausted,
    TransferFailed,
    StalePlan,
    WaiterConflict,
    /// The caller named a slot holding nothing, holding the wrong kind of
    /// resource, or carrying insufficient rights.
    ///
    /// One variant for all three deliberately: they are indistinguishable to
    /// the caller by design, so a component cannot map its own capability table
    /// by watching which error a probe returns.
    ///
    /// Distinct from [`Self::InvalidOperation`] because it is the one the
    /// retired kernel answers `ERR_BAD_CAP` to, and components test for that
    /// code specifically — `sample-receiver` proves a loan is single-return by
    /// requiring exactly `ERR_BAD_CAP` from the second return. Collapsing it
    /// into `InvalidOperation` would answer `ERR_INVALID_ARG` and make that
    /// check fail against a correct implementation.
    ///
    /// [`Self::UnsupportedCapabilityTransfer`] answers the same status for the
    /// same reason and stays separate only so the root's own markers can name
    /// the cause.
    BadCapability,
}

impl IpcError {
    /// Slime-visible status returned in reply MR0 by the root service loop.
    ///
    /// The codes are generated from `contracts/syscall-abi/v1/schema.zt` and are
    /// the same constants `components/runtime` tests against (B59). The mapping
    /// is deliberately many-to-one: a component learns the failure class, not
    /// which internal predicate rejected it.
    pub const fn slime_status(self) -> i64 {
        use slime_proto::syscall_abi::{
            ERR_BAD_CAP, ERR_INVALID_ARG, ERR_OUT_OF_MEMORY, ERR_PEER_DEAD, ERR_WOULDBLOCK,
        };
        match self {
            Self::BadCapability => ERR_BAD_CAP,
            Self::PeerDead => ERR_PEER_DEAD,
            Self::QueueFull | Self::WouldBlock => ERR_WOULDBLOCK,
            // `ERR_BAD_CAP`, with the other capability failures: `sys_send`
            // answers that for a capability it will not move, and a component
            // written against the retired kernel tests for it. It stays a
            // distinct variant because the root's own marker distinguishes an
            // unmovable capability from an absent one, which is a diagnosis a
            // component is deliberately not given.
            Self::UnsupportedCapabilityTransfer => ERR_BAD_CAP,
            Self::InvalidOperation
            | Self::UnsupportedOperation
            | Self::InvalidLength
            | Self::StalePlan
            | Self::WaiterConflict => ERR_INVALID_ARG,
            Self::DestinationSlotsExhausted | Self::TransferFailed => ERR_OUT_OF_MEMORY,
        }
    }
}

/// One bounded request received on the root service endpoint.
///
/// The root dispatcher interprets `label` according to the narrow service it
/// selects. Keeping the envelope raw here avoids coupling unrelated services
/// into a single public label namespace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Request {
    pub badge: sel4::Badge,
    pub label: sel4::Word,
    pub mrs: [sel4::Word; FAST_MESSAGE_REGISTERS],
    pub len: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Response {
    pub result: i64,
    pub aux: sel4::Word,
}

impl Response {
    pub const fn success(result: i64, aux: sel4::Word) -> Self {
        Self { result, aux }
    }

    pub const fn error(error: IpcError) -> Self {
        Self {
            result: error.slime_status(),
            aux: 0,
        }
    }
}

/// One decoded arrival on the root service endpoint.
///
/// The badge is kept even when decoding fails, because the dispatcher still
/// owes the caller a reply and still attributes the attempt to a task. The raw
/// `MessageInfo` is kept because a fault arrives on this same endpoint and is
/// decoded from it by `fault::decode_fault` before the request envelope is
/// dispatched.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reception {
    pub info: sel4::MessageInfo,
    pub badge: sel4::Badge,
    pub request: Result<Request, IpcError>,
}

// Receive one bounded root-service request. Raw seL4 extra-cap transfer is
// not part of this envelope; capability transport uses its declared native
// mechanism instead.

/// One console message: the payload descriptor and the fast registers behind
/// it.
///
/// The console endpoint has its own narrow labels because one thread serves
/// console, input, block, and directory device traffic.
pub struct ConsoleMessage {
    pub badge: sel4::Badge,
    pub kind: ConsoleKind,
    pub mrs: [sel4::Word; FAST_MESSAGE_REGISTERS],
    pub len: usize,
}

/// What a console-endpoint message asks for.
///
/// Two kinds share one endpoint because one thread serves them and a second
/// endpoint would need a second blocking receive. They are both "the
/// terminal", so one queue between them is the honest shape.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ConsoleKind {
    /// One-way debug output.
    Write,
    /// A read returning one decoded key event.
    InputRead,
    /// A directory inspect, derive, or commit (B45). Here for the same
    /// reason block requests are: the namespace and scope tables came with
    /// the handlers, and a commit racing a lifecycle syscall on one queue
    /// makes each wait for the other for no reason.
    ///
    /// Derive is *not* here: it is the only writer of the caller's capability
    /// table, which the main dispatcher also writes, and two threads writing
    /// one task's table is a data race.
    DirectoryInspect,
    DirectoryCommit,
    /// One sector-granular block-device request (B43). On this thread because
    /// a slow disk must not hold up lifecycle or fabric traffic, and because
    /// the device tables live with whoever answers block requests.
    BlockTransact,
}

impl ConsoleKind {
    const WRITE: sel4::Word = 0;
    const INPUT_READ: sel4::Word = 1;
    const BLOCK_TRANSACT: sel4::Word = 2;
    const DIRECTORY_INSPECT: sel4::Word = 3;
    const DIRECTORY_COMMIT: sel4::Word = 4;

    const fn from_label(label: sel4::Word) -> Option<Self> {
        match label {
            Self::WRITE => Some(Self::Write),
            Self::INPUT_READ => Some(Self::InputRead),
            Self::BLOCK_TRANSACT => Some(Self::BlockTransact),
            Self::DIRECTORY_INSPECT => Some(Self::DirectoryInspect),
            Self::DIRECTORY_COMMIT => Some(Self::DirectoryCommit),
            _ => None,
        }
    }
}

/// Receive one console message through an explicit IPC buffer.
///
/// The `sel4` crate keeps one IPC-buffer slot per address space, and a receive
/// holds it borrowed for as long as it blocks — so the console dispatcher,
/// being a second root thread, names its buffer here rather than using the
/// ambient slot (B41).
pub fn recv_console(
    endpoint: sel4::cap::Endpoint,
    buffer: &mut sel4::IpcBuffer,
) -> Result<ConsoleMessage, IpcError> {
    let reception = endpoint.with(buffer).recv_with_mrs(());
    let len = reception.info.length();
    if len > FAST_MESSAGE_REGISTERS {
        return Err(IpcError::InvalidLength);
    }
    if reception.info.extra_caps() != 0 || reception.info.caps_unwrapped() != 0 {
        return Err(IpcError::UnsupportedCapabilityTransfer);
    }
    let Some(kind) = ConsoleKind::from_label(reception.info.label()) else {
        return Err(IpcError::InvalidOperation);
    };
    Ok(ConsoleMessage {
        badge: reception.badge,
        kind,
        mrs: reception.msg,
        len,
    })
}

/// Answer the previous input read and wait for the next message, in one
/// syscall — the console loop's steady state once a read has been served.
pub fn reply_recv_console(
    endpoint: sel4::cap::Endpoint,
    response: Response,
    buffer: &mut sel4::IpcBuffer,
) -> Result<ConsoleMessage, IpcError> {
    let words = [response.result as sel4::Word, response.aux];
    let info = sel4::MessageInfoBuilder::default()
        .length(words.len())
        .build();
    // `reply_recv` carries its payload in the buffer's message registers
    // rather than the fast ones, so the reply is staged there and the next
    // request is read back out of them.
    buffer.msg_regs_mut()[..words.len()].copy_from_slice(&words);
    let (received, badge) = endpoint.with(&mut *buffer).reply_recv(info, ());
    let len = received.length();
    if len > FAST_MESSAGE_REGISTERS {
        return Err(IpcError::InvalidLength);
    }
    if received.extra_caps() != 0 || received.caps_unwrapped() != 0 {
        return Err(IpcError::UnsupportedCapabilityTransfer);
    }
    let Some(kind) = ConsoleKind::from_label(received.label()) else {
        return Err(IpcError::InvalidOperation);
    };
    let mut mrs = [0 as sel4::Word; FAST_MESSAGE_REGISTERS];
    mrs[..len].copy_from_slice(&buffer.msg_regs()[..len]);
    Ok(ConsoleMessage {
        badge,
        kind,
        mrs,
        len,
    })
}

/// Receive through an explicit IPC buffer rather than the ambient one.
///
/// The `sel4` crate keeps one IPC-buffer slot per address space on this
/// target, and a receive holds it borrowed for as long as it blocks — so a
/// second root thread using the ambient slot would find it permanently taken
/// by whichever thread is parked in `seL4_Recv`. Naming the buffer on the
/// capability sidesteps the slot entirely (B41).
pub fn recv_request_with(endpoint: sel4::cap::Endpoint, buffer: &mut sel4::IpcBuffer) -> Reception {
    let reception = endpoint.with(buffer).recv_with_mrs(());
    Reception {
        info: reception.info.clone(),
        badge: reception.badge,
        request: decode_request(&reception),
    }
}

pub fn recv_request(endpoint: sel4::cap::Endpoint) -> Reception {
    let reception = endpoint.recv_with_mrs(());
    Reception {
        info: reception.info.clone(),
        badge: reception.badge,
        request: decode_request(&reception),
    }
}

fn decode_request(reception: &sel4::RecvWithMRs) -> Result<Request, IpcError> {
    let len = reception.info.length();
    if len > FAST_MESSAGE_REGISTERS {
        return Err(IpcError::InvalidLength);
    }
    let caps = reception.info.extra_caps();
    if reception.info.caps_unwrapped() != 0 || caps > MAX_MESSAGE_CAPS || caps != 0 {
        return Err(IpcError::UnsupportedCapabilityTransfer);
    }
    Ok(Request {
        badge: reception.badge,
        label: reception.info.label(),
        mrs: reception.msg,
        len,
    })
}

/// Reply to the most recent non-MCS request. MR0 carries the bit-exact logical
/// `i64` result and MR1 carries the service-specific auxiliary value.
#[sel4::sel4_cfg(not(KERNEL_MCS))]
pub fn reply(response: Response) {
    let words = [response.result as sel4::Word, response.aux];
    let info = sel4::MessageInfoBuilder::default()
        .length(words.len())
        .build();
    sel4::with_ipc_buffer_mut(|ipc_buffer| {
        ipc_buffer.msg_regs_mut()[..words.len()].copy_from_slice(&words);
        sel4::reply(ipc_buffer, info);
    });
}

/// Poll a notification used to multiplex endpoint, timer, IRQ, and lifecycle
/// readiness. Badges are opaque routing tokens assigned by `slime-root`; no
/// CSpace slot or physical identifier is exposed by this helper.
pub fn poll_notification(notification: sel4::cap::Notification) -> Option<sel4::Badge> {
    let (info, badge) = notification.poll();
    (info.length() != 0 || badge != 0).then_some(badge)
}

/// The maximum binding-name length a `RESOLVE_BINDING` request may carry.
///
/// The message envelope, not the generation string table's 255-byte per-string
/// bound: a name arrives in one request, so a name longer than the envelope
/// cannot be asked about at all and is refused on length rather than silently
/// truncated into a different name. Every grant name in every current fixture is
/// well inside this, and `contracts/component-spec/v1`'s `maxNameBytes` is 64.
pub const MAX_BINDING_NAME: usize = MAX_MESSAGE_BYTES;

/// Which of `instance`'s own capability slots holds the binding named `name`.
///
/// CP2's runtime binding resolution. Before this, a component learned its slot
/// numbers from a constant table `components/bins/build.rs` generated by parsing
/// the generation manifest, compiled them into its own image, and so could not be
/// built outside that one crate against that one manifest (B70). The root already
/// holds the answer: each instance declares its own `InstanceBinding` list of
/// `(grant, slot)`, and each grant carries its name, so this is a lookup over
/// data activation already installed rather than a new authority.
///
/// The instance's *own* binding list is what is searched, not the global
/// `CapBinding` table. The two are different facts and the difference bit during
/// CP2: a `CapBinding` exists once per grant, at whichever holder materializes
/// it, so the endpoint `console` waits on is recorded against `init` alone and
/// resolving through that table answered nothing for `console`. The per-instance
/// list is the one that says which slot *this* instance's CSpace uses.
///
/// `instance` is the caller's own index, taken from the badge the root
/// authenticated. It is a parameter rather than something this function derives:
/// the caller never names it, so there is nothing to forge, and the dispatcher's
/// one job is to pass the authenticated identity through.
///
/// A name the instance does not bind returns `None`, never another instance's
/// slot — the property that makes this answerable for every component. A
/// component learns its own layout, which it already knew at compile time, and
/// nothing else.
use boot_contracts::boot_layout;

pub fn resolve_binding_slot(
    generation: &boot_contracts::generation::Generation<'_>,
    instance: usize,
    name: &[u8],
) -> Option<usize> {
    if !binding_name_admissible(name) {
        return None;
    }
    let name = core::str::from_utf8(name).ok()?;
    let instance_index = instance;
    let instance = generation.instance(instance_index).ok()?;
    // Each binding names a grant by index, so the grant is decoded per binding
    // rather than the grant table being scanned for the name first: an instance
    // binds a handful of grants, while the generation declares up to 128.
    for index in 0..instance.binding_count() {
        let binding = generation.binding(instance, index).ok()?;
        if generation
            .grant(binding.grant)
            .is_ok_and(|grant| grant.name == name)
        {
            return Some(binding.slot);
        }
    }
    // A capability *role* — `kind:<kind>+<right>,<right>` — resolved over the
    // caller's own bindings by what the capability is rather than by what the
    // generation happened to call it.
    //
    // This axis exists because grant names are not stable across generations and
    // therefore cannot be written in a component. `spawn-service` binds
    // `spawn-service-echo` under `valid.zti` and `spawn-service-echo-agent` under
    // `sel4-dango.zti`, and its RPC endpoint is `spawn-service-rpc` in one and
    // `dango-e-spawn-service-rpc` in the other. A component naming either string
    // would be coupled to one manifest, which is the coupling B70 exists to
    // remove, so name lookup alone cannot migrate these sites.
    //
    // `components/bins/build.rs` already demonstrates the right axis: it resolves
    // these same slots by capability kind and rights (`binding_with_right_slot`
    // asks for `bufferCreate`, `related_binding_slot` for `send`+`recv`) and never
    // by grant name. Those are properties of the capability the component needs,
    // so they are answerable from any manifest that grants it. This moves that
    // question from a build script parsing a manifest to the root reading the
    // activation record it already holds.
    //
    // The match is exact on kind and a superset on rights: a component asking for
    // `send`+`recv` accepts a grant carrying those and more, which is the same
    // containment `build.rs` applies. Still the caller's own bindings only, so
    // this discloses nothing a name lookup would not.
    if let Some(role) = name.strip_prefix("kind:") {
        return resolve_role_slot(generation, instance, role);
    }
    // A layout role, only when the caller asked for one *explicitly*, and only
    // for the bootstrap instance.
    //
    // The namespace prefix is what makes this sound, and it exists because the
    // unprefixed version was written twice and failed on real boots twice. The
    // boot layout declares the bootstrap component's executables and channel
    // halves; a grant list declares its capability edges; and the two use
    // overlapping names for different things — `console` is a layout executable
    // at slot 1 while `init-console` is a grant at the same slot, and
    // `console-output` is a grant under one generation and absent from the
    // layout under another. A single flat lookup therefore answered a channel
    // question with an executable slot, and `init` sent into an endpoint nobody
    // was waiting on: a hang rather than a visible error.
    //
    // So the caller states which table it means. `executable:` and `channel:`
    // address the layout's two identity domains, which is exactly the
    // distinction the contract already draws to keep a component and a channel
    // sharing a name apart. An unprefixed name is a grant and can never reach
    // the layout, so no existing caller's meaning changes and no layout entry
    // can shadow a grant.
    if instance_index == generation.bootstrap() {
        if let Some(role) = name.strip_prefix("executable:") {
            return resolve_layout_slot(generation, &boot_layout::component_identity(role));
        }
        if let Some(role) = name.strip_prefix("channel:") {
            return resolve_layout_slot(generation, &boot_layout::channel_identity(role));
        }
    }
    None
}

/// Which of `instance`'s slots carries a capability of `kind` bearing `rights`.
///
/// `role` is `<kind>` or `<kind>+<right>,<right>`. The kind names are the
/// manifest's own `capabilityKind` spellings, so a component asks in the
/// vocabulary the contract already defines rather than in a second one invented
/// here.
///
/// Rights are a *superset* test, matching `build.rs`'s containment check: a
/// component needing `send`+`recv` is served by a grant carrying those and more,
/// because extra rights on a capability it already holds do not change whether
/// this is the slot it asked for. Kind is exact — a `block` capability is never
/// an answer to an `endpoint` question, however its rights overlap.
///
/// An *ambiguous* role is refused, not resolved to the lowest slot. This is the
/// same discipline the removed layout fallback failed: `spawn-service` binds two
/// `executable+exec,spawn` grants under `valid.zti` (`echo` at slot 1, `sysinfo`
/// at slot 2) and three under `sel4-dango.zti`, so a lowest-slot tiebreak would
/// have answered "spawn echo" to a question meaning "spawn sysinfo" — a wrong
/// capability of the right type, which is exactly the failure that presents as a
/// hang instead of an error. A component needing to tell those apart is asking a
/// question this axis cannot answer, and must be told so rather than guessed at.
fn resolve_role_slot(
    generation: &boot_contracts::generation::Generation<'_>,
    instance: boot_contracts::generation::Instance,
    role: &str,
) -> Option<usize> {
    let (kind, rights) = match role.split_once('+') {
        Some((kind, rights)) => (kind, rights),
        None => (role, ""),
    };
    let kind = capability_kind_named(kind)?;
    // An unknown right is refused outright rather than skipped, so a misspelling
    // narrows nothing: `kind:endpoint+snd` must not silently become
    // `kind:endpoint` and match the first endpoint the caller binds.
    for right in rights.split(',').filter(|right| !right.is_empty()) {
        boot_contracts::generation::right_named(right)?;
    }
    let mut found: Option<usize> = None;
    for index in 0..instance.binding_count() {
        let binding = generation.binding(instance, index).ok()?;
        let Ok(grant) = generation.grant(binding.grant) else {
            continue;
        };
        if grant.capability_kind != kind {
            continue;
        }
        if !rights
            .split(',')
            .filter(|right| !right.is_empty())
            .all(|right| {
                boot_contracts::generation::right_named(right)
                    .is_some_and(|bit| grant.rights & bit == bit)
            })
        {
            continue;
        }
        if found.is_some() {
            // Two answers means the question did not identify one capability.
            return None;
        }
        found = Some(binding.slot);
    }
    found
}

/// The `capabilityKind` spelling the generation manifest uses, decoded.
///
/// An unknown kind is `None` rather than a default: a component asking for a kind
/// this root does not know is a question with no answer, and guessing one would
/// hand it a capability of the wrong type.
fn capability_kind_named(name: &str) -> Option<boot_contracts::generation::CapabilityKind> {
    use boot_contracts::generation::CapabilityKind;

    Some(match name {
        "endpoint" => CapabilityKind::Endpoint,
        "executable" => CapabilityKind::Executable,
        "sharedBufferFactory" => CapabilityKind::SharedBufferFactory,
        "block" => CapabilityKind::Block,
        "directory" => CapabilityKind::Directory,
        "input" => CapabilityKind::Input,
        "supervision" => CapabilityKind::Supervision,
        "sharedBuffer" => CapabilityKind::SharedBuffer,
        "loan" => CapabilityKind::Loan,
        _ => return None,
    })
}

/// Which slot of the bootstrap component's CSpace carries `identity`.
///
/// The layout keys entries by identity hash under two domains, so the caller
/// computes the one it means and this compares it. Resource objects share a
/// kind, so the decode is the discriminator: a shared-buffer budget or a fabric
/// graph fails it and is skipped.
fn resolve_layout_slot(
    generation: &boot_contracts::generation::Generation<'_>,
    identity: &[u8; 32],
) -> Option<usize> {
    use boot_contracts::generation::KIND_RESOURCE;

    for index in 0..generation.object_count() {
        let object = generation.object(index).ok()?;
        if object.kind != KIND_RESOURCE {
            continue;
        }
        let Ok(layout) = boot_layout::BootLayout::decode(object.bytes) else {
            continue;
        };
        for entry in 0..layout.entry_count() {
            let entry = layout.entry(entry)?;
            if &entry.name_identity == identity {
                return Some(entry.slot as usize);
            }
        }
    }
    None
}

/// The bounds `resolve_binding_slot` applies before it looks anything up.
///
/// Separated from the lookup so the guards are reachable without a decoded
/// generation: building a valid v5 generation by hand in a unit test would test
/// the encoder, not this. The lookup itself is proved on a real boot by
/// `just runtime_binding_resolution_check`, where a component resolves its own
/// slot and a name it was not granted is refused.
pub fn binding_name_admissible(name: &[u8]) -> bool {
    !name.is_empty() && name.len() <= MAX_BINDING_NAME && core::str::from_utf8(name).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use boot_contracts::generation::{
        SERVICE_CAPABILITY_TRANSFER, SERVICE_DIRECTORY, SERVICE_LIFECYCLE, SERVICE_SHARED_BUFFER,
        SERVICE_SPAWN, SERVICE_SUPERVISION,
    };
    use slime_proto::syscall_abi::{
        capability_table_labels, capability_transfer_labels, directory_labels, lifecycle_labels,
        shared_buffer_labels, spawn_labels, supervision_labels,
    };

    /// Every declared operation routes to the mechanism that owns it. B61 moved
    /// this out of the binary so the routing is checkable without a boot; before
    /// that, a label mapped to the wrong service was observable only as a
    /// component's request being refused on a real plane.
    #[test]
    fn every_declared_label_routes_to_its_owning_service() {
        for (label, service) in [
            (lifecycle_labels::EXIT, SERVICE_LIFECYCLE),
            (lifecycle_labels::UNHEALTHY, SERVICE_LIFECYCLE),
            (spawn_labels::SPAWN, SERVICE_SPAWN),
            (supervision_labels::STATUS, SERVICE_SUPERVISION),
            (supervision_labels::DERIVE, SERVICE_SUPERVISION),
            (capability_table_labels::DROP, SERVICE_CAPABILITY_TRANSFER),
            (
                capability_table_labels::OCCUPANCY,
                SERVICE_CAPABILITY_TRANSFER,
            ),
            (
                capability_table_labels::RESOLVE_BINDING,
                SERVICE_CAPABILITY_TRANSFER,
            ),
            (
                capability_transfer_labels::EXPORT,
                SERVICE_CAPABILITY_TRANSFER,
            ),
            (
                capability_transfer_labels::IMPORT,
                SERVICE_CAPABILITY_TRANSFER,
            ),
            (
                capability_transfer_labels::EXPORT_CANCEL,
                SERVICE_CAPABILITY_TRANSFER,
            ),
            (
                capability_transfer_labels::EXPORT_FINALIZE,
                SERVICE_CAPABILITY_TRANSFER,
            ),
            (shared_buffer_labels::CREATE, SERVICE_SHARED_BUFFER),
            (shared_buffer_labels::RELEASE, SERVICE_SHARED_BUFFER),
            (shared_buffer_labels::MAP, SERVICE_SHARED_BUFFER),
            (shared_buffer_labels::UNMAP, SERVICE_SHARED_BUFFER),
            (shared_buffer_labels::SEAL, SERVICE_SHARED_BUFFER),
            (shared_buffer_labels::LOAN, SERVICE_SHARED_BUFFER),
            (shared_buffer_labels::LOAN_MAP, SERVICE_SHARED_BUFFER),
            (shared_buffer_labels::RETURN, SERVICE_SHARED_BUFFER),
            (shared_buffer_labels::REVOKE, SERVICE_SHARED_BUFFER),
            (shared_buffer_labels::OCCUPANCY, SERVICE_SHARED_BUFFER),
            (directory_labels::DERIVE, SERVICE_DIRECTORY),
        ] {
            assert_eq!(
                service_for_root_label(label),
                Some(service),
                "label {label} routed to the wrong mechanism"
            );
        }
    }

    /// B46 deleted the logical channel, wait-set, and endpoint-create operations
    /// and their labels were not reused. A component image built before that
    /// cutover still invokes them, so each must be refused rather than falling
    /// through to whichever mechanism now sits at a nearby number — a
    /// re-meaning would hand an old caller authority it never asked for.
    #[test]
    fn retired_and_unknown_labels_are_refused() {
        // The gaps `contracts/syscall-abi/v1/schema.zt` documents as retired,
        // plus the fixture label (not part of the component ABI) and values
        // outside the table entirely.
        for label in [
            0,
            1,
            2,
            6,
            7,
            8,
            10,
            11,
            14,
            16,
            17,
            18,
            19,
            20,
            // 37 was here until CP2 assigned it to `RESOLVE_BINDING`. Moving it
            // out of this list is the whole change: a number this test asserts
            // routes nowhere and a number the contract declares are the same
            // fact stated twice, so assigning a label must fail here first.
            38,
            64,
            sel4::Word::MAX,
        ] {
            assert_eq!(
                service_for_root_label(label),
                None,
                "retired or unknown label {label} was routed to a mechanism"
            );
        }
    }

    /// The fixture directive shares the root endpoint but is not a component
    /// operation: it is the two-fixture boot proof's handshake. Routing it to a
    /// mechanism would expose it to any component that guessed the label.
    #[test]
    fn the_fixture_directive_is_not_a_component_service() {
        assert_eq!(
            service_for_root_label(slime_proto::syscall_abi::fixture_labels::DIRECTIVE),
            None
        );
    }

    /// A binding name must be non-empty, fit one message, and be UTF-8.
    ///
    /// Each is a refusal a component could otherwise turn into a lookup: an empty
    /// name matches no grant but would still walk the table, a name longer than
    /// the envelope would be a truncated *different* name, and non-UTF-8 bytes
    /// cannot name a grant at all. Refusing before the lookup is what keeps the
    /// operation's cost a function of the generation rather than of the request.
    #[test]
    fn binding_names_are_bounded_before_any_lookup() {
        assert!(binding_name_admissible(b"console-output"));
        assert!(binding_name_admissible(&[b'a'; MAX_BINDING_NAME]));
        assert!(!binding_name_admissible(b""));
        assert!(!binding_name_admissible(&[b'a'; MAX_BINDING_NAME + 1]));
        // A lone continuation byte: valid length, not a valid name.
        assert!(!binding_name_admissible(&[0x80]));
    }

    /// The name bound is the message envelope's, not the string table's.
    ///
    /// `MAX_STRING_BYTES` is 255 and a request carries 64 bytes, so binding the
    /// name to the string table would admit names no request could deliver and
    /// silently compare a truncation.
    #[test]
    fn binding_name_bound_fits_one_request() {
        assert_eq!(MAX_BINDING_NAME, MAX_MESSAGE_BYTES);
        assert!(MAX_BINDING_NAME <= boot_contracts::generation::MAX_STRING_BYTES);
    }

    /// The two layout namespaces are distinct domains, and neither collides with
    /// the other for the same text.
    ///
    /// This is what lets one query serve grants and layout roles without a layout
    /// entry shadowing a grant. The unprefixed-name version of this operation was
    /// written twice and failed on real boots twice: `console` is a layout
    /// executable at slot 1 while `init-console` is a grant at the same slot, and
    /// a flat lookup answered a channel question with an executable slot. The
    /// prefix makes the caller state which table it means, so the domains below
    /// must genuinely differ.
    #[test]
    fn layout_namespaces_are_distinct_domains() {
        use boot_contracts::boot_layout::{channel_identity, component_identity};
        assert_ne!(component_identity("console"), channel_identity("console"));
        assert_ne!(
            component_identity("console-output"),
            channel_identity("console-output")
        );
        // And a prefixed request never matches the grant name it contains, so a
        // grant called `executable:x` could not be reached as a layout role.
        assert!(binding_name_admissible(b"executable:console"));
        assert!(binding_name_admissible(b"channel:console-output"));
    }

    /// Every `capabilityKind` the manifest can spell decodes, and nothing else
    /// does.
    ///
    /// The role query is only manifest-independent if it speaks the manifest's own
    /// vocabulary. A kind this table missed would be unaskable while looking
    /// askable, and an unknown kind must be no answer rather than a default, since
    /// defaulting would hand out a capability of the wrong type.
    #[test]
    fn every_manifest_capability_kind_is_askable() {
        use boot_contracts::generation::CapabilityKind;
        for (spelling, kind) in [
            ("endpoint", CapabilityKind::Endpoint),
            ("executable", CapabilityKind::Executable),
            ("sharedBufferFactory", CapabilityKind::SharedBufferFactory),
            ("block", CapabilityKind::Block),
            ("directory", CapabilityKind::Directory),
            ("input", CapabilityKind::Input),
            ("supervision", CapabilityKind::Supervision),
            ("sharedBuffer", CapabilityKind::SharedBuffer),
            ("loan", CapabilityKind::Loan),
        ] {
            assert_eq!(capability_kind_named(spelling), Some(kind), "{spelling}");
        }
        assert_eq!(capability_kind_named("Endpoint"), None);
        assert_eq!(capability_kind_named(""), None);
        assert_eq!(capability_kind_named("notAKind"), None);
    }

    /// Rights names come from the schema, so the generated lookup agrees with the
    /// generated constants.
    ///
    /// Both sides are rendered from `rightBits` in
    /// `contracts/generation/v5/gen_rust.zt`; this asserts the pairing rather than
    /// trusting that two emitters stayed in step.
    #[test]
    fn manifest_right_spellings_match_their_bits() {
        use boot_contracts::generation::{
            RIGHT_BLOCK_READ, RIGHT_BUFFER_CREATE, RIGHT_EXEC, RIGHT_RECV, RIGHT_SEND, right_named,
        };
        assert_eq!(right_named("send"), Some(RIGHT_SEND));
        assert_eq!(right_named("recv"), Some(RIGHT_RECV));
        assert_eq!(right_named("exec"), Some(RIGHT_EXEC));
        assert_eq!(right_named("bufferCreate"), Some(RIGHT_BUFFER_CREATE));
        assert_eq!(right_named("blockRead"), Some(RIGHT_BLOCK_READ));
        // Rust spellings are not manifest spellings, and only the latter is asked.
        assert_eq!(right_named("RIGHT_SEND"), None);
        assert_eq!(right_named("buffer_create"), None);
        assert_eq!(right_named(""), None);
    }

    /// An ambiguous role parses but resolves to nothing, and the refusal is not
    /// an accident of parsing.
    ///
    /// This is a non-vacuous pairing (B67): the same spelling that must be refused
    /// when several bindings match is a *well-formed* query whose kind and rights
    /// all decode, so the refusal comes from the ambiguity itself rather than from
    /// a rejected name. `sel4-dango.zti` grants `spawn-service` three
    /// `send`+`recv` endpoints — the RPC channel plus one context endpoint per
    /// command — which is exactly this case, and it was observed: resolving that
    /// role hung the dango plane at `dango> $(sysinfo)` until the query was left
    /// refusing it and `RPC_SLOT` restored.
    #[test]
    fn an_ambiguous_role_is_well_formed_yet_unanswerable() {
        use boot_contracts::generation::{CapabilityKind, right_named};
        // Every part of `kind:endpoint+send,recv` decodes, so nothing about the
        // spelling explains a `None`.
        assert_eq!(
            capability_kind_named("endpoint"),
            Some(CapabilityKind::Endpoint)
        );
        assert!(right_named("send").is_some());
        assert!(right_named("recv").is_some());
        // And an unknown right does not silently widen the query to "any endpoint":
        // it must be refused, or a misspelling would match the first endpoint bound.
        assert_eq!(right_named("snd"), None);
    }
}
