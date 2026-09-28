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
from itertools import pairwise
from unittest.mock import patch
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

import link_peer as lp  # noqa: E402
import tcp_impairment_peer as tip  # noqa: E402
import tcp_listener_peer as tlp  # noqa: E402
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


# Scripted impairments. The model below is a reduced smoltcp client and the
# impairment probe's scenario sequence, run in virtual time against the pure
# peer. It is not evidence about the real guest: it proves the scripts and the
# qualifier agree on what a conforming stack does, so the controls can then show
# the qualifier refusing each corrupted variant of that run.

MODEL_MSS = 536
MODEL_TICK = 0.001
MODEL_LATENCY = 0.0005
MODEL_POLL = 0.01
MODEL_LIMITS = {port: 16 for port in tip.SCENARIOS}


@dataclasses.dataclass
class ModelSocket:
    """smoltcp's client behavior, reduced to what the impairment scripts exercise."""

    port: int
    local_port: int
    isn: int
    retry_limit: int
    syn_due: float = 0.0
    state: str = "syn-sent"
    peer_isn: int = 0
    tx: bytearray = dataclasses.field(default_factory=bytearray)
    snd_una: int = 0
    snd_nxt: int = 0
    fin_queued: bool = False
    fin_sent: bool = False
    fin_acked: bool = False
    remote_window: int = 0
    rx: bytearray = dataclasses.field(default_factory=bytearray)
    rcv_nxt: int = 0
    held: dict[int, int] = dataclasses.field(default_factory=dict)
    ranges: list[list[int]] = dataclasses.field(default_factory=list)
    peer_fin: bool = False
    advertised: int = tip.SOCKET_BUFFER_BYTES
    advertised_edge: int = tip.SOCKET_BUFFER_BYTES
    handshake_ack: bool = False
    ack_due: bool = False
    retransmit_timeout: float = 1.0
    retransmit_due: float | None = None
    last_ack: int = 0
    dup_acks: int = 0
    probe_due: float | None = None
    probe_delay: float = 1.0
    challenge_after: float = 0.0
    last_remote: float = 0.0
    high: int | None = None
    retries: int = 0
    terminal: str | None = None

    def frame(self, flags: int, offset: int = 0, payload: bytes = b"") -> bytes:
        syn = bool(flags & lp.TCP_SYN)
        window = tip.SOCKET_BUFFER_BYTES - len(self.rx)
        acknowledgment = (self.peer_isn + 1 + self.rcv_nxt + int(self.peer_fin)) & lp.TCP_SEQUENCE_MASK if flags & lp.TCP_ACK else 0
        if flags & lp.TCP_ACK:
            self.advertised = window
            self.advertised_edge = self.rcv_nxt + window
        sequence = self.isn if syn else self.isn + 1 + offset
        options = b"\x02\x04\x05\xb4" if syn else b""
        segment = lp.tcp(lp.GUEST_IP, lp.PEER_IP, self.local_port, self.port, sequence, acknowledgment, flags, payload, window, options)
        return lp.ethernet(lp.PEER_MAC, GUEST_MAC, lp.ETHERTYPE_IPV4, lp.ipv4(lp.GUEST_IP, lp.PEER_IP, lp.IP_PROTOCOL_TCP, segment))

    def guard(self, start: int, length: int) -> bool:
        """The service's egress retry budget: False means it aborts instead of sending."""
        if self.high is not None and start < self.high:
            if self.retries >= self.retry_limit:
                return False
            self.retries += 1
        self.high = start + length if self.high is None else max(self.high, start + length)
        return True

    def abort(self, reason: str) -> bytes:
        self.state = "closed"
        self.terminal = reason
        return self.frame(lp.TCP_RST | lp.TCP_ACK, self.snd_nxt)

    def write(self, data: bytes) -> int:
        count = min(len(data), tip.SOCKET_BUFFER_BYTES - len(self.tx))
        self.tx.extend(data[:count])
        return count

    def read(self) -> bytes:
        data = bytes(self.rx)
        self.rx.clear()
        window = tip.SOCKET_BUFFER_BYTES
        if data and window // 2 >= self.advertised:
            self.ack_due = True
        return data

    def tick(self, now: float) -> list[bytes]:
        if self.state == "syn-sent":
            if now < self.syn_due:
                return []
            self.syn_due = now + self.retransmit_timeout
            return [self.frame(lp.TCP_SYN)] if self.guard(0, 1) else [self.abort("timeout")]
        if self.state != "established":
            return []
        out: list[bytes] = []
        if self.handshake_ack:
            out.append(self.frame(lp.TCP_ACK))
            self.handshake_ack = False
        end = self.snd_una + len(self.tx)
        if (self.tx or self.fin_sent and not self.fin_acked) and now - self.last_remote >= tip.SOCKET_TIMEOUT_SECONDS:
            return [self.abort("timeout")]
        if self.retransmit_due is not None and now >= self.retransmit_due:
            self.snd_nxt = self.snd_una
            self.fin_sent = self.fin_acked
            self.retransmit_timeout = min(self.retransmit_timeout * 2, 60.0)
            self.retransmit_due = None
        while self.snd_nxt < end:
            size = min(MODEL_MSS, end - self.snd_nxt, self.snd_una + self.remote_window - self.snd_nxt)
            if size <= 0 or size < MODEL_MSS and self.snd_nxt > self.snd_una:
                break
            start = self.snd_nxt - self.snd_una
            if not self.guard(1 + self.snd_nxt, size):
                return [*out, self.abort("timeout")]
            out.append(self.frame(lp.TCP_ACK | lp.TCP_PSH, self.snd_nxt, bytes(self.tx[start : start + size])))
            self.snd_nxt += size
            self.ack_due = False
            if self.retransmit_due is None:
                self.retransmit_due = now + self.retransmit_timeout
        if self.remote_window == 0 and self.snd_nxt == self.snd_una < end:
            if self.probe_due is None:
                self.probe_due = now
            if now >= self.probe_due:
                if not self.guard(1 + self.snd_nxt, 1):
                    return [*out, self.abort("timeout")]
                out.append(self.frame(lp.TCP_ACK | lp.TCP_PSH, self.snd_nxt, bytes(self.tx[:1])))
                self.probe_delay = min(self.probe_delay * 2, 60.0)
                self.probe_due = now + self.probe_delay
                self.ack_due = False
        else:
            self.probe_due = None
            self.probe_delay = self.retransmit_timeout
        if self.fin_queued and not self.fin_sent and self.snd_nxt == end:
            if not self.guard(1 + end, 1):
                return [*out, self.abort("timeout")]
            out.append(self.frame(lp.TCP_ACK | lp.TCP_FIN, end))
            self.fin_sent = True
            self.ack_due = False
            if self.retransmit_due is None:
                self.retransmit_due = now + self.retransmit_timeout
        if self.ack_due:
            out.append(self.frame(lp.TCP_ACK, self.snd_nxt + int(self.fin_sent)))
            self.ack_due = False
        if self.fin_acked and self.peer_fin:
            self.state = "done"
        return out

    def receive(self, frame: lp.Frame, now: float) -> None:
        if self.state not in ("syn-sent", "established"):
            return
        self.last_remote = now
        if frame.tcp_flags & lp.TCP_RST:
            self.state = "closed"
            self.terminal = "reset"
            return
        if self.state == "syn-sent":
            if frame.tcp_flags == lp.TCP_SYN | lp.TCP_ACK and frame.tcp_acknowledgment == (self.isn + 1) & lp.TCP_SEQUENCE_MASK:
                self.peer_isn = frame.tcp_sequence
                self.remote_window = frame.tcp_window
                self.state = "established"
                self.handshake_ack = True
            return
        if not frame.tcp_flags & lp.TCP_ACK:
            return
        offset = (frame.tcp_sequence - self.peer_isn - 1) & lp.TCP_SEQUENCE_MASK
        payload = frame.tcp_payload
        if payload:
            left, right = self.rcv_nxt, self.advertised_edge
            if left == right or not (left <= offset < right or left < offset + len(payload) <= right):
                if now >= self.challenge_after:
                    self.challenge_after = now + 1.0
                    self.ack_due = True
                return
        acked = (frame.tcp_acknowledgment - self.isn - 1) & lp.TCP_SEQUENCE_MASK
        end = self.snd_una + len(self.tx)
        if self.snd_una <= acked <= end + int(self.fin_sent):
            if acked > self.snd_una or self.fin_sent and not self.fin_acked and acked == end + 1:
                del self.tx[: min(acked, end) - self.snd_una]
                self.snd_una = min(acked, end)
                self.fin_acked = self.fin_acked or self.fin_sent and acked == end + 1
                self.dup_acks = 0
                self.last_ack = acked
                self.remote_window = frame.tcp_window
                self.retransmit_timeout = 1.0
                in_flight = self.snd_nxt > self.snd_una or self.fin_sent and not self.fin_acked
                self.retransmit_due = now + self.retransmit_timeout if in_flight else None
            elif acked == self.last_ack and not payload and frame.tcp_window == self.remote_window and self.snd_nxt > self.snd_una:
                self.dup_acks += 1
                if self.dup_acks == 3:
                    self.snd_nxt = self.snd_una
            else:
                self.remote_window = frame.tcp_window
                self.last_ack = acked
            self.snd_nxt = max(self.snd_nxt, self.snd_una)
        if payload:
            low, high = max(offset, self.rcv_nxt), min(offset + len(payload), self.advertised_edge)
            if high > low:
                if low == self.rcv_nxt:
                    self.rx.extend(payload[low - offset : high - offset])
                    self.rcv_nxt = high
                else:
                    merged = sorted([*self.ranges, [low, high]])
                    ranges: list[list[int]] = []
                    for start, stop in merged:
                        if ranges and start <= ranges[-1][1]:
                            ranges[-1][1] = max(ranges[-1][1], stop)
                        else:
                            ranges.append([start, stop])
                    if len(ranges) > tip.ASSEMBLER_SEGMENTS:
                        return
                    self.ranges = ranges
                    self.held.update((index, payload[index - offset]) for index in range(low, high))
                while self.ranges and self.ranges[0][0] <= self.rcv_nxt:
                    start, stop = self.ranges.pop(0)
                    self.rx.extend(self.held[index] for index in range(self.rcv_nxt, stop) if index >= self.rcv_nxt)
                    self.rcv_nxt = max(self.rcv_nxt, stop)
            self.ack_due = True
        if frame.tcp_flags & lp.TCP_FIN and offset + len(payload) == self.rcv_nxt and not self.peer_fin:
            self.peer_fin = True
            self.ack_due = True


