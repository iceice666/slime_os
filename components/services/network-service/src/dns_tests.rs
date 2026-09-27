//! Host integration tests execute the production engine over real smoltcp UDP/TCP.
use super::tests::{HOLDER, Link, PEER, declarations, interface, pending, request};
use super::*;
use boot_contracts::network_destination as contract;
use std::vec::Vec;
extern crate std;

fn policy(controlled: bool) -> Vec<u8> {
    use contract::*;
    let base = declarations(RIGHT_CONNECT | RIGHT_SEND | RIGHT_RECV, 8192, 4, 4);
    let mut rows = Vec::new();
    if controlled {
        rows.push(base[HEADER_BYTES..].to_vec());
    }
    let mut named = base[HEADER_BYTES..].to_vec();
    named[OFF_ENTRY_ADDRESS_KIND] = ADDRESS_DNS;
    named[OFF_ENTRY_ADDRESS..OFF_ENTRY_ADDRESS_END].fill(0);
    named[OFF_ENTRY_NAME..OFF_ENTRY_NAME + 11].copy_from_slice(b"example.com");
    named[OFF_ENTRY_NAME_LEN..OFF_ENTRY_NAME_LEN + 2].copy_from_slice(&11u16.to_le_bytes());
    named[OFF_ENTRY_DNS_RECORD_LIMIT..OFF_ENTRY_DNS_RECORD_LIMIT + 4]
        .copy_from_slice(&4u32.to_le_bytes());
    rows.push(named);
    let mut resolver = base[HEADER_BYTES..].to_vec();
    resolver[OFF_ENTRY_HOLDER_IDENTITY..OFF_ENTRY_HOLDER_IDENTITY_END]
        .copy_from_slice(&holder_identity("network-service"));
    resolver[OFF_ENTRY_TRANSPORT] = TRANSPORT_UDP;
    resolver[OFF_ENTRY_RIGHTS..OFF_ENTRY_RIGHTS + 2]
        .copy_from_slice(&(RIGHT_SEND | RIGHT_RECV).to_le_bytes());
    resolver[OFF_ENTRY_PORT..OFF_ENTRY_PORT + 2]
        .copy_from_slice(&(if controlled { 1053u16 } else { 53 }).to_le_bytes());
    resolver[OFF_ENTRY_DNS_RECORD_LIMIT..OFF_ENTRY_DNS_RECORD_LIMIT + 4]
        .copy_from_slice(&4u32.to_le_bytes());
    rows.push(resolver);
    rows.sort_by_key(|row| {
        (
            row[OFF_ENTRY_HOLDER_IDENTITY..OFF_ENTRY_HOLDER_IDENTITY_END].to_vec(),
            row[OFF_ENTRY_TRANSPORT],
            row[OFF_ENTRY_ADDRESS_KIND],
        )
    });
    let mut bytes = base[..HEADER_BYTES].to_vec();
    bytes[OFF_HEADER_DESTINATION_COUNT..OFF_HEADER_DESTINATION_COUNT + 4]
        .copy_from_slice(&(rows.len() as u32).to_le_bytes());
    let size = HEADER_BYTES + rows.len() * ENTRY_BYTES;
    bytes[OFF_HEADER_TOTAL_LEN..OFF_HEADER_TOTAL_LEN + 4]
        .copy_from_slice(&(size as u32).to_le_bytes());
    for row in rows {
        bytes.extend_from_slice(&row);
    }
    bytes
}
fn named_request() -> WireNetworkRequest {
    let mut req = request(wire::OP_CONNECT, 0);
    req.address_kind = wire::ADDRESS_DNS;
    req.name_len = 11;
    req.endpoint.fill(0);
    req.endpoint[..11].copy_from_slice(b"example.com");
    req
}

