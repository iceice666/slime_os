//! Service-owned IO0 queue and CPU-only payload for one admitted application.

use slime_proto::io_queue::{self, REQUEST_PAYLOAD_BYTES};
use slime_proto::io_queue_ring::{Outstanding, Queue, QueueError, Submission};
use slime_proto::network_service::{
    self, WireNetworkCompletion, WireNetworkLoan, WireNetworkRequest,
};
use slime_proto::valid_network_request;
use slime_rt::ERR_SUCCESS;

use crate::tcp;

const BASE: u64 = 0x0000_001a_0000_0000;
const STRIDE: u64 = 0x0001_0000;

struct Region {
    buffer: slime_rt::SharedBuffer,
    base: u64,
    mapped: bool,
    live: bool,
    lease: Option<u64>,
}

impl Region {
    fn create(factory: u32, pages: usize, base: u64, length: usize) -> Result<Self, ()> {
        let buffer = slime_rt::shared_buffer_create(factory, pages, true).map_err(|_| ())?;
        let mut region = Self {
            buffer,
            base,
            mapped: false,
            live: true,
            lease: None,
        };
        if slime_rt::shared_buffer_map(buffer.slot, base, 0, length as u64, true) != ERR_SUCCESS {
            return Err(());
        }
        region.mapped = true;
        Ok(region)
    }

    fn delegate(&mut self, peer: u32, role: u8, length: usize) -> Result<(), ()> {
        let loan = slime_rt::shared_buffer_loan(self.buffer.slot, peer, 0, length as u64, true)
            .map_err(|_| ())?;
        self.lease = Some(loan.id);
        let descriptor = WireNetworkLoan {
            magic: network_service::NETWORK_MAGIC,
            version: network_service::FORMAT_VERSION,
            role,
            reserved0: [0; 1],
            buffer: self.buffer.id,
            lease: loan.id,
            length: length as u64,
            reserved: [0; 32],
        };
        if slime_rt::capability_delegate(
            peer,
            loan.slot,
            slime_rt::CapabilityDisposition::Move,
            slime_proto::capability_transfer::OBJECT_KIND_SHARED_BUFFER_LOAN,
            boot_contracts::generation::RIGHT_BUFFER_MAP
                | boot_contracts::generation::RIGHT_BUFFER_WRITE,
            &descriptor.encode(),
        ) != ERR_SUCCESS
        {
            return Err(());
        }
        Ok(())
    }

    fn release(&mut self) -> bool {
        if !self.live {
            return true;
        }
        self.live = false;
        if let Some(lease) = self.lease.take() {
            // Normally the client returned its loan before the endpoint close.
            // On failed setup or a poisoned session revoke any remaining access.
            let _ = slime_rt::shared_buffer_revoke(self.buffer.slot, lease);
        }
        let mut success = true;
        if self.mapped {
            success &= slime_rt::shared_buffer_unmap(self.buffer.slot, self.base) == ERR_SUCCESS;
            self.mapped = false;
        }
        success &= slime_rt::shared_buffer_release(self.buffer.slot) == ERR_SUCCESS;
        success
    }
}