class ModelProbe:
    """The impairment probe's scenario sequence, driving one model socket at a time."""

    STEPS = (
        (tip.REORDER_PORT, "echo"),
        (tip.LOSS_PORT, "echo"),
        (tip.SILENT_PORT, "silent"),
        (tip.SILENT_PORT, "echo"),
        (tip.RECEIVE_WINDOW_PORT, "stall"),
        (tip.SEND_WINDOW_PORT, "echo"),
    )

    def __init__(self, limits: dict[int, int]) -> None:
        self.limits = limits
        self.step = -1
        self.socket: ModelSocket | None = None
        self.results: list[tuple[int, str, str | None, bytes, int | None]] = []
        self.done = False
        self.start(0.0)

    def start(self, now: float) -> None:
        self.step += 1
        if self.step == len(self.STEPS):
            self.done = True
            self.socket = None
            return
        port, self.mode = self.STEPS[self.step]
        self.socket = ModelSocket(port, 49152 + self.step, 0x47000000 + self.step * 0x100000, self.limits[port], syn_due=now)
        self.written = 0
        self.received = bytearray()
        self.stall_until: float | None = None
        self.queued: int | None = None

    def tick(self, now: float) -> list[bytes]:
        socket = self.socket
        if socket is None:
            return []
        if socket.state == "established":
            self.application(socket, now)
        out = socket.tick(now)
        if socket.state in ("done", "closed"):
            self.results.append((socket.port, self.mode, socket.terminal, bytes(self.received), self.queued))
            self.start(now)
        return out

    def application(self, socket: ModelSocket, now: float) -> None:
        if self.written < tip.STREAM_BYTES:
            count = socket.write(tip.STREAM[self.written :])
            if not count and self.queued is None and socket.port == tip.SEND_WINDOW_PORT:
                self.queued = self.written
            self.written += count
        if self.mode == "silent":
            return
        if self.mode == "stall":
            if self.written < tip.STREAM_BYTES:
                return
            if self.stall_until is None:
                self.stall_until = now + tip.RECEIVE_STALL_MS / 1000
            if now < self.stall_until:
                return
        self.received.extend(socket.read())
        if len(self.received) == tip.STREAM_BYTES:
            socket.fin_queued = True


def simulate(limits: dict[int, int] = MODEL_LIMITS) -> tuple[tip.ImpairmentPeer, ModelProbe]:
    peer = tip.ImpairmentPeer()
    probe = ModelProbe(limits)
    arp = lp.ethernet(lp.BROADCAST, GUEST_MAC, lp.ETHERTYPE_ARP, lp.arp(lp.ARP_REQUEST, GUEST_MAC, lp.GUEST_IP, bytes(6), lp.PEER_IP))
    to_peer: list[tuple[float, bytes]] = [(MODEL_LATENCY, arp)]
    to_guest: list[tuple[float, bytes]] = []
    next_poll = 0.0
    tick = 0
    drain = 0
    while drain < 500 and tick < 120_000:
        tick += 1
        drain += int(probe.done)
        now = tick * MODEL_TICK
        due = [raw for when, raw in to_peer if when <= now]
        to_peer = [(when, raw) for when, raw in to_peer if when > now]
        for raw in due:
            to_guest.extend((now + MODEL_LATENCY, reply) for reply in peer.handle(raw, now))
        if now >= next_poll:
            to_guest.extend((now + MODEL_LATENCY, reply) for reply in peer.poll(now))
            next_poll = now + MODEL_POLL
        due = [raw for when, raw in to_guest if when <= now]
        to_guest = [(when, raw) for when, raw in to_guest if when > now]
        for raw in due:
            frame = lp.decode(raw)
            if probe.socket is not None and frame is not None and frame.kind == "tcp" and frame.tcp_destination_port == probe.socket.local_port:
                probe.socket.receive(frame, now)
        to_peer.extend((now + MODEL_LATENCY, raw) for raw in probe.tick(now))
    expect(probe.done, f"the model guest did not finish every scenario: {peer.ledger.summary()}")
    return peer, probe


def impairment_tcp(port: int, sequence: int, acknowledgment: int, flags: int, payload: bytes = b"", window: int = tip.SOCKET_BUFFER_BYTES, source_port: int = 50000) -> bytes:
    return tcp_frame(sequence, acknowledgment, flags, payload, port=port, source_port=source_port, window=window)


def impairment_open(peer: tip.ImpairmentPeer, port: int, now: float = 0.0, source_port: int = 50000) -> tip.Connection:
    replies = peer.handle(impairment_tcp(port, 100, 0, lp.TCP_SYN, source_port=source_port), now)
    expect(len(replies) == 1, "an impairment SYN did not receive exactly one SYN-ACK")
    connection = peer.ledger.connections[-1]
    peer.handle(impairment_tcp(port, 101, connection.peer_isn + 1, lp.TCP_ACK, source_port=source_port), now)
    expect(connection.handshake, "an impairment handshake did not complete")
    return connection


def payload_offsets(replies: list[bytes], connection: tip.Connection) -> list[tuple[int, int]]:
    decoded = [lp.decode(raw) for raw in replies]
    return [((frame.tcp_sequence - connection.peer_isn - 1) & lp.TCP_SEQUENCE_MASK, len(frame.tcp_payload)) for frame in decoded if frame is not None and frame.tcp_payload]


