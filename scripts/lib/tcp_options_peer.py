"""Scripted peer for the declared TCP socket options.

Two guest holders open connections with different declared options: one keeps
the service's baseline (Nagle on, no keep-alive, hop limit 64, a 4 s idle
timeout) and one is tuned (Nagle off, 500 ms keep-alive, hop limit 7, a 2 s
idle timeout). Each scenario owns one destination port. This peer withholds
its acknowledgment of a burst of small writes so Nagle's coalescing, or its
absence, shows on the wire; drops the first segment of a stream and answers
the rest with duplicate acknowledgments so loss recovery shows; answers
keep-alive probes for a quiet period; and, on two ports, never speaks again
after its SYN-ACK so each declared idle timeout ends the connection. Every
guest packet's IPv4 TTL is recorded, so the declared hop limit is judged on
every scenario. Frames carry a timestamp and a global order.

The declared values below are what the composition and the network service
must also declare; the qualifier holds the wire to them independently of any
guest counter. Encoders, the decoder and addressing are `link_peer`'s.
Standard library only.
"""

from __future__ import annotations

import dataclasses
import socket
import threading
import time
from collections.abc import Mapping
from itertools import pairwise

import link_peer as lp

NAGLE_ON_PORT = 4290
LOSS_PORT = 4291
SILENT_PORT = 4292
NAGLE_OFF_PORT = 4293
KEEPALIVE_PORT = 4294
DEAD_PEER_PORT = 4295
SCENARIOS: dict[int, str] = {
    NAGLE_ON_PORT: "nagle-on",
    LOSS_PORT: "loss",
    SILENT_PORT: "silent-timeout",
    NAGLE_OFF_PORT: "nagle-off",
    KEEPALIVE_PORT: "keepalive",
    DEAD_PEER_PORT: "dead-peer",
}


@dataclasses.dataclass(frozen=True)
class Options:
    """One binding's declared TCP options, as the service must apply them."""

    keepalive_ms: int
    nagle: bool
    hop_limit: int
    idle_timeout_ms: int


# The two declared bindings: the service's baseline and a tuned one that flips
# every toggle. The composition declares these per holder and the service
# states them at activation.
BASELINE = Options(keepalive_ms=0, nagle=True, hop_limit=64, idle_timeout_ms=4000)
TUNED = Options(keepalive_ms=500, nagle=False, hop_limit=7, idle_timeout_ms=2000)
HOLDERS: dict[str, Options] = {"baseline": BASELINE, "tuned": TUNED}
HOLDER_PORTS: dict[str, tuple[int, ...]] = {
    "baseline": (NAGLE_ON_PORT, LOSS_PORT, SILENT_PORT),
    "tuned": (NAGLE_OFF_PORT, KEEPALIVE_PORT, DEAD_PEER_PORT),
}
PORT_HOLDER: dict[int, str] = {port: holder for holder, ports in HOLDER_PORTS.items() for port in ports}
PORT_OPTIONS: dict[int, Options] = {port: HOLDERS[holder] for port, holder in PORT_HOLDER.items()}

# Declared contract bounds for a binding's options. Keep-alive zero means off.
KEEPALIVE_MIN_MS = 100
KEEPALIVE_MAX_MS = 60_000
HOP_LIMIT_MIN = 1
HOP_LIMIT_MAX = 255
IDLE_TIMEOUT_MIN_MS = 1_000
IDLE_TIMEOUT_MAX_MS = 60_000

# Declared socket bounds and the peer's own advertisement: a 512-byte MSS makes
# a full segment exactly one quarter of the guest's transmit buffer.
SOCKET_BUFFER_BYTES = 2048
PEER_MSS = 512
PEER_WINDOW = 4096
MSS_OPTION = b"\x02\x04" + PEER_MSS.to_bytes(2, "big")


def pattern(seed: int, length: int) -> bytes:
    """Byte `i` of every scripted stream is `(31 * i + seed) mod 256`."""
    return bytes((index * 31 + seed) & 255 for index in range(length))


# Streams the guest sends: a burst of small writes for the Nagle scenarios, a
# stream that fills the transmit buffer twice for loss recovery, and a short
# stream the silent peer never acknowledges.
BURST_WRITES = 8
BURST_WRITE_BYTES = 64
BURST = pattern(5, BURST_WRITES * BURST_WRITE_BYTES)
STREAM_BYTES = 4096
STREAM = pattern(17, STREAM_BYTES)
SILENT_BYTES = 256
SILENT = pattern(29, SILENT_BYTES)
GUEST_STREAMS: dict[int, bytes] = {
    NAGLE_ON_PORT: BURST,
    NAGLE_OFF_PORT: BURST,
    LOSS_PORT: STREAM,
    SILENT_PORT: SILENT,
    KEEPALIVE_PORT: b"",
    DEAD_PEER_PORT: b"",
}
# The peer's whole stream on a completed scenario: one completion byte.
DONE = b"D"
KEEPALIVE_PROBE = b"\x00"

