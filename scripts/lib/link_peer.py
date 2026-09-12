"""A frame-level Ethernet peer for the network planes' QEMU socket backend.

QEMU's ``-netdev socket,udp=...`` hands every frame the guest transmits to one
UDP socket and injects every datagram it receives on another as a frame. This
module is the other end of that wire: one host with one MAC and one IPv4
address that answers ARP and ICMP echo requests, sends its own, and keeps a
ledger of every frame it saw. It is the whole network the plane sees, so a
gate can assert what left the guest, not only what came back.

Standard library only: the gate needs loopback and nothing else.
"""

from __future__ import annotations

import dataclasses
import socket
import struct
import threading
import time

ETHERTYPE_IPV4 = 0x0800
ETHERTYPE_ARP = 0x0806
IP_PROTOCOL_ICMP = 1
IP_PROTOCOL_TCP = 6
ARP_REQUEST = 1
ARP_REPLY = 2
ICMP_ECHO_REPLY = 0
ICMP_ECHO_REQUEST = 8
MIN_FRAME = 60
BROADCAST = b"\xff" * 6

PEER_MAC = bytes.fromhex("52540053" "4c02")
PEER_IP = bytes([10, 0, 0, 2])
GUEST_IP = bytes([10, 0, 0, 1])


def checksum(data: bytes) -> int:
    """RFC 1071 one's-complement sum over 16-bit words."""
    if len(data) % 2:
        data += b"\x00"
    total = sum(struct.unpack("!%dH" % (len(data) // 2), data))
    while total >> 16:
        total = (total & 0xFFFF) + (total >> 16)
    return (~total) & 0xFFFF


def ethernet(destination: bytes, source: bytes, ethertype: int, payload: bytes) -> bytes:
    frame = destination + source + struct.pack("!H", ethertype) + payload
    if len(frame) < MIN_FRAME:
        frame += bytes(MIN_FRAME - len(frame))
    return frame


def arp(operation: int, sender_mac: bytes, sender_ip: bytes, target_mac: bytes, target_ip: bytes) -> bytes:
    return struct.pack("!HHBBH", 1, ETHERTYPE_IPV4, 6, 4, operation) + sender_mac + sender_ip + target_mac + target_ip


def ipv4(source: bytes, destination: bytes, protocol: int, payload: bytes, identification: int = 0) -> bytes:
    header = struct.pack("!BBHHHBBH4s4s", 0x45, 0, 20 + len(payload), identification, 0x4000, 64, protocol, 0, source, destination)
    header = header[:10] + struct.pack("!H", checksum(header)) + header[12:]
    return header + payload


def icmp_echo(kind: int, identifier: int, sequence: int, payload: bytes) -> bytes:
    body = struct.pack("!BBHHH", kind, 0, 0, identifier, sequence) + payload
    return body[:2] + struct.pack("!H", checksum(body)) + body[4:]


@dataclasses.dataclass(frozen=True)
class Frame:
    """What a frame said it was, decoded far enough to classify and answer it."""

    destination: bytes
    source: bytes
    ethertype: int
    arp_operation: int | None = None
    arp_sender_mac: bytes | None = None
    arp_sender_ip: bytes | None = None
    arp_target_ip: bytes | None = None
    ip_source: bytes | None = None
    ip_destination: bytes | None = None
    ip_protocol: int | None = None
    ip_payload: bytes = b""
    icmp_type: int | None = None
    icmp_identifier: int | None = None
    icmp_sequence: int | None = None
    icmp_payload: bytes = b""

    @property
    def kind(self) -> str:
        if self.ethertype == ETHERTYPE_ARP:
            return "arp-request" if self.arp_operation == ARP_REQUEST else "arp-reply" if self.arp_operation == ARP_REPLY else "arp"
        if self.ethertype == ETHERTYPE_IPV4 and self.ip_protocol is not None:
            if self.ip_protocol == IP_PROTOCOL_ICMP:
                return "icmp-echo-request" if self.icmp_type == ICMP_ECHO_REQUEST else "icmp-echo-reply" if self.icmp_type == ICMP_ECHO_REPLY else "icmp"
            if self.ip_protocol == IP_PROTOCOL_TCP:
                return "tcp"
            return "ipv4"
        return "other"


def decode(frame: bytes) -> Frame | None:
    if len(frame) < 14:
        return None
    destination, source = frame[:6], frame[6:12]
    (ethertype,) = struct.unpack("!H", frame[12:14])
    body = frame[14:]
    if ethertype == ETHERTYPE_ARP and len(body) >= 28:
        hardware, protocol, hardware_len, protocol_len, operation = struct.unpack("!HHBBH", body[:8])
        if (hardware, protocol, hardware_len, protocol_len) != (1, ETHERTYPE_IPV4, 6, 4):
            return Frame(destination, source, ethertype)
        return Frame(
            destination,
            source,
            ethertype,
            arp_operation=operation,
            arp_sender_mac=body[8:14],
            arp_sender_ip=body[14:18],
            arp_target_ip=body[24:28],
        )
    if ethertype == ETHERTYPE_IPV4 and len(body) >= 20:
        version_ihl = body[0]
        header_len = (version_ihl & 0x0F) * 4
        if version_ihl >> 4 != 4 or header_len < 20 or len(body) < header_len:
            return Frame(destination, source, ethertype)
        (total_len,) = struct.unpack("!H", body[2:4])
        protocol = body[9]
        if checksum(body[:header_len]) != 0:
            return Frame(destination, source, ethertype)
        payload = body[header_len:total_len]
        decoded = Frame(
            destination,
            source,
            ethertype,
            ip_source=body[12:16],
            ip_destination=body[16:20],
            ip_protocol=protocol,
            ip_payload=payload,
        )
        if protocol == IP_PROTOCOL_ICMP and len(payload) >= 8 and checksum(payload) == 0:
            icmp_type, _code, _sum, identifier, sequence = struct.unpack("!BBHHH", payload[:8])
            decoded = dataclasses.replace(
                decoded,
                icmp_type=icmp_type,
                icmp_identifier=identifier,
                icmp_sequence=sequence,
                icmp_payload=payload[8:],
            )
        return decoded
    return Frame(destination, source, ethertype)


@dataclasses.dataclass
class Ledger:
    """Every frame the peer saw or sent, and what it made of them."""

    received: list[Frame] = dataclasses.field(default_factory=list)
    sent: list[Frame] = dataclasses.field(default_factory=list)
    guest_mac: bytes | None = None

    def count_received(self, kind: str) -> int:
        return sum(1 for frame in self.received if frame.kind == kind)

    def count_sent(self, kind: str) -> int:
        return sum(1 for frame in self.sent if frame.kind == kind)

    def foreign_sources(self, expected_mac: bytes) -> list[bytes]:
        return sorted({frame.source for frame in self.received if frame.source != expected_mac})

    def ip_destinations(self) -> set[bytes]:
        return {frame.ip_destination for frame in self.received if frame.ip_destination is not None}

    def echo_replies_matching(self, identifier: int) -> list[int]:
        return sorted(
            frame.icmp_sequence
            for frame in self.received
            if frame.kind == "icmp-echo-reply" and frame.icmp_identifier == identifier and frame.icmp_sequence is not None
        )

    def summary(self) -> str:
        kinds: dict[str, int] = {}
        for frame in self.received:
            kinds[frame.kind] = kinds.get(frame.kind, 0) + 1
        received = ", ".join(f"{kind}={count}" for kind, count in sorted(kinds.items())) or "none"
        return f"received {len(self.received)} ({received}); sent {len(self.sent)}"


class Peer:
    """The pure half: given a frame from the guest, what the peer answers."""

    def __init__(self, mac: bytes = PEER_MAC, ip: bytes = PEER_IP, guest_ip: bytes = GUEST_IP) -> None:
        self.mac = mac
        self.ip = ip
        self.guest_ip = guest_ip
        self.ledger = Ledger()

    def handle(self, raw: bytes) -> list[bytes]:
        frame = decode(raw)
        if frame is None:
            return []
        self.ledger.received.append(frame)
        replies: list[bytes] = []
        if frame.kind == "arp-request" and frame.arp_target_ip == self.ip and frame.arp_sender_mac and frame.arp_sender_ip:
            replies.append(self.emit(ethernet(frame.source, self.mac, ETHERTYPE_ARP, arp(ARP_REPLY, self.mac, self.ip, frame.arp_sender_mac, frame.arp_sender_ip))))
        if frame.kind == "arp-reply" and frame.arp_sender_ip == self.guest_ip and frame.arp_sender_mac:
            self.ledger.guest_mac = frame.arp_sender_mac
        if frame.kind == "icmp-echo-request" and frame.ip_destination == self.ip and frame.ip_source and frame.icmp_identifier is not None and frame.icmp_sequence is not None:
            reply = icmp_echo(ICMP_ECHO_REPLY, frame.icmp_identifier, frame.icmp_sequence, frame.icmp_payload)
            replies.append(self.emit(ethernet(frame.source, self.mac, ETHERTYPE_IPV4, ipv4(self.ip, frame.ip_source, IP_PROTOCOL_ICMP, reply))))
        return replies

    def arp_request_for_guest(self) -> bytes:
        return self.emit(ethernet(BROADCAST, self.mac, ETHERTYPE_ARP, arp(ARP_REQUEST, self.mac, self.ip, bytes(6), self.guest_ip)))

    def echo_request(self, identifier: int, sequence: int, payload: bytes) -> bytes | None:
        if self.ledger.guest_mac is None:
            return None
        request = icmp_echo(ICMP_ECHO_REQUEST, identifier, sequence, payload)
        return self.emit(ethernet(self.ledger.guest_mac, self.mac, ETHERTYPE_IPV4, ipv4(self.ip, self.guest_ip, IP_PROTOCOL_ICMP, request, identification=sequence)))

    def emit(self, raw: bytes) -> bytes:
        decoded = decode(raw)
        if decoded is not None:
            self.ledger.sent.append(decoded)
        return raw


ECHO_IDENTIFIER = 0x5153
ECHO_COUNT = 5
ECHO_INTERVAL_SECONDS = 0.25
ECHO_PAYLOAD = bytes(range(48))


def serve(receiver: socket.socket, qemu_port: int, stop: threading.Event, peer: Peer) -> None:
    """Run `peer` against QEMU's UDP backend until `stop` is set.

    Every 250 ms the peer asks for the guest's MAC until it has it, then sends
    one ICMP echo request per interval up to `ECHO_COUNT`. Replies to the
    guest's own ARP and echo requests go out as they arrive.
    """
    sender = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        next_probe = time.monotonic()
        sequence = 0
        while not stop.is_set():
            try:
                raw, _ = receiver.recvfrom(4096)
            except socket.timeout:
                raw = None
            if raw is not None:
                for reply in peer.handle(raw):
                    sender.sendto(reply, ("127.0.0.1", qemu_port))
            now = time.monotonic()
            if now >= next_probe:
                next_probe = now + ECHO_INTERVAL_SECONDS
                if peer.ledger.guest_mac is None:
                    sender.sendto(peer.arp_request_for_guest(), ("127.0.0.1", qemu_port))
                elif sequence < ECHO_COUNT:
                    sequence += 1
                    request = peer.echo_request(ECHO_IDENTIFIER, sequence, ECHO_PAYLOAD)
                    if request is not None:
                        sender.sendto(request, ("127.0.0.1", qemu_port))
    finally:
        sender.close()
