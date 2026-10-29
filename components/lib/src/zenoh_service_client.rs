//! Synchronous requests to the Zenoh service over one explicitly provisioned IO0 ring.
//!
//! The service owns the Zenoh link, the session and every grant. This adapter owns one request at a time. It
//! never reuses a payload after losing a completion: it returns both loans and poisons the session instead.
//! A request names a grant row by its index and never a key, and the service learns the holder from the
//! endpoint the request arrives on.
//!
//! The pattern is `network_io`. The control endpoint carries attach, detach and abort. The provisioning endpoint
//! carries the two loans, the ring first. Publish and take travel in the IO0 ring, and a sample's header and
//! payload travel in the data page. Notifications and deadlines are not used: a completion that does not arrive
//! within the bounded polling is lost, and the session is poisoned.

use slime_proto::io_queue::{self, WireBufferSlice};
use slime_proto::io_queue_ring::{Outstanding, Queue, QueueError};
use slime_proto::zenoh_service_client as wire;
use slime_proto::zenoh_service_client_wire::{
    WireSampleHeader, WireServiceCompletion, WireServiceLoan, WireServiceRequest,
};
use slime_rt::{ERR_SUCCESS, MAX_CAPS_PER_MSG, MAX_MSG};

#[cfg(not(test))]
const ANSWER_YIELDS: u32 = 2_000_000;
// Tests exercise retry exhaustion without a live peer, including under Miri.
#[cfg(test)]
const ANSWER_YIELDS: u32 = 4;

const RING_BYTES: usize = wire::RING_BYTES as usize;
const DATA_BYTES: usize = wire::DATA_BYTES as usize;
const QUEUE_SLOTS: usize = wire::QUEUE_SLOTS as usize;
const COMPLETION_BYTES: usize = wire::COMPLETION_BYTES as usize;
const LOAN_BYTES: usize = wire::LOAN_BYTES as usize;
const HEADER_BYTES: usize = wire::SAMPLE_HEADER_BYTES as usize;
/// The largest sample payload: the header and the payload fill the data page.
pub const MAX_PAYLOAD_BYTES: usize = wire::MAX_SAMPLE_PAYLOAD_BYTES as usize;
const MAX_ROWS: usize = wire::MAX_ROWS as usize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientError {
    /// Provisioning failed.
    Setup,
    /// The request is outside the protocol. Nothing was published.
    BadRequest,
    /// The service answered outside the protocol.
    Malformed,
    /// The completion did not arrive, or the session is gone.
    Lost,
    /// The IO0 layer refused the request. The request settled and the session is still usable.
    Refused,
}

/// What the service said about a publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PublishReply {
    /// The service's status, `wire::STATUS_SUCCESS` when it accepted the publication.
    pub status: i32,
    /// The number of local holders the sample reached.
    pub reached: u32,
    /// Whether the service sent a copy to the peer.
    pub peer_sent: bool,
}

/// The next item of the holder's ring.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Item {
    /// The ring is empty.
    None,
    /// A sample, its payload copied to the caller's buffer. `row` is the holder's grant row for the sample's key.
    Sample {
        row: usize,
        sequence: i64,
        timestamp: i64,
        len: usize,
    },
    /// The count of samples the ring dropped since the last take.
    Loss { count: u32 },
}

/// What the service said about a take.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TakeReply {
    pub status: i32,
    pub item: Item,
}

struct Region {
    slot: u32,
    descriptor: WireServiceLoan,
    live: bool,
}

impl Region {
    fn receive(
        control: u32,
        peer: u32,
        role: u8,
        base: u64,
        length: usize,
    ) -> Result<Self, ClientError> {
        let mut bytes = [0; MAX_MSG];
        let mut caps = [0; MAX_CAPS_PER_MSG];
        for _ in 0..ANSWER_YIELDS {
            match slime_rt::recv(peer, &mut bytes, &mut caps) {
                slime_rt::ERR_WOULDBLOCK => match slime_rt::recv(control, &mut bytes, &mut caps) {
                    slime_rt::ERR_WOULDBLOCK => slime_rt::yield_now(),
                    count if count == COMPLETION_BYTES as i64 => {
                        // The service refuses the attach with a typed completion on the control endpoint.
                        let reply = WireServiceCompletion::decode(&bytes)
                            .filter(|reply| {
                                valid_completion(reply, wire::OP_ATTACH as u8)
                                    && reply.status != wire::STATUS_SUCCESS as i32
                            })
                            .ok_or(ClientError::Malformed)?;
                        let _ = reply;
                        return Err(ClientError::Setup);
                    }
                    count if count < 0 => return Err(ClientError::Lost),
                    _ => return Err(ClientError::Malformed),
                },
                count if count == LOAN_BYTES as i64 => {
                    let descriptor =
                        WireServiceLoan::decode(&bytes).ok_or(ClientError::Malformed)?;
                    // `valid_loan` fixes the length from the role, and `length` is the length of that role.
                    if !valid_loan(&descriptor, role) {
                        return Err(ClientError::Malformed);
                    }
                    let slot = slime_rt::capability_import().map_err(|_| ClientError::Setup)?;
                    let region = Self {
                        slot,
                        descriptor,
                        live: true,
                    };
                    if slime_rt::shared_buffer_loan_map(slot, base, 0, length as u64) != ERR_SUCCESS
                    {
                        return Err(ClientError::Setup);
                    }
                    return Ok(region);
                }
                count if count < 0 => return Err(ClientError::Lost),
                _ => return Err(ClientError::Malformed),
            }
        }
        Err(ClientError::Lost)
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
/// Dropping or poisoning returns the client loans but does not detach the service session. Use
/// [`Self::finish`] for normal reclamation, or [`Self::abort`] after a failure.
pub struct ClientIo<'a> {
    queue: Option<Queue<'a>>,
    data: Option<&'a mut [u8]>,
    outstanding: Outstanding<QUEUE_SLOTS>,
    ring: Region,
    payload: Region,
    peer: u32,
    next_id: u64,
    rows: usize,
}

impl<'a> ClientIo<'a> {
    /// Ask the nontransferable control endpoint for a ring and a data loan. The loans arrive on the separate
    /// provisioning endpoint, ring first. The attach completion reports how many grant rows the holder has.
    ///
    /// # Safety
    /// Both bases must name free, page-aligned, nonoverlapping VSpace regions of the generated sizes. No
    /// other Rust reference may access either region for `'a`, and no other code may replace or unmap these
    /// mappings. The endpoints must resolve to the same trusted Zenoh service, and no concurrent logical
    /// capability import may race this session's provisioning.
    pub unsafe fn attach(
        peer: u32,
        provision: u32,
        ring_base: u64,
        data_base: u64,
    ) -> Result<Self, ClientError> {
        if peer == provision
            || !ring_base.is_multiple_of(RING_BYTES as u64)
            || !data_base.is_multiple_of(DATA_BYTES as u64)
            || ring_base == data_base
            || ring_base.checked_add(RING_BYTES as u64).is_none()
            || data_base.checked_add(DATA_BYTES as u64).is_none()
            || ring_base == 0
            || data_base == 0
        {
            return Err(ClientError::BadRequest);
        }
        if slime_rt::send(peer, &control(wire::OP_ATTACH as u8).encode(), &[]) != ERR_SUCCESS {
            return Err(ClientError::Lost);
        }
        let mut ring = Region::receive(
            peer,
            provision,
            wire::LOAN_ROLE_RING as u8,
            ring_base,
            RING_BYTES,
        )?;
        let mut payload = Region::receive(
            peer,
            provision,
            wire::LOAN_ROLE_DATA as u8,
            data_base,
            DATA_BYTES,
        )?;
        // The service holds a session once both loans have arrived. A failure from here on must end it.
        let mut answer = AttachAnswer::Unread;
        let setup = (|| {
            if ring.descriptor.buffer == payload.descriptor.buffer
                || ring.descriptor.lease == payload.descriptor.lease
            {
                return Err(ClientError::Malformed);
            }
            // SAFETY: the trusted service allocated distinct backing objects and the caller grants these
            // disjoint mapped regions for the lifetime.
            let ring_bytes =
                unsafe { core::slice::from_raw_parts_mut(ring_base as *mut u8, RING_BYTES) };
            // SAFETY: the separate data loan has the same caller contract.
            let data = unsafe { core::slice::from_raw_parts_mut(data_base as *mut u8, DATA_BYTES) };
            let queue =
                Queue::attach(ring_bytes, QUEUE_SLOTS).map_err(|_| ClientError::Malformed)?;
            let outstanding = Outstanding::new(queue.epoch());
            let rows = await_control(peer, wire::OP_ATTACH as u8).inspect_err(|error| {
                answer = if *error == ClientError::Lost {
                    AttachAnswer::Absent
                } else {
                    AttachAnswer::Read
                };
            })?;
            answer = AttachAnswer::Read;
            if rows == 0 || rows as usize > MAX_ROWS {
                return Err(ClientError::Malformed);
            }
            Ok((queue, data, outstanding, rows))
        })();
        match setup {
            Ok((queue, data, outstanding, rows)) => Ok(Self {
                queue: Some(queue),
                data: Some(data),
                outstanding,
                ring,
                payload,
                peer,
                next_id: 1,
                rows: rows as usize,
            }),
            Err(error) => {
                let _ = payload.release();
                let _ = ring.release();
                abandon_attach(peer, answer);
                Err(error)
            }
        }
    }

