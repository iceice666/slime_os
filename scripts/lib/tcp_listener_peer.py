"""Scripted peer for the external TCP listener and its close semantics.

The guest listens on one exact external endpoint for one admitted remote host.
This peer is that host and two impostors: it opens every listener connection
itself, sends SYNs the listener must ignore or refuse, and drives half-close,
simultaneous close, and close with unread or unsent data on accepted
connections. The guest's probe opens one control connection to the peer and
reports its progress as lines; the peer answers with one-byte cues where the
guest must wait for wire evidence it cannot see. Every frame carries a global
order, so the qualifier judges each scenario, and the ordering between them,
from the ledger alone.

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
from collections.abc import Callable

import link_peer as lp

# The declared listener: exact local endpoint, one admitted remote address,
# backlog one and two accepted connections, each reserving both 2048-byte
# socket buffers against the listener's byte budget.
LISTEN_PORT = 4270
UNDECLARED_PORT = 4271
CONTROL_PORT = 4280
BACKLOG = 1
ACCEPTED_LIMIT = 2
SOCKET_BUFFER_BYTES = 2048
LISTENER_BYTE_BUDGET = ACCEPTED_LIMIT * 2 * SOCKET_BUFFER_BYTES

# Impostors: an on-link host the listener does not admit, and an address the
# guest does not own.
INTRUDER_MAC = bytes.fromhex("525400534c03")
INTRUDER_IP = bytes([10, 0, 0, 3])
FOREIGN_IP = bytes([10, 0, 0, 9])


def pattern(seed: int, length: int) -> bytes:
    """Byte `i` of every scripted stream is `(31 * i + seed) mod 256`."""
    return bytes((index * 31 + seed) & 255 for index in range(length))


EXCHANGE = pattern(3, 2048)
LOCAL_HALF_GUEST = pattern(41, 1024)
LOCAL_HALF_PEER = pattern(59, 2048)
PEER_HALF_PEER = pattern(73, 2048)
PEER_HALF_GUEST = pattern(97, 1024)
UNREAD = pattern(113, 512)
UNSENT = pattern(131, 2048)

# The guest's control stream, line by line, and the peer's cues, in order.
CONTROL_LINES: tuple[bytes, ...] = (
    b"ready\n",
    b"accepted 1\n",
    b"accepted 2\n",
    b"done local-half-close\n",
    b"done peer-half-close\n",
    b"accepted 3\n",
    b"done simultaneous-close\n",
    b"accepted 4\n",
    b"done unread-close\n",
    b"accepted 5\n",
    b"queued unsent-close\n",
    b"done unsent-close\n",
    b"finished\n",
)
CUE_ACCEPT_HELD = b"B"
CUE_HALF_CLOSE = b"H"
CUE_UNREAD = b"U"
CUES = CUE_ACCEPT_HELD + CUE_HALF_CLOSE + CUE_UNREAD

# Script timing: a quiet interval after the refused SYNs, a settle interval
# before every new connection, and the zero window held after the guest
# reports its queued bytes.
NEGATIVE_QUIET_SECONDS = 1.0
SETTLE_SECONDS = 0.25
UNSENT_HOLD_SECONDS = 1.0
SEGMENT = 512
PEER_WINDOW = 4096
RETRY_INTERVAL_SECONDS = 0.5
RETRY_LIMIT = 8
PEER_ISN_BASE = 0x4C490000
MASK = lp.TCP_SEQUENCE_MASK


@dataclasses.dataclass(frozen=True)
class Attempt:
    """One connection the peer opens toward the guest, and what it must meet."""

    name: str
    source_port: int
    source_ip: bytes
    source_mac: bytes
    destination_ip: bytes
    destination_port: int
    outcome: str  # "silent", "reset" or "accept"


ATTEMPTS: tuple[Attempt, ...] = (
    Attempt("unadmitted-source", 50100, INTRUDER_IP, INTRUDER_MAC, lp.GUEST_IP, LISTEN_PORT, "silent"),
    Attempt("undeclared-port", 50101, lp.PEER_IP, lp.PEER_MAC, lp.GUEST_IP, UNDECLARED_PORT, "silent"),
    Attempt("foreign-address", 50102, lp.PEER_IP, lp.PEER_MAC, FOREIGN_IP, LISTEN_PORT, "silent"),
    Attempt("exchange", 50110, lp.PEER_IP, lp.PEER_MAC, lp.GUEST_IP, LISTEN_PORT, "accept"),
    Attempt("backlog-held", 50111, lp.PEER_IP, lp.PEER_MAC, lp.GUEST_IP, LISTEN_PORT, "accept"),
    Attempt("backlog-full", 50112, lp.PEER_IP, lp.PEER_MAC, lp.GUEST_IP, LISTEN_PORT, "reset"),
    Attempt("accepted-limit", 50113, lp.PEER_IP, lp.PEER_MAC, lp.GUEST_IP, LISTEN_PORT, "reset"),
    Attempt("simultaneous-close", 50114, lp.PEER_IP, lp.PEER_MAC, lp.GUEST_IP, LISTEN_PORT, "accept"),
    Attempt("unread-close", 50115, lp.PEER_IP, lp.PEER_MAC, lp.GUEST_IP, LISTEN_PORT, "accept"),
    Attempt("unsent-close", 50116, lp.PEER_IP, lp.PEER_MAC, lp.GUEST_IP, LISTEN_PORT, "accept"),
)
ATTEMPT = {attempt.name: attempt for attempt in ATTEMPTS}

# Per accepted connection: the exact stream each side sends, and whether it
# ends with a FIN. The exchange connection carries the local half-close; the
# backlog-held connection carries the peer half-close.
GUEST_STREAMS: dict[str, bytes] = {
    "exchange": EXCHANGE + LOCAL_HALF_GUEST,
    "backlog-held": PEER_HALF_GUEST,
    "simultaneous-close": b"",
    "unread-close": b"",
    "unsent-close": UNSENT,
}
PEER_STREAMS: dict[str, bytes] = {
    "exchange": EXCHANGE + LOCAL_HALF_PEER,
    "backlog-held": PEER_HALF_PEER,
    "simultaneous-close": b"",
    "unread-close": UNREAD,
    "unsent-close": b"",
}
FIN_CLOSED = frozenset({"exchange", "backlog-held", "simultaneous-close", "unsent-close"})


@dataclasses.dataclass(frozen=True)
class Observation:
    """A frame, when the peer met it, and its place in the total wire order."""

    order: int
    time: float
    frame: lp.Frame


@dataclasses.dataclass
class Connection:
    """One TCP connection with the guest and the script state shaping the peer's side."""

    name: str
    peer_ip: bytes
    peer_mac: bytes
    peer_port: int
    guest_port: int
    peer_isn: int
    guest_isn: int | None = None
    window_open: bool = True
    outgoing: bytearray = dataclasses.field(default_factory=bytearray)
    sent: int = 0
    guest_ack: int = 0
    guest_window: int = 0
    received: bytearray = dataclasses.field(default_factory=bytearray)
    handshake: bool = False
    guest_fin: bool = False
    fin_queued: bool = False
    fin_sent: bool = False
    withhold_fin_ack: bool = False
    reset: bool = False
    retry_due: float | None = None
    retransmissions: int = 0

    @property
    def closed(self) -> bool:
        return self.fin_sent and self.guest_ack == self.sent + 1 and self.guest_fin

    def acknowledgment(self) -> int:
        assert self.guest_isn is not None
        fin = int(self.guest_fin and not self.withhold_fin_ack)
        return (self.guest_isn + 1 + len(self.received) + fin) & MASK

    def sequence(self, offset: int) -> int:
        return (self.peer_isn + 1 + offset) & MASK


