"""Scripted peer for declared TCP socket buffer sizes.

Two guest holders declare different socket buffers: `buffers-large` declares
65536 bytes each way and `buffers-baseline` keeps the service's 2048. Each
holder runs three scenarios on its own destination ports, the large holder
first:

* tx-flight: the guest streams 262144 bytes. The peer acknowledges every
  segment until it holds 131072 bytes, so Reno's congestion window grows past
  the large buffer, then withholds acknowledgments for 600 ms. Whatever the
  guest has unacknowledged at the end of that hold is its transmit buffer.
* rx-window: the peer streams twice the holder's receive buffer while the
  probe does not read. The guest's first zero window acknowledges exactly its
  receive buffer. Its SYN carries the window scale its buffer implies.
* throughput: the guest streams 1048576 bytes to a peer that acknowledges
  every segment at once, and the qualifier reports the elapsed time.

The peer advertises an MSS of 1024, which divides both buffers, and a
131072-byte window with window scale shift 2, so neither limits the guest
before its own buffers do. Frames carry a timestamp and a global order.

The pure judges at the end hold the network service's per-connection reports,
the composition's rows and the generation builder's verdicts to the same
declared values. Encoders, the decoder and addressing are `link_peer`'s.
Standard library only.
"""

from __future__ import annotations

import dataclasses
import re
import socket
import threading
import time
from collections.abc import Callable, Mapping, Sequence

import link_peer as lp


@dataclasses.dataclass(frozen=True)
class Buffers:
    """One holder's declared socket buffers, as the service must apply them."""

    rx_bytes: int
    tx_bytes: int


LARGE = Buffers(rx_bytes=65536, tx_bytes=65536)
BASELINE = Buffers(rx_bytes=2048, tx_bytes=2048)
HOLDERS: dict[str, Buffers] = {"large": LARGE, "baseline": BASELINE}
HOLDER_NAMES: dict[str, str] = {"large": "buffers-large", "baseline": "buffers-baseline"}
# The large holder runs first; the baseline holder starts after its last close.
HOLDER_ORDER: tuple[str, ...] = ("large", "baseline")

SCENARIO_ORDER: tuple[str, ...] = ("tx-flight", "rx-window", "throughput")
HOLDER_PORTS: dict[str, tuple[int, ...]] = {
    "large": (4300, 4301, 4302),
    "baseline": (4310, 4311, 4312),
}
SCENARIOS: dict[int, str] = {port: name for ports in HOLDER_PORTS.values() for port, name in zip(ports, SCENARIO_ORDER, strict=True)}
PORT_HOLDER: dict[int, str] = {port: holder for holder, ports in HOLDER_PORTS.items() for port in ports}

# network-application/v4: each buffer field admits a power of two from 2048
# to 65536. The builder must accept every admitted value and refuse these.
ADMITTED_BUFFER_BYTES: tuple[int, ...] = (2048, 4096, 8192, 16384, 32768, 65536)
REFUSED_BUFFER_BYTES: tuple[int, ...] = (0, 1024, 3072, 131072)
BUFFER_FIELDS: tuple[str, ...] = ("rxBufferBytes", "txBufferBytes")
POOL_SOCKETS = 4

# The peer's own advertisement: a 1024-byte MSS divides both buffers, so
# Nagle cannot hold a short tail below either limit, and a 131072-byte window
# (field 32768, shift 2) never limits the guest before its buffers do.
PEER_MSS = 1024
PEER_WINDOW_SHIFT = 2
PEER_WINDOW_FIELD = 32768
PEER_WINDOW = PEER_WINDOW_FIELD << PEER_WINDOW_SHIFT
SYN_WINDOW_LIMIT = 0xFFFF
SYNACK_OPTIONS = b"\x02\x04" + PEER_MSS.to_bytes(2, "big") + b"\x01\x03\x03" + bytes([PEER_WINDOW_SHIFT])


def pattern(seed: int, length: int) -> bytes:
    """Byte `i` of every scripted stream is `(31 * i + seed) mod 256`."""
    return bytes((index * 31 + seed) & 255 for index in range(length))


# tx-flight: the stream fills the large buffer four times. The peer
# acknowledges every segment until it holds RAMP_BYTES, which grows Reno's
# congestion window (2048 bytes plus every acknowledged byte in slow start)
# past the large buffer, and then withholds acknowledgments for HOLD_SECONDS,
# well inside smoltcp's one-second retransmission floor.
TX_STREAM_BYTES = 262144
RAMP_BYTES = 131072
HOLD_SECONDS = 0.6
TX_STREAM = pattern(7, TX_STREAM_BYTES)

# rx-window: the peer streams twice the holder's receive buffer while the
# probe does not read for RECEIVE_STALL_MS.
RECEIVE_STALL_MS = 2000
RX_STREAMS: dict[str, bytes] = {holder: pattern(13 + index, 2 * buffers.rx_bytes) for index, (holder, buffers) in enumerate(HOLDERS.items())}

# throughput: a fixed stream acknowledged at once; only its bytes are judged.
THROUGHPUT_BYTES = 1048576
THROUGHPUT_STREAM = pattern(11, THROUGHPUT_BYTES)

# The peer's whole stream on a completed guest stream: one completion byte.
DONE = b"D"


def guest_stream_for(port: int) -> bytes:
    return {"tx-flight": TX_STREAM, "rx-window": b"", "throughput": THROUGHPUT_STREAM}[SCENARIOS[port]]


def peer_stream_for(port: int) -> bytes:
    return RX_STREAMS[PORT_HOLDER[port]] if SCENARIOS[port] == "rx-window" else DONE


