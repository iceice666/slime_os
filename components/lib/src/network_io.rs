//! Synchronous TCP requests over one explicitly provisioned IO0 ring.
//!
//! The service owns network policy. This adapter owns one request at a time and
//! never reuses a payload after losing its completion: it returns both loans
//! and poisons the session instead.

use slime_proto::io_queue::{self, WireBufferSlice};
use slime_proto::io_queue_ring::{Outstanding, Queue, QueueError};
use slime_proto::network_service::{
    self as net, WireNetworkCompletion, WireNetworkLoan, WireNetworkRequest,
};
use slime_rt::{ERR_SUCCESS, MAX_CAPS_PER_MSG, MAX_MSG};

const ANSWER_YIELDS: u32 = 2_000_000;

/// Explicitly granted wake slots and a per-request monotonic tick budget.
#[derive(Clone, Copy)]
pub struct NetworkNotifications {
    pub request_signal: u32,
    pub completion_wait: u32,
    pub timeout_ticks: u64,
}

struct NotificationWait {
    request_signal: u32,
    set: slime_rt::WaitSet,
    timeout_ticks: u64,
}

impl NotificationWait {
    fn new(control: u32, config: NetworkNotifications) -> Result<Self, NetworkError> {
        if config.timeout_ticks == 0 {
            return Err(NetworkError::BadRequest);
        }
        let mut set =
            slime_rt::WaitSet::declared(config.completion_wait).map_err(|_| NetworkError::Setup)?;
        if set.declarations().len() != 2 {
            return Err(NetworkError::Setup);
        }
        set.register_slot(slime_rt::wait_set::Kind::Stream, control)
            .map_err(|_| NetworkError::Setup)?;
        set.register_timer().map_err(|_| NetworkError::Setup)?;
        Ok(Self {
            request_signal: config.request_signal,
            set,
            timeout_ticks: config.timeout_ticks,
        })
    }

    fn signal(&self) -> Result<(), NetworkError> {
        if slime_rt::notification_signal(self.request_signal) == ERR_SUCCESS {
            Ok(())
        } else {
            Err(NetworkError::Lost)
        }
    }

    fn wait(&mut self, deadline: u64) -> Result<(), NetworkError> {
        if slime_rt::monotonic_read().map_err(|_| NetworkError::Lost)? >= deadline {
            return Err(NetworkError::Lost);
        }
        self.set.wait().map_err(|_| NetworkError::Lost)?;
        while self.set.next_ready().is_some() {}
        Ok(())
    }
}

struct RequestTimer {
    id: u64,
    deadline: u64,
    active: bool,
}

impl RequestTimer {
    fn finish(&mut self) -> Result<(), NetworkError> {
        let result = slime_rt::timer_cancel(self.id);
        let expired = result == slime_rt::ERR_BAD_CAP
            && slime_rt::monotonic_read().map_err(|_| NetworkError::Lost)? >= self.deadline;
        if result == ERR_SUCCESS || expired {
            self.active = false;
            Ok(())
        } else {
            Err(NetworkError::Lost)
        }
    }
}

