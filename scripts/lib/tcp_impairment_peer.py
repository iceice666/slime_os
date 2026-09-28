"""Scripted TCP impairments for the network plane's external frame peer.

`link_peer.Peer` answers like a well-behaved echo host. Qualifying loss,
reordering, retransmission and window bounds needs a host that misbehaves on a
script instead: it reorders and withholds its own segments, drops the guest's,
injects duplicate acknowledgments, falls silent, and closes its window. Each
scenario owns one destination port, so one boot runs all of them, and every
frame carries a timestamp and a global order so the qualifier can bound
elapsed time, probe spacing and acknowledgment progress from the wire alone.

The declared bounds below are what the network service must also declare; the
qualifier holds the wire to them independently of any guest counter. Encoders,
the decoder and addressing are `link_peer`'s. Standard library only.
"""

from __future__ import annotations

import dataclasses
import socket
import threading
import time
from collections import Counter
from collections.abc import Mapping
from itertools import pairwise

import link_peer as lp

REORDER_PORT = 4260
LOSS_PORT = 4261
SILENT_PORT = 4262
RECEIVE_WINDOW_PORT = 4263
SEND_WINDOW_PORT = 4264
SCENARIOS: dict[int, str] = {
    REORDER_PORT: "reorder",
    LOSS_PORT: "loss",
    SILENT_PORT: "silent",
    RECEIVE_WINDOW_PORT: "receive-window",
    SEND_WINDOW_PORT: "send-window",
}

STREAM_BYTES = 4096
STREAM = bytes((index * 37 + 11) & 255 for index in range(STREAM_BYTES))

# Declared bounds: the service's `tcp bounds` marker states the same values.
ASSEMBLER_SEGMENTS = 4
SOCKET_BUFFER_BYTES = 2048
SOCKET_TIMEOUT_SECONDS = 10.0

# Wire bounds that follow from the declared ones and smoltcp 0.13's retransmission
# timeout, which starts at one second, never falls below it, and doubles on every
# expiry.
TIMEOUT_SLACK_SECONDS = 2.0
LOSS_ELAPSED_SECONDS = 10.0
RECEIVE_STALL_MS = 2000
PEER_PROBE_INITIAL_SECONDS = 1.0
PEER_PROBES_MAX = 3
SEND_HOLD_SECONDS = 3.0
GUEST_PROBES_MAX = 3
GUEST_PROBE_SPACING_SECONDS = 0.9

# Reordering: phase A leaves exactly ASSEMBLER_SEGMENTS disjoint out-of-order
# ranges before the first hole is filled; phase B offers one more, which the
# declared capacity must refuse so only retransmission recovers it.
REORDER_SEGMENT = 128
REORDER_PHASES: tuple[tuple[int, ...], ...] = (
    (1, 3, 5, 7, 0, 2, 4, 6),
    (9, 11, 13, 15, 17, 8, 10, 12, 14, 16),
)
REORDER_RETAINED = (1, 3, 5, 7, 9, 11, 13, 15)
REORDER_REFUSED = 17
REORDER_END = (max(REORDER_PHASES[-1]) + 1) * REORDER_SEGMENT

# Loss: (guest stream offset, transmissions dropped). The first gap is met with
# injected duplicate acknowledgments; the peer withholds its own first
# transmission of the echo segment covering LOSS_WITHHELD_OFFSET.
STREAM_SEGMENT = 512
LOSS_GUEST_DROPS: tuple[tuple[int, int], ...] = ((1024, 1), (3072, 1))
LOSS_DUPLICATE_ACKS = 3
LOSS_WITHHELD_OFFSET = 2048

# Silence: the first connection to SILENT_PORT is acknowledged through this
# offset and then never answered again; the second is an ordinary echo.
SILENT_ACCEPT_BYTES = 1024

CONNECTION_LIMIT = 8
RETRY_INTERVAL_SECONDS = 0.25
RETRY_LIMIT = 16
PEER_ISN_BASE = 0x53490000
MASK = lp.TCP_SEQUENCE_MASK


