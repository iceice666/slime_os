//! Bounded root-side service IPC.
//!
//! Component-to-component messages use declared seL4 Endpoints directly. This
//! module decodes only the policy and capability-update operations the root
//! still owns.

/// Native messages carry at most one capability.
pub const MAX_MESSAGE_CAPS: usize = 1;
/// Maximum native message payload, matching the userspace ABI.
pub const MAX_MESSAGE_BYTES: usize = 64;
/// Compatibility value for generation fabric admission. Native rendezvous
/// supplies backpressure; the root owns no channel queue.
pub const CHANNEL_CAPACITY: usize = 1;
/// Compatibility value for generation graph admission. Components wait on
/// declared Notifications rather than a root wait set.
pub const MAX_WAIT_SOURCES: usize = 9;
/// Message registers the AArch64 fast path carries in architectural registers.
pub const FAST_MESSAGE_REGISTERS: usize = sel4::NUM_FAST_MESSAGE_REGISTERS;
/// Highest label an operation may carry; see [`Operation`].
pub const MAX_OPERATION_LABEL: u16 = 36;

/// The label `InputRead` used to carry, retired with B41.
///
/// Left as a hole rather than renumbered: a component built against the old
/// ABI is refused, where renumbering would have it silently invoke whichever
/// operation moved into slot 17. Input is a Call on the console endpoint now.
pub const RETIRED_INPUT_READ_LABEL: sel4::Word = 17;

/// The labels directory inspect and commit used to carry, retired with B45.
///
/// `DirectoryDerive` keeps label 15: it is the only writer of the caller's
/// capability table, which the main dispatcher also writes, so moving it to
/// the second thread would be a data race rather than a decoupling.
pub const RETIRED_DIRECTORY_LABELS: [sel4::Word; 2] = [14, 16];

/// The labels generation, recovery, and health policy used to carry, retired
/// with B44.
///
/// None of them had a handler either — `HealthConfirm`'s arm existed but was
/// never reached, because boot promotion happens from the supervisor's idle
/// path once every required instance parks, not from a component asking. The
/// rest answered `UnsupportedOperation`. Generation management and recovery
/// are userspace policy built over block authority.
pub const RETIRED_POLICY_LABELS: [sel4::Word; 4] = [8, 10, 18, 19];

/// The label `StoreTransact` used to carry, retired with B43.
///
/// Never had a root handler: it answered `UnsupportedOperation` from
/// `Mediation::Unavailable`, so it was ABI surface for an operation the root
/// does not perform. A durable store is userspace policy built over block
/// authority, so there is nothing here for it to become.
pub const RETIRED_STORE_TRANSACT_LABEL: sel4::Word = 7;

/// The label `BlockTransact` used to carry, retired with B43.
///
/// A hole for the same reason: block requests are a Call on the console
/// endpoint now, where the device tables live, and a component still speaking
/// label 6 must be refused rather than routed somewhere else.
pub const RETIRED_BLOCK_TRANSACT_LABEL: sel4::Word = 6;
/// B46 universal operations retired in favor of native kernel objects.
pub const RETIRED_NATIVE_IPC_LABELS: [sel4::Word; 6] = [1, 2, 11, 20, 30, 31];

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
    pub const fn slime_status(self) -> i64 {
        match self {
            Self::BadCapability => -1,
            Self::PeerDead => -2,
            Self::QueueFull | Self::WouldBlock => -3,
            // `ERR_BAD_CAP`, with the other capability failures: `sys_send`
            // answers that for a capability it will not move, and a component
            // written against the retired kernel tests for it. It stays a
            // distinct variant because the root's own marker distinguishes an
            // unmovable capability from an absent one, which is a diagnosis a
            // component is deliberately not given.
            Self::UnsupportedCapabilityTransfer => -1,
            Self::InvalidOperation
            | Self::UnsupportedOperation
            | Self::InvalidLength
            | Self::StalePlan
            | Self::WaiterConflict => -4,
            Self::DestinationSlotsExhausted | Self::TransferFailed => -5,
        }
    }
}

/// Root service operation encoded in the seL4 message label.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum Operation {
    Yield = 0,
    FixtureDirective = 5,
    Exit = 3,
    Spawn = 4,
    Unhealthy = 9,
    SupervisionStatus = 12,
    CapDrop = 13,
    DirectoryDerive = 15,
    SharedBufferCreate = 21,
    SharedBufferRelease = 22,
    SharedBufferMap = 23,
    SharedBufferUnmap = 24,
    SharedBufferSeal = 25,
    SharedBufferLoan = 26,
    SharedBufferLoanMap = 27,
    SharedBufferReturn = 28,
    SharedBufferRevoke = 29,
    SupervisionDerive = 32,
    CapabilityExport = 33,
    CapabilityImport = 34,
    CapabilityExportCancel = 35,
    CapabilityExportFinalize = 36,
}

