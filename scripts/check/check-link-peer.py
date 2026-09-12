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


def main() -> None:
    check_checksums()
    check_frames_decode_and_pad()
    check_peer_answers_arp_and_echo()
    check_peer_learns_the_guest_and_pings_it()
    (kind,) = struct.unpack("!H", struct.pack("!H", lp.ETHERTYPE_ARP))
    expect(kind == lp.ETHERTYPE_ARP, "struct sanity")
    print("link peer check: ARP and ICMP echo answered exactly for the peer's own address, the guest learned and pinged, and the ledger honest")


if __name__ == "__main__":
    main()
