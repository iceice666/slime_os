//! Bounded, holder-bound TCP connections over the service's smoltcp interface.

use crate::{dns, resolver};
use boot_contracts::network_application::{Application, Backend, NetworkApplications, Role};
use boot_contracts::network_destination::{
    Address, NetworkDestinations, RIGHT_CONNECT, RIGHT_RECV, RIGHT_SEND, Transport,
};
use slime_proto::network_service::{self as wire, WireNetworkRequest};
use smoltcp::iface::{Interface, SocketHandle, SocketSet, SocketStorage};
use smoltcp::socket::tcp::{Socket, SocketBuffer, State};
use smoltcp::socket::udp;
use smoltcp::time::{Duration, Instant};
use smoltcp::wire::{
    EthernetFrame, EthernetProtocol, IpAddress, IpEndpoint, IpProtocol, Ipv4Address, Ipv4Packet,
    TcpPacket, TcpSeqNumber,
};

pub const SOCKETS: usize = 4;
pub const BUFFER_BYTES: usize = 2048;
pub const MAX_INCARNATION: u64 = (1 << 30) - 1;
pub const OPERATION_TIMEOUT: Duration = Duration::from_secs(10);
const FIRST_EPHEMERAL_PORT: u16 = 49152;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Success,
    Denied,
    Malformed,
    Unsupported,
    Exhausted,
    WouldBlock,
    Refused,
    Timeout,
    Reset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Completion {
    pub status: Status,
    pub capability: u64,
    pub capability_kind: u8,
    pub transferred: usize,
    pub eof: bool,
}