@dataclasses.dataclass(frozen=True)
class Observation:
    """A frame, when the peer met it, and whether it crossed the wire.

    `order` totally orders every received and sent observation. A received
    frame the script drops was still seen on the wire; a sent frame the script
    withholds was produced but never reached the guest.
    """

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
    silent_first: bool = False
    window_open: bool = True
    guest_window: int = 0
    received: bytearray = dataclasses.field(default_factory=bytearray)
    handshake: bool = False
    opened_at: float | None = None
    guest_fin: bool = False
    guest_ack: int = 0
    sent: int = 0
    peer_fin: bool = False
    closed: bool = False
    reset: bool = False
    silent_since: float | None = None
    phase: int = 0
    drops: dict[int, int] = dataclasses.field(default_factory=dict)
    duplicate_acks: bool = False
    withheld: bool = False
    probe_due: float | None = None
    probe_interval: float = PEER_PROBE_INITIAL_SECONDS
    probes: int = 0
    retransmissions: int = 0
    retry_due: float | None = None

    @property
    def receive_sequence(self) -> int:
        return (self.guest_isn + 1 + len(self.received) + int(self.guest_fin)) & MASK

    @property
    def segment(self) -> int:
        return REORDER_SEGMENT if self.scenario == "reorder" else STREAM_SEGMENT

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
        dropped = sum(1 for observation in self.received if not observation.delivered)
        withheld = sum(1 for observation in self.sent if not observation.delivered)
        lines = [f"received {len(self.received)} (dropped {dropped}); sent {len(self.sent)} (withheld {withheld}); malformed={self.malformed_received}"]
        lines.extend(
            f"{entry.scenario} port={entry.guest_port} received={len(entry.received)} sent={entry.sent} acknowledged={entry.guest_ack} handshake={int(entry.handshake)} guest_fin={int(entry.guest_fin)} peer_fin={int(entry.peer_fin)} closed={int(entry.closed)} reset={int(entry.reset)} silent={int(entry.silent_since is not None)} probes={entry.probes} retransmissions={entry.retransmissions}"
            for entry in self.connections
        )
        return "; ".join(lines)