def expected_shift(rx_bytes: int) -> int:
    """The window scale smoltcp derives from a receive buffer: its bit length less 16."""
    return max(0, rx_bytes.bit_length() - 16)


def option(options: bytes, kind: int) -> bytes | None:
    """The body of the first TCP option of `kind`, or None; `options` is already validated."""
    offset = 0
    while offset < len(options):
        current = options[offset]
        if current == 0:
            return None
        if current == 1:
            offset += 1
            continue
        length = options[offset + 1]
        if current == kind:
            return options[offset + 2 : offset + length]
        offset += length
    return None


def window_shift(options: bytes) -> int | None:
    body = option(options, 3)
    return body[0] if body is not None and len(body) == 1 else None


def mss_of(options: bytes) -> int | None:
    body = option(options, 2)
    return int.from_bytes(body, "big") if body is not None and len(body) == 2 else None


CONNECTION_LIMIT = 16
RETRY_INTERVAL_SECONDS = 0.5
RETRY_LIMIT = 16
PEER_ISN_BASE = 0x42500000
MASK = lp.TCP_SEQUENCE_MASK


@dataclasses.dataclass(frozen=True)
class Observation:
    """A frame, when the peer met it, and whether it crossed the wire."""

    order: int
    time: float
    frame: lp.Frame
    delivered: bool = True


@dataclasses.dataclass
class Connection:
    """One guest connection and the script state shaping the peer's side of it."""

    scenario: str
    port: int
    guest_mac: bytes
    guest_port: int
    guest_isn: int
    peer_isn: int
    guest_shift: int | None = None
    guest_mss: int = 536
    guest_window: int = 0
    expected: bytes = b""
    peer_stream: bytes = b""
    received: bytearray = dataclasses.field(default_factory=bytearray)
    held: dict[int, bytes] = dataclasses.field(default_factory=dict)
    handshake: bool = False
    hold_until: float | None = None
    hold_done: bool = False
    peer_next: int = 0
    peer_ready: bool = False
    guest_ack: int = 0
    guest_fin: bool = False
    peer_fin: bool = False
    closed: bool = False
    reset: bool = False
    retransmissions: int = 0
    retry_due: float | None = None

    @property
    def receive_sequence(self) -> int:
        return (self.guest_isn + 1 + len(self.received) + int(self.guest_fin)) & MASK

    def peer_sequence(self, offset: int) -> int:
        return (self.peer_isn + 1 + offset) & MASK

    @property
    def holding(self) -> bool:
        return self.hold_until is not None


@dataclasses.dataclass
class Ledger:
    """Every frame the peer saw or produced, and the script state behind them."""

    received: list[Observation] = dataclasses.field(default_factory=list)
    sent: list[Observation] = dataclasses.field(default_factory=list)
    connections: list[Connection] = dataclasses.field(default_factory=list)
    guest_mac: bytes | None = None
    malformed_received: int = 0

    def summary(self) -> str:
        lines = [f"received {len(self.received)}; sent {len(self.sent)}; malformed={self.malformed_received}"]
        lines.extend(
            f"{entry.scenario} port={entry.port} guest_port={entry.guest_port} received={len(entry.received)} peer_sent={entry.peer_next} acknowledged={entry.guest_ack} handshake={int(entry.handshake)} guest_fin={int(entry.guest_fin)} peer_fin={int(entry.peer_fin)} closed={int(entry.closed)} reset={int(entry.reset)} retransmissions={entry.retransmissions}"
            for entry in self.connections
        )
        return "; ".join(lines)


