"""Judge the Zenoh service core from what the Rust tests print.

The service owns one Zenoh session and serves local holders. A holder's authority is a row of
`contracts/zenoh-service-grants/v1`; the core is declared with those rows and the limits beside them.
The Rust tests under `zenoh_service::` print one `[zenoh-exam] service ...` line per thing they did.
This module owns what those lines must be, and nothing here reads the Rust:

* the limits and grant rows the core is declared with, which are the honest record
  `zenoh_grants_exam` shows the contract accepts;
* the keys declared to the peer at open, and the publications written to it, as frames the host
  reference `zenoh_wire` writes from the same keys, sequence numbers and payloads;
* what each holder is delivered, in order, from a publication and from the peer;
* what a full ring does: it keeps the newest samples, counts what it dropped and says so once;
* every refusal, by name and class.

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
LIMITS_LINE = "service limits 3 4 12 3"
# What the core stores beyond its payload bytes, in bytes: a delivery slot, a grant row with its key
# references, a holder's ring cursors and loss count, and the core's own counters. The core prints these
# from `size_of`. Each is at most the matching overhead in the grants contract, which is what the declared
# stack is computed from.
SIZE_NAMES = ("slot", "row", "holder", "fixed")
SIZES = {"slot": 28, "row": 48, "holder": 24, "fixed": 8}


def _payload(sequence: int) -> bytes:
    return bytes([sequence, 2, 3])


def _put_frame(sn: int, key: int, sequence: int) -> str:
    attachment = zenoh_wire.attachment(sequence, 1000 + sequence, GID)
    return zenoh_wire.frame(sn, zenoh_wire.push_put(grants.KEYS[key], attachment, _payload(sequence), True)).hex()


def _declare_frame(sn: int, subscription: int, key: int) -> str:
    return zenoh_wire.frame(sn, zenoh_wire.declare_subscriber(subscription, grants.KEYS[key], True)).hex()


def _delivered(phase: str, holder: int, sequence: int) -> str:
    return f"service delivered {phase} {grants.HOLDERS[holder]} {sequence} {_payload(sequence).hex()}"


REFUSALS = (
    ("publish-without-a-publish-right", "not-granted"),
    ("publish-to-a-key-with-no-row", "not-granted"),
    ("publish-by-an-unknown-holder", "unknown-holder"),
    ("publish-payload-over-the-sample-size", "too-large"),
)
GRANT_REFUSALS = (
    ("grants-name-an-unknown-holder", "invalid-grants"),
    ("grants-repeat-a-holder-and-key", "invalid-grants"),
    ("grants-give-a-row-no-right", "invalid-grants"),
    ("grants-subscribe-more-keys-than-the-session-holds", "invalid-grants"),
    ("grants-with-no-rows", "invalid-grants"),
    ("grants-slot-size-differs-from-the-session", "invalid-grants"),
)


def good_lines() -> list[str]:
    """The honest transcript, in the order the Rust test prints it."""
    out = [LIMITS_LINE]
    for holder, key, publish, subscribe in grants.ROWS:
        out.append(f"service grant {grants.HOLDERS[holder]} {K[key]} {'p' if publish else '-'}{'s' if subscribe else '-'}")
    out += [f"service size {name} {SIZES[name]}" for name in SIZE_NAMES]
    # Two distinct subscribe keys of the three the rows name are declared by the peer's counterpart; the
    # service declares one subscription for each distinct key a holder subscribes to: k1 and k2.
    out += [
        f"service frame declare-1 {_declare_frame(SERVICE_SN, 1, 0)}",
        f"service frame declare-2 {_declare_frame(SERVICE_SN + 1, 2, 1)}",
        "service ok static-declaration-at-open",
        "service published a k1 2 not-subscribed",
        _delivered("before-the-peer-declares", 0, 99),
        _delivered("before-the-peer-declares", 1, 99),
        "service published a k1 2 sent",
        f"service frame publish-a {_put_frame(SERVICE_SN + 2, 0, 100)}",
        "service published b k3 0 sent",
        f"service frame publish-b {_put_frame(SERVICE_SN + 3, 2, 101)}",
        _delivered("after-the-peer-declares", 0, 100),
        _delivered("after-the-peer-declares", 1, 100),
        "service ok publication-goes-local-and-to-a-subscribed-peer",
    ]
    out += [f"service refused {name} {cls}" for name, cls in REFUSALS]
    out.append("service ok a-refused-publication-reaches-no-one")
    out += [_delivered("from-the-peer", 0, 10), _delivered("from-the-peer", 0, 11), _delivered("from-the-peer", 1, 10), _delivered("from-the-peer", 2, 11)]
    out += ["service refused peer-put-to-a-key-nobody-subscribed undeclared-key", "service ok delivery-reaches-every-subscribed-holder"]
    out += [_delivered("while-b-is-full", 0, n) for n in range(20, 26)]
    out += ["service held b-full 4 2", "service lost b-catches-up b 2"]
    out += [_delivered("b-catches-up", 1, n) for n in range(22, 26)]
    out += ["service held b-drained 0 0", "service ok a-slow-holder-loses-its-oldest-and-is-told-once"]
    out.append("service refused take-by-an-unknown-holder unknown-holder")
    out += [f"service refused {name} {cls}" for name, cls in GRANT_REFUSALS]
    out.append("service ok the-core-refuses-grants-it-cannot-serve")
    return out


class ServiceError(Exception):
    pass


def _need(condition: bool, message: str) -> None:
    if not condition:
        raise ServiceError(message)


SHAPES = {"limits": 6, "grant": 5, "size": 4, "frame": 4, "ok": 3, "published": 6, "delivered": 6, "refused": 4, "held": 5, "lost": 5}


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
    _need(at == list(range(first, first + len(SIZE_NAMES))), f"service: the {len(SIZE_NAMES)} size lines must follow the grant lines, found them at {at}")
    bounds = size_bounds()
    for name, line in zip(SIZE_NAMES, lines[first : first + len(SIZE_NAMES)], strict=True):
        fields = line.split()
        _need(fields[2] == name, f"service: size line for {fields[2]!r} where {name!r} is required: {line}")
        _need(fields[3].isdecimal(), f"service: the {name} size is not a whole number of bytes: {line}")
        value = int(fields[3])
        _need(1 <= value <= bounds[name], f"service: a {name} of {value} bytes exceeds the {bounds[name]} the grants contract allows")


def judge(lines: list[str]) -> tuple[int, int]:
    """Check a transcript against the exam; returns (cases, refusals). A case is anything printed but a
    declaration of the core's limits and rows; a refusal is a refused line."""
    for line in lines:
        fields = line.split()
        _need(fields[:1] == ["service"], f"service: a record that is not a service exam line: {line!r}")
        _need(len(fields) > 1 and fields[1] in SHAPES, f"service: unrecognised line: {line}")
        _need(len(fields) == SHAPES[fields[1]], f"service: a {fields[1]} line has {len(fields)} fields, expected {SHAPES[fields[1]]}: {line}")
    _check_sizes(lines)
    wanted = good_lines()
    # A size is judged against a bound and not pinned: a core with a smaller layout is still honest.
    lines = [wanted[index] if line.split()[1] == "size" else line for index, line in enumerate(lines)]
    for index, (seen, want) in enumerate(zip(lines, wanted, strict=False)):
        _need(seen == want, f"service line {index + 1} is {seen!r}, the exam requires {want!r}")
    if len(lines) < len(wanted):
        raise ServiceError(f"service: the transcript stops after {len(lines)} of {len(wanted)} lines; the next is {wanted[len(lines)]!r}")
    if len(lines) > len(wanted):
        raise ServiceError(f"service: {len(lines) - len(wanted)} lines beyond the exam, the first {lines[len(wanted)]!r}")
    refusals = sum(1 for line in lines if line.startswith("service refused "))
    declarations = sum(1 for line in lines if line.startswith(("service limits ", "service grant ")))
    return len(lines) - declarations - refusals, refusals


