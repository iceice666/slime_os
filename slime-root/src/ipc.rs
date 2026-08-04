//! Bounded root-side IPC and readiness state.
//!
//! The service loop uses [`recv_request`] and [`reply`] for the non-MCS seL4
//! fast path. Task/channel management uses [`Channel`] and [`WaitSet`] before
//! touching CSpace state: a logical send or receive is first preflighted, then
//! its four-capability transfer is committed by the task-owned cap adapter, and
//! only then is the queue mutated. This keeps failed operations atomic and
//! leaves capability authority with its original owner.

/// Logical Slime message bound. Fast transport carries control words; a
/// payload wider than this travels through a shared buffer named by a bounded
/// descriptor, never by growing the message.
pub const MAX_MESSAGE_BYTES: usize = 64;
/// Logical capabilities one message may move, transferred all-or-nothing.
pub const MAX_MESSAGE_CAPS: usize = 4;
/// Depth of one directed logical channel.
pub const CHANNEL_CAPACITY: usize = 16;
/// Sources one task may block on in a single wait.
pub const MAX_WAIT_SOURCES: usize = 9;
/// Message registers the AArch64 fast path carries in architectural registers.
pub const FAST_MESSAGE_REGISTERS: usize = sel4::NUM_FAST_MESSAGE_REGISTERS;
/// Highest label an operation may carry; see [`Operation`].
pub const MAX_OPERATION_LABEL: u16 = Operation::TransferWindowBind as u16;

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
    WaitSetFull,
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
            | Self::WaitSetFull
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
    Send = 1,
    Recv = 2,
    Exit = 3,
    Spawn = 4,
    DebugWrite = 5,
    BlockTransact = 6,
    StoreTransact = 7,
    HealthConfirm = 8,
    Unhealthy = 9,
    RecoveryReconstruct = 10,
    EndpointCreate = 11,
    SupervisionStatus = 12,
    CapDrop = 13,
    DirectoryInspect = 14,
    DirectoryDerive = 15,
    DirectoryCommit = 16,
    InputRead = 17,
    GenerationTransact = 18,
    GenerationReceive = 19,
    Wait = 20,
    SharedBufferCreate = 21,
    SharedBufferRelease = 22,
    SharedBufferMap = 23,
    SharedBufferUnmap = 24,
    SharedBufferSeal = 25,
    SharedBufferLoan = 26,
    SharedBufferLoanMap = 27,
    SharedBufferReturn = 28,
    SharedBufferRevoke = 29,
    CapTransfer = 30,
    /// Declare the component-owned transfer window the seL4 transport stages
    /// oversized payloads through. Only the native transport emits it; the
    /// legacy trap ABI addresses caller memory directly and has no window.
    TransferWindowBind = 31,
}

