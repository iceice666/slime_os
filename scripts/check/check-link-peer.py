#!/usr/bin/env python3
"""Host regressions for `scripts/lib/link_peer.py`, the frame-level peer the
network planes talk to. No QEMU: every case drives the peer's pure half with
frames built by the same encoders, and the assertions are on the bytes the
peer would put on the wire and on what its ledger records."""

from __future__ import annotations

import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

import link_peer as lp  # noqa: E402

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


def check_tcp_checksum_known_answer() -> None:
    # Computed by hand once (RFC 793 pseudo-header, RFC 1071 fold) over an
    # odd-length segment, so the encoder is checked against a value it did not
    # produce.
    header = struct.pack("!HHIIBBHHH", 4242, 49152, 0x10000000, 0x0000ABCD, 5 << 4, 0x18, 8192, 0, 0)
    segment = header + b"hello, slime"
    expect(
        lp.tcp_checksum(lp.PEER_IP, lp.GUEST_IP, segment) == 0xb11d,
        "the TCP checksum of the known-answer segment is wrong",
    )
    built = lp.tcp_segment(lp.PEER_IP, lp.GUEST_IP, 4242, 49152, 0x10000000, 0x0000ABCD, 0x18, b"hello, slime")
    expect(struct.unpack("!H", built[16:18])[0] == 0xb11d, "tcp_segment did not place the known checksum")


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


def guest_segment(port: int, seq: int, ack: int, flags: int, payload: bytes = b"", destination_port: int = lp.ECHO_PORT) -> bytes:
    segment = lp.tcp_segment(lp.GUEST_IP, lp.PEER_IP, port, destination_port, seq, ack, flags, payload)
    return lp.ethernet(lp.PEER_MAC, GUEST_MAC, lp.ETHERTYPE_IPV4, lp.ipv4(lp.GUEST_IP, lp.PEER_IP, lp.IP_PROTOCOL_TCP, segment))


def tcp_replies(peer: lp.Peer, raw: bytes) -> list[lp.TcpSegment]:
    out = []
    for reply in peer.handle(raw):
        frame = lp.decode(reply)
        expect(frame is not None and frame.kind == "tcp", "a TCP reply did not decode as TCP")
        segment = lp.decode_tcp(frame)
        expect(segment is not None, "a TCP reply failed its checksum")
        out.append(segment)
    return out


def check_tcp_echo_server() -> None:
    peer = lp.Peer()
    isn = 0x2000
    # Handshake.
    (synack,) = tcp_replies(peer, guest_segment(50000, isn, 0, lp.TCP_SYN))
    expect(synack.flags == lp.TCP_SYN | lp.TCP_ACK and synack.ack == isn + 1 and synack.seq == lp.PEER_ISN, "SYN/ACK contents")
    expect(tcp_replies(peer, guest_segment(50000, isn + 1, lp.PEER_ISN + 1, lp.TCP_ACK)) == [], "the handshake's final ACK was answered")
    expect(peer.tcp.flows[50000].state == "established", "flow not established after the handshake")
    # Data, echoed back in order across two segments.
    payload = bytes(range(256)) * 8  # 2048 bytes
    replies = tcp_replies(peer, guest_segment(50000, isn + 1, lp.PEER_ISN + 1, lp.TCP_ACK | lp.TCP_PSH, payload))
    expect(replies[0].flags == lp.TCP_ACK and replies[0].ack == isn + 1 + len(payload) and replies[0].payload == b"", "data was not acknowledged first")
    echoed = b"".join(reply.payload for reply in replies[1:])
    expect(echoed == payload, "echoed bytes differ from the sent bytes")
    expect(all(len(reply.payload) <= lp.SEGMENT_BYTES for reply in replies[1:]), "an echo segment exceeds the segment bound")
    expect(replies[1].seq == lp.PEER_ISN + 1 and replies[-1].seq + len(replies[-1].payload) == lp.PEER_ISN + 1 + len(payload), "echo sequence numbers")
    # A retransmission of the same data is acknowledged again and echoed no twice.
    dup = tcp_replies(peer, guest_segment(50000, isn + 1, lp.PEER_ISN + 1, lp.TCP_ACK | lp.TCP_PSH, payload))
    expect(len(dup) == 1 and dup[0].flags == lp.TCP_ACK and dup[0].ack == isn + 1 + len(payload) and peer.tcp.flows[50000].echoed == len(payload), "a retransmission was echoed again")
    # The guest closes: its FIN is acknowledged and answered with the server's FIN.
    fin_seq = isn + 1 + len(payload)
    fin_ack = lp.PEER_ISN + 1 + len(payload)
    replies = tcp_replies(peer, guest_segment(50000, fin_seq, fin_ack, lp.TCP_FIN | lp.TCP_ACK))
    expect(len(replies) == 2 and replies[0].flags == lp.TCP_ACK and replies[0].ack == fin_seq + 1 and replies[1].flags == lp.TCP_FIN | lp.TCP_ACK, "FIN handling")
    expect(tcp_replies(peer, guest_segment(50000, fin_seq + 1, fin_ack + 1, lp.TCP_ACK)) == [] and peer.tcp.flows[50000].state == "closed", "the server's FIN was not acknowledged into closed")
    # The refused port answers every SYN with RST.
    (rst,) = tcp_replies(peer, guest_segment(50001, 7, 0, lp.TCP_SYN, destination_port=lp.REFUSED_PORT))
    expect(rst.flags == lp.TCP_RST | lp.TCP_ACK and rst.ack == 8 and peer.tcp.refused == [50001], "refused port")
    # An unknown port is ignored.
    expect(peer.handle(guest_segment(50002, 7, 0, lp.TCP_SYN, destination_port=9)) == [], "an undeclared port was answered")
    expect(peer.ledger.count_received("tcp") == 8 and "50000:closed/rx2048/echo2048" in peer.tcp.summary(), "tcp ledger")
    expect(peer.tcp.flows[50000].closed_by == "guest", "the guest closed first")


