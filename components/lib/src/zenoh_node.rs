//! Shared pieces of the two demo nodes: the service connection as a [`ByteLink`],
//! the reply-to-error mapping, and console lines built in one buffer.
//!
//! The nodes run concurrently on one console, and every `debug_write` is a
//! separate message, so a line assembled from several writes can be interleaved
//! with the other node's. [`Line`] collects a whole line first and is emitted by
//! one write.

use crate::zenoh_link::{ByteLink, LinkError};
use slime_proto::network_service as net;

#[cfg(test)]
#[path = "zenoh_node/tests.rs"]
mod tests;

/// The demo's data key expression, `<domain>/<topic>/<type on the wire>/<type hash>`.
pub const KEY: &str = "0/slime_demo/counter/slime_demo_msgs::msg::dds_::Counter_/RIHS01_a82fd5ffcb96d0a197a5ad3680d1c4e6ba43a962928ecd592fb565eb8129595b";
/// The four samples the demo publishes: `(sequence, value)`.
pub const SAMPLES: [(u32, i32); 4] = [(0, 10), (1, 20), (2, 30), (3, 40)];
/// The one endpoint the generation declares for the session.
pub const ENDPOINT: [u8; 4] = [127, 0, 0, 1];
pub const PORT: u16 = 7447;
/// Zenoh's multicast scouting group. Profile 0 runs no scouting, so no destination row names it.
pub const SCOUT_GROUP: [u8; 4] = [224, 0, 0, 224];
pub const SCOUT_PORT: u16 = 7446;
pub const LEASE_MS: u64 = 2000;

/// The result of one service operation, reduced to what a byte stream needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reply {
    pub success: bool,
    pub status: i32,
    pub transferred: u64,
    pub end_of_stream: bool,
}

/// Maps a service reply to the number of bytes moved.
///
/// A success moves at most what was offered; a would-block moves nothing and is not an error;
/// a reset or any other status ends the stream.
pub fn step(reply: &Reply, offered: usize) -> Result<usize, LinkError> {
    if reply.success {
        let moved = usize::try_from(reply.transferred).map_err(|_| LinkError::Reset)?;
        return if moved <= offered {
            Ok(moved)
        } else {
            Err(LinkError::Reset)
        };
    }
    match reply.status {
        net::STATUS_WOULD_BLOCK => Ok(0),
        net::STATUS_RESET => Err(LinkError::Reset),
        _ => Err(LinkError::Closed),
    }
}

/// The two non-blocking operations a node performs on its one connection.
pub trait Connection {
    fn send(&mut self, bytes: &[u8]) -> Result<Reply, LinkError>;
    fn recv(&mut self, bytes: &mut [u8]) -> Result<Reply, LinkError>;
}

/// A [`ByteLink`] over one connection; every call is non-blocking, so the link owns backpressure.
pub struct Stream<C: Connection> {
    connection: C,
    eof: bool,
}

impl<C: Connection> Stream<C> {
    pub const fn new(connection: C) -> Self {
        Self {
            connection,
            eof: false,
        }
    }

    pub fn into_inner(self) -> C {
        self.connection
    }
}

impl<C: Connection> ByteLink for Stream<C> {
    fn send(&mut self, buf: &[u8]) -> Result<usize, LinkError> {
        let reply = self.connection.send(buf)?;
        step(&reply, buf.len())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, LinkError> {
        let offered = buf.len();
        let reply = self.connection.recv(buf)?;
        if reply.success && reply.transferred == 0 && reply.end_of_stream {
            self.eof = true;
            return Ok(0);
        }
        step(&reply, offered)
    }

    fn eof(&self) -> bool {
        self.eof
    }
}

/// What a request the generation did not grant is expected to draw.
pub fn is_denial(reply: &Reply) -> bool {
    !reply.success && reply.status == net::STATUS_DENIED && reply.transferred == 0
}

/// One console line, collected whole so it is emitted by a single write.
pub struct Line<const N: usize> {
    bytes: [u8; N],
    len: usize,
    clipped: bool,
}

impl<const N: usize> Line<N> {
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
            clipped: false,
        }
    }

    /// Appends `part`; a part that does not fit is refused whole and the line is marked clipped.
    pub fn put(&mut self, part: &[u8]) -> &mut Self {
        let end = self.len.saturating_add(part.len());
        match self.bytes.get_mut(self.len..end) {
            Some(room) => {
                room.copy_from_slice(part);
                self.len = end;
            }
            None => self.clipped = true,
        }
        self
    }

    pub fn put_number(&mut self, value: u64) -> &mut Self {
        let mut digits = [0u8; 20];
        let mut start = digits.len();
        let mut rest = value;
        loop {
            start -= 1;
            digits[start] = b'0' + (rest % 10) as u8;
            rest /= 10;
            if rest == 0 {
                break;
            }
        }
        self.put(digits.get(start..).unwrap_or(&[]))
    }

    pub fn put_signed(&mut self, value: i64) -> &mut Self {
        if value < 0 {
            self.put(b"-");
        }
        self.put_number(value.unsigned_abs())
    }

    pub fn put_hex(&mut self, bytes: &[u8]) -> &mut Self {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        for byte in bytes {
            self.put(&[
                DIGITS[usize::from(byte >> 4)],
                DIGITS[usize::from(byte & 0x0f)],
            ]);
        }
        self
    }

    /// The finished line. Empty when any part did not fit, so a clipped line is never emitted.
    pub fn finish(&self) -> &[u8] {
        if self.clipped {
            return &[];
        }
        self.bytes.get(..self.len).unwrap_or(&[])
    }

    pub const fn clipped(&self) -> bool {
        self.clipped
    }
}