class ImpairmentPeer:
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
        self.ledger.received.append(Observation(order, now, frame, delivered))
        return [reply for reply in replies if reply is not None]

    def record(self, raw: bytes, now: float, delivered: bool = True) -> bytes | None:
        decoded = lp.decode(raw)
        assert decoded is not None
        self.ledger.sent.append(Observation(self.next_order(), now, decoded, delivered))
        return raw if delivered else None

    def send(self, connection: Connection, now: float, sequence: int, flags: int, payload: bytes = b"", *, delivered: bool = True) -> bytes | None:
        window = STREAM_BYTES if connection.window_open else 0
        segment = lp.tcp(self.ip, self.guest_ip, connection.port, connection.guest_port, sequence, connection.receive_sequence, flags, payload, window)
        return self.record(lp.ethernet(connection.guest_mac, self.mac, lp.ETHERTYPE_IPV4, lp.ipv4(self.ip, self.guest_ip, lp.IP_PROTOCOL_TCP, segment)), now, delivered)

    def acknowledge(self, connection: Connection, now: float) -> bytes | None:
        return self.send(connection, now, connection.peer_sequence(connection.sent + int(connection.peer_fin)), lp.TCP_ACK)

    def data(self, connection: Connection, now: float, offset: int, count: int, *, delivered: bool = True) -> bytes | None:
        payload = bytes(connection.received[offset : offset + count])
        return self.send(connection, now, connection.peer_sequence(offset), lp.TCP_ACK | lp.TCP_PSH, payload, delivered=delivered)

    def connection_for(self, frame: lp.Frame) -> Connection | None:
        return next((entry for entry in self.ledger.connections if entry.port == frame.tcp_destination_port and entry.guest_port == frame.tcp_source_port and entry.guest_mac == frame.source), None)

    def handle_tcp(self, frame: lp.Frame, now: float) -> tuple[bool, list[bytes | None]]:
        if frame.destination != self.mac or frame.ip_destination != self.ip or frame.ip_source != self.guest_ip or not frame.tcp_source_port:
            return True, []
        port = frame.tcp_destination_port
        scenario = SCENARIOS.get(port or 0)
        if scenario is None:
            return True, []
        connection = self.connection_for(frame)
        if frame.tcp_flags == lp.TCP_SYN and not frame.tcp_payload:
            if connection is None:
                if len(self.ledger.connections) >= CONNECTION_LIMIT:
                    return True, []
                silent_first = scenario == "silent" and not any(entry.port == SILENT_PORT for entry in self.ledger.connections)
                connection = Connection(
                    scenario,
                    port or 0,
                    frame.source,
                    frame.tcp_source_port,
                    frame.tcp_sequence,
                    PEER_ISN_BASE + len(self.ledger.connections) * 0x10000,
                    silent_first=silent_first,
                    window_open=scenario != "send-window",
                    guest_window=frame.tcp_window,
                )
                self.ledger.connections.append(connection)
            if connection.handshake or frame.tcp_sequence != connection.guest_isn:
                return True, []
            return True, [self.send(connection, now, connection.peer_isn, lp.TCP_SYN | lp.TCP_ACK)]
        if connection is None or connection.reset:
            return True, []
        if connection.silent_since is not None:
            return False, []
        if frame.tcp_flags & lp.TCP_RST:
            if frame.tcp_sequence == connection.receive_sequence:
                connection.reset = True
                self.schedule(connection, now)
            return True, []
        if frame.tcp_flags & ~(lp.TCP_ACK | lp.TCP_PSH | lp.TCP_FIN) or not frame.tcp_flags & lp.TCP_ACK:
            return True, []
        offset = (frame.tcp_sequence - connection.guest_isn - 1) & MASK
        if scenario == "loss" and frame.tcp_payload:
            end = offset + len(frame.tcp_payload)
            for target, count in LOSS_GUEST_DROPS:
                if offset <= target < end and connection.drops.get(target, 0) < count:
                    connection.drops[target] = connection.drops.get(target, 0) + 1
                    return False, []
        acknowledged = (frame.tcp_acknowledgment - connection.peer_isn - 1) & MASK
        if acknowledged > connection.sent + int(connection.peer_fin):
            return True, []
        if not connection.handshake:
            if offset != 0 or acknowledged != 0:
                return True, []
            connection.handshake = True
            connection.opened_at = now
        if acknowledged >= connection.guest_ack:
            connection.guest_ack = acknowledged
            connection.guest_window = frame.tcp_window
        if connection.peer_fin and acknowledged == connection.sent + 1:
            connection.closed = True
        replies: list[bytes | None] = []
        if not connection.window_open:
            # A closed window accepts no byte; a probe gets the unchanged acknowledgment.
            if frame.tcp_payload or frame.tcp_flags & lp.TCP_FIN:
                replies.append(self.acknowledge(connection, now))
            self.schedule(connection, now)
            return True, replies
        if offset > len(connection.received) + int(connection.guest_fin):
            count = 1
            if scenario == "loss" and connection.drops and not connection.duplicate_acks:
                connection.duplicate_acks = True
                count = LOSS_DUPLICATE_ACKS
            replies.extend(self.acknowledge(connection, now) for _ in range(count))
            self.schedule(connection, now)
            return True, replies
        overlap = min(len(frame.tcp_payload), max(0, len(connection.received) - offset))
        if frame.tcp_payload[:overlap] != connection.received[offset : offset + overlap]:
            return True, []
        limit = SILENT_ACCEPT_BYTES if connection.silent_first else STREAM_BYTES
        if not connection.guest_fin:
            connection.received.extend(frame.tcp_payload[overlap:][: max(0, limit - len(connection.received))])
        if frame.tcp_flags & lp.TCP_FIN and not connection.silent_first and offset + len(frame.tcp_payload) == len(connection.received):
            connection.guest_fin = True
        if connection.silent_first and len(connection.received) >= SILENT_ACCEPT_BYTES:
            replies.append(self.acknowledge(connection, now))
            connection.silent_since = now
            self.schedule(connection, now)
            return True, replies
        replies.extend(self.pump(connection, now))
        if not any(reply is not None for reply in replies) and (frame.tcp_payload or frame.tcp_flags & lp.TCP_FIN):
            replies.append(self.acknowledge(connection, now))
        self.schedule(connection, now)
        return True, replies

    def pump(self, connection: Connection, now: float) -> list[bytes | None]:
        """Emit what the script allows of the echo, within the guest's window."""
        if not connection.handshake or connection.silent_first or not connection.window_open or connection.reset:
            return []
        replies: list[bytes | None] = []
        if connection.scenario == "reorder":
            while connection.phase < len(REORDER_PHASES):
                order = REORDER_PHASES[connection.phase]
                start, end = min(order) * REORDER_SEGMENT, (max(order) + 1) * REORDER_SEGMENT
                if connection.sent != start or connection.guest_ack < start or len(connection.received) < end or connection.guest_ack + connection.guest_window < end:
                    break
                replies.extend(self.data(connection, now, index * REORDER_SEGMENT, REORDER_SEGMENT) for index in order)
                connection.sent = end
                connection.phase += 1
            if connection.phase < len(REORDER_PHASES) or connection.guest_ack < REORDER_END:
                return replies
        right = connection.guest_ack + connection.guest_window
        while connection.sent < len(connection.received) and connection.sent < right:
            count = min(connection.segment, len(connection.received) - connection.sent, right - connection.sent)
            withhold = connection.scenario == "loss" and not connection.withheld and connection.sent <= LOSS_WITHHELD_OFFSET < connection.sent + count
            connection.withheld = connection.withheld or withhold
            replies.append(self.data(connection, now, connection.sent, count, delivered=not withhold))
            connection.sent += count
        if connection.guest_fin and not connection.peer_fin and connection.guest_ack == connection.sent == len(connection.received):
            replies.append(self.send(connection, now, connection.peer_sequence(connection.sent), lp.TCP_ACK | lp.TCP_FIN))
            connection.peer_fin = True
        return replies

    def schedule(self, connection: Connection, now: float) -> None:
        if connection.silent_since is not None or connection.reset or connection.closed:
            connection.retry_due = None
            connection.probe_due = None
            return
        pending = connection.handshake and connection.window_open and connection.sent < len(connection.received)
        zero_window = pending and connection.guest_ack == connection.sent and connection.guest_ack + connection.guest_window <= connection.sent
        if connection.scenario == "receive-window" and zero_window:
            if connection.probe_due is None:
                connection.probe_due = now + connection.probe_interval
        else:
            connection.probe_due = None
            connection.probe_interval = PEER_PROBE_INITIAL_SECONDS
        outstanding = not connection.handshake or connection.guest_ack < connection.sent or connection.peer_fin and not connection.closed
        if not outstanding:
            connection.retry_due = None
        elif connection.retry_due is None:
            connection.retry_due = now + RETRY_INTERVAL_SECONDS

    def poll(self, now: float) -> list[bytes]:
        """Advance every timer the script owns: holds, persist probes, retransmissions."""
        replies: list[bytes | None] = []
        for connection in self.ledger.connections:
            if connection.silent_since is not None or connection.reset or connection.closed:
                continue
            if not connection.window_open and connection.opened_at is not None and now >= connection.opened_at + SEND_HOLD_SECONDS:
                connection.window_open = True
                replies.append(self.acknowledge(connection, now))
                replies.extend(self.pump(connection, now))
            if connection.probe_due is not None and now >= connection.probe_due:
                replies.append(self.data(connection, now, connection.sent, 1))
                connection.probes += 1
                connection.probe_interval *= 2
                connection.probe_due = now + connection.probe_interval
            if connection.retry_due is not None and now >= connection.retry_due and connection.retransmissions < RETRY_LIMIT:
                replies.append(self.retransmit(connection, now))
            self.schedule(connection, now)
        return [reply for reply in replies if reply is not None]

    def retransmit(self, connection: Connection, now: float) -> bytes | None:
        """Resend only what the guest has not acknowledged: one segment at its ACK point."""
        connection.retry_due = now + RETRY_INTERVAL_SECONDS
        if not connection.handshake:
            connection.retransmissions += 1
            return self.send(connection, now, connection.peer_isn, lp.TCP_SYN | lp.TCP_ACK)
        if connection.guest_ack < connection.sent:
            connection.retransmissions += 1
            return self.data(connection, now, connection.guest_ack, min(connection.segment, connection.sent - connection.guest_ack))
        if connection.peer_fin and not connection.closed:
            connection.retransmissions += 1
            return self.send(connection, now, connection.peer_sequence(connection.sent), lp.TCP_ACK | lp.TCP_FIN)
        connection.retry_due = None
        return None