# Nagle: the peer withholds its acknowledgment of the first small segment for
# this long, well inside smoltcp's one-second retransmission floor, so every
# later write is queued behind an unacknowledged small segment.
NAGLE_HOLD_SECONDS = 0.6

# Loss: the first transmission of the first segment is dropped; each of the
# three segments that follow it out of order draws one duplicate
# acknowledgment, and the guest must recover well inside the retransmission
# floor, that is by fast retransmit rather than by its timer.
LOSS_DROP_OFFSET = 0
LOSS_DUPLICATE_ACKS = 3
FAST_RETRANSMIT_SECONDS = 0.5

# Keep-alive: the peer answers every probe and stays otherwise quiet for this
# long before it completes the scenario. The tuned interval is 500 ms.
KEEPALIVE_QUIET_SECONDS = 3.0
KEEPALIVE_PROBES_MIN = 4
KEEPALIVE_PROBES_MAX = 8
KEEPALIVE_SPACING_SECONDS = 0.4
DEAD_PROBES_MIN = 2
DEAD_PROBES_MAX = 5

# Idle timeout: the guest must reset a mute connection no earlier than a
# quarter second before its declared idle timeout and no later than two
# seconds after it.
TIMEOUT_EARLY_SECONDS = 0.25
TIMEOUT_SLACK_SECONDS = 2.0
MUTE_PORTS = frozenset({SILENT_PORT, DEAD_PEER_PORT})

CONNECTION_LIMIT = 8
RETRY_INTERVAL_SECONDS = 0.5
RETRY_LIMIT = 8
PEER_ISN_BASE = 0x4F500000
MASK = lp.TCP_SEQUENCE_MASK
IPV4_TTL_OFFSET = 14 + 8


def ipv4(source: bytes, destination: bytes, protocol: int, payload: bytes, ttl: int) -> bytes:
    """`link_peer.ipv4` with an explicit time-to-live, for guests and controls that must vary it."""
    header = lp.ipv4(source, destination, protocol, b"")[:20]
    header = header[:8] + bytes([ttl]) + header[9:10] + b"\x00\x00" + header[12:]
    header = header[:2] + (20 + len(payload)).to_bytes(2, "big") + header[4:]
    header = header[:10] + lp.checksum(header).to_bytes(2, "big") + header[12:]
    return header + payload


def ttl_of(raw: bytes) -> int | None:
    """The IPv4 time-to-live of a raw Ethernet frame, or None for anything else."""
    if len(raw) >= 14 + 20 and raw[12:14] == lp.ETHERTYPE_IPV4.to_bytes(2, "big") and raw[14] >> 4 == 4:
        return raw[IPV4_TTL_OFFSET]
    return None


@dataclasses.dataclass(frozen=True)
class Observation:
    """A frame, when the peer met it, its IPv4 TTL, and whether it crossed the wire."""

    order: int
    time: float
    frame: lp.Frame
    ttl: int | None = None
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
    guest_window: int = 0
    mute: bool = False
    expected: bytes = b""
    received: bytearray = dataclasses.field(default_factory=bytearray)
    held: dict[int, bytes] = dataclasses.field(default_factory=dict)
    handshake: bool = False
    opened_at: float | None = None
    hold_until: float | None = None
    hold_done: bool = False
    ack_withheld: bool = False
    dropped: bool = False
    done_due: float | None = None
    done_sent: bool = False
    guest_ack: int = 0
    guest_fin: bool = False
    peer_fin: bool = False
    closed: bool = False
    reset: bool = False
    keepalives: int = 0
    retransmissions: int = 0
    retry_due: float | None = None

    @property
    def receive_sequence(self) -> int:
        return (self.guest_isn + 1 + len(self.received) + int(self.guest_fin)) & MASK

    @property
    def sent(self) -> int:
        return int(self.done_sent)

    def peer_sequence(self, offset: int) -> int:
        return (self.peer_isn + 1 + offset) & MASK


@dataclasses.dataclass
class Ledger:
    """Every frame the peer saw or produced, and the script state behind them."""

    received: list[Observation] = dataclasses.field(default_factory=list)
    sent: list[Observation] = dataclasses.field(default_factory=list)
    connections: list[Connection] = dataclasses.field(default_factory=list)
    guest_mac: bytes | None = None
    malformed_received: int = 0

    def summary(self) -> str:
        ignored = sum(1 for observation in self.received if not observation.delivered)
        lines = [f"received {len(self.received)} (ignored {ignored}); sent {len(self.sent)}; malformed={self.malformed_received}"]
        lines.extend(
            f"{entry.scenario} port={entry.guest_port} received={len(entry.received)} done={int(entry.done_sent)} acknowledged={entry.guest_ack} handshake={int(entry.handshake)} guest_fin={int(entry.guest_fin)} peer_fin={int(entry.peer_fin)} closed={int(entry.closed)} reset={int(entry.reset)} keepalives={entry.keepalives} retransmissions={entry.retransmissions}"
            for entry in self.connections
        )
        return "; ".join(lines)