@dataclasses.dataclass
class Ledger:
    """Every frame the peer saw or produced, and the script state behind them."""

    received: list[Observation] = dataclasses.field(default_factory=list)
    sent: list[Observation] = dataclasses.field(default_factory=list)
    connections: list[Connection] = dataclasses.field(default_factory=list)
    lines: list[bytes] = dataclasses.field(default_factory=list)
    resets: dict[str, int] = dataclasses.field(default_factory=dict)
    stage: int = 0
    desync: str | None = None
    malformed_received: int = 0

    def summary(self) -> str:
        head = f"received {len(self.received)}; sent {len(self.sent)}; malformed={self.malformed_received}; stage={self.stage}; lines={len(self.lines)}; resets={sorted(self.resets)}"
        if self.desync:
            head += f"; desync: {self.desync}"
        return "; ".join([head, *(
            f"{entry.name} guest_port={entry.guest_port} received={len(entry.received)} sent={entry.sent} acknowledged={entry.guest_ack} handshake={int(entry.handshake)} guest_fin={int(entry.guest_fin)} peer_fin={int(entry.fin_sent)} reset={int(entry.reset)} retransmissions={entry.retransmissions}"
            for entry in self.connections
        )])


class ListenerPeer:
    """The pure half: a frame and the current time in, frames for the wire out."""

    def __init__(self) -> None:
        self.ledger = Ledger()
        self.order = 0
        self.guest_mac: bytes | None = None
        self.control: Connection | None = None
        self.control_buffer = bytearray()
        self.cursor = 0
        self.due: float | None = None

    def next_order(self) -> int:
        self.order += 1
        return self.order

    def record(self, raw: bytes, now: float) -> bytes:
        decoded = lp.decode(raw)
        assert decoded is not None
        self.ledger.sent.append(Observation(self.next_order(), now, decoded))
        return raw

    # Wire encoding.

    def segment(self, connection: Connection, now: float, sequence: int, flags: int, payload: bytes = b"") -> bytes:
        assert self.guest_mac is not None
        acknowledgment = connection.acknowledgment() if connection.guest_isn is not None else 0
        window = PEER_WINDOW if connection.window_open else 0
        segment = lp.tcp(connection.peer_ip, lp.GUEST_IP, connection.peer_port, connection.guest_port, sequence, acknowledgment, flags, payload, window)
        return self.record(lp.ethernet(self.guest_mac, connection.peer_mac, lp.ETHERTYPE_IPV4, lp.ipv4(connection.peer_ip, lp.GUEST_IP, lp.IP_PROTOCOL_TCP, segment)), now)

    def syn(self, attempt: Attempt, now: float) -> bytes:
        assert self.guest_mac is not None
        isn = PEER_ISN_BASE + ATTEMPTS.index(attempt) * 0x10000
        window = 0 if attempt.name == "unsent-close" else PEER_WINDOW
        segment = lp.tcp(attempt.source_ip, attempt.destination_ip, attempt.source_port, attempt.destination_port, isn, 0, lp.TCP_SYN, b"", window)
        return self.record(lp.ethernet(self.guest_mac, attempt.source_mac, lp.ETHERTYPE_IPV4, lp.ipv4(attempt.source_ip, attempt.destination_ip, lp.IP_PROTOCOL_TCP, segment)), now)

    def acknowledge(self, connection: Connection, now: float) -> bytes:
        return self.segment(connection, now, connection.sequence(connection.sent + int(connection.fin_sent)), lp.TCP_ACK)

    def data(self, connection: Connection, now: float, offset: int, count: int) -> bytes:
        return self.segment(connection, now, connection.sequence(offset), lp.TCP_ACK | lp.TCP_PSH, bytes(connection.outgoing[offset : offset + count]))

    def fin(self, connection: Connection, now: float) -> bytes:
        return self.segment(connection, now, connection.sequence(len(connection.outgoing)), lp.TCP_ACK | lp.TCP_FIN)

    # Connections.

    def open(self, name: str, now: float) -> list[bytes]:
        attempt = ATTEMPT[name]
        if attempt.outcome == "accept":
            connection = Connection(
                name,
                attempt.source_ip,
                attempt.source_mac,
                attempt.source_port,
                attempt.destination_port,
                PEER_ISN_BASE + ATTEMPTS.index(attempt) * 0x10000,
                window_open=name != "unsent-close",
                retry_due=now + RETRY_INTERVAL_SECONDS,
            )
            self.ledger.connections.append(connection)
        return [self.syn(attempt, now)]

    def connection(self, name: str) -> Connection | None:
        return next((entry for entry in self.ledger.connections if entry.name == name), None)

    def queue(self, connection: Connection, payload: bytes, now: float, *, fin: bool = False) -> list[bytes]:
        connection.outgoing.extend(payload)
        connection.fin_queued = connection.fin_queued or fin
        replies = self.pump(connection, now)
        self.schedule(connection, now)
        return replies

    def cue(self, cue: bytes, now: float) -> list[bytes]:
        assert self.control is not None
        return self.queue(self.control, cue, now)

    def pump(self, connection: Connection, now: float) -> list[bytes]:
        """Send what the guest's window admits, then a queued FIN after the last byte."""
        if not connection.handshake or connection.reset:
            return []
        replies: list[bytes] = []
        right = connection.guest_ack + connection.guest_window
        while connection.sent < len(connection.outgoing) and connection.sent < right:
            count = min(SEGMENT, len(connection.outgoing) - connection.sent, right - connection.sent)
            replies.append(self.data(connection, now, connection.sent, count))
            connection.sent += count
        if connection.fin_queued and not connection.fin_sent and connection.sent == len(connection.outgoing):
            replies.append(self.fin(connection, now))
            connection.fin_sent = True
        return replies

    def schedule(self, connection: Connection, now: float) -> None:
        outstanding = not connection.handshake or connection.guest_ack < connection.sent + int(connection.fin_sent)
        blocked = connection.handshake and connection.sent < len(connection.outgoing)
        if connection.reset or connection.closed or not (outstanding or blocked):
            connection.retry_due = None
        elif connection.retry_due is None:
            connection.retry_due = now + RETRY_INTERVAL_SECONDS

    def retransmit(self, connection: Connection, now: float) -> list[bytes]:
        """Resend the SYN, the first unacknowledged segment or FIN, or probe a closed guest window."""
        connection.retry_due = now + RETRY_INTERVAL_SECONDS
        if connection.retransmissions >= RETRY_LIMIT:
            return []
        connection.retransmissions += 1
        if not connection.handshake:
            if connection.guest_isn is None:
                return [self.syn(ATTEMPT[connection.name], now)]
            return [self.segment(connection, now, connection.peer_isn, lp.TCP_SYN | lp.TCP_ACK)]
        if connection.guest_ack < connection.sent:
            count = min(SEGMENT, connection.sent - connection.guest_ack)
            return [self.data(connection, now, connection.guest_ack, count)]
        if connection.fin_sent and connection.guest_ack == connection.sent:
            return [self.fin(connection, now)]
        if connection.sent < len(connection.outgoing):
            # A closed guest window with nothing outstanding: a one-byte persist probe.
            replies = [self.data(connection, now, connection.sent, 1)]
            connection.sent += 1
            return replies
        connection.retransmissions -= 1
        connection.retry_due = None
        return []

    # Frames from the guest.

    def handle(self, raw: bytes, now: float) -> list[bytes]:
        frame = lp.decode(raw)
        if frame is None:
            self.ledger.malformed_received += 1
            return []
        self.ledger.received.append(Observation(self.next_order(), now, frame))
        replies: list[bytes] = []
        if frame.kind == "arp-request" and frame.arp_target_ip == lp.PEER_IP and frame.arp_sender_ip == lp.GUEST_IP and frame.arp_sender_mac:
            self.guest_mac = frame.arp_sender_mac
            replies.append(self.record(lp.ethernet(frame.source, lp.PEER_MAC, lp.ETHERTYPE_ARP, lp.arp(lp.ARP_REPLY, lp.PEER_MAC, lp.PEER_IP, frame.arp_sender_mac, lp.GUEST_IP)), now))
        elif frame.kind == "tcp" and frame.ip_source == lp.GUEST_IP and frame.destination == lp.PEER_MAC and frame.tcp_source_port:
            replies.extend(self.handle_tcp(frame, now))
        replies.extend(self.advance(now))
        return replies

    def handle_tcp(self, frame: lp.Frame, now: float) -> list[bytes]:
        attempt = next((entry for entry in ATTEMPTS if entry.source_ip == frame.ip_destination and entry.source_port == frame.tcp_destination_port), None)
        if attempt is not None and attempt.outcome == "reset":
            if frame.tcp_flags & lp.TCP_RST:
                self.ledger.resets[attempt.name] = self.ledger.resets.get(attempt.name, 0) + 1
            return []
        if frame.ip_destination != lp.PEER_IP:
            return []
        connection = next((entry for entry in self.ledger.connections if entry.peer_port == frame.tcp_destination_port and entry.guest_port == frame.tcp_source_port), None)
        if connection is None and frame.tcp_destination_port == CONTROL_PORT and frame.tcp_flags == lp.TCP_SYN and self.control is None:
            self.guest_mac = self.guest_mac or frame.source
            connection = Connection("control", lp.PEER_IP, lp.PEER_MAC, CONTROL_PORT, frame.tcp_source_port or 0, PEER_ISN_BASE - 0x10000, guest_isn=frame.tcp_sequence, guest_window=frame.tcp_window)
            self.ledger.connections.append(connection)
            self.control = connection
        if connection is None or connection.reset:
            return []
        if frame.tcp_flags & lp.TCP_RST:
            connection.reset = True
            self.schedule(connection, now)
            return []
        if frame.tcp_flags & lp.TCP_SYN:
            return self.handle_syn(connection, frame, now)
        if not connection.handshake and connection.guest_isn is not None and connection.name == "control":
            if frame.tcp_flags & lp.TCP_ACK and frame.tcp_acknowledgment == connection.sequence(0):
                connection.handshake = True
                connection.guest_window = frame.tcp_window
            else:
                return []
        if not connection.handshake or not frame.tcp_flags & lp.TCP_ACK:
            return []
        return self.handle_segment(connection, frame, now)

    def handle_syn(self, connection: Connection, frame: lp.Frame, now: float) -> list[bytes]:
        if connection.name == "control":
            if frame.tcp_flags != lp.TCP_SYN or frame.tcp_sequence != connection.guest_isn or connection.handshake:
                return []
            self.schedule(connection, now)
            return [self.segment(connection, now, connection.peer_isn, lp.TCP_SYN | lp.TCP_ACK)]
        if frame.tcp_flags != lp.TCP_SYN | lp.TCP_ACK or frame.tcp_acknowledgment != connection.sequence(0):
            return []
        if connection.guest_isn is None:
            connection.guest_isn = frame.tcp_sequence
            connection.handshake = True
            connection.guest_window = frame.tcp_window
            connection.retry_due = None
        elif frame.tcp_sequence != connection.guest_isn:
            return []
        replies = [self.acknowledge(connection, now)]
        replies.extend(self.pump(connection, now))
        self.schedule(connection, now)
        return replies

    def handle_segment(self, connection: Connection, frame: lp.Frame, now: float) -> list[bytes]:
        assert connection.guest_isn is not None
        acknowledged = (frame.tcp_acknowledgment - connection.peer_isn - 1) & MASK
        if acknowledged > connection.sent + int(connection.fin_sent):
            return []
        if acknowledged >= connection.guest_ack:
            connection.guest_ack = acknowledged
            connection.guest_window = frame.tcp_window
        replies: list[bytes] = []
        if connection.withhold_fin_ack and acknowledged == connection.sent + 1:
            # The guest acknowledged the crossing FIN; now acknowledge its own.
            connection.withhold_fin_ack = False
            replies.append(self.acknowledge(connection, now))
        offset = (frame.tcp_sequence - connection.guest_isn - 1) & MASK
        payload = frame.tcp_payload
        if payload and not connection.window_open:
            # A persist probe against the closed window: not accepted, answered unchanged.
            replies.append(self.acknowledge(connection, now))
            self.schedule(connection, now)
            return replies
        if offset > len(connection.received) + int(connection.guest_fin):
            replies.append(self.acknowledge(connection, now))
            self.schedule(connection, now)
            return replies
        overlap = min(len(payload), max(0, len(connection.received) - offset))
        if payload[:overlap] != connection.received[offset : offset + overlap]:
            return replies
        if not connection.guest_fin:
            connection.received.extend(payload[overlap:])
        fin = bool(frame.tcp_flags & lp.TCP_FIN) and offset + len(payload) == len(connection.received)
        if fin and not connection.guest_fin:
            connection.guest_fin = True
            if connection.name == "simultaneous-close" and not connection.fin_sent:
                # Cross the guest's FIN with ours, acknowledging only its data.
                connection.withhold_fin_ack = True
                connection.fin_queued = True
            elif connection.name in ("control", "unsent-close"):
                connection.fin_queued = True
        if connection is self.control:
            self.control_buffer.extend(payload[overlap:])
            while b"\n" in self.control_buffer:
                line, _, rest = bytes(self.control_buffer).partition(b"\n")
                self.ledger.lines.append(line + b"\n")
                self.control_buffer = bytearray(rest)
        sent = self.pump(connection, now)
        replies.extend(sent)
        if not sent and (payload or fin or frame.tcp_flags & lp.TCP_FIN):
            replies.append(self.acknowledge(connection, now))
        self.schedule(connection, now)
        return replies

    # The script.

    def line(self, text: bytes) -> bool:
        """Consume the next control line if it is `text`; any other line desynchronizes."""
        if self.cursor >= len(self.ledger.lines):
            return False
        if self.ledger.lines[self.cursor] != text:
            self.ledger.desync = f"expected control line {text!r}, got {self.ledger.lines[self.cursor]!r}"
            return False
        self.cursor += 1
        return True

    def settled(self, now: float) -> bool:
        """True once the pending wait has elapsed; a wait not yet armed starts now."""
        if self.due is None:
            self.due = now + SETTLE_SECONDS
        if now < self.due:
            return False
        self.due = None
        return True

    def stages(self, now: float) -> tuple[tuple[Callable[[], bool], Callable[[], list[bytes]]], ...]:
        """The script: each stage's condition, then its action, in order."""
        exchange = lambda: self.connection("exchange")  # noqa: E731
        held = lambda: self.connection("backlog-held")  # noqa: E731
        unread = lambda: self.connection("unread-close")  # noqa: E731
        unsent = lambda: self.connection("unsent-close")  # noqa: E731
        established = lambda connection: connection is not None and connection.handshake  # noqa: E731
        settled = lambda: self.settled(now)  # noqa: E731
        nothing = lambda: []  # noqa: E731

        def negatives() -> list[bytes]:
            self.due = now + NEGATIVE_QUIET_SECONDS
            return [frame for attempt in ATTEMPTS if attempt.outcome == "silent" for frame in self.open(attempt.name, now)]

        def hold() -> list[bytes]:
            self.due = now + UNSENT_HOLD_SECONDS
            return []

        def open_window() -> list[bytes]:
            connection = unsent()
            assert connection is not None
            connection.window_open = True
            replies = [self.acknowledge(connection, now)]
            self.schedule(connection, now)
            return replies

        def echoed() -> bool:
            connection = exchange()
            return connection is not None and len(connection.received) >= len(EXCHANGE) and connection.guest_ack >= len(EXCHANGE)

        def half_closed() -> bool:
            connection = exchange()
            return connection is not None and connection.guest_fin and len(connection.received) == len(GUEST_STREAMS["exchange"])

        return (
            (lambda: self.line(CONTROL_LINES[0]), negatives),
            (settled, lambda: self.open("exchange", now)),
            (lambda: established(exchange()) and self.line(CONTROL_LINES[1]), lambda: self.queue(exchange(), EXCHANGE, now)),
            (echoed, nothing),
            (settled, lambda: self.open("backlog-held", now)),
            (lambda: established(held()), lambda: self.open("backlog-full", now)),
            (lambda: bool(self.ledger.resets.get("backlog-full")), lambda: self.cue(CUE_ACCEPT_HELD, now)),
            (lambda: self.line(CONTROL_LINES[2]), nothing),
            (settled, lambda: self.open("accepted-limit", now)),
            (lambda: bool(self.ledger.resets.get("accepted-limit")), lambda: self.cue(CUE_HALF_CLOSE, now)),
            (half_closed, lambda: self.queue(exchange(), LOCAL_HALF_PEER, now, fin=True)),
            (lambda: self.line(CONTROL_LINES[3]), lambda: self.queue(held(), PEER_HALF_PEER, now, fin=True)),
            (lambda: self.line(CONTROL_LINES[4]), nothing),
            (settled, lambda: self.open("simultaneous-close", now)),
            (lambda: established(self.connection("simultaneous-close")) and self.line(CONTROL_LINES[5]), nothing),
            (lambda: self.line(CONTROL_LINES[6]), nothing),
            (settled, lambda: self.open("unread-close", now)),
            (lambda: established(unread()) and self.line(CONTROL_LINES[7]), lambda: self.queue(unread(), UNREAD, now)),
            (lambda: unread().guest_ack >= len(UNREAD), lambda: self.cue(CUE_UNREAD, now)),
            (lambda: self.line(CONTROL_LINES[8]), nothing),
            (settled, lambda: self.open("unsent-close", now)),
            (lambda: established(unsent()) and self.line(CONTROL_LINES[9]), nothing),
            (lambda: self.line(CONTROL_LINES[10]), hold),
            (settled, open_window),
            (lambda: self.line(CONTROL_LINES[11]), nothing),
            (lambda: self.line(CONTROL_LINES[12]), nothing),
        )

    def advance(self, now: float) -> list[bytes]:
        replies: list[bytes] = []
        if self.guest_mac is None:
            return replies
        stages = self.stages(now)
        while self.ledger.desync is None and self.ledger.stage < len(stages):
            condition, action = stages[self.ledger.stage]
            if not condition():
                break
            replies.extend(action())
            self.ledger.stage += 1
        return replies

    def poll(self, now: float) -> list[bytes]:
        """Advance the script's own clock and every retransmission timer."""
        replies: list[bytes] = []
        for connection in self.ledger.connections:
            if connection.retry_due is not None and now >= connection.retry_due:
                replies.extend(self.retransmit(connection, now))
        replies.extend(self.advance(now))
        return replies

    @property
    def complete(self) -> bool:
        return self.ledger.stage == len(self.stages(0.0)) and self.control is not None and self.control.closed


