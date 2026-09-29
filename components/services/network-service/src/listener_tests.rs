//! Host tests for the external listener, its ingress and egress policy, and
//! close semantics, against a real smoltcp peer on the other end of the link.

extern crate std;

use super::tests::{Link, declarations, interface, pending, request};
use super::*;
use boot_contracts::network_application as app;
use std::vec;
use std::vec::Vec;

const LISTENER: [u8; 32] = [2; 32];
const CLIENT: [u8; 32] = [3; 32];
const LOCAL: [u8; 4] = [10, 0, 0, 1];
const ADMITTED: [u8; 4] = [10, 0, 0, 2];
const PORT: u16 = 4270;

fn complete(outcome: Outcome) -> Completion {
    match outcome {
        Outcome::Complete(value) => value,
        other => panic!("{other:?}"),
    }
}

/// One external listener for `LISTENER` and one external client row.
fn applications() -> Vec<u8> {
    let mut bytes = vec![0; app::HEADER_BYTES + 2 * app::ENTRY_BYTES];
    bytes[..app::MAGIC.len()].copy_from_slice(&app::MAGIC);
    for (offset, value) in [
        (app::OFF_HEADER_FORMAT_VERSION, app::FORMAT_VERSION),
        (app::OFF_HEADER_HEADER_SIZE, app::HEADER_BYTES as u32),
        (app::OFF_HEADER_APPLICATION_COUNT, 2),
        (app::OFF_HEADER_TOTAL_LEN, bytes.len() as u32),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for (index, holder) in [LISTENER, CLIENT].iter().enumerate() {
        let start = app::HEADER_BYTES + index * app::ENTRY_BYTES;
        let row = &mut bytes[start..start + app::ENTRY_BYTES];
        row[app::OFF_ENTRY_HOLDER_IDENTITY..app::OFF_ENTRY_HOLDER_IDENTITY_END]
            .copy_from_slice(holder);
        row[app::OFF_ENTRY_CONTROL_BINDING..app::OFF_ENTRY_CONTROL_BINDING + 7]
            .copy_from_slice(b"control");
        row[app::OFF_ENTRY_PROVISION_BINDING..app::OFF_ENTRY_PROVISION_BINDING + 9]
            .copy_from_slice(b"provision");
        row[app::OFF_ENTRY_BACKEND] = app::BACKEND_EXTERNAL;
        row[app::OFF_ENTRY_ROLE] = app::ROLE_CLIENT;
        if *holder == LISTENER {
            row[app::OFF_ENTRY_ROLE] = app::ROLE_LISTENER;
            row[app::OFF_ENTRY_PEER_KIND] = app::PEER_IPV4;
            row[app::OFF_ENTRY_LOCAL_IPV4..app::OFF_ENTRY_LOCAL_IPV4_END].copy_from_slice(&LOCAL);
            row[app::OFF_ENTRY_ADMITTED_PEER_IPV4..app::OFF_ENTRY_ADMITTED_PEER_IPV4_END]
                .copy_from_slice(&ADMITTED);
            row[app::OFF_ENTRY_LOCAL_PORT..app::OFF_ENTRY_LOCAL_PORT + 2]
                .copy_from_slice(&PORT.to_le_bytes());
            row[app::OFF_ENTRY_RIGHTS..app::OFF_ENTRY_RIGHTS + 2].copy_from_slice(
                &(app::RIGHT_LISTEN | app::RIGHT_SEND | app::RIGHT_RECV).to_le_bytes(),
            );
            for (offset, value) in [
                (app::OFF_ENTRY_BACKLOG, 1u32),
                (app::OFF_ENTRY_ACCEPTED_SOCKET_LIMIT, 2),
                (app::OFF_ENTRY_BYTE_BUDGET, 8192),
                (app::OFF_ENTRY_TIMER_BUDGET, 2),
                (app::OFF_ENTRY_QUEUE_DEPTH, 2),
                (app::OFF_ENTRY_RETRY_LIMIT, 8),
            ] {
                row[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            }
        }
    }
    bytes
}

fn listen_request(address: [u8; 4], port: u16) -> WireNetworkRequest {
    let mut value = request(wire::OP_CONNECT, 0);
    value.op = wire::OP_LISTEN;
    value.endpoint[..4].copy_from_slice(&address);
    value.port = port;
    value
}

fn capability_request(op: u8, capability: u64) -> WireNetworkRequest {
    let mut value = request(op, capability);
    if op == wire::OP_ACCEPT {
        value.transport = wire::TRANSPORT_TCP;
    }
    value
}

/// The service's interface and a real smoltcp peer host joined by a link.
struct Net {
    local_link: Link,
    local_iface: Interface,
    peer_link: Link,
    peer_iface: Interface,
    peers: SocketSet<'static>,
    handles: Vec<SocketHandle>,
    time: i64,
}

impl Net {
    fn new() -> Self {
        let mut local_link = Link::default();
        let mut peer_link = Link::default();
        let local_iface = interface(&mut local_link, 1);
        let peer_iface = interface(&mut peer_link, 2);
        let storage: &'static mut [SocketStorage<'static>] =
            std::boxed::Box::leak(std::boxed::Box::new([
                SocketStorage::EMPTY,
                SocketStorage::EMPTY,
                SocketStorage::EMPTY,
                SocketStorage::EMPTY,
            ]));
        let mut peers = SocketSet::new(storage);
        let handles = (0..4)
            .map(|_| {
                let mut socket = Socket::new(
                    SocketBuffer::new(vec![0u8; 4096].leak()),
                    SocketBuffer::new(vec![0u8; 4096].leak()),
                );
                socket.set_ack_delay(None);
                peers.add(socket)
            })
            .collect();
        Self {
            local_link,
            local_iface,
            peer_link,
            peer_iface,
            peers,
            handles,
            time: 0,
        }
    }

    fn socket(&mut self, index: usize) -> &mut Socket<'static> {
        self.peers.get_mut::<Socket>(self.handles[index])
    }

    fn connect(&mut self, index: usize) {
        let handle = self.handles[index];
        let context = self.peer_iface.context();
        self.peers
            .get_mut::<Socket>(handle)
            .connect(
                context,
                (IpAddress::v4(10, 0, 0, 1), PORT),
                50000 + index as u16,
            )
            .unwrap();
    }

    /// Exchange frames both ways; the service's egress guard judges its own.
    fn pump(&mut self, engine: &mut Engine<'_>) {
        for _ in 0..16 {
            self.time += 1;
            let now = Instant::from_millis(self.time);
            engine.tick(now);
            self.local_iface
                .poll(now, &mut self.local_link, engine.sockets());
            while let Some(frame) = self.local_link.tx.pop_front() {
                if engine.permit_egress(&frame) {
                    self.peer_link.rx.push_back(frame);
                }
            }
            self.peer_iface
                .poll(now, &mut self.peer_link, &mut self.peers);
            let replies: Vec<_> = self.peer_link.tx.drain(..).collect();
            self.local_link.rx.extend(replies);
        }
    }
}

