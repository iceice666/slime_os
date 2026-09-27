#!/usr/bin/env python3
"""Host regressions for `scripts/lib/link_peer.py`, the frame-level peer the
network planes talk to. No QEMU: every case drives the peer's pure half with
frames built by the same encoders, and the assertions are on the bytes the
peer would put on the wire and on what its ledger records."""

from __future__ import annotations

import copy
import dataclasses
import struct
import sys
import threading
from unittest.mock import patch
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

import link_peer as lp  # noqa: E402
from harness import load_script  # noqa: E402

GUEST_MAC = bytes.fromhex("5254005" "34c01")


def fail(message: str) -> None:
    raise SystemExit(f"link peer check: {message}")


def expect(condition: bool, message: str) -> None:
    if not condition:
        fail(message)


def check_checksums() -> None:
    header = lp.ipv4(lp.PEER_IP, lp.GUEST_IP, lp.IP_PROTOCOL_ICMP, b"")[:20]
    expect(lp.checksum(header) == 0, "an IPv4 header the encoder produced does not verify")
    echo = lp.icmp_echo(lp.ICMP_ECHO_REQUEST, 7, 9, b"abc")
    expect(lp.checksum(echo) == 0, "an ICMP echo the encoder produced does not verify")
    corrupted = bytearray(echo)
    corrupted[-1] ^= 1
    expect(lp.checksum(bytes(corrupted)) != 0, "a corrupted ICMP echo still verifies")


def check_frames_decode_and_pad() -> None:
    request = lp.ethernet(lp.BROADCAST, GUEST_MAC, lp.ETHERTYPE_ARP, lp.arp(lp.ARP_REQUEST, GUEST_MAC, lp.GUEST_IP, bytes(6), lp.PEER_IP))
    expect(len(request) == lp.MIN_FRAME, "an ARP request is not padded to the minimum frame")
    frame = lp.decode(request)
    expect(frame is not None and frame.kind == "arp-request" and frame.arp_target_ip == lp.PEER_IP, "ARP request decode")
    expect(lp.decode(b"\x00" * 10) is None, "a runt frame decoded")
    bad_ip = bytearray(lp.ethernet(GUEST_MAC, lp.PEER_MAC, lp.ETHERTYPE_IPV4, lp.ipv4(lp.PEER_IP, lp.GUEST_IP, lp.IP_PROTOCOL_ICMP, lp.icmp_echo(lp.ICMP_ECHO_REQUEST, 1, 1, b""))))
    bad_ip[14 + 10] ^= 0xFF
    decoded = lp.decode(bytes(bad_ip))
    expect(decoded is not None and decoded.kind == "other", "a frame with a bad IPv4 checksum was classified as IPv4")


def check_peer_answers_arp_and_echo() -> None:
    peer = lp.Peer()
    request = lp.ethernet(lp.BROADCAST, GUEST_MAC, lp.ETHERTYPE_ARP, lp.arp(lp.ARP_REQUEST, GUEST_MAC, lp.GUEST_IP, bytes(6), lp.PEER_IP))
    replies = peer.handle(request)
    expect(len(replies) == 1, "an ARP request for the peer got no single reply")
    reply = lp.decode(replies[0])
    expect(reply is not None and reply.kind == "arp-reply" and reply.destination == GUEST_MAC and reply.arp_sender_mac == lp.PEER_MAC and reply.arp_sender_ip == lp.PEER_IP, "ARP reply contents")
    other = lp.ethernet(lp.BROADCAST, GUEST_MAC, lp.ETHERTYPE_ARP, lp.arp(lp.ARP_REQUEST, GUEST_MAC, lp.GUEST_IP, bytes(6), bytes([10, 0, 0, 3])))
    expect(peer.handle(other) == [], "an ARP request for another host was answered")
    echo = lp.ethernet(lp.PEER_MAC, GUEST_MAC, lp.ETHERTYPE_IPV4, lp.ipv4(lp.GUEST_IP, lp.PEER_IP, lp.IP_PROTOCOL_ICMP, lp.icmp_echo(lp.ICMP_ECHO_REQUEST, 0x1234, 3, b"payload")))
    replies = peer.handle(echo)
    expect(len(replies) == 1, "an echo request to the peer got no single reply")
    reply = lp.decode(replies[0])
    expect(reply is not None and reply.kind == "icmp-echo-reply" and reply.ip_destination == lp.GUEST_IP and reply.icmp_identifier == 0x1234 and reply.icmp_sequence == 3 and reply.icmp_payload == b"payload", "echo reply contents")
    elsewhere = lp.ethernet(lp.PEER_MAC, GUEST_MAC, lp.ETHERTYPE_IPV4, lp.ipv4(lp.GUEST_IP, bytes([10, 0, 0, 3]), lp.IP_PROTOCOL_ICMP, lp.icmp_echo(lp.ICMP_ECHO_REQUEST, 1, 1, b"")))
    expect(peer.handle(elsewhere) == [], "an echo request for another host was answered")
    expect(peer.ledger.count_received("arp-request") == 2 and peer.ledger.count_received("icmp-echo-request") == 2, "ledger counts")
    expect(peer.ledger.count_sent("arp-reply") == 1 and peer.ledger.count_sent("icmp-echo-reply") == 1, "ledger sent counts")
    expect(bytes([10, 0, 0, 3]) in peer.ledger.ip_destinations(), "the ledger lost the undeclared destination")
    expect(peer.ledger.foreign_sources(GUEST_MAC) == [], "the guest MAC was reported as foreign")
    expect(peer.ledger.foreign_sources(lp.PEER_MAC) == [GUEST_MAC], "a foreign MAC went unreported")