class BuffersPeer:
    """The pure half: a frame and the current time in, frames for the wire out."""

    def __init__(self, mac: bytes = lp.PEER_MAC, ip: bytes = lp.PEER_IP, guest_ip: bytes = lp.GUEST_IP) -> None:
        self.mac = mac
        self.ip = ip
        self.guest_ip = guest_ip
        self.ledger = Ledger()
        self.order = 0

    def next_order(self) -> int:
        self.order += 1
        return self.order

    def handle(self, raw: bytes, now: float) -> list[bytes]:
        frame = lp.decode(raw)
        if frame is None:
            self.ledger.malformed_received += 1
            return []
        order = self.next_order()
        delivered = True
        replies: list[bytes] = []
        if frame.kind == "arp-request" and frame.arp_target_ip == self.ip and frame.arp_sender_mac and frame.arp_sender_ip == self.guest_ip:
            self.ledger.guest_mac = frame.arp_sender_mac
            reply = lp.ethernet(frame.source, self.mac, lp.ETHERTYPE_ARP, lp.arp(lp.ARP_REPLY, self.mac, self.ip, frame.arp_sender_mac, frame.arp_sender_ip))
            replies.append(self.record(reply, now))
        elif frame.kind == "tcp":
            delivered, tcp_replies = self.handle_tcp(frame, now)
            replies.extend(tcp_replies)
        self.ledger.received.append(Observation(order, now, frame, delivered))
        return replies

    def record(self, raw: bytes, now: float) -> bytes:
        decoded = lp.decode(raw)
        assert decoded is not None
        self.ledger.sent.append(Observation(self.next_order(), now, decoded))
        return raw

    def send(self, connection: Connection, now: float, sequence: int, flags: int, payload: bytes = b"", options: bytes = b"") -> bytes:
        window = SYN_WINDOW_LIMIT if flags & lp.TCP_SYN else PEER_WINDOW_FIELD
        segment = lp.tcp(self.ip, self.guest_ip, connection.port, connection.guest_port, sequence, connection.receive_sequence, flags, payload, window, options)
        return self.record(lp.ethernet(connection.guest_mac, self.mac, lp.ETHERTYPE_IPV4, lp.ipv4(self.ip, self.guest_ip, lp.IP_PROTOCOL_TCP, segment)), now)

    def acknowledge(self, connection: Connection, now: float) -> bytes:
        return self.send(connection, now, connection.peer_sequence(connection.peer_next + int(connection.peer_fin)), lp.TCP_ACK)

    def connection_for(self, frame: lp.Frame) -> Connection | None:
        return next((entry for entry in self.ledger.connections if entry.port == frame.tcp_destination_port and entry.guest_port == frame.tcp_source_port and entry.guest_mac == frame.source), None)

    def handle_tcp(self, frame: lp.Frame, now: float) -> tuple[bool, list[bytes]]:
        if frame.destination != self.mac or frame.ip_destination != self.ip or frame.ip_source != self.guest_ip or not frame.tcp_source_port:
            return True, []
        port = frame.tcp_destination_port or 0
        scenario = SCENARIOS.get(port)
        if scenario is None:
            return True, []
        connection = self.connection_for(frame)
        if frame.tcp_flags == lp.TCP_SYN and not frame.tcp_payload:
            if connection is None:
                if len(self.ledger.connections) >= CONNECTION_LIMIT:
                    return True, []
                connection = Connection(
                    scenario,
                    port,
                    frame.source,
                    frame.tcp_source_port,
                    frame.tcp_sequence,
                    PEER_ISN_BASE + len(self.ledger.connections) * 0x100000,
                    guest_shift=window_shift(frame.tcp_options),
                    guest_mss=mss_of(frame.tcp_options) or 536,
                    guest_window=frame.tcp_window,
                    expected=guest_stream_for(port),
                    peer_stream=peer_stream_for(port),
                )
                self.ledger.connections.append(connection)
            if connection.handshake or frame.tcp_sequence != connection.guest_isn:
                return True, []
            self.schedule(connection, now)
            return True, [self.send(connection, now, connection.peer_isn, lp.TCP_SYN | lp.TCP_ACK, options=SYNACK_OPTIONS)]
        if connection is None or connection.reset:
            return True, []
        if frame.tcp_flags & lp.TCP_RST:
            if frame.tcp_sequence == connection.receive_sequence:
                connection.reset = True
                connection.retry_due = None
            return True, []
        if frame.tcp_flags & ~(lp.TCP_ACK | lp.TCP_PSH | lp.TCP_FIN) or not frame.tcp_flags & lp.TCP_ACK:
            return True, []
        offset = (frame.tcp_sequence - connection.guest_isn - 1) & MASK
        acknowledged = (frame.tcp_acknowledgment - connection.peer_isn - 1) & MASK
        if acknowledged > connection.peer_next + int(connection.peer_fin):
            return True, []
        if not connection.handshake:
            if offset != 0 or acknowledged != 0:
                return True, []
            connection.handshake = True
            connection.peer_ready = scenario == "rx-window"
        if acknowledged >= connection.guest_ack:
            if acknowledged > connection.guest_ack:
                connection.retry_due = None
            reopened = connection.handshake and connection.guest_window == 0 and frame.tcp_window != 0
            connection.guest_ack = acknowledged
            scale = connection.guest_shift if connection.guest_shift is not None else 0
            connection.guest_window = frame.tcp_window << scale
            if reopened:
                # A window probe the closed window dropped is resent from the
                # guest's acknowledgment rather than left as a hole.
                connection.peer_next = min(connection.peer_next, connection.guest_ack)
        if connection.peer_fin and acknowledged == connection.peer_next + 1:
            connection.closed = True
        replies: list[bytes] = []
        if offset > len(connection.received) + int(connection.guest_fin):
            if frame.tcp_payload and offset + len(frame.tcp_payload) <= len(connection.expected):
                connection.held[offset] = frame.tcp_payload
            replies.append(self.acknowledge(connection, now))
            self.schedule(connection, now)
            return True, replies
        overlap = min(len(frame.tcp_payload), max(0, len(connection.received) - offset))
        if frame.tcp_payload[:overlap] != connection.received[offset : offset + overlap]:
            return True, []
        fresh = frame.tcp_payload[overlap:]
        if not connection.guest_fin and fresh:
            room = len(connection.expected) - len(connection.received)
            connection.received.extend(fresh[: max(0, room)])
            while connection.held:
                start = min(connection.held)
                if start > len(connection.received):
                    break
                segment = connection.held.pop(start)
                connection.received.extend(segment[len(connection.received) - start :])
        if frame.tcp_flags & lp.TCP_FIN and offset + len(frame.tcp_payload) == len(connection.received):
            connection.guest_fin = True
        if scenario == "tx-flight" and not connection.hold_done and (connection.holding or len(connection.received) >= RAMP_BYTES):
            # Every acknowledgment is withheld from the segment that reaches
            # the ramp until the hold ends.
            if connection.hold_until is None:
                connection.hold_until = now + HOLD_SECONDS
            self.schedule(connection, now)
            return True, []
        if frame.tcp_payload or frame.tcp_flags & lp.TCP_FIN:
            replies.append(self.acknowledge(connection, now))
        replies.extend(self.pump(connection, now))
        self.schedule(connection, now)
        return True, replies

    def segment_limit(self, connection: Connection) -> int:
        return min(PEER_MSS, connection.guest_mss)

    def pump(self, connection: Connection, now: float) -> list[bytes]:
        """Send the peer's stream inside the guest's window, then answer the guest's FIN."""
        if not connection.handshake or connection.reset or connection.holding:
            return []
        replies: list[bytes] = []
        finished = len(connection.received) == len(connection.expected)
        if connection.scenario != "rx-window" and finished and not connection.peer_ready:
            connection.peer_ready = True
        if connection.peer_ready:
            edge = min(len(connection.peer_stream), connection.guest_ack + connection.guest_window)
            while connection.peer_next < edge:
                end = min(edge, connection.peer_next + self.segment_limit(connection))
                replies.append(self.send(connection, now, connection.peer_sequence(connection.peer_next), lp.TCP_ACK | lp.TCP_PSH, connection.peer_stream[connection.peer_next : end]))
                connection.peer_next = end
        if connection.guest_fin and not connection.peer_fin and connection.peer_next == len(connection.peer_stream) and connection.guest_ack == connection.peer_next:
            replies.append(self.send(connection, now, connection.peer_sequence(connection.peer_next), lp.TCP_ACK | lp.TCP_FIN))
            connection.peer_fin = True
        return replies

    def outstanding(self, connection: Connection) -> bool:
        if not connection.handshake:
            return True
        if connection.peer_fin:
            return not connection.closed
        unsent = connection.peer_ready and connection.peer_next < len(connection.peer_stream)
        return connection.guest_ack < connection.peer_next or unsent

    def schedule(self, connection: Connection, now: float) -> None:
        if connection.reset or connection.closed or not self.outstanding(connection):
            connection.retry_due = None
        elif connection.retry_due is None:
            connection.retry_due = now + RETRY_INTERVAL_SECONDS

    def poll(self, now: float) -> list[bytes]:
        """Advance every timer the script owns: the hold and retransmissions or window probes."""
        replies: list[bytes] = []
        for connection in self.ledger.connections:
            if connection.reset or connection.closed:
                continue
            if connection.hold_until is not None and now >= connection.hold_until:
                connection.hold_until = None
                connection.hold_done = True
                replies.append(self.acknowledge(connection, now))
            replies.extend(self.pump(connection, now))
            if connection.retry_due is not None and now >= connection.retry_due:
                retried = self.retransmit(connection, now)
                if retried is not None:
                    replies.append(retried)
            self.schedule(connection, now)
        return replies

    def retransmit(self, connection: Connection, now: float) -> bytes | None:
        """Resend the oldest thing the guest has not acknowledged; a closed window gets a one-byte probe."""
        connection.retry_due = now + RETRY_INTERVAL_SECONDS
        if connection.retransmissions >= RETRY_LIMIT:
            return None
        if not connection.handshake:
            connection.retransmissions += 1
            return self.send(connection, now, connection.peer_isn, lp.TCP_SYN | lp.TCP_ACK, options=SYNACK_OPTIONS)
        if connection.peer_fin and not connection.closed and connection.guest_ack == connection.peer_next:
            connection.retransmissions += 1
            return self.send(connection, now, connection.peer_sequence(connection.peer_next), lp.TCP_ACK | lp.TCP_FIN)
        start = connection.guest_ack
        if start < len(connection.peer_stream) and connection.peer_ready:
            length = 1 if connection.guest_window == 0 else min(self.segment_limit(connection), connection.guest_window, len(connection.peer_stream) - start)
            connection.retransmissions += 1
            connection.peer_next = max(connection.peer_next, start + length)
            return self.send(connection, now, connection.peer_sequence(start), lp.TCP_ACK | lp.TCP_PSH, connection.peer_stream[start : start + length])
        connection.retry_due = None
        return None