impl<const N: usize> Default for Line<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// `[<who>] denial class=<class> refused=1\n`
pub fn denial_line(who: &[u8], class: &[u8]) -> Line<96> {
    let mut line = Line::new();
    line.put(who)
        .put(b"denial class=")
        .put(class)
        .put(b" refused=1\n");
    line
}

/// `[<who>] <what> sample=<index> hex=<batch>\n`. The demo's frames are 184 bytes, so 368 hex digits;
/// a longer batch clips the line, and a clipped line is never emitted.
pub fn wire_line(who: &[u8], what: &[u8], index: u64, batch: &[u8]) -> Line<640> {
    let mut line = Line::new();
    line.put(who)
        .put(what)
        .put(b" sample=")
        .put_number(index)
        .put(b" hex=")
        .put_hex(batch)
        .put(b"\n");
    line
}

/// `[<who>] <prefix><key>\n`
pub fn key_line(who: &[u8], prefix: &[u8]) -> Line<320> {
    let mut line = Line::new();
    line.put(who).put(prefix).put(KEY.as_bytes()).put(b"\n");
    line
}

/// `[<who>] sample validated sequence=<n> value=<v>\n`
pub fn validated_line(who: &[u8], sequence: u32, value: i32) -> Line<128> {
    let mut line = Line::new();
    line.put(who)
        .put(b"sample validated sequence=")
        .put_number(u64::from(sequence))
        .put(b" value=")
        .put_signed(i64::from(value))
        .put(b"\n");
    line
}

/// A received sample checked against the declared one; `Err` names what differed.
pub fn check_sample(index: usize, payload: &[u8]) -> Result<(u32, i32), &'static [u8]> {
    let Some(&(sequence, value)) = SAMPLES.get(index) else {
        return Err(b"unexpected sample");
    };
    let counter = crate::ros_cdr::Counter::decode(payload).map_err(|_| b"cdr decode".as_slice())?;
    if counter.sequence != sequence || counter.value != value {
        return Err(b"sample value");
    }
    Ok((sequence, value))
}

/// The UDP connect a node would send to join Zenoh's multicast scouting group.
///
/// Profile 0 runs no scouting and no destination row names the group, so the service must refuse
/// it. The request is well formed on purpose: a refusal of a malformed request would prove
/// nothing about authority.
pub fn scouting_request() -> net::WireNetworkRequest {
    let mut endpoint = [0u8; 24];
    endpoint[..4].copy_from_slice(&SCOUT_GROUP);
    net::WireNetworkRequest {
        magic: net::NETWORK_MAGIC,
        version: net::FORMAT_VERSION,
        op: net::OP_CONNECT,
        transport: net::TRANSPORT_UDP,
        flags: 0,
        port: SCOUT_PORT,
        name_len: 0,
        capability: 0,
        address_kind: net::ADDRESS_IPV4,
        reserved: [0; 7],
        endpoint,
    }
}

pub const PUBLISHER: &[u8] = b"[ros2-demo-publisher] ";
pub const SUBSCRIBER: &[u8] = b"[ros2-demo-subscriber] ";
/// The subscriber's one-line account of what it received.
pub const SUMMARY: &[u8] =
    b"[rpi5-ros2-demo] received count=4 sequences=0,1,2,3 values=10,20,30,40\n";