def serve(receiver: socket.socket, qemu_port: int, stop: threading.Event, peer: ImpairmentPeer) -> None:
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


def handshake(flow: Flow) -> None:
    syns = [entry for entry in flow.guest if entry.frame.tcp_flags == lp.TCP_SYN]
    require(bool(syns) and syns[0] is flow.guest[0], "the connection does not begin with the guest's SYN")
    require(len({entry.frame.tcp_sequence for entry in syns}) == 1 and not any(entry.frame.tcp_payload for entry in syns), "the guest's SYN changed or carried data")
    flow.guest_isn = syns[0].frame.tcp_sequence
    synacks = [entry for entry in flow.peer if entry.frame.tcp_flags == lp.TCP_SYN | lp.TCP_ACK]
    require(bool(synacks) and all(entry.frame.tcp_acknowledgment == (flow.guest_isn + 1) & MASK for entry in synacks), "the peer's SYN-ACK is missing or acknowledges another SYN")
    flow.peer_isn = synacks[0].frame.tcp_sequence
    require(all(entry.frame.tcp_sequence == flow.peer_isn for entry in synacks), "the peer's SYN-ACK changed")
    require(any(entry.frame.tcp_flags & lp.TCP_ACK and not entry.frame.tcp_flags & (lp.TCP_SYN | lp.TCP_RST) and flow.guest_offset(entry.frame) == 0 and flow.guest_acked(entry.frame) == 0 for entry in flow.guest), "the guest's handshake ACK is missing")
    for entry in flow.guest[1:]:
        frame = entry.frame
        if frame.tcp_flags == lp.TCP_SYN:
            continue
        require(frame.tcp_flags & lp.TCP_SYN == 0, "the guest sent a SYN inside an established connection")
        require(frame.tcp_flags & lp.TCP_RST or frame.tcp_flags & lp.TCP_ACK, "a guest segment lacks an acknowledgment")
        require(frame.tcp_window <= SOCKET_BUFFER_BYTES, f"the guest advertised {frame.tcp_window} bytes, beyond its declared {SOCKET_BUFFER_BYTES}-byte receive buffer")
    require(syns[0].frame.tcp_window <= SOCKET_BUFFER_BYTES, "the guest's SYN advertised more than its declared receive buffer")


