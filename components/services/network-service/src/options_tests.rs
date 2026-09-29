//! Host tests for declared per-row TCP options: what each open applies, what
//! the egress guard charges for keep-alive probes, and the typed timeout a
//! silent peer produces, against a real smoltcp peer on the other end.

extern crate std;

use super::tests::{HOLDER, Link, declarations, interface, pending, request};
use super::*;
use boot_contracts::network_application as app;
use std::vec;
use std::vec::Vec;

const PORT: u16 = 8080;
const TUNED: SocketOptions = SocketOptions {
    keepalive_ms: 500,
    idle_timeout_ms: 2000,
    hop_limit: 7,
    nagle: false,
};

/// One external client row for `HOLDER` declaring `options`.
fn applications(options: SocketOptions) -> Vec<u8> {
    let mut bytes = vec![0; app::HEADER_BYTES + app::ENTRY_BYTES];
    bytes[..app::MAGIC.len()].copy_from_slice(&app::MAGIC);
    for (offset, value) in [
        (app::OFF_HEADER_FORMAT_VERSION, app::FORMAT_VERSION),
        (app::OFF_HEADER_HEADER_SIZE, app::HEADER_BYTES as u32),
        (app::OFF_HEADER_APPLICATION_COUNT, 1),
        (app::OFF_HEADER_TOTAL_LEN, bytes.len() as u32),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    let row = &mut bytes[app::HEADER_BYTES..];
    row[app::OFF_ENTRY_HOLDER_IDENTITY..app::OFF_ENTRY_HOLDER_IDENTITY_END]
        .copy_from_slice(&HOLDER);
    row[app::OFF_ENTRY_CONTROL_BINDING..app::OFF_ENTRY_CONTROL_BINDING + 7]
        .copy_from_slice(b"control");
    row[app::OFF_ENTRY_PROVISION_BINDING..app::OFF_ENTRY_PROVISION_BINDING + 9]
        .copy_from_slice(b"provision");
    row[app::OFF_ENTRY_ROLE] = app::ROLE_CLIENT;
    row[app::OFF_ENTRY_BACKEND] = app::BACKEND_EXTERNAL;
    row[app::OFF_ENTRY_KEEPALIVE_MS..app::OFF_ENTRY_KEEPALIVE_MS_END]
        .copy_from_slice(&options.keepalive_ms.to_le_bytes());
    row[app::OFF_ENTRY_IDLE_TIMEOUT_MS..app::OFF_ENTRY_IDLE_TIMEOUT_MS_END]
        .copy_from_slice(&options.idle_timeout_ms.to_le_bytes());
    row[app::OFF_ENTRY_HOP_LIMIT] = options.hop_limit;
    row[app::OFF_ENTRY_NAGLE] = u8::from(options.nagle);
    bytes
}

/// The service's interface and a real smoltcp peer listening on `PORT`.
struct Net {
    local_link: Link,
    local_iface: Interface,
    peer_link: Link,
    peer_iface: Interface,
    peers: SocketSet<'static>,
    time: i64,
    /// Frames the service published, with the IPv4 time-to-live of each
    /// IPv4 frame.
    published: Vec<(Option<u8>, Vec<u8>)>,
    /// While set, nothing from the peer reaches the service.
    mute: bool,
}

impl Net {
    fn new() -> Self {
        let mut local_link = Link::default();
        let mut peer_link = Link::default();
        let local_iface = interface(&mut local_link, 1);
        let peer_iface = interface(&mut peer_link, 2);
        let storage: &'static mut [SocketStorage<'static>] =
            std::boxed::Box::leak(std::boxed::Box::new([SocketStorage::EMPTY]));
        let mut peers = SocketSet::new(storage);
        let mut socket = Socket::new(
            SocketBuffer::new(vec![0u8; 4096].leak()),
            SocketBuffer::new(vec![0u8; 4096].leak()),
        );
        socket.set_ack_delay(None);
        socket.listen(PORT).unwrap();
        peers.add(socket);
        Self {
            local_link,
            local_iface,
            peer_link,
            peer_iface,
            peers,
            time: 0,
            published: Vec::new(),
            mute: false,
        }
    }

    /// Advance `millis` of simulated time in 10 ms steps, exchanging frames.
    fn run(&mut self, engine: &mut Engine<'_>, millis: i64) {
        for _ in 0..millis / 10 {
            self.time += 10;
            let now = Instant::from_millis(self.time);
            engine.tick(now);
            self.local_iface
                .poll(now, &mut self.local_link, engine.sockets());
            while let Some(frame) = self.local_link.tx.pop_front() {
                if engine.permit_egress(&frame) {
                    let ttl = (frame[12..14] == [0x08, 0x00]).then(|| frame[14 + 8]);
                    self.published.push((ttl, frame.clone()));
                    self.peer_link.rx.push_back(frame);
                }
            }
            self.peer_iface
                .poll(now, &mut self.peer_link, &mut self.peers);
            let replies: Vec<_> = self.peer_link.tx.drain(..).collect();
            if !self.mute {
                self.local_link.rx.extend(replies);
            }
        }
    }

    fn now(&self) -> Instant {
        Instant::from_millis(self.time)
    }

    /// Published TCP segments: (flags byte, payload length, sequence number).
    fn segments(&self) -> Vec<(bool, bool, usize, u32)> {
        self.published
            .iter()
            .filter_map(|(_, frame)| {
                let ip = Ipv4Packet::new_checked(&frame[14..]).ok()?;
                (ip.next_header() == IpProtocol::Tcp).then_some(())?;
                let tcp = TcpPacket::new_checked(ip.payload()).ok()?;
                Some((
                    tcp.rst(),
                    tcp.payload() == [0],
                    tcp.payload().len(),
                    tcp.seq_number().0 as u32,
                ))
            })
            .collect()
    }
}