def transcript_mutations() -> list[tuple[str, list[str]]]:
    good = good_lines()
    out: list[tuple[str, list[str]]] = [
        ("an empty transcript", []),
        ("a line that is not a service exam line", [*good, "subscribers ok declare-several-own-subscribers"]),
        ("an unrecognised kind of line", [*good, "service dropped a k1"]),
        ("a line with an extra field", [*good[:-1], good[-1] + " extra"]),
        ("a line repeated", [*good, good[-1]]),
        ("the first two scenarios swapped", _swap(good, "service ok static-declaration-at-open", "service ok publication-goes-local-and-to-a-subscribed-peer")),
    ]
    for index, line in enumerate(good):
        out.append((f"line {index + 1} missing ({line[8:60]})", good[:index] + good[index + 1 :]))
    for index, line in enumerate(good):
        out.append((f"line {index + 1} with its last field changed ({line[8:60]})", [*good[:index], _change_last(line), *good[index + 1 :]]))
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
            raise ServiceError(f"control: {label} did not change the transcript")
        try:
            judge(lines)
        except ServiceError:
            refused += 1
            continue
        raise ServiceError(f"control: the service judge accepted {label}")
    return refused


def declaration_controls() -> int:
    """The core is declared with the record the grants contract accepts, and with nothing else: the limits
    and rows the exam prints are the grants exam's own, and that record is valid. A record that drifts from
    what the exam prints, or a stack too small for it, is refused by the contract."""
    _need(grants.contract_accepts(grants.record()), "the record the service core is declared with is refused by the grants contract")
    printed = good_lines()
    holders, slots, sample, _key, local = grants.LIMITS
    _need(printed[0] == f"service limits {holders} {slots} {sample} {local}", f"the exam prints {printed[0]!r}, the grants record declares {grants.LIMITS}")
    rows = [f"service grant {grants.HOLDERS[h]} {K[k]} {'p' if p else '-'}{'s' if s else '-'}" for h, k, p, s in grants.ROWS]
    _need(printed[1 : 1 + len(rows)] == rows, "the rows the exam prints are not the rows the grants record declares")
    refused = 0
    for label, text in (
        ("a stack of one byte", grants.record(stack=1)),
        ("a stack one byte short", grants.record(stack=grants.required_stack(*grants.LIMITS[:4], len(grants.ROWS)) - 1)),
        ("a fourth subscribe key for three local subscriptions", grants.record(rows=[(grants.HOLDERS[0], grants.KEYS[0], False, True), (grants.HOLDERS[0], grants.KEYS[1], False, True), (grants.HOLDERS[1], grants.KEYS[2], False, True), (grants.HOLDERS[2], grants.KEYS[3], False, True)])),
    ):
        _need(not grants.contract_accepts(text), f"control: the grants contract accepted {label}")
        refused += 1
    return refused


