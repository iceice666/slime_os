"""Judge the Zenoh service core over a link from what the Rust test prints.

The core of `zenoh_service_exam` stands on a bare session. This exam runs the same core over a link: the
single send slot, the notice queue and a byte stream that can fail. The Rust test under the module path
`zenoh_service_link::` prints one `[zenoh-exam] servicelink ...` line per thing it did. This module owns what
those lines must be, and nothing here reads the Rust:

* the limits and grant rows, which are the honest record `zenoh_grants_exam` shows the contract accepts;
* that a declaration or a publication queues through the link and counts only once the send slot flushed,
  and that a second one finds the slot taken and is refused as `busy`;
* the frames the link wrote, which the host reference `zenoh_wire` writes from the same keys, sequence
  numbers and payloads;
* what each holder is delivered, with the id of the subscription that matched;
* a burst the service does not drain, which fills the notice queue and waits in the stream, and whose overflow the ring counts as loss;
* a byte stream that accepts a frame and then resets, which leaves no copy in a local ring.

The last case documents a limit and does not hide it. A frame handed to the byte stream just before the stream
resets may still reach the peer, so a holder can be told that a publication failed which the peer received.
Profile 0 has no acknowledgement, so the core can know that the link took the bytes and no more.

Like the other judges it is trusted only after it refuses a mutation of an honest transcript.
"""

from __future__ import annotations

import re

import zenoh_grants_exam as grants
import zenoh_wire
from harness import ROOT

K = ("k1", "k2", "k3", "k4")
GID = bytes([0xE0] * 16)
# The service is the listener the exam opens, so its first FRAME carries its initial sequence number.
SERVICE_SN = 7000
NOTICES = 8
LIMITS_LINE = f"servicelink limits 3 4 12 3 {NOTICES}"
SIZE_NAMES = ("slot", "row", "holder", "fixed")
# What the core stores beyond its payload bytes. The link-facing core also holds one publication that waits
# for the flush, so the fixed part is larger than the session-facing core's. Each stays within the matching
# overhead in the grants contract.
SIZES = {"slot": 28, "row": 40, "holder": 8, "fixed": 168}
# The burst: twenty Puts from a peer that the service does not drain. The notice queue holds `NOTICES` of them
# and the stream holds the rest. The service then drains all twenty, and the ring keeps the newest four and
# counts the other sixteen as lost.
BURST_SENT = 20
BURST_RING = 4
BURST_LOST = 16


def _payload(sequence: int) -> bytes:
    return bytes([sequence, 2, 3])


def _put_frame(sn: int, key: int, sequence: int) -> str:
    attachment = zenoh_wire.attachment(sequence, 1000 + sequence, GID)
    return zenoh_wire.frame(sn, zenoh_wire.push_put(grants.KEYS[key], attachment, _payload(sequence), True)).hex()


def _declare_frame(sn: int, subscription: int, key: int) -> str:
    return zenoh_wire.frame(sn, zenoh_wire.declare_subscriber(subscription, grants.KEYS[key], True)).hex()


def _delivered(phase: str, holder: int, subscription: int, sequence: int) -> str:
    return f"servicelink delivered {phase} {grants.HOLDERS[holder]} {subscription} {sequence} {_payload(sequence).hex()}"