pub const SUCCESS: &[u8] = b"[rpi5-ros2-demo] success profile=rpi5-ros2-demo-v2 samples=4\n";

/// What a node needs from its component: a clock, a way to yield, and the console.
pub trait Host {
    fn now_ms(&mut self) -> u64;
    /// Yields once; `false` when the node's deadline has passed.
    fn idle(&mut self) -> bool;
    /// One console message, which is exactly one line.
    fn write(&mut self, line: &[u8]);
}

/// Why a node stopped before it finished its part of the exchange.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeError {
    Link(crate::zenoh_link::Error),
    Deadline,
    /// The peer closed before every sample was exchanged.
    PeerClosedEarly,
    /// A line did not fit its buffer, so it was not emitted.
    LineClipped,
    /// A received sample differed from the declared one.
    Sample(&'static [u8]),
    /// The session machine or link refused an operation the node needed.
    Operation(&'static [u8]),
}

impl From<crate::zenoh_link::Error> for NodeError {
    fn from(error: crate::zenoh_link::Error) -> Self {
        Self::Link(error)
    }
}

fn emit<H: Host, const N: usize>(host: &mut H, line: &Line<N>) -> Result<(), NodeError> {
    if line.clipped() {
        return Err(NodeError::LineClipped);
    }
    host.write(line.finish());
    Ok(())
}

fn say<H: Host>(host: &mut H, who: &[u8], message: &[u8]) -> Result<(), NodeError> {
    let mut line = Line::<160>::new();
    line.put(who).put(message);
    emit(host, &line)
}

/// The announced session parameters, as the nodes print them.
fn open_line(who: &[u8], role: &[u8]) -> Line<160> {
    let mut line = Line::new();
    line.put(who)
        .put(b"session open role=")
        .put(role)
        .put(b" initial_sn=0 lease_ms=")
        .put_number(LEASE_MS)
        .put(b"\n");
    line
}

/// What one [`Publisher::step`] or [`Subscriber::step`] concluded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The node has more to do; the caller yields and steps again.
    Continue,
    /// The node finished its part, having exchanged this many samples.
    Done(usize),
}

/// The publisher's part of the exchange.
///
/// It opens the session, waits for the subscriber's declaration of the demo key, publishes the four
/// samples one frame at a time, and finishes once the subscriber has closed the session. The caller
/// yields between steps, so two nodes sharing a CPU alternate.
pub struct Publisher {
    next: usize,
    declared: bool,
    opened: bool,
    started: bool,
}

impl Publisher {
    pub const fn new() -> Self {
        Self {
            next: 0,
            declared: false,
            opened: false,
            started: false,
        }
    }

    pub fn step<L: ByteLink, H: Host>(
        &mut self,
        link: &mut crate::zenoh_link::Link<L>,
        host: &mut H,
    ) -> Result<Step, NodeError> {
        use crate::zenoh_link::Notice;
        let handle = link.handle();
        let now = host.now_ms();
        if !self.started {
            link.open(handle, now)?;
            self.started = true;
        }
        link.pump(handle, now)?;
        while let Some(notice) = link.next_notice() {
            match notice {
                Notice::HandshakeComplete if !self.opened => {
                    self.opened = true;
                    emit(host, &open_line(PUBLISHER, b"connector"))?;
                }
                Notice::SubscriberDeclared { .. } if !self.declared => {
                    self.declared = true;
                    emit(host, &key_line(PUBLISHER, b"declaration matched key="))?;
                }
                Notice::Closed { .. } => {
                    return if self.next == SAMPLES.len() {
                        Ok(Step::Done(self.next))
                    } else {
                        Err(NodeError::PeerClosedEarly)
                    };
                }
                _ => {}
            }
        }
        if self.declared && self.next < SAMPLES.len() && !link.send_pending() {
            let Some(&(sequence, value)) = SAMPLES.get(self.next) else {
                return Err(NodeError::Operation(b"sample table"));
            };
            let mut cdr = [0u8; crate::ros_cdr::MAX_SERIALIZED_BYTES];
            let length = crate::ros_cdr::Counter { sequence, value }
                .encode(&mut cdr)
                .map_err(|_| NodeError::Operation(b"cdr encode"))?;
            let attachment = crate::zenoh_profile0::encode::attachment(
                i64::from(sequence) + 1,
                1000 + i64::from(sequence),
                &GID,
            )
            .map_err(|_| NodeError::Operation(b"attachment"))?;
            link.publish(handle, KEY, &attachment, cdr.get(..length).unwrap_or(&[]))?;
            emit(
                host,
                &wire_line(
                    PUBLISHER,
                    b"wire sent",
                    self.next as u64,
                    link.last_sent_batch(),
                ),
            )?;
            self.next += 1;
        }
        if self.next == SAMPLES.len() && !link.send_pending() && link.is_closed() {
            return Ok(Step::Done(self.next));
        }
        Ok(Step::Continue)
    }
}