def serve(receiver: socket.socket, qemu_port: int, stop: threading.Event, peer: ListenerPeer) -> None:
    """Run `peer` against QEMU's UDP backend until `stop` is set.

    The guest resolves the peer by ARP before it opens the control connection;
    the peer initiates nothing until it knows the guest's MAC.
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


# Qualification: the ledger alone, independent of the peer's own state.


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


@dataclasses.dataclass
class Flow:
    """One TCP tuple as the wire shows it: the peer-side address and port, the guest port."""

    peer_ip: bytes
    peer_port: int
    guest_port: int
    guest: list[Observation] = dataclasses.field(default_factory=list)
    peer: list[Observation] = dataclasses.field(default_factory=list)
    guest_isn: int = 0
    peer_isn: int = 0

    def guest_offset(self, frame: lp.Frame) -> int:
        return (frame.tcp_sequence - self.guest_isn - 1) & MASK

    def peer_offset(self, frame: lp.Frame) -> int:
        return (frame.tcp_sequence - self.peer_isn - 1) & MASK

    def guest_acked(self, frame: lp.Frame) -> int:
        """How much of the peer's stream, FIN included, a guest frame acknowledges."""
        return (frame.tcp_acknowledgment - self.peer_isn - 1) & MASK

    def peer_acked(self, frame: lp.Frame) -> int:
        """How much of the guest's stream, FIN included, a peer frame acknowledges."""
        return (frame.tcp_acknowledgment - self.guest_isn - 1) & MASK

    def merged(self) -> list[tuple[bool, Observation]]:
        """Both directions in wire order; the flag is True for guest frames."""
        return sorted([(True, entry) for entry in self.guest] + [(False, entry) for entry in self.peer], key=lambda item: item[1].order)