#[test]
fn hostname_without_seed_fails_closed_and_creates_no_handle() {
    let mut storage = [SocketStorage::EMPTY; SOCKETS];
    let mut rx = [[0; BUFFER_BYTES]; SOCKETS];
    let mut tx = rx;
    let mut engine = Engine::new(&mut storage, &mut rx, &mut tx, 1, 0).unwrap();
    let mut link = Link::default();
    let mut iface = interface(&mut link, 1);
    let bytes = policy(false);
    let destinations = NetworkDestinations::decode(&bytes).unwrap();
    assert!(matches!(
        engine.handle(
            &destinations,
            HOLDER,
            named_request(),
            &mut [],
            Instant::ZERO,
            &mut iface
        ),
        Outcome::Complete(Completion {
            status: Status::Unsupported,
            ..
        })
    ));
    assert_eq!(engine.allocated(), 0);
}

#[test]
fn real_udp_resolve_connect_is_holder_bound_and_releases() {
    exchange(true, 90, false, false);
}
#[test]
fn public_policy_refuses_private_answer_without_exact_exception() {
    exchange(false, 90, false, false);
}
#[test]
fn zero_ttl_refuses_connect() {
    exchange(true, 0, false, false);
}
#[test]
fn wrong_resolver_port_and_transaction_cannot_complete() {
    exchange(true, 90, true, false);
}
#[test]
fn multi_address_refusal_falls_back_before_sending_application_data() {
    exchange(true, 90, false, true);
}

#[test]
fn received_positive_ttl_expires_before_fallback_and_fresh_resolution_recovers() {
    exchange(true, 1, false, true);
}