def serve(receiver: socket.socket, qemu_port: int, stop: threading.Event, peer: BuffersPeer) -> None:
    """Run `peer` against QEMU's UDP backend until `stop` is set.

    The guest resolves the peer by ARP before its first SYN, so the peer never
    initiates traffic; every timer is advanced after each receive or timeout.
    """
    sender = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        while not stop.is_set():
            try:
                raw, _ = receiver.recvfrom(4096)
            except socket.timeout:
                raw = None
            if raw is not None:
                for reply in peer.handle(raw, time.monotonic()):
                    sender.sendto(reply, ("127.0.0.1", qemu_port))
            for reply in peer.poll(time.monotonic()):
                sender.sendto(reply, ("127.0.0.1", qemu_port))
    finally:
        sender.close()


# ---------------------------------------------------------------------------
# Qualification: the wire alone, independent of the peer's own state.


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


@dataclasses.dataclass
class Flow:
    """One connection as the wire shows it."""

    port: int
    guest_port: int
    guest: list[Observation]
    peer: list[Observation]
    guest_isn: int = 0
    peer_isn: int = 0
    guest_shift: int = 0

    @property
    def scenario(self) -> str:
        return SCENARIOS[self.port]

    @property
    def holder(self) -> str:
        return PORT_HOLDER[self.port]

    @property
    def buffers(self) -> Buffers:
        return HOLDERS[self.holder]

    def guest_offset(self, frame: lp.Frame) -> int:
        return (frame.tcp_sequence - self.guest_isn - 1) & MASK

    def guest_acked(self, frame: lp.Frame) -> int:
        """How much of the peer's stream a guest frame acknowledges."""
        return (frame.tcp_acknowledgment - self.peer_isn - 1) & MASK

    def peer_offset(self, frame: lp.Frame) -> int:
        return (frame.tcp_sequence - self.peer_isn - 1) & MASK

    def peer_acked(self, frame: lp.Frame) -> int:
        """How much of the guest's stream a peer frame acknowledges."""
        return (frame.tcp_acknowledgment - self.guest_isn - 1) & MASK

    def guest_window(self, frame: lp.Frame) -> int:
        return frame.tcp_window << self.guest_shift

    def merged(self) -> list[tuple[bool, Observation]]:
        """Both directions in wire order; the flag is True for guest frames."""
        return sorted([(True, entry) for entry in self.guest] + [(False, entry) for entry in self.peer], key=lambda item: item[1].order)

    @property
    def last_order(self) -> int:
        return max(entry.order for entry in self.guest + self.peer)