def reparsed(frame: lp.Frame) -> lp.Frame | None:
    assert frame.ip_source is not None and frame.ip_destination is not None
    return lp.decode(lp.ethernet(frame.destination, frame.source, lp.ETHERTYPE_IPV4, lp.ipv4(frame.ip_source, frame.ip_destination, lp.IP_PROTOCOL_TCP, frame.ip_payload)))


def split_flows(ledger: Ledger, guest_mac: bytes) -> dict[tuple[bytes, int, int], Flow]:
    """Every guest frame is ARP for the admitted peer or TCP to it; group both directions by tuple."""
    flows: dict[tuple[bytes, int, int], Flow] = {}
    for observation in ledger.received:
        frame = observation.frame
        require(frame.source == guest_mac, "a frame came from an undeclared source MAC")
        require(frame.destination in (lp.PEER_MAC, lp.BROADCAST), "the guest addressed a host other than the admitted peer")
        if frame.ethertype == lp.ETHERTYPE_ARP:
            require(frame.arp_operation == lp.ARP_REQUEST and frame.arp_sender_mac == guest_mac and frame.arp_sender_ip == lp.GUEST_IP, "malformed ARP from the guest")
            require(frame.arp_target_ip == lp.PEER_IP, "the guest resolved an address other than the admitted peer")
            continue
        require(frame.kind == "tcp" and frame.ip_source == lp.GUEST_IP and frame.ip_destination == lp.PEER_IP, "the guest emitted undeclared IPv4 traffic")
        require(frame.tcp_source_port is not None and reparsed(frame) == frame, "decoded guest evidence disagrees with its checksum-validated packet")
        require(frame.tcp_window <= SOCKET_BUFFER_BYTES, f"the guest advertised a {frame.tcp_window}-byte window beyond its declared {SOCKET_BUFFER_BYTES}-byte buffer")
        assert frame.tcp_source_port is not None and frame.tcp_destination_port is not None
        key = (lp.PEER_IP, frame.tcp_destination_port, frame.tcp_source_port)
        flows.setdefault(key, Flow(*key)).guest.append(observation)
    for observation in ledger.sent:
        frame = observation.frame
        if frame.ethertype == lp.ETHERTYPE_ARP:
            continue
        require(frame.kind == "tcp" and frame.destination == guest_mac and reparsed(frame) == frame, "the peer produced an undeclared or corrupt frame")
        assert frame.ip_source is not None and frame.tcp_source_port is not None and frame.tcp_destination_port is not None
        key = (frame.ip_source, frame.tcp_source_port, frame.tcp_destination_port)
        flows.setdefault(key, Flow(*key)).peer.append(observation)
    return flows