def check_peer_learns_the_guest_and_pings_it() -> None:
    peer = lp.Peer()
    expect(peer.echo_request(1, 1, b"") is None, "an echo request was built before the guest MAC was known")
    probe = lp.decode(peer.arp_request_for_guest())
    expect(probe is not None and probe.kind == "arp-request" and probe.arp_target_ip == lp.GUEST_IP and probe.destination == lp.BROADCAST, "guest ARP probe")
    reply = lp.ethernet(lp.PEER_MAC, GUEST_MAC, lp.ETHERTYPE_ARP, lp.arp(lp.ARP_REPLY, GUEST_MAC, lp.GUEST_IP, lp.PEER_MAC, lp.PEER_IP))
    expect(peer.handle(reply) == [], "an ARP reply was answered")
    expect(peer.ledger.guest_mac == GUEST_MAC, "the guest MAC was not learned from its ARP reply")
    request = peer.echo_request(lp.ECHO_IDENTIFIER, 4, lp.ECHO_PAYLOAD)
    decoded = lp.decode(request or b"")
    expect(decoded is not None and decoded.kind == "icmp-echo-request" and decoded.destination == GUEST_MAC and decoded.ip_destination == lp.GUEST_IP and decoded.icmp_sequence == 4, "echo request to the guest")
    answer = lp.ethernet(lp.PEER_MAC, GUEST_MAC, lp.ETHERTYPE_IPV4, lp.ipv4(lp.GUEST_IP, lp.PEER_IP, lp.IP_PROTOCOL_ICMP, lp.icmp_echo(lp.ICMP_ECHO_REPLY, lp.ECHO_IDENTIFIER, 4, lp.ECHO_PAYLOAD)))
    peer.handle(answer)
    expect(peer.ledger.echo_replies_matching(lp.ECHO_IDENTIFIER) == [4], "the guest's echo reply was not matched")
    expect(peer.ledger.echo_replies_matching(0) == [], "an echo reply matched a foreign identifier")


def tcp_frame(
    sequence: int = 100,
    acknowledgment: int = 0,
    flags: int = lp.TCP_SYN,
    payload: bytes = b"",
    port: int = lp.TCP_ECHO_PORT,
    source_port: int = 50000,
    window: int = lp.TCP_STREAM_LIMIT,
    options: bytes = b"",
    destination_ip: bytes = lp.PEER_IP,
    destination_mac: bytes = lp.PEER_MAC,
) -> bytes:
    segment = lp.tcp(lp.GUEST_IP, destination_ip, source_port, port, sequence, acknowledgment, flags, payload, window, options)
    return lp.ethernet(destination_mac, GUEST_MAC, lp.ETHERTYPE_IPV4, lp.ipv4(lp.GUEST_IP, destination_ip, lp.IP_PROTOCOL_TCP, segment))