impl Operation {
    pub const fn from_label(label: sel4::Word) -> Result<Self, IpcError> {
        Ok(match label {
            0 => Self::Yield,
            3 => Self::Exit,
            4 => Self::Spawn,
            5 => Self::FixtureDirective,
            9 => Self::Unhealthy,
            12 => Self::SupervisionStatus,
            13 => Self::CapDrop,
            15 => Self::DirectoryDerive,
            21 => Self::SharedBufferCreate,
            22 => Self::SharedBufferRelease,
            23 => Self::SharedBufferMap,
            24 => Self::SharedBufferUnmap,
            25 => Self::SharedBufferSeal,
            26 => Self::SharedBufferLoan,
            27 => Self::SharedBufferLoanMap,
            28 => Self::SharedBufferReturn,
            29 => Self::SharedBufferRevoke,
            32 => Self::SupervisionDerive,
            33 => Self::CapabilityExport,
            34 => Self::CapabilityImport,
            35 => Self::CapabilityExportCancel,
            36 => Self::CapabilityExportFinalize,
            _ => return Err(IpcError::InvalidOperation),
        })
    }

    pub const fn label(self) -> sel4::Word {
        self as u16 as sel4::Word
    }

    /// How the root task answers this operation. Every label a component can
    /// emit resolves here, so a dispatcher covers the whole legacy syscall
    /// surface with a bounded reply instead of an unimplemented panic.
    pub const fn mediation(self) -> Mediation {
        match self {
            // `SYS_YIELD` is the one operation with a direct kernel
            // equivalent: the component invokes `seL4_Yield` itself and never
            // reaches the root endpoint.
            Self::Yield => Mediation::DirectKernel,
            Self::Exit
            | Self::FixtureDirective
            | Self::Spawn
            | Self::Unhealthy
            | Self::SupervisionStatus
            | Self::CapDrop
            | Self::SharedBufferCreate
            | Self::SharedBufferRelease
            | Self::SharedBufferMap
            | Self::SharedBufferUnmap
            | Self::SharedBufferSeal
            | Self::SharedBufferLoan
            | Self::SharedBufferLoanMap
            | Self::SharedBufferReturn
            | Self::SharedBufferRevoke
            | Self::SupervisionDerive
            | Self::CapabilityExport
            | Self::CapabilityImport
            | Self::CapabilityExportCancel
            | Self::CapabilityExportFinalize
            | Self::DirectoryDerive => Mediation::RootService,
        }
    }

    /// The bounded answer for an operation the root task does not mediate.
    /// `None` means the dispatcher must produce a real result.
    pub const fn unmediated_response(self) -> Option<Response> {
        match self.mediation() {
            Mediation::RootService => None,
            Mediation::DirectKernel => Some(Response::error(IpcError::UnsupportedOperation)),
        }
    }
}

/// Who answers an [`Operation`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mediation {
    /// Answered by the root service loop over the child's slot-1 endpoint.
    RootService,
    /// Performed by the component against the kernel with no root round trip.
    ///
    /// B44 removed a third class, `Unavailable`: an operation the root
    /// classified as carrying no mechanism answered `UnsupportedOperation`
    /// and nothing else, which is ABI surface for something the root does not
    /// do. Every such label is gone rather than reclassified, so the class
    /// has no members and no reason to exist.
    DirectKernel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Request {
    pub badge: sel4::Badge,
    pub operation: Operation,
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
/// decoded from it by `fault::decode_fault`, which needs the message rather
/// than the operation this decoder tried to read out of it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reception {
    pub info: sel4::MessageInfo,
    pub badge: sel4::Badge,
    pub request: Result<Request, IpcError>,
}

/// Receive one root-service request. The component transport calls the root
/// endpoint in child CSpace slot 1; the task loop dispatches the returned
/// operation and answers it with [`reply`]. Raw seL4 extra-cap transfer is not
/// part of this fast ABI; logical capability transfer goes through the bounded
/// preflight/commit path below.
/// One console message: the payload descriptor and the fast registers behind
/// it, with no operation label.
///
/// The console endpoint carries exactly one kind of message, so a label would
/// be a constant. It deliberately does not reuse [`Operation`]: that table is
/// the *universal dispatcher's* ABI, and B41's point is that console traffic
/// is no longer part of it.
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
    let operation = Operation::from_label(reception.info.label())?;
    let caps = reception.info.extra_caps();
    if reception.info.caps_unwrapped() != 0 || caps > MAX_MESSAGE_CAPS || caps != 0 {
        return Err(IpcError::UnsupportedCapabilityTransfer);
    }
    Ok(Request {
        badge: reception.badge,
        operation,
        mrs: reception.msg,
        len,
    })
}

/// Reply to the most recent non-MCS request. MR0 carries the bit-exact logical
/// `i64` result and MR1 carries the operation-specific auxiliary value.
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

    #[test]
    fn retired_native_ipc_labels_remain_holes() {
        for label in RETIRED_NATIVE_IPC_LABELS {
            assert_eq!(
                Operation::from_label(label),
                Err(IpcError::InvalidOperation)
            );
        }
    }

    #[test]
    fn declared_service_labels_round_trip() {
        for label in 0..=sel4::Word::from(MAX_OPERATION_LABEL) {
            if RETIRED_NATIVE_IPC_LABELS.contains(&label)
                || [
                    RETIRED_INPUT_READ_LABEL,
                    RETIRED_BLOCK_TRANSACT_LABEL,
                    RETIRED_STORE_TRANSACT_LABEL,
                ]
                .contains(&label)
                || RETIRED_POLICY_LABELS.contains(&label)
                || RETIRED_DIRECTORY_LABELS.contains(&label)
            {
                assert_eq!(
                    Operation::from_label(label),
                    Err(IpcError::InvalidOperation)
                );
                continue;
            }
            if let Ok(operation) = Operation::from_label(label) {
                assert_eq!(operation.label(), label);
            }
        }
    }
}