class OptionsPeer:
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
        replies: list[bytes | None] = []
        if frame.kind == "arp-request" and frame.arp_target_ip == self.ip and frame.arp_sender_mac and frame.arp_sender_ip == self.guest_ip:
            self.ledger.guest_mac = frame.arp_sender_mac
            reply = lp.ethernet(frame.source, self.mac, lp.ETHERTYPE_ARP, lp.arp(lp.ARP_REPLY, self.mac, self.ip, frame.arp_sender_mac, frame.arp_sender_ip))
            replies.append(self.record(reply, now))
        elif frame.kind == "tcp":
            delivered, tcp_replies = self.handle_tcp(frame, now)
            replies.extend(tcp_replies)
        self.ledger.received.append(Observation(order, now, frame, ttl_of(raw), delivered))
        return [reply for reply in replies if reply is not None]

    def record(self, raw: bytes, now: float) -> bytes:
        decoded = lp.decode(raw)
        assert decoded is not None
        self.ledger.sent.append(Observation(self.next_order(), now, decoded, ttl_of(raw)))
        return raw

    def send(self, connection: Connection, now: float, sequence: int, flags: int, payload: bytes = b"", options: bytes = b"") -> bytes:
        segment = lp.tcp(self.ip, self.guest_ip, connection.port, connection.guest_port, sequence, connection.receive_sequence, flags, payload, PEER_WINDOW, options)
        return self.record(lp.ethernet(connection.guest_mac, self.mac, lp.ETHERTYPE_IPV4, lp.ipv4(self.ip, self.guest_ip, lp.IP_PROTOCOL_TCP, segment)), now)

    def acknowledge(self, connection: Connection, now: float) -> bytes:
        return self.send(connection, now, connection.peer_sequence(connection.sent + int(connection.peer_fin)), lp.TCP_ACK)

    def connection_for(self, frame: lp.Frame) -> Connection | None:
        return next((entry for entry in self.ledger.connections if entry.port == frame.tcp_destination_port and entry.guest_port == frame.tcp_source_port and entry.guest_mac == frame.source), None)

    def handle_tcp(self, frame: lp.Frame, now: float) -> tuple[bool, list[bytes | None]]:
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
                    PEER_ISN_BASE + len(self.ledger.connections) * 0x10000,
                    guest_window=frame.tcp_window,
                    mute=port in MUTE_PORTS,
                    expected=GUEST_STREAMS[port],
                )
                self.ledger.connections.append(connection)
            if connection.handshake or frame.tcp_sequence != connection.guest_isn:
                return True, []
            return True, [self.send(connection, now, connection.peer_isn, lp.TCP_SYN | lp.TCP_ACK, options=MSS_OPTION)]
        if connection is None or connection.reset:
            return True, []
        if connection.mute:
            # Nothing after the SYN-ACK is answered or even acted on.
            return False, []
        if frame.tcp_flags & lp.TCP_RST:
            if frame.tcp_sequence == connection.receive_sequence:
                connection.reset = True
                self.schedule(connection, now)
            return True, []
        if frame.tcp_flags & ~(lp.TCP_ACK | lp.TCP_PSH | lp.TCP_FIN) or not frame.tcp_flags & lp.TCP_ACK:
            return True, []
        offset = (frame.tcp_sequence - connection.guest_isn - 1) & MASK
        acknowledged = (frame.tcp_acknowledgment - connection.peer_isn - 1) & MASK
        if acknowledged > connection.sent + int(connection.peer_fin):
            return True, []
        if not connection.handshake:
            if offset != 0 or acknowledged != 0:
                return True, []
            connection.handshake = True
            connection.opened_at = now
            if scenario == "keepalive":
                connection.done_due = now + KEEPALIVE_QUIET_SECONDS
        if acknowledged >= connection.guest_ack:
            connection.guest_ack = acknowledged
            connection.guest_window = frame.tcp_window
        if connection.peer_fin and acknowledged == connection.sent + 1:
            connection.closed = True
        replies: list[bytes | None] = []
        if frame.tcp_payload == KEEPALIVE_PROBE and offset == (len(connection.received) - 1) & MASK:
            # A keep-alive probe repeats the byte before the next sequence
            # number; it is answered with the unchanged acknowledgment.
            connection.keepalives += 1
            replies.append(self.acknowledge(connection, now))
            self.schedule(connection, now)
            return True, replies
        if scenario == "loss" and frame.tcp_payload and offset == LOSS_DROP_OFFSET and not connection.dropped:
            connection.dropped = True
            return False, []
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
        if scenario in ("nagle-on", "nagle-off") and frame.tcp_payload and not connection.hold_done:
            # The first small segment is not acknowledged until the hold ends.
            if connection.hold_until is None:
                connection.hold_until = now + NAGLE_HOLD_SECONDS
            connection.ack_withheld = True
            self.schedule(connection, now)
            return True, []
        if frame.tcp_payload or frame.tcp_flags & lp.TCP_FIN:
            replies.append(self.acknowledge(connection, now))
        replies.extend(self.pump(connection, now))
        self.schedule(connection, now)
        return True, replies

    def pump(self, connection: Connection, now: float) -> list[bytes | None]:
        """Complete the scenario once its stream or quiet period is done, then answer the guest's FIN."""
        if not connection.handshake or connection.mute or connection.reset:
            return []
        replies: list[bytes | None] = []
        finished = bool(connection.expected) and len(connection.received) == len(connection.expected) and connection.hold_until is None
        quiet = connection.done_due is not None and now >= connection.done_due
        if not connection.done_sent and (finished or quiet):
            replies.append(self.send(connection, now, connection.peer_sequence(0), lp.TCP_ACK | lp.TCP_PSH, DONE))
            connection.done_sent = True
        if connection.guest_fin and not connection.peer_fin and connection.done_sent and connection.guest_ack == connection.sent:
            replies.append(self.send(connection, now, connection.peer_sequence(connection.sent), lp.TCP_ACK | lp.TCP_FIN))
            connection.peer_fin = True
        return replies

    def schedule(self, connection: Connection, now: float) -> None:
        if connection.mute or connection.reset or connection.closed:
            connection.retry_due = None
            return
        outstanding = not connection.handshake or connection.guest_ack < connection.sent or connection.peer_fin and not connection.closed
        if not outstanding:
            connection.retry_due = None
        elif connection.retry_due is None:
            connection.retry_due = now + RETRY_INTERVAL_SECONDS

    def poll(self, now: float) -> list[bytes]:
        """Advance every timer the script owns: the Nagle hold, the quiet period, retransmissions."""
        replies: list[bytes | None] = []
        for connection in self.ledger.connections:
            if connection.mute or connection.reset or connection.closed:
                continue
            if connection.hold_until is not None and now >= connection.hold_until:
                connection.hold_until = None
                connection.hold_done = True
                if connection.ack_withheld:
                    connection.ack_withheld = False
                    replies.append(self.acknowledge(connection, now))
                replies.extend(self.pump(connection, now))
            if connection.done_due is not None and not connection.done_sent and now >= connection.done_due:
                replies.extend(self.pump(connection, now))
            if connection.retry_due is not None and now >= connection.retry_due and connection.retransmissions < RETRY_LIMIT:
                replies.append(self.retransmit(connection, now))
            self.schedule(connection, now)
        return [reply for reply in replies if reply is not None]

    def retransmit(self, connection: Connection, now: float) -> bytes | None:
        """Resend only what the guest has not acknowledged: the SYN-ACK, the completion byte or the FIN."""
        connection.retry_due = now + RETRY_INTERVAL_SECONDS
        if not connection.handshake:
            connection.retransmissions += 1
            return self.send(connection, now, connection.peer_isn, lp.TCP_SYN | lp.TCP_ACK, options=MSS_OPTION)
        if connection.done_sent and connection.guest_ack < connection.sent:
            connection.retransmissions += 1
            return self.send(connection, now, connection.peer_sequence(0), lp.TCP_ACK | lp.TCP_PSH, DONE)
        if connection.peer_fin and not connection.closed:
            connection.retransmissions += 1
            return self.send(connection, now, connection.peer_sequence(connection.sent), lp.TCP_ACK | lp.TCP_FIN)
        connection.retry_due = None
        return None


