"""Judge a guest-to-host Zenoh Profile 0 exchange from a packet capture and a listener receipt.

`zenoh_exchange` judges what the nodes print. This judges what crossed the wire: one classic PCAP
taken at the guest's virtio-net device, reassembled into the two TCP byte streams, split into
Zenoh batches by their two-byte stream length, and held against the host wire reference
(`zenoh_wire`, written independently of the Rust codec), the guest's own report of what it sent,
and the record a stock eclipse-zenoh peer wrote about what its subscriber delivered.

Admitted on the wire: ARP, and TCP between the guest and the declared peer. Nothing else: no DNS,
no UDP (so no scouting), no ICMP, no second connection, no other address or port, no reset.

Every judge is a pure function over bytes and text. `synthesize` builds the capture an honest
guest would produce, `controls` mutates it, and a judge is trusted only after it refuses every
mutation and accepts the honest variants (reordered, retransmitted and re-segmented streams).

Limit: QEMU user networking is not a physical NIC, and a capture proves the packets it holds, not
that the peer's subscriber acted on them; that is what the receipt is for.
"""

from __future__ import annotations

import copy
import re
import shutil
import struct
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

import http_capture
import zenoh_exchange
import zenoh_wire
import zutai_cli
from harness import ROOT

GUEST = http_capture.GUEST
PEER = "10.0.2.2"
PORT = 17447
LISTEN = f"tcp/127.0.0.1:{PORT}"
SAMPLES = 4
LEASE_MS = 2000
PUBLISHER = "ros2-demo-publisher"
SUBSCRIBER = "ros2-demo-subscriber"
DENIALS = ("undeclared-endpoint", "listen", "scouting")
MAX_STREAM = 1 << 20

ID_KEEP_ALIVE = 4
TCP_FIN, TCP_SYN, TCP_RST, TCP_ACK = 1, 2, 4, 16


class CaptureError(ValueError):
    """Evidence the judge refuses."""


def _need(condition: bool, reason: str) -> None:
    if not condition:
        raise CaptureError(reason)


@dataclass(frozen=True)
class Segment:
    outgoing: bool
    source_port: int
    destination_port: int
    seq: int
    ack: int
    flags: int
    payload: bytes


# ------------------------------------------------------------------------------ capture


def _check_arp(frame: bytes) -> None:
    """An ARP request or reply between the guest and the declared peer, and nothing else.

    The sender and target protocol addresses must be the two endpoints, one each, and the sender's
    hardware address must be the Ethernet source; the guest's is the MAC the composition declares.
    """
    _need(len(frame) >= 42, "short ARP frame")
    htype, ptype, hlen, plen, operation = struct.unpack_from("!HHBBH", frame, 14)
    _need((htype, ptype, hlen, plen) == (1, 0x0800, 6, 4), "ARP that is not Ethernet and IPv4")
    _need(operation in (1, 2), f"ARP operation {operation} is neither a request nor a reply")
    sender_mac = frame[22:28]
    sender, target = (".".join(str(b) for b in frame[at : at + 4]) for at in (28, 38))
    _need({sender, target} == {GUEST, PEER}, f"ARP from {sender} for {target}: only {GUEST} and {PEER} may speak")
    _need(sender_mac == frame[6:12], "the ARP sender hardware address is not the Ethernet source")
    _need((sender == GUEST) == (sender_mac == GUEST_MAC), f"the ARP sender {sender} has hardware address {sender_mac.hex()}")


def segments(data: bytes) -> tuple[list[Segment], int]:
    """Every TCP segment in the capture, and the number of ARP frames. Anything else is refused."""
    found: list[Segment] = []
    arp = 0
    try:
        frames = list(http_capture._frames(data))
    except ValueError as error:
        raise CaptureError(str(error)) from error
    for _, frame in frames:
        ethertype = int.from_bytes(frame[12:14], "big")
        if ethertype == 0x0806:
            _check_arp(frame)
            arp += 1
            continue
        _need(ethertype == 0x0800, f"Ethernet protocol {ethertype:#06x} is neither ARP nor IPv4")
        ip = frame[14:]
        _need(len(ip) >= 20 and ip[0] >> 4 == 4, "IPv4 header")
        header_len, total = (ip[0] & 15) * 4, int.from_bytes(ip[2:4], "big")
        _need(20 <= header_len <= total <= len(ip), "IPv4 lengths")
        _need(http_capture._checksum(ip[:header_len]) == 0, "IPv4 checksum")
        _need(int.from_bytes(ip[6:8], "big") & 0xBFFF == 0, "fragmented or reserved IPv4")
        _need(ip[9] == 6, f"IP protocol {ip[9]} is not TCP: no DNS, UDP or ICMP is admitted")
        source = ".".join(str(b) for b in ip[12:16])
        destination = ".".join(str(b) for b in ip[16:20])
        _need((source, destination) in ((GUEST, PEER), (PEER, GUEST)), f"packet between {source} and {destination}")
        body = ip[header_len:total]
        _need(len(body) >= 20, "TCP header")
        offset = (body[12] >> 4) * 4
        _need(20 <= offset <= len(body) and body[12] & 15 == 0, "TCP data offset")
        pseudo = ip[12:20] + bytes((0, 6)) + len(body).to_bytes(2, "big")
        _need(http_capture._checksum(pseudo + body) == 0, "TCP checksum")
        sport, dport, seq, ack = struct.unpack_from("!HHII", body)
        found.append(Segment(source == GUEST, sport, dport, seq, ack, body[13], body[offset:]))
    return found, arp


def _reassemble(segs: list[Segment], isn: int, label: str) -> bytes:
    """One direction's byte stream, from the sender's own sequence numbers.

    Retransmission and reordering are allowed; a gap, a retransmission that differs, and a FIN
    that does not sit exactly at the end of the delivered bytes are refused.
    """
    pieces: list[tuple[int, bytes]] = []
    fin: int | None = None
    for s in segs:
        if s.flags & TCP_SYN:
            _need(not s.payload, f"{label}: a SYN carries data")
            continue
        offset = (s.seq - isn - 1) & 0xFFFFFFFF
        _need(offset < 1 << 31, f"{label}: a segment before the connection's first byte")
        if s.payload:
            pieces.append((offset, s.payload))
        if s.flags & TCP_FIN:
            end = offset + len(s.payload)
            _need(fin is None or fin == end, f"{label}: two different FINs")
            fin = end
    pieces.sort(key=lambda piece: piece[0])
    stream = bytearray()
    for offset, payload in pieces:
        _need(offset <= len(stream), f"{label}: a gap of {offset - len(stream)} bytes at offset {len(stream)}")
        overlap = bytes(stream[offset : offset + len(payload)])
        _need(payload[: len(overlap)] == overlap, f"{label}: a retransmission differs from the first bytes")
        stream += payload[len(overlap) :]
        _need(len(stream) <= MAX_STREAM, f"{label}: stream over {MAX_STREAM} bytes")
    _need(fin is not None, f"{label}: the stream was never closed with a FIN")
    _need(fin == len(stream), f"{label}: the FIN is at {fin} but {len(stream)} bytes were delivered")
    return bytes(stream)