def good_lines() -> list[str]:
    """The honest transcript, in the order the Rust test prints it."""
    out = [LIMITS_LINE]
    for holder, key, publish, subscribe in grants.ROWS:
        out.append(f"servicelink grant {grants.HOLDERS[holder]} {K[key]} {'p' if publish else '-'}{'s' if subscribe else '-'}")
    out += [f"servicelink size {name} {SIZES[name]}" for name in SIZE_NAMES]
    # One: a declaration queues through the link, a second finds the send slot taken, and the subscription
    # counts as declared only after the slot flushed.
    out += [
        "servicelink declare 1 queued 0",
        "servicelink refused declare-while-one-waits busy",
        "servicelink refused declare-after-the-flush-before-settle busy",
        "servicelink declare 1 declared 1",
        f"servicelink frame declare-1 {_declare_frame(SERVICE_SN, 1, 0)}",
        "servicelink declare 2 queued 1",
        "servicelink refused declare-while-one-waits busy",
        "servicelink refused declare-after-the-flush-before-settle busy",
        "servicelink declare 2 declared 2",
        f"servicelink frame declare-2 {_declare_frame(SERVICE_SN + 1, 2, 1)}",
        "servicelink ok declaration-goes-one-frame-at-a-time",
    ]
    # Two and three: a publication reaches the local rings at once when the peer did not declare the key, and
    # only after the flush when it did. A second publication finds the send slot taken.
    out += [
        "servicelink published a k1 not-subscribed 2",
        _delivered("before-the-peer-declares", 0, 1, 99),
        _delivered("before-the-peer-declares", 1, 1, 99),
        "servicelink published a k1 queued",
        "servicelink held before-the-flush a 0 0",
        "servicelink held before-the-flush b 0 0",
        "servicelink refused publish-while-one-waits busy",
        "servicelink refused publish-after-the-flush-before-settle busy",
        "servicelink settled publish-a 2",
        f"servicelink frame publish-a {_put_frame(SERVICE_SN + 2, 0, 100)}",
        "servicelink published b k3 queued",
        "servicelink settled publish-b 0",
        f"servicelink frame publish-b {_put_frame(SERVICE_SN + 3, 2, 101)}",
        _delivered("after-the-flush", 0, 1, 100),
        _delivered("after-the-flush", 1, 1, 100),
        "servicelink ok a-publication-reaches-the-rings-after-the-flush",
    ]
    # Four: a refused publication reaches no ring and queues nothing.
    out += [
        "servicelink refused publish-without-a-publish-right not-granted",
        "servicelink refused publish-to-a-key-with-no-row not-granted",
        "servicelink refused publish-by-an-unknown-holder unknown-holder",
        "servicelink refused publish-payload-over-the-sample-size too-large",
        "servicelink ok a-refused-publication-reaches-no-one",
    ]
    # Five: a peer's sample is routed by the subscription that matched, so a holder that subscribes to two
    # keys tells them apart.
    out += [
        "servicelink routed 2",
        _delivered("from-the-peer", 0, 1, 10),
        _delivered("from-the-peer", 0, 2, 11),
        _delivered("from-the-peer", 1, 1, 10),
        _delivered("from-the-peer", 2, 2, 11),
        "servicelink ok a-sample-from-the-peer-is-routed-by-its-subscription",
    ]
    # Six: a burst the service does not drain.
    out += [
        f"servicelink burst {BURST_SENT} {NOTICES}",
        f"servicelink held after-the-burst a {BURST_RING} {BURST_LOST}",
        "servicelink ok a-burst-waits-in-the-stream-and-the-ring-counts-the-loss",
    ]
    # Seven: a stream that accepts the frame and then resets.
    out += [
        "servicelink published a k1 queued-on-a-stream-that-will-reset",
        "servicelink pump reset",
        "servicelink refused settle-after-reset link-closed",
        "servicelink held after-the-reset a 0 0",
        "servicelink held after-the-reset b 0 0",
        "servicelink peer-received after-the-reset 1",
        "servicelink ok a-frame-handed-over-before-a-reset-reaches-no-local-ring",
    ]
    return out


class ServiceLinkError(Exception):
    pass


def _need(condition: bool, message: str) -> None:
    if not condition:
        raise ServiceLinkError(message)


SHAPES = {
    "limits": 7,
    "grant": 5,
    "size": 4,
    "declare": 5,
    "frame": 4,
    "ok": 3,
    "published": (5, 6),
    "held": 6,
    "refused": 4,
    "settled": 4,
    "delivered": 7,
    "routed": 3,
    "burst": 4,
    "pump": 3,
    "peer-received": 4,
}


def size_bounds() -> dict[str, int]:
    """The overheads the grants contract states, which the sizes the core prints may not exceed."""
    constants = grants.schema_constants()
    return {
        "slot": constants["SLOT_OVERHEAD_BYTES"],
        "row": constants["ROW_OVERHEAD_BYTES"],
        "holder": constants["HOLDER_OVERHEAD_BYTES"],
        "fixed": constants["SERVICE_FIXED_BYTES"],
    }