fn exchange(controlled: bool, ttl: u32, spoof: bool, fallback: bool) {
    let mut storage = [SocketStorage::EMPTY; SOCKETS + 1];
    let mut rx = [[0; BUFFER_BYTES]; SOCKETS];
    let mut tx = rx;
    let mut engine = Engine::new(&mut storage, &mut rx, &mut tx, 1, 0).unwrap();
    let bytes = policy(controlled);
    let destinations = NetworkDestinations::decode(&bytes).unwrap();
    let mut udp_rx = [0; resolver::BUFFER_BYTES];
    let mut udp_tx = udp_rx;
    let mut rx_meta = [udp::PacketMetadata::EMPTY];
    let mut tx_meta = rx_meta;
    engine
        .enable_dns(
            &destinations,
            [7; 32],
            &mut rx_meta,
            &mut udp_rx,
            &mut tx_meta,
            &mut udp_tx,
        )
        .unwrap();
    let mut link = Link::default();
    let mut iface = interface(&mut link, 1);
    iface
        .routes_mut()
        .add_default_ipv4_route(Ipv4Address::from(PEER))
        .unwrap();
    let mut peer_link = Link::default();
    let mut peer_iface = interface(&mut peer_link, 2);
    let mut peer_storage = [SocketStorage::EMPTY; 3];
    let mut peer_sockets = SocketSet::new(&mut peer_storage[..]);
    let mut dns_rx = [0; 512];
    let mut dns_tx = dns_rx;
    let mut dns_rx_meta = [udp::PacketMetadata::EMPTY];
    let mut dns_tx_meta = dns_rx_meta;
    let mut peer_dns = udp::Socket::new(
        udp::PacketBuffer::new(&mut dns_rx_meta[..], &mut dns_rx[..]),
        udp::PacketBuffer::new(&mut dns_tx_meta[..], &mut dns_tx[..]),
    );
    peer_dns.bind(if controlled { 1053 } else { 53 }).unwrap();
    let dns_handle = peer_sockets.add(peer_dns);
    let mut wrong_rx = [0; 512];
    let mut wrong_tx = wrong_rx;
    let mut wrong_rx_meta = [udp::PacketMetadata::EMPTY];
    let mut wrong_tx_meta = wrong_rx_meta;
    let mut wrong_dns = udp::Socket::new(
        udp::PacketBuffer::new(&mut wrong_rx_meta[..], &mut wrong_rx[..]),
        udp::PacketBuffer::new(&mut wrong_tx_meta[..], &mut wrong_tx[..]),
    );
    wrong_dns.bind(1054).unwrap();
    let wrong_handle = peer_sockets.add(wrong_dns);
    let mut tcp_rx = [0; BUFFER_BYTES];
    let mut tcp_tx = tcp_rx;
    let mut peer_tcp = Socket::new(
        SocketBuffer::new(&mut tcp_rx[..]),
        SocketBuffer::new(&mut tcp_tx[..]),
    );
    peer_tcp.listen(8080).unwrap();
    let tcp_handle = peer_sockets.add(peer_tcp);
    let mut query_tuples = Vec::new();
    for cycle in 0..6 {
        // Alternate expiry with a fresh usable response in the same engine.
        let expires = ttl == 1 && cycle % 2 == 0;
        let response_ttl = if ttl == 1 && !expires { 90 } else { ttl };
        let base = cycle * 20000;
        let now = Instant::from_millis(base);
        engine.tick(now);
        peer_sockets.get_mut::<Socket>(tcp_handle).abort();
        peer_sockets
            .get_mut::<Socket>(tcp_handle)
            .listen(8080)
            .unwrap();
        let mut id = pending(engine.handle(
            &destinations,
            HOLDER,
            named_request(),
            &mut [],
            now,
            &mut iface,
        ));
        assert_eq!(
            engine
                .poll_pending([9; 32], id, Instant::ZERO)
                .unwrap()
                .status,
            Status::Denied
        );
        let mut observed_query = false;
        let tuples_before = query_tuples.len();
        let mut completion = None;
        let mut response = None;
        let mut reset_injected = false;
        let mut syn_destinations = Vec::new();
        let mut completion_time = None;
        let mut answer_time = None;
        for step in 0..1000 {
            let now = Instant::from_millis(base + step * 10);
            engine.advance_dns(&destinations, &mut iface, now);
            engine.tick(now);
            iface.poll(now, &mut link, engine.sockets());
            while let Some(frame) = link.tx.pop_front() {
                assert!(engine.permit_egress(&frame));
                let ethernet = EthernetFrame::new_checked(&frame[..]).unwrap();
                if ethernet.ethertype() == EthernetProtocol::Ipv4 {
                    let ip = Ipv4Packet::new_checked(ethernet.payload()).unwrap();
                    if ip.next_header() == IpProtocol::Udp {
                        assert!(!engine.permit_egress(&frame));
                    } else if ip.next_header() == IpProtocol::Tcp {
                        let tcp = TcpPacket::new_checked(ip.payload()).unwrap();
                        if tcp.syn() {
                            assert!(tcp.payload().is_empty());
                            let destination = ip.dst_addr();
                            if !syn_destinations.contains(&destination) {
                                syn_destinations.push(destination);
                            }
                        }
                        // The default gateway answers ARP, but drops the first
                        // destination's TCP packets after we observe the SYN.
                        if ip.dst_addr() == Ipv4Address::new(8, 8, 8, 8) {
                            continue;
                        }
                    }
                }
                peer_link.rx.push_back(frame);
            }
            peer_iface.poll(now, &mut peer_link, &mut peer_sockets);
            link.rx.extend(peer_link.tx.drain(..));
            if let Ok((query, metadata)) = peer_sockets.get_mut::<udp::Socket>(dns_handle).recv() {
                observed_query = true;
                let tuple = (query[..2].to_vec(), metadata.endpoint.port);
                assert!(!query_tuples.contains(&tuple));
                query_tuples.push(tuple);
                assert_eq!(&query[12..], b"\x07example\x03com\0\0\x01\0\x01");
                let mut reply = query.to_vec();
                reply[2..4].copy_from_slice(&0x8180u16.to_be_bytes());
                reply[7] = if fallback { 2 } else { 1 };
                for address in if fallback {
                    &[[8, 8, 8, 8], PEER][..]
                } else {
                    &[PEER][..]
                } {
                    reply.extend_from_slice(&[0xc0, 12, 0, 1, 0, 1]);
                    reply.extend_from_slice(&response_ttl.to_be_bytes());
                    reply.extend_from_slice(&[0, 4]);
                    reply.extend_from_slice(address);
                }
                response = Some((reply, metadata.endpoint));
            }
            if let Some((reply, endpoint)) = response.take() {
                if cycle == 0 && controlled && ttl != 0 && !spoof && !reset_injected {
                    let old_id = id;
                    assert_eq!(engine.reset_all(now).handles, 1);
                    assert_eq!(
                        engine.poll_pending(HOLDER, old_id, now).unwrap().status,
                        Status::Denied
                    );
                    id = pending(engine.handle(
                        &destinations,
                        HOLDER,
                        named_request(),
                        &mut [],
                        now,
                        &mut iface,
                    ));
                    assert_ne!(old_id, id);
                    reset_injected = true;
                    peer_sockets
                        .get_mut::<udp::Socket>(dns_handle)
                        .send_slice(&reply, endpoint)
                        .unwrap();
                    continue;
                }
                if spoof {
                    peer_sockets
                        .get_mut::<udp::Socket>(wrong_handle)
                        .send_slice(&reply, endpoint)
                        .unwrap();
                    let mut wrong_id = reply;
                    wrong_id[0] ^= 1;
                    peer_sockets
                        .get_mut::<udp::Socket>(dns_handle)
                        .send_slice(&wrong_id, endpoint)
                        .unwrap();
                } else {
                    peer_sockets
                        .get_mut::<udp::Socket>(dns_handle)
                        .send_slice(&reply, endpoint)
                        .unwrap();
                    answer_time = Some(now);
                }
            }
            if let Some(result) = engine.poll_pending(HOLDER, id, now) {
                completion = Some(result);
                completion_time = Some(now);
                break;
            }
        }
        assert!(observed_query);
        let completion = completion.expect("bounded terminal result");
        if !controlled || ttl == 0 {
            assert_eq!(completion.status, Status::Denied);
        } else if spoof {
            assert_eq!(query_tuples.len() - tuples_before, 3);
            assert_eq!(completion.status, Status::Timeout);
        } else if expires {
            assert_eq!(completion.status, Status::Timeout);
            assert_eq!(syn_destinations, [Ipv4Address::new(8, 8, 8, 8)]);
            assert!(completion_time.unwrap() >= answer_time.unwrap() + Duration::from_secs(1));
            assert!(!peer_sockets.get::<Socket>(tcp_handle).may_recv());
        } else {
            assert_eq!(completion.status, Status::Success);
            assert_eq!(completion.capability, id);
            assert!(peer_sockets.get::<Socket>(tcp_handle).may_recv());
            if fallback {
                assert_eq!(
                    syn_destinations,
                    [Ipv4Address::new(8, 8, 8, 8), Ipv4Address::from(PEER)]
                );
            }
        }
        engine.release_holder(HOLDER);
        assert_eq!(engine.allocated(), 0);
        assert!(engine.name_connect.is_none());
        // Drain the holder's authorized RST before reusing either peer tuple.
        let now = Instant::from_millis(base + 10000);
        iface.poll(now, &mut link, engine.sockets());
        while let Some(frame) = link.tx.pop_front() {
            if engine.permit_egress(&frame) {
                peer_link.rx.push_back(frame);
            }
        }
        peer_iface.poll(now, &mut peer_link, &mut peer_sockets);
        link.rx.extend(peer_link.tx.drain(..));
    }
}