def stream(entries: list[Observation], offset_of, expected: bytes, side: str) -> tuple[dict[int, int], int | None]:
    """Check every payload byte against `expected`; return first order per offset and the FIN offset."""
    first: dict[int, int] = {}
    fins: set[int] = set()
    for entry in entries:
        frame = entry.frame
        offset = offset_of(frame)
        payload = frame.tcp_payload
        if payload:
            require(offset + len(payload) <= len(expected) and payload == expected[offset : offset + len(payload)], f"{side} bytes at offset {offset} differ from the declared stream")
        else:
            # A segment without bytes sits at most one past the stream: after its FIN.
            require(offset <= len(expected) + 1, f"{side} sequence {offset} lies beyond the declared stream")
        for index in range(offset, offset + len(payload)):
            first.setdefault(index, entry.order)
        if frame.tcp_flags & lp.TCP_FIN:
            fins.add(offset + len(payload))
    require(len(fins) <= 1, f"{side} FIN moved between transmissions")
    require(set(first) == set(range(len(expected))), f"{side} stream is incomplete: {len(first)} of {len(expected)} bytes")
    return first, (fins.pop() if fins else None)


def first_order(entries: list[Observation], predicate) -> int | None:
    return next((entry.order for entry in entries if predicate(entry.frame)), None)