def serve(receiver: socket.socket, qemu_port: int, stop: threading.Event, peer: OptionsPeer) -> None:
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


@dataclasses.dataclass
class Flow:
    """One connection as the wire shows it, independent of the peer's own state."""

    port: int
    guest_port: int
    guest: list[Observation]
    peer: list[Observation]
    guest_isn: int = 0
    peer_isn: int = 0

    @property
    def scenario(self) -> str:
        return SCENARIOS[self.port]

    @property
    def options(self) -> Options:
        return PORT_OPTIONS[self.port]

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

    def merged(self) -> list[tuple[bool, Observation]]:
        """Both directions in wire order; the flag is True for guest frames."""
        return sorted([(True, entry) for entry in self.guest] + [(False, entry) for entry in self.peer], key=lambda item: item[1].order)


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


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
        require(observation.ttl is not None, "a guest TCP packet has no recorded IPv4 time-to-live")
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
    """Check the three-way handshake and the peer's MSS; return the peer's SYN-ACK."""
    syns = [entry for entry in flow.guest if entry.frame.tcp_flags == lp.TCP_SYN]
    require(bool(syns) and syns[0] is flow.guest[0], "the connection does not begin with the guest's SYN")
    require(len({entry.frame.tcp_sequence for entry in syns}) == 1 and not any(entry.frame.tcp_payload for entry in syns), "the guest's SYN changed or carried data")
    flow.guest_isn = syns[0].frame.tcp_sequence
    synacks = [entry for entry in flow.peer if entry.frame.tcp_flags == lp.TCP_SYN | lp.TCP_ACK]
    require(bool(synacks) and all(entry.frame.tcp_acknowledgment == (flow.guest_isn + 1) & MASK for entry in synacks), "the peer's SYN-ACK is missing or acknowledges another SYN")
    flow.peer_isn = synacks[0].frame.tcp_sequence
    require(all(entry.frame.tcp_sequence == flow.peer_isn and entry.frame.tcp_options == MSS_OPTION for entry in synacks), f"the peer's SYN-ACK changed or did not declare the {PEER_MSS}-byte MSS")
    require(any(entry.frame.tcp_flags & lp.TCP_ACK and not entry.frame.tcp_flags & (lp.TCP_SYN | lp.TCP_RST) and flow.guest_offset(entry.frame) == 0 and flow.guest_acked(entry.frame) == 0 for entry in flow.guest), "the guest's handshake ACK is missing")
    for entry in flow.guest[1:]:
        frame = entry.frame
        if frame.tcp_flags == lp.TCP_SYN:
            continue
        require(frame.tcp_flags & lp.TCP_SYN == 0, "the guest sent a SYN inside an established connection")
        require(frame.tcp_flags & lp.TCP_RST or frame.tcp_flags & lp.TCP_ACK, "a guest segment lacks an acknowledgment")
        require(frame.tcp_window <= SOCKET_BUFFER_BYTES, f"the guest advertised {frame.tcp_window} bytes, beyond its declared {SOCKET_BUFFER_BYTES}-byte receive buffer")
    require(syns[0].frame.tcp_window <= SOCKET_BUFFER_BYTES, "the guest's SYN advertised more than its declared receive buffer")
    return synacks[0]