impl Completion {
    fn status(status: Status) -> Self {
        Self {
            status,
            capability: 0,
            capability_kind: wire::CAPABILITY_NONE,
            transferred: 0,
            eof: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Complete(Completion),
    Pending { capability: u64 },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Resolving,
    Connecting,
    Connected,
    Closing,
}

#[derive(Clone, Copy)]
struct Connection {
    id: u64,
    holder: [u8; 32],
    destination: usize,
    rights: u16,
    phase: Phase,
    deadline: Instant,
    retry_limit: u32,
    retries: u32,
    sent_end: Option<TcpSeqNumber>,
    terminal: Option<Status>,
    peer_fin_seen: bool,
    listener: Option<u64>,
}

#[derive(Clone, Copy)]
struct Listener {
    id: u64,
    holder: [u8; 32],
    peer: [u8; 32],
    address: [u8; 4],
    port: u16,
    rights: u16,
    accepted_limit: u32,
    byte_budget: u32,
    timer_budget: u32,
    queue_depth: u32,
    retry_limit: u32,
    socket: Option<usize>,
    active: bool,
    retries: u32,
    sent_end: Option<TcpSeqNumber>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Reclaimed {
    pub handles: usize,
    pub sockets: usize,
    pub bytes: usize,
}

#[derive(Clone, Copy)]
struct Reset {
    local: IpEndpoint,
    remote: IpEndpoint,
    deadline: Instant,
}

#[derive(Clone, Copy)]
pub struct DnsObservation {
    pub name: [u8; 24],
    pub name_len: usize,
    pub addresses: [[u8; 4]; dns::MAX_ADDRESSES],
    pub count: usize,
    pub ttl_seconds: u32,
    pub selected: [u8; 4],
    pub attempt: usize,
    pub port: u16,
}

#[derive(Clone, Copy)]
struct NameConnect {
    index: usize,
    port: u16,
    addresses: [[u8; 4]; dns::MAX_ADDRESSES],
    count: usize,
    next: usize,
    deadline: Instant,
    expiry: Instant,
}

pub struct Engine<'a> {
    sockets: SocketSet<'a>,
    handles: [SocketHandle; SOCKETS],
    connections: [Option<Connection>; SOCKETS],
    listeners: [Option<Listener>; SOCKETS],
    namespace: u64,
    serial: u32,
    next_port: u16,
    resets: [Option<Reset>; SOCKETS],
    last_now: Instant,
    resolver: Option<resolver::Resolver>,
    dns_capacity: bool,
    name_connect: Option<NameConnect>,
    dns_observation: Option<DnsObservation>,
    peaks: Peaks,
    timeout_reclaimed: Option<Reclaimed>,
}

/// The most bytes any socket held queued at once in each direction.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Peaks {
    pub rx: usize,
    pub tx: usize,
}

impl<'a> Engine<'a> {
    pub fn new(
        storage: &'a mut [SocketStorage<'a>],
        rx: &'a mut [[u8; BUFFER_BYTES]; SOCKETS],
        tx: &'a mut [[u8; BUFFER_BYTES]; SOCKETS],
        epoch: u64,
        backend: u8,
    ) -> Result<Self, Status> {
        if epoch == 0 || epoch > MAX_INCARNATION || backend > 1 || storage.len() < SOCKETS {
            return Err(Status::Exhausted);
        }
        let dns_capacity = storage.len() > SOCKETS && backend == 0;
        let mut sockets = SocketSet::new(&mut storage[..]);
        let mut buffers = rx.iter_mut().zip(tx.iter_mut());
        let handles = core::array::from_fn(|_| {
            let (rx, tx) = buffers.next().expect("one buffer pair per socket");
            sockets.add(Socket::new(
                SocketBuffer::new(&mut rx[..]),
                SocketBuffer::new(&mut tx[..]),
            ))
        });
        Ok(Self {
            sockets,
            handles,
            connections: [None; SOCKETS],
            listeners: [None; SOCKETS],
            // The caller must durably allocate a fresh incarnation before admission.
            // Checked disjoint fields prevent aliasing across incarnations/backends.
            namespace: (1 << 63) | (epoch << 33) | (u64::from(backend) << 32),
            serial: 0,
            next_port: FIRST_EPHEMERAL_PORT,
            resets: [None; SOCKETS],
            last_now: Instant::ZERO,
            resolver: None,
            dns_capacity,
            name_connect: None,
            dns_observation: None,
            peaks: Peaks::default(),
            timeout_reclaimed: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn enable_dns(
        &mut self,
        destinations: &NetworkDestinations<'_>,
        seed: [u8; 32],
        rx_meta: &'a mut [udp::PacketMetadata; 1],
        rx: &'a mut [u8; resolver::BUFFER_BYTES],
        tx_meta: &'a mut [udp::PacketMetadata; 1],
        tx: &'a mut [u8; resolver::BUFFER_BYTES],
    ) -> Result<(), Status> {
        if self.resolver.is_some() || !self.dns_capacity {
            return Err(Status::Denied);
        }
        let socket = udp::Socket::new(
            udp::PacketBuffer::new(&mut rx_meta[..], &mut rx[..]),
            udp::PacketBuffer::new(&mut tx_meta[..], &mut tx[..]),
        );
        let handle = self.sockets.add(socket);
        match resolver::Resolver::new(handle, destinations, seed) {
            Ok(resolver) => {
                self.resolver = Some(resolver);
                Ok(())
            }
            Err(error) => {
                self.sockets.remove(handle);
                Err(error)
            }
        }
    }

    pub fn take_dns_observation(&mut self) -> Option<DnsObservation> {
        self.dns_observation.take()
    }

    /// Resources released with connections that ended in a typed timeout since
    /// the last call, counted once each when the holder disposes of the handle.
    pub fn take_timeout_reclamation(&mut self) -> Option<Reclaimed> {
        self.timeout_reclaimed.take()
    }

    /// Fold the current receive and transmit queue depths into the peaks.
    /// Every socket owns fixed buffers, so a peak can never exceed them.
    pub fn observe_peaks(&mut self) {
        for handle in self.handles {
            let socket = self.sockets.get::<Socket>(handle);
            self.peaks.rx = self.peaks.rx.max(socket.recv_queue());
            self.peaks.tx = self.peaks.tx.max(socket.send_queue());
        }
    }

    pub const fn peaks(&self) -> Peaks {
        self.peaks
    }

    fn cancel_dns(&mut self) {
        self.name_connect = None;
        if let Some(resolver) = self.resolver.as_mut() {
            resolver.cancel(&mut self.sockets);
        }
    }

    /// Advance only the pending service-owned name-to-connect transaction.
    /// The original destination row remains attached to the connection; answers
    /// never mint numeric authority or authorize any other holder/name/port.
    pub fn advance_dns(
        &mut self,
        destinations: &NetworkDestinations<'_>,
        iface: &mut Interface,
        now: Instant,
    ) {
        self.last_now = now;
        let Some(mut pending) = self.name_connect else {
            return;
        };
        let mut observed_answer = None;
        let Some(connection) = self.connections[pending.index] else {
            self.cancel_dns();
            return;
        };
        if now >= pending.deadline || (pending.count != 0 && now >= pending.expiry) {
            self.fail_dns(pending.index, Status::Timeout);
            return;
        }
        if connection.phase == Phase::Resolving {
            let Some(result) = self
                .resolver
                .as_mut()
                .expect("pending resolver")
                .poll(&mut self.sockets, now)
            else {
                return;
            };
            self.resolver
                .as_mut()
                .expect("pending resolver")
                .cancel(&mut self.sockets);
            let answer = match result {
                Ok(answer) => answer,
                Err(status) => {
                    self.fail_dns(pending.index, status);
                    return;
                }
            };
            let Some(destination) = destinations.destination(connection.destination) else {
                self.fail_dns(pending.index, Status::Denied);
                return;
            };
            if answer.ttl_seconds == 0 || answer.count > destination.dns_record_limit as usize {
                self.fail_dns(pending.index, Status::Denied);
                return;
            }
            // A controlled fixture is an exact additional numeric grant, not a
            // private-range escape hatch. Reject a mixed allowed/forbidden set.
            if answer.addresses[..answer.count].iter().any(|address| {
                !(dns::is_public_address(*address)
                    || (self
                        .resolver
                        .as_ref()
                        .is_some_and(resolver::Resolver::controlled)
                        && destinations.authorizes(
                            &connection.holder,
                            Transport::Tcp,
                            Address::Ipv4(*address),
                            pending.port,
                            boot_contracts::network_destination::Right::Connect,
                        )))
            }) {
                self.fail_dns(pending.index, Status::Denied);
                return;
            }
            observed_answer = Some(answer);
            pending.addresses = answer.addresses;
            pending.count = answer.count;
            pending.expiry = match answer
                .expires_at_millis(now.total_millis() as u64)
                .and_then(|value| i64::try_from(value).ok())
            {
                Some(expiry) => Instant::from_millis(expiry).min(now + Duration::from_secs(60)),
                None => {
                    self.fail_dns(pending.index, Status::Timeout);
                    return;
                }
            };
        } else {
            let state = self
                .sockets
                .get::<Socket>(self.handles[pending.index])
                .state();
            if matches!(state, State::Established | State::CloseWait)
                && connection.terminal.is_none()
            {
                self.sockets
                    .get_mut::<Socket>(self.handles[pending.index])
                    .set_timeout(Some(OPERATION_TIMEOUT));
                self.name_connect = None;
                return;
            }
            if connection.terminal.is_none() && state != State::Closed && now < connection.deadline
            {
                return;
            }
        }
        if pending.next == pending.count {
            self.fail_dns(pending.index, Status::Refused);
            return;
        }
        // No application data has been sent: address fallback occurs only while
        // connecting, never as transparent replay of a partially sent request.
        self.sockets
            .get_mut::<Socket>(self.handles[pending.index])
            .abort();
        self.resets[pending.index] = None;
        let Some(port) = self.ephemeral_port() else {
            self.fail_dns(pending.index, Status::Exhausted);
            return;
        };
        let socket = self.sockets.get_mut::<Socket>(self.handles[pending.index]);
        if socket
            .connect(
                iface.context(),
                (
                    IpAddress::Ipv4(Ipv4Address::from(pending.addresses[pending.next])),
                    pending.port,
                ),
                port,
            )
            .is_err()
        {
            self.fail_dns(pending.index, Status::Refused);
            return;
        }
        socket.set_timeout(Some(Duration::from_secs(2)));
        socket.set_ack_delay(None);
        self.next_port = port.checked_add(1).unwrap_or(FIRST_EPHEMERAL_PORT);
        if let Some(destination) = destinations.destination(connection.destination)
            && let Address::Dns(name) = destination.address
        {
            let mut query_name = [0; 24];
            let Some(target) = query_name.get_mut(..name.len()) else {
                self.fail_dns(pending.index, Status::Denied);
                return;
            };
            target.copy_from_slice(name);
            self.dns_observation = Some(DnsObservation {
                name: query_name,
                name_len: name.len(),
                addresses: pending.addresses,
                count: observed_answer.map_or(0, |answer| answer.count),
                ttl_seconds: observed_answer.map_or(0, |answer| answer.ttl_seconds),
                selected: pending.addresses[pending.next],
                attempt: pending.next + 1,
                port: pending.port,
            });
        }
        pending.next += 1;
        self.connections[pending.index] = Some(Connection {
            phase: Phase::Connecting,
            deadline: now + Duration::from_secs(2),
            terminal: None,
            sent_end: None,
            ..connection
        });
        self.name_connect = Some(pending);
    }

    fn fail_dns(&mut self, index: usize, status: Status) {
        self.cancel_dns();
        self.abort_socket(index);
        if let Some(connection) = self.connections[index].as_mut() {
            connection.terminal = Some(status);
        }
    }

    /// Poll this set before draining every produced frame through `permit_egress`.
    /// Drain before handling requests that can release or reuse a connection.
    pub fn sockets(&mut self) -> &mut SocketSet<'a> {
        &mut self.sockets
    }

    /// Call after interface polling and before publishing each queued frame.
    /// Only TCP has holder accounting; interface ARP and ICMP bypass this guard.
    /// A segment overlapping an already-transmitted sequence consumes one retry;
    /// the budget is conservative and applies over the whole connection lifetime.
    pub fn permit_egress(&mut self, frame: &[u8]) -> bool {
        let Ok(ethernet) = EthernetFrame::new_checked(frame) else {
            return false;
        };
        if ethernet.ethertype() != EthernetProtocol::Ipv4 {
            return true;
        }
        let Ok(ip) = Ipv4Packet::new_checked(ethernet.payload()) else {
            return false;
        };
        if ip.next_header() == IpProtocol::Udp {
            return self
                .resolver
                .as_mut()
                .is_some_and(|resolver| resolver.permit_egress(&ip));
        }
        if ip.next_header() != IpProtocol::Tcp {
            return true;
        }
        let Ok(tcp) = TcpPacket::new_checked(ip.payload()) else {
            return false;
        };
        let local = IpEndpoint::new(IpAddress::Ipv4(ip.src_addr()), tcp.src_port());
        let remote = IpEndpoint::new(IpAddress::Ipv4(ip.dst_addr()), tcp.dst_port());
        if tcp.rst() {
            if !tcp.payload().is_empty() || tcp.syn() || tcp.fin() {
                return false;
            }
            if let Some(index) = self.resets.iter().position(|reset| {
                reset.is_some_and(|reset| reset.local == local && reset.remote == remote)
            }) {
                self.resets[index] = None;
                return true;
            }
            return false;
        }
        let Some(index) = self.handles.iter().position(|handle| {
            let socket = self.sockets.get::<Socket>(*handle);
            socket.local_endpoint() == Some(local) && socket.remote_endpoint() == Some(remote)
        }) else {
            return false;
        };
        if let Some(listener_index) = self
            .listeners
            .iter()
            .position(|listener| listener.is_some_and(|listener| listener.socket == Some(index)))
        {
            let listener = self.listeners[listener_index].expect("located listener");
            let admitted_peer =
                self.connections
                    .iter()
                    .enumerate()
                    .any(|(peer_index, connection)| {
                        connection.is_some_and(|connection| connection.holder == listener.peer)
                            && self
                                .sockets
                                .get::<Socket>(self.handles[peer_index])
                                .local_endpoint()
                                == Some(remote)
                            && self
                                .sockets
                                .get::<Socket>(self.handles[peer_index])
                                .remote_endpoint()
                                == Some(local)
                    });
            if !admitted_peer {
                return false;
            }
            let length = tcp.payload().len() + usize::from(tcp.syn()) + usize::from(tcp.fin());
            if length != 0 {
                let listener = self.listeners[listener_index]
                    .as_mut()
                    .expect("located listener");
                if listener.sent_end.is_some_and(|end| tcp.seq_number() < end) {
                    if listener.retries >= listener.retry_limit {
                        self.abort_socket(index);
                        return false;
                    }
                    listener.retries += 1;
                }
                let end = tcp.seq_number() + length;
                listener.sent_end = Some(listener.sent_end.map_or(end, |old| old.max(end)));
            }
            return true;
        }
        let Some(connection) = self.connections[index].as_mut() else {
            // TIME-WAIT may acknowledge duplicate FINs, but cannot initiate data.
            return self.sockets.get::<Socket>(self.handles[index]).state() == State::TimeWait
                && tcp.payload().is_empty()
                && !tcp.syn()
                && !tcp.fin()
                && !tcp.rst();
        };
        if connection.terminal.is_some() {
            return tcp.rst();
        }
        let length = tcp.payload().len() + usize::from(tcp.syn()) + usize::from(tcp.fin());
        if length == 0 {
            return true;
        }
        let end = tcp.seq_number() + length;
        if connection
            .sent_end
            .is_some_and(|sent| tcp.seq_number() < sent)
        {
            if connection.retries >= connection.retry_limit {
                connection.terminal = Some(Status::Timeout);
                self.abort_socket(index);
                return false;
            }
            connection.retries += 1;
        }
        connection.sent_end = Some(connection.sent_end.map_or(end, |sent| sent.max(end)));
        true
    }

    fn abort_socket(&mut self, index: usize) {
        let socket = self.sockets.get_mut::<Socket>(self.handles[index]);
        if let (Some(local), Some(remote)) = (socket.local_endpoint(), socket.remote_endpoint()) {
            self.resets[index] = Some(Reset {
                local,
                remote,
                deadline: self.last_now + OPERATION_TIMEOUT,
            });
        }
        socket.abort();
    }

    pub fn tick(&mut self, now: Instant) {
        self.last_now = now;
        for reset in &mut self.resets {
            if reset.is_some_and(|reset| now >= reset.deadline) {
                *reset = None;
            }
        }
        self.refresh_listeners();
    }

    pub fn reset_all(&mut self, now: Instant) -> Reclaimed {
        self.last_now = now;
        let handles = self.allocated();
        let mut sockets = 0;
        for index in 0..SOCKETS {
            if self.connections[index].is_some()
                || self.resets[index].is_some()
                || self.sockets.get::<Socket>(self.handles[index]).state() != State::Closed
                || self
                    .listeners
                    .iter()
                    .flatten()
                    .any(|listener| listener.socket == Some(index))
            {
                self.abort_socket(index);
                sockets += 1;
            }
        }
        self.cancel_dns();
        self.connections.fill(None);
        self.listeners.fill(None);
        Reclaimed {
            handles,
            sockets,
            bytes: sockets * 2 * BUFFER_BYTES,
        }
    }

    fn free_socket(&self) -> Option<usize> {
        (0..SOCKETS).find(|index| {
            self.connections[*index].is_none()
                && self.resets[*index].is_none()
                && !self
                    .listeners
                    .iter()
                    .flatten()
                    .any(|listener| listener.socket == Some(*index))
                && self.sockets.get::<Socket>(self.handles[*index]).state() == State::Closed
        })
    }

    fn ephemeral_port(&self) -> Option<u16> {
        let mut port = self.next_port;
        loop {
            // Retain both ends: a loopback peer can outlive its released client.
            // Closed sockets may still hold a tuple for a queued reset.
            let occupied = self.handles.iter().any(|handle| {
                let socket = self.sockets.get::<Socket>(*handle);
                socket
                    .local_endpoint()
                    .is_some_and(|endpoint| endpoint.port == port)
                    || socket
                        .remote_endpoint()
                        .is_some_and(|endpoint| endpoint.port == port)
            }) || self
                .resets
                .iter()
                .flatten()
                .any(|reset| reset.local.port == port || reset.remote.port == port)
                || self
                    .listeners
                    .iter()
                    .flatten()
                    .any(|listener| listener.port == port);
            if !occupied {
                return Some(port);
            }
            port = port.checked_add(1).unwrap_or(FIRST_EPHEMERAL_PORT);
            if port == self.next_port {
                return None;
            }
        }
    }

    fn next_id(&mut self) -> Option<u64> {
        self.serial = self.serial.checked_add(1)?;
        Some(self.namespace | u64::from(self.serial))
    }

    fn children(&self, listener: u64) -> usize {
        self.connections
            .iter()
            .flatten()
            .filter(|connection| connection.listener == Some(listener))
            .count()
    }

    fn arm_listener(&mut self, index: usize) -> bool {
        let listener = self.listeners[index].expect("listener exists");
        let children = self.children(listener.id) as u32;
        if !listener.active
            || listener.socket.is_some()
            || children >= listener.accepted_limit
            || children + 1 > listener.timer_budget
            || children + 1 > listener.queue_depth
            || (children + 1) * (2 * BUFFER_BYTES as u32) > listener.byte_budget
        {
            return false;
        }
        let Some(socket_index) = self.free_socket() else {
            return false;
        };
        let socket = self.sockets.get_mut::<Socket>(self.handles[socket_index]);
        if socket
            .listen((
                IpAddress::Ipv4(Ipv4Address::from(listener.address)),
                listener.port,
            ))
            .is_err()
        {
            return false;
        }
        socket.set_timeout(Some(OPERATION_TIMEOUT));
        socket.set_ack_delay(None);
        self.listeners[index] = Some(Listener {
            socket: Some(socket_index),
            retries: 0,
            sent_end: None,
            ..listener
        });
        true
    }

    fn refresh_listeners(&mut self) {
        for index in 0..SOCKETS {
            let Some(listener) = self.listeners[index] else {
                continue;
            };
            if listener.socket.is_some_and(|slot| {
                self.sockets.get::<Socket>(self.handles[slot]).state() == State::Closed
            }) {
                self.listeners[index]
                    .as_mut()
                    .expect("listener exists")
                    .socket = None;
            }
            if !listener.active && self.children(listener.id) == 0 {
                self.listeners[index] = None;
            } else {
                self.arm_listener(index);
            }
        }
    }

    pub fn listen(
        &mut self,
        holder: [u8; 32],
        policy: &Application<'_>,
        request: WireNetworkRequest,
        now: Instant,
    ) -> Outcome {
        self.last_now = now;
        let fail = |status| Outcome::Complete(Completion::status(status));
        if !slime_proto::valid_network_request(&request) || request.op != wire::OP_LISTEN {
            return fail(Status::Malformed);
        }
        if policy.holder_identity != holder
            || policy.role != Role::Listener
            || policy.backend != Backend::Loopback
            || policy.backlog != 1
            || policy.rights & boot_contracts::network_application::RIGHT_LISTEN == 0
            || request.transport != wire::TRANSPORT_TCP
            || request.address_kind != wire::ADDRESS_IPV4
            || request.endpoint[..4] != policy.local_ipv4
            || request.port != policy.local_port
        {
            return fail(Status::Denied);
        }
        self.refresh_listeners();
        if self.listeners.iter().flatten().any(|listener| {
            listener.address == policy.local_ipv4 && listener.port == policy.local_port
        }) {
            return fail(Status::Exhausted);
        }
        let Some(index) = self.listeners.iter().position(Option::is_none) else {
            return fail(Status::Exhausted);
        };
        let Some(id) = self.next_id() else {
            return fail(Status::Exhausted);
        };
        self.listeners[index] = Some(Listener {
            id,
            holder,
            peer: policy.allowed_peer_identity,
            address: policy.local_ipv4,
            port: policy.local_port,
            rights: policy.rights,
            accepted_limit: policy.accepted_socket_limit,
            byte_budget: policy.byte_budget,
            timer_budget: policy.timer_budget,
            queue_depth: policy.queue_depth,
            retry_limit: policy.retry_limit,
            socket: None,
            active: true,
            retries: 0,
            sent_end: None,
        });
        if !self.arm_listener(index) {
            self.listeners[index] = None;
            return fail(Status::Exhausted);
        }
        Outcome::Complete(Completion {
            capability: id,
            capability_kind: wire::CAPABILITY_TCP_LISTENER,
            ..Completion::status(Status::Success)
        })
    }

    pub fn accept(&mut self, holder: [u8; 32], listener_id: u64, now: Instant) -> Outcome {
        self.last_now = now;
        let fail = |status| Outcome::Complete(Completion::status(status));
        let Some(index) = self.listeners.iter().position(|listener| {
            listener.is_some_and(|listener| {
                listener.id == listener_id && listener.holder == holder && listener.active
            })
        }) else {
            return fail(Status::Denied);
        };
        self.refresh_listeners();
        let listener = self.listeners[index].expect("active listener");
        let Some(slot) = listener.socket else {
            return fail(Status::WouldBlock);
        };
        let socket = self.sockets.get::<Socket>(self.handles[slot]);
        if !matches!(socket.state(), State::Established | State::CloseWait) {
            return fail(Status::WouldBlock);
        }
        let peer_ok = self
            .connections
            .iter()
            .enumerate()
            .any(|(peer_index, connection)| {
                connection.is_some_and(|connection| connection.holder == listener.peer)
                    && self
                        .sockets
                        .get::<Socket>(self.handles[peer_index])
                        .local_endpoint()
                        == socket.remote_endpoint()
                    && self
                        .sockets
                        .get::<Socket>(self.handles[peer_index])
                        .remote_endpoint()
                        == socket.local_endpoint()
            });
        if !peer_ok {
            return fail(Status::Denied);
        }
        let Some(id) = self.next_id() else {
            return fail(Status::Exhausted);
        };
        self.connections[slot] = Some(Connection {
            id,
            holder,
            destination: usize::MAX,
            rights: listener.rights,
            phase: Phase::Connected,
            deadline: now + OPERATION_TIMEOUT,
            retry_limit: listener.retry_limit,
            retries: listener.retries,
            sent_end: listener.sent_end,
            terminal: None,
            peer_fin_seen: false,
            listener: Some(listener.id),
        });
        self.listeners[index]
            .as_mut()
            .expect("active listener")
            .socket = None;
        self.arm_listener(index);
        Outcome::Complete(Completion {
            capability: id,
            capability_kind: wire::CAPABILITY_TCP_CONNECTION,
            ..Completion::status(Status::Success)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn handle_local(
        &mut self,
        destinations: &NetworkDestinations<'_>,
        applications: &NetworkApplications<'_>,
        holder: [u8; 32],
        request: WireNetworkRequest,
        payload: &mut [u8],
        now: Instant,
        iface: &mut Interface,
    ) -> Outcome {
        let fail = |status| Outcome::Complete(Completion::status(status));
        self.last_now = now;
        if !slime_proto::valid_network_request(&request) {
            return fail(Status::Malformed);
        }
        let Some(policy) = applications
            .by_holder(&holder)
            .filter(|policy| policy.backend == Backend::Loopback)
        else {
            return fail(Status::Denied);
        };
        if request.op == wire::OP_LISTEN {
            return self.listen(holder, &policy, request, now);
        }
        if request.op == wire::OP_ACCEPT {
            return self.accept(holder, request.capability, now);
        }
        if request.op == wire::OP_CONNECT {
            if policy.role != Role::Client
                || request.transport != wire::TRANSPORT_TCP
                || request.address_kind != wire::ADDRESS_IPV4
            {
                return fail(Status::Denied);
            }
            let Some(listener_policy) = (0..applications.application_count())
                .filter_map(|index| applications.application(index))
                .find(|listener| {
                    listener.role == Role::Listener
                        && listener.local_ipv4 == request.endpoint[..4]
                        && listener.local_port == request.port
                        && listener.allowed_peer_identity == holder
                })
            else {
                return fail(Status::Denied);
            };
            if !destinations.authorizes(
                &holder,
                Transport::Tcp,
                Address::Ipv4(listener_policy.local_ipv4),
                request.port,
                boot_contracts::network_destination::Right::Connect,
            ) {
                return fail(Status::Denied);
            }
            self.refresh_listeners();
            let Some(listener) = self.listeners.iter().flatten().find(|listener| {
                listener.active
                    && listener.holder == listener_policy.holder_identity
                    && listener.address == listener_policy.local_ipv4
                    && listener.port == listener_policy.local_port
            }) else {
                return fail(Status::Refused);
            };
            if !listener.socket.is_some_and(|slot| {
                self.sockets.get::<Socket>(self.handles[slot]).state() == State::Listen
            }) || self.connections.iter().enumerate().any(|(slot, entry)| {
                entry.is_some_and(|connection| connection.phase == Phase::Connecting)
                    && self
                        .sockets
                        .get::<Socket>(self.handles[slot])
                        .remote_endpoint()
                        .is_some_and(|endpoint| {
                            endpoint.addr == IpAddress::Ipv4(Ipv4Address::from(listener.address))
                                && endpoint.port == listener.port
                        })
            }) {
                return fail(Status::Exhausted);
            }
        }
        if request.op == wire::OP_CLOSE
            && let Some(index) = self.listeners.iter().position(|listener| {
                listener.is_some_and(|listener| {
                    listener.active
                        && listener.id == request.capability
                        && listener.holder == holder
                })
            })
        {
            let listener = self.listeners[index].as_mut().expect("located listener");
            listener.active = false;
            if let Some(slot) = listener.socket.take() {
                self.abort_socket(slot);
            }
            self.refresh_listeners();
            return fail(Status::Success);
        }
        self.handle(destinations, holder, request, payload, now, iface)
    }

    /// At least one live connection exists, and all of its queued bytes have
    /// been acknowledged. smoltcp retains transmitted bytes until their ACK.
    pub fn transmit_drained(&self) -> bool {
        let mut connected = false;
        for (index, connection) in self.connections.iter().enumerate() {
            let Some(connection) = connection else {
                continue;
            };
            let socket = self.sockets.get::<Socket>(self.handles[index]);
            if connection.phase != Phase::Connected
                || connection.terminal.is_some()
                || !matches!(socket.state(), State::Established | State::CloseWait)
                || socket.send_queue() != 0
            {
                return false;
            }
            connected = true;
        }
        connected
    }

    pub fn allocated(&self) -> usize {
        self.connections.iter().flatten().count()
            + self
                .listeners
                .iter()
                .flatten()
                .filter(|listener| listener.active)
                .count()
    }

    pub fn release_holder(&mut self, holder: [u8; 32]) -> Reclaimed {
        if self.name_connect.is_some_and(|pending| {
            self.connections[pending.index].is_some_and(|connection| connection.holder == holder)
        }) {
            self.cancel_dns();
        }
        let mut reclaimed = Reclaimed::default();
        let mut slots = [false; SOCKETS];
        for (index, entry) in self.connections.iter().enumerate() {
            if entry.is_some_and(|connection| connection.holder == holder) {
                slots[index] = true;
                reclaimed.handles += 1;
            }
        }
        for listener in self
            .listeners
            .iter()
            .flatten()
            .filter(|listener| listener.holder == holder)
        {
            reclaimed.handles += usize::from(listener.active);
            if let Some(slot) = listener.socket {
                slots[slot] = true;
            }
        }
        for (index, owned) in slots.iter().enumerate() {
            if !owned {
                continue;
            }
            let socket = self.sockets.get::<Socket>(self.handles[index]);
            if let (Some(local), Some(remote)) = (socket.local_endpoint(), socket.remote_endpoint())
            {
                for (other, connection) in self.connections.iter_mut().enumerate() {
                    if other != index
                        && !slots[other]
                        && self
                            .sockets
                            .get::<Socket>(self.handles[other])
                            .local_endpoint()
                            == Some(remote)
                        && self
                            .sockets
                            .get::<Socket>(self.handles[other])
                            .remote_endpoint()
                            == Some(local)
                        && let Some(connection) = connection
                    {
                        connection.terminal = Some(Status::Reset);
                    }
                }
            }
            self.abort_socket(index);
            self.connections[index] = None;
            reclaimed.sockets += 1;
        }
        for listener in &mut self.listeners {
            if listener.is_some_and(|listener| listener.holder == holder) {
                *listener = None;
            }
        }
        reclaimed.bytes = reclaimed.sockets * 2 * BUFFER_BYTES;
        reclaimed
    }

    #[allow(clippy::too_many_arguments)]
    pub fn handle(
        &mut self,
        destinations: &NetworkDestinations<'_>,
        holder: [u8; 32],
        request: WireNetworkRequest,
        payload: &mut [u8],
        now: Instant,
        iface: &mut Interface,
    ) -> Outcome {
        self.last_now = now;
        if !slime_proto::valid_network_request(&request) {
            return Outcome::Complete(Completion::status(Status::Malformed));
        }
        if request.op == wire::OP_CONNECT {
            return self.connect(destinations, holder, request, now, iface);
        }
        if !matches!(request.op, wire::OP_SEND | wire::OP_RECV | wire::OP_CLOSE) {
            return Outcome::Complete(Completion::status(Status::Unsupported));
        }
        let Some(index) = self.find(holder, request.capability) else {
            return Outcome::Complete(Completion::status(Status::Denied));
        };
        let connection = self.connections[index].expect("located connection");
        let required = match request.op {
            wire::OP_SEND => RIGHT_SEND,
            wire::OP_RECV => RIGHT_RECV,
            _ => 0,
        };
        if connection.rights & required != required {
            return Outcome::Complete(Completion::status(Status::Denied));
        }
        if connection.phase != Phase::Connected {
            return Outcome::Complete(Completion::status(Status::WouldBlock));
        }
        let socket = self.sockets.get_mut::<Socket>(self.handles[index]);
        if request.op == wire::OP_CLOSE {
            let peer_fin_seen = socket.state() == State::CloseWait;
            socket.close();
            self.connections[index] = Some(Connection {
                phase: Phase::Closing,
                peer_fin_seen,
                deadline: now + OPERATION_TIMEOUT,
                ..connection
            });
            return Outcome::Pending {
                capability: connection.id,
            };
        }
        if let Some(status) = connection.terminal {
            return Outcome::Complete(Completion::status(status));
        }
        let mut completion = Completion::status(Status::Success);
        match request.op {
            wire::OP_SEND => {
                if !socket.may_send() {
                    completion.status = Status::Reset;
                } else if !payload.is_empty() && !socket.can_send() {
                    completion.status = Status::WouldBlock;
                } else {
                    match socket.send_slice(payload) {
                        Ok(count) => completion.transferred = count,
                        Err(_) => completion.status = Status::Reset,
                    }
                }
            }
            wire::OP_RECV => {
                if socket.can_recv() {
                    match socket.recv_slice(payload) {
                        Ok(count) => {
                            completion.transferred = count;
                            completion.eof = !socket.may_recv() && socket.state() != State::Closed;
                        }
                        Err(_) => completion.status = Status::Reset,
                    }
                } else if socket.state() == State::Closed {
                    completion.status = Status::Reset;
                } else if !socket.may_recv() {
                    completion.eof = true;
                } else if !payload.is_empty() {
                    completion.status = Status::WouldBlock;
                }
            }
            _ => unreachable!(),
        }
        Outcome::Complete(completion)
    }

    // reconnect_limit bounds automatic recovery only; each call is an explicit
    // holder request and this engine never schedules an automatic reconnect.
    fn connect(
        &mut self,
        destinations: &NetworkDestinations<'_>,
        holder: [u8; 32],
        request: WireNetworkRequest,
        now: Instant,
        iface: &mut Interface,
    ) -> Outcome {
        let fail = |status| Outcome::Complete(Completion::status(status));
        if request.transport != wire::TRANSPORT_TCP
            || !matches!(request.address_kind, wire::ADDRESS_IPV4 | wire::ADDRESS_DNS)
        {
            return fail(Status::Unsupported);
        }
        let named = request.address_kind == wire::ADDRESS_DNS;
        let address: [u8; 4] = request.endpoint[..4].try_into().expect("IPv4 prefix");
        let requested = if named {
            Address::Dns(&request.endpoint[..request.name_len as usize])
        } else {
            Address::Ipv4(address)
        };
        let Some((destination_index, destination)) = (0..destinations.destination_count())
            .find_map(|index| {
                let destination = destinations.destination(index)?;
                (destination.holder_identity == holder
                    && destination.transport == Transport::Tcp
                    && destination.address == requested
                    && destination.port == request.port
                    && destination.rights & RIGHT_CONNECT != 0)
                    .then_some((index, destination))
            })
        else {
            return fail(Status::Denied);
        };
        if named && (destination.dns_record_limit == 0 || self.resolver.is_none()) {
            return fail(Status::Unsupported);
        }
        if named && self.name_connect.is_some() {
            return fail(Status::Exhausted);
        }
        let charged = self
            .connections
            .iter()
            .flatten()
            .filter(|connection| connection.destination == destination_index)
            .count() as u32;
        // Every admitted connection reserves both entire buffers, one work slot,
        // and one timer until close or a failed connect returns its resources.
        if charged >= destination.socket_limit
            || charged >= destination.queue_depth
            || charged >= destination.timer_budget
            || (charged + 1) * (2 * BUFFER_BYTES as u32) > destination.byte_budget
        {
            return fail(Status::Exhausted);
        }
        let Some(index) = self.free_socket() else {
            return fail(Status::Exhausted);
        };
        let Some(serial) = self.serial.checked_add(1) else {
            return fail(Status::Exhausted);
        };
        let Some(port) = (if named {
            Some(0)
        } else {
            self.ephemeral_port()
        }) else {
            return fail(Status::Exhausted);
        };
        let socket = self.sockets.get_mut::<Socket>(self.handles[index]);
        if !named
            && socket
                .connect(
                    iface.context(),
                    (IpAddress::Ipv4(Ipv4Address::from(address)), request.port),
                    port,
                )
                .is_err()
        {
            return fail(Status::Refused);
        }
        socket.set_timeout(Some(OPERATION_TIMEOUT));
        socket.set_ack_delay(None);
        if !named {
            self.next_port = port.checked_add(1).unwrap_or(FIRST_EPHEMERAL_PORT);
        }
        self.serial = serial;
        let id = self.namespace | u64::from(serial);
        self.connections[index] = Some(Connection {
            id,
            holder,
            destination: destination_index,
            rights: destination.rights,
            phase: if named {
                Phase::Resolving
            } else {
                Phase::Connecting
            },
            deadline: now + OPERATION_TIMEOUT,
            retry_limit: destination.retry_limit,
            retries: 0,
            sent_end: None,
            terminal: None,
            peer_fin_seen: false,
            listener: None,
        });
        if named {
            if let Err(status) = self.resolver.as_mut().expect("enabled resolver").start(
                &mut self.sockets,
                &request.endpoint[..request.name_len as usize],
                now,
            ) {
                self.connections[index] = None;
                return fail(status);
            }
            self.name_connect = Some(NameConnect {
                index,
                port: request.port,
                addresses: [[0; 4]; dns::MAX_ADDRESSES],
                count: 0,
                next: 0,
                deadline: now + Duration::from_secs(15),
                expiry: now,
            });
        }
        Outcome::Pending { capability: id }
    }

    pub fn poll_pending(
        &mut self,
        holder: [u8; 32],
        capability: u64,
        now: Instant,
    ) -> Option<Completion> {
        self.last_now = now;
        let Some(index) = self.find(holder, capability) else {
            return Some(Completion::status(Status::Denied));
        };
        let connection = self.connections[index].expect("located connection");
        if self
            .name_connect
            .is_some_and(|pending| pending.index == index)
        {
            return None;
        }
        let socket = self.sockets.get_mut::<Socket>(self.handles[index]);
        let status = match connection.phase {
            Phase::Resolving | Phase::Connecting | Phase::Closing
                if connection.terminal.is_some() =>
            {
                connection.terminal.expect("terminal status")
            }
            Phase::Connecting
                if matches!(socket.state(), State::Established | State::CloseWait) =>
            {
                self.connections[index] = Some(Connection {
                    phase: Phase::Connected,
                    ..connection
                });
                return Some(Completion {
                    capability,
                    capability_kind: wire::CAPABILITY_TCP_CONNECTION,
                    ..Completion::status(Status::Success)
                });
            }
            Phase::Connecting if now >= connection.deadline => Status::Timeout,
            Phase::Connecting if socket.state() == State::Closed => Status::Refused,
            Phase::Closing if socket.state() == State::TimeWait => Status::Success,
            Phase::Closing if socket.state() == State::Closed => {
                if connection.peer_fin_seen {
                    Status::Success
                } else {
                    Status::Reset
                }
            }
            Phase::Closing if now >= connection.deadline => Status::Timeout,
            _ => return None,
        };
        if status != Status::Success {
            self.abort_socket(index);
        }
        if status == Status::Timeout {
            let reclaimed = self
                .timeout_reclaimed
                .get_or_insert_with(Reclaimed::default);
            reclaimed.handles += 1;
            reclaimed.sockets += 1;
            reclaimed.bytes += 2 * BUFFER_BYTES;
        }
        self.connections[index] = None;
        Some(Completion::status(status))
    }

    fn find(&self, holder: [u8; 32], capability: u64) -> Option<usize> {
        self.connections.iter().position(|entry| {
            entry.is_some_and(|connection| {
                connection.id == capability && connection.holder == holder
            })
        })
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use boot_contracts::network_destination as contract;
    use smoltcp::iface::Config;
    use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
    use smoltcp::wire::{EthernetAddress, HardwareAddress, IpCidr};
    use std::collections::VecDeque;
    use std::vec;
    use std::vec::Vec;

    pub(super) const HOLDER: [u8; 32] = [1; 32];
    pub(super) const PEER: [u8; 4] = [10, 0, 0, 2];

    #[derive(Default)]
    pub(super) struct Link {
        pub(super) rx: VecDeque<Vec<u8>>,
        pub(super) tx: VecDeque<Vec<u8>>,
    }
    pub(super) struct Rx(Vec<u8>);
    pub(super) struct Tx<'a>(&'a mut VecDeque<Vec<u8>>);
    impl RxToken for Rx {
        fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
            f(&self.0)
        }
    }
    impl TxToken for Tx<'_> {
        fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
            let mut bytes = vec![0; len];
            let result = f(&mut bytes);
            self.0.push_back(bytes);
            result
        }
    }
    impl Device for Link {
        type RxToken<'a> = Rx;
        type TxToken<'a> = Tx<'a>;
        fn receive(&mut self, _: Instant) -> Option<(Rx, Tx<'_>)> {
            self.rx
                .pop_front()
                .map(|bytes| (Rx(bytes), Tx(&mut self.tx)))
        }
        fn transmit(&mut self, _: Instant) -> Option<Tx<'_>> {
            Some(Tx(&mut self.tx))
        }
        fn capabilities(&self) -> DeviceCapabilities {
            let mut caps = DeviceCapabilities::default();
            caps.medium = Medium::Ethernet;
            caps.max_transmission_unit = 1514;
            caps
        }
    }
    pub(super) fn interface(link: &mut Link, host: u8) -> Interface {
        let mut iface = Interface::new(
            Config::new(HardwareAddress::Ethernet(EthernetAddress([
                2, 0, 0, 0, 0, host,
            ]))),
            link,
            Instant::ZERO,
        );
        iface.update_ip_addrs(|addresses| {
            addresses
                .push(IpCidr::new(
                    IpAddress::Ipv4(Ipv4Address::new(10, 0, 0, host)),
                    24,
                ))
                .unwrap();
        });
        iface
    }
    pub(super) fn declarations(rights: u16, bytes: u32, timers: u32, sockets: u32) -> Vec<u8> {
        use contract::*;
        let mut object = vec![0; HEADER_BYTES + ENTRY_BYTES];
        object[OFF_HEADER_MAGIC..OFF_HEADER_MAGIC + MAGIC.len()].copy_from_slice(&MAGIC);
        for (offset, value) in [
            (OFF_HEADER_FORMAT_VERSION, FORMAT_VERSION),
            (OFF_HEADER_HEADER_SIZE, HEADER_BYTES as u32),
            (OFF_HEADER_DESTINATION_COUNT, 1),
            (OFF_HEADER_TOTAL_LEN, (HEADER_BYTES + ENTRY_BYTES) as u32),
        ] {
            object[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        let entry = &mut object[HEADER_BYTES..];
        entry[OFF_ENTRY_HOLDER_IDENTITY..OFF_ENTRY_HOLDER_IDENTITY_END].copy_from_slice(&HOLDER);
        entry[OFF_ENTRY_TRANSPORT] = TRANSPORT_TCP;
        entry[OFF_ENTRY_ADDRESS_KIND] = ADDRESS_IPV4;
        entry[OFF_ENTRY_ADDRESS..OFF_ENTRY_ADDRESS + 4].copy_from_slice(&PEER);
        entry[OFF_ENTRY_PORT..OFF_ENTRY_PORT + 2].copy_from_slice(&8080u16.to_le_bytes());
        entry[OFF_ENTRY_RIGHTS..OFF_ENTRY_RIGHTS + 2].copy_from_slice(&rights.to_le_bytes());
        for (offset, value) in [
            (OFF_ENTRY_QUEUE_DEPTH, 4),
            (OFF_ENTRY_BYTE_BUDGET, bytes),
            (OFF_ENTRY_TIMER_BUDGET, timers),
            (OFF_ENTRY_SOCKET_LIMIT, sockets),
            (OFF_ENTRY_RETRY_LIMIT, 2),
        ] {
            entry[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        object
    }
    pub(super) fn request(op: u8, capability: u64) -> WireNetworkRequest {
        let mut request = WireNetworkRequest {
            magic: wire::NETWORK_MAGIC,
            version: wire::FORMAT_VERSION,
            op,
            transport: wire::TRANSPORT_NONE,
            flags: 0,
            port: 0,
            name_len: 0,
            capability,
            address_kind: wire::ADDRESS_NONE,
            reserved: [0; 7],
            endpoint: [0; 24],
        };
        if op == wire::OP_CONNECT {
            request.transport = wire::TRANSPORT_TCP;
            request.port = 8080;
            request.address_kind = wire::ADDRESS_IPV4;
            request.endpoint[..4].copy_from_slice(&PEER);
        }
        request
    }
    pub(super) fn pending(outcome: Outcome) -> u64 {
        match outcome {
            Outcome::Pending { capability } => capability,
            other => panic!("{other:?}"),
        }
    }
    fn complete(outcome: Outcome) -> Completion {
        match outcome {
            Outcome::Complete(value) => value,
            other => panic!("{other:?}"),
        }
    }

    fn local_applications(rights: u16) -> Vec<u8> {
        use boot_contracts::network_application as app;
        let mut bytes = vec![0; app::HEADER_BYTES + 3 * app::ENTRY_BYTES];
        bytes[..app::MAGIC.len()].copy_from_slice(&app::MAGIC);
        for (offset, value) in [
            (app::OFF_HEADER_FORMAT_VERSION, app::FORMAT_VERSION),
            (app::OFF_HEADER_HEADER_SIZE, app::HEADER_BYTES as u32),
            (app::OFF_HEADER_APPLICATION_COUNT, 3),
            (app::OFF_HEADER_TOTAL_LEN, bytes.len() as u32),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        for (index, holder) in [[1; 32], [2; 32], [3; 32]].iter().enumerate() {
            let start = app::HEADER_BYTES + index * app::ENTRY_BYTES;
            let row = &mut bytes[start..start + app::ENTRY_BYTES];
            row[app::OFF_ENTRY_HOLDER_IDENTITY..app::OFF_ENTRY_HOLDER_IDENTITY_END]
                .copy_from_slice(holder);
            row[app::OFF_ENTRY_CONTROL_BINDING..app::OFF_ENTRY_CONTROL_BINDING + 7]
                .copy_from_slice(b"control");
            row[app::OFF_ENTRY_PROVISION_BINDING..app::OFF_ENTRY_PROVISION_BINDING + 9]
                .copy_from_slice(b"provision");
            row[app::OFF_ENTRY_BACKEND] = app::BACKEND_LOOPBACK;
            row[app::OFF_ENTRY_ROLE] = if index == 1 {
                app::ROLE_LISTENER
            } else {
                app::ROLE_CLIENT
            };
            if index == 1 {
                row[app::OFF_ENTRY_ALLOWED_PEER_IDENTITY..app::OFF_ENTRY_ALLOWED_PEER_IDENTITY_END]
                    .copy_from_slice(&HOLDER);
                row[app::OFF_ENTRY_LOCAL_IPV4..app::OFF_ENTRY_LOCAL_IPV4_END]
                    .copy_from_slice(&[127, 0, 0, 1]);
                row[app::OFF_ENTRY_LOCAL_PORT..app::OFF_ENTRY_LOCAL_PORT + 2]
                    .copy_from_slice(&8080u16.to_le_bytes());
                row[app::OFF_ENTRY_RIGHTS..app::OFF_ENTRY_RIGHTS + 2]
                    .copy_from_slice(&rights.to_le_bytes());
                for (offset, value) in [
                    (app::OFF_ENTRY_BACKLOG, 1u32),
                    (app::OFF_ENTRY_ACCEPTED_SOCKET_LIMIT, 1),
                    (app::OFF_ENTRY_BYTE_BUDGET, 8192),
                    (app::OFF_ENTRY_TIMER_BUDGET, 2),
                    (app::OFF_ENTRY_QUEUE_DEPTH, 2),
                    (app::OFF_ENTRY_RETRY_LIMIT, 2),
                ] {
                    row[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
                }
            }
        }
        bytes
    }

    fn local_request(op: u8, capability: u64) -> WireNetworkRequest {
        let mut value = request(
            if op == wire::OP_LISTEN {
                wire::OP_CONNECT
            } else {
                op
            },
            capability,
        );
        value.op = op;
        if matches!(op, wire::OP_LISTEN | wire::OP_CONNECT) {
            value.endpoint[..4].copy_from_slice(&[127, 0, 0, 1]);
        }
        if op == wire::OP_ACCEPT {
            value.transport = wire::TRANSPORT_TCP;
        }
        value
    }

    #[test]
    fn local_listener_preallocation_limits_and_authority_refuse_without_output() {
        use boot_contracts::network_application::{RIGHT_LISTEN, RIGHT_RECV};
        let mut storage = [SocketStorage::EMPTY; SOCKETS];
        let mut rx = [[0; BUFFER_BYTES]; SOCKETS];
        let mut tx = [[0; BUFFER_BYTES]; SOCKETS];
        let mut engine = Engine::new(&mut storage, &mut rx, &mut tx, 44, 1).unwrap();
        let bytes = local_applications(RIGHT_LISTEN | RIGHT_RECV);
        let applications = NetworkApplications::decode(&bytes).unwrap();
        let policy = applications.by_holder(&[2; 32]).unwrap();
        for mutation in 0..7 {
            let mut denied = policy;
            match mutation {
                0 => denied.byte_budget = 4095,
                1 => denied.timer_budget = 0,
                2 => denied.queue_depth = 0,
                3 => denied.accepted_socket_limit = 0,
                4 => denied.rights = RIGHT_RECV,
                5 => denied.backlog = 0,
                _ => denied.backlog = 2,
            }
            let status = complete(engine.listen(
                [2; 32],
                &denied,
                local_request(wire::OP_LISTEN, 0),
                Instant::ZERO,
            ))
            .status;
            assert_eq!(
                status,
                if mutation >= 4 {
                    Status::Denied
                } else {
                    Status::Exhausted
                }
            );
            assert_eq!(engine.allocated(), 0);
            assert!(engine.sockets.iter().all(|(_, socket)| matches!(socket, smoltcp::socket::Socket::Tcp(socket) if socket.state()==State::Closed)));
        }
        assert_eq!(
            complete(engine.listen(
                HOLDER,
                &policy,
                local_request(wire::OP_LISTEN, 0),
                Instant::ZERO
            ))
            .status,
            Status::Denied
        );
        let id = complete(engine.listen(
            [2; 32],
            &policy,
            local_request(wire::OP_LISTEN, 0),
            Instant::ZERO,
        ))
        .capability;
        let listener = engine.listeners.iter_mut().flatten().next().unwrap();
        listener.port = FIRST_EPHEMERAL_PORT;
        assert_eq!(engine.ephemeral_port(), Some(FIRST_EPHEMERAL_PORT + 1));
        assert_eq!(engine.release_holder([2; 32]).handles, 1);
        assert_eq!(
            complete(engine.accept([2; 32], id, Instant::ZERO)).status,
            Status::Denied
        );
    }

    #[test]
    fn local_listener_real_connect_accept_rights_close_and_rearm() {
        local_listener_flow(false);
    }

    #[test]
    fn holder_loss_reflects_one_authorized_reset_and_reclaims_exact_charges() {
        local_listener_flow(true);
    }

    fn local_listener_flow(reset_case: bool) {
        use crate::loopback::Loopback;
        use boot_contracts::network_application::RIGHT_LISTEN;
        let mut storage = [SocketStorage::EMPTY; SOCKETS];
        let mut rx = [[0; BUFFER_BYTES]; SOCKETS];
        let mut tx = [[0; BUFFER_BYTES]; SOCKETS];
        let mut engine = Engine::new(&mut storage, &mut rx, &mut tx, 44, 1).unwrap();
        let mut device = Loopback::new();
        let mut iface = Interface::new(
            Config::new(HardwareAddress::Ethernet(EthernetAddress([
                2, 0, 0, 0, 0, 1,
            ]))),
            &mut device,
            Instant::ZERO,
        );
        iface.update_ip_addrs(|addresses| {
            addresses
                .push(IpCidr::new(IpAddress::v4(127, 0, 0, 1), 8))
                .unwrap();
        });
        let mut destinations_bytes =
            declarations(RIGHT_CONNECT | RIGHT_SEND | RIGHT_RECV, 8192, 2, 2);
        let offset = contract::HEADER_BYTES + contract::OFF_ENTRY_ADDRESS;
        destinations_bytes[offset..offset + 4].copy_from_slice(&[127, 0, 0, 1]);
        let destinations = NetworkDestinations::decode(&destinations_bytes).unwrap();
        let application_bytes = local_applications(RIGHT_LISTEN | RIGHT_RECV);
        let applications = NetworkApplications::decode(&application_bytes).unwrap();
        let listener_holder = [2; 32];
        let result = complete(engine.handle_local(
            &destinations,
            &applications,
            HOLDER,
            local_request(wire::OP_CONNECT, 0),
            &mut [],
            Instant::ZERO,
            &mut iface,
        ));
        assert_eq!(result.status, Status::Refused);
        assert_eq!(engine.allocated(), 0);
        let mut wrong = local_request(wire::OP_LISTEN, 0);
        wrong.port += 1;
        assert_eq!(
            complete(engine.handle_local(
                &destinations,
                &applications,
                listener_holder,
                wrong,
                &mut [],
                Instant::ZERO,
                &mut iface
            ))
            .status,
            Status::Denied
        );
        let listener = complete(engine.handle_local(
            &destinations,
            &applications,
            listener_holder,
            local_request(wire::OP_LISTEN, 0),
            &mut [],
            Instant::ZERO,
            &mut iface,
        ));
        assert_eq!(listener.status, Status::Success);
        assert_eq!(listener.capability_kind, wire::CAPABILITY_TCP_LISTENER);
        assert_eq!(
            complete(engine.accept(HOLDER, listener.capability, Instant::ZERO)).status,
            Status::Denied
        );
        assert_eq!(
            complete(engine.accept(listener_holder, listener.capability, Instant::ZERO)).status,
            Status::WouldBlock
        );
        assert_eq!(
            complete(engine.handle_local(
                &destinations,
                &applications,
                [3; 32],
                local_request(wire::OP_CONNECT, 0),
                &mut [],
                Instant::ZERO,
                &mut iface
            ))
            .status,
            Status::Denied
        );
        assert_eq!(
            complete(engine.handle_local(
                &destinations,
                &applications,
                listener_holder,
                local_request(wire::OP_LISTEN, 0),
                &mut [],
                Instant::ZERO,
                &mut iface
            ))
            .status,
            Status::Exhausted
        );
        assert_eq!(device.egress_count(), 0);
        let client = pending(engine.handle_local(
            &destinations,
            &applications,
            HOLDER,
            local_request(wire::OP_CONNECT, 0),
            &mut [],
            Instant::ZERO,
            &mut iface,
        ));
        assert_eq!(
            complete(engine.handle_local(
                &destinations,
                &applications,
                HOLDER,
                local_request(wire::OP_CONNECT, 0),
                &mut [],
                Instant::ZERO,
                &mut iface
            ))
            .status,
            Status::Exhausted
        );
        for time in 0..20 {
            iface.poll(Instant::from_millis(time), &mut device, engine.sockets());
            device.flush(|frame| engine.permit_egress(frame));
        }
        assert_eq!(
            engine
                .poll_pending(HOLDER, client, Instant::from_millis(20))
                .unwrap()
                .status,
            Status::Success
        );
        let accepted = complete(engine.accept(
            listener_holder,
            listener.capability,
            Instant::from_millis(20),
        ));
        assert_eq!(accepted.status, Status::Success);
        assert_eq!(accepted.capability_kind, wire::CAPABILITY_TCP_CONNECTION);
        assert_eq!(engine.allocated(), 3);
        assert_eq!(
            complete(engine.handle_local(
                &destinations,
                &applications,
                listener_holder,
                local_request(wire::OP_SEND, accepted.capability),
                &mut [1],
                Instant::from_millis(20),
                &mut iface
            ))
            .status,
            Status::Denied
        );
        assert_eq!(
            complete(engine.handle_local(
                &destinations,
                &applications,
                HOLDER,
                local_request(wire::OP_RECV, accepted.capability),
                &mut [0; 32],
                Instant::from_millis(20),
                &mut iface
            ))
            .status,
            Status::Denied
        );
        if reset_case {
            let mut received = [0; 32];
            assert_eq!(
                complete(engine.handle_local(
                    &destinations,
                    &applications,
                    listener_holder,
                    local_request(wire::OP_RECV, accepted.capability),
                    &mut received,
                    Instant::from_millis(20),
                    &mut iface
                ))
                .status,
                Status::WouldBlock
            );
            let released = engine.release_holder(HOLDER);
            assert_eq!(
                released,
                Reclaimed {
                    handles: 1,
                    sockets: 1,
                    bytes: 4096
                }
            );
            assert_eq!(
                complete(engine.handle_local(
                    &destinations,
                    &applications,
                    HOLDER,
                    local_request(wire::OP_SEND, client),
                    &mut [1],
                    Instant::from_millis(20),
                    &mut iface
                ))
                .status,
                Status::Denied
            );
            assert_eq!(
                complete(engine.handle_local(
                    &destinations,
                    &applications,
                    listener_holder,
                    local_request(wire::OP_RECV, accepted.capability),
                    &mut received,
                    Instant::from_millis(20),
                    &mut iface
                ))
                .status,
                Status::Reset
            );
            let mut resets = 0;
            for time in 20..40 {
                engine.tick(Instant::from_millis(time));
                iface.poll(Instant::from_millis(time), &mut device, engine.sockets());
                device.flush(|frame| {
                    let ethernet = EthernetFrame::new_checked(frame).unwrap();
                    let reset = if ethernet.ethertype() == EthernetProtocol::Ipv4 {
                        let ip = Ipv4Packet::new_checked(ethernet.payload()).unwrap();
                        ip.next_header() == IpProtocol::Tcp
                            && TcpPacket::new_checked(ip.payload()).unwrap().rst()
                    } else {
                        false
                    };
                    if reset {
                        let mut wrong = frame.to_vec();
                        let mut ethernet = EthernetFrame::new_unchecked(&mut wrong[..]);
                        let mut ip = Ipv4Packet::new_unchecked(ethernet.payload_mut());
                        ip.set_dst_addr(Ipv4Address::new(127, 0, 0, 2));
                        assert!(!engine.permit_egress(&wrong));
                    }
                    let allowed = engine.permit_egress(frame);
                    if reset {
                        assert!(allowed);
                        assert!(!engine.permit_egress(frame));
                        resets += 1;
                    }
                    allowed
                });
            }
            assert_eq!(resets, 1);
            assert_eq!(device.reset_count(), 1);
            let accepted_slot = engine.find(listener_holder, accepted.capability).unwrap();
            assert_eq!(
                engine
                    .sockets
                    .get::<Socket>(engine.handles[accepted_slot])
                    .state(),
                State::Closed
            );
            pending(engine.handle_local(
                &destinations,
                &applications,
                listener_holder,
                local_request(wire::OP_CLOSE, accepted.capability),
                &mut [],
                Instant::from_millis(40),
                &mut iface,
            ));
            assert_eq!(
                engine
                    .poll_pending(
                        listener_holder,
                        accepted.capability,
                        Instant::from_millis(40)
                    )
                    .unwrap()
                    .status,
                Status::Reset
            );
            engine.tick(Instant::from_millis(41));
            let fresh = pending(engine.handle_local(
                &destinations,
                &applications,
                HOLDER,
                local_request(wire::OP_CONNECT, 0),
                &mut [],
                Instant::from_millis(41),
                &mut iface,
            ));
            assert_ne!(fresh, client);
            for time in 41..61 {
                iface.poll(Instant::from_millis(time), &mut device, engine.sockets());
                device.flush(|frame| engine.permit_egress(frame));
            }
            assert_eq!(
                engine
                    .poll_pending(HOLDER, fresh, Instant::from_millis(61))
                    .unwrap()
                    .status,
                Status::Success
            );
            let fresh_peer = complete(engine.accept(
                listener_holder,
                listener.capability,
                Instant::from_millis(61),
            ));
            assert_eq!(fresh_peer.status, Status::Success);
            let mut payload = *b"fresh";
            assert_eq!(
                complete(engine.handle_local(
                    &destinations,
                    &applications,
                    HOLDER,
                    local_request(wire::OP_SEND, fresh),
                    &mut payload,
                    Instant::from_millis(61),
                    &mut iface
                ))
                .transferred,
                5
            );
            for time in 61..81 {
                iface.poll(Instant::from_millis(time), &mut device, engine.sockets());
                device.flush(|frame| engine.permit_egress(frame));
            }
            let got = complete(engine.handle_local(
                &destinations,
                &applications,
                listener_holder,
                local_request(wire::OP_RECV, fresh_peer.capability),
                &mut received,
                Instant::from_millis(81),
                &mut iface,
            ));
            assert_eq!(&received[..got.transferred], b"fresh");
            assert_eq!(
                engine.reset_all(Instant::from_millis(81)),
                Reclaimed {
                    handles: 3,
                    sockets: 2,
                    bytes: 8192
                }
            );
            assert_eq!(engine.allocated(), 0);
            assert!(engine.resets.iter().flatten().count() >= 1);
            engine.tick(Instant::from_millis(81) + OPERATION_TIMEOUT);
            assert!(engine.resets.iter().all(Option::is_none));
            return;
        }
        let mut payload = *b"opaque local stream";
        assert_eq!(
            complete(engine.handle_local(
                &destinations,
                &applications,
                HOLDER,
                local_request(wire::OP_SEND, client),
                &mut payload,
                Instant::from_millis(20),
                &mut iface
            ))
            .transferred,
            payload.len()
        );
        for time in 20..40 {
            iface.poll(Instant::from_millis(time), &mut device, engine.sockets());
            device.flush(|frame| engine.permit_egress(frame));
        }
        let mut received = [0; 32];
        let got = complete(engine.handle_local(
            &destinations,
            &applications,
            listener_holder,
            local_request(wire::OP_RECV, accepted.capability),
            &mut received,
            Instant::from_millis(40),
            &mut iface,
        ));
        assert_eq!(&received[..got.transferred], &payload);
        assert_eq!(
            complete(engine.handle_local(
                &destinations,
                &applications,
                listener_holder,
                local_request(wire::OP_CLOSE, listener.capability),
                &mut [],
                Instant::from_millis(40),
                &mut iface
            ))
            .status,
            Status::Success
        );
        assert_eq!(
            complete(engine.accept(
                listener_holder,
                listener.capability,
                Instant::from_millis(40)
            ))
            .status,
            Status::Denied
        );
        assert_eq!(
            complete(engine.handle_local(
                &destinations,
                &applications,
                listener_holder,
                local_request(wire::OP_LISTEN, 0),
                &mut [],
                Instant::from_millis(40),
                &mut iface
            ))
            .status,
            Status::Exhausted
        );
        pending(engine.handle_local(
            &destinations,
            &applications,
            HOLDER,
            local_request(wire::OP_CLOSE, client),
            &mut [],
            Instant::from_millis(40),
            &mut iface,
        ));
        for time in 40..60 {
            iface.poll(Instant::from_millis(time), &mut device, engine.sockets());
            device.flush(|frame| engine.permit_egress(frame));
        }
        pending(engine.handle_local(
            &destinations,
            &applications,
            listener_holder,
            local_request(wire::OP_CLOSE, accepted.capability),
            &mut [],
            Instant::from_millis(60),
            &mut iface,
        ));
        for time in 60..80 {
            iface.poll(Instant::from_millis(time), &mut device, engine.sockets());
            device.flush(|frame| engine.permit_egress(frame));
        }
        assert_eq!(
            engine
                .poll_pending(HOLDER, client, Instant::from_millis(80))
                .unwrap()
                .status,
            Status::Success
        );
        assert_eq!(
            engine
                .poll_pending(
                    listener_holder,
                    accepted.capability,
                    Instant::from_millis(80)
                )
                .unwrap()
                .status,
            Status::Success
        );
        let again = complete(engine.handle_local(
            &destinations,
            &applications,
            listener_holder,
            local_request(wire::OP_LISTEN, 0),
            &mut [],
            Instant::from_millis(80),
            &mut iface,
        ));
        assert_eq!(again.status, Status::Success);
        assert_ne!(again.capability, listener.capability);
        assert_eq!(engine.release_holder(listener_holder).handles, 1);
        assert_eq!(engine.allocated(), 0);
        assert_eq!(device.rejected_count(), 0);
    }

    #[test]
    fn ephemeral_ports_wrap_skip_live_and_reset_tuples_and_preserve_failed_cursor() {
        let mut storage = [SocketStorage::EMPTY; SOCKETS];
        let mut rx = [[0; BUFFER_BYTES]; SOCKETS];
        let mut tx = [[0; BUFFER_BYTES]; SOCKETS];
        let mut engine = Engine::new(&mut storage, &mut rx, &mut tx, 1, 0).unwrap();
        let mut link = Link::default();
        let mut iface = interface(&mut link, 1);
        let mut bytes = declarations(RIGHT_CONNECT, 16384, 4, 4);
        let mut other = bytes[contract::HEADER_BYTES..].to_vec();
        other[contract::OFF_ENTRY_HOLDER_IDENTITY..contract::OFF_ENTRY_HOLDER_IDENTITY_END].fill(2);
        bytes.extend_from_slice(&other);
        let length = bytes.len() as u32;
        for (offset, value) in [
            (contract::OFF_HEADER_DESTINATION_COUNT, 2u32),
            (contract::OFF_HEADER_TOTAL_LEN, length),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        let destinations = NetworkDestinations::decode(&bytes).unwrap();
        engine.next_port = u16::MAX;
        let first = pending(engine.handle(
            &destinations,
            HOLDER,
            request(wire::OP_CONNECT, 0),
            &mut [],
            Instant::ZERO,
            &mut iface,
        ));
        assert_eq!(engine.next_port, FIRST_EPHEMERAL_PORT);
        engine.next_port = u16::MAX;
        let second = pending(engine.handle(
            &destinations,
            [2; 32],
            request(wire::OP_CONNECT, 0),
            &mut [],
            Instant::ZERO,
            &mut iface,
        ));
        let second_slot = engine.find([2; 32], second).unwrap();
        assert_eq!(
            engine
                .sockets
                .get::<Socket>(engine.handles[second_slot])
                .local_endpoint()
                .unwrap()
                .port,
            FIRST_EPHEMERAL_PORT
        );
        assert_ne!(first, second);
        engine.release_holder(HOLDER);
        engine.release_holder([2; 32]);
        engine.next_port = u16::MAX;
        assert_eq!(engine.ephemeral_port(), Some(FIRST_EPHEMERAL_PORT + 1));
        // A retained reset alone quarantines either end even without a socket tuple.
        let mut storage = [SocketStorage::EMPTY; SOCKETS];
        let mut rx = [[0; BUFFER_BYTES]; SOCKETS];
        let mut tx = [[0; BUFFER_BYTES]; SOCKETS];
        let mut engine = Engine::new(&mut storage, &mut rx, &mut tx, 2, 0).unwrap();
        engine.resets[0] = Some(Reset {
            local: IpEndpoint::new(IpAddress::v4(10, 0, 0, 1), FIRST_EPHEMERAL_PORT),
            remote: IpEndpoint::new(IpAddress::v4(10, 0, 0, 2), FIRST_EPHEMERAL_PORT + 1),
            deadline: Instant::ZERO + OPERATION_TIMEOUT,
        });
        assert_eq!(engine.ephemeral_port(), Some(FIRST_EPHEMERAL_PORT + 2));
        engine.tick(Instant::ZERO + OPERATION_TIMEOUT);
        assert_eq!(engine.ephemeral_port(), Some(FIRST_EPHEMERAL_PORT));
        iface.update_ip_addrs(|addresses| addresses.clear());
        for _ in 0..=u16::MAX - FIRST_EPHEMERAL_PORT {
            assert_eq!(
                complete(engine.handle(
                    &destinations,
                    HOLDER,
                    request(wire::OP_CONNECT, 0),
                    &mut [],
                    Instant::ZERO,
                    &mut iface
                ))
                .status,
                Status::Refused
            );
        }
        assert_eq!(engine.next_port, FIRST_EPHEMERAL_PORT);
        assert_eq!(engine.serial, 0);
        assert_eq!(engine.allocated(), 0);
        iface.update_ip_addrs(|addresses| {
            addresses
                .push(IpCidr::new(IpAddress::v4(10, 0, 0, 1), 24))
                .unwrap()
        });
        pending(engine.handle(
            &destinations,
            [2; 32],
            request(wire::OP_CONNECT, 0),
            &mut [],
            Instant::ZERO,
            &mut iface,
        ));
    }

    #[test]
    fn local_listener_accepts_two_live_connections_with_independent_payloads() {
        use crate::loopback::Loopback;
        use boot_contracts::network_application as app;
        let mut storage = [SocketStorage::EMPTY; SOCKETS];
        let mut rx = [[0; BUFFER_BYTES]; SOCKETS];
        let mut tx = [[0; BUFFER_BYTES]; SOCKETS];
        let mut engine = Engine::new(&mut storage, &mut rx, &mut tx, 1, 1).unwrap();
        let mut device = Loopback::new();
        let mut iface = Interface::new(
            Config::new(HardwareAddress::Ethernet(EthernetAddress([
                2, 0, 0, 0, 0, 1,
            ]))),
            &mut device,
            Instant::ZERO,
        );
        iface.update_ip_addrs(|addresses| {
            addresses
                .push(IpCidr::new(IpAddress::v4(127, 0, 0, 1), 8))
                .unwrap()
        });
        let mut bytes = declarations(RIGHT_CONNECT | RIGHT_SEND | RIGHT_RECV, 8192, 2, 2);
        let offset = contract::HEADER_BYTES + contract::OFF_ENTRY_ADDRESS;
        bytes[offset..offset + 4].copy_from_slice(&[127, 0, 0, 1]);
        let destinations = NetworkDestinations::decode(&bytes).unwrap();
        let mut bytes = local_applications(app::RIGHT_LISTEN | app::RIGHT_SEND | app::RIGHT_RECV);
        let offset = app::HEADER_BYTES + app::ENTRY_BYTES + app::OFF_ENTRY_ACCEPTED_SOCKET_LIMIT;
        bytes[offset..offset + 4].copy_from_slice(&2u32.to_le_bytes());
        let applications = NetworkApplications::decode(&bytes).unwrap();
        let listener = complete(engine.handle_local(
            &destinations,
            &applications,
            [2; 32],
            local_request(wire::OP_LISTEN, 0),
            &mut [],
            Instant::ZERO,
            &mut iface,
        ))
        .capability;
        let mut pairs = [(0, 0); 2];
        for (ordinal, pair) in pairs.iter_mut().enumerate() {
            let now = Instant::from_millis(ordinal as i64 * 20);
            pair.0 = pending(engine.handle_local(
                &destinations,
                &applications,
                HOLDER,
                local_request(wire::OP_CONNECT, 0),
                &mut [],
                now,
                &mut iface,
            ));
            assert_eq!(
                complete(engine.handle_local(
                    &destinations,
                    &applications,
                    HOLDER,
                    local_request(wire::OP_CONNECT, 0),
                    &mut [],
                    now,
                    &mut iface
                ))
                .status,
                Status::Exhausted
            );
            for _ in 0..20 {
                iface.poll(now, &mut device, engine.sockets());
                device.flush(|frame| engine.permit_egress(frame));
            }
            assert_eq!(
                engine.poll_pending(HOLDER, pair.0, now).unwrap().status,
                Status::Success
            );
            let accepted = complete(engine.accept([2; 32], listener, now));
            assert_eq!(accepted.status, Status::Success);
            pair.1 = accepted.capability;
        }
        assert_eq!(engine.children(listener), 2);
        assert_eq!(engine.allocated(), 5);
        assert_eq!(
            complete(engine.handle_local(
                &destinations,
                &applications,
                HOLDER,
                local_request(wire::OP_CONNECT, 0),
                &mut [],
                Instant::from_millis(40),
                &mut iface
            ))
            .status,
            Status::Exhausted
        );
        for (ordinal, (client, _)) in pairs.iter().enumerate() {
            assert_eq!(
                complete(engine.handle_local(
                    &destinations,
                    &applications,
                    HOLDER,
                    local_request(wire::OP_SEND, *client),
                    &mut [ordinal as u8 + 1],
                    Instant::from_millis(40),
                    &mut iface
                ))
                .transferred,
                1
            );
        }
        for _ in 0..20 {
            iface.poll(Instant::from_millis(40), &mut device, engine.sockets());
            device.flush(|frame| engine.permit_egress(frame));
        }
        for (ordinal, (_, accepted)) in pairs.iter().enumerate() {
            let mut received = [0];
            assert_eq!(
                complete(engine.handle_local(
                    &destinations,
                    &applications,
                    [2; 32],
                    local_request(wire::OP_RECV, *accepted),
                    &mut received,
                    Instant::from_millis(40),
                    &mut iface
                ))
                .transferred,
                1
            );
            assert_eq!(received, [ordinal as u8 + 1]);
        }
    }

    #[test]
    fn incarnation_backend_and_serial_fields_are_checked_and_disjoint() {
        let mut identities = Vec::new();
        for epoch in [1, 2, MAX_INCARNATION] {
            for backend in [0, 1] {
                let mut storage = [SocketStorage::EMPTY; SOCKETS];
                let mut rx = [[0; BUFFER_BYTES]; SOCKETS];
                let mut tx = [[0; BUFFER_BYTES]; SOCKETS];
                let mut engine =
                    Engine::new(&mut storage, &mut rx, &mut tx, epoch, backend).unwrap();
                let first = engine.next_id().unwrap();
                assert_eq!(
                    first,
                    (1u64 << 63) | (epoch << 33) | (u64::from(backend) << 32) | 1
                );
                assert!(identities.iter().all(|id| *id != first));
                identities.push(first);
                engine.serial = u32::MAX - 1;
                let last = engine.next_id().unwrap();
                assert_eq!(last & u64::from(u32::MAX), u64::from(u32::MAX));
                assert!(identities.iter().all(|id| *id != last));
                identities.push(last);
                assert_eq!(engine.next_id(), None);
                assert_eq!(engine.next_id(), None);
            }
        }
        for (epoch, backend) in [
            (0, 0),
            (MAX_INCARNATION + 1, 0),
            (u64::MAX, 0),
            (1, 2),
            (1, u8::MAX),
        ] {
            let mut storage = [SocketStorage::EMPTY; SOCKETS];
            let mut rx = [[0; BUFFER_BYTES]; SOCKETS];
            let mut tx = [[0; BUFFER_BYTES]; SOCKETS];
            assert!(matches!(
                Engine::new(&mut storage, &mut rx, &mut tx, epoch, backend),
                Err(Status::Exhausted)
            ));
        }
    }

    #[test]
    fn authority_budget_timeout_and_stale_handles() {
        let mut storage = [SocketStorage::EMPTY; SOCKETS];
        let mut rx = [[0; BUFFER_BYTES]; SOCKETS];
        let mut tx = [[0; BUFFER_BYTES]; SOCKETS];
        let mut engine = Engine::new(&mut storage, &mut rx, &mut tx, 7, 0).unwrap();
        let mut link = Link::default();
        let mut iface = interface(&mut link, 1);
        let bytes = declarations(RIGHT_CONNECT | RIGHT_SEND | RIGHT_RECV, 4096, 1, 4);
        let destinations = NetworkDestinations::decode(&bytes).unwrap();
        for holder in [[2; 32], [0; 32]] {
            assert_eq!(
                complete(engine.handle(
                    &destinations,
                    holder,
                    request(wire::OP_CONNECT, 0),
                    &mut [],
                    Instant::ZERO,
                    &mut iface
                ))
                .status,
                Status::Denied
            );
        }
        let mut wrong = request(wire::OP_CONNECT, 0);
        wrong.port += 1;
        assert_eq!(
            complete(engine.handle(
                &destinations,
                HOLDER,
                wrong,
                &mut [],
                Instant::ZERO,
                &mut iface
            ))
            .status,
            Status::Denied
        );
        wrong = request(wire::OP_CONNECT, 0);
        wrong.endpoint[3] += 1;
        assert_eq!(
            complete(engine.handle(
                &destinations,
                HOLDER,
                wrong,
                &mut [],
                Instant::ZERO,
                &mut iface
            ))
            .status,
            Status::Denied
        );
        iface.poll(Instant::ZERO, &mut link, engine.sockets());
        assert!(link.tx.is_empty());
        assert_eq!(engine.allocated(), 0);
        let id = pending(engine.handle(
            &destinations,
            HOLDER,
            request(wire::OP_CONNECT, 0),
            &mut [],
            Instant::ZERO,
            &mut iface,
        ));
        assert_ne!(id >> 63, 0);
        assert_eq!(
            complete(engine.handle(
                &destinations,
                HOLDER,
                request(wire::OP_CONNECT, 0),
                &mut [],
                Instant::ZERO,
                &mut iface
            ))
            .status,
            Status::Exhausted
        );
        assert_eq!(
            engine
                .poll_pending([2; 32], id, Instant::ZERO)
                .unwrap()
                .status,
            Status::Denied
        );
        assert_eq!(engine.allocated(), 1);
        assert_eq!(
            engine
                .poll_pending(HOLDER, id, Instant::ZERO + OPERATION_TIMEOUT)
                .unwrap()
                .status,
            Status::Timeout
        );
        assert_eq!(engine.allocated(), 0);
        assert_eq!(
            complete(engine.handle(
                &destinations,
                HOLDER,
                request(wire::OP_SEND, id),
                &mut [1],
                Instant::ZERO,
                &mut iface
            ))
            .status,
            Status::Denied
        );
        let next = pending(engine.handle(
            &destinations,
            HOLDER,
            request(wire::OP_CONNECT, 0),
            &mut [],
            Instant::ZERO,
            &mut iface,
        ));
        assert_ne!(id, next);
        assert_eq!(engine.release_holder([2; 32]).handles, 0);
        assert_eq!(engine.release_holder(HOLDER).handles, 1);
        assert_eq!(engine.allocated(), 0);
    }

    #[test]
    fn independent_resource_bounds_and_missing_rights() {
        let mut storage = [SocketStorage::EMPTY; SOCKETS];
        let mut rx = [[0; BUFFER_BYTES]; SOCKETS];
        let mut tx = [[0; BUFFER_BYTES]; SOCKETS];
        let mut engine = Engine::new(&mut storage, &mut rx, &mut tx, 1, 0).unwrap();
        let mut link = Link::default();
        let mut iface = interface(&mut link, 1);
        for (budget, timers) in [(4095, 4), (16384, 0)] {
            let bytes = declarations(RIGHT_CONNECT, budget, timers, 4);
            let destinations = NetworkDestinations::decode(&bytes).unwrap();
            assert_eq!(
                complete(engine.handle(
                    &destinations,
                    HOLDER,
                    request(wire::OP_CONNECT, 0),
                    &mut [],
                    Instant::ZERO,
                    &mut iface
                ))
                .status,
                Status::Exhausted
            );
            assert_eq!(engine.allocated(), 0);
        }
        for limit in [
            contract::OFF_ENTRY_SOCKET_LIMIT,
            contract::OFF_ENTRY_QUEUE_DEPTH,
            contract::OFF_ENTRY_TIMER_BUDGET,
        ] {
            let mut bytes = declarations(RIGHT_CONNECT, 16384, 4, 4);
            let offset = contract::HEADER_BYTES + limit;
            bytes[offset..offset + 4].copy_from_slice(&1u32.to_le_bytes());
            let destinations = NetworkDestinations::decode(&bytes).unwrap();
            pending(engine.handle(
                &destinations,
                HOLDER,
                request(wire::OP_CONNECT, 0),
                &mut [],
                Instant::ZERO,
                &mut iface,
            ));
            assert_eq!(
                complete(engine.handle(
                    &destinations,
                    HOLDER,
                    request(wire::OP_CONNECT, 0),
                    &mut [],
                    Instant::ZERO,
                    &mut iface
                ))
                .status,
                Status::Exhausted
            );
            assert_eq!(engine.release_holder(HOLDER).handles, 1);
            engine.tick(Instant::ZERO + OPERATION_TIMEOUT);
        }
        let bytes = declarations(RIGHT_SEND | RIGHT_RECV, 16384, 4, 4);
        let destinations = NetworkDestinations::decode(&bytes).unwrap();
        assert_eq!(
            complete(engine.handle(
                &destinations,
                HOLDER,
                request(wire::OP_CONNECT, 0),
                &mut [],
                Instant::ZERO,
                &mut iface
            ))
            .status,
            Status::Denied
        );
        let bytes = declarations(RIGHT_CONNECT, 16384, 4, 4);
        let destinations = NetworkDestinations::decode(&bytes).unwrap();
        let mut first = 0;
        for _ in 0..SOCKETS {
            let id = pending(engine.handle(
                &destinations,
                HOLDER,
                request(wire::OP_CONNECT, 0),
                &mut [],
                Instant::ZERO,
                &mut iface,
            ));
            if first == 0 {
                first = id;
            }
            for op in [wire::OP_SEND, wire::OP_RECV] {
                assert_eq!(
                    complete(engine.handle(
                        &destinations,
                        HOLDER,
                        request(op, id),
                        &mut [0; 1],
                        Instant::ZERO,
                        &mut iface
                    ))
                    .status,
                    Status::Denied
                );
            }
        }
        assert_eq!(
            complete(engine.handle(
                &destinations,
                HOLDER,
                request(wire::OP_CONNECT, 0),
                &mut [],
                Instant::ZERO,
                &mut iface
            ))
            .status,
            Status::Exhausted
        );
        let index = engine.find(HOLDER, first).unwrap();
        engine
            .sockets
            .get_mut::<Socket>(engine.handles[index])
            .abort();
        assert_eq!(
            engine
                .poll_pending(HOLDER, first, Instant::ZERO)
                .unwrap()
                .status,
            Status::Refused
        );
        assert_eq!(engine.release_holder(HOLDER).handles, SOCKETS - 1);
        engine.serial = u32::MAX;
        assert_eq!(
            complete(engine.handle(
                &destinations,
                HOLDER,
                request(wire::OP_CONNECT, 0),
                &mut [],
                Instant::ZERO,
                &mut iface
            ))
            .status,
            Status::Exhausted
        );
    }

    #[test]
    fn interface_icmp_echo_is_not_charged_to_a_holder() {
        use smoltcp::phy::ChecksumCapabilities;
        use smoltcp::wire::{Icmpv4Packet, Icmpv4Repr, Ipv4Repr};
        let mut storage = [SocketStorage::EMPTY; SOCKETS];
        let mut rx = [[0; BUFFER_BYTES]; SOCKETS];
        let mut tx = [[0; BUFFER_BYTES]; SOCKETS];
        let mut engine = Engine::new(&mut storage, &mut rx, &mut tx, 1, 0).unwrap();
        let mut link = Link::default();
        let mut iface = interface(&mut link, 1);
        let icmp = Icmpv4Repr::EchoRequest {
            ident: 7,
            seq_no: 1,
            data: b"interface traffic",
        };
        let ip = Ipv4Repr {
            src_addr: Ipv4Address::from(PEER),
            dst_addr: Ipv4Address::new(10, 0, 0, 1),
            next_header: IpProtocol::Icmp,
            payload_len: icmp.buffer_len(),
            hop_limit: 64,
        };
        let mut frame =
            vec![0; EthernetFrame::<&[u8]>::buffer_len(ip.buffer_len() + icmp.buffer_len())];
        let mut ethernet = EthernetFrame::new_unchecked(&mut frame[..]);
        ethernet.set_ethertype(EthernetProtocol::Ipv4);
        ethernet.set_src_addr(EthernetAddress([2, 0, 0, 0, 0, 2]));
        ethernet.set_dst_addr(EthernetAddress([2, 0, 0, 0, 0, 1]));
        let mut packet = Ipv4Packet::new_unchecked(ethernet.payload_mut());
        ip.emit(&mut packet, &ChecksumCapabilities::default());
        icmp.emit(
            &mut Icmpv4Packet::new_unchecked(packet.payload_mut()),
            &ChecksumCapabilities::default(),
        );
        let mut peer_link = Link::default();
        let mut peer_iface = interface(&mut peer_link, 2);
        let mut peer_storage = [];
        let mut peer_sockets = SocketSet::new(&mut peer_storage[..]);
        let mut reply = None;
        for _ in 0..4 {
            link.rx.push_back(frame.clone());
            iface.poll(Instant::ZERO, &mut link, engine.sockets());
            while let Some(outgoing) = link.tx.pop_front() {
                assert!(engine.permit_egress(&outgoing));
                let ethernet = EthernetFrame::new_checked(&outgoing[..]).unwrap();
                if ethernet.ethertype() == EthernetProtocol::Ipv4 {
                    reply = Some(outgoing);
                } else {
                    assert_eq!(ethernet.ethertype(), EthernetProtocol::Arp);
                    peer_link.rx.push_back(outgoing);
                }
            }
            peer_iface.poll(Instant::ZERO, &mut peer_link, &mut peer_sockets);
            link.rx.extend(peer_link.tx.drain(..));
        }
        let reply = reply.expect("automatic ICMP echo reply after ARP resolution");
        let ethernet = EthernetFrame::new_checked(&reply[..]).unwrap();
        let packet = Ipv4Packet::new_checked(ethernet.payload()).unwrap();
        assert_eq!(packet.next_header(), IpProtocol::Icmp);
        assert_eq!(
            Icmpv4Repr::parse(
                &Icmpv4Packet::new_checked(packet.payload()).unwrap(),
                &ChecksumCapabilities::default()
            )
            .unwrap(),
            Icmpv4Repr::EchoReply {
                ident: 7,
                seq_no: 1,
                data: b"interface traffic"
            }
        );
        assert_eq!(engine.allocated(), 0);
        assert_eq!(engine.serial, 0);
    }

    #[test]
    fn retry_budget_covers_syn_data_fin_and_sequence_wrap() {
        use smoltcp::phy::ChecksumCapabilities;
        use smoltcp::wire::{Ipv4Repr, TcpControl, TcpRepr};
        for control in [TcpControl::Syn, TcpControl::None, TcpControl::Fin] {
            let mut storage = [SocketStorage::EMPTY; SOCKETS];
            let mut rx = [[0; BUFFER_BYTES]; SOCKETS];
            let mut tx = [[0; BUFFER_BYTES]; SOCKETS];
            let mut engine = Engine::new(&mut storage, &mut rx, &mut tx, 1, 0).unwrap();
            let mut link = Link::default();
            let mut iface = interface(&mut link, 1);
            let bytes = declarations(RIGHT_CONNECT, 4096, 1, 1);
            let destinations = NetworkDestinations::decode(&bytes).unwrap();
            let id = pending(engine.handle(
                &destinations,
                HOLDER,
                request(wire::OP_CONNECT, 0),
                &mut [],
                Instant::ZERO,
                &mut iface,
            ));
            let socket = engine.sockets.get::<Socket>(engine.handles[0]);
            let local = socket.local_endpoint().unwrap();
            let remote = socket.remote_endpoint().unwrap();
            let tcp = TcpRepr {
                src_port: local.port,
                dst_port: remote.port,
                control,
                seq_number: TcpSeqNumber(i32::MAX),
                ack_number: None,
                window_len: 2048,
                window_scale: None,
                max_seg_size: None,
                sack_permitted: false,
                sack_ranges: [None; 3],
                timestamp: None,
                payload: b"payload",
            };
            let ip = Ipv4Repr {
                src_addr: Ipv4Address::new(10, 0, 0, 1),
                dst_addr: Ipv4Address::from(PEER),
                next_header: IpProtocol::Tcp,
                payload_len: tcp.buffer_len(),
                hop_limit: 64,
            };
            let mut frame =
                vec![0; EthernetFrame::<&[u8]>::buffer_len(ip.buffer_len() + tcp.buffer_len())];
            let mut ethernet = EthernetFrame::new_unchecked(&mut frame[..]);
            ethernet.set_ethertype(EthernetProtocol::Ipv4);
            let mut packet = Ipv4Packet::new_unchecked(ethernet.payload_mut());
            ip.emit(&mut packet, &ChecksumCapabilities::default());
            tcp.emit(
                &mut TcpPacket::new_unchecked(packet.payload_mut()),
                &local.addr,
                &remote.addr,
                &ChecksumCapabilities::default(),
            );
            assert!(engine.permit_egress(&frame));
            assert!(engine.permit_egress(&frame));
            assert!(engine.permit_egress(&frame));
            assert!(!engine.permit_egress(&frame));
            assert_eq!(engine.take_timeout_reclamation(), None);
            assert_eq!(
                engine
                    .poll_pending(HOLDER, id, Instant::ZERO)
                    .unwrap()
                    .status,
                Status::Timeout
            );
            assert_eq!(engine.allocated(), 0);
            assert_eq!(
                engine.take_timeout_reclamation(),
                Some(Reclaimed {
                    handles: 1,
                    sockets: 1,
                    bytes: 2 * BUFFER_BYTES,
                })
            );
            assert_eq!(engine.take_timeout_reclamation(), None);
        }
    }

    #[test]
    fn real_peer_partial_io_eof_and_close_reclaims() {
        real_peer(false);
    }

    #[test]
    fn active_close_quarantines_time_wait_ports_until_tuple_reclamation() {
        real_peer(true);
    }

    fn real_peer(active_close: bool) {
        let mut storage = [SocketStorage::EMPTY; SOCKETS];
        let mut rx = [[0; BUFFER_BYTES]; SOCKETS];
        let mut tx = [[0; BUFFER_BYTES]; SOCKETS];
        let mut engine = Engine::new(&mut storage, &mut rx, &mut tx, 9, 0).unwrap();
        let mut local_link = Link::default();
        let mut peer_link = Link::default();
        let mut local_iface = interface(&mut local_link, 1);
        let mut peer_iface = interface(&mut peer_link, 2);
        let mut peer_storage = [SocketStorage::EMPTY];
        let mut peer_rx = [0; 4096];
        let mut peer_tx = [0; 4096];
        let mut peer_sockets = SocketSet::new(&mut peer_storage[..]);
        let mut peer = Socket::new(
            SocketBuffer::new(&mut peer_rx[..]),
            SocketBuffer::new(&mut peer_tx[..]),
        );
        peer.listen(8080).unwrap();
        peer.set_ack_delay(None);
        let peer_handle = peer_sockets.add(peer);
        let bytes = declarations(RIGHT_CONNECT | RIGHT_SEND | RIGHT_RECV, 4096, 1, 1);
        let destinations = NetworkDestinations::decode(&bytes).unwrap();
        let id = pending(engine.handle(
            &destinations,
            HOLDER,
            request(wire::OP_CONNECT, 0),
            &mut [],
            Instant::ZERO,
            &mut local_iface,
        ));
        let mut pump = |engine: &mut Engine<'_>, peer_sockets: &mut SocketSet<'_>, time: i64| {
            for _ in 0..12 {
                local_iface.poll(
                    Instant::from_millis(time),
                    &mut local_link,
                    engine.sockets(),
                );
                while let Some(frame) = local_link.tx.pop_front() {
                    assert!(engine.permit_egress(&frame));
                    peer_link.rx.push_back(frame);
                }
                peer_iface.poll(Instant::from_millis(time), &mut peer_link, peer_sockets);
                local_link.rx.extend(peer_link.tx.drain(..));
            }
        };
        pump(&mut engine, &mut peer_sockets, 0);
        assert_eq!(
            engine
                .poll_pending(HOLDER, id, Instant::ZERO)
                .unwrap()
                .capability,
            id
        );
        // Keep the interface in the pump closure while handle only needs a context on CONNECT.
        let mut dummy_link = Link::default();
        let mut context = interface(&mut dummy_link, 3);
        let mut payload = [0x5a; BUFFER_BYTES + 91];
        let sent = complete(engine.handle(
            &destinations,
            HOLDER,
            request(wire::OP_SEND, id),
            &mut payload,
            Instant::ZERO,
            &mut context,
        ));
        assert_eq!(sent.transferred, BUFFER_BYTES);
        assert!(!engine.transmit_drained());
        engine.observe_peaks();
        assert_eq!(engine.peaks().tx, BUFFER_BYTES);
        assert_eq!(
            complete(engine.handle(
                &destinations,
                HOLDER,
                request(wire::OP_SEND, id),
                &mut payload,
                Instant::ZERO,
                &mut context
            ))
            .status,
            Status::WouldBlock
        );
        assert_eq!(
            complete(engine.handle(
                &destinations,
                [2; 32],
                request(wire::OP_RECV, id),
                &mut payload,
                Instant::ZERO,
                &mut context
            ))
            .status,
            Status::Denied
        );
        pump(&mut engine, &mut peer_sockets, 10);
        assert!(engine.transmit_drained());
        if active_close {
            let local_port = engine
                .sockets
                .get::<Socket>(engine.handles[0])
                .local_endpoint()
                .unwrap()
                .port;
            pending(engine.handle(
                &destinations,
                HOLDER,
                request(wire::OP_CLOSE, id),
                &mut [],
                Instant::from_millis(10),
                &mut context,
            ));
            pump(&mut engine, &mut peer_sockets, 20);
            peer_sockets.get_mut::<Socket>(peer_handle).close();
            pump(&mut engine, &mut peer_sockets, 30);
            assert_eq!(
                engine.sockets.get::<Socket>(engine.handles[0]).state(),
                State::TimeWait
            );
            assert_eq!(
                engine
                    .poll_pending(HOLDER, id, Instant::from_millis(30))
                    .unwrap()
                    .status,
                Status::Success
            );
            assert_eq!(engine.allocated(), 0);
            assert_eq!(
                engine.sockets.get::<Socket>(engine.handles[0]).state(),
                State::TimeWait
            );
            pump(&mut engine, &mut peer_sockets, 40);
            engine.next_port = local_port;
            let next = pending(engine.handle(
                &destinations,
                HOLDER,
                request(wire::OP_CONNECT, 0),
                &mut [],
                Instant::from_millis(40),
                &mut context,
            ));
            let index = engine.find(HOLDER, next).unwrap();
            assert_ne!(index, 0);
            assert_ne!(
                engine
                    .sockets
                    .get::<Socket>(engine.handles[index])
                    .local_endpoint()
                    .unwrap()
                    .port,
                local_port
            );
            assert_eq!(engine.release_holder(HOLDER).handles, 1);
            let reclaimed = engine.reset_all(Instant::from_millis(40));
            assert_eq!(
                reclaimed,
                Reclaimed {
                    handles: 0,
                    sockets: 2,
                    bytes: 8192
                }
            );
            assert!(engine.sockets.iter().all(|(_,socket)| matches!(socket, smoltcp::socket::Socket::Tcp(socket) if socket.state()==State::Closed)));
            // Drain the aborts so no socket retains an old reset tuple.
            pump(&mut engine, &mut peer_sockets, 41);
            engine.tick(Instant::from_millis(40) + OPERATION_TIMEOUT);
            assert!(engine.resets.iter().all(Option::is_none));
            engine.next_port = local_port;
            let fresh = pending(engine.handle(
                &destinations,
                HOLDER,
                request(wire::OP_CONNECT, 0),
                &mut [],
                Instant::from_millis(10041),
                &mut context,
            ));
            let slot = engine.find(HOLDER, fresh).unwrap();
            assert_eq!(
                engine
                    .sockets
                    .get::<Socket>(engine.handles[slot])
                    .local_endpoint()
                    .unwrap()
                    .port,
                local_port
            );
            return;
        }
        let peer = peer_sockets.get_mut::<Socket>(peer_handle);
        let mut received = [0; 4096];
        assert_eq!(peer.recv_slice(&mut received).unwrap(), BUFFER_BYTES);
        assert_eq!(&received[..BUFFER_BYTES], &payload[..BUFFER_BYTES]);
        assert_eq!(peer.send_slice(b"external peer bytes").unwrap(), 19);
        peer.close();
        pump(&mut engine, &mut peer_sockets, 20);
        let mut received = [0; 7];
        let first = complete(engine.handle(
            &destinations,
            HOLDER,
            request(wire::OP_RECV, id),
            &mut received,
            Instant::from_millis(20),
            &mut context,
        ));
        assert_eq!(first.transferred, 7);
        assert!(!first.eof);
        assert_eq!(&received, b"externa");
        let mut rest = [0; 32];
        let last = complete(engine.handle(
            &destinations,
            HOLDER,
            request(wire::OP_RECV, id),
            &mut rest,
            Instant::from_millis(20),
            &mut context,
        ));
        assert_eq!(&rest[..last.transferred], b"l peer bytes");
        assert!(last.eof);
        assert_eq!(
            pending(engine.handle(
                &destinations,
                HOLDER,
                request(wire::OP_CLOSE, id),
                &mut [],
                Instant::from_millis(20),
                &mut context
            )),
            id
        );
        pump(&mut engine, &mut peer_sockets, 30);
        assert_eq!(
            engine
                .poll_pending(HOLDER, id, Instant::from_millis(30))
                .unwrap()
                .status,
            Status::Success
        );
        assert_eq!(engine.allocated(), 0);
        assert!(!engine.transmit_drained());
        assert_eq!(
            complete(engine.handle(
                &destinations,
                HOLDER,
                request(wire::OP_RECV, id),
                &mut rest,
                Instant::from_millis(30),
                &mut context
            ))
            .status,
            Status::Denied
        );
    }
}

#[cfg(test)]
#[path = "dns_tests.rs"]
mod dns_integration_tests;