def streams(data: bytes) -> tuple[bytes, bytes, int]:
    """The guest-to-peer and peer-to-guest streams of the capture's one connection, and its ARP count."""
    segs, arp = segments(data)
    syns = [s for s in segs if s.outgoing and s.flags & TCP_SYN and not s.flags & TCP_ACK]
    _need(len(syns) >= 1, "no connection attempt: the capture holds no guest SYN")
    _need(len({s.source_port for s in syns}) == 1, f"{len({s.source_port for s in syns})} connections, exactly one is admitted")
    _need(len({s.seq for s in syns}) == 1, "two SYNs with different sequence numbers")
    syn = syns[0]
    _need(syn.destination_port == PORT, f"the connection goes to port {syn.destination_port}, the exam declares {PORT}")
    for s in segs:
        _need(not s.flags & TCP_RST, "a reset: the connection did not close gracefully")
        ports = (s.source_port, s.destination_port)
        _need(ports == ((syn.source_port, PORT) if s.outgoing else (PORT, syn.source_port)), "a segment outside the one connection")
    synacks = [s for s in segs if not s.outgoing and s.flags & TCP_SYN and s.flags & TCP_ACK]
    _need(len({s.seq for s in synacks}) == 1, "the peer never accepted: no single SYN-ACK")
    _need(synacks[0].ack == (syn.seq + 1) & 0xFFFFFFFF, "the SYN-ACK acknowledges another sequence number")
    guest = _reassemble([s for s in segs if s.outgoing], syn.seq, "guest stream")
    peer = _reassemble([s for s in segs if not s.outgoing], synacks[0].seq, "peer stream")
    return guest, peer, arp


def split(stream: bytes, label: str) -> list[bytes]:
    """Zenoh batches by their two-byte little-endian stream length."""
    batches, at = [], 0
    while at < len(stream):
        _need(at + 2 <= len(stream), f"{label}: a stream length cut short")
        length = int.from_bytes(stream[at : at + 2], "little")
        _need(length > 0, f"{label}: an empty batch")
        _need(at + 2 + length <= len(stream), f"{label}: a batch of {length} bytes with {len(stream) - at - 2} left")
        batches.append(stream[at + 2 : at + 2 + length])
        at += 2 + length
    return batches


# -------------------------------------------------------------------------------- wire


def _kind(batch: bytes) -> int:
    return batch[0] & 0x1F


def judge_guest(batches: list[bytes], reported: dict[int, str], contract: dict) -> dict:
    """The guest's batches: a connector's INIT_SYN and OPEN_SYN, the four samples, keep-alives, a close."""
    _need(len(batches) >= 2, "the guest sent fewer than an INIT_SYN and an OPEN_SYN")
    init, opened = batches[0], batches[1]
    _need(_kind(init) == zenoh_wire.ID_INIT and not init[0] & zenoh_wire.FLAG_A, "the first guest batch is not an INIT_SYN")
    _need(len(init) >= 3 and init[1] == zenoh_wire.PROTOCOL_VERSION, "the INIT_SYN is not wire version 9")
    _need(init[2] & 3 == zenoh_wire.WHATAMI_PEER, "the INIT_SYN is not a peer")
    _need(_kind(opened) == zenoh_wire.ID_OPEN and not opened[0] & zenoh_wire.FLAG_A, "the second guest batch is not an OPEN_SYN")
    try:
        lease, at = zenoh_wire.read_leb128(opened, 1)
        initial_sn, at = zenoh_wire.read_leb128(opened, at)
        cookie, at = zenoh_wire.read_leb128(opened, at)
    except zenoh_wire.WireError as error:
        raise CaptureError(f"OPEN_SYN: {error}") from error
    _need(at + cookie == len(opened), "the OPEN_SYN length disagrees with its cookie")
    lease *= 1000 if opened[0] & zenoh_wire.FLAG_T else 1
    _need(lease == LEASE_MS, f"the guest announced a {lease} ms lease, the exam declares {LEASE_MS}")
    frames: list[bytes] = []
    keep_alives = 0
    closed = False
    for batch in batches[2:]:
        _need(not closed, "a batch after the guest's CLOSE")
        if batch == bytes([ID_KEEP_ALIVE]):
            keep_alives += 1
        elif _kind(batch) == zenoh_wire.ID_CLOSE and len(batch) == 2:
            closed = True
        elif batch[0] == zenoh_wire.ID_FRAME | zenoh_wire.FLAG_R:
            frames.append(batch)
        else:
            raise CaptureError(f"a guest batch the exam does not name: header {batch[0]:#04x}, {len(batch)} bytes")
    _need(len(frames) == SAMPLES, f"the guest put {len(frames)} frames on the wire, the demo has {SAMPLES} samples")
    _need(sorted(reported) == list(range(SAMPLES)), "the guest reported its sent batches for the wrong samples")
    received = []
    for index, (sequence, _value, cdr) in enumerate(contract["samples"]):
        batch = frames[index]
        _need(batch.hex() == reported[sequence], f"sample {sequence}: the wire holds other bytes than the guest reported")
        try:
            decoded = zenoh_wire.decode_push_frame(batch)
        except zenoh_wire.WireError as error:
            raise CaptureError(f"sample {sequence}: {error}") from error
        _need(decoded["key"] == contract["key"], f"sample {sequence}: key expression {decoded['key']!r} is not the demo's")
        _need(decoded["sn"] == initial_sn + index, f"sample {sequence}: frame sn {decoded['sn']} is not {initial_sn} + {index}")
        _need(decoded["payload"].hex() == cdr, f"sample {sequence}: the payload is not the contract's")
        attachment = zenoh_wire.attachment(decoded["sequence"], decoded["timestamp"], decoded["gid"])
        again = zenoh_wire.frame(decoded["sn"], zenoh_wire.push_put(decoded["key"], attachment, decoded["payload"], True))
        _need(again == batch, f"sample {sequence}: not the canonical encoding of its own fields")
        received.append((decoded["key"], decoded["payload"].hex(), attachment.hex()))
    return {"frames": len(frames), "bytes": sum(len(f) for f in frames), "keep_alives": keep_alives, "closed": closed, "samples": received}