def check_tcp_echo_then_close() -> None:
    peer = lp.Peer()
    isn = 0x3000
    port = 50010
    closing = lp.CLOSING_PORT
    (synack,) = tcp_replies(peer, guest_segment(port, isn, 0, lp.TCP_SYN, destination_port=closing))
    expect(synack.flags == lp.TCP_SYN | lp.TCP_ACK and synack.source_port == closing and synack.ack == isn + 1, "SYN/ACK from the closing port")
    expect(tcp_replies(peer, guest_segment(port, isn + 1, lp.PEER_ISN + 1, lp.TCP_ACK, destination_port=closing)) == [], "the handshake's final ACK was answered")
    flow = peer.tcp.flows[port]
    expect(flow.state == "established" and flow.server_port == closing, "flow not established on the closing port")
    # Data is acknowledged and echoed, and the peer's FIN follows the echo.
    payload = bytes(range(64))
    replies = tcp_replies(peer, guest_segment(port, isn + 1, lp.PEER_ISN + 1, lp.TCP_ACK | lp.TCP_PSH, payload, destination_port=closing))
    expect(len(replies) == 3 and replies[0].flags == lp.TCP_ACK and replies[0].ack == isn + 1 + len(payload), "data was not acknowledged first")
    expect(replies[1].flags == lp.TCP_ACK | lp.TCP_PSH and replies[1].payload == payload and replies[1].seq == lp.PEER_ISN + 1, "echo")
    expect(replies[2].flags == lp.TCP_FIN | lp.TCP_ACK and replies[2].seq == lp.PEER_ISN + 1 + len(payload) and replies[2].payload == b"", "the peer's FIN did not follow the echo")
    expect(flow.state == "fin-sent" and flow.closed_by == "peer" and flow.fin_sent and not flow.fin_received, "flow state after the peer's FIN")
    # The guest acknowledges the peer's FIN: the flow is not closed until the
    # guest's own FIN.
    guest_seq = isn + 1 + len(payload)
    fin_ack = lp.PEER_ISN + 1 + len(payload) + 1
    expect(tcp_replies(peer, guest_segment(port, guest_seq, fin_ack, lp.TCP_ACK, destination_port=closing)) == [] and flow.state == "fin-sent", "a bare ACK of the peer's FIN closed the flow")
    (ack,) = tcp_replies(peer, guest_segment(port, guest_seq, fin_ack, lp.TCP_FIN | lp.TCP_ACK, destination_port=closing))
    expect(ack.flags == lp.TCP_ACK and ack.ack == guest_seq + 1 and flow.state == "closed" and flow.closed_by == "peer", "the guest's FIN was not acknowledged into closed")
    expect(f"{port}:closed/rx64/echo64" in peer.tcp.summary() and peer.tcp.refused == [], "closing-port ledger")


def main() -> None:
    check_checksums()
    check_tcp_checksum_known_answer()
    check_frames_decode_and_pad()
    check_peer_answers_arp_and_echo()
    check_peer_learns_the_guest_and_pings_it()
    check_tcp_echo_server()
    check_tcp_echo_then_close()
    (kind,) = struct.unpack("!H", struct.pack("!H", lp.ETHERTYPE_ARP))
    expect(kind == lp.ETHERTYPE_ARP, "struct sanity")
    print("link peer check: ARP and ICMP echo answered exactly for the peer's own address, the guest learned and pinged, TCP echoed in order, closed first where declared and refused where declared, and the ledger honest")


if __name__ == "__main__":
    main()