impl Operation {
    pub const fn from_label(label: sel4::Word) -> Result<Self, IpcError> {
        Ok(match label {
            0 => Self::Yield,
            1 => Self::Send,
            2 => Self::Recv,
            3 => Self::Exit,
            4 => Self::Spawn,
            5 => Self::DebugWrite,
            6 => Self::BlockTransact,
            7 => Self::StoreTransact,
            8 => Self::HealthConfirm,
            9 => Self::Unhealthy,
            10 => Self::RecoveryReconstruct,
            11 => Self::EndpointCreate,
            12 => Self::SupervisionStatus,
            13 => Self::CapDrop,
            14 => Self::DirectoryInspect,
            15 => Self::DirectoryDerive,
            16 => Self::DirectoryCommit,
            17 => Self::InputRead,
            18 => Self::GenerationTransact,
            19 => Self::GenerationReceive,
            20 => Self::Wait,
            21 => Self::SharedBufferCreate,
            22 => Self::SharedBufferRelease,
            23 => Self::SharedBufferMap,
            24 => Self::SharedBufferUnmap,
            25 => Self::SharedBufferSeal,
            26 => Self::SharedBufferLoan,
            27 => Self::SharedBufferLoanMap,
            28 => Self::SharedBufferReturn,
            29 => Self::SharedBufferRevoke,
            30 => Self::CapTransfer,
            31 => Self::TransferWindowBind,
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
            Self::Send
            | Self::Recv
            | Self::Exit
            | Self::Spawn
            | Self::DebugWrite
            | Self::HealthConfirm
            | Self::Unhealthy
            | Self::EndpointCreate
            | Self::SupervisionStatus
            | Self::CapDrop
            | Self::Wait
            | Self::SharedBufferCreate
            | Self::SharedBufferRelease
            | Self::SharedBufferMap
            | Self::SharedBufferUnmap
            | Self::SharedBufferSeal
            | Self::SharedBufferLoan
            | Self::SharedBufferLoanMap
            | Self::SharedBufferReturn
            | Self::SharedBufferRevoke
            | Self::CapTransfer
            | Self::TransferWindowBind => Mediation::RootService,
            // Storage, directory, input, recovery, and generation planes have
            // no seL4 mechanism owner in this cutover. They answer with the
            // ordinary Slime error rather than faulting the caller.
            Self::BlockTransact
            | Self::StoreTransact
            | Self::RecoveryReconstruct
            | Self::DirectoryInspect
            | Self::DirectoryDerive
            | Self::DirectoryCommit
            | Self::InputRead
            | Self::GenerationTransact
            | Self::GenerationReceive => Mediation::Unavailable,
        }
    }

    /// The bounded answer for an operation the root task does not mediate.
    /// `None` means the dispatcher must produce a real result.
    pub const fn unmediated_response(self) -> Option<Response> {
        match self.mediation() {
            Mediation::RootService => None,
            Mediation::DirectKernel | Mediation::Unavailable => {
                Some(Response::error(IpcError::UnsupportedOperation))
            }
        }
    }
}

/// Who answers an [`Operation`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mediation {
    /// Answered by the root service loop over the child's slot-1 endpoint.
    RootService,
    /// Performed by the component against the kernel with no root round trip.
    DirectKernel,
    /// Carries no mechanism in this cutover; answered with a bounded error.
    Unavailable,
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
    if reception.info.extra_caps() != 0 || reception.info.caps_unwrapped() != 0 {
        return Err(IpcError::UnsupportedCapabilityTransfer);
    }
    Ok(Request {
        badge: reception.badge,
        operation: Operation::from_label(reception.info.label())?,
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

pub type TaskKey = u32;
pub type ChannelKey = u32;
pub type SupervisionKey = u32;
pub type LogicalCap = u32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Message {
    bytes: [u8; MAX_MESSAGE_BYTES],
    len: u8,
    caps: [Option<LogicalCap>; MAX_MESSAGE_CAPS],
}

impl Message {
    pub fn new(bytes: &[u8], caps: &[LogicalCap]) -> Result<Self, IpcError> {
        if bytes.len() > MAX_MESSAGE_BYTES || caps.len() > MAX_MESSAGE_CAPS {
            return Err(IpcError::InvalidLength);
        }
        let mut message = Self {
            bytes: [0; MAX_MESSAGE_BYTES],
            len: bytes.len() as u8,
            caps: [None; MAX_MESSAGE_CAPS],
        };
        message.bytes[..bytes.len()].copy_from_slice(bytes);
        for (destination, capability) in message.caps.iter_mut().zip(caps.iter().copied()) {
            *destination = Some(capability);
        }
        Ok(message)
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }

    pub const fn len(&self) -> usize {
        self.len as usize
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn caps(&self) -> &[Option<LogicalCap>; MAX_MESSAGE_CAPS] {
        &self.caps
    }

    pub fn cap_count(&self) -> usize {
        self.caps
            .iter()
            .filter(|capability| capability.is_some())
            .count()
    }
}

impl Default for Message {
    fn default() -> Self {
        Self {
            bytes: [0; MAX_MESSAGE_BYTES],
            len: 0,
            caps: [None; MAX_MESSAGE_CAPS],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WakeDecision {
    pub task: TaskKey,
    pub cause: WakeCause,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WakeCause {
    MessageAvailable(ChannelKey),
    SendCapacity(ChannelKey),
    PeerDeath(ChannelKey),
    Notification(sel4::Badge),
    Supervision(SupervisionKey),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SendPlan {
    revision: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReceivePlan {
    revision: u32,
    required_slots: u8,
}

impl ReceivePlan {
    pub const fn required_slots(self) -> usize {
        self.required_slots as usize
    }
}

/// Fixed-depth directed logical channel. Capability values in queued messages
/// are root-owned logical handles; the task module alone resolves them to
/// concrete CSpace slots through [`CapabilityTransfer`].
#[derive(Debug, Eq, PartialEq)]
pub struct Channel {
    key: ChannelKey,
    queue: [Option<Message>; CHANNEL_CAPACITY],
    head: usize,
    len: usize,
    revision: u32,
    peer_alive: bool,
    recv_waiter: Option<TaskKey>,
    send_waiter: Option<TaskKey>,
}

impl Channel {
    pub const fn new(key: ChannelKey) -> Self {
        Self {
            key,
            queue: [const { None }; CHANNEL_CAPACITY],
            head: 0,
            len: 0,
            revision: 0,
            peer_alive: true,
            recv_waiter: None,
            send_waiter: None,
        }
    }

    pub const fn key(&self) -> ChannelKey {
        self.key
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn capacity(&self) -> usize {
        CHANNEL_CAPACITY
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn is_full(&self) -> bool {
        self.len == CHANNEL_CAPACITY
    }

    pub const fn peer_alive(&self) -> bool {
        self.peer_alive
    }

    pub const fn receive_ready(&self) -> bool {
        self.len != 0 || !self.peer_alive
    }

    pub const fn send_ready(&self) -> bool {
        self.len < CHANNEL_CAPACITY || !self.peer_alive
    }

    pub fn preflight_send(&self) -> Result<SendPlan, IpcError> {
        if !self.peer_alive {
            return Err(IpcError::PeerDead);
        }
        if self.is_full() {
            return Err(IpcError::QueueFull);
        }
        Ok(SendPlan {
            revision: self.revision,
        })
    }

    pub fn commit_send(
        &mut self,
        plan: SendPlan,
        message: Message,
    ) -> Result<Option<WakeDecision>, IpcError> {
        if plan.revision != self.revision {
            return Err(IpcError::StalePlan);
        }
        if !self.peer_alive {
            return Err(IpcError::PeerDead);
        }
        if self.is_full() {
            return Err(IpcError::QueueFull);
        }
        let tail = (self.head + self.len) % CHANNEL_CAPACITY;
        self.queue[tail] = Some(message);
        self.len += 1;
        self.bump_revision();
        Ok(self.recv_waiter.take().map(|task| WakeDecision {
            task,
            cause: WakeCause::MessageAvailable(self.key),
        }))
    }

    pub fn preflight_receive(&self, available_slots: usize) -> Result<ReceivePlan, IpcError> {
        let Some(message) = self.front() else {
            return if self.peer_alive {
                Err(IpcError::WouldBlock)
            } else {
                Err(IpcError::PeerDead)
            };
        };
        let required_slots = message.cap_count();
        if available_slots < required_slots {
            return Err(IpcError::DestinationSlotsExhausted);
        }
        Ok(ReceivePlan {
            revision: self.revision,
            required_slots: required_slots as u8,
        })
    }

    pub fn commit_receive(
        &mut self,
        plan: ReceivePlan,
    ) -> Result<(Message, Option<WakeDecision>), IpcError> {
        if plan.revision != self.revision {
            return Err(IpcError::StalePlan);
        }
        let message = self.pop_front().ok_or(IpcError::StalePlan)?;
        if message.cap_count() != plan.required_slots() {
            return Err(IpcError::StalePlan);
        }
        self.bump_revision();
        let wake = self.send_waiter.take().map(|task| WakeDecision {
            task,
            cause: WakeCause::SendCapacity(self.key),
        });
        Ok((message, wake))
    }

    pub fn register_receive_waiter(&mut self, task: TaskKey) -> Result<(), IpcError> {
        register_waiter(&mut self.recv_waiter, task)
    }

    pub fn register_send_waiter(&mut self, task: TaskKey) -> Result<(), IpcError> {
        register_waiter(&mut self.send_waiter, task)
    }

    pub fn clear_waiter(&mut self, task: TaskKey) {
        if self.recv_waiter == Some(task) {
            self.recv_waiter = None;
        }
        if self.send_waiter == Some(task) {
            self.send_waiter = None;
        }
    }

    pub fn mark_peer_dead(&mut self) -> WakeBatch<2> {
        if self.peer_alive {
            self.peer_alive = false;
            self.bump_revision();
        }
        let mut wakes = WakeBatch::new();
        if let Some(task) = self.recv_waiter.take() {
            wakes.push(WakeDecision {
                task,
                cause: WakeCause::PeerDeath(self.key),
            });
        }
        if let Some(task) = self.send_waiter.take() {
            wakes.push(WakeDecision {
                task,
                cause: WakeCause::PeerDeath(self.key),
            });
        }
        wakes
    }

    fn front(&self) -> Option<&Message> {
        self.queue[self.head].as_ref()
    }

    fn pop_front(&mut self) -> Option<Message> {
        let message = self.queue[self.head].take()?;
        self.head = (self.head + 1) % CHANNEL_CAPACITY;
        self.len -= 1;
        Some(message)
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

fn register_waiter(slot: &mut Option<TaskKey>, task: TaskKey) -> Result<(), IpcError> {
    match *slot {
        None => {
            *slot = Some(task);
            Ok(())
        }
        Some(existing) if existing == task => Ok(()),
        Some(_) => Err(IpcError::WaiterConflict),
    }
}

/// Task-owned adapter used after queue and destination-capacity preflight. It
/// must either move every listed logical capability or move none of them.
pub trait CapabilityTransfer {
    type Error;

    fn transfer_atomic(
        &mut self,
        capabilities: &[Option<LogicalCap>; MAX_MESSAGE_CAPS],
    ) -> Result<[Option<LogicalCap>; MAX_MESSAGE_CAPS], Self::Error>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReceiveOutcome {
    pub message: Message,
    pub wake: Option<WakeDecision>,
}

/// Send with every bound checked before any capability moves. A failed
/// transfer leaves the sender holding its capabilities and enqueues nothing.
pub fn send_atomic<T: CapabilityTransfer>(
    channel: &mut Channel,
    mut message: Message,
    transfer: &mut T,
) -> Result<Option<WakeDecision>, IpcError> {
    let plan = channel.preflight_send()?;
    message.caps = transfer
        .transfer_atomic(message.caps())
        .map_err(|_| IpcError::TransferFailed)?;
    channel.commit_send(plan, message)
}

/// Receive with all cap-table checks before mutation. A failed cap transfer
/// leaves the message queued and consumes no sender wake.
pub fn receive_atomic<T: CapabilityTransfer>(
    channel: &mut Channel,
    available_slots: usize,
    transfer: &mut T,
) -> Result<ReceiveOutcome, IpcError> {
    let plan = channel.preflight_receive(available_slots)?;
    let original = *channel.front().ok_or(IpcError::StalePlan)?;
    let transferred = transfer
        .transfer_atomic(original.caps())
        .map_err(|_| IpcError::TransferFailed)?;
    let (mut message, wake) = channel.commit_receive(plan)?;
    message.caps = transferred;
    Ok(ReceiveOutcome { message, wake })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadinessSource {
    EndpointReceive(ChannelKey),
    EndpointSend(ChannelKey),
    NotificationBadge(sel4::Badge),
    Supervision(SupervisionKey),
}

pub trait ReadinessProbe {
    fn is_ready(&self, source: ReadinessSource) -> bool;

    fn register(&mut self, task: TaskKey, source: ReadinessSource) -> Result<(), IpcError>;

    fn clear(&mut self, task: TaskKey, source: ReadinessSource);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitDecision {
    Ready(ReadinessSource),
    Registered,
}

/// Bounded multi-source wait descriptor. `arm` probes, registers, and probes
/// again, closing the lost-wakeup window without assuming that one wake means
/// one source is consumable; callers re-poll every source after waking.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WaitSet {
    sources: [Option<ReadinessSource>; MAX_WAIT_SOURCES],
    len: usize,
}

impl WaitSet {
    pub const fn new() -> Self {
        Self {
            sources: [None; MAX_WAIT_SOURCES],
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn push(&mut self, source: ReadinessSource) -> Result<(), IpcError> {
        if self.len == MAX_WAIT_SOURCES {
            return Err(IpcError::WaitSetFull);
        }
        if self.sources[..self.len].contains(&Some(source)) {
            return Ok(());
        }
        self.sources[self.len] = Some(source);
        self.len += 1;
        Ok(())
    }

    pub fn arm<P: ReadinessProbe>(
        &self,
        task: TaskKey,
        probe: &mut P,
    ) -> Result<WaitDecision, IpcError> {
        if let Some(source) = self.first_ready(probe) {
            return Ok(WaitDecision::Ready(source));
        }
        for (registered, source) in self.iter().enumerate() {
            if let Err(error) = probe.register(task, source) {
                for rollback in self.iter().take(registered) {
                    probe.clear(task, rollback);
                }
                return Err(error);
            }
        }
        if let Some(source) = self.first_ready(probe) {
            self.clear(task, probe);
            return Ok(WaitDecision::Ready(source));
        }
        Ok(WaitDecision::Registered)
    }

    pub fn clear<P: ReadinessProbe>(&self, task: TaskKey, probe: &mut P) {
        for source in self.iter() {
            probe.clear(task, source);
        }
    }

    fn first_ready<P: ReadinessProbe>(&self, probe: &P) -> Option<ReadinessSource> {
        self.iter().find(|source| probe.is_ready(*source))
    }

    fn iter(&self) -> impl Iterator<Item = ReadinessSource> + '_ {
        self.sources[..self.len].iter().flatten().copied()
    }
}

impl Default for WaitSet {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WakeBatch<const CAPACITY: usize> {
    entries: [Option<WakeDecision>; CAPACITY],
    len: usize,
}

impl<const CAPACITY: usize> WakeBatch<CAPACITY> {
    pub const fn new() -> Self {
        Self {
            entries: [None; CAPACITY],
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, index: usize) -> Option<WakeDecision> {
        if index >= self.len {
            return None;
        }
        self.entries[index]
    }

    fn push(&mut self, wake: WakeDecision) {
        if self.entries[..self.len].contains(&Some(wake)) {
            return;
        }
        debug_assert!(self.len < CAPACITY);
        self.entries[self.len] = Some(wake);
        self.len += 1;
    }
}

impl<const CAPACITY: usize> Default for WakeBatch<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RejectTransfer;

    impl CapabilityTransfer for RejectTransfer {
        type Error = ();

        fn transfer_atomic(
            &mut self,
            _capabilities: &[Option<LogicalCap>; MAX_MESSAGE_CAPS],
        ) -> Result<[Option<LogicalCap>; MAX_MESSAGE_CAPS], Self::Error> {
            Err(())
        }
    }

    #[test]
    fn receive_transfer_failure_keeps_message_and_capacity() {
        let mut channel = Channel::new(7);
        let plan = channel.preflight_send().unwrap();
        channel
            .commit_send(plan, Message::new(b"data", &[41, 42]).unwrap())
            .unwrap();

        let result = receive_atomic(&mut channel, 2, &mut RejectTransfer);
        assert_eq!(result, Err(IpcError::TransferFailed));
        assert_eq!(channel.len(), 1);
        assert_eq!(channel.preflight_receive(2).unwrap().required_slots(), 2);
    }

    #[test]
    fn send_transfer_failure_enqueues_nothing() {
        let mut channel = Channel::new(5);
        let message = Message::new(b"grant", &[3]).unwrap();
        assert_eq!(
            send_atomic(&mut channel, message, &mut RejectTransfer),
            Err(IpcError::TransferFailed)
        );
        assert!(channel.is_empty());
    }

    #[test]
    fn send_to_dead_peer_never_reaches_transfer() {
        let mut channel = Channel::new(6);
        let _ = channel.mark_peer_dead();
        // `RejectTransfer` would report `TransferFailed` if preflight let the
        // capability move before observing peer death.
        assert_eq!(
            send_atomic(&mut channel, Message::default(), &mut RejectTransfer),
            Err(IpcError::PeerDead)
        );
    }

    #[test]
    fn peer_death_wakes_receive_and_send_waiters() {
        let mut channel = Channel::new(9);
        channel.register_receive_waiter(11).unwrap();
        channel.register_send_waiter(12).unwrap();
        let wakes = channel.mark_peer_dead();
        assert_eq!(wakes.len(), 2);
        assert_eq!(channel.preflight_send(), Err(IpcError::PeerDead));
        assert_eq!(channel.preflight_receive(0), Err(IpcError::PeerDead));
    }

    #[test]
    fn queue_bound_is_exact() {
        let mut channel = Channel::new(1);
        for _ in 0..CHANNEL_CAPACITY {
            let plan = channel.preflight_send().unwrap();
            channel.commit_send(plan, Message::default()).unwrap();
        }
        assert_eq!(channel.len(), CHANNEL_CAPACITY);
        assert_eq!(channel.preflight_send(), Err(IpcError::QueueFull));
    }

    struct RaceProbe {
        ready_on_second_pass: bool,
        probes: usize,
        registrations: usize,
        clears: usize,
    }

    impl ReadinessProbe for RaceProbe {
        fn is_ready(&self, _source: ReadinessSource) -> bool {
            self.ready_on_second_pass && self.probes >= 1
        }

        fn register(&mut self, _task: TaskKey, _source: ReadinessSource) -> Result<(), IpcError> {
            self.registrations += 1;
            self.probes += 1;
            Ok(())
        }

        fn clear(&mut self, _task: TaskKey, _source: ReadinessSource) {
            self.clears += 1;
        }
    }

    #[test]
    fn wait_set_rechecks_after_registration() {
        let mut set = WaitSet::new();
        set.push(ReadinessSource::EndpointReceive(4)).unwrap();
        set.push(ReadinessSource::Supervision(8)).unwrap();
        let mut probe = RaceProbe {
            ready_on_second_pass: true,
            probes: 0,
            registrations: 0,
            clears: 0,
        };
        let decision = set.arm(3, &mut probe).unwrap();
        assert_eq!(
            decision,
            WaitDecision::Ready(ReadinessSource::EndpointReceive(4))
        );
        assert_eq!(probe.registrations, 2);
        assert_eq!(probe.clears, 2);
    }

    #[test]
    fn every_legacy_label_resolves_to_a_bounded_answer() {
        for label in 0..=sel4::Word::from(MAX_OPERATION_LABEL) {
            let operation = Operation::from_label(label).expect("legacy label is known");
            assert_eq!(operation.label(), label);
            match operation.mediation() {
                Mediation::RootService => assert_eq!(operation.unmediated_response(), None),
                Mediation::DirectKernel | Mediation::Unavailable => assert_eq!(
                    operation.unmediated_response(),
                    Some(Response::error(IpcError::UnsupportedOperation))
                ),
            }
        }
        assert_eq!(
            Operation::from_label(sel4::Word::from(MAX_OPERATION_LABEL) + 1),
            Err(IpcError::InvalidOperation)
        );
    }
}