def open_tcp(peer: lp.Peer, sequence: int = 100, source_port: int = 50000, options: bytes = b"") -> lp.TcpConnection:
    syn = tcp_frame(sequence=sequence, source_port=source_port, options=options)
    replies = peer.handle(syn)
    expect(len(replies) == 1, "TCP SYN did not receive exactly one reply")
    reply = lp.decode(replies[0])
    expect(reply is not None and reply.tcp_flags == lp.TCP_SYN | lp.TCP_ACK and reply.tcp_acknowledgment == (sequence + 1) & lp.TCP_SEQUENCE_MASK, "TCP SYN-ACK contents")
    expect(peer.handle(syn) == replies, "repeated SYN did not retransmit identical SYN-ACK")
    connection = peer.ledger.tcp_connections[-1]
    expect(not connection.handshake, "SYN alone counted as a handshake")
    peer.handle(tcp_frame(connection.receive_sequence, connection.send_sequence, lp.TCP_ACK, source_port=source_port))
    expect(connection.handshake, "valid third handshake leg was not observed")
    return connection


def check_tcp_stream_and_close() -> None:
    peer = lp.Peer()
    connection = open_tcp(peer, sequence=0xFFFFFC00, options=b"\x02\x04\x02\x00")
    payload = bytes((index * 37 + 11) & 255 for index in range(lp.TCP_STREAM_LIMIT))
    offset = 0
    wire_echo = bytearray()
    for count in (1, 1000, 7, 1460, 1628):
        part = payload[offset : offset + count]
        frame = tcp_frame(connection.receive_sequence, connection.send_sequence, lp.TCP_ACK | lp.TCP_PSH, part)
        replies = peer.handle(frame)
        for raw in replies:
            decoded = lp.decode(raw)
            expect(decoded is not None and decoded.tcp_source_port == lp.TCP_ECHO_PORT and len(decoded.tcp_payload) <= 512, "TCP echo ignored peer MSS or checksum")
            assert decoded is not None
            expect(decoded.tcp_sequence == (connection.peer_initial_sequence + 1 + len(wire_echo)) & lp.TCP_SEQUENCE_MASK, "echo sequence is not contiguous")
            wire_echo.extend(decoded.tcp_payload)
        before = bytes(connection.received), bytes(connection.sent)
        duplicates = peer.handle(frame)
        expect(before == (bytes(connection.received), bytes(connection.sent)), "retransmission delivered or echoed bytes twice")
        expect(all(not lp.decode(raw).tcp_payload for raw in duplicates), "duplicate data caused a new echo delivery")
        offset += count
    expect(offset == lp.TCP_STREAM_LIMIT and wire_echo == payload, "full stream wire echo differs from received bytes")
    expect(connection.received == payload and connection.sent == payload, "ledger stream differs from wire bytes")
    peer.handle(tcp_frame(connection.receive_sequence, connection.send_sequence, lp.TCP_ACK))
    expect(connection.acknowledged == payload, "ACK ledger did not capture the actual acknowledged stream")
    expect(not connection.closed, "data ACK alone closed connection")
    fin = tcp_frame(connection.receive_sequence, connection.send_sequence, lp.TCP_FIN | lp.TCP_ACK)
    replies = peer.handle(fin)
    expect(connection.guest_fin and connection.peer_fin and not connection.closed, "TCP FIN exchange not recorded independently")
    expect(len(replies) == 1 and lp.decode(replies[0]).tcp_flags == lp.TCP_FIN | lp.TCP_ACK, "guest FIN did not receive peer FIN")
    expect(peer.handle(fin) == replies, "unacknowledged FIN retransmission changed sequence or flags")
    peer.handle(tcp_frame(connection.receive_sequence, connection.send_sequence, lp.TCP_ACK))
    expect(connection.closed, "final FIN acknowledgment was not observed")
    peer.handle(fin)
    expect(connection.received == payload and connection.closed, "duplicate FIN changed the closed stream")