def hop_limit(flow: Flow) -> None:
    """Every guest packet of this connection carries the binding's declared hop limit."""
    declared = flow.options.hop_limit
    for entry in flow.guest:
        require(entry.ttl == declared, f"a guest packet on port {flow.port} carried TTL {entry.ttl}, not the declared hop limit {declared}")


def is_probe(frame: lp.Frame, offset: int, high: int) -> bool:
    """A keep-alive probe repeats one null byte just below the guest's transmitted edge."""
    return frame.tcp_payload == KEEPALIVE_PROBE and offset == (high - 1) & MASK


def guest_data(flow: Flow) -> list[tuple[Observation, int]]:
    """The guest's data segments with their stream offsets, keep-alive probes excluded."""
    high = 0
    segments: list[tuple[Observation, int]] = []
    for entry in flow.guest:
        frame = entry.frame
        if not frame.tcp_payload or frame.tcp_flags & (lp.TCP_SYN | lp.TCP_RST):
            continue
        offset = flow.guest_offset(frame)
        if is_probe(frame, offset, high):
            continue
        segments.append((entry, offset))
        high = max(high, offset + len(frame.tcp_payload))
    return segments


def probes(flow: Flow) -> list[Observation]:
    """The guest's keep-alive probes, in wire order."""
    high = 0
    found: list[Observation] = []
    for entry in flow.guest:
        frame = entry.frame
        if not frame.tcp_payload or frame.tcp_flags & (lp.TCP_SYN | lp.TCP_RST):
            continue
        offset = flow.guest_offset(frame)
        if is_probe(frame, offset, high):
            found.append(entry)
        else:
            high = max(high, offset + len(frame.tcp_payload))
    return found


def probe_orders(flow: Flow) -> set[int]:
    return {entry.order for entry in probes(flow)}


def guest_stream(flow: Flow, expected: bytes, *, delivered_only: bool = True) -> set[int]:
    """Every guest transmission is the declared stream; return the offsets the peer accepted."""
    covered: set[int] = set()
    for entry, start in guest_data(flow):
        payload = entry.frame.tcp_payload
        require(start + len(payload) <= len(expected), "the guest stream exceeds its declared length")
        require(payload == expected[start : start + len(payload)], "a guest transmission differs from the declared stream bytes")
        if entry.delivered or not delivered_only:
            covered.update(range(start, start + len(payload)))
    return covered