impl Drop for Region {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

pub struct Application {
    // Drop the borrowed ring view before its backing regions on every exit.
    queue: Queue<'static>,
    ring: Region,
    data: Region,
    outstanding: Outstanding<{ network_service::QUEUE_SLOTS }>,
    pending: Option<(Submission, u8, u64)>,
    waiting: Option<(Submission, WireNetworkRequest, smoltcp::time::Instant)>,
    completion_signal: Option<u32>,
    last_request: u64,
    failed: bool,
    disconnected: bool,
    pub sent: u64,
    pub received: u64,
}

impl Application {
    /// Allocate distinct backing objects owned by the service. Application
    /// descriptor bytes can never select or replace either mapped object.
    pub fn attach(
        index: usize,
        factory: u32,
        peer: u32,
        epoch: u64,
        completion_signal: Option<u32>,
    ) -> Result<Self, ()> {
        let base = BASE
            .checked_add((index as u64).checked_mul(STRIDE).ok_or(())?)
            .ok_or(())?;
        let data_base = base
            .checked_add(network_service::RING_BYTES as u64)
            .ok_or(())?;
        let mut ring = Region::create(
            factory,
            network_service::RING_PAGES,
            base,
            network_service::RING_BYTES,
        )?;
        let mut data = Region::create(
            factory,
            network_service::DATA_PAGES,
            data_base,
            network_service::DATA_BYTES,
        )?;
        // SAFETY: this index owns a disjoint VSpace range and a fresh buffer;
        // only the queue view borrows the ring until that view is dropped.
        let bytes = unsafe {
            core::slice::from_raw_parts_mut(base as *mut u8, network_service::RING_BYTES)
        };
        slime_proto::io_queue_ring::format(bytes, network_service::QUEUE_SLOTS, epoch)
            .map_err(|_| ())?;
        ring.delegate(
            peer,
            network_service::LOAN_ROLE_RING,
            network_service::RING_BYTES,
        )?;
        data.delegate(
            peer,
            network_service::LOAN_ROLE_DATA,
            network_service::DATA_BYTES,
        )?;
        let queue = Queue::attach(bytes, network_service::QUEUE_SLOTS).map_err(|_| ())?;
        let outstanding = Outstanding::new(queue.epoch());
        Ok(Self {
            queue,
            ring,
            data,
            outstanding,
            pending: None,
            waiting: None,
            completion_signal,
            last_request: 0,
            failed: false,
            disconnected: false,
            sent: 0,
            received: 0,
        })
    }