def guest_coverage(flow: Flow) -> set[int]:
    """Bytes the peer accepted from the guest; every transmission must be the declared stream."""
    delivered: set[int] = set()
    for entry in flow.guest:
        frame = entry.frame
        if not frame.tcp_payload:
            continue
        start = flow.guest_offset(frame)
        require(start + len(frame.tcp_payload) <= STREAM_BYTES, "the guest stream exceeds its declared length")
        require(frame.tcp_payload == STREAM[start : start + len(frame.tcp_payload)], "a guest transmission differs from the declared stream bytes")
        if entry.delivered:
            delivered.update(range(start, start + len(frame.tcp_payload)))
    return delivered


def peer_coverage(flow: Flow) -> set[int]:
    delivered: set[int] = set()
    for entry in flow.peer:
        frame = entry.frame
        if not frame.tcp_payload:
            continue
        start = flow.peer_offset(frame)
        require(start + len(frame.tcp_payload) <= STREAM_BYTES and frame.tcp_payload == STREAM[start : start + len(frame.tcp_payload)], "a peer transmission differs from the echoed stream")
        if entry.delivered:
            delivered.update(range(start, start + len(frame.tcp_payload)))
    return delivered


def acknowledgments(flow: Flow) -> None:
    """The guest acknowledges only delivered peer bytes and keeps its TX in flight within its buffer."""
    delivered: set[int] = set()
    peer_fin = False
    guest_acked = 0
    for from_guest, entry in flow.merged():
        frame = entry.frame
        if not from_guest:
            if not entry.delivered or frame.tcp_flags & lp.TCP_SYN:
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
        require(acked <= contiguous + int(peer_fin and contiguous == STREAM_BYTES), "the guest acknowledged peer bytes it never received")
        if frame.tcp_payload:
            end = flow.guest_offset(frame) + len(frame.tcp_payload)
            require(end - guest_acked <= SOCKET_BUFFER_BYTES, f"the guest had {end - guest_acked} bytes in flight, beyond its declared {SOCKET_BUFFER_BYTES}-byte transmit buffer")


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


def closed(flow: Flow) -> Observation:
    """Graceful close of a complete echo; returns the guest's final acknowledgment."""
    require(any(entry.delivered and entry.frame.tcp_flags & lp.TCP_FIN and flow.guest_offset(entry.frame) + len(entry.frame.tcp_payload) == STREAM_BYTES for entry in flow.guest), "the guest's FIN is missing")
    require(any(entry.delivered and entry.frame.tcp_flags & lp.TCP_FIN and flow.peer_offset(entry.frame) + len(entry.frame.tcp_payload) == STREAM_BYTES and flow.peer_acked(entry.frame) == STREAM_BYTES + 1 for entry in flow.peer), "the peer's FIN is missing")
    final = [entry for entry in flow.guest if entry.delivered and entry.frame.tcp_flags == lp.TCP_ACK and flow.guest_offset(entry.frame) == STREAM_BYTES + 1 and flow.guest_acked(entry.frame) == STREAM_BYTES + 1]
    require(bool(final), "the guest's final acknowledgment is missing")
    require(not any(entry.frame.tcp_flags & lp.TCP_RST for entry in flow.guest + flow.peer), "a completed exchange was reset")
    return final[0]


