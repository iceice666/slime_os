//! Bounded root-side service IPC.
//!
//! Component-to-component messages use declared seL4 Endpoints directly. This
//! module decodes only the bounded wire envelope received by root services;
//! each service owns the meaning of its request labels.

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