def size_controls() -> int:
    """A size above the contract's overhead is refused for that reason, for each of the four parts, and so
    are the size lines out of order. The honest sizes are within the bounds."""
    good = good_lines()
    bounds = size_bounds()
    for name in SIZE_NAMES:
        _need(SIZES[name] <= bounds[name], f"control: the exam's honest {name} size {SIZES[name]} exceeds the contract's {bounds[name]}")
    refused = 0
    first = 1 + len(grants.ROWS)
    for offset, name in enumerate(SIZE_NAMES):
        lines = list(good)
        lines[first + offset] = f"service size {name} {bounds[name] + 1}"
        try:
            judge(lines)
        except ServiceError as error:
            _need("exceeds" in str(error), f"control: a {name} one byte over its bound was refused for another reason: {error}")
            refused += 1
            continue
        raise ServiceError(f"control: the service judge accepted a {name} size one byte over the contract's {bounds[name]}")
    swapped = list(good)
    swapped[first], swapped[first + 1] = swapped[first + 1], swapped[first]
    try:
        judge(swapped)
    except ServiceError:
        return refused + 1
    raise ServiceError("control: the service judge accepted the slot and row size lines swapped")


def all_controls() -> int:
    return transcript_controls() + declaration_controls() + size_controls()


# What the admitted item fixes, held here as well so the gate stays correct after the item is completed
# and retired to .tasks/terminal. While the item is active its stored body is compared with these; once
# it is gone these still bind. Both counts are exact.
PINNED_CASES = 50
PINNED_CONTROLS = 126
ADMISSION = "slime-zenoh-service-core-"


def item_pins() -> tuple[int, int]:
    """The case count and the control count the admitted item fixes, read from its stored body, or (0, 0)."""
    for path in sorted((ROOT / ".tasks" / "items").glob("*.md")):
        text = path.read_text(encoding="utf-8")
        if "zenoh_service_check" not in text or ADMISSION not in text or re.search(r"^state: cancelled$", text, re.M):
            continue
        found = re.search(r'id = "all-cases-observed";.*?intValue = (\d+);', text, re.S)
        controls = re.search(r'id = "exactly-\d+-controls";.*?observation = "negativeControlsRefused";.*?intValue = (\d+);', text, re.S)
        if found and controls:
            return int(found.group(1)), int(controls.group(1))
    return 0, 0