def complete_echo(flow: Flow, limit: int) -> tuple[int, Observation]:
    handshake(flow)
    require(guest_coverage(flow) == set(range(STREAM_BYTES)), "the guest stream did not reach the peer completely")
    require(peer_coverage(flow) == set(range(STREAM_BYTES)), "the echoed stream did not reach the guest completely")
    acknowledgments(flow)
    count = retries(flow)
    require(count <= limit, f"the guest retransmitted {count} segments, beyond the destination's declared retry limit {limit}")
    return count, closed(flow)


def qualify_reorder(flow: Flow, limit: int) -> str:
    complete_echo(flow, limit)
    require(all(entry.delivered for entry in flow.guest + flow.peer), "the reordering scenario dropped or withheld a frame")
    first: dict[int, int] = {}
    transmissions: Counter[int] = Counter()
    later: dict[int, list[int]] = {}
    for entry in flow.peer:
        frame = entry.frame
        if not frame.tcp_payload:
            continue
        start = flow.peer_offset(frame)
        if start >= REORDER_END:
            continue
        require(start % REORDER_SEGMENT == 0 and len(frame.tcp_payload) == REORDER_SEGMENT, "a reordered segment is not one declared 128-byte segment")
        index = start // REORDER_SEGMENT
        transmissions[index] += 1
        first.setdefault(index, entry.order)
        later.setdefault(index, []).append(entry.order)
    for phase in REORDER_PHASES:
        require(all(index in first for index in phase), "a reordering phase was not transmitted")
        require(sorted(phase, key=lambda index: first[index]) == list(phase), "a reordering phase left the peer in an undeclared order")
    phase_b = min(first[index] for index in REORDER_PHASES[1])
    phase_a_end = (max(REORDER_PHASES[0]) + 1) * REORDER_SEGMENT
    require(any(entry.order < phase_b and flow.guest_acked(entry.frame) >= phase_a_end for entry in flow.guest), "the second phase began before the first was acknowledged")
    for index in REORDER_RETAINED:
        require(transmissions[index] == 1, f"out-of-order segment {index} was retransmitted: the guest did not retain {ASSEMBLER_SEGMENTS} out-of-order ranges")
    require(transmissions[REORDER_REFUSED] >= 2, f"segment {REORDER_REFUSED} was never retransmitted: the guest retained more than {ASSEMBLER_SEGMENTS} out-of-order ranges")
    recovery = later[REORDER_REFUSED][1]
    before = [flow.guest_acked(entry.frame) for entry in flow.guest if entry.order < recovery and entry.frame.tcp_flags & lp.TCP_ACK and not entry.frame.tcp_flags & lp.TCP_SYN]
    require(max(before) == REORDER_REFUSED * REORDER_SEGMENT, "the guest's acknowledgment did not stop exactly at the refused segment before its retransmission")
    return f"reorder retained={len(REORDER_RETAINED)} refused=1 capacity={ASSEMBLER_SEGMENTS} peer_retransmissions={sum(transmissions.values()) - len(transmissions)}"


def qualify_loss(flow: Flow, limit: int) -> str:
    count, final = complete_echo(flow, limit)
    dropped: dict[int, int] = {target: 0 for target, _ in LOSS_GUEST_DROPS}
    first_drop: Observation | None = None
    for entry in flow.guest:
        frame = entry.frame
        expected = False
        if frame.tcp_payload:
            start = flow.guest_offset(frame)
            for target, times in LOSS_GUEST_DROPS:
                if start <= target < start + len(frame.tcp_payload) and dropped[target] < times:
                    dropped[target] += 1
                    expected = True
                    break
        require(entry.delivered != expected, "the peer's guest drops differ from the declared loss script")
        if expected and first_drop is None:
            first_drop = entry
    require(all(dropped[target] == times for target, times in LOSS_GUEST_DROPS) and first_drop is not None, "a declared guest loss was never exercised")
    assert first_drop is not None
    gap = flow.guest_offset(first_drop.frame)
    run = 0
    previous: lp.Frame | None = None
    injected = False
    for entry in flow.peer:
        frame = entry.frame
        pure = entry.order > first_drop.order and frame.tcp_flags == lp.TCP_ACK and not frame.tcp_payload and flow.peer_acked(frame) == gap
        run = run + 1 if pure and previous == frame else int(pure)
        previous = frame if pure else None
        injected = injected or run >= LOSS_DUPLICATE_ACKS
    require(injected, "the peer did not inject the declared duplicate acknowledgments at the first gap")
    withheld = [entry for entry in flow.peer if not entry.delivered]
    covering = [entry for entry in flow.peer if entry.frame.tcp_payload and flow.peer_offset(entry.frame) <= LOSS_WITHHELD_OFFSET < flow.peer_offset(entry.frame) + len(entry.frame.tcp_payload)]
    require(len(withheld) == 1 and bool(covering) and covering[0] is withheld[0], "the peer did not withhold exactly its first transmission covering the declared offset")
    require(any(entry.delivered for entry in covering[1:]), "the withheld echo segment was never recovered")
    minimum = sum(times for _, times in LOSS_GUEST_DROPS)
    require(count >= minimum, f"the guest retransmitted {count} segments, fewer than the {minimum} dropped transmissions")
    elapsed = final.time - flow.guest[0].time
    require(elapsed <= LOSS_ELAPSED_SECONDS, f"the lossy exchange took {elapsed:.2f} s, beyond the declared {LOSS_ELAPSED_SECONDS:.0f} s")
    return f"loss guest_drops={minimum} duplicate_acks={LOSS_DUPLICATE_ACKS} withheld=1 guest_retries={count} limit={limit} elapsed_s={elapsed:.2f}"