def judge_peer(batches: list[bytes]) -> int:
    """The peer's batches: an INIT_ACK and an OPEN_ACK, then only frames, keep-alives and a close."""
    _need(len(batches) >= 2, "the peer sent fewer than an INIT_ACK and an OPEN_ACK")
    _need(_kind(batches[0]) == zenoh_wire.ID_INIT and batches[0][0] & zenoh_wire.FLAG_A, "the first peer batch is not an INIT_ACK")
    _need(_kind(batches[1]) == zenoh_wire.ID_OPEN and batches[1][0] & zenoh_wire.FLAG_A, "the second peer batch is not an OPEN_ACK")
    for batch in batches[2:]:
        _need(_kind(batch) in (zenoh_wire.ID_FRAME, ID_KEEP_ALIVE, zenoh_wire.ID_CLOSE), f"a peer batch of kind {_kind(batch)}")
    return len(batches)


def judge_capture(data: bytes, reported: dict[int, str], contract: dict) -> dict:
    guest, peer, arp = streams(data)
    result = judge_guest(split(guest, "guest stream"), reported, contract)
    result["peer_batches"] = judge_peer(split(peer, "peer stream"))
    result.update(guest_stream=len(guest), peer_stream=len(peer), arp=arp)
    return result


# ---------------------------------------------------------------------------- transcript


def guest_lines(contract: dict) -> list[str]:
    """What the connector prints when an honest guest runs against an external listener."""
    key = contract["key"]
    lines = [f"[{PUBLISHER}] denial class={cls} refused=1" for cls in DENIALS]
    lines += [f"[{PUBLISHER}] session open role=connector initial_sn=0 lease_ms={LEASE_MS}", f"[{PUBLISHER}] declaration matched key={key}"]
    for sequence, _value, cdr in contract["samples"]:
        att = zenoh_wire.attachment(sequence + 1, 1000, bytes(range(0xE0, 0xF0)))
        batch = zenoh_wire.frame(sequence, zenoh_wire.push_put(key, att, bytes.fromhex(cdr), True))
        lines.append(f"[{PUBLISHER}] wire sent sample={sequence} hex={batch.hex()}")
    lines.append(f"[{PUBLISHER}] session closed samples={SAMPLES}")
    return lines


def judge_guest_lines(lines: list[str], contract: dict) -> dict[int, str]:
    """The connector's report: its denials, one session, one match, four sent batches, one close.

    Returns the hex of each batch it says it sent, by sample, for the wire to be held against.
    """
    own = [line for line in lines if line.startswith(f"[{PUBLISHER}] ")]
    _need(len(own) == len(lines), "a line from a node other than the connector")
    key = re.escape(contract["key"])
    patterns = {
        "session": rf"\[{PUBLISHER}\] session open role=connector initial_sn=\d+ lease_ms={LEASE_MS}",
        "match": rf"\[{PUBLISHER}\] declaration matched key={key}",
        "close": rf"\[{PUBLISHER}\] session closed samples={SAMPLES}",
    }
    for name, pattern in patterns.items():
        count = len([1 for line in lines if re.fullmatch(pattern, line)])
        _need(count == 1, f"{name}: {count} matching lines, expected exactly one")
    for cls in DENIALS:
        count = lines.count(f"[{PUBLISHER}] denial class={cls} refused=1")
        _need(count == 1, f"{cls} denial: {count} lines, expected exactly one")
    _need(len([1 for line in lines if " denial " in line]) == len(DENIALS), "a denial the exam does not name")
    sent = {}
    wire = [m for line in lines if (m := re.fullmatch(rf"\[{PUBLISHER}\] wire sent sample=(\d+) hex=([0-9a-f]+)", line))]
    _need(len(wire) == SAMPLES, f"{len(wire)} sent batches reported, expected {SAMPLES}")
    for m in wire:
        sent[int(m.group(1))] = m.group(2)
    _need(sorted(sent) == list(range(SAMPLES)), "the reported samples are not 0 to 3 once each")
    _need(len([1 for line in lines if line.startswith(f"[{PUBLISHER}] ") and not re.search(r" (denial|wire sent|session|declaration matched) ", line)]) == 0, "a connector line the exam does not name")
    position = {line: i for i, line in enumerate(lines)}
    opened = next(i for i, line in enumerate(lines) if re.fullmatch(patterns["session"], line))
    matched = next(i for i, line in enumerate(lines) if re.fullmatch(patterns["match"], line))
    where = [next(i for i, line in enumerate(lines) if f" wire sent sample={n} " in line) for n in range(SAMPLES)]
    closed = position[f"[{PUBLISHER}] session closed samples={SAMPLES}"]
    for cls in DENIALS:
        _need(position[f"[{PUBLISHER}] denial class={cls} refused=1"] < opened, f"the {cls} denial came after the session opened")
    _need(opened < matched < where[0], "the order is not session open, declaration matched, first sample")
    _need(where == sorted(where), f"the sent batches are reported in the order of samples {where}, not 0 to {SAMPLES - 1}")
    _need(closed == len(lines) - 1 and closed > where[-1], "the session did not close last")
    return sent


# ------------------------------------------------------------------------------ receipt


def judge_receipt(listen: str, remote_peers: int, samples: list[tuple[str, str, str]], wire: list[tuple[str, str, str]]) -> int:
    """The stock listener's own account of what its subscriber delivered, against the wire."""
    _need(listen == LISTEN, f"the listener reports {listen!r}, the exam declares {LISTEN!r}")
    _need(1 <= remote_peers <= 8, f"the listener saw {remote_peers} remote peers")
    _need(len(samples) == SAMPLES, f"the listener delivered {len(samples)} samples, expected {SAMPLES}")
    for index, (got, sent) in enumerate(zip(samples, wire, strict=True)):
        _need(got[0] == sent[0], f"sample {index}: the listener matched key {got[0]!r}, the wire carried {sent[0]!r}")
        _need(got[1] == sent[1], f"sample {index}: the listener delivered payload {got[1]}, the wire carried {sent[1]}")
        _need(got[2] == sent[2], f"sample {index}: the listener delivered another attachment than the wire carried")
    return len(samples)


CLOSURE = "sel4-zenoh-external"
GENERATION = 169
INSTANCES = ("init", "network-service", "virtio-net-driver", PUBLISHER, SUBSCRIBER)
INTERFACE = {"holder": "network-service", "address": GUEST, "gateway": PEER, "mac": "52:54:00:53:4c:01", "prefixLen": 24}


def external_from_text(text: str) -> tuple:
    """Generation, instances, interfaces, destinations and applications of a derived manifest."""
    generation, instances, destinations, applications = zenoh_exchange.composition_from_text(text)
    return generation, instances, zenoh_exchange.parse_rows(text, "networkInterfaces"), destinations, applications