impl Drop for RequestTimer {
    fn drop(&mut self) {
        if self.active {
            let _ = slime_rt::timer_cancel(self.id);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkError {
    Setup,
    BadRequest,
    Malformed,
    Lost,
}

/// An owned service connection, valid only on the session which created it.
#[derive(Debug, Eq, PartialEq)]
pub struct Connection {
    id: u64,
    owner: u64,
}

impl Connection {
    pub const fn id(&self) -> u64 {
        self.id
    }
}

/// An owned service listener, valid only on the session which created it.
#[derive(Debug, Eq, PartialEq)]
pub struct Listener {
    id: u64,
    owner: u64,
}

/// Queue admission and network results remain separate, including short I/O.
#[derive(Debug, Eq, PartialEq)]
pub struct NetworkReply {
    pub queue_status: u32,
    pub status_detail: i32,
    pub flags: u32,
    pub capability_kind: u8,
    pub capability: u64,
    pub transferred: u64,
    owner: u64,
}

impl NetworkReply {
    pub fn take_listener(&mut self) -> Option<Listener> {
        if !self.is_success()
            || self.capability_kind != net::CAPABILITY_TCP_LISTENER
            || self.capability == 0
        {
            return None;
        }
        let listener = Listener {
            id: self.capability,
            owner: self.owner,
        };
        self.capability = 0;
        self.capability_kind = net::CAPABILITY_NONE;
        Some(listener)
    }

    pub const fn is_success(&self) -> bool {
        self.queue_status == io_queue::STATUS_OK && self.status_detail == net::STATUS_SUCCESS
    }

    pub fn take_connection(&mut self) -> Option<Connection> {
        if !self.is_success()
            || self.capability_kind != net::CAPABILITY_TCP_CONNECTION
            || self.capability == 0
        {
            return None;
        }
        let connection = Connection {
            id: self.capability,
            owner: self.owner,
        };
        self.capability = 0;
        self.capability_kind = net::CAPABILITY_NONE;
        Some(connection)
    }
}

struct Region {
    slot: u32,
    descriptor: WireNetworkLoan,
    live: bool,
}

impl Region {
    fn receive(
        control: u32,
        peer: u32,
        role: u8,
        base: u64,
        length: usize,
    ) -> Result<Self, NetworkError> {
        let mut bytes = [0; MAX_MSG];
        let mut caps = [0; MAX_CAPS_PER_MSG];
        for _ in 0..ANSWER_YIELDS {
            match slime_rt::recv(peer, &mut bytes, &mut caps) {
                slime_rt::ERR_WOULDBLOCK => match slime_rt::recv(control, &mut bytes, &mut caps) {
                    slime_rt::ERR_WOULDBLOCK => slime_rt::yield_now(),
                    count if count == net::COMPLETION_BYTES as i64 => {
                        let reply = WireNetworkCompletion::decode(&bytes)
                            .filter(|reply| {
                                slime_proto::valid_network_completion(reply)
                                    && reply.op == net::OP_ATTACH
                                    && reply.status_detail != net::STATUS_SUCCESS
                            })
                            .ok_or(NetworkError::Malformed)?;
                        let _ = reply;
                        return Err(NetworkError::Setup);
                    }
                    count if count < 0 => return Err(NetworkError::Lost),
                    _ => return Err(NetworkError::Malformed),
                },
                count if count == net::LOAN_BYTES as i64 => {
                    let descriptor =
                        WireNetworkLoan::decode(&bytes).ok_or(NetworkError::Malformed)?;
                    if descriptor.magic != net::NETWORK_MAGIC
                        || descriptor.version != net::FORMAT_VERSION
                        || descriptor.role != role
                        || descriptor.reserved0 != [0; 1]
                        || descriptor.reserved != [0; 32]
                        || descriptor.length != length as u64
                        || descriptor.buffer == 0
                        || descriptor.lease == 0
                    {
                        return Err(NetworkError::Malformed);
                    }
                    let slot = slime_rt::capability_import().map_err(|_| NetworkError::Setup)?;
                    let region = Self {
                        slot,
                        descriptor,
                        live: true,
                    };
                    if slime_rt::shared_buffer_loan_map(slot, base, 0, length as u64) != ERR_SUCCESS
                    {
                        return Err(NetworkError::Setup);
                    }
                    return Ok(region);
                }
                count if count < 0 => return Err(NetworkError::Lost),
                _ => return Err(NetworkError::Malformed),
            }
        }
        Err(NetworkError::Lost)
    }

    fn release(&mut self) -> bool {
        if !self.live {
            return true;
        }
        self.live = false;
        slime_rt::shared_buffer_return(self.slot) == ERR_SUCCESS
    }
}

impl Drop for Region {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

/// A session borrows both mappings exclusively until finish or drop.
///
/// Dropping or poisoning returns client loans but does not detach the service
/// session. Use [`Self::finish`] for normal service-side buffer reclamation;
/// automatic client-death/session cleanup requires supervision integration.
pub struct NetworkIo<'a> {
    queue: Option<Queue<'a>>,
    data: Option<&'a mut [u8]>,
    outstanding: Outstanding<{ net::QUEUE_SLOTS }>,
    ring: Region,
    payload: Region,
    peer: u32,
    next_id: u64,
    notifications: Option<NotificationWait>,
}

impl<'a> NetworkIo<'a> {
    /// Ask the nontransferable control endpoint for a ring and payload loan.
    /// Loans arrive on the separate provisioning endpoint; only the control
    /// endpoint carries attach status and teardown. The service owns the buffers.
    ///
    /// # Safety
    /// Both bases must name free, page-aligned, nonoverlapping VSpace regions
    /// of the generated sizes. No other Rust reference may access either region
    /// for `'a`, and no other code may replace or unmap these mappings. The
    /// endpoints must resolve to the same trusted network service, and no concurrent
    /// logical capability import may race this session's provisioning.
    pub unsafe fn attach(
        peer: u32,
        provision: u32,
        ring_base: u64,
        data_base: u64,
    ) -> Result<Self, NetworkError> {
        // SAFETY: the caller supplies the mapping and endpoint guarantees above.
        unsafe { Self::attach_configured(peer, provision, ring_base, data_base, None) }
    }

    /// Attach with declared completion readiness and a bounded request timer.
    /// Invalid explicit wake configuration never falls back to polling.
    ///
    /// # Safety
    /// The mapping and endpoint requirements are identical to [`Self::attach`].
    pub unsafe fn attach_with_notifications(
        peer: u32,
        provision: u32,
        ring_base: u64,
        data_base: u64,
        config: NetworkNotifications,
    ) -> Result<Self, NetworkError> {
        let notifications = NotificationWait::new(peer, config)?;
        // SAFETY: the caller supplies the same mapping and endpoint guarantees.
        unsafe {
            Self::attach_configured(peer, provision, ring_base, data_base, Some(notifications))
        }
    }

    unsafe fn attach_configured(
        peer: u32,
        provision: u32,
        ring_base: u64,
        data_base: u64,
        notifications: Option<NotificationWait>,
    ) -> Result<Self, NetworkError> {
        if peer == provision
            || !ring_base.is_multiple_of(net::RING_BYTES as u64)
            || !data_base.is_multiple_of(net::DATA_BYTES as u64)
            || ring_base == data_base
            || ring_base.checked_add(net::RING_BYTES as u64).is_none()
            || data_base.checked_add(net::DATA_BYTES as u64).is_none()
            || ring_base == 0
            || data_base == 0
        {
            return Err(NetworkError::BadRequest);
        }
        if let Some(wake) = &notifications {
            wake.signal()?;
        }
        if slime_rt::send(peer, &request(net::OP_ATTACH, 0).encode(), &[]) != ERR_SUCCESS {
            return Err(NetworkError::Lost);
        }
        let ring = Region::receive(
            peer,
            provision,
            net::LOAN_ROLE_RING,
            ring_base,
            net::RING_BYTES,
        )?;
        let payload = Region::receive(
            peer,
            provision,
            net::LOAN_ROLE_DATA,
            data_base,
            net::DATA_BYTES,
        )?;
        if ring.descriptor.buffer == payload.descriptor.buffer
            || ring.descriptor.lease == payload.descriptor.lease
        {
            return Err(NetworkError::Malformed);
        }
        // SAFETY: the trusted service allocated distinct backing objects and
        // the caller grants these disjoint mapped regions for the lifetime.
        let ring_bytes =
            unsafe { core::slice::from_raw_parts_mut(ring_base as *mut u8, net::RING_BYTES) };
        // SAFETY: the separate payload loan has the same caller contract.
        let data =
            unsafe { core::slice::from_raw_parts_mut(data_base as *mut u8, net::DATA_BYTES) };
        let queue =
            Queue::attach(ring_bytes, net::QUEUE_SLOTS).map_err(|_| NetworkError::Malformed)?;
        let outstanding = Outstanding::new(queue.epoch());
        await_control(peer, net::OP_ATTACH)?;
        Ok(Self {
            queue: Some(queue),
            data: Some(data),
            outstanding,
            ring,
            payload,
            peer,
            next_id: 1,
            notifications,
        })
    }

    pub fn notification_wakes(&self) -> usize {
        self.notifications
            .as_ref()
            .map_or(0, |wake| wake.set.wakes())
    }

    pub fn connect_ipv4(
        &mut self,
        address: [u8; 4],
        port: u16,
    ) -> Result<NetworkReply, NetworkError> {
        if port == 0 {
            return Err(NetworkError::BadRequest);
        }
        let mut request = request(net::OP_CONNECT, 0);
        request.transport = net::TRANSPORT_TCP;
        request.address_kind = net::ADDRESS_IPV4;
        request.endpoint[..4].copy_from_slice(&address);
        request.port = port;
        self.transact_raw(request, io_queue::DIRECTION_NONE, 0)
    }

    pub fn listen_ipv4(
        &mut self,
        address: [u8; 4],
        port: u16,
    ) -> Result<NetworkReply, NetworkError> {
        if port == 0 {
            return Err(NetworkError::BadRequest);
        }
        let mut request = request(net::OP_LISTEN, 0);
        request.transport = net::TRANSPORT_TCP;
        request.address_kind = net::ADDRESS_IPV4;
        request.endpoint[..4].copy_from_slice(&address);
        request.port = port;
        self.transact_raw(request, io_queue::DIRECTION_NONE, 0)
    }

    pub fn accept(&mut self, listener: &Listener) -> Result<NetworkReply, NetworkError> {
        self.check_listener(listener)?;
        let mut request = request(net::OP_ACCEPT, listener.id);
        request.transport = net::TRANSPORT_TCP;
        self.transact_raw(request, io_queue::DIRECTION_NONE, 0)
    }

    pub fn close_listener(&mut self, listener: Listener) -> Result<NetworkReply, NetworkError> {
        self.check_listener(&listener)?;
        self.transact_raw(
            request(net::OP_CLOSE, listener.id),
            io_queue::DIRECTION_NONE,
            0,
        )
    }

    fn check_listener(&self, listener: &Listener) -> Result<(), NetworkError> {
        if listener.owner != self.payload.descriptor.buffer {
            return Err(NetworkError::BadRequest);
        }
        if self.queue.is_none() {
            return Err(NetworkError::Lost);
        }
        Ok(())
    }

    pub fn send(
        &mut self,
        connection: &Connection,
        bytes: &[u8],
    ) -> Result<NetworkReply, NetworkError> {
        self.check_connection(connection)?;
        self.check_length(bytes.len())?;
        self.data.as_mut().ok_or(NetworkError::Lost)?[..bytes.len()].copy_from_slice(bytes);
        self.transact_raw(
            request(net::OP_SEND, connection.id),
            io_queue::DIRECTION_DEVICE_READ,
            bytes.len() as u64,
        )
    }

    pub fn recv(
        &mut self,
        connection: &Connection,
        bytes: &mut [u8],
    ) -> Result<NetworkReply, NetworkError> {
        self.check_connection(connection)?;
        self.check_length(bytes.len())?;
        let reply = self.transact_raw(
            request(net::OP_RECV, connection.id),
            io_queue::DIRECTION_DEVICE_WRITE,
            bytes.len() as u64,
        )?;
        if reply.is_success() {
            let count = reply.transferred as usize;
            bytes[..count].copy_from_slice(&self.data.as_ref().ok_or(NetworkError::Lost)?[..count]);
        }
        Ok(reply)
    }

    pub fn close(&mut self, connection: Connection) -> Result<NetworkReply, NetworkError> {
        self.check_connection(&connection)?;
        self.transact_raw(
            request(net::OP_CLOSE, connection.id),
            io_queue::DIRECTION_NONE,
            0,
        )
    }

    fn check_connection(&self, connection: &Connection) -> Result<(), NetworkError> {
        if connection.owner != self.payload.descriptor.buffer {
            return Err(NetworkError::BadRequest);
        }
        if self.queue.is_none() {
            return Err(NetworkError::Lost);
        }
        Ok(())
    }

    fn check_length(&self, length: usize) -> Result<(), NetworkError> {
        if length == 0 || length > net::DATA_BYTES {
            Err(NetworkError::BadRequest)
        } else {
            Ok(())
        }
    }

    /// Submit a protocol payload without hiding service refusals. Invalid
    /// protocol operations may be sent for conformance testing; IO0 slices must
    /// still fit this session's sole payload mapping.
    pub fn transact_raw(
        &mut self,
        request: WireNetworkRequest,
        direction: u32,
        length: u64,
    ) -> Result<NetworkReply, NetworkError> {
        if length > net::DATA_BYTES as u64
            || (direction == io_queue::DIRECTION_NONE && length != 0)
            || !matches!(
                direction,
                io_queue::DIRECTION_NONE
                    | io_queue::DIRECTION_DEVICE_READ
                    | io_queue::DIRECTION_DEVICE_WRITE
            )
        {
            return Err(NetworkError::BadRequest);
        }
        let slice = if direction == io_queue::DIRECTION_NONE {
            WireBufferSlice {
                buffer: 0,
                lease: 0,
                offset: 0,
                length: 0,
                direction,
                reserved: [0; 4],
            }
        } else {
            WireBufferSlice {
                buffer: self.payload.descriptor.buffer,
                lease: self.payload.descriptor.lease,
                offset: 0,
                length,
                direction,
                reserved: [0; 4],
            }
        };
        let queue = self.queue.as_mut().ok_or(NetworkError::Lost)?;
        let id = self.next_id;
        self.next_id = id.checked_add(1).ok_or(NetworkError::Lost)?;
        self.outstanding
            .admit(id, slice.lease, slice.length)
            .map_err(|_| NetworkError::Malformed)?;
        let mut timer = None;
        let mut result = (|| {
            queue
                .submit(id, &slice, &request.encode(), false, net::DATA_BYTES as u64)
                .map_err(|_| NetworkError::Malformed)?;
            let deadline = if let Some(wake) = &self.notifications {
                let deadline = slime_rt::monotonic_read()
                    .map_err(|_| NetworkError::Lost)?
                    .checked_add(wake.timeout_ticks)
                    .ok_or(NetworkError::BadRequest)?;
                timer = Some(RequestTimer {
                    id: slime_rt::timer_arm(wake.timeout_ticks).map_err(|_| NetworkError::Lost)?,
                    deadline,
                    active: true,
                });
                wake.signal()?;
                deadline
            } else {
                0
            };
            let mut body = [0u8; io_queue::COMPLETION_PAYLOAD_BYTES];
            for _ in 0..ANSWER_YIELDS {
                match queue.take_completion(&self.outstanding, &mut body) {
                    Ok(completion) => {
                        if completion.request_id != id
                            || completion.payload_len != net::COMPLETION_BYTES
                            || completion.transferred > length
                        {
                            return Err(NetworkError::Malformed);
                        }
                        let reply = WireNetworkCompletion::decode(&body[..completion.payload_len])
                            .filter(|reply| {
                                slime_proto::valid_network_completion(reply)
                                    && reply.op == request.op
                            })
                            .ok_or(NetworkError::Malformed)?;
                        return Ok(NetworkReply {
                            queue_status: completion.status,
                            status_detail: reply.status_detail,
                            flags: reply.flags,
                            capability_kind: reply.capability_kind,
                            capability: reply.capability,
                            transferred: completion.transferred,
                            owner: self.payload.descriptor.buffer,
                        });
                    }
                    Err(QueueError::Empty) if queue.driver_state() != io_queue::DRIVER_DEAD => {
                        if let Some(wake) = &mut self.notifications {
                            wake.wait(deadline)?;
                        } else {
                            slime_rt::yield_now();
                        }
                    }
                    Err(QueueError::Empty | QueueError::Closed | QueueError::StaleEpoch) => {
                        return Err(NetworkError::Lost);
                    }
                    Err(_) => return Err(NetworkError::Malformed),
                }
            }
            Err(NetworkError::Lost)
        })();
        if let Some(timer) = &mut timer
            && timer.finish().is_err()
        {
            result = Err(NetworkError::Lost);
        }
        let status = match &result {
            Ok(reply) => reply.queue_status,
            Err(NetworkError::Lost) => io_queue::STATUS_DEVICE_ERROR,
            Err(_) => io_queue::STATUS_MALFORMED,
        };
        let settled = self.outstanding.settle(id, status);
        if !matches!(settled, Ok(entry) if entry.lease == slice.lease) {
            self.poison();
            return Err(NetworkError::Malformed);
        }
        if result.is_err() {
            self.poison();
        }
        result
    }

    /// Return both loans before asking the service to release its buffers.
    /// Consuming self prevents requests racing with teardown.
    pub fn finish(mut self) -> Result<(), NetworkError> {
        if self.queue.is_none() {
            return Err(NetworkError::Lost);
        }
        self.queue.take();
        self.data.take();
        let payload_ok = self.payload.release();
        let ring_ok = self.ring.release();
        if !ring_ok || !payload_ok {
            return Err(NetworkError::Setup);
        }
        if let Some(wake) = &self.notifications {
            wake.signal()?;
        }
        if slime_rt::send(self.peer, &request(net::OP_CLOSE, u64::MAX).encode(), &[]) != ERR_SUCCESS
        {
            return Err(NetworkError::Lost);
        }
        await_control(self.peer, net::OP_CLOSE)
    }

    fn poison(&mut self) {
        // Drop every borrowed view before removing the backing mappings.
        self.queue.take();
        self.data.take();
        let _ = self.ring.release();
        let _ = self.payload.release();
    }
}

impl Drop for NetworkIo<'_> {
    fn drop(&mut self) {
        self.queue.take();
        self.data.take();
    }
}

fn request(op: u8, capability: u64) -> WireNetworkRequest {
    WireNetworkRequest {
        magic: net::NETWORK_MAGIC,
        version: net::FORMAT_VERSION,
        op,
        transport: net::TRANSPORT_NONE,
        flags: 0,
        port: 0,
        name_len: 0,
        capability,
        address_kind: net::ADDRESS_NONE,
        reserved: [0; 7],
        endpoint: [0; 24],
    }
}

fn await_control(peer: u32, op: u8) -> Result<(), NetworkError> {
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    for _ in 0..ANSWER_YIELDS {
        match slime_rt::recv(peer, &mut bytes, &mut caps) {
            slime_rt::ERR_WOULDBLOCK => slime_rt::yield_now(),
            count if count == net::COMPLETION_BYTES as i64 => {
                let reply = WireNetworkCompletion::decode(&bytes).ok_or(NetworkError::Malformed)?;
                return if slime_proto::valid_network_completion(&reply)
                    && reply.op == op
                    && reply.status_detail == net::STATUS_SUCCESS
                    && reply.flags == 0
                    && reply.capability == 0
                    && reply.capability_kind == net::CAPABILITY_NONE
                {
                    Ok(())
                } else {
                    Err(NetworkError::Malformed)
                };
            }
            count if count < 0 => return Err(NetworkError::Lost),
            _ => return Err(NetworkError::Malformed),
        }
    }
    Err(NetworkError::Lost)
}