def _check_sizes(lines: list[str]) -> None:
    """The four size lines follow the grant lines, name the four stored parts in order, and each value is
    a whole number of bytes no larger than the grants contract's overhead for that part."""
    first = 1 + len(grants.ROWS)
    at = [index for index, line in enumerate(lines) if line.split()[1] == "size"]
    _need(at == list(range(first, first + len(SIZE_NAMES))), f"servicelink: the {len(SIZE_NAMES)} size lines must follow the grant lines, found them at {at}")
    bounds = size_bounds()
    for name, line in zip(SIZE_NAMES, lines[first : first + len(SIZE_NAMES)], strict=True):
        fields = line.split()
        _need(fields[2] == name, f"servicelink: size line for {fields[2]!r} where {name!r} is required: {line}")
        _need(fields[3].isdecimal(), f"servicelink: the {name} size is not a whole number of bytes: {line}")
        value = int(fields[3])
        _need(1 <= value <= bounds[name], f"servicelink: a {name} of {value} bytes exceeds the {bounds[name]} the grants contract allows")


def _shape_ok(fields: list[str]) -> bool:
    shape = SHAPES.get(fields[1])
    if shape is None:
        return False
    return len(fields) in shape if isinstance(shape, tuple) else len(fields) == shape


def judge(lines: list[str]) -> tuple[int, int]:
    """Check a transcript against the exam; returns (cases, refusals). A case is anything printed but a
    declaration of the core's limits and rows; a refusal is a refused line."""
    for line in lines:
        fields = line.split()
        _need(fields[:1] == ["servicelink"], f"servicelink: a record that is not a servicelink exam line: {line!r}")
        _need(len(fields) > 1 and fields[1] in SHAPES, f"servicelink: unrecognised line: {line}")
        _need(_shape_ok(fields), f"servicelink: a {fields[1]} line has {len(fields)} fields: {line}")
    _check_sizes(lines)
    wanted = good_lines()
    # A size is judged against a bound and not pinned: a core with a smaller layout is still honest.
    lines = [wanted[index] if line.split()[1] == "size" else line for index, line in enumerate(lines)]
    for index, (seen, want) in enumerate(zip(lines, wanted, strict=False)):
        _need(seen == want, f"servicelink line {index + 1} is {seen!r}, the exam requires {want!r}")
    if len(lines) < len(wanted):
        raise ServiceLinkError(f"servicelink: the transcript stops after {len(lines)} of {len(wanted)} lines; the next is {wanted[len(lines)]!r}")
    if len(lines) > len(wanted):
        raise ServiceLinkError(f"servicelink: {len(lines) - len(wanted)} lines beyond the exam, the first {lines[len(wanted)]!r}")
    refusals = sum(1 for line in lines if line.startswith("servicelink refused "))
    declarations = sum(1 for line in lines if line.startswith(("servicelink limits ", "servicelink grant ")))
    return len(lines) - declarations - refusals, refusals


def transcript_mutations() -> list[tuple[str, list[str]]]:
    good = good_lines()
    out: list[tuple[str, list[str]]] = [
        ("an empty transcript", []),
        ("a line that is not a servicelink exam line", [*good, "service ok static-declaration-at-open"]),
        ("an unrecognised kind of line", [*good, "servicelink dropped a k1"]),
        ("a line with an extra field", [*good[:-1], good[-1] + " extra"]),
        ("a line repeated", [*good, good[-1]]),
        ("the first two scenarios swapped", _swap(good, "servicelink ok declaration-goes-one-frame-at-a-time", "servicelink ok a-publication-reaches-the-rings-after-the-flush")),
    ]
    for index, line in enumerate(good):
        out.append((f"line {index + 1} missing ({line[12:64]})", good[:index] + good[index + 1 :]))
    for index, line in enumerate(good):
        out.append((f"line {index + 1} with its last field changed ({line[12:64]})", [*good[:index], _change_last(line), *good[index + 1 :]]))
    return out