    /// How many grant rows the holder has. A request names a row by an index below this.
    pub const fn rows(&self) -> usize {
        self.rows
    }

    /// Publish `payload` on the key of grant row `row`. `sequence` is the holder's own counter. The service
    /// stamps the timestamp.
    pub fn publish(
        &mut self,
        row: usize,
        sequence: i64,
        payload: &[u8],
    ) -> Result<PublishReply, ClientError> {
        if row >= self.rows || payload.len() > MAX_PAYLOAD_BYTES {
            return Err(ClientError::BadRequest);
        }
        let (direction, length) = if payload.is_empty() {
            (io_queue::DIRECTION_NONE, 0)
        } else {
            self.data.as_mut().ok_or(ClientError::Lost)?[..payload.len()].copy_from_slice(payload);
            (io_queue::DIRECTION_DEVICE_READ, payload.len() as u64)
        };
        let completion = self.transact(
            request(wire::OP_PUBLISH as u8, row as u16, sequence),
            direction,
            length,
        )?;
        let reply = completion.reply;
        let accepted = reply.status == wire::STATUS_SUCCESS as i32;
        // A publish completion echoes the request's row. The service read the whole payload when it accepted the
        // publication and none of it when it refused.
        if reply.item != wire::ITEM_NONE as u8
            || reply.row != row as u16
            || completion.transferred != if accepted { length } else { 0 }
            || (!accepted && reply.length != 0)
        {
            return Err(self.malformed());
        }
        Ok(PublishReply {
            status: reply.status,
            reached: reply.length,
            peer_sent: reply.flags & wire::FLAG_PEER_SENT as u32 != 0,
        })
    }

    /// Take the holder's next item. A sample's payload is copied to `out`, which must be no larger than the
    /// largest payload. A sample longer than `out` is the service's to refuse.
    pub fn take(&mut self, out: &mut [u8]) -> Result<TakeReply, ClientError> {
        if out.len() > MAX_PAYLOAD_BYTES {
            return Err(ClientError::BadRequest);
        }
        let capacity = HEADER_BYTES + out.len();
        let completion = self.transact(
            request(wire::OP_TAKE as u8, 0, 0),
            io_queue::DIRECTION_DEVICE_WRITE,
            capacity as u64,
        )?;
        let reply = completion.reply;
        let item = if reply.status != wire::STATUS_SUCCESS as i32 {
            if completion.transferred != 0 || reply.row != 0 {
                return Err(self.malformed());
            }
            Item::None
        } else {
            match i64::from(reply.item) {
                wire::ITEM_NONE
                    if reply.length == 0 && completion.transferred == 0 && reply.row == 0 =>
                {
                    Item::None
                }
                wire::ITEM_LOSS
                    if reply.length > 0 && completion.transferred == 0 && reply.row == 0 =>
                {
                    Item::Loss {
                        count: reply.length,
                    }
                }
                // `transact` already refuses a completion that moved more than the request's capacity, so a length
                // over `out` cannot get here. The check stays so this match does not depend on that.
                wire::ITEM_SAMPLE
                    if reply.length as usize <= out.len()
                        && usize::from(reply.row) < self.rows
                        && completion.transferred
                            == (HEADER_BYTES + reply.length as usize) as u64 =>
                {
                    let len = reply.length as usize;
                    let page = self.data.as_ref().ok_or(ClientError::Lost)?;
                    let Some(header) = WireSampleHeader::decode(&page[..HEADER_BYTES]) else {
                        return Err(self.malformed());
                    };
                    out[..len].copy_from_slice(&page[HEADER_BYTES..HEADER_BYTES + len]);
                    Item::Sample {
                        row: usize::from(reply.row),
                        sequence: header.sequence,
                        timestamp: header.timestamp,
                        len,
                    }
                }
                _ => return Err(self.malformed()),
            }
        };
        Ok(TakeReply {
            status: reply.status,
            item,
        })
    }

    fn transact(
        &mut self,
        request: WireServiceRequest,
        direction: u32,
        length: u64,
    ) -> Result<Answer, ClientError> {
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
        let queue = self.queue.as_mut().ok_or(ClientError::Lost)?;
        let id = self.next_id;
        self.next_id = id.checked_add(1).ok_or(ClientError::Lost)?;
        self.outstanding
            .admit(id, slice.lease, slice.length)
            .map_err(|_| ClientError::Malformed)?;
        let mut published = false;
        let result = (|| {
            queue
                .submit(id, &slice, &request.encode(), false, DATA_BYTES as u64)
                .map_err(|error| match error {
                    QueueError::Closed | QueueError::StaleEpoch => ClientError::Lost,
                    QueueError::Malformed | QueueError::TooLarge => ClientError::BadRequest,
                    _ => ClientError::Malformed,
                })?;
            published = true;
            let mut body = [0u8; io_queue::COMPLETION_PAYLOAD_BYTES];
            for _ in 0..ANSWER_YIELDS {
                match queue.take_completion(&self.outstanding, &mut body) {
                    Ok(completion) => {
                        if completion.request_id != id
                            || completion.payload_len != COMPLETION_BYTES
                            || completion.transferred > length
                        {
                            return Err(ClientError::Malformed);
                        }
                        let reply = WireServiceCompletion::decode(&body[..completion.payload_len])
                            .filter(|reply| valid_completion(reply, request.op))
                            .ok_or(ClientError::Malformed)?;
                        return Ok(Answer {
                            reply,
                            queue_status: completion.status,
                            transferred: completion.transferred,
                        });
                    }
                    Err(QueueError::Empty) if queue.driver_state() != io_queue::DRIVER_DEAD => {
                        slime_rt::yield_now();
                    }
                    Err(QueueError::Empty | QueueError::Closed | QueueError::StaleEpoch) => {
                        return Err(ClientError::Lost);
                    }
                    Err(_) => return Err(ClientError::Malformed),
                }
            }
            Err(ClientError::Lost)
        })();
        let status = match &result {
            Ok(answer) => answer.queue_status,
            Err(ClientError::Lost) => io_queue::STATUS_DEVICE_ERROR,
            Err(_) => io_queue::STATUS_MALFORMED,
        };
        let settled = self.outstanding.settle(id, status);
        if !matches!(settled, Ok(entry) if entry.lease == slice.lease) {
            if published {
                self.poison();
            }
            return Err(ClientError::Malformed);
        }
        // Only a published request can leave the peer using this payload.
        if published && result.is_err() {
            self.poison();
        }
        match result {
            Ok(answer) if answer.queue_status != io_queue::STATUS_OK => Err(ClientError::Refused),
            other => other,
        }
    }