def peer_stream(flow: Flow) -> list[Observation]:
    """The peer sends exactly its one completion byte, possibly repeated."""
    data = [entry for entry in flow.peer if entry.frame.tcp_payload]
    require(all(flow.peer_offset(entry.frame) == 0 and entry.frame.tcp_payload == DONE for entry in data), "the peer sent something other than its completion byte")
    return data


def acknowledgments(flow: Flow) -> None:
    """The guest acknowledges only delivered peer bytes and keeps its transmissions inside its buffer."""
    delivered: set[int] = set()
    peer_fin = False
    guest_acked = 0
    skipped = probe_orders(flow)
    for from_guest, entry in flow.merged():
        frame = entry.frame
        if entry.order in skipped:
            continue
        if not from_guest:
            if frame.tcp_flags & lp.TCP_SYN:
                continue
            if frame.tcp_payload:
                start = flow.peer_offset(frame)
                delivered.update(range(start, start + len(frame.tcp_payload)))
            peer_fin = peer_fin or bool(frame.tcp_flags & lp.TCP_FIN)
            guest_acked = max(guest_acked, flow.peer_acked(frame))
            continue
        if frame.tcp_flags & (lp.TCP_SYN | lp.TCP_RST):
            continue
        acked = flow.guest_acked(frame)
        contiguous = 0
        while contiguous in delivered:
            contiguous += 1
        require(acked <= contiguous + int(peer_fin and contiguous == len(DONE)), "the guest acknowledged peer bytes it never received")
        if frame.tcp_payload:
            end = flow.guest_offset(frame) + len(frame.tcp_payload)
            require(end - guest_acked <= SOCKET_BUFFER_BYTES, f"the guest had {end - guest_acked} bytes in flight, beyond its declared {SOCKET_BUFFER_BYTES}-byte transmit buffer")


def retries(flow: Flow) -> int:
    """Segments that start below the guest's transmitted high-water mark, as the service's egress guard counts them."""
    high: int | None = None
    count = 0
    skipped = probe_orders(flow)
    for entry in flow.guest:
        frame = entry.frame
        if entry.order in skipped:
            continue
        length = len(frame.tcp_payload) + int(bool(frame.tcp_flags & lp.TCP_SYN)) + int(bool(frame.tcp_flags & lp.TCP_FIN))
        if not length:
            continue
        start = 0 if frame.tcp_flags & lp.TCP_SYN else flow.guest_offset(frame) + 1
        if high is not None and start < high:
            count += 1
        high = start + length if high is None else max(high, start + length)
    return count


def bounded_retries(flow: Flow, limit: int) -> int:
    count = retries(flow)
    require(count <= limit, f"the guest retransmitted {count} segments, beyond the destination's declared retry limit {limit}")
    return count


def closed(flow: Flow, guest_bytes: int) -> Observation:
    """Graceful close after the completion byte; returns the guest's final acknowledgment."""
    require(any(entry.frame.tcp_flags & lp.TCP_FIN and flow.guest_offset(entry.frame) + len(entry.frame.tcp_payload) == guest_bytes for entry in flow.guest), "the guest's FIN is missing")
    require(any(entry.frame.tcp_flags & lp.TCP_FIN and flow.peer_offset(entry.frame) + len(entry.frame.tcp_payload) == len(DONE) and flow.peer_acked(entry.frame) == guest_bytes + 1 for entry in flow.peer), "the peer's FIN is missing")
    final = [entry for entry in flow.guest if entry.frame.tcp_flags == lp.TCP_ACK and flow.guest_offset(entry.frame) == guest_bytes + 1 and flow.guest_acked(entry.frame) == len(DONE) + 1]
    require(bool(final), "the guest's final acknowledgment is missing")
    require(not any(entry.frame.tcp_flags & lp.TCP_RST for entry in flow.guest + flow.peer), "a completed scenario was reset")
    return final[0]


def completed(flow: Flow, expected: bytes, limit: int) -> int:
    """A scenario the guest finished: exact stream, one completion byte, graceful close, bounded retries."""
    handshake(flow)
    hop_limit(flow)
    require(guest_stream(flow, expected) == set(range(len(expected))), "the guest stream did not reach the peer completely")
    require(len(peer_stream(flow)) >= 1, "the peer never completed the scenario")
    acknowledgments(flow)
    count = bounded_retries(flow, limit)
    closed(flow, len(expected))
    return count


