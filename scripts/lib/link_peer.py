"""A frame-level Ethernet peer for the network planes' QEMU socket backend.

QEMU's ``-netdev socket,udp=...`` hands every frame the guest transmits to one
UDP socket and injects every datagram it receives on another as a frame. This
module is the other end of that wire: one host with one MAC and one IPv4
address that answers ARP, ICMP echo, and bounded TCP echo requests, and keeps a
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

TCP_FIN = 0x01
TCP_SYN = 0x02
TCP_RST = 0x04
TCP_PSH = 0x08
TCP_ACK = 0x10
TCP_ECHO_PORT = 4242
TCP_REFUSED_PORT = 4243
TCP_STREAM_LIMIT = 4096
TCP_CONNECTION_LIMIT = 4
TCP_SEGMENT_LIMIT = 1024
TCP_SEQUENCE_MASK = 0xFFFFFFFF
TCP_RETRY_INTERVAL_SECONDS = 0.25
TCP_RETRY_LIMIT = 8


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


def tcp(
    source_ip: bytes,
    destination_ip: bytes,
    source_port: int,
    destination_port: int,
    sequence: int,
    acknowledgment: int,
    flags: int,
    payload: bytes = b"",
    window: int = TCP_STREAM_LIMIT,
    options: bytes = b"",
) -> bytes:
    """Encode the externally specified RFC 9293 TCP header and checksum."""
    if len(options) > 40 or len(options) % 4:
        raise ValueError("TCP options must occupy at most ten complete words")
    header = struct.pack("!HHIIBBHHH", source_port, destination_port, sequence & TCP_SEQUENCE_MASK, acknowledgment & TCP_SEQUENCE_MASK, (5 + len(options) // 4) << 4, flags, window, 0, 0)
    segment = header + options + payload
    pseudoheader = source_ip + destination_ip + struct.pack("!BBH", 0, IP_PROTOCOL_TCP, len(segment))
    return segment[:16] + struct.pack("!H", checksum(pseudoheader + segment)) + segment[18:]


def tcp_options_valid(options: bytes) -> bool:
    offset = 0
    while offset < len(options):
        kind = options[offset]
        if kind == 0:
            return not any(options[offset + 1 :])
        if kind == 1:
            offset += 1
            continue
        if offset + 2 > len(options):
            return False
        length = options[offset + 1]
        if length < 2 or offset + length > len(options):
            return False
        if kind == 2 and (length != 4 or int.from_bytes(options[offset + 2 : offset + 4], "big") == 0):
            return False
        offset += length
    return True


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
    tcp_source_port: int | None = None
    tcp_destination_port: int | None = None
    tcp_sequence: int = 0
    tcp_acknowledgment: int = 0
    tcp_flags: int = 0
    tcp_window: int = 0
    tcp_options: bytes = b""
    tcp_payload: bytes = b""

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
        if total_len < header_len or total_len > len(body) or checksum(body[:header_len]) != 0 or int.from_bytes(body[6:8], "big") & 0xBFFF:
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
        if protocol == IP_PROTOCOL_TCP:
            if len(payload) < 20:
                return decoded
            source_port, destination_port, sequence, acknowledgment, offset, flags, window, _sum, urgent = struct.unpack("!HHIIBBHHH", payload[:20])
            tcp_header_len = (offset >> 4) * 4
            pseudoheader = body[12:20] + struct.pack("!BBH", 0, IP_PROTOCOL_TCP, len(payload))
            if tcp_header_len < 20 or tcp_header_len > len(payload) or offset & 0x0F or urgent or checksum(pseudoheader + payload) != 0:
                return decoded
            options = payload[20:tcp_header_len]
            if not tcp_options_valid(options):
                return decoded
            decoded = dataclasses.replace(
                decoded,
                tcp_source_port=source_port,
                tcp_destination_port=destination_port,
                tcp_sequence=sequence,
                tcp_acknowledgment=acknowledgment,
                tcp_flags=flags,
                tcp_window=window,
                tcp_options=options,
                tcp_payload=payload[tcp_header_len:],
            )
        return decoded
    return Frame(destination, source, ethertype)


@dataclasses.dataclass
class TcpConnection:
    """Bounded accepted byte streams; packet retransmissions are not deliveries."""

    guest_mac: bytes
    guest_port: int
    guest_initial_sequence: int
    peer_initial_sequence: int
    segment_limit: int = 536
    received: bytearray = dataclasses.field(default_factory=bytearray)
    sent: bytearray = dataclasses.field(default_factory=bytearray)
    acknowledged: bytearray = dataclasses.field(default_factory=bytearray)
    handshake: bool = False
    guest_fin: bool = False
    peer_fin: bool = False
    closed: bool = False
    reset: bool = False
    held: bool = False
    abandoned: bool = False
    retransmissions: int = 0
    retry_deadline: float | None = None

    @property
    def receive_sequence(self) -> int:
        return (self.guest_initial_sequence + 1 + len(self.received) + int(self.guest_fin)) & TCP_SEQUENCE_MASK

    @property
    def send_sequence(self) -> int:
        return (self.peer_initial_sequence + 1 + len(self.sent) + int(self.peer_fin)) & TCP_SEQUENCE_MASK


@dataclasses.dataclass
class Ledger:
    """Every frame the peer saw or sent, and what it made of them."""

    received: list[Frame] = dataclasses.field(default_factory=list)
    sent: list[Frame] = dataclasses.field(default_factory=list)
    guest_mac: bytes | None = None
    tcp_connections: list[TcpConnection] = dataclasses.field(default_factory=list)
    tcp_resets_sent: int = 0
    malformed_received: int = 0

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
        tcp = "; ".join(f"tcp port={entry.guest_port} received={len(entry.received)} sent={len(entry.sent)} acknowledged={len(entry.acknowledged)} handshake={int(entry.handshake)} guest_fin={int(entry.guest_fin)} peer_fin={int(entry.peer_fin)} closed={int(entry.closed)} reset={int(entry.reset)} held={int(entry.held)} abandoned={int(entry.abandoned)} retransmissions={entry.retransmissions}" for entry in self.tcp_connections)
        return f"received {len(self.received)} ({received}); sent {len(self.sent)}; resets={self.tcp_resets_sent}; malformed={self.malformed_received}" + (f"; {tcp}" if tcp else "")


class Peer:
    """The pure half: given a frame from the guest, what the peer answers.

    TCP admits four retained connections and 4096 application bytes each.
    Gaps elicit the current ACK for sender retransmission, not reassembly.
    A bounded timer retransmits only already-emitted, unacknowledged bytes.
    """

    def __init__(self, mac: bytes = PEER_MAC, ip: bytes = PEER_IP, guest_ip: bytes = GUEST_IP, *, hold_first_tcp: bool = False) -> None:
        self.mac = mac
        self.ip = ip
        self.guest_ip = guest_ip
        self.hold_first_tcp = hold_first_tcp
        self.ledger = Ledger()

    def handle(self, raw: bytes) -> list[bytes]:
        frame = decode(raw)
        if frame is None:
            self.ledger.malformed_received += 1
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
        if frame.kind == "tcp":
            replies.extend(self.handle_tcp(frame))
            self.schedule_retries(time.monotonic())
        return replies

    def schedule_retries(self, now: float) -> None:
        for connection in self.ledger.tcp_connections:
            outstanding = not connection.handshake or len(connection.sent) > len(connection.acknowledged) or connection.peer_fin and not connection.closed
            if connection.reset or connection.abandoned or not outstanding:
                connection.retry_deadline = None
            elif connection.retry_deadline is None:
                connection.retry_deadline = now + TCP_RETRY_INTERVAL_SECONDS

    def retransmit(self, now: float) -> list[bytes]:
        replies = []
        for connection in self.ledger.tcp_connections:
            if connection.retry_deadline is None or now < connection.retry_deadline or connection.retransmissions >= TCP_RETRY_LIMIT:
                continue
            if not connection.handshake:
                sequence = connection.peer_initial_sequence
                flags = TCP_SYN | TCP_ACK
                payload = b""
            elif len(connection.acknowledged) < len(connection.sent):
                offset = len(connection.acknowledged)
                sequence = connection.peer_initial_sequence + 1 + offset
                flags = TCP_ACK | TCP_PSH
                payload = bytes(connection.sent[offset : offset + connection.segment_limit])
            elif connection.peer_fin and not connection.closed:
                sequence = connection.send_sequence - 1
                flags = TCP_ACK | TCP_FIN
                payload = b""
            else:
                connection.retry_deadline = None
                continue
            segment = tcp(self.ip, self.guest_ip, TCP_ECHO_PORT, connection.guest_port, sequence, connection.receive_sequence, flags, payload)
            replies.append(self.emit(ethernet(connection.guest_mac, self.mac, ETHERTYPE_IPV4, ipv4(self.ip, self.guest_ip, IP_PROTOCOL_TCP, segment))))
            connection.retransmissions += 1
            connection.retry_deadline = now + TCP_RETRY_INTERVAL_SECONDS
        return replies

    def tcp_reply(self, frame: Frame, sequence: int, acknowledgment: int, flags: int, payload: bytes = b"", window: int = TCP_STREAM_LIMIT) -> bytes:
        assert frame.tcp_source_port is not None and frame.tcp_destination_port is not None
        segment = tcp(self.ip, self.guest_ip, frame.tcp_destination_port, frame.tcp_source_port, sequence, acknowledgment, flags, payload, window)
        return self.emit(ethernet(frame.source, self.mac, ETHERTYPE_IPV4, ipv4(self.ip, self.guest_ip, IP_PROTOCOL_TCP, segment)))

    def handle_tcp(self, frame: Frame) -> list[bytes]:
        if frame.destination != self.mac or frame.ip_destination != self.ip or frame.ip_source != self.guest_ip or not frame.tcp_source_port:
            return []
        if frame.tcp_destination_port == TCP_REFUSED_PORT:
            if frame.tcp_flags == TCP_SYN and not frame.tcp_payload:
                self.ledger.tcp_resets_sent += 1
                return [self.tcp_reply(frame, 0, frame.tcp_sequence + 1, TCP_RST | TCP_ACK)]
            return []
        if frame.tcp_destination_port != TCP_ECHO_PORT:
            return []
        connection = next((entry for entry in self.ledger.tcp_connections if entry.guest_port == frame.tcp_source_port and entry.guest_mac == frame.source and not entry.abandoned), None)
        if frame.tcp_flags == TCP_SYN and not frame.tcp_payload:
            # The reset fixture deliberately discards its held server session
            # before accepting a fresh connection. This is not a wire FIN/RST.
            if connection is not None and connection.held and len(connection.received) == 1024 and frame.tcp_sequence != connection.guest_initial_sequence:
                connection.abandoned = True
                connection.retry_deadline = None
                connection = None
            if connection is None:
                if len(self.ledger.tcp_connections) >= TCP_CONNECTION_LIMIT:
                    return []
                connection = TcpConnection(frame.source, frame.tcp_source_port, frame.tcp_sequence, 0x534C0000 + len(self.ledger.tcp_connections) * 0x10000, held=self.hold_first_tcp and not self.ledger.tcp_connections)
                offset = 0
                while offset < len(frame.tcp_options) and frame.tcp_options[offset] != 0:
                    kind = frame.tcp_options[offset]
                    if kind == 2:
                        connection.segment_limit = min(TCP_SEGMENT_LIMIT, int.from_bytes(frame.tcp_options[offset + 2 : offset + 4], "big"))
                    offset += 1 if kind == 1 else frame.tcp_options[offset + 1]
                self.ledger.tcp_connections.append(connection)
            if connection.handshake or connection.guest_initial_sequence != frame.tcp_sequence:
                return []
            return [self.tcp_reply(frame, connection.peer_initial_sequence, connection.receive_sequence, TCP_SYN | TCP_ACK)]
        if connection is None or connection.reset:
            return []
        if frame.tcp_flags & TCP_RST:
            if frame.tcp_sequence == connection.receive_sequence:
                connection.reset = True
            return []
        if frame.tcp_flags & ~(TCP_ACK | TCP_PSH | TCP_FIN) or not frame.tcp_flags & TCP_ACK:
            return []
        acknowledgment = (frame.tcp_acknowledgment - connection.peer_initial_sequence - 1) & TCP_SEQUENCE_MASK
        if acknowledgment > len(connection.sent) + int(connection.peer_fin):
            return []
        if not connection.handshake:
            if frame.tcp_sequence != connection.receive_sequence or acknowledgment != 0:
                return []
            connection.handshake = True
        sequence_offset = (frame.tcp_sequence - connection.guest_initial_sequence - 1) & TCP_SEQUENCE_MASK
        if sequence_offset > len(connection.received) + int(connection.guest_fin):
            return [self.tcp_reply(frame, connection.send_sequence, connection.receive_sequence, TCP_ACK, window=TCP_STREAM_LIMIT)]
        acknowledged_bytes = min(acknowledgment, len(connection.sent))
        if acknowledged_bytes > len(connection.acknowledged):
            connection.acknowledged.extend(connection.sent[len(connection.acknowledged) : acknowledged_bytes])
        if connection.peer_fin and acknowledgment == len(connection.sent) + 1:
            connection.closed = True
        overlap = min(len(frame.tcp_payload), max(0, len(connection.received) - sequence_offset))
        if frame.tcp_payload[:overlap] != connection.received[sequence_offset : sequence_offset + overlap]:
            return []
        new_payload = frame.tcp_payload[overlap:]
        if new_payload and (connection.guest_fin or len(connection.received) + len(new_payload) > TCP_STREAM_LIMIT):
            return [self.tcp_reply(frame, connection.send_sequence, connection.receive_sequence, TCP_ACK, window=TCP_STREAM_LIMIT)]
        connection.received.extend(new_payload)
        if frame.tcp_flags & TCP_FIN and sequence_offset + len(frame.tcp_payload) == len(connection.received):
            connection.guest_fin = True
        replies = []
        available = 0 if connection.held else max(0, frame.tcp_window - (len(connection.sent) - len(connection.acknowledged)))
        while len(connection.sent) < len(connection.received) and available:
            count = min(connection.segment_limit, available, len(connection.received) - len(connection.sent))
            payload = bytes(connection.received[len(connection.sent) : len(connection.sent) + count])
            replies.append(self.tcp_reply(frame, connection.send_sequence, connection.receive_sequence, TCP_ACK | TCP_PSH, payload, TCP_STREAM_LIMIT))
            connection.sent.extend(payload)
            available -= count
        if connection.guest_fin and not connection.peer_fin and len(connection.sent) == len(connection.received) and available:
            replies.append(self.tcp_reply(frame, connection.send_sequence, connection.receive_sequence, TCP_ACK | TCP_FIN, window=TCP_STREAM_LIMIT))
            connection.peer_fin = True
        elif frame.tcp_flags & TCP_FIN and connection.peer_fin and not connection.closed and not replies:
            replies.append(self.tcp_reply(frame, connection.send_sequence - 1, connection.receive_sequence, TCP_ACK | TCP_FIN))
        elif (frame.tcp_payload or frame.tcp_flags & TCP_FIN) and not replies:
            replies.append(self.tcp_reply(frame, connection.send_sequence, connection.receive_sequence, TCP_ACK, window=TCP_STREAM_LIMIT))
        return replies

    def echo_probe_ready(self) -> bool:
        if not self.hold_first_tcp:
            return True
        return len(self.ledger.tcp_connections) == 2 and self.ledger.tcp_connections[1].handshake

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


def qualify_tcp_ledger(ledger: Ledger, guest_mac: bytes) -> None:
    """Require packet-derived IO11 evidence, independently of service counters."""
    def require(condition: bool, message: str) -> None:
        if not condition:
            raise ValueError(message)

    require(ledger.malformed_received == 0, "unclassifiable Ethernet frames were received")
    require(ledger.guest_mac == guest_mac and not ledger.foreign_sources(guest_mac), "guest MAC was missing or foreign")
    require(ledger.count_received("arp-reply") >= 1, "guest ARP reply missing")
    for frame in ledger.received:
        require(frame.destination in (PEER_MAC, BROADCAST), "undeclared Ethernet destination")
        if frame.ethertype == ETHERTYPE_ARP:
            require(frame.arp_operation in (ARP_REQUEST, ARP_REPLY) and frame.arp_sender_mac == guest_mac and frame.arp_sender_ip == GUEST_IP and frame.arp_target_ip in (PEER_IP, GUEST_IP), "malformed or undeclared ARP target/source")
        else:
            require(frame.ethertype == ETHERTYPE_IPV4 and frame.ip_source == GUEST_IP and frame.ip_destination == PEER_IP, "malformed or undeclared IPv4 source/destination")
            require(frame.ip_protocol in (IP_PROTOCOL_TCP, IP_PROTOCOL_ICMP), "undeclared IPv4 protocol")
    icmp = [frame for frame in ledger.received if frame.ip_protocol == IP_PROTOCOL_ICMP]
    require(len(icmp) == ECHO_COUNT, "missing or duplicate ICMP evidence")
    require(sorted(frame.icmp_sequence or 0 for frame in icmp) == list(range(1, ECHO_COUNT + 1)), "ICMP sequence evidence differs")
    for frame in icmp:
        require(frame.icmp_type == ICMP_ECHO_REPLY and frame.icmp_identifier == ECHO_IDENTIFIER and frame.icmp_payload == ECHO_PAYLOAD and checksum(frame.ip_payload) == 0, "ICMP reply bytes or checksum differ")
        require(frame.ip_payload == icmp_echo(ICMP_ECHO_REPLY, ECHO_IDENTIFIER, frame.icmp_sequence or 0, ECHO_PAYLOAD), "ICMP decoded evidence disagrees with packet")
        require(any(sent.icmp_type == ICMP_ECHO_REQUEST and sent.icmp_identifier == frame.icmp_identifier and sent.icmp_sequence == frame.icmp_sequence and sent.icmp_payload == frame.icmp_payload for sent in ledger.sent), "ICMP reply lacks matching emitted request")
    require(len(ledger.tcp_connections) == 1, "expected exactly one TCP connection")
    connection = ledger.tcp_connections[0]
    expected = bytes((index * 37 + 11) & 255 for index in range(TCP_STREAM_LIMIT))
    require(connection.guest_mac == guest_mac and connection.handshake and connection.guest_fin and connection.peer_fin and connection.closed and not connection.reset, "TCP handshake or graceful close incomplete")
    require(connection.received == expected and connection.sent == expected and connection.acknowledged == expected, "TCP stream ledger is not the exact acknowledged 4096-byte echo")
    incoming = [frame for frame in ledger.received if frame.ip_protocol == IP_PROTOCOL_TCP]
    outgoing = [frame for frame in ledger.sent if frame.ip_protocol == IP_PROTOCOL_TCP]
    for frames, from_guest in ((incoming, True), (outgoing, False)):
        for frame in frames:
            source_ip, destination_ip = (GUEST_IP, PEER_IP) if from_guest else (PEER_IP, GUEST_IP)
            require(frame.ip_source == source_ip and frame.ip_destination == destination_ip, "TCP packet has undeclared IP endpoint")
            require(frame.source == (guest_mac if from_guest else PEER_MAC) and frame.destination == (PEER_MAC if from_guest else guest_mac), "TCP packet has undeclared MAC endpoint")
            require(frame.tcp_source_port is not None and frame.tcp_destination_port is not None, "TCP checksum or header was invalid")
            reparsed = decode(ethernet(frame.destination, frame.source, ETHERTYPE_IPV4, ipv4(source_ip, destination_ip, IP_PROTOCOL_TCP, frame.ip_payload)))
            require(reparsed == frame, "TCP decoded evidence disagrees with checksum-validated packet")
            peer_port = frame.tcp_destination_port if from_guest else frame.tcp_source_port
            require(peer_port in (TCP_ECHO_PORT, TCP_REFUSED_PORT), "TCP packet addressed undeclared port")
            if peer_port == TCP_ECHO_PORT:
                require((frame.tcp_source_port if from_guest else frame.tcp_destination_port) == connection.guest_port, "TCP packet has unexpected client port")
    guest = [frame for frame in incoming if frame.tcp_destination_port == TCP_ECHO_PORT]
    peer = [frame for frame in outgoing if frame.tcp_source_port == TCP_ECHO_PORT]
    guest_start = (connection.guest_initial_sequence + 1) & TCP_SEQUENCE_MASK
    peer_start = (connection.peer_initial_sequence + 1) & TCP_SEQUENCE_MASK
    require(any(frame.tcp_flags == TCP_SYN and frame.tcp_sequence == connection.guest_initial_sequence for frame in guest), "wire SYN missing")
    require(any(frame.tcp_flags == TCP_SYN | TCP_ACK and frame.tcp_sequence == connection.peer_initial_sequence and frame.tcp_acknowledgment == guest_start for frame in peer), "wire SYN-ACK missing")
    require(any(frame.tcp_flags & TCP_ACK and frame.tcp_sequence == guest_start and frame.tcp_acknowledgment == peer_start for frame in guest), "wire handshake ACK missing")

    def stream(frames: list[Frame], initial_sequence: int) -> bytes:
        observed: dict[int, int] = {}
        for frame in frames:
            offset = (frame.tcp_sequence - initial_sequence) & TCP_SEQUENCE_MASK
            if frame.tcp_payload:
                require(not frame.tcp_flags & (TCP_SYN | TCP_RST) and frame.tcp_flags & TCP_ACK != 0, "stream payload has invalid flags")
                require(offset + len(frame.tcp_payload) <= TCP_STREAM_LIMIT, "wire stream exceeds expected bounds")
                for index, value in enumerate(frame.tcp_payload, offset):
                    require(index not in observed or observed[index] == value, "wire retransmission changed bytes")
                    observed[index] = value
        require(len(observed) == TCP_STREAM_LIMIT, "wire stream has missing bytes")
        return bytes(observed[index] for index in range(TCP_STREAM_LIMIT))

    require(stream(guest, guest_start) == expected and stream(peer, peer_start) == expected, "wire stream bytes differ from expected echo")
    guest_end = (guest_start + TCP_STREAM_LIMIT) & TCP_SEQUENCE_MASK
    peer_end = (peer_start + TCP_STREAM_LIMIT) & TCP_SEQUENCE_MASK
    require(any(frame.tcp_flags & TCP_FIN and (frame.tcp_sequence + len(frame.tcp_payload)) & TCP_SEQUENCE_MASK == guest_end for frame in guest), "wire guest FIN missing")
    require(any(frame.tcp_flags & TCP_FIN and (frame.tcp_sequence + len(frame.tcp_payload)) & TCP_SEQUENCE_MASK == peer_end and frame.tcp_acknowledgment == (guest_end + 1) & TCP_SEQUENCE_MASK for frame in peer), "wire peer FIN missing")
    require(any(frame.tcp_flags == TCP_ACK and frame.tcp_sequence == (guest_end + 1) & TCP_SEQUENCE_MASK and frame.tcp_acknowledgment == (peer_end + 1) & TCP_SEQUENCE_MASK for frame in guest), "wire final ACK missing")
    refused = [frame for frame in incoming if frame.tcp_destination_port == TCP_REFUSED_PORT]
    resets = [frame for frame in outgoing if frame.tcp_source_port == TCP_REFUSED_PORT]
    require(bool(refused) and bool(resets) and ledger.tcp_resets_sent == len(resets), "TCP refusal evidence missing")
    require(all(frame.tcp_flags == TCP_SYN and not frame.tcp_payload for frame in refused), "refused port received unexpected traffic")
    for reset in resets:
        require(reset.tcp_flags == TCP_RST | TCP_ACK and not reset.tcp_payload and reset.tcp_sequence == 0, "refused port did not send valid reset")
        require(any(frame.tcp_source_port == reset.tcp_destination_port and (frame.tcp_sequence + 1) & TCP_SEQUENCE_MASK == reset.tcp_acknowledgment for frame in refused), "TCP reset lacks matching SYN")
    require(not any(frame.tcp_flags & TCP_RST for frame in guest + peer), "echo connection was reset")


def qualify_driver_reset_ledger(ledger: Ledger, guest_mac: bytes) -> None:
    """Prove held pre-reset bytes and independently qualify the fresh stream."""
    def require(condition: bool, message: str) -> None:
        if not condition:
            raise ValueError(message)

    require(len(ledger.tcp_connections) == 2, "reset proof requires exactly two TCP incarnations")
    held, fresh = ledger.tcp_connections
    require(held.held and held.abandoned and held.handshake and not held.reset and not held.closed and not held.guest_fin and not held.peer_fin, "held session was not an explicit server abandonment")
    require(not fresh.held and not fresh.abandoned and fresh.guest_initial_sequence != held.guest_initial_sequence, "fresh session identity was not distinct")
    expected = bytes((index * 37 + 11) & 255 for index in range(1024))
    require(held.received == expected and not held.sent and not held.acknowledged, "held session bytes differ or were echoed")
    require(held.guest_mac == guest_mac, "held session MAC differs")
    incoming_cut = next((index for index, frame in enumerate(ledger.received) if frame.ip_protocol == IP_PROTOCOL_TCP and frame.tcp_flags == TCP_SYN and frame.tcp_sequence == fresh.guest_initial_sequence and frame.tcp_source_port == fresh.guest_port), None)
    outgoing_cut = next((index for index, frame in enumerate(ledger.sent) if frame.ip_protocol == IP_PROTOCOL_TCP and frame.tcp_flags == TCP_SYN | TCP_ACK and frame.tcp_sequence == fresh.peer_initial_sequence and frame.tcp_destination_port == fresh.guest_port and frame.tcp_acknowledgment == (fresh.guest_initial_sequence + 1) & TCP_SEQUENCE_MASK), None)
    require(incoming_cut is not None and outgoing_cut is not None, "fresh wire handshake boundary missing")
    assert incoming_cut is not None and outgoing_cut is not None
    first_guest = [frame for frame in ledger.received[:incoming_cut] if frame.ip_protocol == IP_PROTOCOL_TCP]
    first_peer = [frame for frame in ledger.sent[:outgoing_cut] if frame.ip_protocol == IP_PROTOCOL_TCP]
    for frames, from_guest in ((first_guest, True), (first_peer, False)):
        for frame in frames:
            source_ip, destination_ip = (GUEST_IP, PEER_IP) if from_guest else (PEER_IP, GUEST_IP)
            require(frame.ip_source == source_ip and frame.ip_destination == destination_ip, "held wire addressed undeclared IP")
            require(frame.source == (guest_mac if from_guest else PEER_MAC) and frame.destination == (PEER_MAC if from_guest else guest_mac), "held wire addressed undeclared MAC")
            require(frame.tcp_source_port == (held.guest_port if from_guest else TCP_ECHO_PORT) and frame.tcp_destination_port == (TCP_ECHO_PORT if from_guest else held.guest_port), "held wire addressed undeclared port")
            reparsed = decode(ethernet(frame.destination, frame.source, ETHERTYPE_IPV4, ipv4(source_ip, destination_ip, IP_PROTOCOL_TCP, frame.ip_payload)))
            require(reparsed == frame and frame.tcp_source_port is not None, "held TCP checksum or decoded evidence differs")
            require(not frame.tcp_flags & (TCP_RST | TCP_FIN), "held wire unexpectedly closed or reset")
    guest_start = (held.guest_initial_sequence + 1) & TCP_SEQUENCE_MASK
    peer_start = (held.peer_initial_sequence + 1) & TCP_SEQUENCE_MASK
    require(any(frame.tcp_flags == TCP_SYN and frame.tcp_sequence == held.guest_initial_sequence for frame in first_guest), "held SYN missing")
    require(any(frame.tcp_flags == TCP_SYN | TCP_ACK and frame.tcp_sequence == held.peer_initial_sequence and frame.tcp_acknowledgment == guest_start for frame in first_peer), "held SYN-ACK missing")
    require(any(frame.tcp_flags == TCP_ACK and frame.tcp_sequence == guest_start and frame.tcp_acknowledgment == peer_start for frame in first_guest), "held handshake ACK missing")
    observed: dict[int, int] = {}
    for frame in first_guest:
        if frame.tcp_payload:
            offset = (frame.tcp_sequence - guest_start) & TCP_SEQUENCE_MASK
            require(frame.tcp_flags in (TCP_ACK, TCP_ACK | TCP_PSH) and frame.tcp_acknowledgment == peer_start, "held data sequence/flags differ")
            require(offset + len(frame.tcp_payload) <= 1024, "held wire exceeds bounded prefix")
            for index, value in enumerate(frame.tcp_payload, offset):
                require(index not in observed or observed[index] == value, "held retransmission changed bytes")
                observed[index] = value
    require(len(observed) == 1024 and bytes(observed[index] for index in range(1024)) == expected, "held wire bytes missing or incorrect")
    require(not any(frame.tcp_payload for frame in first_peer), "held peer emitted echo bytes")
    require(any(frame.tcp_flags == TCP_ACK and frame.tcp_sequence == peer_start and frame.tcp_acknowledgment == (guest_start + 1024) & TCP_SEQUENCE_MASK for frame in first_peer), "held full-stream ACK missing")
    filtered = dataclasses.replace(
        ledger,
        tcp_connections=[fresh],
        received=[frame for index, frame in enumerate(ledger.received) if index >= incoming_cut or frame.ip_protocol != IP_PROTOCOL_TCP],
        sent=[frame for index, frame in enumerate(ledger.sent) if index >= outgoing_cut or frame.ip_protocol != IP_PROTOCOL_TCP],
    )
    qualify_tcp_ledger(filtered, guest_mac)


def serve(receiver: socket.socket, qemu_port: int, stop: threading.Event, peer: Peer) -> None:
    """Run `peer` against QEMU's UDP backend until `stop` is set.

    Every 250 ms the peer asks for the guest's MAC until it has it, then sends
    one ICMP echo request per interval up to `ECHO_COUNT`. The reset fixture
    starts these requests only after its fresh second handshake, so it proves
    recovered ICMP traffic rather than ICMP loss recovery during device reset.
    Replies to the guest's own ARP and echo requests go out as they arrive.
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
            for reply in peer.retransmit(now):
                sender.sendto(reply, ("127.0.0.1", qemu_port))
            if now >= next_probe:
                next_probe = now + ECHO_INTERVAL_SECONDS
                if peer.ledger.guest_mac is None:
                    sender.sendto(peer.arp_request_for_guest(), ("127.0.0.1", qemu_port))
                elif sequence < ECHO_COUNT and peer.echo_probe_ready():
                    sequence += 1
                    request = peer.echo_request(ECHO_IDENTIFIER, sequence, ECHO_PAYLOAD)
                    if request is not None:
                        sender.sendto(request, ("127.0.0.1", qemu_port))
    finally:
        sender.close()