def reparsed(frame: lp.Frame) -> lp.Frame | None:
    assert frame.ip_source is not None and frame.ip_destination is not None
    return lp.decode(lp.ethernet(frame.destination, frame.source, lp.ETHERTYPE_IPV4, lp.ipv4(frame.ip_source, frame.ip_destination, lp.IP_PROTOCOL_TCP, frame.ip_payload)))


def split_flows(ledger: Ledger, guest_mac: bytes) -> list[Flow]:
    flows: dict[tuple[int, int], Flow] = {}
    for observation in ledger.received:
        frame = observation.frame
        require(frame.source == guest_mac, "a frame came from an undeclared source MAC")
        require(frame.destination in (lp.PEER_MAC, lp.BROADCAST), "a frame addressed an undeclared Ethernet destination")
        if frame.ethertype == lp.ETHERTYPE_ARP:
            require(frame.arp_operation in (lp.ARP_REQUEST, lp.ARP_REPLY) and frame.arp_sender_mac == guest_mac and frame.arp_sender_ip == lp.GUEST_IP and frame.arp_target_ip == lp.PEER_IP, "malformed or undeclared ARP")
            continue
        require(frame.kind == "tcp" and frame.ip_source == lp.GUEST_IP and frame.ip_destination == lp.PEER_IP, "the guest emitted undeclared IPv4 traffic")
        require(frame.tcp_source_port is not None and frame.tcp_destination_port in SCENARIOS, "the guest addressed an undeclared TCP port or sent an invalid header")
        require(reparsed(frame) == frame, "decoded guest evidence disagrees with its checksum-validated packet")
        assert frame.tcp_source_port is not None and frame.tcp_destination_port is not None
        key = (frame.tcp_source_port, frame.tcp_destination_port)
        flows.setdefault(key, Flow(key[1], key[0], [], [])).guest.append(observation)
    for observation in ledger.sent:
        frame = observation.frame
        if frame.ethertype == lp.ETHERTYPE_ARP:
            continue
        require(frame.kind == "tcp" and frame.source == lp.PEER_MAC and frame.destination == guest_mac and reparsed(frame) == frame, "the peer produced an undeclared or corrupt frame")
        assert frame.tcp_source_port is not None and frame.tcp_destination_port is not None
        key = (frame.tcp_destination_port, frame.tcp_source_port)
        require(key in flows, "the peer answered a connection the guest never opened")
        flows[key].peer.append(observation)
    return sorted(flows.values(), key=lambda flow: flow.guest[0].order)


def handshake(flow: Flow) -> Observation:
    """Check the handshake, the guest's SYN against its declared receive buffer, and the peer's options."""
    syns = [entry for entry in flow.guest if entry.frame.tcp_flags == lp.TCP_SYN]
    require(bool(syns) and syns[0] is flow.guest[0], "the connection does not begin with the guest's SYN")
    first = syns[0].frame
    require(all(entry.frame.tcp_sequence == first.tcp_sequence and entry.frame.tcp_options == first.tcp_options and entry.frame.tcp_window == first.tcp_window and not entry.frame.tcp_payload for entry in syns), "the guest's SYN changed or carried data")
    flow.guest_isn = first.tcp_sequence
    declared = flow.buffers.rx_bytes
    shift = window_shift(first.tcp_options)
    require(shift == expected_shift(declared), f"the guest's SYN carried window scale {shift}, not the {expected_shift(declared)} its declared {declared}-byte receive buffer implies")
    assert shift is not None
    flow.guest_shift = shift
    require(first.tcp_window == min(declared, SYN_WINDOW_LIMIT), f"the guest's SYN window was {first.tcp_window}, not the {min(declared, SYN_WINDOW_LIMIT)} its declared receive buffer implies")
    synacks = [entry for entry in flow.peer if entry.frame.tcp_flags == lp.TCP_SYN | lp.TCP_ACK]
    require(bool(synacks) and all(entry.frame.tcp_acknowledgment == (flow.guest_isn + 1) & MASK for entry in synacks), "the peer's SYN-ACK is missing or acknowledges another SYN")
    flow.peer_isn = synacks[0].frame.tcp_sequence
    require(all(entry.frame.tcp_sequence == flow.peer_isn and entry.frame.tcp_options == SYNACK_OPTIONS for entry in synacks), f"the peer's SYN-ACK changed or did not declare the {PEER_MSS}-byte MSS and window scale {PEER_WINDOW_SHIFT}")
    require(any(entry.frame.tcp_flags & lp.TCP_ACK and not entry.frame.tcp_flags & (lp.TCP_SYN | lp.TCP_RST) and flow.guest_offset(entry.frame) == 0 and flow.guest_acked(entry.frame) == 0 for entry in flow.guest), "the guest's handshake ACK is missing")
    for entry in flow.guest[1:]:
        frame = entry.frame
        if frame.tcp_flags == lp.TCP_SYN:
            continue
        require(frame.tcp_flags & lp.TCP_SYN == 0, "the guest sent a SYN inside an established connection")
        require(frame.tcp_flags & lp.TCP_RST or frame.tcp_flags & lp.TCP_ACK, "a guest segment lacks an acknowledgment")
        require(flow.guest_window(frame) <= declared, f"the guest advertised {flow.guest_window(frame)} bytes, beyond its declared {declared}-byte receive buffer")
    return synacks[0]


def guest_data(flow: Flow) -> list[tuple[Observation, int]]:
    """The guest's data segments with their stream offsets."""
    return [(entry, flow.guest_offset(entry.frame)) for entry in flow.guest if entry.frame.tcp_payload and not entry.frame.tcp_flags & (lp.TCP_SYN | lp.TCP_RST)]