    /// An answer outside the protocol: the service may still be using the data page, so the session is poisoned.
    fn malformed(&mut self) -> ClientError {
        self.poison();
        ClientError::Malformed
    }

    /// Return both loans before asking the service to release its buffers. Consuming `self` prevents requests
    /// racing with teardown.
    pub fn finish(self) -> Result<(), ClientError> {
        self.teardown(wire::OP_DETACH as u8)
    }

    /// Discard this session, including a poisoned in-flight request. The acknowledgement attests
    /// service-owned reclamation.
    pub fn abort(self) -> Result<(), ClientError> {
        self.teardown(wire::OP_ABORT as u8)
    }

    fn teardown(mut self, op: u8) -> Result<(), ClientError> {
        let usable = self.queue.is_some();
        self.queue.take();
        self.data.take();
        let payload_ok = self.payload.release();
        let ring_ok = self.ring.release();
        let abort = op == wire::OP_ABORT as u8;
        if !abort && !usable {
            return Err(ClientError::Lost);
        }
        if !abort && (!ring_ok || !payload_ok) {
            return Err(ClientError::Setup);
        }
        if slime_rt::send(self.peer, &control(op).encode(), &[]) != ERR_SUCCESS {
            return Err(ClientError::Lost);
        }
        await_control(self.peer, op).map(|_| ())
    }

    fn poison(&mut self) {
        // Drop every borrowed view before removing the backing mappings.
        self.queue.take();
        self.data.take();
        let _ = self.ring.release();
        let _ = self.payload.release();
    }
}

impl Drop for ClientIo<'_> {
    fn drop(&mut self) {
        self.queue.take();
        self.data.take();
    }
}

struct Answer {
    reply: WireServiceCompletion,
    queue_status: u32,
    transferred: u64,
}

fn request(op: u8, row: u16, sequence: i64) -> WireServiceRequest {
    WireServiceRequest {
        magic: wire::SERVICE_MAGIC as u32,
        version: wire::FORMAT_VERSION as u16,
        op,
        flags: 0,
        row,
        reserved: [0; 2],
        sequence,
        reserved2: [0; 12],
    }
}

fn control(op: u8) -> WireServiceRequest {
    request(op, 0, 0)
}

fn valid_loan(descriptor: &WireServiceLoan, expected_role: u8) -> bool {
    let length = match i64::from(expected_role) {
        wire::LOAN_ROLE_RING => RING_BYTES,
        wire::LOAN_ROLE_DATA => DATA_BYTES,
        _ => return false,
    };
    descriptor.magic == wire::SERVICE_MAGIC as u32
        && descriptor.version == wire::FORMAT_VERSION as u16
        && descriptor.role == expected_role
        && descriptor.reserved0 == [0; 1]
        && descriptor.reserved == [0; 32]
        && descriptor.length == length as u64
        && descriptor.buffer != 0
        && descriptor.lease != 0
}

/// A completion the protocol allows for `op`. Success carries the fields of the operation. A failure carries
/// a failure status and nothing else.
fn valid_completion(reply: &WireServiceCompletion, op: u8) -> bool {
    let status = i64::from(reply.status);
    if reply.magic != wire::SERVICE_MAGIC as u32
        || reply.version != wire::FORMAT_VERSION as u16
        || reply.op != op
        || reply.reserved != [0; 2]
        || i64::from(reply.flags) & !wire::KNOWN_COMPLETION_FLAGS != 0
        || !(wire::STATUS_LINK_CLOSED..=wire::STATUS_SUCCESS).contains(&status)
    {
        return false;
    }
    if status != wire::STATUS_SUCCESS {
        return reply.item == wire::ITEM_NONE as u8 && reply.length == 0 && reply.flags == 0;
    }
    match i64::from(op) {
        wire::OP_PUBLISH => reply.item == wire::ITEM_NONE as u8,
        wire::OP_TAKE => {
            reply.flags == 0
                && matches!(
                    i64::from(reply.item),
                    wire::ITEM_NONE | wire::ITEM_SAMPLE | wire::ITEM_LOSS
                )
        }
        wire::OP_ATTACH | wire::OP_DETACH | wire::OP_ABORT => {
            reply.item == wire::ITEM_NONE as u8
                && reply.flags == 0
                && reply.row == 0
                && (i64::from(op) == wire::OP_ATTACH || reply.length == 0)
        }
        _ => false,
    }
}

/// Wait for the service's completion on the control endpoint. Returns its `length`, which is the number of
/// grant rows after an attach and zero after a detach or an abort.
/// What the adapter knows of the service's attach answer when setup fails.
#[derive(Clone, Copy, PartialEq)]
enum AttachAnswer {
    /// Setup failed before the adapter looked for it.
    Unread,
    /// The adapter consumed it.
    Read,
    /// The adapter waited and none came.
    Absent,
}

/// End the service's session after an attach that the adapter could not use. The loans are already back.
/// The service sends the attach answer with a send that blocks until it is received, so the answer is read
/// before the abort is sent. With no answer the service is not known to be receiving, and no abort is sent.
fn abandon_attach(peer: u32, answer: AttachAnswer) {
    match answer {
        AttachAnswer::Absent => return,
        AttachAnswer::Unread => {
            if await_control(peer, wire::OP_ATTACH as u8) == Err(ClientError::Lost) {
                return;
            }
        }
        AttachAnswer::Read => {}
    }
    if slime_rt::send(peer, &control(wire::OP_ABORT as u8).encode(), &[]) != ERR_SUCCESS {
        return;
    }
    let _ = await_control(peer, wire::OP_ABORT as u8);
}

fn await_control(peer: u32, op: u8) -> Result<u32, ClientError> {
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    for _ in 0..ANSWER_YIELDS {
        match slime_rt::recv(peer, &mut bytes, &mut caps) {
            slime_rt::ERR_WOULDBLOCK => slime_rt::yield_now(),
            count if count == COMPLETION_BYTES as i64 => {
                let reply = WireServiceCompletion::decode(&bytes).ok_or(ClientError::Malformed)?;
                return if valid_completion(&reply, op)
                    && reply.status == wire::STATUS_SUCCESS as i32
                {
                    Ok(reply.length)
                } else {
                    Err(ClientError::Malformed)
                };
            }
            count if count < 0 => return Err(ClientError::Lost),
            _ => return Err(ClientError::Malformed),
        }
    }
    Err(ClientError::Lost)
}