def check_external_rows(generation: int, instances: list[str], interfaces: list[dict], destinations: list[dict], applications: list[dict]) -> int:
    """Refuse a composition that grants the demo more or less than one outbound connection.

    Returns the number of authority facts verified.
    """
    facts = 0

    def need(condition: bool, message: str) -> None:
        nonlocal facts
        if not condition:
            raise CaptureError(message)
        facts += 1

    need(generation == GENERATION, f"generation {generation}, the exam pins {GENERATION}")
    need(sorted(instances) == sorted(INSTANCES), f"instances {sorted(instances)}, expected {sorted(INSTANCES)}")
    need(len(interfaces) == 1, f"{len(interfaces)} network interfaces, the exam declares one")
    need({k: interfaces[0].get(k) for k in INTERFACE} == INTERFACE, "the interface is not the user-network address, gateway and MAC the exam declares")
    need(len(destinations) == 1, f"{len(destinations)} destination rows, the exam declares one connect endpoint")
    row = destinations[0]
    need(row.get("holder") == PUBLISHER, "the destination is not held by the publisher")
    need((row.get("address"), row.get("port")) == (PEER, PORT), f"the destination is not {PEER}:{PORT}")
    need(row.get("transport") == "tcp" and row.get("addressKind") == "ipv4", "the destination is not exact IPv4 TCP")
    need(sorted(row.get("rights", [])) == ["connect", "recv", "send"], "the destination rights are not exactly connect, send, recv")
    need(row.get("listenerLimit") == 0 and row.get("dnsRecordLimit") == 0, "the destination grants listen or DNS authority")
    need(len(applications) == 1 and applications[0].get("holder") == PUBLISHER, "the only network application is not the publisher")
    publisher = applications[0]
    need(publisher.get("role") == "client" and publisher.get("rights") == [], "the publisher is not a rightless client")
    need(publisher.get("backend") == "external", "the publisher does not use the external backend")
    need(publisher.get("admittedPeer") == "" and publisher.get("admittedPeerAddress") == "", "the publisher admits a peer")
    need(all(r.get("holder") != SUBSCRIBER for r in destinations + applications), "the subscriber holds network authority")
    return facts


def good_external() -> tuple:
    interface = dict(INTERFACE)
    destination = {
        "holder": PUBLISHER, "transport": "tcp", "addressKind": "ipv4", "address": PEER, "port": PORT,
        "rights": ["connect", "send", "recv"], "listenerLimit": 0, "dnsRecordLimit": 0,
    }
    publisher = {"holder": PUBLISHER, "role": "client", "backend": "external", "rights": [], "admittedPeer": "", "admittedPeerAddress": ""}
    return (GENERATION, list(INSTANCES), [interface], [destination], [publisher])


def external_mutations() -> list[tuple[str, Callable[[list], None]]]:
    def interface(**c: object) -> Callable[[list], None]:
        return lambda v: v[2][0].update(c)

    def dest(**c: object) -> Callable[[list], None]:
        return lambda v: v[3][0].update(c)

    def publisher(**c: object) -> Callable[[list], None]:
        return lambda v: v[4][0].update(c)

    return [
        ("generation 168", lambda v: v.__setitem__(0, 168)),
        ("generation 170", lambda v: v.__setitem__(0, 170)),
        ("an extra instance", lambda v: v[1].append("zenoh-router")),
        ("the subscriber missing", lambda v: v[1].remove(SUBSCRIBER)),
        ("no driver", lambda v: v[1].remove("virtio-net-driver")),
        ("no interface", lambda v: v[2].clear()),
        ("two interfaces", lambda v: v[2].append(dict(v[2][0]))),
        ("another guest address", interface(address="10.0.2.16")),
        ("another gateway", interface(gateway="10.0.2.3")),
        ("another MAC", interface(mac="52:54:00:53:4c:02")),
        ("another prefix", interface(prefixLen=16)),
        ("no destination", lambda v: v[3].clear()),
        ("a second destination", lambda v: v[3].append(dict(v[3][0], port=PORT + 1))),
        ("a resolver row", lambda v: v[3].append(dict(v[3][0], holder="network-service", transport="udp", port=53))),
        ("port 17446, one under", dest(port=PORT - 1)),
        ("port 17448, one over", dest(port=PORT + 1)),
        ("the loopback address", dest(address="127.0.0.1")),
        ("a DNS name", dest(addressKind="dns", address="example.test")),
        ("a udp destination", dest(transport="udp")),
        ("a missing right", dest(rights=["connect", "send"])),
        ("an extra right", dest(rights=["connect", "send", "recv", "listen"])),
        ("a listen limit", dest(listenerLimit=1)),
        ("a resolver record", dest(dnsRecordLimit=1)),
        ("the destination held by the subscriber", dest(holder=SUBSCRIBER)),
        ("a loopback backend", publisher(backend="loopback")),
        ("a publisher with rights", publisher(rights=["listen"])),
        ("a listening publisher", publisher(role="listener")),
        ("a publisher admitting a peer", publisher(admittedPeer=SUBSCRIBER)),
        ("a subscriber application row", lambda v: v[4].append(dict(v[4][0], holder=SUBSCRIBER))),
        ("no application", lambda v: v[4].clear()),
    ]


def external_controls() -> tuple[int, int]:
    """Judge the honest composition, then refuse every mutation. Returns (facts, refused)."""
    good = good_external()
    facts = check_external_rows(*good)
    refused = 0
    for label, change in external_mutations():
        copy_ = copy.deepcopy(list(good))
        change(copy_)
        try:
            check_external_rows(*copy_)
        except CaptureError:
            refused += 1
            continue
        raise CaptureError(f"control: the composition judge accepted {label}")
    return facts, refused


def read_receipt(path: Path) -> tuple[str, int, list[tuple[str, str, str]]]:
    """Decode a listener record through contracts/zenoh-external-receipt/v1, then read its values.

    The contract's decoder refuses a closed-set violation, a wrong version and every bound; what is
    read afterwards is the decoder's own spelling of an accepted record.
    """
    scratch = ROOT / "build" / "zenoh-external" / "decoder"
    scratch.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(ROOT / "contracts" / "zenoh-external-receipt" / "v1" / "schema.zt", scratch / "schema.zt")
    (scratch / "decode.zt").write_text(
        's ::= import "schema.zt"; env ::= import stdlib.env; load ::= import stdlib.load; '
        "main :: { env : Env; load : Load; } -> Validation DecodeIssue s.Receipt ! { * env.GetEffects; * load.ZtiEffects; } "
        '= caps => [ path := env.get caps.env "SLIME_RECEIPT_PATH" ?? ""; s.decodeReceipt (load.zti caps.load path) ]; main\n',
        encoding="utf-8",
    )
    try:
        decoded = zutai_cli.evaluate("run", scratch / "decode.zt", input_path=path, env_var="SLIME_RECEIPT_PATH")
    except zutai_cli.ZutaiError as error:
        raise CaptureError(f"the listener record does not decode: {error}") from error
    _need(decoded.startswith("#valid"), f"the listener record is refused by its contract: {decoded[:160]}")
    listen = re.search(r'\blisten = "([^"]*)"', decoded)
    peers = re.search(r"\bmaxRemotePeers = (\d+)", decoded)
    _need(listen is not None and peers is not None, "the decoded record has no listen or maxRemotePeers")
    rows = re.findall(r'\{ attachmentHex = "([0-9a-f]*)";\s*key = "([^"]*)";\s*payloadHex = "([0-9a-f]*)" \}', decoded)
    _need(len(rows) == decoded.count("attachmentHex ="), "a sample the reader cannot parse")
    return listen.group(1), int(peers.group(1)), [(key, payload, attachment) for attachment, key, payload in rows]