def guest_stream(flow: Flow, expected: bytes) -> set[int]:
    """Every guest transmission is the declared stream; return the offsets the peer accepted."""
    covered: set[int] = set()
    for entry, start in guest_data(flow):
        payload = entry.frame.tcp_payload
        require(start + len(payload) <= len(expected), "the guest stream exceeds its declared length")
        require(payload == expected[start : start + len(payload)], "a guest transmission differs from the declared stream bytes")
        if entry.delivered:
            covered.update(range(start, start + len(payload)))
    return covered


def peer_stream(flow: Flow, expected: bytes) -> None:
    """The peer sends only its declared stream for this scenario."""
    for entry in flow.peer:
        frame = entry.frame
        if not frame.tcp_payload:
            continue
        start = flow.peer_offset(frame)
        require(start + len(frame.tcp_payload) <= len(expected) and frame.tcp_payload == expected[start : start + len(frame.tcp_payload)], "the peer sent something other than its declared stream")


def in_flight(flow: Flow) -> list[tuple[Observation, int]]:
    """After every guest data segment, the guest bytes on the wire the peer has not acknowledged."""
    high = 0
    acked = 0
    points: list[tuple[Observation, int]] = []
    for from_guest, entry in flow.merged():
        frame = entry.frame
        if frame.tcp_flags & (lp.TCP_SYN | lp.TCP_RST):
            continue
        if from_guest:
            if frame.tcp_payload:
                high = max(high, flow.guest_offset(frame) + len(frame.tcp_payload))
                points.append((entry, high - acked))
        elif frame.tcp_flags & lp.TCP_ACK:
            acked = max(acked, min(flow.peer_acked(frame), high))
    return points


def acknowledgments(flow: Flow) -> None:
    """The guest acknowledges only peer bytes on the wire and never exceeds its transmit buffer."""
    delivered: set[int] = set()
    peer_fin = False
    for from_guest, entry in flow.merged():
        frame = entry.frame
        if not from_guest:
            if frame.tcp_flags & lp.TCP_SYN:
                continue
            if frame.tcp_payload:
                start = flow.peer_offset(frame)
                delivered.update(range(start, start + len(frame.tcp_payload)))
            peer_fin = peer_fin or bool(frame.tcp_flags & lp.TCP_FIN)
            continue
        if frame.tcp_flags & (lp.TCP_SYN | lp.TCP_RST):
            continue
        contiguous = 0
        while contiguous in delivered:
            contiguous += 1
        require(flow.guest_acked(frame) <= contiguous + int(peer_fin), "the guest acknowledged peer bytes it never received")
    declared = flow.buffers.tx_bytes
    peak = max((count for _, count in in_flight(flow)), default=0)
    require(peak <= declared, f"the guest had {peak} unacknowledged bytes in flight, beyond its declared {declared}-byte transmit buffer")


def retries(flow: Flow) -> int:
    """Segments that start below the guest's transmitted high-water mark, as the service's egress guard counts them."""
    high: int | None = None
    count = 0
    for entry in flow.guest:
        frame = entry.frame
        length = len(frame.tcp_payload) + int(bool(frame.tcp_flags & lp.TCP_SYN)) + int(bool(frame.tcp_flags & lp.TCP_FIN))
        if not length:
            continue
        start = 0 if frame.tcp_flags & lp.TCP_SYN else flow.guest_offset(frame) + 1
        if high is not None and start < high:
            count += 1
        high = start + length if high is None else max(high, start + length)
    return count


def closed(flow: Flow, guest_bytes: int, peer_bytes: int) -> Observation:
    """Graceful close after both streams; returns the guest's final acknowledgment."""
    require(any(entry.frame.tcp_flags & lp.TCP_FIN and flow.guest_offset(entry.frame) + len(entry.frame.tcp_payload) == guest_bytes for entry in flow.guest), "the guest's FIN is missing")
    require(any(entry.frame.tcp_flags & lp.TCP_FIN and flow.peer_offset(entry.frame) + len(entry.frame.tcp_payload) == peer_bytes and flow.peer_acked(entry.frame) == guest_bytes + 1 for entry in flow.peer), "the peer's FIN is missing")
    final = [entry for entry in flow.guest if entry.frame.tcp_flags == lp.TCP_ACK and flow.guest_offset(entry.frame) == guest_bytes + 1 and flow.guest_acked(entry.frame) == peer_bytes + 1]
    require(bool(final), "the guest's final acknowledgment is missing")
    require(not any(entry.frame.tcp_flags & lp.TCP_RST for entry in flow.guest + flow.peer), "a completed scenario was reset")
    return final[0]


def completed(flow: Flow, limit: int) -> Observation:
    """A scenario both sides finished: exact streams, bounded retries, graceful close."""
    handshake(flow)
    expected = guest_stream_for(flow.port)
    peer_expected = peer_stream_for(flow.port)
    require(guest_stream(flow, expected) == set(range(len(expected))), "the guest stream did not reach the peer completely")
    peer_stream(flow, peer_expected)
    acknowledgments(flow)
    require(any(flow.guest_acked(entry.frame) >= len(peer_expected) for entry in flow.guest if not entry.frame.tcp_flags & (lp.TCP_SYN | lp.TCP_RST)), "the guest never acknowledged the peer's whole stream")
    count = retries(flow)
    require(count <= limit, f"the guest retransmitted {count} segments, beyond the destination's declared retry limit {limit}")
    return closed(flow, len(expected), len(peer_expected))