#[test]
fn name_port_holder_authority_expiry_and_death_are_fail_closed() {
    let mut storage = [SocketStorage::EMPTY; SOCKETS + 1];
    let mut rx = [[0; BUFFER_BYTES]; SOCKETS];
    let mut tx = rx;
    let mut engine = Engine::new(&mut storage, &mut rx, &mut tx, 1, 0).unwrap();
    let bytes = policy(true);
    let destinations = NetworkDestinations::decode(&bytes).unwrap();
    let mut udp_rx = [0; resolver::BUFFER_BYTES];
    let mut udp_tx = udp_rx;
    let mut rx_meta = [udp::PacketMetadata::EMPTY];
    let mut tx_meta = rx_meta;
    engine
        .enable_dns(
            &destinations,
            [5; 32],
            &mut rx_meta,
            &mut udp_rx,
            &mut tx_meta,
            &mut udp_tx,
        )
        .unwrap();
    let mut link = Link::default();
    let mut iface = interface(&mut link, 1);
    for (holder, req) in [
        ([9; 32], named_request()),
        (
            HOLDER,
            WireNetworkRequest {
                port: 80,
                ..named_request()
            },
        ),
        (HOLDER, {
            let mut req = named_request();
            req.endpoint[0] = b'z';
            req
        }),
    ] {
        assert!(matches!(
            engine.handle(
                &destinations,
                holder,
                req,
                &mut [],
                Instant::ZERO,
                &mut iface
            ),
            Outcome::Complete(Completion {
                status: Status::Denied,
                ..
            })
        ));
    }
    let id = pending(engine.handle(
        &destinations,
        HOLDER,
        named_request(),
        &mut [],
        Instant::ZERO,
        &mut iface,
    ));
    assert!(matches!(
        engine.handle(
            &destinations,
            HOLDER,
            named_request(),
            &mut [],
            Instant::ZERO,
            &mut iface
        ),
        Outcome::Complete(Completion {
            status: Status::Exhausted,
            ..
        })
    ));
    let mut pending_name = engine.name_connect.unwrap();
    pending_name.count = 1;
    pending_name.expiry = Instant::from_millis(100);
    engine.name_connect = Some(pending_name);
    engine.advance_dns(&destinations, &mut iface, Instant::from_millis(100));
    assert_eq!(
        engine
            .poll_pending(HOLDER, id, Instant::from_millis(100))
            .unwrap()
            .status,
        Status::Timeout
    );
    assert_eq!(engine.allocated(), 0);
    let id = pending(engine.handle(
        &destinations,
        HOLDER,
        named_request(),
        &mut [],
        Instant::from_millis(200),
        &mut iface,
    ));
    let released = engine.release_holder(HOLDER);
    assert_eq!(released.handles, 1);
    assert!(engine.name_connect.is_none());
    assert_eq!(
        engine
            .poll_pending(HOLDER, id, Instant::from_millis(200))
            .unwrap()
            .status,
        Status::Denied
    );
}