def _change_last(line: str) -> str:
    head, last = line.rsplit(" ", 1)
    if re.fullmatch(r"[0-9a-f]+", last) and len(last) > 4:
        return f"{head} {last[:-1]}{'0' if last[-1] != '0' else '1'}"
    return f"{head} {last}x"


def _swap(good: list[str], a: str, b: str) -> list[str]:
    out = list(good)
    i, j = out.index(a), out.index(b)
    out[i], out[j] = out[j], out[i]
    return out


def transcript_controls() -> int:
    good = good_lines()
    judge(good)
    refused = 0
    for label, lines in transcript_mutations():
        if lines == good:
            raise ServiceLinkError(f"control: {label} did not change the transcript")
        try:
            judge(lines)
        except ServiceLinkError:
            refused += 1
            continue
        raise ServiceLinkError(f"control: the servicelink judge accepted {label}")
    return refused


def declaration_controls() -> int:
    """The core is declared with the record the grants contract accepts, and the limits and rows the exam
    prints are that record's."""
    _need(grants.contract_accepts(grants.record()), "the record the service core is declared with is refused by the grants contract")
    printed = good_lines()
    holders, slots, sample, _key, local = grants.LIMITS
    _need(printed[0] == f"servicelink limits {holders} {slots} {sample} {local} {NOTICES}", f"the exam prints {printed[0]!r}, the grants record declares {grants.LIMITS}")
    rows = [f"servicelink grant {grants.HOLDERS[h]} {K[k]} {'p' if p else '-'}{'s' if s else '-'}" for h, k, p, s in grants.ROWS]
    _need(printed[1 : 1 + len(rows)] == rows, "the rows the exam prints are not the rows the grants record declares")
    return 2


def size_controls() -> int:
    """A size above the contract's overhead is refused for that reason, for each of the four parts, and so
    are the size lines out of order."""
    good = good_lines()
    bounds = size_bounds()
    for name in SIZE_NAMES:
        _need(SIZES[name] <= bounds[name], f"control: the exam's honest {name} size {SIZES[name]} exceeds the contract's {bounds[name]}")
    refused = 0
    first = 1 + len(grants.ROWS)
    for offset, name in enumerate(SIZE_NAMES):
        lines = list(good)
        lines[first + offset] = f"servicelink size {name} {bounds[name] + 1}"
        try:
            judge(lines)
        except ServiceLinkError as error:
            _need("exceeds" in str(error), f"control: a {name} one byte over its bound was refused for another reason: {error}")
            refused += 1
            continue
        raise ServiceLinkError(f"control: the servicelink judge accepted a {name} size one byte over the contract's {bounds[name]}")
    swapped = list(good)
    swapped[first], swapped[first + 1] = swapped[first + 1], swapped[first]
    try:
        judge(swapped)
    except ServiceLinkError:
        refused += 1
    else:
        raise ServiceLinkError("control: the servicelink judge accepted the slot and row size lines swapped")
    return refused


def all_controls() -> int:
    return transcript_controls() + declaration_controls() + size_controls()


# What the admitted item fixes, held here as well so the gate stays correct after the item is completed
# and retired to .tasks/terminal. While the item is active its stored body is compared with these; once
# it is gone these still bind. Both counts are exact.
PINNED_CASES = 52
PINNED_CONTROLS = 129
ADMISSION = "slime-zenoh-service-link-"


def item_pins() -> tuple[int, int]:
    """The case count and the control count the admitted item fixes, read from its stored body, or (0, 0)."""
    for path in sorted((ROOT / ".tasks" / "items").glob("*.md")):
        text = path.read_text(encoding="utf-8")
        if "zenoh_service_link_check" not in text or ADMISSION not in text or re.search(r"^state: cancelled$", text, re.M):
            continue
        found = re.search(r'id = "all-cases-observed";.*?intValue = (\d+);', text, re.S)
        controls = re.search(r'id = "exactly-\d+-controls";.*?observation = "negativeControlsRefused";.*?intValue = (\d+);', text, re.S)
        if found and controls:
            return int(found.group(1)), int(controls.group(1))
    return 0, 0