def check_tcp_partial_and_window() -> None:
    peer = lp.Peer()
    connection = open_tcp(peer)
    data = b"actual short input, not the fixture"
    peer.handle(tcp_frame(101, connection.send_sequence, lp.TCP_ACK, data, window=0))
    expect(connection.received == data and connection.sent == b"", "zero window was ignored or input invented")
    replies = peer.handle(tcp_frame(connection.receive_sequence, connection.send_sequence, lp.TCP_ACK, window=5))
    expect(connection.sent == data[:5] and len(replies) == 1, "small receive window was exceeded")
    replies = peer.handle(tcp_frame(connection.receive_sequence, connection.send_sequence, lp.TCP_ACK, window=64))
    expect(connection.sent == data, "window update did not release queued real bytes")
    expect(b"".join(lp.decode(raw).tcp_payload for raw in replies) == data[5:], "queued echo did not originate from received bytes")
    original = bytes(connection.received)
    peer.handle(tcp_frame(connection.receive_sequence + 5, connection.send_sequence, lp.TCP_ACK, b"gap"))
    expect(connection.received == original, "out-of-order stream gap was delivered")
    peer.handle(tcp_frame(101, connection.send_sequence, lp.TCP_ACK, b"wrong"))
    expect(connection.received == original, "conflicting duplicate replaced accepted stream")
    peer.handle(tcp_frame(connection.receive_sequence - 3, connection.send_sequence, lp.TCP_ACK, data[-3:] + b"!"))
    expect(connection.received == data + b"!", "overlapping retransmission lost its new suffix")
    peer.handle(tcp_frame(connection.receive_sequence, connection.send_sequence, lp.TCP_FIN | lp.TCP_ACK))
    peer.handle(tcp_frame(connection.receive_sequence, connection.send_sequence, lp.TCP_ACK))
    expect(connection.closed and connection.received == data + b"!", "partial stream close fabricated a full fixture")
    before = bytes(connection.received)
    peer.handle(tcp_frame(connection.receive_sequence, connection.send_sequence, lp.TCP_ACK, b"after FIN"))
    expect(connection.received == before, "closed connection accepted more stream bytes")


def check_tcp_bounds_and_refusal() -> None:
    peer = lp.Peer()
    for frame in (tcp_frame(destination_ip=bytes([10, 0, 0, 3])), tcp_frame(destination_mac=lp.BROADCAST), tcp_frame(port=80), tcp_frame(flags=lp.TCP_ACK)):
        expect(peer.handle(frame) == [], "undeclared TCP endpoint or unsolicited ACK was answered")
    expect(peer.ledger.tcp_connections == [], "invalid traffic allocated a TCP connection")
    replies = peer.handle(tcp_frame(port=lp.TCP_REFUSED_PORT))
    expect(len(replies) == 1 and peer.ledger.tcp_resets_sent == 1, "closed TCP port was not refused")
    reply = lp.decode(replies[0])
    expect(reply is not None and reply.tcp_flags == lp.TCP_RST | lp.TCP_ACK and reply.tcp_acknowledgment == 101 and reply.tcp_source_port == lp.TCP_REFUSED_PORT, "refusal was not a valid TCP reset")
    for index in range(lp.TCP_CONNECTION_LIMIT):
        connection = open_tcp(peer, source_port=50000 + index)
    expect(peer.handle(tcp_frame(source_port=51000)) == [] and len(peer.ledger.tcp_connections) == lp.TCP_CONNECTION_LIMIT, "connection capacity was not bounded")
    peer.handle(tcp_frame(connection.receive_sequence, connection.send_sequence, lp.TCP_ACK, b"x" * (lp.TCP_STREAM_LIMIT + 1), source_port=connection.guest_port))
    expect(not connection.received, "oversized stream exceeded bounded storage")
    peer.handle(tcp_frame(connection.receive_sequence + 1, connection.send_sequence, lp.TCP_RST, source_port=connection.guest_port))
    expect(not connection.reset, "out-of-window reset was accepted")
    peer.handle(tcp_frame(connection.receive_sequence, connection.send_sequence, lp.TCP_RST, source_port=connection.guest_port))
    expect(connection.reset and not connection.closed, "reset was not distinguished from graceful close")