def qualify_silent(first: Flow, fresh: Flow, limit: int) -> str:
    handshake(first)
    coverage = guest_coverage(first)
    require(coverage == set(range(len(coverage))) and len(coverage) >= SILENT_ACCEPT_BYTES, "the silent peer did not accept the declared stream prefix")
    require(not any(entry.frame.tcp_payload for entry in first.peer), "the silent peer echoed bytes")
    require(all(entry.delivered for entry in first.peer), "the silent peer withheld a frame")
    acknowledgments(first)
    last = first.peer[-1]
    require(last.frame.tcp_flags == lp.TCP_ACK and first.peer_acked(last.frame) == SILENT_ACCEPT_BYTES, "the silent peer's last frame is not its acknowledgment of the declared prefix")
    after = [entry for entry in first.guest if entry.order > last.order]
    require(bool(after) and all(not entry.delivered for entry in after), "the peer answered after falling silent")
    require(all(entry.delivered for entry in first.guest if entry.order < last.order), "the peer dropped a guest frame before falling silent")
    resets = [entry for entry in first.guest if entry.frame.tcp_flags & lp.TCP_RST]
    require(len(resets) == 1 and resets[0] is first.guest[-1], "the guest did not end the silent connection with exactly one reset")
    high = max(first.guest_offset(entry.frame) + len(entry.frame.tcp_payload) for entry in first.guest if entry.order < last.order and not entry.frame.tcp_flags & lp.TCP_SYN)
    retried = [entry for entry in after if entry.frame.tcp_payload and first.guest_offset(entry.frame) < high]
    require(bool(retried), "the guest never retransmitted to the silent peer")
    count = retries(first)
    require(count <= limit, f"the guest retransmitted {count} segments to the silent peer, beyond the declared retry limit {limit}")
    waited = resets[0].time - last.time
    bound = SOCKET_TIMEOUT_SECONDS + TIMEOUT_SLACK_SECONDS
    require(waited <= bound, f"the guest reset the silent connection {waited:.2f} s after the peer fell silent, beyond {bound:.0f} s")
    require(fresh.guest[0].order > resets[0].order, "the fresh connection began before the silent one was reset")
    complete_echo(fresh, limit)
    return f"silent guest_retries={count} limit={limit} reset_after_s={waited:.2f} fresh=1"


def qualify_receive_window(flow: Flow, limit: int) -> str:
    complete_echo(flow, limit)
    require(all(entry.delivered for entry in flow.guest + flow.peer), "the receive-window scenario dropped or withheld a frame")
    zero = [entry for entry in flow.guest if not entry.frame.tcp_flags & lp.TCP_SYN and entry.frame.tcp_window == 0 and flow.guest_acked(entry.frame) < STREAM_BYTES]
    require(bool(zero), "the stalled guest never advertised a zero receive window")
    probes: list[tuple[Observation, int]] = []
    latest: lp.Frame | None = None
    for from_guest, entry in flow.merged():
        frame = entry.frame
        if from_guest:
            if not frame.tcp_flags & lp.TCP_SYN:
                latest = frame
            continue
        if not frame.tcp_payload or latest is None:
            continue
        start = flow.peer_offset(frame)
        right = flow.guest_acked(latest) + latest.tcp_window
        if len(frame.tcp_payload) == 1 and start >= right:
            probes.append((entry, start))
        else:
            require(start + len(frame.tcp_payload) <= right, "the peer sent beyond the guest's advertised window")
    require(1 <= len(probes) <= PEER_PROBES_MAX, f"{len(probes)} persist probes were needed, outside 1..{PEER_PROBES_MAX}")
    answered = 0
    for probe, start in probes:
        answer = next((entry for entry in flow.guest if entry.order > probe.order), None)
        require(answer is not None, "a persist probe went unanswered")
        assert answer is not None
        if answer.frame.tcp_window == 0:
            require(flow.guest_acked(answer.frame) <= start, "the guest accepted a probe byte beyond its zero window")
            answered += int(flow.guest_acked(answer.frame) == start)
    require(answered >= 1, "no persist probe was answered with a zero-window acknowledgment")
    require(any(entry.order > zero[0].order and entry.frame.tcp_window > 0 for entry in flow.guest), "the guest never reopened its window")
    return f"receive-window zero_window_acks={len(zero)} peer_probes={len(probes)} answered={answered}"