def qualify_tx_flight(flow: Flow, limit: int) -> str:
    """The peer's hold leaves exactly the declared transmit buffer unacknowledged."""
    handshake(flow)
    declared = flow.buffers.tx_bytes
    ramp = next((entry for entry, offset in guest_data(flow) if offset + len(entry.frame.tcp_payload) >= RAMP_BYTES), None)
    require(ramp is not None, "the guest never reached the ramp")
    assert ramp is not None
    before = [entry for entry in flow.peer if entry.order < ramp.order and entry.frame.tcp_flags == lp.TCP_ACK]
    after = next((entry for entry in flow.peer if entry.order > ramp.order and entry.frame.tcp_flags & lp.TCP_ACK and not entry.frame.tcp_flags & lp.TCP_SYN), None)
    require(bool(before) and after is not None, "the peer's hold has no acknowledgment on either side")
    assert after is not None
    held = after.time - ramp.time
    require(held >= HOLD_SECONDS - 0.001, f"the peer withheld acknowledgments for {held:.3f} s, not its declared {HOLD_SECONDS:.1f} s")
    acked = max(flow.peer_acked(entry.frame) for entry in before)
    high = max(offset + len(entry.frame.tcp_payload) for entry, offset in guest_data(flow) if entry.order < after.order)
    plateau = high - acked
    require(plateau == declared, f"the guest had {plateau} unacknowledged bytes in flight at the end of the hold, not its declared {declared}-byte transmit buffer")
    completed(flow, limit)
    return f"{flow.holder} tx-flight in_flight={plateau} declared_tx={declared} held_s={held:.3f}"


def qualify_rx_window(flow: Flow, limit: int) -> str:
    """The guest's first zero window acknowledges exactly its declared receive buffer."""
    handshake(flow)
    declared = flow.buffers.rx_bytes
    require(not guest_data(flow), "the guest sent data in the rx-window scenario")
    zero = next((entry for entry in flow.guest if entry.frame.tcp_flags & lp.TCP_ACK and not entry.frame.tcp_flags & (lp.TCP_SYN | lp.TCP_RST) and entry.frame.tcp_window == 0), None)
    require(zero is not None, "the guest never advertised a zero window")
    assert zero is not None
    accepted = flow.guest_acked(zero.frame)
    require(accepted == declared, f"the guest's first zero window acknowledged {accepted} bytes, not its declared {declared}-byte receive buffer")
    completed(flow, limit)
    return f"{flow.holder} rx-window zero_window_at={accepted} declared_rx={declared} shift={flow.guest_shift}"


@dataclasses.dataclass(frozen=True)
class Throughput:
    holder: str
    bytes: int
    elapsed_ns: int

    def line(self) -> str:
        return f"[tcp-buffers] throughput holder={HOLDER_NAMES[self.holder]} bytes={self.bytes} elapsed_ns={self.elapsed_ns}"


def qualify_throughput(flow: Flow, limit: int) -> Throughput:
    completed(flow, limit)
    first = guest_data(flow)[0][0]
    last = next(entry for entry in flow.peer if entry.frame.tcp_flags & lp.TCP_ACK and not entry.frame.tcp_flags & lp.TCP_SYN and flow.peer_acked(entry.frame) >= THROUGHPUT_BYTES)
    return Throughput(flow.holder, THROUGHPUT_BYTES, round((last.time - first.time) * 1e9))


@dataclasses.dataclass(frozen=True)
class OpenReport:
    """One `tcp open` line: the holder, socket index, local port and sizes the service applied."""

    holder: str
    socket: int
    local_port: int
    rx_bytes: int
    tx_bytes: int


OPEN_REPORT = re.compile(r"\[network-service\] tcp open holder=([a-z0-9-]+) socket=([0-9]+) local_port=([0-9]+) rx_bytes=([0-9]+) tx_bytes=([0-9]+)$", re.M)


def open_reports(transcript: str) -> list[OpenReport]:
    return [OpenReport(match[1], int(match[2]), int(match[3]), int(match[4]), int(match[5])) for match in OPEN_REPORT.finditer(transcript)]


def qualify_reports(flows: Sequence[Flow], reports: Sequence[OpenReport]) -> str:
    """Pair every connection report with its wire connection; require holder order and one baseline reuse of a large socket."""
    require(len(reports) == len(flows), f"the service reported {len(reports)} connections, but the wire shows {len(flows)}")
    by_port = {flow.guest_port: flow for flow in flows}
    names = {name: holder for holder, name in HOLDER_NAMES.items()}
    used: dict[str, set[int]] = {holder: set() for holder in HOLDERS}
    reused = 0
    for report in reports:
        flow = by_port.get(report.local_port)
        require(flow is not None, f"an open report names local port {report.local_port}, which no wire connection used")
        assert flow is not None
        holder = names.get(report.holder)
        require(holder == flow.holder, f"the open report for local port {report.local_port} names {report.holder}, but the wire connection belongs to {HOLDER_NAMES[flow.holder]}")
        require((report.rx_bytes, report.tx_bytes) == (flow.buffers.rx_bytes, flow.buffers.tx_bytes), f"the open report for {report.holder} applied rx={report.rx_bytes} tx={report.tx_bytes}, not the declared rx={flow.buffers.rx_bytes} tx={flow.buffers.tx_bytes}")
        require(0 <= report.socket < POOL_SOCKETS, f"the open report names socket {report.socket}, outside the {POOL_SOCKETS}-socket pool")
        if holder == "baseline" and report.socket in used["large"]:
            reused += 1
        used[holder].add(report.socket)
    require(len({report.local_port for report in reports}) == len(reports), "two open reports name the same local port")
    large = [flow for flow in flows if flow.holder == "large"]
    baseline = [flow for flow in flows if flow.holder == "baseline"]
    closed_at = max(flow.last_order for flow in large)
    require(all(flow.guest[0].order > closed_at for flow in baseline), "a buffers-baseline SYN came before the last buffers-large connection closed")
    require(reused >= 1, "no buffers-baseline connection opened on a socket a buffers-large connection used")
    return f"open reports={len(reports)} baseline_reused_large_sockets={reused} large_sockets={sorted(used['large'])} baseline_sockets={sorted(used['baseline'])}"