#[test]
fn ingress_drops_every_unadmitted_segment_to_a_listener_and_counts_bare_syns() {
    let rules = [Some(ListenerRule {
        local: LOCAL,
        port: PORT,
        admitted: ADMITTED,
    })];
    let frame = |source: [u8; 4], destination: [u8; 4], port: u16, syn: bool, ack: bool| {
        let mut bytes = vec![0u8; 14 + 20 + 20];
        bytes[12..14].copy_from_slice(&[0x08, 0x00]);
        let ip = &mut bytes[14..34];
        ip[0] = 0x45;
        ip[2..4].copy_from_slice(&40u16.to_be_bytes());
        ip[8] = 64;
        ip[9] = 6;
        ip[12..16].copy_from_slice(&source);
        ip[16..20].copy_from_slice(&destination);
        let tcp = &mut bytes[34..54];
        tcp[0..2].copy_from_slice(&50000u16.to_be_bytes());
        tcp[2..4].copy_from_slice(&port.to_be_bytes());
        tcp[12] = 5 << 4;
        tcp[13] = (u8::from(syn) * 0x02) | (u8::from(ack) * 0x10);
        bytes
    };
    assert_eq!(
        listener_ingress(&rules, &frame(ADMITTED, LOCAL, PORT, true, false)),
        Ingress::Deliver
    );
    assert_eq!(
        listener_ingress(&rules, &frame([10, 0, 0, 3], LOCAL, PORT, true, false)),
        Ingress::Drop { syn: true }
    );
    assert_eq!(
        listener_ingress(&rules, &frame([10, 0, 0, 3], LOCAL, PORT, false, true)),
        Ingress::Drop { syn: false }
    );
    assert_eq!(
        listener_ingress(&rules, &frame([10, 0, 0, 3], LOCAL, PORT, true, true)),
        Ingress::Drop { syn: false }
    );
    // Other endpoints are not this rule's to judge.
    assert_eq!(
        listener_ingress(&rules, &frame([10, 0, 0, 3], LOCAL, PORT + 1, true, false)),
        Ingress::Deliver
    );
    assert_eq!(
        listener_ingress(
            &rules,
            &frame([10, 0, 0, 3], [10, 0, 0, 9], PORT, true, false)
        ),
        Ingress::Deliver
    );
    assert_eq!(listener_ingress(&rules, &[0; 10]), Ingress::Deliver);
    assert_eq!(
        listener_ingress(&[], &frame([10, 0, 0, 3], LOCAL, PORT, true, false)),
        Ingress::Deliver
    );
}