def qualify_send_window(flow: Flow, limit: int) -> str:
    complete_echo(flow, limit)
    require(all(entry.delivered for entry in flow.guest + flow.peer), "the send-window scenario dropped or withheld a frame")
    synack = next(entry for entry in flow.peer if entry.frame.tcp_flags == lp.TCP_SYN | lp.TCP_ACK)
    require(synack.frame.tcp_window == 0, "the peer did not open with a zero window")
    opening = next((entry for entry in flow.peer if entry.frame.tcp_window > 0), None)
    require(opening is not None, "the peer never opened its window")
    assert opening is not None
    held = [entry for entry in flow.peer if entry.order < opening.order]
    require(all(flow.peer_acked(entry.frame) == 0 for entry in held if not entry.frame.tcp_flags & lp.TCP_SYN), "the peer accepted a byte while its window was closed")
    established = next(entry for entry in flow.guest if entry.frame.tcp_flags & lp.TCP_ACK and not entry.frame.tcp_flags & lp.TCP_SYN)
    hold = opening.time - established.time
    require(hold >= SEND_HOLD_SECONDS - 0.001, "the peer's zero window was shorter than declared")
    probes = [entry for entry in flow.guest if entry.order < opening.order and entry.frame.tcp_payload]
    require(all(len(entry.frame.tcp_payload) == 1 and flow.guest_offset(entry.frame) == 0 for entry in probes), "the guest sent more than a one-byte probe into a zero window")
    require(1 <= len(probes) <= GUEST_PROBES_MAX, f"the guest sent {len(probes)} persist probes during the {SEND_HOLD_SECONDS:.0f} s zero window, outside 1..{GUEST_PROBES_MAX}")
    gaps = [later.time - earlier.time for earlier, later in pairwise(probes)]
    require(all(gap >= GUEST_PROBE_SPACING_SECONDS for gap in gaps), "the guest's persist probes were spaced below its one-second retransmission floor")
    spacing = f"{min(gaps):.2f}" if gaps else "none"
    return f"send-window guest_probes={len(probes)} min_spacing_s={spacing} hold_s={hold:.2f}"


def qualify_impairment_ledger(ledger: Ledger, guest_mac: bytes, retry_limits: Mapping[int, int]) -> list[str]:
    """Raise ValueError unless the wire alone proves every scenario; return one summary per scenario."""
    require(ledger.malformed_received == 0, "unclassifiable Ethernet frames were received")
    require(ledger.guest_mac == guest_mac, "the guest's ARP request was missing or carried a foreign MAC")
    require(set(retry_limits) == set(SCENARIOS), "a scenario destination has no declared retry limit")
    flows = split_flows(ledger, guest_mac)
    require(sorted(flow.port for flow in flows) == sorted([*SCENARIOS, SILENT_PORT]), "expected one connection per scenario plus one fresh silent-port connection")
    require(len({flow.guest_port for flow in flows}) == len(flows), "the guest reused a source port across scenario connections")
    by_port = {port: [flow for flow in flows if flow.port == port] for port in SCENARIOS}
    return [
        qualify_reorder(by_port[REORDER_PORT][0], retry_limits[REORDER_PORT]),
        qualify_loss(by_port[LOSS_PORT][0], retry_limits[LOSS_PORT]),
        qualify_silent(by_port[SILENT_PORT][0], by_port[SILENT_PORT][1], retry_limits[SILENT_PORT]),
        qualify_receive_window(by_port[RECEIVE_WINDOW_PORT][0], retry_limits[RECEIVE_WINDOW_PORT]),
        qualify_send_window(by_port[SEND_WINDOW_PORT][0], retry_limits[SEND_WINDOW_PORT]),
    ]