def qualify_control(flow: Flow) -> None:
    syns = [entry for entry in flow.guest if entry.frame.tcp_flags == lp.TCP_SYN]
    require(bool(syns) and syns[0] is flow.guest[0] and len({entry.frame.tcp_sequence for entry in syns}) == 1, "the control connection does not begin with one guest SYN")
    flow.guest_isn = syns[0].frame.tcp_sequence
    synacks = [entry for entry in flow.peer if entry.frame.tcp_flags == lp.TCP_SYN | lp.TCP_ACK]
    require(bool(synacks) and all(flow.peer_acked(entry.frame) == 0 for entry in synacks), "the control SYN-ACK is missing or acknowledges another SYN")
    flow.peer_isn = synacks[0].frame.tcp_sequence
    guest = [entry for entry in flow.guest if not entry.frame.tcp_flags & lp.TCP_SYN]
    peer = [entry for entry in flow.peer if not entry.frame.tcp_flags & lp.TCP_SYN]
    require(not any(entry.frame.tcp_flags & lp.TCP_RST for entry in flow.guest + flow.peer), "the control connection was reset")
    lines = b"".join(CONTROL_LINES)
    _, guest_fin = stream(guest, flow.guest_offset, lines, "control")
    _, peer_fin = stream(peer, flow.peer_offset, CUES, "cue")
    require(guest_fin == len(lines) and peer_fin == len(CUES), "the control connection did not close with both FINs after its exact streams")
    require(any(flow.guest_acked(entry.frame) == len(CUES) + 1 for entry in guest), "the guest never acknowledged the peer's control FIN")
    require(any(flow.peer_acked(entry.frame) == len(lines) + 1 for entry in peer), "the peer never acknowledged the guest's control FIN")


def control_orders(flow: Flow) -> tuple[list[int], list[int]]:
    """The wire order at which each control line was complete, and each cue first sent."""
    guest_first, _ = stream([entry for entry in flow.guest if not entry.frame.tcp_flags & lp.TCP_SYN], flow.guest_offset, b"".join(CONTROL_LINES), "control")
    peer_first, _ = stream([entry for entry in flow.peer if not entry.frame.tcp_flags & lp.TCP_SYN], flow.peer_offset, CUES, "cue")
    ends, total = [], 0
    for line in CONTROL_LINES:
        total += len(line)
        ends.append(max(guest_first[index] for index in range(total - len(line), total)))
    return ends, [peer_first[index] for index in range(len(CUES))]


def qualify_accepted(flow: Flow, name: str) -> dict[str, int]:
    """Exact handshake, streams, FINs, bounds and reset policy of one accepted connection."""
    syns = [entry for entry in flow.peer if entry.frame.tcp_flags == lp.TCP_SYN]
    require(bool(syns) and syns[0] is flow.peer[0] and len({entry.frame.tcp_sequence for entry in syns}) == 1, f"{name}: the connection does not begin with the peer's SYN")
    flow.peer_isn = syns[0].frame.tcp_sequence
    synacks = [entry for entry in flow.guest if entry.frame.tcp_flags == lp.TCP_SYN | lp.TCP_ACK]
    require(bool(synacks) and synacks[0] is flow.guest[0], f"{name}: the guest did not answer the admitted SYN with a SYN-ACK first")
    require(all(entry.frame.tcp_acknowledgment == (flow.peer_isn + 1) & MASK and entry.frame.tcp_sequence == synacks[0].frame.tcp_sequence and not entry.frame.tcp_payload for entry in synacks), f"{name}: the guest's SYN-ACK changed or acknowledges another SYN")
    flow.guest_isn = synacks[0].frame.tcp_sequence
    guest = [entry for entry in flow.guest if not entry.frame.tcp_flags & lp.TCP_SYN]
    require(len(guest) == len(flow.guest) - len(synacks), f"{name}: the guest sent a SYN inside the connection")
    resets = [entry for entry in guest if entry.frame.tcp_flags & lp.TCP_RST]
    require(not resets or name == "unread-close", f"{name}: the guest reset a connection it should close gracefully")
    orders: dict[str, int] = {"synack": synacks[0].order}
    guest_data = [entry for entry in guest if not entry.frame.tcp_flags & lp.TCP_RST]
    peer_data = [entry for entry in flow.peer if not entry.frame.tcp_flags & lp.TCP_SYN]
    guest_first, guest_fin = stream(guest_data, flow.guest_offset, GUEST_STREAMS[name], f"{name}: guest")
    peer_first, peer_fin = stream(peer_data, flow.peer_offset, PEER_STREAMS[name], f"{name}: peer")
    closes = name in FIN_CLOSED
    require((guest_fin == len(GUEST_STREAMS[name])) if closes else guest_fin is None, f"{name}: the guest's FIN is missing, misplaced or unexpected")
    require((peer_fin == len(PEER_STREAMS[name])) if closes else peer_fin is None, f"{name}: the peer's FIN is missing, misplaced or unexpected")
    peer_end = len(PEER_STREAMS[name]) + int(closes)
    require(max((flow.guest_acked(entry.frame) for entry in guest_data if entry.frame.tcp_flags & lp.TCP_ACK), default=0) == peer_end, f"{name}: the guest did not acknowledge exactly the peer's stream")
    # Never acknowledge or keep in flight beyond what the wire and the buffer allow.
    sent_end, peer_acked = 0, 0
    for is_guest, entry in flow.merged():
        frame = entry.frame
        if frame.tcp_flags & lp.TCP_SYN:
            continue
        if is_guest:
            if frame.tcp_flags & lp.TCP_ACK:
                require(flow.guest_acked(frame) <= sent_end, f"{name}: the guest acknowledged bytes the peer had not sent")
            if frame.tcp_payload:
                end = flow.guest_offset(frame) + len(frame.tcp_payload)
                require(end - peer_acked <= SOCKET_BUFFER_BYTES, f"{name}: the guest held more than {SOCKET_BUFFER_BYTES} bytes in flight")
        else:
            sent_end = max(sent_end, flow.peer_offset(frame) + len(frame.tcp_payload) + int(bool(frame.tcp_flags & lp.TCP_FIN)))
            if frame.tcp_flags & lp.TCP_ACK:
                peer_acked = max(peer_acked, flow.peer_acked(frame))
    if guest_fin is not None:
        orders["guest_fin"] = min(entry.order for entry in guest_data if entry.frame.tcp_flags & lp.TCP_FIN)
    if peer_fin is not None:
        orders["peer_fin"] = min(entry.order for entry in peer_data if entry.frame.tcp_flags & lp.TCP_FIN)
    if guest_first:
        orders["guest_first"] = min(guest_first.values())
    if peer_first:
        orders["peer_first"] = min(peer_first.values())
    if resets:
        orders["reset"] = resets[0].order
    return orders