#[cfg(test)]
mod tests {
    type LoanCase = (&'static str, fn(&mut WireServiceLoan));
    type AbortCase = (
        &'static str,
        std::vec::Vec<std::vec::Vec<u8>>,
        std::vec::Vec<std::vec::Vec<u8>>,
        bool,
        ClientError,
    );
    type AttachCase = (
        &'static str,
        std::vec::Vec<std::vec::Vec<u8>>,
        std::vec::Vec<std::vec::Vec<u8>>,
        ClientError,
    );

    use super::*;
    use slime_proto::io_queue_ring::format;
    use slime_proto::zenoh_service_client_wire::{WireSampleHeader, WireServiceRequest};

    const MAGIC: u32 = wire::SERVICE_MAGIC as u32;
    const VERSION: u16 = wire::FORMAT_VERSION as u16;

    fn line(text: &str) {
        println!("[zenoh-exam] clientio {text}");
    }

    /// The failed-attach exam prints under its own marker, so the adapter exam's judge never reads these lines.
    fn abort_line(text: &str) {
        println!("[zenoh-abort] {text}");
    }

    fn hex(bytes: &[u8]) -> std::string::String {
        bytes
            .iter()
            .map(|byte| std::format!("{byte:02x}"))
            .collect()
    }

    fn answer(
        op: i64,
        status: i64,
        item: i64,
        row: u16,
        length: u32,
        flags: u32,
    ) -> WireServiceCompletion {
        WireServiceCompletion {
            magic: MAGIC,
            version: VERSION,
            op: op as u8,
            item: item as u8,
            status: status as i32,
            row,
            reserved: [0; 2],
            length,
            flags,
        }
    }

    fn loan(role: i64, buffer: u64, lease: u64, length: usize) -> std::vec::Vec<u8> {
        WireServiceLoan {
            magic: MAGIC,
            version: VERSION,
            role: role as u8,
            reserved0: [0],
            buffer,
            lease,
            length: length as u64,
            reserved: [0; 32],
        }
        .encode()
        .to_vec()
    }

    /// A session over two buffers the test owns, with the data loan's identity the tests below use.
    fn session<'a>(ring: &'a mut [u8], data: &'a mut [u8], rows: usize) -> ClientIo<'a> {
        format(ring, QUEUE_SLOTS, 7).unwrap();
        let queue = Queue::attach(ring, QUEUE_SLOTS).unwrap();
        let region = |role, buffer, lease, length| Region {
            slot: buffer as u32,
            descriptor: WireServiceLoan {
                magic: MAGIC,
                version: VERSION,
                role,
                reserved0: [0],
                buffer,
                lease,
                length,
                reserved: [0; 32],
            },
            live: true,
        };
        ClientIo {
            queue: Some(queue),
            data: Some(data),
            outstanding: Outstanding::new(7),
            ring: region(wire::LOAN_ROLE_RING as u8, 1, 2, RING_BYTES as u64),
            payload: region(wire::LOAN_ROLE_DATA as u8, 3, 4, DATA_BYTES as u64),
            peer: 5,
            next_id: 1,
            rows,
        }
    }

    /// Plays the service: takes the adapter's request out of the ring, returns it, and completes it with `reply`.
    fn serve(
        io: &mut ClientIo<'_>,
        reply: WireServiceCompletion,
        transferred: u64,
    ) -> (WireServiceRequest, WireBufferSlice) {
        let mut body = [0u8; io_queue::REQUEST_PAYLOAD_BYTES];
        let queue = io.queue.as_mut().unwrap();
        let submission = queue.take_request(&mut body, DATA_BYTES as u64).unwrap();
        let request = WireServiceRequest::decode(&body).unwrap();
        queue
            .complete(
                submission.request_id,
                io_queue::STATUS_OK,
                transferred,
                &reply.encode(),
                false,
            )
            .unwrap();
        (request, submission.slice)
    }

    fn control_answers(op: i64, rows: u32) -> std::vec::Vec<std::vec::Vec<u8>> {
        std::vec![
            answer(op, wire::STATUS_SUCCESS, wire::ITEM_NONE, 0, rows, 0)
                .encode()
                .to_vec()
        ]
    }

    /// The service's data page after a take that returned a sample: the header, then the payload.
    fn put_sample(io: &mut ClientIo<'_>, sequence: i64, timestamp: i64, payload: &[u8]) {
        let page = io.data.as_mut().unwrap();
        page[..HEADER_BYTES].copy_from_slice(
            &WireSampleHeader {
                sequence,
                timestamp,
            }
            .encode(),
        );
        page[HEADER_BYTES..HEADER_BYTES + payload.len()].copy_from_slice(payload);
    }

    fn a_publication_travels_as_a_request_and_its_payload_in_the_data_page() {
        slime_rt::test_script(std::vec::Vec::new(), std::vec::Vec::new());
        let mut ring = [0u8; RING_BYTES];
        let mut data = [0u8; DATA_BYTES];
        let mut io = session(&mut ring, &mut data, 3);
        // Play the service by hand: the completion is placed in the ring before the adapter looks for it.
        io.queue
            .as_mut()
            .unwrap()
            .complete(
                1,
                io_queue::STATUS_OK,
                3,
                &answer(
                    wire::OP_PUBLISH,
                    wire::STATUS_SUCCESS,
                    wire::ITEM_NONE,
                    2,
                    2,
                    wire::FLAG_PEER_SENT as u32,
                )
                .encode(),
                false,
            )
            .unwrap();
        let reply = io.publish(2, -7, &[1, 2, 3]).unwrap();
        line(&std::format!(
            "publish reply {} {} {}",
            reply.status,
            reply.reached,
            reply.peer_sent
        ));
        assert_eq!(
            reply,
            PublishReply {
                status: 0,
                reached: 2,
                peer_sent: true
            }
        );
        let mut body = [0u8; io_queue::REQUEST_PAYLOAD_BYTES];
        let submission = io
            .queue
            .as_mut()
            .unwrap()
            .take_request(&mut body, DATA_BYTES as u64)
            .unwrap();
        let request = WireServiceRequest::decode(&body).unwrap();
        line(&std::format!("publish request {}", hex(&request.encode())));
        line(&std::format!(
            "publish slice {} {} {} {} {} {}",
            submission.slice.offset,
            submission.slice.length,
            submission.slice.direction,
            submission.slice.buffer,
            submission.slice.lease,
            hex(&data_prefix(&io, 3))
        ));
        assert_eq!(request.op, wire::OP_PUBLISH as u8);
        assert_eq!(request.row, 2);
        assert_eq!(request.sequence, -7);
        assert_eq!(submission.slice.direction, io_queue::DIRECTION_DEVICE_READ);
        assert_eq!(submission.slice.length, 3);
        assert_eq!(submission.slice.lease, 4);
        assert_eq!(submission.slice.buffer, 3);
    }

    fn data_prefix(io: &ClientIo<'_>, count: usize) -> std::vec::Vec<u8> {
        io.data.as_ref().unwrap()[..count].to_vec()
    }

    /// Place the service's completion for request `id`, as the driver would.
    fn complete(io: &mut ClientIo<'_>, id: u64, reply: WireServiceCompletion, transferred: u64) {
        io.queue
            .as_mut()
            .unwrap()
            .complete(id, io_queue::STATUS_OK, transferred, &reply.encode(), false)
            .unwrap();
    }

    fn take_request(io: &mut ClientIo<'_>) -> (WireServiceRequest, WireBufferSlice) {
        let mut body = [0u8; io_queue::REQUEST_PAYLOAD_BYTES];
        let submission = io
            .queue
            .as_mut()
            .unwrap()
            .take_request(&mut body, DATA_BYTES as u64)
            .unwrap();
        (WireServiceRequest::decode(&body).unwrap(), submission.slice)
    }

    fn a_take_returns_a_sample_with_its_row_header_and_payload() {
        slime_rt::test_script(std::vec::Vec::new(), std::vec::Vec::new());
        let mut ring = [0u8; RING_BYTES];
        let mut data = [0u8; DATA_BYTES];
        let mut io = session(&mut ring, &mut data, 3);
        put_sample(&mut io, 41, -9, &[7, 8, 9, 10]);
        complete(
            &mut io,
            1,
            answer(
                wire::OP_TAKE,
                wire::STATUS_SUCCESS,
                wire::ITEM_SAMPLE,
                1,
                4,
                0,
            ),
            20,
        );
        let mut out = [0u8; 16];
        let reply = io.take(&mut out).unwrap();
        let Item::Sample {
            row,
            sequence,
            timestamp,
            len,
        } = reply.item
        else {
            panic!("a sample")
        };
        line(&std::format!(
            "take sample {row} {sequence} {timestamp} {len} {}",
            hex(&out[..len])
        ));
        assert_eq!(
            reply.item,
            Item::Sample {
                row: 1,
                sequence: 41,
                timestamp: -9,
                len: 4
            }
        );
        assert_eq!(&out[..4], &[7, 8, 9, 10]);
        let (request, slice) = take_request(&mut io);
        line(&std::format!("take request {}", hex(&request.encode())));
        line(&std::format!(
            "take slice {} {} {} {} {}",
            slice.offset,
            slice.length,
            slice.direction,
            slice.buffer,
            slice.lease
        ));
        assert_eq!(request.op, wire::OP_TAKE as u8);
        assert_eq!(slice.direction, io_queue::DIRECTION_DEVICE_WRITE);
        assert_eq!(slice.length, (HEADER_BYTES + 16) as u64);
    }

    fn a_take_returns_a_loss_and_an_empty_ring() {
        slime_rt::test_script(std::vec::Vec::new(), std::vec::Vec::new());
        let mut ring = [0u8; RING_BYTES];
        let mut data = [0u8; DATA_BYTES];
        let mut io = session(&mut ring, &mut data, 3);
        complete(
            &mut io,
            1,
            answer(
                wire::OP_TAKE,
                wire::STATUS_SUCCESS,
                wire::ITEM_LOSS,
                0,
                16,
                0,
            ),
            0,
        );
        complete(
            &mut io,
            2,
            answer(
                wire::OP_TAKE,
                wire::STATUS_SUCCESS,
                wire::ITEM_NONE,
                0,
                0,
                0,
            ),
            0,
        );
        let mut out = [0u8; 4];
        let loss = io.take(&mut out).unwrap();
        let none = io.take(&mut out).unwrap();
        let Item::Loss { count } = loss.item else {
            panic!("a loss")
        };
        line(&std::format!("take loss {count}"));
        line("take none");
        assert_eq!(loss.item, Item::Loss { count: 16 });
        assert_eq!(none.item, Item::None);
    }

    fn a_request_outside_the_protocol_is_refused_before_it_is_published() {
        slime_rt::test_script(std::vec::Vec::new(), std::vec::Vec::new());
        let mut ring = [0u8; RING_BYTES];
        let mut data = [0u8; DATA_BYTES];
        let mut io = session(&mut ring, &mut data, 3);
        let mut out = [0u8; MAX_PAYLOAD_BYTES + 1];
        let too_big = [0u8; MAX_PAYLOAD_BYTES + 1];
        let refusals = [
            ("row-past-the-holders-rows", io.publish(3, 1, &[1])),
            ("payload-over-the-data-page", io.publish(0, 1, &too_big)),
        ];
        for (name, result) in refusals {
            line(&std::format!("refused {name} {:?}", result.unwrap_err()));
            assert_eq!(result, Err(ClientError::BadRequest));
        }
        let result = io.take(&mut out);
        line(&std::format!(
            "refused take-buffer-over-the-data-page {:?}",
            result.unwrap_err()
        ));
        assert_eq!(result, Err(ClientError::BadRequest));
        // A buffer of exactly the largest payload is accepted by the adapter: it publishes the request, and only
        // then does the ring's own bound apply. The refusal above is therefore the adapter's, not the ring's.
        let mut exact = [0u8; MAX_PAYLOAD_BYTES];
        complete(
            &mut io,
            1,
            answer(
                wire::OP_TAKE,
                wire::STATUS_SUCCESS,
                wire::ITEM_NONE,
                0,
                0,
                0,
            ),
            0,
        );
        let accepted = io.take(&mut exact).unwrap();
        line(&std::format!("take largest-buffer {}", accepted.status));
        assert_eq!(accepted.item, Item::None);
        // Nothing was published by the refusals: only the one request for the largest buffer is in the ring.
        assert_eq!(io.queue.as_ref().unwrap().submitted(), 1);
        assert!(io.queue.is_some());
    }

    fn a_missing_completion_poisons_the_session_and_returns_both_loans() {
        slime_rt::test_script(std::vec::Vec::new(), std::vec::Vec::new());
        let mut ring = [0u8; RING_BYTES];
        let mut data = [0u8; DATA_BYTES];
        let mut io = session(&mut ring, &mut data, 3);
        slime_rt::test_script(std::vec::Vec::new(), std::vec::Vec::new());
        let result = io.publish(0, 1, &[1]);
        line(&std::format!(
            "lost publish {:?} {} {}",
            result.unwrap_err(),
            io.queue.is_none(),
            slime_rt::test_returns()
        ));
        assert_eq!(result, Err(ClientError::Lost));
        assert!(io.queue.is_none() && io.data.is_none());
        assert_eq!(slime_rt::test_returns(), 2);
        // A poisoned session refuses every later request without publishing it.
        assert_eq!(io.publish(0, 2, &[1]), Err(ClientError::Lost));
        assert_eq!(io.take(&mut [0u8; 4]), Err(ClientError::Lost));
        line("lost later-requests Lost");
    }

    fn an_answer_outside_the_protocol_poisons_the_session() {
        slime_rt::test_script(std::vec::Vec::new(), std::vec::Vec::new());
        // (name, the completion the service sends, the bytes it reports moved)
        let cases = [
            (
                "wrong-row",
                answer(
                    wire::OP_PUBLISH,
                    wire::STATUS_SUCCESS,
                    wire::ITEM_NONE,
                    1,
                    1,
                    0,
                ),
                1u64,
            ),
            (
                "wrong-operation",
                answer(
                    wire::OP_TAKE,
                    wire::STATUS_SUCCESS,
                    wire::ITEM_NONE,
                    0,
                    0,
                    0,
                ),
                1,
            ),
            (
                "an-item-on-a-publish",
                answer(
                    wire::OP_PUBLISH,
                    wire::STATUS_SUCCESS,
                    wire::ITEM_SAMPLE,
                    0,
                    1,
                    0,
                ),
                1,
            ),
            (
                "short-transfer",
                answer(
                    wire::OP_PUBLISH,
                    wire::STATUS_SUCCESS,
                    wire::ITEM_NONE,
                    0,
                    1,
                    0,
                ),
                0,
            ),
            (
                "an-unknown-flag",
                answer(
                    wire::OP_PUBLISH,
                    wire::STATUS_SUCCESS,
                    wire::ITEM_NONE,
                    0,
                    1,
                    2,
                ),
                1,
            ),
            (
                "a-status-below-the-set",
                answer(
                    wire::OP_PUBLISH,
                    wire::STATUS_LINK_CLOSED - 1,
                    wire::ITEM_NONE,
                    0,
                    0,
                    0,
                ),
                0,
            ),
            (
                "a-wrong-magic",
                WireServiceCompletion {
                    magic: MAGIC ^ 1,
                    ..answer(
                        wire::OP_PUBLISH,
                        wire::STATUS_SUCCESS,
                        wire::ITEM_NONE,
                        0,
                        1,
                        0,
                    )
                },
                1,
            ),
            (
                "a-wrong-version",
                WireServiceCompletion {
                    version: VERSION + 1,
                    ..answer(
                        wire::OP_PUBLISH,
                        wire::STATUS_SUCCESS,
                        wire::ITEM_NONE,
                        0,
                        1,
                        0,
                    )
                },
                1,
            ),
            (
                "a-nonzero-reserved",
                WireServiceCompletion {
                    reserved: [1, 0],
                    ..answer(
                        wire::OP_PUBLISH,
                        wire::STATUS_SUCCESS,
                        wire::ITEM_NONE,
                        0,
                        1,
                        0,
                    )
                },
                1,
            ),
            (
                "a-failure-carrying-a-count",
                answer(
                    wire::OP_PUBLISH,
                    wire::STATUS_DENIED,
                    wire::ITEM_NONE,
                    0,
                    3,
                    0,
                ),
                0,
            ),
        ];
        for (name, reply, transferred) in cases {
            let mut ring = [0u8; RING_BYTES];
            let mut data = [0u8; DATA_BYTES];
            let mut io = session(&mut ring, &mut data, 3);
            slime_rt::test_script(std::vec::Vec::new(), std::vec::Vec::new());
            complete(&mut io, 1, reply, transferred);
            let result = io.publish(0, 1, &[1]);
            line(&std::format!(
                "malformed {name} {:?} {} {}",
                result.unwrap_err(),
                io.queue.is_none(),
                slime_rt::test_returns()
            ));
            assert_eq!(result, Err(ClientError::Malformed), "{name}");
            assert!(io.queue.is_none(), "{name}");
            assert_eq!(slime_rt::test_returns(), 2, "{name}");
        }
    }

    fn a_take_answer_outside_the_protocol_poisons_the_session() {
        slime_rt::test_script(std::vec::Vec::new(), std::vec::Vec::new());
        let sample = |row: u16, length: u32| {
            answer(
                wire::OP_TAKE,
                wire::STATUS_SUCCESS,
                wire::ITEM_SAMPLE,
                row,
                length,
                0,
            )
        };
        // (name, completion, bytes moved)
        let cases = [
            ("a-sample-from-a-row-the-holder-lacks", sample(3, 4), 20u64),
            ("a-sample-longer-than-the-buffer", sample(0, 5), 21),
            (
                "a-consistent-sample-longer-than-the-buffer",
                sample(0, 5),
                (HEADER_BYTES + 5) as u64,
            ),
            ("a-sample-whose-transfer-does-not-match", sample(0, 4), 19),
            (
                "a-loss-of-zero",
                answer(
                    wire::OP_TAKE,
                    wire::STATUS_SUCCESS,
                    wire::ITEM_LOSS,
                    0,
                    0,
                    0,
                ),
                0,
            ),
            (
                "a-loss-that-moved-bytes",
                answer(
                    wire::OP_TAKE,
                    wire::STATUS_SUCCESS,
                    wire::ITEM_LOSS,
                    0,
                    2,
                    0,
                ),
                16,
            ),
            (
                "an-empty-ring-with-a-row",
                answer(
                    wire::OP_TAKE,
                    wire::STATUS_SUCCESS,
                    wire::ITEM_NONE,
                    1,
                    0,
                    0,
                ),
                0,
            ),
            (
                "a-take-failure-carrying-a-count",
                answer(wire::OP_TAKE, wire::STATUS_DENIED, wire::ITEM_NONE, 0, 3, 0),
                0,
            ),
            (
                "a-take-with-a-wrong-magic",
                WireServiceCompletion {
                    magic: MAGIC ^ 1,
                    ..answer(
                        wire::OP_TAKE,
                        wire::STATUS_SUCCESS,
                        wire::ITEM_NONE,
                        0,
                        0,
                        0,
                    )
                },
                0,
            ),
            (
                "a-take-with-a-wrong-version",
                WireServiceCompletion {
                    version: VERSION + 1,
                    ..answer(
                        wire::OP_TAKE,
                        wire::STATUS_SUCCESS,
                        wire::ITEM_NONE,
                        0,
                        0,
                        0,
                    )
                },
                0,
            ),
            (
                "a-flag-on-a-take",
                answer(
                    wire::OP_TAKE,
                    wire::STATUS_SUCCESS,
                    wire::ITEM_NONE,
                    0,
                    0,
                    1,
                ),
                0,
            ),
        ];
        for (name, reply, transferred) in cases {
            let mut ring = [0u8; RING_BYTES];
            let mut data = [0u8; DATA_BYTES];
            let mut io = session(&mut ring, &mut data, 3);
            slime_rt::test_script(std::vec::Vec::new(), std::vec::Vec::new());
            put_sample(&mut io, 1, 1, &[1, 2, 3, 4, 5]);
            complete(&mut io, 1, reply, transferred);
            let mut out = [0u8; 4];
            let result = io.take(&mut out);
            line(&std::format!(
                "malformed {name} {:?} {} {}",
                result.unwrap_err(),
                io.queue.is_none(),
                slime_rt::test_returns()
            ));
            assert_eq!(result, Err(ClientError::Malformed), "{name}");
            assert!(io.queue.is_none(), "{name}");
        }
    }

    fn a_service_refusal_is_a_status_and_leaves_the_session_usable() {
        slime_rt::test_script(std::vec::Vec::new(), std::vec::Vec::new());
        let mut ring = [0u8; RING_BYTES];
        let mut data = [0u8; DATA_BYTES];
        let mut io = session(&mut ring, &mut data, 3);
        complete(
            &mut io,
            1,
            answer(
                wire::OP_PUBLISH,
                wire::STATUS_DENIED,
                wire::ITEM_NONE,
                2,
                0,
                0,
            ),
            0,
        );
        complete(
            &mut io,
            2,
            answer(
                wire::OP_PUBLISH,
                wire::STATUS_LINK_CLOSED,
                wire::ITEM_NONE,
                2,
                0,
                0,
            ),
            0,
        );
        complete(
            &mut io,
            3,
            answer(
                wire::OP_PUBLISH,
                wire::STATUS_SUCCESS,
                wire::ITEM_NONE,
                2,
                0,
                0,
            ),
            1,
        );
        let denied = io.publish(2, 1, &[1]).unwrap();
        let closed = io.publish(2, 2, &[1]).unwrap();
        let sent = io.publish(2, 3, &[1]).unwrap();
        line(&std::format!(
            "status {} {} {}",
            denied.status,
            closed.status,
            sent.status
        ));
        assert_eq!((denied.status, closed.status, sent.status), (-1, -5, 0));
        assert!(io.queue.is_some());
        assert_eq!(slime_rt::test_returns(), 0);
    }

    fn attach_refuses_bad_endpoints_and_bases_before_it_sends_anything() {
        slime_rt::test_script(std::vec::Vec::new(), std::vec::Vec::new());
        slime_rt::test_script(std::vec::Vec::new(), std::vec::Vec::new());
        // (name, control endpoint, provisioning endpoint, ring base, data base)
        let cases: [(&str, u32, u32, u64, u64); 6] = [
            ("one-endpoint-for-both", 5, 5, 0x10_0000, 0x20_0000),
            ("an-unaligned-ring-base", 5, 6, 0x10_0001, 0x20_0000),
            ("an-unaligned-data-base", 5, 6, 0x10_0000, 0x20_0001),
            ("the-same-base-twice", 5, 6, 0x10_0000, 0x10_0000),
            ("a-zero-ring-base", 5, 6, 0, 0x20_0000),
            (
                "a-ring-base-that-wraps",
                5,
                6,
                u64::MAX - 4095 + 1,
                0x20_0000,
            ),
        ];
        for (name, control, provision, ring_base, data_base) in cases {
            // SAFETY: every case is refused before a mapping is built from the addresses.
            let result = unsafe { ClientIo::attach(control, provision, ring_base, data_base) };
            line(&std::format!(
                "attach-refused {name} {:?}",
                result.as_ref().err().unwrap()
            ));
            assert_eq!(result.err(), Some(ClientError::BadRequest), "{name}");
        }
        assert!(slime_rt::test_sent().is_empty());
        let _ = control_answers;
    }

    fn a_loan_is_valid_only_with_its_role_length_and_identities() {
        slime_rt::test_script(std::vec::Vec::new(), std::vec::Vec::new());
        let ring = |mutate: fn(&mut WireServiceLoan)| {
            let mut descriptor =
                WireServiceLoan::decode(&loan(wire::LOAN_ROLE_RING, 11, 21, RING_BYTES)).unwrap();
            mutate(&mut descriptor);
            valid_loan(&descriptor, wire::LOAN_ROLE_RING as u8)
        };
        assert!(ring(|_| {}));
        line("loan honest valid");
        let cases: [LoanCase; 8] = [
            ("wrong-magic", |d| d.magic ^= 1),
            ("wrong-version", |d| d.version = 2),
            ("wrong-role", |d| d.role = wire::LOAN_ROLE_DATA as u8),
            ("reserved0-set", |d| d.reserved0 = [1]),
            ("reserved-set", |d| d.reserved[31] = 1),
            ("wrong-length", |d| d.length = RING_BYTES as u64 - 1),
            ("zero-buffer", |d| d.buffer = 0),
            ("zero-lease", |d| d.lease = 0),
        ];
        for (name, mutate) in cases {
            assert!(!ring(mutate), "{name}");
            line(&std::format!("loan {name} refused"));
        }
        let data =
            WireServiceLoan::decode(&loan(wire::LOAN_ROLE_DATA, 12, 22, DATA_BYTES)).unwrap();
        assert!(valid_loan(&data, wire::LOAN_ROLE_DATA as u8));
        assert!(!valid_loan(&data, wire::LOAN_ROLE_RING as u8));
        assert!(!valid_loan(&data, 3));
        line("loan data-role checked");
    }

    fn teardown_returns_both_loans_then_waits_for_the_services_acknowledgement() {
        slime_rt::test_script(std::vec::Vec::new(), std::vec::Vec::new());
        for abort in [false, true] {
            let mut ring = [0u8; RING_BYTES];
            let mut data = [0u8; DATA_BYTES];
            let io = session(&mut ring, &mut data, 3);
            let op = if abort {
                wire::OP_ABORT
            } else {
                wire::OP_DETACH
            };
            slime_rt::test_script(control_answers(op, 0), std::vec::Vec::new());
            let result = if abort { io.abort() } else { io.finish() };
            let sent = slime_rt::test_sent();
            // Both loans are back before the request goes out, and the acknowledgement was read.
            let before_send = slime_rt::test_returns_at_send();
            line(&std::format!(
                "teardown {} {} returns {} before-send {:?} sent {} unread {}",
                if abort { "abort" } else { "detach" },
                if result.is_ok() { "ok" } else { "failed" },
                slime_rt::test_returns(),
                before_send,
                sent.len(),
                slime_rt::test_control_pending()
            ));
            assert_eq!(result, Ok(()));
            assert_eq!(slime_rt::test_returns(), 2);
            assert_eq!(before_send, std::vec![2]);
            assert_eq!(slime_rt::test_control_pending(), 0);
            assert_eq!(sent.len(), 1);
            let request = WireServiceRequest::decode(&sent[0]).unwrap();
            line(&std::format!("teardown request {}", hex(&request.encode())));
            assert_eq!(request.op, op as u8);
        }
    }

    fn a_teardown_the_service_does_not_acknowledge_is_lost() {
        slime_rt::test_script(std::vec::Vec::new(), std::vec::Vec::new());
        let mut ring = [0u8; RING_BYTES];
        let mut data = [0u8; DATA_BYTES];
        let io = session(&mut ring, &mut data, 3);
        let result = io.finish();
        line(&std::format!(
            "teardown unacknowledged {:?} returns {}",
            result.unwrap_err(),
            slime_rt::test_returns()
        ));
        assert_eq!(result, Err(ClientError::Lost));
        assert_eq!(slime_rt::test_returns(), 2);
        // An acknowledgement for another operation is not this teardown's.
        let mut ring = [0u8; RING_BYTES];
        let mut data = [0u8; DATA_BYTES];
        let io = session(&mut ring, &mut data, 3);
        slime_rt::test_script(control_answers(wire::OP_ABORT, 0), std::vec::Vec::new());
        let result = io.finish();
        line(&std::format!(
            "teardown wrong-acknowledgement {:?}",
            result.unwrap_err()
        ));
        assert_eq!(result, Err(ClientError::Malformed));
    }

    fn a_poisoned_session_finishes_with_an_error_and_aborts_cleanly() {
        slime_rt::test_script(std::vec::Vec::new(), std::vec::Vec::new());
        let mut ring = [0u8; RING_BYTES];
        let mut data = [0u8; DATA_BYTES];
        let mut io = session(&mut ring, &mut data, 3);
        slime_rt::test_script(control_answers(wire::OP_ABORT, 0), std::vec::Vec::new());
        io.poison();
        let before = slime_rt::test_returns();
        // A poisoned session returned its loans already: finish does not claim a normal detach.
        let mut ring2 = [0u8; RING_BYTES];
        let mut data2 = [0u8; DATA_BYTES];
        let mut again = session(&mut ring2, &mut data2, 3);
        again.poison();
        slime_rt::test_script(control_answers(wire::OP_DETACH, 0), std::vec::Vec::new());
        assert_eq!(again.finish(), Err(ClientError::Lost));
        // The poisoned session sent no detach: its loans were already back, and finish does not claim success.
        assert!(slime_rt::test_sent().is_empty());
        line("poisoned finish Lost");
        slime_rt::test_script(control_answers(wire::OP_ABORT, 0), std::vec::Vec::new());
        assert_eq!(io.abort(), Ok(()));
        assert_eq!(slime_rt::test_sent().len(), 1);
        line("poisoned abort ok");
        let _ = before;
    }

    /// A page the adapter may map, aligned as `attach` requires. The test formats the ring through the pointer, as
    /// the service does, and does not touch either page while a session over them is alive.
    #[repr(align(4096))]
    struct Page([u8; 4096]);

    fn page() -> *mut u8 {
        Box::into_raw(Box::new(Page([0; 4096]))).cast::<u8>()
    }

    fn formatted_ring() -> *mut u8 {
        let ring = page();
        // SAFETY: `ring` points to a live, aligned page that nothing else references.
        format(
            unsafe { core::slice::from_raw_parts_mut(ring, RING_BYTES) },
            QUEUE_SLOTS,
            7,
        )
        .unwrap();
        ring
    }

    fn release(page: *mut u8) {
        // SAFETY: `page` came from `page()` and no session over it is alive.
        drop(unsafe { Box::from_raw(page.cast::<Page>()) });
    }

    fn provisioning(ring: (u64, u64), data: (u64, u64)) -> std::vec::Vec<std::vec::Vec<u8>> {
        std::vec![
            loan(wire::LOAN_ROLE_RING, ring.0, ring.1, RING_BYTES),
            loan(wire::LOAN_ROLE_DATA, data.0, data.1, DATA_BYTES),
        ]
    }

    fn attach_takes_the_ring_then_the_data_page_and_reports_the_holders_rows() {
        slime_rt::test_script(std::vec::Vec::new(), std::vec::Vec::new());
        let (ring, data) = (formatted_ring(), page());
        slime_rt::test_script(
            control_answers(wire::OP_ATTACH, 3),
            provisioning((11, 21), (12, 22)),
        );
        // SAFETY: both pages are live, aligned, distinct and unreferenced until the session ends.
        let io = unsafe { ClientIo::attach(5, 6, ring as u64, data as u64) }.unwrap();
        line(&std::format!(
            "attach rows {} imports {}",
            io.rows(),
            slime_rt::test_imports()
        ));
        line(&std::format!(
            "attach request {}",
            hex(&slime_rt::test_sent()[0])
        ));
        assert_eq!((io.rows(), slime_rt::test_imports()), (3, 2));
        assert_eq!(
            (io.ring.descriptor.buffer, io.payload.descriptor.buffer),
            (11, 12)
        );
        slime_rt::test_script(control_answers(wire::OP_DETACH, 0), std::vec::Vec::new());
        assert_eq!(io.finish(), Ok(()));
        assert_eq!(slime_rt::test_returns(), 2);
        release(ring);
        release(data);
    }

    fn attach_refuses_what_the_service_may_not_provide() {
        slime_rt::test_script(std::vec::Vec::new(), std::vec::Vec::new());
        // (name, provisioning loans, control answer, the error)
        let refused_attach = answer(
            wire::OP_ATTACH,
            wire::STATUS_DENIED,
            wire::ITEM_NONE,
            0,
            0,
            0,
        )
        .encode()
        .to_vec();
        let cases: [AttachCase; 9] = [
            (
                "data-before-ring",
                std::vec![loan(wire::LOAN_ROLE_DATA, 12, 22, DATA_BYTES)],
                std::vec::Vec::new(),
                ClientError::Malformed,
            ),
            (
                "one-buffer-for-both",
                provisioning((11, 21), (11, 22)),
                control_answers(wire::OP_ATTACH, 3),
                ClientError::Malformed,
            ),
            (
                "one-lease-for-both",
                provisioning((11, 21), (12, 21)),
                control_answers(wire::OP_ATTACH, 3),
                ClientError::Malformed,
            ),
            (
                "no-rows",
                provisioning((11, 21), (12, 22)),
                control_answers(wire::OP_ATTACH, 0),
                ClientError::Malformed,
            ),
            (
                "more-rows-than-the-index-holds",
                provisioning((11, 21), (12, 22)),
                control_answers(wire::OP_ATTACH, wire::MAX_ROWS as u32 + 1),
                ClientError::Malformed,
            ),
            (
                "an-attach-the-service-refused",
                std::vec::Vec::new(),
                std::vec![refused_attach],
                ClientError::Setup,
            ),
            (
                "no-attach-answer",
                provisioning((11, 21), (12, 22)),
                std::vec::Vec::new(),
                ClientError::Lost,
            ),
            (
                "the-data-loan-never-arrives",
                std::vec![loan(wire::LOAN_ROLE_RING, 11, 21, RING_BYTES)],
                std::vec::Vec::new(),
                ClientError::Lost,
            ),
            (
                "no-loans-at-all",
                std::vec::Vec::new(),
                std::vec::Vec::new(),
                ClientError::Lost,
            ),
        ];
        for (name, loans, control, error) in cases {
            let (ring, data) = (formatted_ring(), page());
            slime_rt::test_script(control, loans);
            // SAFETY: both pages are live, aligned, distinct and unreferenced until the call returns.
            let result = unsafe { ClientIo::attach(5, 6, ring as u64, data as u64) };
            line(&std::format!(
                "attach-refused {name} {:?} returns {}",
                result.as_ref().err().unwrap(),
                slime_rt::test_returns()
            ));
            let failure = result.err();
            assert_eq!(failure, Some(error), "{name}");
            release(ring);
            release(data);
        }
    }

    fn attach_refuses_a_ring_the_service_did_not_format() {
        slime_rt::test_script(std::vec::Vec::new(), std::vec::Vec::new());
        let (ring, data) = (page(), page());
        slime_rt::test_script(
            control_answers(wire::OP_ATTACH, 3),
            provisioning((11, 21), (12, 22)),
        );
        // SAFETY: both pages are live, aligned, distinct and unreferenced until the call returns.
        let result = unsafe { ClientIo::attach(5, 6, ring as u64, data as u64) };
        line(&std::format!(
            "attach-refused unformatted-ring {:?} returns {}",
            result.as_ref().err().unwrap(),
            slime_rt::test_returns()
        ));
        assert_eq!(result.err(), Some(ClientError::Malformed));
        assert_eq!(slime_rt::test_returns(), 2);
        release(ring);
        release(data);
    }

    /// A failed attach ends the service's session when the service holds one. Each case prints what the adapter sent,
    /// when the loans went back and how many scripted answers were unread at each send.
    fn a_failed_attach_ends_the_service_session() {
        slime_rt::test_script(std::vec::Vec::new(), std::vec::Vec::new());
        let ack = |op| control_answers(op, 0).remove(0);
        let refused_attach = answer(
            wire::OP_ATTACH,
            wire::STATUS_DENIED,
            wire::ITEM_NONE,
            0,
            0,
            0,
        )
        .encode()
        .to_vec();
        let both = || provisioning((11, 21), (12, 22));
        // (name, loans, answers the service gives, ring formatted, the error)
        let cases: [AbortCase; 8] = [
            (
                "unformatted-ring",
                both(),
                std::vec![ack(wire::OP_ATTACH), ack(wire::OP_ABORT)],
                false,
                ClientError::Malformed,
            ),
            (
                "one-buffer-for-both",
                provisioning((11, 21), (11, 22)),
                std::vec![ack(wire::OP_ATTACH), ack(wire::OP_ABORT)],
                true,
                ClientError::Malformed,
            ),
            (
                "no-rows",
                both(),
                std::vec![
                    control_answers(wire::OP_ATTACH, 0).remove(0),
                    ack(wire::OP_ABORT)
                ],
                true,
                ClientError::Malformed,
            ),
            (
                "too-many-rows",
                both(),
                std::vec![
                    control_answers(wire::OP_ATTACH, wire::MAX_ROWS as u32 + 1).remove(0),
                    ack(wire::OP_ABORT)
                ],
                true,
                ClientError::Malformed,
            ),
            (
                "abort-not-acknowledged",
                both(),
                std::vec![ack(wire::OP_ATTACH)],
                false,
                ClientError::Malformed,
            ),
            (
                "abort-wrongly-acknowledged",
                both(),
                std::vec![ack(wire::OP_ATTACH), ack(wire::OP_DETACH)],
                false,
                ClientError::Malformed,
            ),
            (
                "no-attach-answer",
                both(),
                std::vec::Vec::new(),
                true,
                ClientError::Lost,
            ),
            (
                "attach-refused-by-the-service",
                std::vec::Vec::new(),
                std::vec![refused_attach],
                true,
                ClientError::Setup,
            ),
        ];
        let ops = |sent: &[std::vec::Vec<u8>]| {
            sent.iter()
                .map(
                    |bytes| match i64::from(WireServiceRequest::decode(bytes).unwrap().op) {
                        wire::OP_ATTACH => "attach",
                        wire::OP_ABORT => "abort",
                        _ => "other",
                    },
                )
                .collect::<std::vec::Vec<_>>()
                .join(",")
        };
        let csv = |values: std::vec::Vec<usize>| {
            values
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<std::vec::Vec<_>>()
                .join(",")
        };
        // Sorted, so the print does not fix the order of the two returns. A slot returned twice shows twice.
        let slots = |mut returned: std::vec::Vec<u32>| {
            returned.sort_unstable();
            if returned.is_empty() {
                "none".into()
            } else {
                returned
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect::<std::vec::Vec<_>>()
                    .join(",")
            }
        };
        for (name, loans, answers, formatted, error) in cases {
            let (ring, data) = (if formatted { formatted_ring() } else { page() }, page());
            slime_rt::test_script(answers, loans);
            // SAFETY: both pages are live, aligned, distinct and unreferenced until the call returns.
            let result = unsafe { ClientIo::attach(5, 6, ring as u64, data as u64) };
            let failure = result.err();
            assert_eq!(failure, Some(error), "{name}");
            let sent = slime_rt::test_sent();
            abort_line(&std::format!(
                "failed-attach {name} {:?} sent {} returns-at-send {} unread-at-send {} returns {} slots {} imports {} provision-unread {} unread {} abort-request {}",
                failure.unwrap(),
                ops(&sent),
                csv(slime_rt::test_returns_at_send()),
                csv(slime_rt::test_pending_at_send()),
                slime_rt::test_returns(),
                slots(slime_rt::test_returned_slots()),
                slime_rt::test_imports(),
                slime_rt::test_provision_pending(),
                slime_rt::test_control_pending(),
                sent.get(1)
                    .map_or_else(|| "none".into(), |bytes| hex(bytes))
            ));
            release(ring);
            release(data);
        }
    }

    #[test]
    fn the_abort_exam() {
        a_failed_attach_ends_the_service_session();
        abort_line("ok a-failed-attach-ends-the-service-session");
    }

    /// Every scenario in a fixed order, so the transcript the judge reads does not depend on test names.
    #[test]
    fn the_client_exam() {
        a_loan_is_valid_only_with_its_role_length_and_identities();
        attach_refuses_bad_endpoints_and_bases_before_it_sends_anything();
        attach_refuses_what_the_service_may_not_provide();
        attach_refuses_a_ring_the_service_did_not_format();
        attach_takes_the_ring_then_the_data_page_and_reports_the_holders_rows();
        a_publication_travels_as_a_request_and_its_payload_in_the_data_page();
        a_take_returns_a_sample_with_its_row_header_and_payload();
        a_take_returns_a_loss_and_an_empty_ring();
        a_request_outside_the_protocol_is_refused_before_it_is_published();
        a_service_refusal_is_a_status_and_leaves_the_session_usable();
        an_answer_outside_the_protocol_poisons_the_session();
        a_take_answer_outside_the_protocol_poisons_the_session();
        a_missing_completion_poisons_the_session_and_returns_both_loans();
        teardown_returns_both_loans_then_waits_for_the_services_acknowledgement();
        a_teardown_the_service_does_not_acknowledge_is_lost();
        a_poisoned_session_finishes_with_an_error_and_aborts_cleanly();
        line("ok every-request-is-bounded-and-every-answer-is-checked");
    }
}