struct Session<'s> {
    destinations: NetworkDestinations<'s>,
    applications: NetworkApplications<'s>,
    context: Interface,
}

impl Session<'_> {
    fn handle(
        &mut self,
        engine: &mut Engine<'_>,
        request: WireNetworkRequest,
        payload: &mut [u8],
        now: Instant,
    ) -> Outcome {
        engine.handle_local(
            &self.destinations,
            &self.applications,
            HOLDER,
            request,
            payload,
            now,
            &mut self.context,
        )
    }
}

fn complete(outcome: Outcome) -> Completion {
    match outcome {
        Outcome::Complete(value) => value,
        other => panic!("{other:?}"),
    }
}

/// Connect `HOLDER` to the peer and wait for establishment.
fn established(engine: &mut Engine<'_>, net: &mut Net, session: &mut Session<'_>) -> u64 {
    let id = pending(session.handle(engine, request(wire::OP_CONNECT, 0), &mut [], net.now()));
    for _ in 0..50 {
        net.run(engine, 10);
        if let Some(done) = engine.poll_pending(HOLDER, id, net.now()) {
            assert_eq!(done.status, Status::Success);
            return id;
        }
    }
    panic!("connect never completed");
}

fn engine_parts() -> (
    [SocketStorage<'static>; SOCKETS],
    [[u8; BUFFER_BYTES]; SOCKETS],
    [[u8; BUFFER_BYTES]; SOCKETS],
) {
    (
        [SocketStorage::EMPTY; SOCKETS],
        [[0; BUFFER_BYTES]; SOCKETS],
        [[0; BUFFER_BYTES]; SOCKETS],
    )
}

#[test]
fn every_open_applies_the_declared_row_and_reno_and_the_wire_carries_its_hop_limit() {
    let (mut storage, mut rx, mut tx) = engine_parts();
    let mut engine = Engine::new(&mut storage, &mut rx, &mut tx, 21, 0).unwrap();
    assert_eq!(engine.congestion_control(), b"reno");
    let destination_bytes = declarations(RIGHT_CONNECT | RIGHT_SEND | RIGHT_RECV, 4096, 1, 1);
    let application_bytes = applications(TUNED);
    let mut net = Net::new();
    let mut context_link = Link::default();
    let mut session = Session {
        destinations: NetworkDestinations::decode(&destination_bytes).unwrap(),
        applications: NetworkApplications::decode(&application_bytes).unwrap(),
        context: interface(&mut context_link, 1),
    };
    let id = established(&mut engine, &mut net, &mut session);
    let index = engine.find(HOLDER, id).unwrap();
    let socket = engine.sockets.get::<Socket>(engine.handles[index]);
    assert!(!socket.nagle_enabled());
    assert_eq!(socket.hop_limit(), Some(7));
    assert_eq!(socket.keep_alive(), Some(Duration::from_millis(500)));
    assert_eq!(socket.timeout(), Some(Duration::from_millis(2000)));
    assert_eq!(socket.ack_delay(), None);
    assert_eq!(socket.congestion_control(), CongestionControl::Reno);
    assert!(net.published.iter().any(|(ttl, _)| ttl.is_some()));
    assert!(
        net.published
            .iter()
            .all(|(ttl, _)| ttl.is_none_or(|ttl| ttl == 7))
    );

    // A holder without a declared row keeps the service's defaults, which a
    // pooled socket regains even after a tuned connection used it.
    pending(session.handle(&mut engine, request(wire::OP_CLOSE, id), &mut [], net.now()));
    let (mut storage, mut rx, mut tx) = engine_parts();
    let mut plain = Engine::new(&mut storage, &mut rx, &mut tx, 22, 0).unwrap();
    let mut net = Net::new();
    let id = pending(plain.handle(
        &session.destinations,
        HOLDER,
        request(wire::OP_CONNECT, 0),
        &mut [],
        net.now(),
        &mut session.context,
    ));
    let index = plain.find(HOLDER, id).unwrap();
    let socket = plain.sockets.get::<Socket>(plain.handles[index]);
    assert!(socket.nagle_enabled());
    assert_eq!(socket.hop_limit(), Some(64));
    assert_eq!(socket.keep_alive(), None);
    assert_eq!(socket.timeout(), Some(OPERATION_TIMEOUT));
    net.run(&mut plain, 50);
    assert!(net.published.iter().any(|(ttl, _)| ttl.is_some()));
    assert!(
        net.published
            .iter()
            .all(|(ttl, _)| ttl.is_none_or(|ttl| ttl == 64))
    );
}

#[test]
fn keep_alive_probes_are_never_charged_as_retries_and_hold_an_answered_connection() {
    let (mut storage, mut rx, mut tx) = engine_parts();
    let mut engine = Engine::new(&mut storage, &mut rx, &mut tx, 23, 0).unwrap();
    // The destination's retry limit is two; six probes must not exhaust it.
    let destination_bytes = declarations(RIGHT_CONNECT | RIGHT_SEND | RIGHT_RECV, 4096, 1, 1);
    let application_bytes = applications(TUNED);
    let mut net = Net::new();
    let mut context_link = Link::default();
    let mut session = Session {
        destinations: NetworkDestinations::decode(&destination_bytes).unwrap(),
        applications: NetworkApplications::decode(&application_bytes).unwrap(),
        context: interface(&mut context_link, 1),
    };
    let id = established(&mut engine, &mut net, &mut session);
    let before = net.segments().len();
    net.run(&mut engine, 3000);
    let probes = net.segments()[before..]
        .iter()
        .filter(|(_, probe, length, _)| *probe && *length == 1)
        .count();
    assert!(
        (5..=7).contains(&probes),
        "{probes} probes in three seconds"
    );
    let index = engine.find(HOLDER, id).unwrap();
    let connection = engine.connections[index].unwrap();
    assert_eq!(connection.retries, 0);
    assert_eq!(connection.terminal, None);
    let mut buffer = [0u8; 8];
    assert_eq!(
        complete(session.handle(
            &mut engine,
            request(wire::OP_RECV, id),
            &mut buffer,
            net.now()
        ))
        .status,
        Status::WouldBlock
    );
}

#[test]
fn a_silent_peer_ends_the_connection_with_one_reset_and_a_typed_timeout() {
    for (options, data) in [
        (
            SocketOptions {
                keepalive_ms: 0,
                idle_timeout_ms: 4000,
                hop_limit: 64,
                nagle: true,
            },
            true,
        ),
        (TUNED, false),
    ] {
        let (mut storage, mut rx, mut tx) = engine_parts();
        let mut engine = Engine::new(&mut storage, &mut rx, &mut tx, 24, 0).unwrap();
        let destination_bytes = declarations(RIGHT_CONNECT | RIGHT_SEND | RIGHT_RECV, 4096, 1, 1);
        let application_bytes = applications(options);
        let mut net = Net::new();
        let mut context_link = Link::default();
        let mut session = Session {
            destinations: NetworkDestinations::decode(&destination_bytes).unwrap(),
            applications: NetworkApplications::decode(&application_bytes).unwrap(),
            context: interface(&mut context_link, 1),
        };
        let id = established(&mut engine, &mut net, &mut session);
        net.mute = true;
        let silent_at = net.time;
        if data {
            let mut payload = [7u8; 256];
            assert_eq!(
                complete(session.handle(
                    &mut engine,
                    request(wire::OP_SEND, id),
                    &mut payload,
                    net.now()
                ))
                .transferred,
                256
            );
        }
        let before = net.segments().len();
        let mut buffer = [0u8; 8];
        let mut ended = None;
        for _ in 0..800 {
            net.run(&mut engine, 10);
            let status = complete(session.handle(
                &mut engine,
                request(wire::OP_RECV, id),
                &mut buffer,
                net.now(),
            ))
            .status;
            if status != Status::WouldBlock {
                assert_eq!(status, Status::Timeout);
                ended = Some(net.time - silent_at);
                break;
            }
        }
        let waited = ended.expect("the silent connection never timed out");
        let idle = i64::from(options.idle_timeout_ms);
        assert!(
            (idle - 50..=idle + 50).contains(&waited),
            "timed out after {waited} ms, declared {idle} ms"
        );
        let after = &net.segments()[before..];
        assert_eq!(after.iter().filter(|(rst, ..)| *rst).count(), 1);
        assert!(after.last().unwrap().0, "the reset is the last segment");
        let probes = after
            .iter()
            .filter(|(_, probe, length, _)| *probe && *length == 1)
            .count();
        if data {
            assert_eq!(probes, 0);
            assert!(
                after
                    .iter()
                    .filter(|(_, _, length, _)| *length == 256)
                    .count()
                    >= 2
            );
        } else {
            assert!(
                (2..=4).contains(&probes),
                "{probes} probes before the timeout"
            );
        }
        // Close reports the same typed timeout and returns the whole socket.
        let close = session.handle(&mut engine, request(wire::OP_CLOSE, id), &mut [], net.now());
        let close = match close {
            Outcome::Pending { capability } => {
                engine.poll_pending(HOLDER, capability, net.now()).unwrap()
            }
            Outcome::Complete(value) => value,
        };
        assert_eq!(close.status, Status::Timeout);
        assert_eq!(
            engine.take_timeout_reclamation(),
            Some(Reclaimed {
                handles: 1,
                sockets: 1,
                bytes: 2 * BUFFER_BYTES,
            })
        );
        assert_eq!(engine.allocated(), 0);
        net.run(&mut engine, 100);
        assert_eq!(
            net.segments()[before..]
                .iter()
                .filter(|(rst, ..)| *rst)
                .count(),
            1
        );
    }
}