def check_tcp_malformed() -> None:
    valid = tcp_frame(options=b"\x02\x04\x05\xb4")
    # Mutations repair enclosing checksums so each inner bound is exercised.
    def mutated(offset: int, value: bytes, fix_tcp: bool = False, fix_ip: bool = False) -> bytes:
        raw = bytearray(valid)
        raw[offset : offset + len(value)] = value
        if fix_tcp:
            raw[50:52] = b"\x00\x00"
            total_len = int.from_bytes(raw[16:18], "big")
            pseudoheader = bytes(raw[26:34]) + struct.pack("!BBH", 0, lp.IP_PROTOCOL_TCP, total_len - 20)
            raw[50:52] = struct.pack("!H", lp.checksum(pseudoheader + bytes(raw[34 : 14 + total_len])))
        if fix_ip:
            raw[24:26] = b"\x00\x00"
            raw[24:26] = struct.pack("!H", lp.checksum(bytes(raw[14:34])))
        return bytes(raw)

    invalid = [
        valid[:40],
        mutated(16, b"\x00\x13", fix_ip=True),
        mutated(16, b"\xff\xff", fix_ip=True),
        mutated(20, b"\x20\x00", fix_ip=True),
        mutated(20, b"\x40\x01", fix_ip=True),
        mutated(50, b"\xff\xff"),
        mutated(46, b"\x40", fix_tcp=True),
        mutated(46, b"\xf0", fix_tcp=True),
        mutated(55, b"\x01", fix_tcp=True),
        mutated(55, b"\x05", fix_tcp=True),
        mutated(56, b"\x00\x00", fix_tcp=True),
        mutated(26, b"\x0a\x00\x00\x03", fix_ip=True),
        mutated(47, bytes([lp.TCP_SYN | lp.TCP_FIN]), fix_tcp=True),
        mutated(47, bytes([lp.TCP_SYN | lp.TCP_RST]), fix_tcp=True),
        tcp_frame(payload=b"SYN data is not supported"),
    ]
    for index, raw in enumerate(invalid):
        peer = lp.Peer()
        expect(peer.handle(raw) == [] and not peer.ledger.tcp_connections, f"malformed TCP case {index} reached listener")
    peer = lp.Peer()
    connection = open_tcp(peer)
    peer.handle(tcp_frame(connection.receive_sequence, connection.send_sequence + 1, lp.TCP_ACK, b"unacknowledgeable"))
    expect(not connection.received, "ACK for unsent bytes admitted payload")


def check_tcp_retransmission_timer() -> None:
    peer = lp.Peer()
    connection = open_tcp(peer)
    original = b"real echoed bytes survive a dropped frame"
    lost = peer.handle(tcp_frame(connection.receive_sequence, connection.send_sequence, lp.TCP_ACK, original))
    expect(len(lost) == 1 and connection.sent == original and not connection.acknowledged, "drop fixture did not leave actual unacknowledged data")
    deadline = connection.retry_deadline
    assert deadline is not None
    expect(peer.retransmit(deadline - 0.001) == [], "peer retried before timer deadline")
    retry = peer.retransmit(deadline)
    expect(retry == lost, "lost echo retry did not preserve wire bytes and sequence")
    expect(connection.sent == original and connection.received == original and connection.retransmissions == 1, "retransmission changed application delivery counters")
    peer.handle(tcp_frame(connection.receive_sequence, connection.send_sequence, lp.TCP_ACK))
    expect(connection.acknowledged == original and peer.retransmit(deadline + 10) == [], "acknowledged data was retransmitted")
    fin = peer.handle(tcp_frame(connection.receive_sequence, connection.send_sequence, lp.TCP_FIN | lp.TCP_ACK))
    assert connection.retry_deadline is not None
    expect(peer.retransmit(connection.retry_deadline) == fin, "lost FIN was not retransmitted")
    peer.handle(tcp_frame(connection.receive_sequence, connection.send_sequence, lp.TCP_ACK))
    expect(peer.retransmit(deadline + 100) == [], "closed connection was retransmitted")
    peer = lp.Peer()
    connection = open_tcp(peer)
    peer.handle(tcp_frame(connection.receive_sequence, connection.send_sequence, lp.TCP_ACK, b"first"))
    peer.handle(tcp_frame(connection.receive_sequence, connection.peer_initial_sequence + 1, lp.TCP_ACK, b"second"))
    assert connection.retry_deadline is not None
    retry = peer.retransmit(connection.retry_deadline)
    packet = lp.decode(retry[0])
    expect(packet is not None and packet.tcp_sequence == connection.peer_initial_sequence + 1 and packet.tcp_acknowledgment == connection.receive_sequence and packet.tcp_payload == b"firstsecond", "gap recovery did not use oldest real bytes with latest ACK")
    expect(connection.received == b"firstsecond" and connection.sent == b"firstsecond", "gap recovery fabricated delivery")
    peer = lp.Peer()
    original_synack = peer.handle(tcp_frame())
    connection = peer.ledger.tcp_connections[0]
    for _ in range(lp.TCP_RETRY_LIMIT):
        assert connection.retry_deadline is not None
        expect(peer.retransmit(connection.retry_deadline) == original_synack, "lost SYN-ACK was not faithfully retransmitted")
    assert connection.retry_deadline is not None
    expect(peer.retransmit(connection.retry_deadline + 100) == [] and connection.retransmissions == lp.TCP_RETRY_LIMIT, "retry limit was unbounded")