def sub_mss_flight(flow: Flow) -> int:
    """The most sub-MSS guest segments that were unacknowledged at the same time."""
    inflight: set[int] = set()
    peak = 0
    for from_guest, entry in flow.merged():
        frame = entry.frame
        if from_guest:
            if frame.tcp_payload and not frame.tcp_flags & (lp.TCP_SYN | lp.TCP_RST) and len(frame.tcp_payload) < PEER_MSS:
                inflight.add(flow.guest_offset(frame) + len(frame.tcp_payload))
                peak = max(peak, len(inflight))
        elif frame.tcp_flags & lp.TCP_ACK and not frame.tcp_flags & lp.TCP_SYN:
            acked = flow.peer_acked(frame)
            inflight = {end for end in inflight if end > acked}
    return peak


def qualify_nagle(flow: Flow, limit: int) -> str:
    count = completed(flow, BURST, limit)
    segments = guest_data(flow)
    require(len(segments[0][0].frame.tcp_payload) < PEER_MSS, "the first write did not leave as a small segment")
    first = segments[0][0]
    answer = next((entry for entry in flow.peer if entry.order > first.order and entry.frame.tcp_flags & lp.TCP_ACK and not entry.frame.tcp_flags & lp.TCP_SYN), None)
    require(answer is not None, "the peer never acknowledged the burst")
    assert answer is not None
    hold = answer.time - first.time
    require(hold >= NAGLE_HOLD_SECONDS - 0.001, f"the peer acknowledged the first small segment after {hold:.2f} s, before its declared {NAGLE_HOLD_SECONDS:.1f} s hold")
    peak = sub_mss_flight(flow)
    if flow.options.nagle:
        require(peak <= 1, f"{peak} sub-MSS segments were in flight at once although Nagle was declared on")
        require(all(entry.order > answer.order for entry, _ in segments[1:]), "a second segment left before the first small one was acknowledged although Nagle was declared on")
    else:
        require(peak >= 2, "never two sub-MSS segments in flight at once although Nagle was declared off")
    return f"{flow.scenario} writes={BURST_WRITES} segments={len(segments)} sub_mss_peak={peak} hold_s={hold:.2f} guest_retries={count} limit={limit}"


def qualify_loss(flow: Flow, limit: int) -> str:
    count = completed(flow, STREAM, limit)
    undelivered = [entry for entry in flow.guest if not entry.delivered]
    require(len(undelivered) == 1, "the peer did not drop exactly one guest frame")
    drop = undelivered[0]
    require(drop.frame.tcp_payload and flow.guest_offset(drop.frame) == LOSS_DROP_OFFSET and len(drop.frame.tcp_payload) == PEER_MSS, "the drop was not the first full segment of the stream")
    require(drop is next(entry for entry, _ in guest_data(flow)), "the drop was not the first transmission of the first segment")
    duplicates = [entry for entry in flow.peer if entry.order > drop.order and entry.frame.tcp_flags == lp.TCP_ACK and not entry.frame.tcp_payload and flow.peer_acked(entry.frame) == LOSS_DROP_OFFSET]
    require(len(duplicates) >= LOSS_DUPLICATE_ACKS, f"the peer sent {len(duplicates)} duplicate acknowledgments at the gap, fewer than {LOSS_DUPLICATE_ACKS}")
    third = duplicates[LOSS_DUPLICATE_ACKS - 1]
    recovery = next((entry for entry, offset in guest_data(flow) if entry.order > drop.order and offset == LOSS_DROP_OFFSET), None)
    require(recovery is not None, "the dropped segment was never retransmitted")
    assert recovery is not None
    require(recovery.order > third.order, "the guest retransmitted before the third duplicate acknowledgment")
    elapsed = recovery.time - third.time
    require(elapsed <= FAST_RETRANSMIT_SECONDS, f"the guest recovered {elapsed:.2f} s after the third duplicate acknowledgment, beyond the {FAST_RETRANSMIT_SECONDS:.1f} s fast-retransmit bound")
    before = max(offset + len(entry.frame.tcp_payload) for entry, offset in guest_data(flow) if entry.order < recovery.order)
    require(before == SOCKET_BUFFER_BYTES, f"the guest had sent {before} bytes before recovering, not its whole {SOCKET_BUFFER_BYTES}-byte transmit buffer")
    require(count >= 1, "the guest never retransmitted")
    return f"loss dropped=1 duplicate_acks={len(duplicates)} fast_retransmit_s={elapsed:.2f} guest_retries={count} limit={limit}"