def receipt_cases(contract: dict) -> list[tuple[str, str, bool]]:
    """(label, record text, accepted) for the listener record, on each boundary the contract fixes."""
    key = contract["key"]
    att = "00" * 8 + "e8030000" + "00" * 4 + "10" + "e0" * 16
    payload = contract["samples"][0][2]

    def sample(k: str = key, p: str = payload, a: str = att) -> str:
        return f'    {{ key = "{k}"; payloadHex = "{p}"; attachmentHex = "{a}"; }};\n'

    def record(version: int = 1, peer: str = "1.0.0", peers: int = 1, rows: list[str] | None = None, top: str = "", inner: str = "") -> str:
        body = "".join(rows if rows is not None else [sample() for _ in range(SAMPLES)])
        if inner:
            body = body.replace(" };\n", f" {inner} }};\n", 1)
        return f'{{\n  formatVersion = {version};\n  peerVersion = "{peer}";\n  listen = "{LISTEN}";\n  maxRemotePeers = {peers};\n{top}  samples = [\n{body}  ];\n}}\n'

    def with_first(first: str) -> list[str]:
        return [first] + [sample() for _ in range(SAMPLES - 1)]

    return [
        ("an honest record", record(), True),
        ("the shortest key, 1 byte", record(rows=with_first(sample(k="k"))), True),
        ("a key of 128 bytes, one under the longest", record(rows=with_first(sample(k="k" * 128))), True),
        ("the longest key, 129 bytes", record(rows=with_first(sample(k="k" * 129))), True),
        ("a payload of 11 bytes, one under the longest", record(rows=with_first(sample(p="00" * 11))), True),
        ("the longest payload, 12 bytes", record(rows=with_first(sample(p="00" * 12))), True),
        ("two remote peers, one over the fewest", record(peers=2), True),
        ("seven remote peers, one under the most", record(peers=7), True),
        ("eight remote peers", record(peers=8), True),
        ("three samples", record(rows=[sample() for _ in range(SAMPLES - 1)]), False),
        ("five samples", record(rows=[sample() for _ in range(SAMPLES + 1)]), False),
        ("format version 2", record(version=2), False),
        ("another peer version", record(peer="1.1.0"), False),
        ("no remote peer", record(peers=0), False),
        ("nine remote peers", record(peers=9), False),
        ("a key of 130 bytes", record(rows=with_first(sample(k="k" * 130))), False),
        ("an empty key", record(rows=with_first(sample(k=""))), False),
        ("a payload of 13 bytes", record(rows=with_first(sample(p="00" * 13))), False),
        ("uppercase payload hex", record(rows=with_first(sample(p="0A"))), False),
        ("odd-length payload hex", record(rows=with_first(sample(p="0"))), False),
        ("an attachment of 32 bytes", record(rows=with_first(sample(a=att[:-2]))), False),
        ("an attachment of 34 bytes", record(rows=with_first(sample(a=att + "00"))), False),
        ("a field the contract does not name", record(top='  note = "x";\n'), False),
        ("a sample field the contract does not name", record(inner='note = "x";'), False),
    ]


def receipt_controls(contract: dict) -> tuple[int, int]:
    """Run each record through the contract's decoder and the reader. Returns (accepted, refused)."""
    scratch = ROOT / "build" / "zenoh-external"
    scratch.mkdir(parents=True, exist_ok=True)
    path = scratch / "receipt-control.zti"
    accepted = refused = 0
    for label, text, should in receipt_cases(contract):
        path.write_text(text, encoding="utf-8")
        try:
            read_receipt(path)
            took = True
        except CaptureError:
            took = False
        _need(took == should, f"control: the receipt reader {'refused' if should else 'accepted'} {label}")
        accepted, refused = accepted + took, refused + (not took)
    return accepted, refused


def controls_refused(contract: dict) -> int:
    """Every corrupted record the three judges refuse: captures and reports, listener records, compositions."""
    _samples, _accepted, capture = controls(contract)
    return capture + receipt_controls(contract)[1] + external_controls()[1]


# What the admitted item fixes, held here as well so the gate stays correct after the item is
# completed and retired to .tasks/terminal. While the item is active its stored body is compared
# with these; once it is gone these still bind. The count is exact, not a floor.
PINNED_CASES = 4
PINNED_CONTROLS = 112
ADMISSION = "slime-zenoh-profile0-external-listener-"


def item_pins() -> tuple[int, int]:
    """The case count and control count the admitted item fixes, read from its stored body, or (0, 0)."""
    for path in sorted((ROOT / ".tasks" / "items").glob("*.md")):
        text = path.read_text(encoding="utf-8")
        if "zenoh_external_check" not in text or ADMISSION not in text:
            continue
        cases = re.search(r'id = "all-cases-observed";.*?intValue = (\d+);', text, re.S)
        controls_ = re.search(r'id = "exactly-\d+-controls";.*?observation = "negativeControlsRefused";.*?intValue = (\d+);', text, re.S)
        if cases and controls_:
            return int(cases.group(1)), int(controls_.group(1))
    return 0, 0


def check_pins(contract: dict) -> int:
    """Refuse a checker whose counts moved from the pins, or from the admitted item while it is active."""
    total = controls_refused(contract)
    _need(total == PINNED_CONTROLS, f"the judges refuse {total} corrupted records, the checker pins {PINNED_CONTROLS}")
    cases, controls_ = item_pins()
    _need(not cases or cases == PINNED_CASES, f"the admitted item fixes {cases} cases, the checker pins {PINNED_CASES}")
    _need(not controls_ or controls_ == PINNED_CONTROLS, f"the admitted item fixes {controls_} controls, the checker pins {PINNED_CONTROLS}")
    return total


# ---------------------------------------------------------------------------- building


GUEST_MAC = bytes.fromhex("525400534c01")
PEER_MAC = bytes.fromhex("525502000202")
GUEST_PORT = 49200


def _ip(source: str, destination: str, protocol: int, body: bytes) -> bytes:
    src, dst = bytes(int(x) for x in source.split(".")), bytes(int(x) for x in destination.split("."))
    header = struct.pack("!BBHHHBBH4s4s", 0x45, 0, 20 + len(body), 0, 0x4000, 64, protocol, 0, src, dst)
    header = header[:10] + http_capture._checksum(header).to_bytes(2, "big") + header[12:]
    return header + body