impl Default for Publisher {
    fn default() -> Self {
        Self::new()
    }
}

/// The subscriber's part of the exchange.
///
/// It declares the demo key as soon as the session is open, validates each sample against the
/// declared table, and tears the session down by undeclaring before it closes.
pub struct Subscriber {
    received: usize,
    declared: bool,
    undeclared: bool,
    closing: bool,
}

impl Subscriber {
    pub const fn new() -> Self {
        Self {
            received: 0,
            declared: false,
            undeclared: false,
            closing: false,
        }
    }

    pub fn step<L: ByteLink, H: Host>(
        &mut self,
        link: &mut crate::zenoh_link::Link<L>,
        host: &mut H,
    ) -> Result<Step, NodeError> {
        use crate::zenoh_link::Notice;
        let handle = link.handle();
        let now = host.now_ms();
        link.pump(handle, now)?;
        while let Some(notice) = link.next_notice() {
            match notice {
                Notice::HandshakeComplete => emit(host, &open_line(SUBSCRIBER, b"listener"))?,
                Notice::SampleDelivered(sample) => {
                    emit(
                        host,
                        &wire_line(
                            SUBSCRIBER,
                            b"wire received",
                            self.received as u64,
                            link.last_received_batch(),
                        ),
                    )?;
                    let (sequence, value) =
                        check_sample(self.received, sample.payload()).map_err(NodeError::Sample)?;
                    emit(host, &validated_line(SUBSCRIBER, sequence, value))?;
                    self.received += 1;
                }
                Notice::SubscriberUndeclared { .. } => self.undeclared = true,
                Notice::Closed { .. } if self.closing => {}
                Notice::Closed { .. } => {
                    return if self.received == SAMPLES.len() {
                        Ok(Step::Done(self.received))
                    } else {
                        Err(NodeError::PeerClosedEarly)
                    };
                }
                Notice::SubscriberDeclared { .. } => {}
            }
        }
        if link.is_open() && !self.declared && !link.send_pending() {
            link.declare_subscriber(handle, 1, KEY)?;
            self.declared = true;
            emit(
                host,
                &key_line(SUBSCRIBER, b"declared subscriber id=1 key="),
            )?;
        }
        if self.received == SAMPLES.len()
            && self.declared
            && !self.undeclared
            && !link.send_pending()
        {
            link.undeclare_subscriber(handle, 1, KEY)?;
            self.undeclared = true;
        }
        if self.undeclared && !self.closing && !link.send_pending() {
            host.write(SUMMARY);
            say(host, SUBSCRIBER, b"undeclared subscriber id=1\n")?;
            say(host, SUBSCRIBER, b"session closing samples=4\n")?;
            link.close(handle, 0)?;
            self.closing = true;
        }
        if self.closing && !link.send_pending() {
            return Ok(Step::Done(self.received));
        }
        Ok(Step::Continue)
    }
}

impl Default for Subscriber {
    fn default() -> Self {
        Self::new()
    }
}

/// The source identifier the connector announces, `0xa1` repeated.
pub const PUBLISHER_ZID: [u8; 8] = [0xa1; 8];
pub const SUBSCRIBER_ZID: [u8; 8] = [0xb2; 8];
/// The cookie the listener issues in INIT_ACK and expects echoed in OPEN_SYN.
pub const COOKIE: &[u8] = b"slime-zenoh-profile-0";
/// The attachment's source GID, the same for every sample.
pub const GID: [u8; 16] = [
    0xe0, 0xe1, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xeb, 0xec, 0xed, 0xee, 0xef,
];

/// The connector's session parameters.
pub fn publisher_config() -> crate::zenoh_profile0::session::Config<'static> {
    crate::zenoh_profile0::session::Config {
        zid: &PUBLISHER_ZID,
        lease_ms: LEASE_MS,
        initial_sn: 0,
        cookie: b"",
    }
}

/// The listener's session parameters.
pub fn subscriber_config() -> crate::zenoh_profile0::session::Config<'static> {
    crate::zenoh_profile0::session::Config {
        zid: &SUBSCRIBER_ZID,
        lease_ms: LEASE_MS,
        initial_sn: 0,
        cookie: COOKIE,
    }
}