def qualify_buffers_ledger(ledger: Ledger, guest_mac: bytes, retry_limits: Mapping[int, int], reports: Sequence[OpenReport]) -> tuple[list[str], list[Throughput]]:
    """Raise ValueError unless the wire and the service's reports prove every scenario."""
    require(ledger.malformed_received == 0, "unclassifiable Ethernet frames were received")
    require(ledger.guest_mac == guest_mac, "the guest's ARP request was missing or carried a foreign MAC")
    require(set(retry_limits) == set(SCENARIOS), "a scenario destination has no declared retry limit")
    flows = split_flows(ledger, guest_mac)
    require(sorted(flow.port for flow in flows) == sorted(SCENARIOS), "expected exactly one connection per scenario")
    require(len({flow.guest_port for flow in flows}) == len(flows), "the guest reused a source port across scenario connections")
    by_port = {flow.port: flow for flow in flows}
    for holder, ports in HOLDER_PORTS.items():
        starts = [by_port[port].guest[0].order for port in ports]
        require(starts == sorted(starts), f"the {holder} scenarios did not run in program order")
    summaries: list[str] = []
    rates: list[Throughput] = []
    for holder in HOLDER_ORDER:
        tx, rx, rate = (by_port[port] for port in HOLDER_PORTS[holder])
        summaries.append(qualify_tx_flight(tx, retry_limits[tx.port]))
        summaries.append(qualify_rx_window(rx, retry_limits[rx.port]))
        throughput = qualify_throughput(rate, retry_limits[rate.port])
        rates.append(throughput)
        summaries.append(f"{holder} throughput bytes={throughput.bytes} elapsed_ns={throughput.elapsed_ns}")
    summaries.append(qualify_reports(flows, reports))
    return summaries, rates


# ---------------------------------------------------------------------------
# Composition and builder judges: pure functions over the derived rows.


def judge_application_rows(applications: Mapping[str, Mapping[str, str]]) -> int:
    """Hold the derived composition's application rows to the declared holders; return facts checked."""
    facts = 0
    require(set(applications) == set(HOLDER_NAMES.values()), f"the composition must declare exactly the application rows {sorted(HOLDER_NAMES.values())}, found {sorted(applications)}")
    for holder, name in HOLDER_NAMES.items():
        row = applications[name]
        declared = HOLDERS[holder]
        expected = {
            "role": "client",
            "backend": "external",
            "nagle": "true",
            "rxBufferBytes": str(declared.rx_bytes),
            "txBufferBytes": str(declared.tx_bytes),
        }
        for key, value in expected.items():
            require(row.get(key) == value, f"{name} must declare {key}={value}, found {row.get(key)!r}")
            facts += 1
    return facts


def judge_destination_rows(destinations: Sequence[Mapping[str, str]]) -> dict[int, int]:
    """Hold the destination rows to one exact 10.0.0.2 TCP grant per scenario port; return the retry limits."""
    limits: dict[int, int] = {}
    for holder, name in HOLDER_NAMES.items():
        rows = [row for row in destinations if row.get("holder") == name]
        require(sorted(int(row.get("port", "0")) for row in rows) == sorted(HOLDER_PORTS[holder]), f"{name} must hold exactly one destination per scenario port {HOLDER_PORTS[holder]}")
        for row in rows:
            port = int(row["port"])
            require((row.get("address"), row.get("addressKind"), row.get("transport")) == ("10.0.0.2", "ipv4", "tcp") and {"connect", "send", "recv"} <= set(row.get("rights", "").split(",")), f"scenario destination {port} is not an exact 10.0.0.2 TCP connect/send/recv grant")
            limits[port] = int(row.get("retryLimit", "-1"))
            require(1 <= limits[port] <= 16, f"scenario destination {port} declares retry limit {limits[port]}, outside the contract's 1..16")
    require(not any("listen" in row.get("rights", "").split(",") for row in destinations), "no destination in the buffers composition may carry listen")
    require(set(limits) == set(SCENARIOS), "the buffers composition declares a destination outside the scenario ports")
    return limits


def judge_builder(build: Callable[[dict], bytes], manifest: dict) -> tuple[int, int]:
    """Require the builder to accept every admitted buffer value and refuse every refused one on both rows and fields.

    `build` is the generation builder's application encoder; it refuses by
    raising SystemExit. Returns (accepted, refused).
    """
    accepted = refused = 0
    rows = manifest.get("networkApplications", [])
    holders = {row.get("holder") for row in rows}
    require(set(HOLDER_NAMES.values()) <= holders, "the derived manifest lacks a buffers holder row")
    for name in HOLDER_NAMES.values():
        for field in BUFFER_FIELDS:
            for value, admitted in [*((value, True) for value in ADMITTED_BUFFER_BYTES), *((value, False) for value in REFUSED_BUFFER_BYTES)]:
                candidate = {**manifest, "networkApplications": [dict(row, **({field: value} if row.get("holder") == name else {})) for row in rows]}
                try:
                    build(candidate)
                except SystemExit as error:
                    require(not admitted, f"the builder refused {name} {field}={value}, which network-application/v4 admits: {error}")
                    refused += 1
                    continue
                require(admitted, f"the builder accepted {name} {field}={value}, which network-application/v4 refuses")
                accepted += 1
    return accepted, refused