def check_impairment_scripts() -> None:
    expect(tip.REORDER_RETAINED == tuple(sorted(index for phase in tip.REORDER_PHASES for index in phase[: phase.index(min(phase))][: tip.ASSEMBLER_SEGMENTS])), "retained reorder segments do not follow from the phases and the declared capacity")
    expect(all(len(phase[: phase.index(min(phase))]) <= tip.ASSEMBLER_SEGMENTS + 1 for phase in tip.REORDER_PHASES) and tip.REORDER_REFUSED == tip.REORDER_PHASES[-1][tip.ASSEMBLER_SEGMENTS], "the refused reorder segment is not the first beyond capacity")
    peer = tip.ImpairmentPeer()
    connection = impairment_open(peer, tip.REORDER_PORT)
    replies = peer.handle(impairment_tcp(tip.REORDER_PORT, 101, connection.peer_isn + 1, lp.TCP_ACK | lp.TCP_PSH, tip.STREAM[:1024]), 0.0)
    expect([offset // tip.REORDER_SEGMENT for offset, _ in payload_offsets(replies, connection)] == list(tip.REORDER_PHASES[0]), "the first reordering phase left in an undeclared order")
    expect(peer.poll(0.1) == [] and peer.poll(0.3) != [], "the reorder peer retransmitted before its timer or not at all")
    peer = tip.ImpairmentPeer()
    connection = impairment_open(peer, tip.LOSS_PORT)
    ack = connection.peer_isn + 1
    expect(len(peer.handle(impairment_tcp(tip.LOSS_PORT, 101, ack, lp.TCP_ACK, tip.STREAM[:536]), 0.0)) == 2, "undropped loss data was not echoed with its acknowledgment")
    expect(peer.handle(impairment_tcp(tip.LOSS_PORT, 101 + 536, ack, lp.TCP_ACK, tip.STREAM[536:1072]), 0.0) == [] and not peer.ledger.received[-1].delivered, "the declared guest loss was delivered")
    duplicates = peer.handle(impairment_tcp(tip.LOSS_PORT, 101 + 1072, ack, lp.TCP_ACK, tip.STREAM[1072:1608]), 0.0)
    expect(len(duplicates) == tip.LOSS_DUPLICATE_ACKS and len(set(duplicates)) == 1, "the first gap was not met with identical injected duplicate acknowledgments")
    expect(peer.handle(impairment_tcp(tip.LOSS_PORT, 101 + 1072, ack, lp.TCP_ACK, tip.STREAM[1072:1608]), 0.0) != duplicates, "duplicate acknowledgments were injected twice")
    peer = tip.ImpairmentPeer()
    connection = impairment_open(peer, tip.SILENT_PORT)
    ack = connection.peer_isn + 1
    replies = peer.handle(impairment_tcp(tip.SILENT_PORT, 101, ack, lp.TCP_ACK, tip.STREAM[:1500]), 0.0)
    expect(len(replies) == 1 and lp.decode(replies[0]).tcp_acknowledgment == 101 + tip.SILENT_ACCEPT_BYTES and connection.silent_since == 0.0, "the silent peer did not acknowledge exactly the declared prefix before falling silent")
    expect(peer.handle(impairment_tcp(tip.SILENT_PORT, 101 + 1500, ack, lp.TCP_ACK, tip.STREAM[1500:2000]), 1.0) == [] and peer.poll(20.0) == [], "the silent peer answered or retransmitted")
    fresh = impairment_open(peer, tip.SILENT_PORT, 21.0, source_port=50001)
    expect(not fresh.silent_first and len(peer.handle(impairment_tcp(tip.SILENT_PORT, 101, fresh.peer_isn + 1, lp.TCP_ACK, b"fresh", source_port=50001), 21.0)) == 1, "the fresh silent-port connection was not an ordinary echo")
    peer = tip.ImpairmentPeer()
    replies = peer.handle(impairment_tcp(tip.SEND_WINDOW_PORT, 100, 0, lp.TCP_SYN), 0.0)
    expect(lp.decode(replies[0]).tcp_window == 0, "the send-window SYN-ACK did not close the window")
    connection = peer.ledger.connections[0]
    peer.handle(impairment_tcp(tip.SEND_WINDOW_PORT, 101, connection.peer_isn + 1, lp.TCP_ACK), 0.0)
    probe = peer.handle(impairment_tcp(tip.SEND_WINDOW_PORT, 101, connection.peer_isn + 1, lp.TCP_ACK | lp.TCP_PSH, tip.STREAM[:1]), 0.5)
    answer = lp.decode(probe[0])
    expect(len(probe) == 1 and answer.tcp_acknowledgment == 101 and answer.tcp_window == 0 and not connection.received, "a zero-window probe byte was accepted or unanswered")
    expect(peer.poll(tip.SEND_HOLD_SECONDS - 0.01) == [], "the zero window opened before the declared hold")
    opened = peer.poll(tip.SEND_HOLD_SECONDS)
    expect(len(opened) == 1 and lp.decode(opened[0]).tcp_window == tip.STREAM_BYTES, "the zero window did not open with a window update after the hold")
    peer = tip.ImpairmentPeer()
    connection = impairment_open(peer, tip.RECEIVE_WINDOW_PORT)
    ack = connection.peer_isn + 1
    replies = peer.handle(impairment_tcp(tip.RECEIVE_WINDOW_PORT, 101, ack, lp.TCP_ACK | lp.TCP_PSH, tip.STREAM[:1024], window=512), 0.0)
    expect(payload_offsets(replies, connection) == [(0, 512)], "the echo exceeded the guest's advertised window")
    peer.handle(impairment_tcp(tip.RECEIVE_WINDOW_PORT, 101 + 1024, ack + 512, lp.TCP_ACK, window=0), 0.1)
    expect(peer.poll(0.1 + tip.PEER_PROBE_INITIAL_SECONDS - 0.01) == [], "a persist probe preceded the declared interval")
    probes = peer.poll(0.1 + tip.PEER_PROBE_INITIAL_SECONDS)
    expect(payload_offsets(probes, connection) == [(512, 1)] and connection.sent == 512, "the persist probe was not one unsent byte at the window edge")
    expect(peer.poll(0.1 + 3 * tip.PEER_PROBE_INITIAL_SECONDS - 0.01) == [] and len(peer.poll(0.1 + 3 * tip.PEER_PROBE_INITIAL_SECONDS + 0.001)) == 1, "persist probes did not back off by doubling")
    resumed = peer.handle(impairment_tcp(tip.RECEIVE_WINDOW_PORT, 101 + 1024, ack + 512, lp.TCP_ACK, window=2048), 4.0)
    expect(payload_offsets(resumed, connection) == [(512, 512)] and connection.probe_due is None, "a window update did not resume the echo or stop probing")


def check_impairment_simulation() -> tip.Ledger:
    peer, probe = simulate()
    summaries = tip.qualify_impairment_ledger(peer.ledger, GUEST_MAC, MODEL_LIMITS)
    expect(len(summaries) == len(tip.SCENARIOS), "the impairment qualifier did not summarize every scenario")
    outcomes = {(port, mode): (terminal, received, queued) for port, mode, terminal, received, queued in probe.results}
    expect(outcomes[(tip.SILENT_PORT, "silent")][0] == "timeout", "the model's silent connection did not time out")
    expect(all(received == tip.STREAM and terminal is None for (port, mode), (terminal, received, _) in outcomes.items() if mode != "silent"), "a model exchange did not receive the exact stream")
    expect(outcomes[(tip.SEND_WINDOW_PORT, "echo")][2] == tip.SOCKET_BUFFER_BYTES, "the model did not queue exactly its transmit buffer behind the zero window")
    guarded, _ = simulate({**MODEL_LIMITS, tip.SILENT_PORT: 2})
    tip.qualify_impairment_ledger(guarded.ledger, GUEST_MAC, {**MODEL_LIMITS, tip.SILENT_PORT: 2})
    return peer.ledger


def forge(observation: tip.Observation, **changes: object) -> tip.Observation:
    frame = observation.frame
    fields: dict[str, object] = {"sequence": frame.tcp_sequence, "acknowledgment": frame.tcp_acknowledgment, "flags": frame.tcp_flags, "payload": frame.tcp_payload, "window": frame.tcp_window}
    fields.update(changes)
    assert frame.ip_source is not None and frame.ip_destination is not None and frame.tcp_source_port is not None and frame.tcp_destination_port is not None
    segment = lp.tcp(frame.ip_source, frame.ip_destination, frame.tcp_source_port, frame.tcp_destination_port, fields["sequence"], fields["acknowledgment"], fields["flags"], fields["payload"], fields["window"], frame.tcp_options)  # type: ignore[arg-type]
    decoded = lp.decode(lp.ethernet(frame.destination, frame.source, lp.ETHERTYPE_IPV4, lp.ipv4(frame.ip_source, frame.ip_destination, lp.IP_PROTOCOL_TCP, segment)))
    assert decoded is not None
    return dataclasses.replace(observation, frame=decoded)


def check_impairment_controls(ledger: tip.Ledger) -> None:
    def flow(altered: tip.Ledger, port: int, index: int = 0) -> tuple[tip.Flow, list[int], list[int]]:
        flows = [entry for entry in tip.split_flows(altered, GUEST_MAC) if entry.port == port]
        tip.handshake(flows[index])
        guest = [altered.received.index(entry) for entry in flows[index].guest]
        peer = [altered.sent.index(entry) for entry in flows[index].peer]
        return flows[index], guest, peer

    def refused(fragment: str, mutate: object, limits: dict[int, int] = MODEL_LIMITS) -> None:
        altered = copy.deepcopy(ledger)
        mutate(altered)  # type: ignore[operator]
        try:
            tip.qualify_impairment_ledger(altered, GUEST_MAC, limits)
        except ValueError as error:
            expect(fragment in str(error), f"an impairment control was refused for another reason: expected {fragment!r}, got {error}")
            return
        fail(f"impairment qualification accepted corrupted evidence: {fragment}")

    def reorder_segment(altered: tip.Ledger, index: int) -> list[int]:
        found, _, peer = flow(altered, tip.REORDER_PORT)
        return [position for position in peer if altered.sent[position].frame.tcp_payload and found.peer_offset(altered.sent[position].frame) == index * tip.REORDER_SEGMENT]

    def drop_refused_retransmission(altered: tip.Ledger) -> None:
        del altered.sent[reorder_segment(altered, tip.REORDER_REFUSED)[1]]

    def repeat_retained(altered: tip.Ledger) -> None:
        position = reorder_segment(altered, 5)[0]
        altered.sent.insert(position + 1, dataclasses.replace(altered.sent[position], order=altered.sent[position].order))

    def swap_phase(altered: tip.Ledger) -> None:
        first, second = reorder_segment(altered, 1)[0], reorder_segment(altered, 3)[0]
        altered.sent[first], altered.sent[second] = dataclasses.replace(altered.sent[second], order=altered.sent[first].order), dataclasses.replace(altered.sent[first], order=altered.sent[second].order)

    def deliver_drop(altered: tip.Ledger) -> None:
        position = next(index for index, entry in enumerate(altered.received) if not entry.delivered)
        altered.received[position] = dataclasses.replace(altered.received[position], delivered=True)

    def remove_duplicates(altered: tip.Ledger) -> None:
        found, _, peer = flow(altered, tip.LOSS_PORT)
        pure = [position for position in peer if altered.sent[position].frame.tcp_flags == lp.TCP_ACK and not altered.sent[position].frame.tcp_payload]
        runs = [position for previous, position in pairwise(pure) if position == previous + 1 and altered.sent[position].frame == altered.sent[previous].frame]
        for position in sorted(runs, reverse=True):
            del altered.sent[position]

    def deliver_withheld(altered: tip.Ledger) -> None:
        position = next(index for index, entry in enumerate(altered.sent) if not entry.delivered)
        altered.sent[position] = dataclasses.replace(altered.sent[position], delivered=True)

    def slow_loss(altered: tip.Ledger) -> None:
        _, guest, _ = flow(altered, tip.LOSS_PORT)
        last = guest[-1]
        altered.received[last] = dataclasses.replace(altered.received[last], time=altered.received[last].time + tip.LOSS_ELAPSED_SECONDS)

    def remove_reset(altered: tip.Ledger) -> None:
        _, guest, _ = flow(altered, tip.SILENT_PORT)
        del altered.received[guest[-1]]

    def late_reset(altered: tip.Ledger) -> None:
        _, guest, _ = flow(altered, tip.SILENT_PORT)
        altered.received[guest[-1]] = dataclasses.replace(altered.received[guest[-1]], time=altered.received[guest[-1]].time + tip.SOCKET_TIMEOUT_SECONDS)

    def answer_silence(altered: tip.Ledger) -> None:
        _, guest, peer = flow(altered, tip.SILENT_PORT)
        last = altered.sent[peer[-1]]
        altered.sent.insert(peer[-1] + 1, dataclasses.replace(last, order=altered.received[guest[-1]].order - 1))

    def undeliver_before_silence(altered: tip.Ledger) -> None:
        _, guest, _ = flow(altered, tip.SILENT_PORT)
        altered.received[guest[1]] = dataclasses.replace(altered.received[guest[1]], delivered=False)

    def remove_fresh(altered: tip.Ledger) -> None:
        found, guest, peer = flow(altered, tip.SILENT_PORT, 1)
        for position in sorted(peer, reverse=True):
            del altered.sent[position]
        for position in sorted(guest, reverse=True):
            del altered.received[position]

    def open_zero_windows(altered: tip.Ledger) -> None:
        _, guest, _ = flow(altered, tip.RECEIVE_WINDOW_PORT)
        for position in guest:
            if altered.received[position].frame.tcp_window == 0 and not altered.received[position].frame.tcp_flags & lp.TCP_RST:
                altered.received[position] = forge(altered.received[position], window=1)

    def remove_probes(altered: tip.Ledger) -> None:
        found, _, peer = flow(altered, tip.RECEIVE_WINDOW_PORT)
        for position in sorted(peer, reverse=True):
            frame = altered.sent[position].frame
            if len(frame.tcp_payload) == 1:
                del altered.sent[position]

    def accept_probe(altered: tip.Ledger) -> None:
        found, guest, peer = flow(altered, tip.RECEIVE_WINDOW_PORT)
        probe = next(altered.sent[position] for position in peer if len(altered.sent[position].frame.tcp_payload) == 1)
        position = next(position for position in guest if altered.received[position].order > probe.order)
        answer = altered.received[position]
        altered.received[position] = forge(answer, acknowledgment=(answer.frame.tcp_acknowledgment + 1) & lp.TCP_SEQUENCE_MASK, window=0)

    def open_synack(altered: tip.Ledger) -> None:
        _, _, peer = flow(altered, tip.SEND_WINDOW_PORT)
        altered.sent[peer[0]] = forge(altered.sent[peer[0]], window=tip.STREAM_BYTES)

    def crowd_probes(altered: tip.Ledger) -> None:
        found, guest, _ = flow(altered, tip.SEND_WINDOW_PORT)
        position = next(position for position in guest if altered.received[position].frame.tcp_payload)
        probe = altered.received[position]
        altered.received.insert(position + 1, dataclasses.replace(probe, time=probe.time + 0.1))

    def widen_probe(altered: tip.Ledger) -> None:
        _, guest, _ = flow(altered, tip.SEND_WINDOW_PORT)
        position = next(position for position in guest if altered.received[position].frame.tcp_payload)
        altered.received[position] = forge(altered.received[position], payload=tip.STREAM[:2])

    def widen_window(altered: tip.Ledger) -> None:
        _, guest, _ = flow(altered, tip.LOSS_PORT)
        altered.received[guest[-1]] = forge(altered.received[guest[-1]], window=tip.SOCKET_BUFFER_BYTES + 1)

    def corrupt_stream(altered: tip.Ledger) -> None:
        _, guest, _ = flow(altered, tip.LOSS_PORT)
        position = next(position for position in guest if altered.received[position].frame.tcp_payload)
        payload = altered.received[position].frame.tcp_payload
        altered.received[position] = forge(altered.received[position], payload=bytes([payload[0] ^ 1]) + payload[1:])

    def foreign_source(altered: tip.Ledger) -> None:
        altered.received[0] = dataclasses.replace(altered.received[0], frame=dataclasses.replace(altered.received[0].frame, source=bytes(6)))

    def malformed(altered: tip.Ledger) -> None:
        altered.malformed_received = 1

    def acknowledge_withheld(altered: tip.Ledger) -> None:
        withheld = next(entry for entry in altered.sent if not entry.delivered)
        found, guest, _ = flow(altered, tip.LOSS_PORT)
        position = next(position for position in guest if altered.received[position].order > withheld.order)
        target = (found.peer_offset(withheld.frame) + len(withheld.frame.tcp_payload) + found.peer_isn + 1) & lp.TCP_SEQUENCE_MASK
        altered.received[position] = forge(altered.received[position], acknowledgment=target)

    refused("was never retransmitted", drop_refused_retransmission)
    refused("was retransmitted", repeat_retained)
    refused("undeclared order", swap_phase)
    refused("drops differ", deliver_drop)
    refused("duplicate acknowledgments", remove_duplicates)
    refused("withhold", deliver_withheld)
    refused("never received", acknowledge_withheld)
    refused("lossy exchange took", slow_loss)
    refused("declared retry limit", lambda altered: None, {**MODEL_LIMITS, tip.LOSS_PORT: 1})
    refused("exactly one reset", remove_reset)
    refused("fell silent", late_reset)
    refused("silent", answer_silence)
    refused("before falling silent", undeliver_before_silence)
    refused("fresh silent-port connection", remove_fresh)
    refused("zero receive window", open_zero_windows)
    refused("persist probes", remove_probes)
    refused("beyond its zero window", accept_probe)
    refused("zero window", open_synack)
    refused("spaced below", crowd_probes)
    refused("one-byte probe", widen_probe)
    refused("receive buffer", widen_window)
    refused("declared stream bytes", corrupt_stream)
    refused("source MAC", foreign_source)
    refused("unclassifiable", malformed)


# The external-listener script against a model guest: a listening smoltcp stack
# behind the service's ingress and egress policy, and the listener probe's
# program. Like the impairment model above, it exists so the script and the
# qualifier agree on what a conforming guest does before the controls corrupt it.

LISTENER_MSS = 536
TIME_WAIT_SECONDS = 10.0
LISTENER_POOL = 4


@dataclasses.dataclass
class ListenerModelSocket:
    """smoltcp's socket, reduced to the in-order exchanges and closes the listener script drives."""

    local_port: int
    remote_port: int = 0
    isn: int = 0
    state: str = "closed"
    peer_isn: int = 0
    tx: bytearray = dataclasses.field(default_factory=bytearray)
    snd_una: int = 0
    snd_nxt: int = 0
    fin_queued: bool = False
    fin_sent: bool = False
    fin_acked: bool = False
    rx: bytearray = dataclasses.field(default_factory=bytearray)
    rcv_nxt: int = 0
    peer_fin: bool = False
    peer_fin_first: bool = False
    remote_window: int = 0
    ack_due: bool = False
    syn_due: float | None = None
    retransmit_due: float | None = None
    probe_due: float | None = None
    probe_delay: float = 1.0
    time_wait_until: float | None = None
    reset: bool = False

    def frame(self, flags: int, offset: int = 0, payload: bytes = b"") -> bytes:
        syn = bool(flags & lp.TCP_SYN)
        acknowledgment = (self.peer_isn + 1 + self.rcv_nxt + int(self.peer_fin)) & lp.TCP_SEQUENCE_MASK if flags & lp.TCP_ACK else 0
        sequence = self.isn if syn else self.isn + 1 + offset
        options = b"\x02\x04\x02\x18" if syn else b""
        window = tlp.SOCKET_BUFFER_BYTES - len(self.rx)
        segment = lp.tcp(lp.GUEST_IP, lp.PEER_IP, self.local_port, self.remote_port, sequence, acknowledgment, flags, payload, window, options)
        return lp.ethernet(lp.PEER_MAC, GUEST_MAC, lp.ETHERTYPE_IPV4, lp.ipv4(lp.GUEST_IP, lp.PEER_IP, lp.IP_PROTOCOL_TCP, segment))

    @property
    def end(self) -> int:
        return self.snd_una + len(self.tx)

    def write(self, data: bytes) -> int:
        count = min(len(data), tlp.SOCKET_BUFFER_BYTES - len(self.tx))
        self.tx.extend(data[:count])
        return count

    def read(self) -> bytes:
        data = bytes(self.rx)
        self.rx.clear()
        if data:
            self.ack_due = True
        return data

    def abort(self) -> bytes:
        self.state = "closed"
        self.reset = True
        return self.frame(lp.TCP_RST | lp.TCP_ACK, self.snd_nxt + int(self.fin_sent))

    def tick(self, now: float) -> list[bytes]:
        if self.state in ("syn-sent", "syn-received"):
            if self.syn_due is None or now < self.syn_due:
                return []
            self.syn_due = now + 1.0
            return [self.frame(lp.TCP_SYN if self.state == "syn-sent" else lp.TCP_SYN | lp.TCP_ACK)]
        if self.state != "open":
            return []
        if self.time_wait_until is not None:
            if now >= self.time_wait_until:
                self.state = "closed"
                return []
            if not self.ack_due:
                return []
            self.ack_due = False
            return [self.frame(lp.TCP_ACK, self.end + 1)]
        out: list[bytes] = []
        if self.retransmit_due is not None and now >= self.retransmit_due:
            self.snd_nxt = self.snd_una
            self.fin_sent = self.fin_acked
            self.retransmit_due = None
        while self.snd_nxt < self.end:
            size = min(LISTENER_MSS, self.end - self.snd_nxt, self.snd_una + self.remote_window - self.snd_nxt)
            if size <= 0:
                break
            start = self.snd_nxt - self.snd_una
            out.append(self.frame(lp.TCP_ACK | lp.TCP_PSH, self.snd_nxt, bytes(self.tx[start : start + size])))
            self.snd_nxt += size
            self.ack_due = False
            self.retransmit_due = self.retransmit_due or now + 1.0
        if self.remote_window == 0 and self.snd_nxt == self.snd_una < self.end:
            self.probe_due = self.probe_due or now + self.probe_delay
            if now >= self.probe_due:
                out.append(self.frame(lp.TCP_ACK | lp.TCP_PSH, self.snd_nxt, bytes(self.tx[:1])))
                self.probe_delay *= 2
                self.probe_due = now + self.probe_delay
                self.ack_due = False
        else:
            self.probe_due = None
            self.probe_delay = 1.0
        if self.fin_queued and not self.fin_sent and self.snd_nxt == self.end:
            out.append(self.frame(lp.TCP_ACK | lp.TCP_FIN, self.end))
            self.fin_sent = True
            self.ack_due = False
            self.retransmit_due = self.retransmit_due or now + 1.0
        if self.ack_due:
            out.append(self.frame(lp.TCP_ACK, self.snd_nxt + int(self.fin_sent)))
            self.ack_due = False
        self.settle(now)
        return out

    def settle(self, now: float) -> None:
        if self.fin_sent and self.fin_acked and self.peer_fin:
            if self.peer_fin_first:
                self.state = "closed"
            elif self.time_wait_until is None:
                self.time_wait_until = now + TIME_WAIT_SECONDS

    def receive(self, frame: lp.Frame, now: float) -> None:
        if frame.tcp_flags & lp.TCP_RST:
            self.state = "closed"
            self.reset = True
            return
        if self.state == "listen":
            if frame.tcp_flags == lp.TCP_SYN:
                self.remote_port = frame.tcp_source_port or 0
                self.peer_isn = frame.tcp_sequence
                self.remote_window = frame.tcp_window
                self.state = "syn-received"
                self.syn_due = now
            return
        if self.state == "syn-sent":
            if frame.tcp_flags == lp.TCP_SYN | lp.TCP_ACK and frame.tcp_acknowledgment == (self.isn + 1) & lp.TCP_SEQUENCE_MASK:
                self.peer_isn = frame.tcp_sequence
                self.remote_window = frame.tcp_window
                self.state = "open"
                self.ack_due = True
            return
        if self.state == "syn-received":
            if frame.tcp_flags == lp.TCP_SYN:
                self.syn_due = now
                return
            if not frame.tcp_flags & lp.TCP_ACK or frame.tcp_acknowledgment != (self.isn + 1) & lp.TCP_SEQUENCE_MASK:
                return
            self.state = "open"
        if self.state != "open" or not frame.tcp_flags & lp.TCP_ACK:
            return
        acked = (frame.tcp_acknowledgment - self.isn - 1) & lp.TCP_SEQUENCE_MASK
        if self.snd_una <= acked <= self.end + int(self.fin_sent):
            end = self.end
            data_acked = min(acked, end)
            if data_acked > self.snd_una:
                del self.tx[: data_acked - self.snd_una]
                self.snd_una = data_acked
            self.fin_acked = self.fin_acked or self.fin_sent and acked == end + 1
            self.snd_nxt = max(self.snd_nxt, self.snd_una)
            self.remote_window = frame.tcp_window
            in_flight = self.snd_nxt > self.snd_una or self.fin_sent and not self.fin_acked
            self.retransmit_due = now + 1.0 if in_flight else None
        offset = (frame.tcp_sequence - self.peer_isn - 1) & lp.TCP_SEQUENCE_MASK
        payload = frame.tcp_payload
        if payload:
            if offset == self.rcv_nxt and not self.peer_fin:
                accepted = payload[: tlp.SOCKET_BUFFER_BYTES - len(self.rx)]
                self.rx.extend(accepted)
                self.rcv_nxt += len(accepted)
            self.ack_due = True
        if frame.tcp_flags & lp.TCP_FIN:
            if offset + len(payload) == self.rcv_nxt and not self.peer_fin:
                self.peer_fin = True
                self.peer_fin_first = not self.fin_sent
            self.ack_due = True
        self.settle(now)


class ListenerModelGuest:
    """The service's listener policy over a four-socket pool, and the probe's program."""

    def __init__(self) -> None:
        self.sockets: list[ListenerModelSocket] = []
        self.armed: ListenerModelSocket | None = None
        self.children: list[ListenerModelSocket] = []
        self.listening = False
        self.resolved = False
        self.unadmitted = 0
        self.excess = 0
        self.closes: list[tuple[str, int, int]] = []
        self.out: list[bytes] = [lp.ethernet(lp.BROADCAST, GUEST_MAC, lp.ETHERTYPE_ARP, lp.arp(lp.ARP_REQUEST, GUEST_MAC, lp.GUEST_IP, bytes(6), lp.PEER_IP))]
        self.now = 0.0
        self.isn = 0x47100000
        self.done = False
        self.program = self.run()

    def allocate(self, local_port: int) -> ListenerModelSocket | None:
        if len(self.sockets) >= LISTENER_POOL:
            return None
        self.isn += 0x100000
        socket_ = ListenerModelSocket(local_port, isn=self.isn)
        self.sockets.append(socket_)
        return socket_

    def arm(self) -> None:
        if self.listening and self.armed is None and len(self.children) < tlp.ACCEPTED_LIMIT:
            self.armed = self.allocate(tlp.LISTEN_PORT)
            if self.armed is not None:
                self.armed.state = "listen"

    def receive(self, raw: bytes, now: float) -> None:
        frame = lp.decode(raw)
        if frame is None or frame.destination not in (GUEST_MAC, lp.BROADCAST):
            return
        if frame.kind == "arp-reply" and frame.arp_sender_ip == lp.PEER_IP:
            self.resolved = True
            return
        if frame.kind != "tcp" or frame.ip_destination != lp.GUEST_IP:
            return
        admitted = frame.ip_source == lp.PEER_IP
        if frame.tcp_destination_port == tlp.LISTEN_PORT and not admitted:
            self.unadmitted += int(frame.tcp_flags == lp.TCP_SYN)
            return
        if not admitted:
            return
        match = next((entry for entry in self.sockets if entry.state not in ("listen", "closed") and entry.local_port == frame.tcp_destination_port and entry.remote_port == frame.tcp_source_port), None)
        if match is not None:
            match.receive(frame, now)
            return
        if frame.tcp_flags != lp.TCP_SYN or frame.tcp_destination_port != tlp.LISTEN_PORT:
            return
        if self.armed is not None and self.armed.state == "listen":
            self.armed.receive(frame, now)
            return
        # No listening socket: smoltcp answers with a reset, and the egress guard
        # publishes it only for the admitted peer at the declared endpoint.
        self.excess += 1
        segment = lp.tcp(lp.GUEST_IP, lp.PEER_IP, tlp.LISTEN_PORT, frame.tcp_source_port or 0, 0, frame.tcp_sequence + 1, lp.TCP_RST | lp.TCP_ACK, b"", 0)
        self.out.append(lp.ethernet(lp.PEER_MAC, GUEST_MAC, lp.ETHERTYPE_IPV4, lp.ipv4(lp.GUEST_IP, lp.PEER_IP, lp.IP_PROTOCOL_TCP, segment)))

    def tick(self, now: float) -> list[bytes]:
        self.now = now
        if not self.done:
            try:
                next(self.program)
            except StopIteration:
                self.done = True
        for entry in list(self.sockets):
            self.out.extend(entry.tick(now))
        self.sockets = [entry for entry in self.sockets if entry.state != "closed" or entry in self.children]
        self.arm()
        out, self.out = self.out, []
        return out

    # The probe's program: every `yield` waits one tick.

    def send(self, connection: ListenerModelSocket, data: bytes):
        offset = 0
        while offset < len(data):
            offset += connection.write(data[offset:])
            if offset < len(data):
                yield

    def cue(self, control: ListenerModelSocket, cue: bytes):
        while not control.rx:
            yield
        expect(bytes(control.rx[:1]) == cue, f"the model probe expected cue {cue!r}, got {bytes(control.rx[:1])!r}")
        del control.rx[:1]
        control.ack_due = True

    def accept(self):
        while self.armed is None or self.armed.state != "open":
            yield
        child = self.armed
        self.children.append(child)
        self.armed = None
        self.arm()
        return child

    def read_to_eof(self, connection: ListenerModelSocket):
        data = bytearray()
        while not (connection.peer_fin and not connection.rx):
            data.extend(connection.read())
            yield
        return bytes(data)

    def close(self, connection: ListenerModelSocket):
        unread, unsent = len(connection.rx), len(connection.tx)
        if unread:
            self.out.append(connection.abort())
            terminal = "reset"
        else:
            connection.fin_queued = True
            while connection.time_wait_until is None and connection.state != "closed":
                yield
            terminal = "time-wait" if connection.time_wait_until is not None else "closed"
        if connection in self.children:
            self.children.remove(connection)
            self.closes.append((terminal, unread, unsent))

    def run(self):
        while not self.resolved:
            yield
        self.listening = True
        self.arm()
        control = self.allocate(49152)
        assert control is not None
        control.remote_port, control.state, control.syn_due = tlp.CONTROL_PORT, "syn-sent", self.now
        while control.state != "open":
            yield
        lines = iter(tlp.CONTROL_LINES)
        yield from self.send(control, next(lines))
        exchange = yield from self.accept()
        yield from self.send(control, next(lines))
        echoed = bytearray()
        while len(echoed) < len(tlp.EXCHANGE):
            echoed.extend(exchange.read())
            yield
        yield from self.send(exchange, bytes(echoed))
        yield from self.cue(control, tlp.CUE_ACCEPT_HELD)
        held = yield from self.accept()
        yield from self.send(control, next(lines))
        yield from self.cue(control, tlp.CUE_HALF_CLOSE)
        yield from self.send(exchange, tlp.LOCAL_HALF_GUEST)
        exchange.fin_queued = True
        received = yield from self.read_to_eof(exchange)
        expect(received == tlp.LOCAL_HALF_PEER, "the model probe did not receive the peer's bytes after its half-close")
        yield from self.close(exchange)
        yield from self.send(control, next(lines))
        received = yield from self.read_to_eof(held)
        expect(received == tlp.PEER_HALF_PEER, "the model probe did not receive the peer's bytes before its FIN")
        yield from self.send(held, tlp.PEER_HALF_GUEST)
        yield from self.close(held)
        yield from self.send(control, next(lines))
        crossing = yield from self.accept()
        yield from self.send(control, next(lines))
        yield from self.close(crossing)
        yield from self.send(control, next(lines))
        unread = yield from self.accept()
        yield from self.send(control, next(lines))
        yield from self.cue(control, tlp.CUE_UNREAD)
        yield from self.close(unread)
        yield from self.send(control, next(lines))
        unsent = yield from self.accept()
        yield from self.send(control, next(lines))
        expect(unsent.write(tlp.UNSENT) == len(tlp.UNSENT), "the model probe could not queue its unsent bytes")
        yield from self.send(control, next(lines))
        yield from self.close(unsent)
        yield from self.send(control, next(lines))
        self.listening = False
        if self.armed is not None:
            self.armed.state = "closed"
            self.armed = None
        yield from self.send(control, next(lines))
        control.fin_queued = True
        while control.time_wait_until is None and control.state != "closed":
            yield


def listener_simulate() -> tuple[tlp.ListenerPeer, ListenerModelGuest]:
    peer = tlp.ListenerPeer()
    guest = ListenerModelGuest()
    to_peer: list[tuple[float, bytes]] = []
    to_guest: list[tuple[float, bytes]] = []
    next_poll = 0.0
    tick = 0
    drain = 0
    while drain < 500 and tick < 120_000:
        tick += 1
        drain += int(guest.done and peer.complete)
        now = tick * MODEL_TICK
        due = [raw for when, raw in to_peer if when <= now]
        to_peer = [(when, raw) for when, raw in to_peer if when > now]
        for raw in due:
            to_guest.extend((now + MODEL_LATENCY, reply) for reply in peer.handle(raw, now))
        if now >= next_poll:
            to_guest.extend((now + MODEL_LATENCY, reply) for reply in peer.poll(now))
            next_poll = now + MODEL_POLL
        due = [raw for when, raw in to_guest if when <= now]
        to_guest = [(when, raw) for when, raw in to_guest if when > now]
        for raw in due:
            guest.receive(raw, now)
        to_peer.extend((now + MODEL_LATENCY, raw) for raw in guest.tick(now))
    expect(guest.done and peer.complete, f"the model listener did not finish the script: {peer.ledger.summary()}")
    return peer, guest


def check_listener_scripts() -> None:
    expect(tlp.LISTENER_BYTE_BUDGET == tlp.ACCEPTED_LIMIT * 2 * tlp.SOCKET_BUFFER_BYTES == 8192, "the listener byte budget does not reserve both buffers of every accepted connection")
    expect(len({attempt.source_port for attempt in tlp.ATTEMPTS}) == len(tlp.ATTEMPTS), "two attempts share a source port, so their flows would merge")
    expect([attempt.outcome for attempt in tlp.ATTEMPTS].count("reset") == 2, "the script must provoke exactly one backlog and one accepted-limit refusal")
    expect(set(tlp.GUEST_STREAMS) == set(tlp.PEER_STREAMS) == {attempt.name for attempt in tlp.ATTEMPTS if attempt.outcome == "accept"}, "every accepted attempt needs both declared streams")
    expect(tlp.pattern(3, 4) == bytes([3, 34, 65, 96]), "the declared stream formula changed")
    peer = tlp.ListenerPeer()
    expect(peer.poll(5.0) == [], "the peer initiated traffic before it knew the guest")
    peer.guest_mac = GUEST_MAC
    expect(peer.poll(6.0) == [], "the peer sent a SYN before the guest reported that it listens")
    syn = lp.decode(peer.open("unsent-close", 0.0)[0])
    expect(syn is not None and syn.tcp_window == 0 and syn.tcp_flags == lp.TCP_SYN, "the unsent-close SYN did not open with a zero window")
    impostor = lp.decode(peer.open("unadmitted-source", 0.0)[0])
    expect(impostor is not None and impostor.source == tlp.INTRUDER_MAC and impostor.ip_source == tlp.INTRUDER_IP, "the unadmitted SYN did not come from the impostor")


def check_listener_simulation() -> tlp.Ledger:
    peer, guest = listener_simulate()
    summaries = tlp.qualify_listener_ledger(peer.ledger, GUEST_MAC)
    expect(len(summaries) == len(tlp.ATTEMPTS) + 2, f"the listener qualifier did not summarize every case: {summaries}")
    expect((guest.unadmitted, guest.excess) == (1, 2), f"the model refused {guest.unadmitted} unadmitted and {guest.excess} excess SYNs, expected 1 and 2")
    # The peer half-close reply may still be queued when its close is requested.
    settled = [(terminal, unread, None if index == 1 else unsent) for index, (terminal, unread, unsent) in enumerate(guest.closes)]
    expect(settled == [("time-wait", 0, 0), ("closed", 0, None), ("time-wait", 0, 0), ("reset", len(tlp.UNREAD), 0), ("time-wait", 0, len(tlp.UNSENT))], f"the model's closes settled differently: {guest.closes}")
    return peer.ledger


def forge_listener(observation: tlp.Observation, **changes: object) -> tlp.Observation:
    frame = observation.frame
    fields: dict[str, object] = {"sequence": frame.tcp_sequence, "acknowledgment": frame.tcp_acknowledgment, "flags": frame.tcp_flags, "payload": frame.tcp_payload, "window": frame.tcp_window}
    fields.update(changes)
    assert frame.ip_source is not None and frame.ip_destination is not None and frame.tcp_source_port is not None and frame.tcp_destination_port is not None
    segment = lp.tcp(frame.ip_source, frame.ip_destination, frame.tcp_source_port, frame.tcp_destination_port, fields["sequence"], fields["acknowledgment"], fields["flags"], fields["payload"], fields["window"], frame.tcp_options)  # type: ignore[arg-type]
    decoded = lp.decode(lp.ethernet(frame.destination, frame.source, lp.ETHERTYPE_IPV4, lp.ipv4(frame.ip_source, frame.ip_destination, lp.IP_PROTOCOL_TCP, segment)))
    assert decoded is not None
    return dataclasses.replace(observation, frame=decoded)


def guest_segment(order: float, source_port: int, destination_port: int, sequence: int, acknowledgment: int, flags: int, payload: bytes = b"", window: int = 1024, destination: bytes = lp.PEER_MAC, destination_ip: bytes = lp.PEER_IP) -> tlp.Observation:
    segment = lp.tcp(lp.GUEST_IP, destination_ip, source_port, destination_port, sequence, acknowledgment, flags, payload, window)
    frame = lp.decode(lp.ethernet(destination, GUEST_MAC, lp.ETHERTYPE_IPV4, lp.ipv4(lp.GUEST_IP, destination_ip, lp.IP_PROTOCOL_TCP, segment)))
    assert frame is not None
    return tlp.Observation(order, order, frame)  # type: ignore[arg-type]


def check_listener_controls(ledger: tlp.Ledger) -> int:
    def positions(altered: tlp.Ledger, name: str, *, guest: bool) -> list[int]:
        attempt = tlp.ATTEMPT[name]
        entries = altered.received if guest else altered.sent
        return [index for index, entry in enumerate(entries) if entry.frame.kind == "tcp" and (entry.frame.tcp_destination_port if guest else entry.frame.tcp_source_port) == attempt.source_port]

    def flow(altered: tlp.Ledger, name: str) -> tlp.Flow:
        attempt = tlp.ATTEMPT[name]
        found = tlp.split_flows(altered, GUEST_MAC)[(attempt.source_ip, attempt.source_port, attempt.destination_port)]
        found.peer_isn = found.peer[0].frame.tcp_sequence
        found.guest_isn = found.guest[0].frame.tcp_sequence if found.guest else 0
        return found

    def append_guest(altered: tlp.Ledger, observation: tlp.Observation) -> None:
        altered.received.append(observation)
        altered.received.sort(key=lambda entry: entry.order)

    count = 0

    def refused(fragment: str, mutate: object) -> None:
        nonlocal count
        altered = copy.deepcopy(ledger)
        mutate(altered)  # type: ignore[operator]
        try:
            tlp.qualify_listener_ledger(altered, GUEST_MAC)
        except ValueError as error:
            expect(fragment in str(error), f"a listener control was refused for another reason: expected {fragment!r}, got {error}")
            count += 1
            return
        fail(f"listener qualification accepted corrupted evidence: {fragment}")

    def answer_unadmitted(altered: tlp.Ledger) -> None:
        syn = altered.sent[positions(altered, "unadmitted-source", guest=False)[0]]
        append_guest(altered, guest_segment(syn.order + 0.5, tlp.LISTEN_PORT, 50100, 7, syn.frame.tcp_sequence + 1, lp.TCP_SYN | lp.TCP_ACK, destination=tlp.INTRUDER_MAC, destination_ip=tlp.INTRUDER_IP))

    def resolve_intruder(altered: tlp.Ledger) -> None:
        frame = lp.decode(lp.ethernet(lp.BROADCAST, GUEST_MAC, lp.ETHERTYPE_ARP, lp.arp(lp.ARP_REQUEST, GUEST_MAC, lp.GUEST_IP, bytes(6), tlp.INTRUDER_IP)))
        assert frame is not None
        append_guest(altered, tlp.Observation(altered.received[-1].order + 0.5, 0.0, frame))  # type: ignore[arg-type]

    def answer_port(name: str, source_port: int, flags: int):
        def mutate(altered: tlp.Ledger) -> None:
            syn = altered.sent[positions(altered, name, guest=False)[0]]
            append_guest(altered, guest_segment(syn.order + 0.5, source_port, tlp.ATTEMPT[name].source_port, 0, syn.frame.tcp_sequence + 1, flags))
        return mutate

    def accept_refused(altered: tlp.Ledger) -> None:
        index = positions(altered, "backlog-full", guest=True)[0]
        altered.received[index] = forge_listener(altered.received[index], flags=lp.TCP_SYN | lp.TCP_ACK, sequence=5)

    def repeat_refusal(altered: tlp.Ledger) -> None:
        index = positions(altered, "accepted-limit", guest=True)[0]
        altered.received.insert(index + 1, dataclasses.replace(altered.received[index], order=altered.received[index].order + 0.5))

    def drop_refusal(altered: tlp.Ledger) -> None:
        del altered.received[positions(altered, "accepted-limit", guest=True)[0]]

    def corrupt_echo(altered: tlp.Ledger) -> None:
        index = next(index for index in positions(altered, "exchange", guest=True) if altered.received[index].frame.tcp_payload)
        payload = altered.received[index].frame.tcp_payload
        altered.received[index] = forge_listener(altered.received[index], payload=bytes([payload[0] ^ 1]) + payload[1:])

    def drop_fin(name: str):
        def mutate(altered: tlp.Ledger) -> None:
            for index in positions(altered, name, guest=True):
                frame = altered.received[index].frame
                if frame.tcp_flags & lp.TCP_FIN:
                    altered.received[index] = forge_listener(altered.received[index], flags=frame.tcp_flags & ~lp.TCP_FIN)
        return mutate

    def early_half_close_bytes(altered: tlp.Ledger) -> None:
        found = flow(altered, "exchange")
        fin = min(entry.order for entry in found.guest if entry.frame.tcp_flags & lp.TCP_FIN)
        index = next(index for index in positions(altered, "exchange", guest=False) if altered.sent[index].frame.tcp_payload and found.peer_offset(altered.sent[index].frame) >= len(tlp.EXCHANGE))
        altered.sent[index] = dataclasses.replace(altered.sent[index], order=fin - 0.5)

    def early_reply(altered: tlp.Ledger) -> None:
        found = flow(altered, "backlog-held")
        fin = min(entry.order for entry in found.peer if entry.frame.tcp_flags & lp.TCP_FIN)
        index = next(index for index in positions(altered, "backlog-held", guest=True) if altered.received[index].frame.tcp_payload)
        moved = forge_listener(altered.received[index], acknowledgment=found.peer_isn + 1 + len(tlp.PEER_HALF_PEER))
        altered.received[index] = dataclasses.replace(moved, order=fin - 0.25)

    def acknowledge_crossing(altered: tlp.Ledger) -> None:
        for index in positions(altered, "simultaneous-close", guest=False):
            frame = altered.sent[index].frame
            if frame.tcp_flags & lp.TCP_FIN:
                altered.sent[index] = forge_listener(altered.sent[index], acknowledgment=frame.tcp_acknowledgment + 1)

    def fin_instead_of_reset(altered: tlp.Ledger) -> None:
        for index in positions(altered, "unread-close", guest=True):
            frame = altered.received[index].frame
            if frame.tcp_flags & lp.TCP_RST:
                altered.received[index] = forge_listener(altered.received[index], flags=lp.TCP_ACK | lp.TCP_FIN)

    def premature_reset(altered: tlp.Ledger) -> None:
        index = next(index for index in positions(altered, "unread-close", guest=True) if altered.received[index].frame.tcp_flags & lp.TCP_RST)
        cue = min(entry.order for entry in altered.sent if entry.frame.tcp_payload == tlp.CUE_UNREAD)
        altered.received[index] = dataclasses.replace(altered.received[index], order=cue - 0.5)

    def widen_probe(altered: tlp.Ledger) -> None:
        found = flow(altered, "unsent-close")
        opened = min(entry.order for entry in found.peer if entry.frame.tcp_window > 0)
        synack = found.guest[0]
        append_guest(altered, guest_segment(opened - 0.5, tlp.LISTEN_PORT, tlp.ATTEMPT["unsent-close"].source_port, synack.frame.tcp_sequence + 1, found.peer_isn + 1, lp.TCP_ACK | lp.TCP_PSH, tlp.UNSENT[:2]))

    def open_syn(altered: tlp.Ledger) -> None:
        index = positions(altered, "unsent-close", guest=False)[0]
        altered.sent[index] = forge_listener(altered.sent[index], window=tlp.PEER_WINDOW)

    def early_window(altered: tlp.Ledger) -> None:
        index = next(index for index in positions(altered, "unsent-close", guest=False) if altered.sent[index].frame.tcp_flags == lp.TCP_ACK)
        altered.sent[index] = forge_listener(altered.sent[index], window=1)

    def widen_window(altered: tlp.Ledger) -> None:
        index = positions(altered, "exchange", guest=True)[-1]
        altered.received[index] = forge_listener(altered.received[index], window=tlp.SOCKET_BUFFER_BYTES + 1)

    def acknowledge_unsent(altered: tlp.Ledger) -> None:
        found = flow(altered, "exchange")
        first = min(entry.order for entry in found.peer if entry.frame.tcp_payload)
        append_guest(altered, guest_segment(first - 0.5, tlp.LISTEN_PORT, tlp.ATTEMPT["exchange"].source_port, found.guest_isn + 1, found.peer_isn + 101, lp.TCP_ACK))

    def corrupt_line(altered: tlp.Ledger) -> None:
        index = next(index for index, entry in enumerate(altered.received) if entry.frame.tcp_payload == tlp.CONTROL_LINES[0])
        altered.received[index] = forge_listener(altered.received[index], payload=b"reddy\n")

    def reorder_backlog(altered: tlp.Ledger) -> None:
        index = positions(altered, "backlog-full", guest=True)[0]
        altered.received[index] = dataclasses.replace(altered.received[index], order=10**9)

    def skip_attempt(altered: tlp.Ledger) -> None:
        del altered.sent[positions(altered, "unadmitted-source", guest=False)[0]]

    def desync(altered: tlp.Ledger) -> None:
        altered.desync = "forged"

    def unfinished(altered: tlp.Ledger) -> None:
        altered.stage -= 1

    refused("the guest addressed a host other than the admitted peer", answer_unadmitted)
    refused("resolved an address other than the admitted peer", resolve_intruder)
    refused("undeclared-port: the guest answered", answer_port("undeclared-port", tlp.UNDECLARED_PORT, lp.TCP_RST | lp.TCP_ACK))
    refused("foreign-address: the guest answered", answer_port("foreign-address", tlp.LISTEN_PORT, lp.TCP_SYN | lp.TCP_ACK))
    refused("backlog-full: the refusal is not a reset", accept_refused)
    refused("accepted-limit: expected exactly one guest frame", repeat_refusal)
    refused("accepted-limit: expected exactly one guest frame", drop_refusal)
    refused("exchange: guest bytes at offset", corrupt_echo)
    refused("exchange: the guest's FIN", drop_fin("exchange"))
    refused("local half-close: the peer's half-close bytes preceded the guest's FIN", early_half_close_bytes)
    refused("peer half-close: the guest's bytes did not follow the peer's FIN", early_reply)
    refused("simultaneous close: the peer's FIN acknowledged the guest's FIN", acknowledge_crossing)
    refused("unread-close: the guest's FIN", fin_instead_of_reset)
    refused("unread close: the reset did not follow", premature_reset)
    refused("more than a one-byte probe", widen_probe)
    refused("the peer's window was not zero until it opened", open_syn)
    refused("the peer's window opened before the guest queued its bytes", early_window)
    refused("beyond its declared 2048-byte buffer", widen_window)
    refused("acknowledged bytes the peer had not sent", acknowledge_unsent)
    refused("control bytes at offset", corrupt_line)
    refused("the backlog refusal did not happen", reorder_backlog)
    refused("unadmitted-source: the peer never sent its SYN", skip_attempt)
    refused("left the script", desync)
    refused("the script stopped", unfinished)
    return count


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
    check_impairment_scripts()
    check_impairment_controls(check_impairment_simulation())
    check_listener_scripts()
    listener_controls = check_listener_controls(check_listener_simulation())
    (kind,) = struct.unpack("!H", struct.pack("!H", lp.ETHERTYPE_ARP))
    expect(kind == lp.ETHERTYPE_ARP, "struct sanity")
    print("link peer check: ARP/ICMP preserved; bounded real TCP echo, checksums, stream delivery, windows, retransmissions, refusal, and close verified")
    print("link peer check: scripted reorder, loss, silence and window impairments qualified against a model stack; 24 corrupted-evidence controls refused")
    print(f"link peer check: scripted external-listener refusals, half-close, simultaneous close and unread/unsent close qualified against a model stack; {listener_controls} corrupted-evidence controls refused")


if __name__ == "__main__":
    main()