#[test]
fn resolver_requires_spare_socket_and_complete_service_authority() {
    use contract::*;
    for mode in 0..6 {
        let mut storage = [SocketStorage::EMPTY; SOCKETS + 1];
        let storage = if mode == 0 {
            &mut storage[..SOCKETS]
        } else {
            &mut storage[..]
        };
        let mut rx = [[0; BUFFER_BYTES]; SOCKETS];
        let mut tx = rx;
        let mut engine = Engine::new(storage, &mut rx, &mut tx, 1, 0).unwrap();
        let mut bytes = policy(true);
        let table = NetworkDestinations::decode(&bytes).unwrap();
        let row = (0..table.destination_count())
            .find(|index| table.destination(*index).unwrap().transport == Transport::Udp)
            .unwrap();
        let offset = HEADER_BYTES + row * ENTRY_BYTES;
        match mode {
            1 => bytes[offset + OFF_ENTRY_RIGHTS..offset + OFF_ENTRY_RIGHTS + 2]
                .copy_from_slice(&RIGHT_SEND.to_le_bytes()),
            2 => bytes
                [offset + OFF_ENTRY_DNS_RECORD_LIMIT..offset + OFF_ENTRY_DNS_RECORD_LIMIT + 4]
                .fill(0),
            3 => bytes[offset + OFF_ENTRY_BYTE_BUDGET..offset + OFF_ENTRY_BYTE_BUDGET + 4]
                .copy_from_slice(&512u32.to_le_bytes()),
            4 => bytes[offset + OFF_ENTRY_PORT..offset + OFF_ENTRY_PORT + 2]
                .copy_from_slice(&5353u16.to_le_bytes()),
            _ => (),
        }
        let table = NetworkDestinations::decode(&bytes).unwrap();
        let mut udp_rx = [0; resolver::BUFFER_BYTES];
        let mut udp_tx = udp_rx;
        let mut rx_meta = [udp::PacketMetadata::EMPTY];
        let mut tx_meta = rx_meta;
        assert_eq!(
            engine.enable_dns(
                &table,
                if mode == 5 { [0; 32] } else { [1; 32] },
                &mut rx_meta,
                &mut udp_rx,
                &mut tx_meta,
                &mut udp_tx
            ),
            Err(Status::Denied)
        );
    }
}