def _tcp(source: str, destination: str, sport: int, dport: int, seq: int, ack: int, flags: int, payload: bytes = b"") -> bytes:
    body = struct.pack("!HHIIBBHHH", sport, dport, seq & 0xFFFFFFFF, ack & 0xFFFFFFFF, 5 << 4, flags, 65535, 0, 0) + payload
    pseudo = bytes(int(x) for x in source.split(".")) + bytes(int(x) for x in destination.split(".")) + bytes((0, 6)) + len(body).to_bytes(2, "big")
    body = body[:16] + http_capture._checksum(pseudo + body).to_bytes(2, "big") + body[18:]
    return _ip(source, destination, 6, body)


def _udp(source: str, destination: str, sport: int, dport: int, payload: bytes) -> bytes:
    return _ip(source, destination, 17, struct.pack("!HHHH", sport, dport, 8 + len(payload), 0) + payload)


def _icmp() -> bytes:
    body = bytes((8, 0, 0, 0, 0, 1, 0, 1))
    return _ip(GUEST, PEER, 1, body[:2] + http_capture._checksum(body).to_bytes(2, "big") + body[4:])


def _ethernet(packet: bytes, outgoing: bool, ethertype: int = 0x0800) -> bytes:
    pair = (PEER_MAC, GUEST_MAC) if outgoing else (GUEST_MAC, PEER_MAC)
    return pair[0] + pair[1] + ethertype.to_bytes(2, "big") + packet


def _arp() -> bytes:
    return _arp_frame()


def _arp_frame(sender: tuple = (10, 0, 2, 15), target: tuple = (10, 0, 2, 2), mac: bytes = GUEST_MAC, operation: int = 1, ptype: int = 0x0800) -> bytes:
    body = struct.pack("!HHBBH", 1, ptype, 6, 4, operation) + mac + bytes(sender) + bytes(6) + bytes(target)
    destination = GUEST_MAC if mac == PEER_MAC else PEER_MAC
    return destination + mac + (0x0806).to_bytes(2, "big") + body


