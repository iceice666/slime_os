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

#[cfg(not(test))]
const ANSWER_YIELDS: u32 = 2_000_000;
// Tests exercise retry exhaustion without a live peer, including under Miri.
#[cfg(test)]
const ANSWER_YIELDS: u32 = 4;

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
        deadline: Option<u64>,
    ) -> Result<Self, NetworkError> {
        let mut bytes = [0; MAX_MSG];
        let mut caps = [0; MAX_CAPS_PER_MSG];
        for _ in 0..ANSWER_YIELDS {
            check_deadline(deadline)?;
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
                    if !slime_proto::valid_network_loan(&descriptor, role)
                        || descriptor.length != length as u64
                    {
                        return Err(NetworkError::Malformed);
                    }
                    let slot = slime_rt::capability_import().map_err(|_| NetworkError::Setup)?;
                    let region = Self {
                        slot,
                        descriptor,
                        live: true,
                    };
                    check_deadline(deadline)?;
                    if slime_rt::shared_buffer_loan_map(slot, base, 0, length as u64) != ERR_SUCCESS
                    {
                        return Err(NetworkError::Setup);
                    }
                    check_deadline(deadline)?;
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
/// session. Use [`Self::finish`] for normal reclamation or acknowledged
/// [`Self::abort_with_deadline`] after failure. Automatic client-death cleanup
/// requires supervision integration.
pub struct NetworkIo<'a> {
    queue: Option<Queue<'a>>,
    data: Option<&'a mut [u8]>,
    outstanding: Outstanding<{ net::QUEUE_SLOTS }>,
    ring: Region,
    payload: Region,
    peer: u32,
    next_id: u64,
    notifications: Option<NotificationWait>,
    deadline: Option<u64>,
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
        unsafe { Self::attach_configured(peer, provision, ring_base, data_base, None, None) }
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
            Self::attach_configured(
                peer,
                provision,
                ring_base,
                data_base,
                Some(notifications),
                None,
            )
        }
    }

    /// Attach and bound all provisioning polls and subsequent transactions by
    /// one absolute deadline. Native endpoint sends remain rendezvous operations;
    /// the composition must supervise a dead peer rather than promise interruption.
    ///
    /// # Safety
    /// The mapping and endpoint requirements are identical to [`Self::attach`].
    pub unsafe fn attach_with_deadline(
        peer: u32,
        provision: u32,
        ring_base: u64,
        data_base: u64,
        config: Option<NetworkNotifications>,
        deadline: u64,
    ) -> Result<Self, NetworkError> {
        check_deadline(Some(deadline))?;
        let notifications = config
            .map(|config| NotificationWait::new(peer, config))
            .transpose()?;
        // SAFETY: the caller supplies the same mapping and endpoint guarantees.
        unsafe {
            Self::attach_configured(
                peer,
                provision,
                ring_base,
                data_base,
                notifications,
                Some(deadline),
            )
        }
    }

    unsafe fn attach_configured(
        peer: u32,
        provision: u32,
        ring_base: u64,
        data_base: u64,
        notifications: Option<NotificationWait>,
        deadline: Option<u64>,
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
        check_deadline(deadline)?;
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
            deadline,
        )?;
        let payload = Region::receive(
            peer,
            provision,
            net::LOAN_ROLE_DATA,
            data_base,
            net::DATA_BYTES,
            deadline,
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
        await_control(peer, net::OP_ATTACH, deadline)?;
        Ok(Self {
            queue: Some(queue),
            data: Some(data),
            outstanding,
            ring,
            payload,
            peer,
            next_id: 1,
            notifications,
            deadline,
        })
    }

    /// Bound subsequent transactions by one absolute monotonic deadline.
    /// A deadline can be shortened, never extended, after it has been installed.
    pub fn set_deadline(&mut self, deadline: u64) -> Result<(), NetworkError> {
        if deadline <= slime_rt::monotonic_read().map_err(|_| NetworkError::Lost)?
            || self.deadline.is_some_and(|old| deadline > old)
        {
            return Err(NetworkError::BadRequest);
        }
        self.deadline = Some(deadline);
        Ok(())
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

    /// Resolve and connect under one exact holder/name/TCP/port grant.
    /// The service retains the answer; this never grants numeric-IP authority.
    pub fn connect_hostname(
        &mut self,
        name: &[u8],
        port: u16,
    ) -> Result<NetworkReply, NetworkError> {
        if name.is_empty() || name.len() > net::MAX_NAME_BYTES || port == 0 {
            return Err(NetworkError::BadRequest);
        }
        let mut request = request(net::OP_CONNECT, 0);
        request.transport = net::TRANSPORT_TCP;
        request.address_kind = net::ADDRESS_DNS;
        request.endpoint[..name.len()].copy_from_slice(name);
        request.name_len = name.len() as u16;
        request.port = port;
        if !slime_proto::valid_network_request(&request) {
            return Err(NetworkError::BadRequest);
        }
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
        self.send_flagged(connection, bytes, 0)
    }

    /// Like [`Self::send`], but a full transmit buffer answers `would-block`
    /// at once rather than being retained by a notification-enabled service,
    /// so the caller observes backpressure instead of waiting through it.
    pub fn send_nonblocking(
        &mut self,
        connection: &Connection,
        bytes: &[u8],
    ) -> Result<NetworkReply, NetworkError> {
        self.send_flagged(connection, bytes, net::FLAG_NONBLOCKING)
    }

    fn send_flagged(
        &mut self,
        connection: &Connection,
        bytes: &[u8],
        flags: u32,
    ) -> Result<NetworkReply, NetworkError> {
        self.check_connection(connection)?;
        self.check_length(bytes.len())?;
        self.data.as_mut().ok_or(NetworkError::Lost)?[..bytes.len()].copy_from_slice(bytes);
        let mut request = request(net::OP_SEND, connection.id);
        request.flags = flags;
        self.transact_raw(request, io_queue::DIRECTION_DEVICE_READ, bytes.len() as u64)
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
            || (direction == io_queue::DIRECTION_NONE) != (length == 0)
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
        if let Some(deadline) = self.deadline
            && slime_rt::monotonic_read().map_err(|_| NetworkError::Lost)? >= deadline
        {
            return Err(NetworkError::Lost);
        }
        let queue = self.queue.as_mut().ok_or(NetworkError::Lost)?;
        let id = self.next_id;
        self.next_id = id.checked_add(1).ok_or(NetworkError::Lost)?;
        self.outstanding
            .admit(id, slice.lease, slice.length)
            .map_err(|_| NetworkError::Malformed)?;
        let mut timer = None;
        let mut published = false;
        let mut result = (|| {
            queue
                .submit(id, &slice, &request.encode(), false, net::DATA_BYTES as u64)
                .map_err(|error| match error {
                    QueueError::Closed | QueueError::StaleEpoch => NetworkError::Lost,
                    QueueError::Malformed | QueueError::TooLarge => NetworkError::BadRequest,
                    _ => NetworkError::Malformed,
                })?;
            published = true;
            let deadline = if let Some(wake) = &self.notifications {
                let now = slime_rt::monotonic_read().map_err(|_| NetworkError::Lost)?;
                let requested = now
                    .checked_add(wake.timeout_ticks)
                    .ok_or(NetworkError::BadRequest)?;
                let deadline = self
                    .deadline
                    .map_or(requested, |limit| limit.min(requested));
                let remaining = deadline
                    .checked_sub(now)
                    .filter(|ticks| *ticks > 0)
                    .ok_or(NetworkError::Lost)?;
                timer = Some(RequestTimer {
                    id: slime_rt::timer_arm(remaining).map_err(|_| NetworkError::Lost)?,
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
                if let Some(deadline) = self.deadline
                    && slime_rt::monotonic_read().map_err(|_| NetworkError::Lost)? >= deadline
                {
                    return Err(NetworkError::Lost);
                }
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
            if published {
                self.poison();
            }
            return Err(NetworkError::Malformed);
        }
        // Only a published request can leave the peer using this payload.
        if published && result.is_err() {
            self.poison();
        }
        result
    }

    /// Return both loans before asking the service to release its buffers.
    /// Consuming self prevents requests racing with teardown.
    pub fn finish(self) -> Result<(), NetworkError> {
        self.finish_configured(None, false)
    }

    /// Return loans and detach with a separate bounded cleanup grace. This never
    /// extends the response deadline or permits additional ring transactions.
    pub fn finish_with_deadline(self, deadline: u64) -> Result<(), NetworkError> {
        self.finish_configured(Some(deadline), false)
    }

    /// Discard this session, including a poisoned in-flight transaction. The
    /// acknowledgement attests service-owned reclamation, not HTTP completion.
    pub fn abort_with_deadline(self, deadline: u64) -> Result<(), NetworkError> {
        self.finish_configured(Some(deadline), true)
    }

    /// Abort failed setup after local provisioning owners have returned their
    /// loans. `peer` must be this caller's nontransferable service control endpoint.
    pub fn abort_session(peer: u32, deadline: u64) -> Result<(), NetworkError> {
        send_teardown(peer, net::OP_ABORT, Some(deadline))
    }

    fn finish_configured(mut self, deadline: Option<u64>, abort: bool) -> Result<(), NetworkError> {
        let usable = self.queue.is_some();
        self.queue.take();
        self.data.take();
        let payload_ok = self.payload.release();
        let ring_ok = self.ring.release();
        if !abort && !usable {
            return Err(NetworkError::Lost);
        }
        if !abort && (!ring_ok || !payload_ok) {
            return Err(NetworkError::Setup);
        }
        check_deadline(deadline)?;
        if let Some(wake) = &self.notifications {
            wake.signal()?;
        }
        send_teardown(
            self.peer,
            if abort { net::OP_ABORT } else { net::OP_CLOSE },
            deadline,
        )
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

fn check_deadline(deadline: Option<u64>) -> Result<(), NetworkError> {
    if let Some(deadline) = deadline
        && slime_rt::monotonic_read().map_err(|_| NetworkError::Lost)? >= deadline
    {
        return Err(NetworkError::Lost);
    }
    Ok(())
}

fn send_teardown(peer: u32, op: u8, deadline: Option<u64>) -> Result<(), NetworkError> {
    check_deadline(deadline)?;
    if slime_rt::send(peer, &request(op, u64::MAX).encode(), &[]) != ERR_SUCCESS {
        return Err(NetworkError::Lost);
    }
    await_control(peer, op, deadline)
}

fn await_control(peer: u32, op: u8, deadline: Option<u64>) -> Result<(), NetworkError> {
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    for _ in 0..ANSWER_YIELDS {
        check_deadline(deadline)?;
        let count = slime_rt::recv(peer, &mut bytes, &mut caps);
        check_deadline(deadline)?;
        match count {
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

#[cfg(test)]
mod tests {
    use super::*;
    use slime_proto::io_queue_ring::format;

    fn session<'a>(ring: &'a mut [u8], data: &'a mut [u8]) -> NetworkIo<'a> {
        format(ring, net::QUEUE_SLOTS, 7).unwrap();
        let queue = Queue::attach(ring, net::QUEUE_SLOTS).unwrap();
        let region = |role, buffer, lease| Region {
            slot: buffer as u32,
            descriptor: WireNetworkLoan {
                magic: net::NETWORK_MAGIC,
                version: net::FORMAT_VERSION,
                role,
                reserved0: [0],
                buffer,
                lease,
                length: net::DATA_BYTES as u64,
                reserved: [0; 32],
            },
            live: true,
        };
        NetworkIo {
            queue: Some(queue),
            data: Some(data),
            outstanding: Outstanding::new(7),
            ring: region(net::LOAN_ROLE_RING, 1, 2),
            payload: region(net::LOAN_ROLE_DATA, 3, 4),
            peer: 5,
            next_id: 1,
            notifications: None,
            deadline: None,
        }
    }

    fn complete_close(io: &mut NetworkIo<'_>) {
        let completion = WireNetworkCompletion {
            magic: net::NETWORK_MAGIC,
            version: net::FORMAT_VERSION,
            op: net::OP_CLOSE,
            capability_kind: net::CAPABILITY_NONE,
            status_detail: net::STATUS_SUCCESS,
            flags: 0,
            capability: 0,
        };
        io.queue
            .as_mut()
            .unwrap()
            .complete(
                io.next_id,
                io_queue::STATUS_OK,
                0,
                &completion.encode(),
                false,
            )
            .unwrap();
    }

    fn scripted_completion(io: &mut NetworkIo<'_>, op: u8, capability: u64, transferred: u64) {
        let completion = WireNetworkCompletion {
            magic: net::NETWORK_MAGIC,
            version: net::FORMAT_VERSION,
            op,
            capability_kind: if op == net::OP_CONNECT {
                net::CAPABILITY_TCP_CONNECTION
            } else {
                net::CAPABILITY_NONE
            },
            status_detail: net::STATUS_SUCCESS,
            flags: 0,
            capability,
        };
        io.queue
            .as_mut()
            .unwrap()
            .complete(
                io.next_id,
                io_queue::STATUS_OK,
                transferred,
                &completion.encode(),
                false,
            )
            .unwrap();
    }

    fn consume_scripted_request(io: &mut NetworkIo<'_>, op: u8) {
        let mut bytes = [0; io_queue::REQUEST_PAYLOAD_BYTES];
        io.queue
            .as_mut()
            .unwrap()
            .take_request(&mut bytes, net::DATA_BYTES as u64)
            .unwrap();
        assert_eq!(WireNetworkRequest::decode(&bytes).unwrap().op, op);
    }

    #[test]
    fn six_adapter_rounds_return_each_loan_once_after_success_and_poison() {
        let mut ring = [0; net::RING_BYTES];
        let mut data = [0; net::DATA_BYTES];
        let mut returns = 0;
        let mut teardowns = 0;
        for round in 0..6 {
            for poisoned in [false, true] {
                let mut io = session(&mut ring, &mut data);
                slime_rt::test_ipc(
                    if poisoned {
                        net::OP_ABORT
                    } else {
                        net::OP_CLOSE
                    },
                    0,
                );
                slime_rt::test_clock(Some((10, 0)));
                scripted_completion(&mut io, net::OP_CONNECT, 100 + round, 0);
                let connection = io
                    .connect_ipv4([10, 0, 2, 2], 80)
                    .unwrap()
                    .take_connection()
                    .unwrap();
                consume_scripted_request(&mut io, net::OP_CONNECT);
                scripted_completion(&mut io, net::OP_SEND, 0, 4);
                assert_eq!(io.send(&connection, b"GET ").unwrap().transferred, 4);
                consume_scripted_request(&mut io, net::OP_SEND);
                let mut received = [0; 4];
                if poisoned {
                    assert_eq!(io.recv(&connection, &mut received), Err(NetworkError::Lost));
                    assert!(io.queue.is_none());
                    assert_eq!(io.abort_with_deadline(20), Ok(()));
                } else {
                    io.data.as_mut().unwrap()[..4].copy_from_slice(b"body");
                    scripted_completion(&mut io, net::OP_RECV, 0, 4);
                    assert_eq!(io.recv(&connection, &mut received).unwrap().transferred, 4);
                    assert_eq!(&received, b"body");
                    consume_scripted_request(&mut io, net::OP_RECV);
                    scripted_completion(&mut io, net::OP_CLOSE, 0, 0);
                    assert!(io.close(connection).unwrap().is_success());
                    consume_scripted_request(&mut io, net::OP_CLOSE);
                    assert_eq!(io.finish_with_deadline(20), Ok(()));
                }
                let (sent, returned) = slime_rt::test_ipc_counts();
                assert_eq!((sent, returned), (1, 2));
                teardowns += sent;
                returns += returned;
                slime_rt::test_clock(None);
            }
        }
        assert_eq!((teardowns, returns), (12, 24));
    }

    #[test]
    fn cleanup_deadline_releases_loans_without_sending() {
        let mut ring = [0; net::RING_BYTES];
        let mut data = [0; net::DATA_BYTES];
        let io = session(&mut ring, &mut data);
        slime_rt::test_ipc(net::OP_CLOSE, 0);
        slime_rt::test_clock(Some((10, 0)));
        assert_eq!(io.finish_with_deadline(10), Err(NetworkError::Lost));
        assert_eq!(slime_rt::test_ipc_counts(), (0, 2));
        slime_rt::test_clock(None);
    }

    #[test]
    fn cleanup_grace_is_independent_and_abort_handles_poison() {
        for abort in [false, true] {
            let mut ring = [0; net::RING_BYTES];
            let mut data = [0; net::DATA_BYTES];
            let mut io = session(&mut ring, &mut data);
            io.deadline = Some(1);
            slime_rt::test_ipc(if abort { net::OP_ABORT } else { net::OP_CLOSE }, 0);
            slime_rt::test_clock(Some((10, 0)));
            let result = if abort {
                io.poison();
                io.abort_with_deadline(20)
            } else {
                io.finish_with_deadline(20)
            };
            assert_eq!(result, Ok(()));
            assert_eq!(slime_rt::test_ipc_counts(), (1, 2));
            slime_rt::test_clock(None);
        }
    }

    #[test]
    fn control_ack_at_deadline_is_failure_without_replay() {
        slime_rt::test_ipc(net::OP_ABORT, 0);
        slime_rt::test_clock(Some((10, 1)));
        assert_eq!(NetworkIo::abort_session(5, 12), Err(NetworkError::Lost));
        assert_eq!(slime_rt::test_ipc_counts(), (1, 0));
        slime_rt::test_clock(None);
    }

    #[test]
    fn expired_setup_does_not_publish_or_import() {
        slime_rt::test_ipc(net::OP_ATTACH, 0);
        slime_rt::test_clock(Some((10, 0)));
        // SAFETY: expiry is checked before these never-accessed mappings.
        let result = unsafe { NetworkIo::attach_with_deadline(5, 6, 4096, 8192, None, 10) };
        assert!(matches!(result, Err(NetworkError::Lost)));
        assert_eq!(slime_rt::test_ipc_counts(), (0, 0));
        assert!(matches!(
            Region::receive(5, 6, net::LOAN_ROLE_RING, 4096, net::RING_BYTES, Some(10)),
            Err(NetworkError::Lost)
        ));
        slime_rt::test_clock(None);
    }

    #[test]
    fn hostname_connect_validates_before_publishing_and_preserves_exact_name() {
        let mut ring = [0; net::RING_BYTES];
        let mut data = [0; net::DATA_BYTES];
        let mut io = session(&mut ring, &mut data);
        for (name, port) in [
            (b"".as_slice(), 80),
            (b"example.com".as_slice(), 0),
            (b"bad\r\nHost: x".as_slice(), 80),
            (b"*.example.com".as_slice(), 80),
            (b"abcdefghijklmnopqrstuvwxyz".as_slice(), 80),
        ] {
            assert_eq!(
                io.connect_hostname(name, port),
                Err(NetworkError::BadRequest)
            );
            assert_eq!(io.next_id, 1);
            assert_eq!(io.queue.as_ref().unwrap().submitted(), 0);
        }
        let completion = WireNetworkCompletion {
            magic: net::NETWORK_MAGIC,
            version: net::FORMAT_VERSION,
            op: net::OP_CONNECT,
            capability_kind: net::CAPABILITY_TCP_CONNECTION,
            status_detail: net::STATUS_SUCCESS,
            flags: 0,
            capability: 123,
        };
        io.queue
            .as_mut()
            .unwrap()
            .complete(1, io_queue::STATUS_OK, 0, &completion.encode(), false)
            .unwrap();
        assert_eq!(
            io.connect_hostname(b"example.com", 80)
                .unwrap()
                .take_connection()
                .unwrap()
                .id(),
            123
        );
        let mut bytes = [0; io_queue::REQUEST_PAYLOAD_BYTES];
        let submitted = io
            .queue
            .as_mut()
            .unwrap()
            .take_request(&mut bytes, net::DATA_BYTES as u64)
            .unwrap();
        let sent = WireNetworkRequest::decode(&bytes).unwrap();
        assert_eq!(sent.address_kind, net::ADDRESS_DNS);
        assert_eq!(sent.port, 80);
        assert_eq!(&sent.endpoint[..sent.name_len as usize], b"example.com");
        assert_eq!(submitted.slice.length, 0);
    }

    #[test]
    fn absolute_deadline_refuses_extension_and_expires_without_publication() {
        let mut ring = [0; net::RING_BYTES];
        let mut data = [0; net::DATA_BYTES];
        let mut io = session(&mut ring, &mut data);
        slime_rt::test_clock(Some((10, 0)));
        assert_eq!(io.set_deadline(10), Err(NetworkError::BadRequest));
        assert_eq!(io.set_deadline(20), Ok(()));
        assert_eq!(io.set_deadline(21), Err(NetworkError::BadRequest));
        assert_eq!(io.set_deadline(19), Ok(()));
        slime_rt::test_clock(Some((19, 0)));
        assert_eq!(
            io.connect_hostname(b"example.com", 80),
            Err(NetworkError::Lost)
        );
        assert_eq!(io.next_id, 1);
        assert_eq!(io.queue.as_ref().unwrap().submitted(), 0);
        assert!(io.ring.live && io.payload.live);
        slime_rt::test_clock(None);
    }

    #[test]
    fn deadline_during_published_request_settles_and_poisons() {
        let mut ring = [0; net::RING_BYTES];
        let mut data = [0; net::DATA_BYTES];
        let mut io = session(&mut ring, &mut data);
        slime_rt::test_clock(Some((10, 1)));
        io.set_deadline(13).unwrap();
        assert_eq!(
            io.connect_hostname(b"example.com", 80),
            Err(NetworkError::Lost)
        );
        assert!(io.outstanding.is_empty());
        assert!(io.queue.is_none() && io.data.is_none());
        assert!(!io.ring.live && !io.payload.live);
        slime_rt::test_clock(None);
    }

    #[test]
    fn invalid_slices_do_not_admit_publish_or_poison() {
        let mut ring = [0; net::RING_BYTES];
        let mut data = [0; net::DATA_BYTES];
        let mut io = session(&mut ring, &mut data);
        for (direction, length) in [
            (io_queue::DIRECTION_DEVICE_READ, 0),
            (io_queue::DIRECTION_DEVICE_WRITE, 0),
            (io_queue::DIRECTION_NONE, 1),
            (io_queue::DIRECTION_DEVICE_READ, net::DATA_BYTES as u64 + 1),
            (u32::MAX, 1),
        ] {
            assert_eq!(
                io.transact_raw(request(net::OP_CLOSE, 1), direction, length),
                Err(NetworkError::BadRequest)
            );
            assert_eq!(io.next_id, 1);
            assert!(io.outstanding.is_empty());
            assert_eq!(io.queue.as_ref().unwrap().submitted(), 0);
            assert!(io.data.is_some() && io.ring.live && io.payload.live);
        }
        complete_close(&mut io);
        assert!(
            io.transact_raw(request(net::OP_CLOSE, 1), io_queue::DIRECTION_NONE, 0)
                .unwrap()
                .is_success()
        );
    }

    #[test]
    fn full_submission_settles_without_poisoning_and_can_retry() {
        let mut ring = [0; net::RING_BYTES];
        let mut data = [0; net::DATA_BYTES];
        let mut io = session(&mut ring, &mut data);
        let empty = WireBufferSlice {
            buffer: 0,
            lease: 0,
            offset: 0,
            length: 0,
            direction: io_queue::DIRECTION_NONE,
            reserved: [0; 4],
        };
        for id in 10..10 + net::QUEUE_SLOTS as u64 {
            io.queue
                .as_mut()
                .unwrap()
                .submit(id, &empty, &[], false, 0)
                .unwrap();
        }
        assert_eq!(
            io.transact_raw(request(net::OP_CLOSE, 1), io_queue::DIRECTION_NONE, 0),
            Err(NetworkError::Malformed)
        );
        assert!(io.outstanding.is_empty());
        assert_eq!(
            io.queue.as_ref().unwrap().submitted(),
            net::QUEUE_SLOTS as u64
        );
        assert!(io.data.is_some() && io.ring.live && io.payload.live);
        let mut body = [0; io_queue::REQUEST_PAYLOAD_BYTES];
        for _ in 0..net::QUEUE_SLOTS {
            io.queue
                .as_mut()
                .unwrap()
                .take_request(&mut body, 0)
                .unwrap();
        }
        complete_close(&mut io);
        assert!(
            io.transact_raw(request(net::OP_CLOSE, 1), io_queue::DIRECTION_NONE, 0)
                .unwrap()
                .is_success()
        );
        assert!(io.outstanding.is_empty());
    }

    #[test]
    fn closed_submission_is_lost_without_publication_or_poisoning() {
        for dead in [false, true] {
            let mut ring = [0; net::RING_BYTES];
            let mut data = [0; net::DATA_BYTES];
            let mut io = session(&mut ring, &mut data);
            if dead {
                io.queue.as_mut().unwrap().mark_driver_dead();
            } else {
                io.queue.as_mut().unwrap().begin_reset();
            }
            assert_eq!(
                io.transact_raw(request(net::OP_CLOSE, 1), io_queue::DIRECTION_NONE, 0),
                Err(NetworkError::Lost)
            );
            assert!(io.outstanding.is_empty());
            assert_eq!(io.queue.as_ref().unwrap().submitted(), 0);
            assert!(io.data.is_some() && io.ring.live && io.payload.live);
        }
    }

    #[test]
    fn lost_completion_after_publication_settles_and_poisons() {
        let mut ring = [0; net::RING_BYTES];
        let mut data = [0; net::DATA_BYTES];
        let mut io = session(&mut ring, &mut data);
        assert_eq!(
            io.transact_raw(request(net::OP_CLOSE, 1), io_queue::DIRECTION_NONE, 0),
            Err(NetworkError::Lost)
        );
        assert!(io.outstanding.is_empty());
        assert!(io.queue.is_none() && io.data.is_none());
        assert!(!io.ring.live && !io.payload.live);
    }

    #[test]
    fn malformed_completion_after_publication_settles_and_poisons() {
        let mut ring = [0; net::RING_BYTES];
        let mut data = [0; net::DATA_BYTES];
        let mut io = session(&mut ring, &mut data);
        io.queue
            .as_mut()
            .unwrap()
            .complete(1, io_queue::STATUS_OK, 0, &[], false)
            .unwrap();
        assert_eq!(
            io.transact_raw(request(net::OP_CLOSE, 1), io_queue::DIRECTION_NONE, 0),
            Err(NetworkError::Malformed)
        );
        assert!(io.outstanding.is_empty());
        assert!(io.queue.is_none() && io.data.is_none());
        assert!(!io.ring.live && !io.payload.live);
        assert_eq!(
            io.transact_raw(request(net::OP_CLOSE, 1), io_queue::DIRECTION_NONE, 0),
            Err(NetworkError::Lost)
        );
    }
}