def qualified_ledger() -> lp.Ledger:
    peer = lp.Peer()
    peer.arp_request_for_guest()
    peer.handle(lp.ethernet(lp.PEER_MAC, GUEST_MAC, lp.ETHERTYPE_ARP, lp.arp(lp.ARP_REPLY, GUEST_MAC, lp.GUEST_IP, lp.PEER_MAC, lp.PEER_IP)))
    for sequence in range(1, lp.ECHO_COUNT + 1):
        peer.echo_request(lp.ECHO_IDENTIFIER, sequence, lp.ECHO_PAYLOAD)
        peer.handle(lp.ethernet(lp.PEER_MAC, GUEST_MAC, lp.ETHERTYPE_IPV4, lp.ipv4(lp.GUEST_IP, lp.PEER_IP, lp.IP_PROTOCOL_ICMP, lp.icmp_echo(lp.ICMP_ECHO_REPLY, lp.ECHO_IDENTIFIER, sequence, lp.ECHO_PAYLOAD))))
    connection = open_tcp(peer)
    payload = bytes((index * 37 + 11) & 255 for index in range(lp.TCP_STREAM_LIMIT))
    for offset in range(0, len(payload), 1024):
        peer.handle(tcp_frame(connection.receive_sequence, connection.send_sequence, lp.TCP_ACK, payload[offset : offset + 1024]))
    peer.handle(tcp_frame(connection.receive_sequence, connection.send_sequence, lp.TCP_FIN | lp.TCP_ACK))
    peer.handle(tcp_frame(connection.receive_sequence, connection.send_sequence, lp.TCP_ACK))
    peer.handle(tcp_frame(port=lp.TCP_REFUSED_PORT, source_port=50001))
    return peer.ledger


def check_tcp_qualification_controls() -> None:
    ledger = qualified_ledger()
    lp.qualify_tcp_ledger(ledger, GUEST_MAC)
    controls: list[lp.Ledger] = []
    malformed_peer = lp.Peer()
    malformed_peer.ledger = copy.deepcopy(ledger)
    expect(malformed_peer.handle(b"runt") == [] and malformed_peer.ledger.malformed_received == 1, "runt frame was not counted")
    controls.append(malformed_peer.ledger)
    for attribute in ("received", "sent", "acknowledged"):
        altered = copy.deepcopy(ledger)
        getattr(altered.tcp_connections[0], attribute)[100] ^= 1
        controls.append(altered)
    for attribute in ("handshake", "guest_fin", "peer_fin", "closed"):
        altered = copy.deepcopy(ledger)
        setattr(altered.tcp_connections[0], attribute, False)
        controls.append(altered)
    altered = copy.deepcopy(ledger)
    altered.tcp_connections *= 2
    controls.append(altered)
    for direction in ("received", "sent"):
        frames = getattr(ledger, direction)
        for index, frame in enumerate(frames):
            if frame.ip_protocol == lp.IP_PROTOCOL_TCP or frame.ip_protocol == lp.IP_PROTOCOL_ICMP:
                altered = copy.deepcopy(ledger)
                del getattr(altered, direction)[index]
                # A repeated SYN is legitimate redundant evidence.
                redundant_handshake_ack = direction == "received" and frame.tcp_flags == lp.TCP_ACK and not frame.tcp_payload and frame.tcp_sequence == ledger.tcp_connections[0].guest_initial_sequence + 1
                if frame.tcp_flags not in (lp.TCP_SYN, lp.TCP_SYN | lp.TCP_ACK) and not redundant_handshake_ack:
                    controls.append(altered)
        index = next(index for index, frame in enumerate(frames) if frame.tcp_payload)
        altered = copy.deepcopy(ledger)
        frame = getattr(altered, direction)[index]
        getattr(altered, direction)[index] = dataclasses.replace(frame, ip_payload=frame.ip_payload[:-1] + bytes([frame.ip_payload[-1] ^ 1]))
        controls.append(altered)
        altered = copy.deepcopy(ledger)
        frame = getattr(altered, direction)[index]
        getattr(altered, direction)[index] = dataclasses.replace(frame, tcp_payload=b"bad" + frame.tcp_payload[3:])
        controls.append(altered)
    for extra in (
        lp.decode(tcp_frame(port=80)),
        lp.decode(tcp_frame(destination_ip=bytes([10, 0, 0, 3]))),
        lp.decode(lp.ethernet(lp.BROADCAST, GUEST_MAC, lp.ETHERTYPE_ARP, lp.arp(lp.ARP_REQUEST, GUEST_MAC, lp.GUEST_IP, bytes(6), bytes([10, 0, 0, 3])))),
        next(frame for frame in ledger.received if frame.ip_protocol == lp.IP_PROTOCOL_ICMP),
    ):
        assert extra is not None
        altered = copy.deepcopy(ledger)
        altered.received.append(extra)
        controls.append(altered)
    for direction, flags in (("received", lp.TCP_SYN), ("sent", lp.TCP_SYN | lp.TCP_ACK)):
        altered = copy.deepcopy(ledger)
        setattr(altered, direction, [frame for frame in getattr(altered, direction) if frame.tcp_flags != flags])
        controls.append(altered)
    for direction in ("received", "sent"):
        altered = copy.deepcopy(ledger)
        frames = getattr(altered, direction)
        index = next(index for index, frame in enumerate(frames) if frame.tcp_payload)
        frame = frames[index]
        payload = bytes([frame.tcp_payload[0] ^ 1]) + frame.tcp_payload[1:]
        segment = lp.tcp(frame.ip_source, frame.ip_destination, frame.tcp_source_port, frame.tcp_destination_port, frame.tcp_sequence, frame.tcp_acknowledgment, frame.tcp_flags, payload, frame.tcp_window, frame.tcp_options)
        replacement = lp.decode(lp.ethernet(frame.destination, frame.source, lp.ETHERTYPE_IPV4, lp.ipv4(frame.ip_source, frame.ip_destination, lp.IP_PROTOCOL_TCP, segment)))
        assert replacement is not None
        frames[index] = replacement
        controls.append(altered)
    altered = copy.deepcopy(ledger)
    index = next(index for index, frame in enumerate(altered.received) if frame.ip_protocol == lp.IP_PROTOCOL_ICMP)
    altered.received[index] = dataclasses.replace(altered.received[index], icmp_payload=b"wrong")
    controls.append(altered)
    for index, altered in enumerate(controls):
        try:
            lp.qualify_tcp_ledger(altered, GUEST_MAC)
        except ValueError:
            continue
        fail(f"TCP ledger qualification accepted corrupted evidence control {index}")