def qualify_local_half_close(flow: Flow, orders: dict[str, int]) -> str:
    exchange = len(EXCHANGE)
    half = [entry for entry in flow.peer if entry.frame.tcp_payload and flow.peer_offset(entry.frame) >= exchange]
    require(bool(half) and orders["guest_fin"] < half[0].order, "local half-close: the peer's half-close bytes preceded the guest's FIN")
    require(not any(entry.frame.tcp_payload and flow.guest_offset(entry.frame) >= len(GUEST_STREAMS["exchange"]) for entry in flow.guest), "local half-close: the guest sent bytes after its FIN")
    acked = first_order(flow.guest, lambda frame: frame.tcp_flags & lp.TCP_ACK and not frame.tcp_flags & lp.TCP_SYN and flow.guest_acked(frame) >= exchange + len(LOCAL_HALF_PEER))
    require(acked is not None and acked > orders["guest_fin"], "local half-close: the guest stopped receiving after shutting down its send direction")
    return f"local-half-close guest_sent={len(GUEST_STREAMS['exchange'])} guest_fin_before_peer_bytes=1 received_after_fin={len(LOCAL_HALF_PEER)}"


def qualify_peer_half_close(flow: Flow, orders: dict[str, int]) -> str:
    fin_acked = first_order(flow.guest, lambda frame: frame.tcp_flags & lp.TCP_ACK and not frame.tcp_flags & lp.TCP_SYN and flow.guest_acked(frame) == len(PEER_HALF_PEER) + 1)
    require(fin_acked is not None and fin_acked > orders["peer_fin"], "peer half-close: the guest never acknowledged the peer's FIN")
    require(orders["guest_first"] > fin_acked, "peer half-close: the guest's bytes did not follow the peer's FIN")
    require(orders["guest_fin"] > orders["peer_fin"], "peer half-close: the guest closed before the peer")
    return f"peer-half-close peer_sent={len(PEER_HALF_PEER)} guest_sent_after_fin={len(PEER_HALF_GUEST)}"


def qualify_simultaneous_close(flow: Flow, orders: dict[str, int]) -> str:
    fins = [entry for entry in flow.peer if entry.frame.tcp_flags & lp.TCP_FIN]
    require(orders["guest_fin"] < orders["peer_fin"], "simultaneous close: the guest's FIN did not cross the peer's")
    require(all(flow.peer_acked(entry.frame) == 0 for entry in fins), "simultaneous close: the peer's FIN acknowledged the guest's FIN, so the closes did not cross")
    crossing = first_order(flow.guest, lambda frame: frame.tcp_flags & lp.TCP_ACK and not frame.tcp_flags & lp.TCP_SYN and flow.guest_acked(frame) == 1 and flow.guest_offset(frame) == 1)
    require(crossing is not None and crossing > orders["peer_fin"], "simultaneous close: the guest did not acknowledge the crossing FIN from its closing state")
    final = first_order(flow.peer, lambda frame: frame.tcp_flags & lp.TCP_ACK and not frame.tcp_flags & lp.TCP_SYN and flow.peer_acked(frame) == 1)
    require(final is not None and final > orders["peer_fin"], "simultaneous close: the peer never acknowledged the guest's FIN")
    return "simultaneous-close crossed=1"


def qualify_unread_close(flow: Flow, orders: dict[str, int], cue: int) -> str:
    resets = [entry for entry in flow.guest if entry.frame.tcp_flags & lp.TCP_RST]
    require(len(resets) == 1, f"unread close: the guest sent {len(resets)} resets, expected one")
    frame = resets[0].frame
    require(not frame.tcp_payload and not frame.tcp_flags & (lp.TCP_SYN | lp.TCP_FIN) and flow.guest_offset(frame) == 0, "unread close: the reset is malformed or not at the guest's next sequence")
    acked = first_order(flow.guest, lambda guest: guest.tcp_flags & lp.TCP_ACK and not guest.tcp_flags & (lp.TCP_SYN | lp.TCP_RST) and flow.guest_acked(guest) == len(UNREAD))
    require(acked is not None and acked < cue < orders["reset"], "unread close: the reset did not follow the guest's acknowledgment of the unread bytes and the cue")
    require(resets[0] is flow.guest[-1], "unread close: the guest sent frames after its reset")
    return f"unread-close unread={len(UNREAD)} reset=1 fin=0"