    pub fn drive(
        &mut self,
        holder: [u8; 32],
        destinations: &boot_contracts::network_destination::NetworkDestinations<'_>,
        local_applications: Option<&boot_contracts::network_application::NetworkApplications<'_>>,
        engine: &mut tcp::Engine<'_>,
        iface: &mut smoltcp::iface::Interface,
        now: smoltcp::time::Instant,
    ) -> bool {
        if self.failed {
            return false;
        }
        if let Some((submission, op, capability)) = self.pending {
            if let Some(result) = engine.poll_pending(holder, capability, now) {
                self.pending = None;
                self.complete(submission, op, result);
                return true;
            }
            return false;
        }
        if let Some((submission, request, deadline)) = self.waiting {
            if now >= deadline {
                self.waiting = None;
                self.refuse(submission, request.op, network_service::STATUS_TIMEOUT);
                return true;
            }
            let outcome = self.dispatch(
                submission,
                request,
                holder,
                destinations,
                local_applications,
                engine,
                iface,
                now,
            );
            match outcome {
                tcp::Outcome::Complete(result) if result.status != tcp::Status::WouldBlock => {
                    self.waiting = None;
                    self.complete(submission, request.op, result);
                    return true;
                }
                tcp::Outcome::Complete(_) => return false,
                tcp::Outcome::Pending { capability } => {
                    self.waiting = None;
                    self.pending = Some((submission, request.op, capability));
                    return true;
                }
            }
        }
        // Reserve a terminal slot before consuming a submission; a client that
        // stops draining cannot force a lost completion or overwrite.
        if self.queue.completions_pending() >= network_service::QUEUE_SLOTS as u64 {
            return false;
        }
        let mut bytes = [0; REQUEST_PAYLOAD_BYTES];
        let submission = match self
            .queue
            .take_request(&mut bytes, network_service::DATA_BYTES as u64)
        {
            Ok(submission) => submission,
            Err(error) if error.error == QueueError::Empty => return false,
            Err(error) => {
                if !slime_proto::admit_network_request_id(
                    &mut self.last_request,
                    error.request_id,
                    error.epoch,
                    self.queue.epoch(),
                ) || self
                    .queue
                    .complete(error.request_id, io_queue::STATUS_MALFORMED, 0, &[], false)
                    .is_err()
                {
                    self.mark_failed();
                } else if let Some(slot) = self.completion_signal
                    && slime_rt::notification_signal(slot) != ERR_SUCCESS
                {
                    self.mark_failed();
                }
                return true;
            }
        };
        let request = (submission.payload_len == network_service::REQUEST_BYTES)
            .then(|| WireNetworkRequest::decode(&bytes[..submission.payload_len]))
            .flatten();
        let op = request.map_or(0, |request| request.op);
        if !slime_proto::admit_network_request_id(
            &mut self.last_request,
            submission.request_id,
            submission.epoch,
            self.queue.epoch(),
        ) {
            self.mark_failed();
            return true;
        }
        if self
            .outstanding
            .admit(
                submission.request_id,
                submission.slice.lease,
                submission.slice.length,
            )
            .is_err()
            || self.outstanding.start(submission.request_id).is_err()
        {
            self.mark_failed();
            return true;
        }
        let Some(request) = request.filter(valid_network_request) else {
            // No operation beyond network-service/v1 exists; in particular a
            // client has none that selects or changes its socket options.
            let status = if request.is_some_and(unknown_operation) {
                network_service::STATUS_UNSUPPORTED
            } else {
                network_service::STATUS_MALFORMED
            };
            self.refuse(submission, op, status);
            return true;
        };
        if self.disconnected && request.op != network_service::OP_CLOSE {
            self.refuse(submission, op, network_service::STATUS_RESET);
            return true;
        }
        let slice = submission.slice;
        if !slime_proto::valid_network_payload_slice(
            &request,
            &slice,
            self.data.buffer.id,
            self.data.lease.unwrap_or(0),
            network_service::DATA_BYTES as u64,
        ) {
            self.refuse(submission, op, network_service::STATUS_MALFORMED);
            return true;
        }
        let outcome = self.dispatch(
            submission,
            request,
            holder,
            destinations,
            local_applications,
            engine,
            iface,
            now,
        );
        match outcome {
            tcp::Outcome::Complete(result)
                if result.status == tcp::Status::WouldBlock
                    && self.completion_signal.is_some()
                    && request.flags & network_service::FLAG_NONBLOCKING == 0
                    && matches!(
                        op,
                        network_service::OP_SEND
                            | network_service::OP_RECV
                            | network_service::OP_ACCEPT
                    ) =>
            {
                self.waiting = Some((
                    submission,
                    request,
                    now + smoltcp::time::Duration::from_secs(4),
                ));
            }
            tcp::Outcome::Complete(result) => self.complete(submission, op, result),
            tcp::Outcome::Pending { capability } => {
                self.pending = Some((submission, op, capability))
            }
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn dispatch(
        &mut self,
        submission: Submission,
        request: WireNetworkRequest,
        holder: [u8; 32],
        destinations: &boot_contracts::network_destination::NetworkDestinations<'_>,
        local_applications: Option<&boot_contracts::network_application::NetworkApplications<'_>>,
        engine: &mut tcp::Engine<'_>,
        iface: &mut smoltcp::iface::Interface,
        now: smoltcp::time::Instant,
    ) -> tcp::Outcome {
        // Validated descriptors retain the service-owned mapping while pending;
        // each dispatch creates a new temporary view and never stores a reference.
        let data = unsafe {
            core::slice::from_raw_parts_mut(
                (self.data.base + submission.slice.offset) as *mut u8,
                submission.slice.length as usize,
            )
        };
        match local_applications {
            Some(table) => {
                engine.handle_local(destinations, table, holder, request, data, now, iface)
            }
            None => engine.handle(destinations, holder, request, data, now, iface),
        }
    }

    fn complete(&mut self, submission: Submission, op: u8, result: tcp::Completion) {
        let status = match result.status {
            tcp::Status::Success => network_service::STATUS_SUCCESS,
            tcp::Status::Denied => network_service::STATUS_DENIED,
            tcp::Status::Malformed => network_service::STATUS_MALFORMED,
            tcp::Status::Unsupported => network_service::STATUS_UNSUPPORTED,
            tcp::Status::Exhausted => network_service::STATUS_EXHAUSTED,
            tcp::Status::WouldBlock => network_service::STATUS_WOULD_BLOCK,
            tcp::Status::Refused => network_service::STATUS_REFUSED,
            tcp::Status::Timeout => network_service::STATUS_TIMEOUT,
            tcp::Status::Reset => network_service::STATUS_RESET,
        };
        let reply = WireNetworkCompletion {
            magic: network_service::NETWORK_MAGIC,
            version: network_service::FORMAT_VERSION,
            op,
            capability_kind: result.capability_kind,
            status_detail: status,
            flags: if result.eof {
                network_service::FLAG_END_OF_STREAM
            } else {
                0
            },
            capability: result.capability,
        };
        if !self.publish(submission, result.transferred as u64, reply) {
            return;
        }
        if op == network_service::OP_SEND {
            self.sent += result.transferred as u64;
        }
        if op == network_service::OP_RECV {
            self.received += result.transferred as u64;
        }
    }

    fn refuse(&mut self, submission: Submission, op: u8, status: i32) {
        let reply = WireNetworkCompletion {
            magic: network_service::NETWORK_MAGIC,
            version: network_service::FORMAT_VERSION,
            op,
            capability_kind: network_service::CAPABILITY_NONE,
            status_detail: status,
            flags: 0,
            capability: 0,
        };
        self.publish(submission, 0, reply);
    }

    fn publish(
        &mut self,
        submission: Submission,
        transferred: u64,
        reply: WireNetworkCompletion,
    ) -> bool {
        if self
            .queue
            .complete(
                submission.request_id,
                io_queue::STATUS_OK,
                transferred,
                &reply.encode(),
                false,
            )
            .is_err()
            || self
                .outstanding
                .settle(submission.request_id, io_queue::STATUS_OK)
                .is_err()
        {
            self.mark_failed();
            return false;
        }
        if let Some(slot) = self.completion_signal
            && slime_rt::notification_signal(slot) != ERR_SUCCESS
        {
            self.mark_failed();
            return false;
        }
        true
    }

    fn mark_failed(&mut self) {
        self.failed = true;
        self.pending = None;
        self.waiting = None;
        self.outstanding
            .settle_all(io_queue::STATUS_DEVICE_ERROR, |_| {});
        self.queue.mark_driver_dead();
        if let Some(slot) = self.completion_signal {
            let _ = slime_rt::notification_signal(slot);
        }
    }

    pub fn reset_requests(&mut self) -> usize {
        self.disconnected = true;
        let mut count = 0;
        if let Some((submission, op, _)) = self.pending.take() {
            self.refuse(submission, op, network_service::STATUS_RESET);
            count += 1;
        }
        if let Some((submission, request, _)) = self.waiting.take() {
            self.refuse(submission, request.op, network_service::STATUS_RESET);
            count += 1;
        }
        count
    }

    pub fn in_flight_payload(&self) -> bool {
        self.waiting.is_some_and(|(_, request, _)| {
            matches!(
                request.op,
                network_service::OP_SEND | network_service::OP_RECV
            )
        })
    }

    pub const fn failed(&self) -> bool {
        self.failed
    }

    pub fn quiescent(&self) -> bool {
        self.pending.is_none()
            && self.waiting.is_none()
            && self.outstanding.is_empty()
            && self.queue.submitted() == 0
            && self.queue.completions_pending() == 0
    }

    pub fn release(mut self) -> bool {
        self.pending = None;
        self.waiting = None;
        self.outstanding
            .settle_all(io_queue::STATUS_PEER_DEAD, |_| {});
        self.queue.mark_driver_dead();
        let (mut ring, mut data) = {
            let Self { ring, data, .. } = self;
            (ring, data)
        };
        let data_ok = data.release();
        let ring_ok = ring.release();
        data_ok && ring_ok
    }
}

/// A well-formed envelope naming an operation network-service/v1 does not
/// define. Known operations with invalid fields remain malformed.
fn unknown_operation(request: WireNetworkRequest) -> bool {
    request.magic == network_service::NETWORK_MAGIC
        && request.version == network_service::FORMAT_VERSION
        && request.flags & !network_service::KNOWN_REQUEST_FLAGS == 0
        && request.reserved.iter().all(|byte| *byte == 0)
        && !matches!(
            request.op,
            network_service::OP_CONNECT
                | network_service::OP_SEND
                | network_service::OP_RECV
                | network_service::OP_CLOSE
                | network_service::OP_LISTEN
                | network_service::OP_ACCEPT
                | network_service::OP_RESOLVE
                | network_service::OP_ATTACH
                | network_service::OP_ABORT
                | network_service::OP_SHUTDOWN
        )
}