def check_observer_failure() -> None:
    checker = load_script("network_observer_control", "check/check-sel4-io-network-plane.py")
    finished = threading.Event()

    def failed_observer(*_args: object) -> None:
        finished.set()
        raise OSError("injected late observer failure")

    def completed_plane(**_kwargs: object) -> str:
        expect(finished.wait(timeout=2), "observer failure control never ran")
        return "nominal serial result"

    with patch.object(checker.link_peer, "serve", failed_observer), patch.object(checker, "run_plane", completed_plane), patch.object(checker, "match_marker_contract") as markers, patch.object(checker, "check_tcp_ledger") as ledger:
        try:
            checker.run_tcp_arm(Path("unused-image"), GUEST_MAC.hex(":"), None)
        except SystemExit as error:
            expect("peer observer failed" in str(error), "observer error did not propagate to gate verdict")
        else:
            fail("late observer failure passed the network gate")
        expect(not markers.called and not ledger.called, "incomplete observer evidence reached qualification")


def check_held_connection_reset_fixture() -> None:
    expect(lp.Peer().echo_probe_ready(), "ordinary peer ICMP probes were gated on TCP")
    peer = lp.Peer(hold_first_tcp=True)
    expect(not peer.echo_probe_ready(), "reset fixture ICMP began before any connection")
    held = open_tcp(peer)
    expect(not peer.echo_probe_ready(), "reset fixture ICMP began before driver recovery")
    expect(held.held, "first reset-fixture connection was not held")
    expect(peer.handle(tcp_frame(sequence=2000)) == [], "held connection abandoned before actual 1024 bytes")
    data = bytes((index * 37 + 11) & 255 for index in range(1024))
    replies = peer.handle(tcp_frame(held.receive_sequence, held.send_sequence, lp.TCP_ACK, data))
    expect(held.received == data and not held.sent and not held.acknowledged, "held fixture echoed or invented stream bytes")
    expect(all(not lp.decode(raw).tcp_payload for raw in replies), "held fixture sent echo data")
    expect(peer.handle(tcp_frame()) == [] and not held.abandoned, "same SYN sequence abandoned held session")
    peer.handle(tcp_frame(sequence=2000))
    expect(not peer.echo_probe_ready(), "reset fixture ICMP began before the fresh handshake completed")
    fresh = open_tcp(peer, sequence=2000)
    expect(peer.echo_probe_ready(), "reset fixture ICMP did not start after fresh handshake")
    expect(held.abandoned and not held.closed and not held.reset and fresh is not held and not fresh.held, "server abandonment was confused with wire FIN/reset")
    reply = peer.handle(tcp_frame(fresh.receive_sequence, fresh.send_sequence, lp.TCP_ACK, b"fresh actual bytes"))
    expect(b"".join(lp.decode(raw).tcp_payload for raw in reply) == b"fresh actual bytes", "fresh reset-fixture session did not echo actual bytes")
    expect(len(peer.ledger.tcp_connections) == 2 and held.received == data, "retired session evidence was overwritten")