def qualify_unsent_close(flow: Flow, orders: dict[str, int], queued: int) -> str:
    opened = first_order(flow.peer, lambda frame: not frame.tcp_flags & lp.TCP_SYN and frame.tcp_window > 0)
    require(opened is not None and opened > queued, "unsent close: the peer's window opened before the guest queued its bytes")
    require(all(entry.frame.tcp_window == 0 for entry in flow.peer if entry.order < opened), "unsent close: the peer's window was not zero until it opened")
    early = [entry for entry in flow.guest if entry.order < opened and entry.frame.tcp_payload]
    require(all(len(entry.frame.tcp_payload) <= 1 for entry in early), "unsent close: the guest sent more than a one-byte probe into the zero window")
    require(all(flow.peer_acked(entry.frame) == 0 for entry in flow.peer if entry.order < opened and not entry.frame.tcp_flags & lp.TCP_SYN), "unsent close: the peer accepted a probe byte while its window was zero")
    require(orders["guest_fin"] > opened, "unsent close: the guest's FIN preceded its queued bytes")
    return f"unsent-close queued={len(UNSENT)} probes={len(early)} fin_after_data=1"


def qualify_listener_ledger(ledger: Ledger, guest_mac: bytes) -> list[str]:
    """Judge every attempt and scenario from the wire; return one summary per case."""
    require(ledger.malformed_received == 0, f"{ledger.malformed_received} unclassifiable frames reached the peer")
    require(ledger.desync is None, f"the guest's control stream left the script: {ledger.desync}")
    require(ledger.stage == len(ListenerPeer().stages(0.0)), f"the script stopped at stage {ledger.stage}")
    flows = split_flows(ledger, guest_mac)
    controls = [flow for flow in flows.values() if flow.peer_ip == lp.PEER_IP and flow.peer_port == CONTROL_PORT]
    require(len(controls) == 1, f"expected one control connection, found {len(controls)}")
    control = controls[0]
    qualify_control(control)
    lines, cues = control_orders(control)
    summaries = [f"control lines={len(CONTROL_LINES)} cues={len(CUES)}"]
    known = {(attempt.source_ip, attempt.source_port, attempt.destination_port) for attempt in ATTEMPTS} | {(control.peer_ip, control.peer_port, control.guest_port)}
    require(set(flows) <= known, "the guest answered a tuple the script never opened")
    syn_orders: dict[str, int] = {}
    accepted: dict[str, dict[str, int]] = {}
    for attempt in ATTEMPTS:
        flow = flows.get((attempt.source_ip, attempt.source_port, attempt.destination_port))
        require(flow is not None and bool(flow.peer), f"{attempt.name}: the peer never sent its SYN")
        assert flow is not None
        syn = flow.peer[0].frame
        require(syn.tcp_flags == lp.TCP_SYN and syn.ip_destination == attempt.destination_ip and syn.source == attempt.source_mac, f"{attempt.name}: the peer's SYN is not the declared attempt")
        syn_orders[attempt.name] = flow.peer[0].order
        if attempt.outcome == "silent":
            require(not flow.guest, f"{attempt.name}: the guest answered a SYN the listener must ignore")
            summaries.append(f"{attempt.name} silent=1")
        elif attempt.outcome == "reset":
            require(len(flow.guest) == 1, f"{attempt.name}: expected exactly one guest frame, the refusal; saw {len(flow.guest)}")
            refusal = flow.guest[0].frame
            require(refusal.tcp_flags == lp.TCP_RST | lp.TCP_ACK and refusal.tcp_sequence == 0 and refusal.tcp_acknowledgment == (syn.tcp_sequence + 1) & MASK and not refusal.tcp_payload, f"{attempt.name}: the refusal is not a reset acknowledging exactly the SYN")
            require(all(entry.frame.tcp_flags == lp.TCP_SYN for entry in flow.peer), f"{attempt.name}: the peer continued a refused connection")
            accepted[attempt.name] = {"reset": flow.guest[0].order}
            summaries.append(f"{attempt.name} reset=1")
        else:
            orders = qualify_accepted(flow, attempt.name)
            accepted[attempt.name] = orders
            if attempt.name == "exchange":
                summaries.append(f"exchange sent={len(EXCHANGE)} echoed={len(EXCHANGE)} identical=1")
                summaries.append(qualify_local_half_close(flow, orders))
            elif attempt.name == "backlog-held":
                summaries.append(qualify_peer_half_close(flow, orders))
            elif attempt.name == "simultaneous-close":
                summaries.append(qualify_simultaneous_close(flow, orders))
            elif attempt.name == "unread-close":
                summaries.append(qualify_unread_close(flow, orders, cues[2]))
            else:
                summaries.append(qualify_unsent_close(flow, orders, lines[10]))
    # The script's ordering, as the wire shows it.
    names = [attempt.name for attempt in ATTEMPTS]
    require([syn_orders[name] for name in names] == sorted(syn_orders.values()), "the attempts did not open in script order")
    silent = [syn_orders[attempt.name] for attempt in ATTEMPTS if attempt.outcome == "silent"]
    require(lines[0] < min(silent) and max(silent) < syn_orders["exchange"], "the refused SYNs did not fall between the guest's report that it listens and the admitted exchange")
    require(accepted["exchange"]["synack"] < lines[1], "the guest reported an accept before the handshake")
    require(syn_orders["backlog-full"] > accepted["backlog-held"]["synack"] and lines[2] > cues[0] > accepted["backlog-full"]["reset"], "the backlog refusal did not happen while the held connection awaited accept")
    require(syn_orders["accepted-limit"] > lines[2] and cues[1] > accepted["accepted-limit"]["reset"], "the accepted-limit refusal did not happen with both connections accepted")
    require(accepted["exchange"]["guest_fin"] > cues[1], "the local half-close began before its cue")
    require(accepted["exchange"]["guest_fin"] < lines[3] < accepted["backlog-held"]["peer_first"], "the local half-close did not finish before the peer half-close began")
    require(accepted["backlog-held"]["guest_fin"] < lines[4] < syn_orders["simultaneous-close"], "the peer half-close did not finish before the next connection")
    require(accepted["simultaneous-close"]["synack"] < lines[5] <= lines[6] < syn_orders["unread-close"], "the simultaneous close was not reported in order")
    require(lines[7] < accepted["unread-close"]["peer_first"] and accepted["unread-close"]["reset"] < lines[8] < syn_orders["unsent-close"], "the unread close was not reported in order")
    require(accepted["unsent-close"]["synack"] < lines[9] <= lines[10] and accepted["unsent-close"]["guest_fin"] < lines[11] <= lines[12], "the unsent close was not reported in order")
    return summaries