def qualify_mute(flow: Flow, limit: int) -> str:
    """A peer that never speaks after its SYN-ACK: the declared idle timeout ends the connection with one reset."""
    synack = handshake(flow)
    hop_limit(flow)
    require(flow.peer == [synack], "the mute peer sent more than its SYN-ACK")
    require(all(entry.delivered for entry in flow.guest if entry.order < synack.order), "the mute peer ignored the guest's SYN")
    after = [entry for entry in flow.guest if entry.order > synack.order]
    require(bool(after) and all(not entry.delivered for entry in after), "the mute peer acted on a frame after its SYN-ACK")
    resets = [entry for entry in flow.guest if entry.frame.tcp_flags & lp.TCP_RST]
    require(len(resets) == 1 and resets[0] is flow.guest[-1], "the guest did not end the mute connection with exactly one reset")
    idle = flow.options.idle_timeout_ms / 1000
    waited = resets[0].time - synack.time
    require(idle - TIMEOUT_EARLY_SECONDS <= waited <= idle + TIMEOUT_SLACK_SECONDS, f"the guest reset the mute connection {waited:.2f} s after the peer's SYN-ACK, outside its declared {idle:.1f} s idle timeout")
    count = bounded_retries(flow, limit)
    found = probes(flow)
    expected = GUEST_STREAMS[flow.port]
    if expected:
        require(not found, "the guest sent keep-alive probes although no interval was declared")
        require(guest_stream(flow, expected, delivered_only=False) == set(range(len(expected))), "the guest did not send the declared stream to the silent peer")
        require(count >= 1, "the guest never retransmitted to the silent peer")
        return f"{flow.scenario} sent={len(expected)} guest_retries={count} limit={limit} reset_after_s={waited:.2f} idle_timeout_s={idle:.1f}"
    require(not guest_data(flow), "the guest sent data to the dead peer")
    require(DEAD_PROBES_MIN <= len(found) <= DEAD_PROBES_MAX, f"the guest sent {len(found)} keep-alive probes to the dead peer, outside {DEAD_PROBES_MIN}..{DEAD_PROBES_MAX}")
    gaps = [later.time - earlier.time for earlier, later in pairwise(found)]
    require(all(gap >= KEEPALIVE_SPACING_SECONDS for gap in gaps), "keep-alive probes were spaced below the declared interval")
    return f"{flow.scenario} probes={len(found)} reset_after_s={waited:.2f} idle_timeout_s={idle:.1f}"


def qualify_keepalive(flow: Flow, limit: int) -> str:
    count = completed(flow, b"", limit)
    require(not guest_data(flow), "the guest sent data during the keep-alive scenario")
    done = peer_stream(flow)[0]
    established = next(entry for entry in flow.guest if entry.frame.tcp_flags & lp.TCP_ACK and not entry.frame.tcp_flags & lp.TCP_SYN)
    quiet = done.time - established.time
    require(quiet >= KEEPALIVE_QUIET_SECONDS - 0.001, f"the peer completed the scenario after {quiet:.2f} s, before its declared {KEEPALIVE_QUIET_SECONDS:.0f} s quiet period")
    found = [entry for entry in probes(flow) if entry.order < done.order]
    require(KEEPALIVE_PROBES_MIN <= len(found) <= KEEPALIVE_PROBES_MAX, f"the guest sent {len(found)} keep-alive probes during the {KEEPALIVE_QUIET_SECONDS:.0f} s quiet period, outside {KEEPALIVE_PROBES_MIN}..{KEEPALIVE_PROBES_MAX}")
    gaps = [later.time - earlier.time for earlier, later in pairwise(found)]
    require(all(gap >= KEEPALIVE_SPACING_SECONDS for gap in gaps), "keep-alive probes were spaced below the declared interval")
    for probe, following in zip(found, [*found[1:], done], strict=True):
        answer = next((entry for entry in flow.peer if probe.order < entry.order < following.order), None)
        require(answer is not None and answer.frame.tcp_flags == lp.TCP_ACK and not answer.frame.tcp_payload and flow.peer_acked(answer.frame) == 0, "a keep-alive probe was not answered with the unchanged acknowledgment before the next")
    spacing = f"{min(gaps):.2f}" if gaps else "none"
    return f"keepalive probes={len(found)} min_spacing_s={spacing} quiet_s={quiet:.2f} guest_retries={count} limit={limit}"


def qualify_options_ledger(ledger: Ledger, guest_mac: bytes, retry_limits: Mapping[int, int]) -> list[str]:
    """Raise ValueError unless the wire alone proves every scenario; return one summary per scenario."""
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
    return [
        qualify_nagle(by_port[NAGLE_ON_PORT], retry_limits[NAGLE_ON_PORT]),
        qualify_loss(by_port[LOSS_PORT], retry_limits[LOSS_PORT]),
        qualify_mute(by_port[SILENT_PORT], retry_limits[SILENT_PORT]),
        qualify_nagle(by_port[NAGLE_OFF_PORT], retry_limits[NAGLE_OFF_PORT]),
        qualify_keepalive(by_port[KEEPALIVE_PORT], retry_limits[KEEPALIVE_PORT]),
        qualify_mute(by_port[DEAD_PEER_PORT], retry_limits[DEAD_PEER_PORT]),
    ]