def check_driver_reset_qualification_controls() -> None:
    payload = bytes((index * 37 + 11) & 255 for index in range(1024))
    fresh_ledger = qualified_ledger()
    # The first fixture's sequence differs from the fresh fixture's 100.
    first = lp.Peer(hold_first_tcp=True)
    held = open_tcp(first, sequence=2000)
    first.handle(tcp_frame(held.receive_sequence, held.send_sequence, lp.TCP_ACK, payload))
    held.abandoned = True
    ledger = dataclasses.replace(fresh_ledger, received=first.ledger.received + fresh_ledger.received, sent=first.ledger.sent + fresh_ledger.sent, tcp_connections=[held, fresh_ledger.tcp_connections[0]])
    lp.qualify_driver_reset_ledger(ledger, GUEST_MAC)
    controls = []
    altered = copy.deepcopy(ledger)
    altered.tcp_connections.append(copy.deepcopy(held))
    controls.append(altered)
    altered = copy.deepcopy(ledger)
    altered.tcp_connections[0].received[10] ^= 1
    controls.append(altered)
    altered = copy.deepcopy(ledger)
    altered.tcp_connections[0].abandoned = False
    controls.append(altered)
    for direction, predicate in (("received", lambda frame: bool(frame.tcp_payload)), ("sent", lambda frame: frame.tcp_flags == lp.TCP_ACK)):
        altered = copy.deepcopy(ledger)
        frames = getattr(altered, direction)
        index = next(index for index, frame in enumerate(frames) if predicate(frame))
        del frames[index]
        controls.append(altered)
    altered = copy.deepcopy(ledger)
    index = next(index for index, frame in enumerate(altered.received) if frame.tcp_payload)
    frame = altered.received[index]
    segment = lp.tcp(lp.GUEST_IP, lp.PEER_IP, frame.tcp_source_port, frame.tcp_destination_port, frame.tcp_sequence, frame.tcp_acknowledgment, frame.tcp_flags, b"wrong" + frame.tcp_payload[5:])
    replacement = lp.decode(lp.ethernet(lp.PEER_MAC, GUEST_MAC, lp.ETHERTYPE_IPV4, lp.ipv4(lp.GUEST_IP, lp.PEER_IP, lp.IP_PROTOCOL_TCP, segment)))
    assert replacement is not None
    altered.received[index] = replacement
    controls.append(altered)
    altered = copy.deepcopy(ledger)
    wrong = lp.decode(tcp_frame(port=80))
    assert wrong is not None
    altered.received.insert(0, wrong)
    controls.append(altered)
    for index, altered in enumerate(controls):
        try:
            lp.qualify_driver_reset_ledger(altered, GUEST_MAC)
        except ValueError:
            continue
        fail(f"driver reset qualifier accepted corrupted evidence {index}")


def main() -> None:
    check_checksums()
    check_frames_decode_and_pad()
    check_peer_answers_arp_and_echo()
    check_peer_learns_the_guest_and_pings_it()
    check_tcp_stream_and_close()
    check_tcp_partial_and_window()
    check_tcp_bounds_and_refusal()
    check_tcp_malformed()
    check_tcp_qualification_controls()
    check_tcp_retransmission_timer()
    check_observer_failure()
    check_held_connection_reset_fixture()
    check_driver_reset_qualification_controls()
    (kind,) = struct.unpack("!H", struct.pack("!H", lp.ETHERTYPE_ARP))
    expect(kind == lp.ETHERTYPE_ARP, "struct sanity")
    print("link peer check: ARP/ICMP preserved; bounded real TCP echo, checksums, stream delivery, windows, retransmissions, refusal, and close verified")


if __name__ == "__main__":
    main()