#[test]
fn external_listener_accepts_its_peer_refuses_excess_and_settles_every_close() {
    let mut storage = [SocketStorage::EMPTY; SOCKETS];
    let mut rx = [[0; BUFFER_BYTES]; SOCKETS];
    let mut tx = [[0; BUFFER_BYTES]; SOCKETS];
    let mut engine = Engine::new(&mut storage, &mut rx, &mut tx, 12, 0).unwrap();
    let mut net = Net::new();
    let mut context_link = Link::default();
    let mut context = interface(&mut context_link, 1);
    let mut foreign_link = Link::default();
    let mut foreign = interface(&mut foreign_link, 9);

    let destinations_bytes = declarations(RIGHT_CONNECT | RIGHT_SEND | RIGHT_RECV, 4096, 1, 1);
    let destinations = NetworkDestinations::decode(&destinations_bytes).unwrap();
    let application_bytes = applications();
    let applications = NetworkApplications::decode(&application_bytes).unwrap();
    let handle =
        |engine: &mut Engine<'_>, holder, request, payload: &mut [u8], iface: &mut Interface| {
            engine.handle_local(
                &destinations,
                &applications,
                holder,
                request,
                payload,
                Instant::ZERO,
                iface,
            )
        };

    // Exact authority: the endpoint, the holder and the interface address.
    for (holder, address, port) in [
        (LISTENER, LOCAL, PORT + 1),
        (LISTENER, [10, 0, 0, 9], PORT),
        (CLIENT, LOCAL, PORT),
    ] {
        assert_eq!(
            complete(handle(
                &mut engine,
                holder,
                listen_request(address, port),
                &mut [],
                &mut context
            ))
            .status,
            Status::Denied
        );
    }
    assert_eq!(
        complete(handle(
            &mut engine,
            LISTENER,
            listen_request(LOCAL, PORT),
            &mut [],
            &mut foreign
        ))
        .status,
        Status::Denied,
        "an external listener needs an address its interface owns"
    );
    let listener = complete(handle(
        &mut engine,
        LISTENER,
        listen_request(LOCAL, PORT),
        &mut [],
        &mut context,
    ));
    assert_eq!(listener.status, Status::Success);
    assert_eq!(listener.capability_kind, wire::CAPABILITY_TCP_LISTENER);

    net.connect(0);
    net.pump(&mut engine);
    let first = complete(engine.accept(LISTENER, listener.capability, Instant::ZERO));
    assert_eq!(first.status, Status::Success);
    // The listener rearms; a second connection completes its handshake and
    // occupies the backlog slot until accepted.
    net.pump(&mut engine);
    net.connect(1);
    net.pump(&mut engine);
    assert_eq!(net.socket(1).state(), State::Established);
    net.connect(2);
    net.pump(&mut engine);
    assert_eq!(engine.excess_refusals(), 1);
    assert_eq!(
        net.socket(2).state(),
        State::Closed,
        "the excess SYN is reset"
    );
    let second = complete(engine.accept(LISTENER, listener.capability, Instant::ZERO));
    assert_eq!(second.status, Status::Success);
    // Both accepted connections are live: the listener stays disarmed.
    net.pump(&mut engine);
    net.socket(2).abort();
    net.connect(2);
    net.pump(&mut engine);
    assert_eq!(engine.excess_refusals(), 2);
    assert_eq!(net.socket(2).state(), State::Closed);

    // Local half-close: FIN goes out, sends are refused, receive continues.
    assert_eq!(
        complete(handle(
            &mut engine,
            LISTENER,
            request(wire::OP_SHUTDOWN, first.capability),
            &mut [],
            &mut context
        ))
        .status,
        Status::Success
    );
    let mut payload = [7u8; 16];
    assert_eq!(
        complete(handle(
            &mut engine,
            LISTENER,
            request(wire::OP_SEND, first.capability),
            &mut payload,
            &mut context
        ))
        .status,
        Status::Refused
    );
    net.pump(&mut engine);
    assert_eq!(net.socket(0).state(), State::CloseWait);
    net.socket(0).send_slice(b"after half-close").unwrap();
    net.socket(0).close();
    net.pump(&mut engine);
    let mut received = [0u8; 32];
    let read = complete(handle(
        &mut engine,
        LISTENER,
        request(wire::OP_RECV, first.capability),
        &mut received,
        &mut context,
    ));
    assert_eq!(&received[..read.transferred], b"after half-close");
    let eof = complete(handle(
        &mut engine,
        LISTENER,
        request(wire::OP_RECV, first.capability),
        &mut received,
        &mut context,
    ));
    assert!(eof.eof && eof.transferred == 0);
    pending(handle(
        &mut engine,
        LISTENER,
        request(wire::OP_CLOSE, first.capability),
        &mut [],
        &mut context,
    ));
    assert_eq!(
        engine
            .poll_pending(LISTENER, first.capability, Instant::ZERO)
            .unwrap()
            .status,
        Status::Success
    );
    assert_eq!(
        engine.take_event(),
        Some(Event::AcceptedClose {
            unread: 0,
            unsent: 0,
            terminal: Terminal::TimeWait,
            bytes: 2 * BUFFER_BYTES
        })
    );

    // Close with unread bytes aborts with a reset and reports local disposal.
    net.socket(1).send_slice(b"unread").unwrap();
    net.pump(&mut engine);
    assert_eq!(
        complete(handle(
            &mut engine,
            LISTENER,
            request(wire::OP_CLOSE, second.capability),
            &mut [],
            &mut context
        ))
        .status,
        Status::Success
    );
    assert_eq!(
        engine.take_event(),
        Some(Event::AcceptedClose {
            unread: 6,
            unsent: 0,
            terminal: Terminal::Reset,
            bytes: 2 * BUFFER_BYTES
        })
    );
    net.pump(&mut engine);
    assert_eq!(
        net.socket(1).state(),
        State::Closed,
        "the peer saw the reset"
    );
    assert_eq!(
        complete(handle(
            &mut engine,
            LISTENER,
            request(wire::OP_RECV, second.capability),
            &mut received,
            &mut context
        ))
        .status,
        Status::Denied,
        "a disposed handle is refused"
    );

    // The listener rearmed once children fell below the limit; closing it
    // reports no live children.
    assert_eq!(
        complete(handle(
            &mut engine,
            LISTENER,
            capability_request(wire::OP_CLOSE, listener.capability),
            &mut [],
            &mut context
        ))
        .status,
        Status::Success
    );
    assert_eq!(
        engine.take_event(),
        Some(Event::ListenerClosed { children: 0 })
    );
    assert_eq!(engine.take_event(), None);
    assert_eq!(engine.allocated(), 0);
}