def pcap(frames: list[bytes]) -> bytes:
    out = struct.pack("<IHHiIII", 0xA1B2C3D4, 2, 4, 0, 0, 65535, 1)
    for i, frame in enumerate(frames):
        out += struct.pack("<IIII", 1 + i // 1000, (i % 1000) * 1000, len(frame), len(frame)) + frame
    return out


def build(scenario: dict) -> bytes:
    """A capture of one connection carrying the scenario's streams.

    `chunk` is the segment size, `reorder` swaps the first two guest segments, `retransmit`
    repeats the first, and `extra` appends frames the scenario wants on the wire.
    """
    gisn, pisn, sport = scenario.get("guest_isn", 1000), scenario.get("peer_isn", 700000), scenario.get("guest_port", GUEST_PORT)
    dport = scenario.get("port", PORT)
    chunk = scenario.get("chunk", 1400)
    guest_stream = b"".join(zenoh_wire.stream(b) for b in scenario["guest"])
    peer_stream = b"".join(zenoh_wire.stream(b) for b in scenario["peer"])

    def pieces(stream: bytes) -> list[tuple[int, bytes]]:
        return [(at, stream[at : at + chunk]) for at in range(0, len(stream), chunk)]

    guest_pieces = pieces(guest_stream)
    if scenario.get("reorder") and len(guest_pieces) > 1:
        guest_pieces[0], guest_pieces[1] = guest_pieces[1], guest_pieces[0]
    if scenario.get("retransmit"):
        guest_pieces.insert(2, guest_pieces[0])
    if "drop_segment" in scenario:
        del guest_pieces[scenario["drop_segment"]]
    frames = [_ethernet(_arp()[14:], True, 0x0806)]
    if not scenario.get("no_syn"):
        frames.append(_ethernet(_tcp(GUEST, PEER, sport, dport, gisn, 0, TCP_SYN), True))
        if not scenario.get("no_synack"):
            frames.append(_ethernet(_tcp(PEER, GUEST, dport, sport, pisn, gisn + 1 + scenario.get("synack_skew", 0), TCP_SYN | TCP_ACK), False))
        frames.append(_ethernet(_tcp(GUEST, PEER, sport, dport, gisn + 1, pisn + 1, TCP_ACK), True))
    for at, payload in guest_pieces:
        frames.append(_ethernet(_tcp(GUEST, PEER, sport, dport, gisn + 1 + at, pisn + 1, TCP_ACK, payload), True))
    for at, payload in pieces(peer_stream):
        frames.append(_ethernet(_tcp(PEER, GUEST, dport, sport, pisn + 1 + at, gisn + 1 + len(guest_stream), TCP_ACK, payload), False))
    frames.extend(scenario.get("extra", []))
    guest_fin = gisn + 1 + len(guest_stream) + scenario.get("fin_skew", 0)
    if scenario.get("guest_fin", True):
        frames.append(_ethernet(_tcp(GUEST, PEER, sport, dport, guest_fin, pisn + 1 + len(peer_stream), TCP_FIN | TCP_ACK), True))
    frames.append(_ethernet(_tcp(PEER, GUEST, dport, sport, pisn + 1 + len(peer_stream), guest_fin + 1, TCP_FIN | TCP_ACK), False))
    frames.append(_ethernet(_tcp(GUEST, PEER, sport, dport, guest_fin + 1, pisn + 2 + len(peer_stream), TCP_ACK), True))
    return pcap(frames)


def synthesize(contract: dict) -> dict:
    """The scenario an honest guest and a stock listener produce, from the host reference."""
    lines = guest_lines(contract)
    sent = judge_guest_lines(lines, contract)
    frames = [bytes.fromhex(sent[i]) for i in range(SAMPLES)]
    zid = bytes(range(1, 9))
    guest = [zenoh_wire.init(zid, 512), zenoh_wire.open_(LEASE_MS, 0, b"slime-cookie-01"), *frames, zenoh_wire.close(0, True)]
    peer = [zenoh_wire.init(bytes(range(0x40, 0x48)), 65535, b"stock-cookie-0001"), zenoh_wire.open_(10000, 4242, ack=True), bytes([ID_KEEP_ALIVE])]
    wire = [(contract["key"], f.hex() and zenoh_wire.decode_push_frame(f)["payload"].hex(), _attachment_hex(f)) for f in frames]
    return {"guest": guest, "peer": peer, "lines": lines, "listen": LISTEN, "remote_peers": 1, "receipt": wire}


def _attachment_hex(batch: bytes) -> str:
    d = zenoh_wire.decode_push_frame(batch)
    return zenoh_wire.attachment(d["sequence"], d["timestamp"], d["gid"]).hex()


def judge_run(pcap_bytes: bytes, lines: list[str], receipt: Path, contract: dict) -> dict:
    """The three witnesses of one real run: the connector's report, the capture, the listener's record."""
    reported = judge_guest_lines(lines, contract)
    result = judge_capture(pcap_bytes, reported, contract)
    listen, remote_peers, samples = read_receipt(receipt)
    result["receipt"] = judge_receipt(listen, remote_peers, samples, result["samples"])
    return result


def judge(scenario: dict, contract: dict) -> dict:
    """Every judge, over one scenario: its report, its capture and the listener's receipt."""
    reported = judge_guest_lines(scenario["lines"], contract)
    result = judge_capture(build(scenario), reported, contract)
    result["receipt"] = judge_receipt(scenario["listen"], scenario["remote_peers"], scenario["receipt"], result["samples"])
    return result


# ---------------------------------------------------------------------------- controls


def _replace_sent(index: int, change: Callable[[bytes], bytes], both: bool) -> Callable[[dict], dict]:
    def run(s: dict) -> dict:
        at = 2 + index
        original = s["guest"][at]
        s["guest"][at] = change(original)
        if both:
            s["lines"] = [line.replace(original.hex(), s["guest"][at].hex()) for line in s["lines"]]
        return s

    return run


def mutations() -> list[tuple[str, Callable[[dict], dict]]]:
    def edit(change: Callable[[dict], None]) -> Callable[[dict], dict]:
        def run(s: dict) -> dict:
            change(s)
            return s

        return run

    def extra(*frames: bytes) -> Callable[[dict], dict]:
        return edit(lambda s: s.setdefault("extra", []).extend(_ethernet(f, True) for f in frames))

    def raw(*frames: bytes) -> Callable[[dict], dict]:
        return edit(lambda s: s.setdefault("extra", []).extend(frames))

    def flip(batch: bytes) -> bytes:
        return batch[:-1] + bytes([batch[-1] ^ 1])

    def wrong_sn(batch: bytes) -> bytes:
        return zenoh_wire.frame(zenoh_wire.read_leb128(batch, 1)[0] + 1, batch[1 + len(zenoh_wire.leb128(zenoh_wire.read_leb128(batch, 1)[0])) :])

    def pad(batch: bytes) -> bytes:
        return batch + b"\x00"

    def other_syn(port: int, address: str = PEER) -> bytes:
        return _tcp(GUEST, address, GUEST_PORT + 1, port, 5000, 0, TCP_SYN)

    def second_connection(s: dict) -> dict:
        s.setdefault("extra", []).append(_ethernet(other_syn(PORT), True))
        return s

    def drop_frame(s: dict) -> dict:
        s["guest"].pop(2 + SAMPLES - 1)
        return s

    def fifth_frame(s: dict) -> dict:
        s["guest"].insert(2 + SAMPLES, s["guest"][2 + SAMPLES - 1])
        return s

    def put_bytes(change: Callable[[list], None]) -> Callable[[dict], dict]:
        return edit(lambda s: change(s["guest"]))

    return [
        ("no connection at all", edit(lambda s: s.update(no_syn=True))),
        ("a second connection from another port", second_connection),
        ("a SYN to another port", extra(other_syn(PORT + 1))),
        ("a SYN to port 17446, one under", extra(other_syn(PORT - 1))),
        ("the only connection goes to port 17446, one under", edit(lambda s: s.update(port=PORT - 1))),
        ("a SYN to another address", extra(other_syn(PORT, "10.0.2.3"))),
        ("the connection goes to another port", edit(lambda s: s.update(port=PORT + 1))),
        ("a DNS query", extra(_udp(GUEST, "10.0.2.3", 49300, 53, b"\x00\x01\x01\x00\x00\x01"))),
        ("a scouting datagram", extra(_udp(GUEST, "224.0.0.224", 49301, 7446, b"\x01"))),
        ("an ICMP echo", extra(_icmp())),
        ("an ARP request for another address", raw(_arp_frame(target=(10, 0, 2, 99)))),
        ("an ARP request for the next address up, 10.0.2.3", raw(_arp_frame(target=(10, 0, 2, 3)))),
        ("an ARP request claiming another sender address", raw(_arp_frame(sender=(10, 0, 2, 16)))),
        ("an ARP request from another hardware address", raw(_arp_frame(mac=bytes.fromhex("525400000001")))),
        ("an ARP operation that is neither request nor reply", raw(_arp_frame(operation=3))),
        ("an ARP request that is not Ethernet and IPv4", raw(_arp_frame(ptype=0x86DD))),
        ("a reset after the data", extra(_tcp(GUEST, PEER, GUEST_PORT, PORT, 9999, 0, TCP_RST))),
        ("a connection the peer never accepts", edit(lambda s: s.update(no_synack=True))),
        ("a SYN-ACK acknowledging one number too many", edit(lambda s: s.update(synack_skew=1))),
        ("a SYN-ACK acknowledging one number too few", edit(lambda s: s.update(synack_skew=-1))),
        ("a stream that never closes", edit(lambda s: s.update(guest_fin=False))),
        ("a FIN one byte past the data", edit(lambda s: s.update(fin_skew=1))),
        ("a FIN one byte short of the data", edit(lambda s: s.update(fin_skew=-1))),
        ("a missing segment", edit(lambda s: s.update(chunk=100, drop_segment=1))),
        ("a missing first segment", edit(lambda s: s.update(chunk=100, drop_segment=0))),
        ("a sample batch changed on the wire only", _replace_sent(1, flip, False)),
        ("a sample batch changed on the wire and in the report", _replace_sent(1, flip, True)),
        ("a sample batch one byte longer", _replace_sent(2, pad, True)),
        ("a frame sequence number one over", _replace_sent(0, wrong_sn, True)),
        ("three frames on the wire", drop_frame),
        ("five frames on the wire", fifth_frame),
        ("frames swapped on the wire", put_bytes(lambda g: g.__setitem__(slice(2, 4), [g[3], g[2]]))),
        ("no INIT_SYN first", put_bytes(lambda g: g.__setitem__(slice(0, 2), [g[1], g[0]]))),
        ("a keep-alive of two bytes", put_bytes(lambda g: g.insert(2, b"\x04\x00"))),
        ("a batch after the close", put_bytes(lambda g: g.append(bytes([ID_KEEP_ALIVE])))),
        ("a lease of 1999 ms", put_bytes(lambda g: g.__setitem__(1, zenoh_wire.open_(1999, 0, b"slime-cookie-01")))),
        ("a lease of 2001 ms", put_bytes(lambda g: g.__setitem__(1, zenoh_wire.open_(2001, 0, b"slime-cookie-01")))),
        ("a batch the exam does not name", put_bytes(lambda g: g.insert(2, bytes([zenoh_wire.ID_DECLARE])))),
        ("a peer that sends no INIT_ACK", edit(lambda s: s.update(peer=[bytes([ID_KEEP_ALIVE])]))),
        ("a peer batch the exam does not name", edit(lambda s: s["peer"].append(bytes([0x0D])))),
        ("a listener on another address", edit(lambda s: s.update(listen="tcp/0.0.0.0:17447"))),
        ("a listener on another port", edit(lambda s: s.update(listen="tcp/127.0.0.1:17448"))),
        ("a listener that saw no remote peer", edit(lambda s: s.update(remote_peers=0))),
        ("a listener that saw nine remote peers", edit(lambda s: s.update(remote_peers=9))),
        ("a receipt of three samples", edit(lambda s: s["receipt"].pop())),
        ("a receipt of five samples", edit(lambda s: s["receipt"].append(s["receipt"][-1]))),
        ("a receipt with another payload", edit(lambda s: s["receipt"].__setitem__(1, (s["receipt"][1][0], "00" * 12, s["receipt"][1][2])))),
        ("a receipt with another attachment", edit(lambda s: s["receipt"].__setitem__(2, (s["receipt"][2][0], s["receipt"][2][1], "00" * 33)))),
        ("a receipt with another key", edit(lambda s: s["receipt"].__setitem__(0, ("other/key", s["receipt"][0][1], s["receipt"][0][2])))),
        ("a receipt in another order", edit(lambda s: s["receipt"].reverse())),
        ("a report that drops a sent batch", edit(lambda s: s.update(lines=[x for x in s["lines"] if "wire sent sample=3 " not in x]))),
        ("a report that duplicates a sent batch", edit(lambda s: s["lines"].insert(-1, next(x for x in s["lines"] if "wire sent sample=0 " in x)))),
        ("a report without the scouting denial", edit(lambda s: s.update(lines=[x for x in s["lines"] if "scouting" not in x]))),
        ("a report with sample 1 before the session opens", edit(lambda s: s["lines"].insert(3, s["lines"].pop(next(n for n, x in enumerate(s["lines"]) if " wire sent sample=1 " in x))))),
        ("a report with sample 1 between the session and the match", edit(lambda s: s["lines"].insert(4, s["lines"].pop(next(n for n, x in enumerate(s["lines"]) if " wire sent sample=1 " in x))))),
        ("a report with samples 2 and 3 swapped", edit(lambda s: s["lines"].insert(8, s["lines"].pop(7)))),
        ("a report with sample 0 last", edit(lambda s: s["lines"].insert(8, s["lines"].pop(5)))),
        ("a report with the listen denial twice", edit(lambda s: s["lines"].insert(0, f"[{PUBLISHER}] denial class=listen refused=1"))),
        ("a report with a denial the exam does not name", edit(lambda s: s["lines"].insert(0, f"[{PUBLISHER}] denial class=invented refused=1"))),
        ("a report from the subscriber", edit(lambda s: s["lines"].insert(0, f"[{SUBSCRIBER}] session open role=listener initial_sn=0 lease_ms=2000"))),
        ("a report whose session closes before the last sample", edit(lambda s: s["lines"].insert(-3, s["lines"].pop()))),
        ("a report whose match precedes the session", edit(lambda s: s["lines"].insert(3, s["lines"].pop(4)))),
    ]


def _frame_starts(data: bytes) -> list[int]:
    """The file offset of each frame in a capture this module built."""
    starts, at = [], 24
    while at < len(data):
        length = int.from_bytes(data[at + 8 : at + 12], "little")
        starts.append(at + 16)
        at += 16 + length
    return starts


def _flip(data: bytes, at: int) -> bytes:
    return data[:at] + bytes([data[at] ^ 1]) + data[at + 1 :]


def byte_mutations() -> list[tuple[str, Callable[[bytes], bytes]]]:
    def first_data_frame(data: bytes) -> int:
        return next(s for s in _frame_starts(data) if int.from_bytes(data[s + 16 : s + 18], "big") > 40 and data[s + 14 + 9] == 6)

    return [
        ("an empty capture", lambda data: b""),
        ("a capture that is not a pcap", lambda data: b"\x00" * 64),
        ("a capture cut inside a record", lambda data: data[:-5]),
        ("a flipped IPv4 header byte under a stale header checksum", lambda data: _flip(data, _frame_starts(data)[1] + 14 + 16)),
        ("a flipped TCP payload byte under a stale TCP checksum", lambda data: _flip(data, first_data_frame(data) + 14 + 20 + 20)),
    ]


def honest_variants() -> list[tuple[str, Callable[[dict], dict]]]:
    def update(**changes: object) -> Callable[[dict], dict]:
        def run(s: dict) -> dict:
            s.update(changes)
            return s

        return run

    return [
        ("segments of 100 bytes", update(chunk=100)),
        ("segments of 1 byte", update(chunk=1)),
        ("the first two guest segments reordered", update(chunk=100, reorder=True)),
        ("a retransmitted segment", update(chunk=100, retransmit=True)),
        ("a guest sequence number about to wrap", update(guest_isn=0xFFFFFF00)),
        ("a peer sequence number about to wrap", update(peer_isn=0xFFFFFFF0)),
        ("keep-alives from the guest", lambda s: {**s, "guest": s["guest"][:2] + [bytes([ID_KEEP_ALIVE])] * 3 + s["guest"][2:]}),
        ("a guest that never sends CLOSE", lambda s: {**s, "guest": s["guest"][:-1]}),
        ("an ARP reply from the peer", lambda s: {**s, "extra": [*s.get("extra", []), _arp_frame(sender=(10, 0, 2, 2), target=(10, 0, 2, 15), mac=PEER_MAC, operation=2)]}),
    ]


def controls(contract: dict) -> tuple[int, int, int]:
    """Judge the honest scenario and its variants, then refuse every mutation.

    Returns (samples judged, honest variants accepted, mutations refused).
    """
    good = synthesize(contract)
    first = judge(copy.deepcopy(good), contract)
    _need(first["frames"] == SAMPLES and first["receipt"] == SAMPLES, "the honest scenario was not judged whole")
    accepted = 0
    for label, change in honest_variants():
        try:
            judge(change(copy.deepcopy(good)), contract)
        except (CaptureError, StopIteration) as error:
            raise CaptureError(f"control: the judge refused an honest capture with {label}: {error}") from error
        accepted += 1
    refused = 0
    for label, mutate in mutations():
        try:
            judge(mutate(copy.deepcopy(good)), contract)
        except (CaptureError, StopIteration):
            refused += 1
            continue
        raise CaptureError(f"control: the judge accepted {label}")
    honest = build(good)
    reported = judge_guest_lines(good["lines"], contract)
    for label, damage in byte_mutations():
        try:
            judge_capture(damage(honest), reported, contract)
        except CaptureError:
            refused += 1
            continue
        raise CaptureError(f"control: the judge accepted {label}")
    return first["frames"], accepted, refused
