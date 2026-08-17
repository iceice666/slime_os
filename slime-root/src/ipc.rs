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
            37,
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
}
