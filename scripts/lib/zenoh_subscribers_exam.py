"""Judge the Zenoh Profile 0 exam for a session that holds several subscriptions of its own.

Profile 0's session held exactly one subscription of its own: `send_declare_subscriber` refused with
`subscriber-limit` as soon as one was held, whatever the configuration declared. `Params` gains the
number of subscriptions this side may declare, `local`, and the table holds `per_side + local` slots.

The Rust tests under `zenoh_subscribers::` print one `[zenoh-exam] subscribers ...` line for each thing
they did. This module owns what those lines must be, and nothing here reads the Rust:

* the scenarios, each printed once and in order;
* the declaration and the undeclaration frames the session wrote, which the host reference
  `zenoh_wire` must reproduce byte for byte from the ids, keys and sequence numbers named here;
* how many subscriptions each side held after a step, and the sample each Put delivered;
* every refusal, by name and class.

Like the other judges it is trusted only after it refuses a mutation of an honest transcript.
"""

from __future__ import annotations

import re

import zenoh_exchange
import zenoh_wire
from harness import ROOT

# The sequence number of the first FRAME the declaring side sends: it is the listener, and the Rust
# tests configure it with this initial number.
LISTENER_SN = 7000
KEYS = (
    zenoh_exchange.demo()["key"],
    "0/slime_demo/extra/second",
    "0/slime_demo/extra/third",
    "0/slime_demo/extra/fourth",
)

OK_SCENARIOS = (
    "declare-several-own-subscribers",
    "put-reaches-every-declared-key",
    "undeclare-one-keeps-the-others",
    "slot-freed-by-undeclare-is-reusable",
    "two-subscriptions-on-one-key-deliver-once",
    "close-releases-every-subscription",
    "demo-params-keep-one",
)

# (name, bytes): the first three are scenario one's declarations, the last is scenario three's
# undeclaration of subscriber 2. All four are frames the listener sends, in sequence, so the host
# reference writes them from the ids and keys named above and the listener's initial number.
FRAMES = (
    *(
        (f"declare-{n}", zenoh_wire.frame(LISTENER_SN + n - 1, zenoh_wire.declare_subscriber(n, KEYS[n - 1], True)).hex())
        for n in (1, 2, 3)
    ),
    ("undeclare-2", zenoh_wire.frame(LISTENER_SN + 3, zenoh_wire.undeclare_subscriber(2, KEYS[1], True)).hex()),
)

# (label, subscriptions the declaring side holds, subscriptions the receiving side holds). Each side
# counts both directions, so after three declarations both hold three.
HELD = (
    ("declared-three", 3, 3),
    ("undeclared-one", 2, 2),
    ("reused-slot", 3, 3),
    ("closed", 0, 0),
)

# (label, attachment sequence of the one sample the Put delivered).
DELIVERED = (
    ("every-key-1", 0),
    ("every-key-2", 1),
    ("every-key-3", 2),
    ("kept-key-1", 10),
    ("kept-key-3", 11),
    ("reused-key-4", 20),
    ("two-on-one-key", 30),
    ("one-of-two-left", 31),
)

REFUSED_SCENARIOS = (
    ("fourth-own-subscriber", "subscriber-limit"),
    ("own-subscriber-repeated-id", "subscriber-limit"),
    ("put-to-undeclared-key-sent", "undeclared-key"),
    ("put-to-undeclared-key-received", "undeclared-key"),
    ("undeclare-unknown-id", "unknown-subscriber"),
    ("undeclare-held-id-on-another-key", "unknown-subscriber"),
    ("put-after-every-subscription-on-a-key-undeclared", "undeclared-key"),
    ("peer-declaration-over-its-limit", "subscriber-limit"),
    ("no-own-subscriber-configured", "invalid-config"),
    ("table-smaller-than-both-sides", "invalid-config"),
    ("table-larger-than-both-sides", "invalid-config"),
    ("own-subscriber-before-open", "not-open"),
    ("demo-params-second-own-subscriber", "subscriber-limit"),
)

SHAPES = {
    ("subscribers", "ok"): 3,
    ("subscribers", "frame"): 4,
    ("subscribers", "held"): 5,
    ("subscribers", "delivered"): 4,
    ("subscribers", "refused"): 4,
}


class SubscribersError(Exception):
    pass


def _need(condition: bool, message: str) -> None:
    if not condition:
        raise SubscribersError(message)


def good_lines() -> list[str]:
    return (
        [f"subscribers ok {name}" for name in OK_SCENARIOS]
        + [f"subscribers frame {name} {hexed}" for name, hexed in FRAMES]
        + [f"subscribers held {label} {declaring} {receiving}" for label, declaring, receiving in HELD]
        + [f"subscribers delivered {label} {sequence}" for label, sequence in DELIVERED]
        + [f"subscribers refused {name} {cls}" for name, cls in REFUSED_SCENARIOS]
    )


def _exactly(label: str, wanted: tuple[str, ...], seen: list[str]) -> None:
    missing = [name for name in wanted if name not in seen]
    extra = [name for name in seen if name not in wanted]
    repeated = sorted({name for name in seen if seen.count(name) > 1})
    _need(not missing, f"subscribers {label} missing: {', '.join(missing[:4])}")
    _need(not extra, f"subscribers {label} not in the exam: {', '.join(extra[:4])}")
    _need(not repeated, f"subscribers {label} reported more than once: {', '.join(repeated[:4])}")


def judge(lines: list[str]) -> tuple[int, int]:
    """Check a transcript against the exam; returns (cases, refusals), a case being a scenario or a sample."""
    split = [line.split() for line in lines]
    for line, fields in zip(lines, split, strict=True):
        _need(fields[:1] == ["subscribers"], f"subscribers: a record that is not a subscribers exam line: {line!r}")
        kind = tuple(fields[:2])
        _need(kind in SHAPES, f"subscribers: unrecognised line: {line}")
        _need(len(fields) == SHAPES[kind], f"subscribers: a {kind[1]} line has {len(fields)} fields, expected {SHAPES[kind]}: {line}")

    done = [f[2] for f in split if f[:2] == ["subscribers", "ok"]]
    _exactly("scenarios", OK_SCENARIOS, done)
    _need(done == list(OK_SCENARIOS), f"subscribers scenarios are out of order: {done}, the exam runs them as {list(OK_SCENARIOS)}")

    frames = [(f[2], f[3]) for f in split if f[:2] == ["subscribers", "frame"]]
    _exactly("frames", tuple(name for name, _ in FRAMES), [name for name, _ in frames])
    _need(
        [name for name, _ in frames] == [name for name, _ in FRAMES],
        f"subscribers frames are out of order: {[name for name, _ in frames]}",
    )
    for (name, hexed), (_, wanted) in zip(frames, FRAMES, strict=True):
        _need(re.fullmatch(r"[0-9a-f]+", hexed) is not None, f"subscribers frame {name} is not lowercase hexadecimal")
        _need(hexed == wanted, f"subscribers frame {name} is {hexed}, the host reference writes {wanted}")

    held = [(f[2], f[3], f[4]) for f in split if f[:2] == ["subscribers", "held"]]
    _exactly("counts", tuple(label for label, _, _ in HELD), [label for label, _, _ in held])
    _need([label for label, _, _ in held] == [label for label, _, _ in HELD], "subscribers counts are out of order")
    for (label, declaring, receiving), (_, want_declaring, want_receiving) in zip(held, HELD, strict=True):
        _need(
            (declaring, receiving) == (str(want_declaring), str(want_receiving)),
            f"subscribers held {label}: {declaring} and {receiving}, the exam requires {want_declaring} and {want_receiving}",
        )

    delivered = [(f[2], f[3]) for f in split if f[:2] == ["subscribers", "delivered"]]
    _exactly("samples", tuple(label for label, _ in DELIVERED), [label for label, _ in delivered])
    _need([label for label, _ in delivered] == [label for label, _ in DELIVERED], "subscribers samples are out of order")
    for (label, sequence), (_, wanted) in zip(delivered, DELIVERED, strict=True):
        _need(sequence == str(wanted), f"subscribers sample {label} carried sequence {sequence}, the Put sent {wanted}")

    refused = [(f[2], f[3]) for f in split if f[:2] == ["subscribers", "refused"]]
    _exactly("refusals", tuple(name for name, _ in REFUSED_SCENARIOS), [name for name, _ in refused])
    _need([name for name, _ in refused] == [name for name, _ in REFUSED_SCENARIOS], "subscribers refusals are out of order")
    wrong = [f"{name}={cls} (expected {dict(REFUSED_SCENARIOS)[name]})" for name, cls in refused if cls != dict(REFUSED_SCENARIOS)[name]]
    _need(not wrong, f"subscribers refusal classes differ: {', '.join(wrong)}")
    return len(done) + len(delivered), len(refused)


def _replaced(good: list[str], old: str, new: str) -> list[str]:
    _need(old in good, f"control: {old!r} is not in the honest transcript")
    return [new if line == old else line for line in good]


def transcript_mutations() -> list[tuple[str, list[str]]]:
    good = good_lines()
    out: list[tuple[str, list[str]]] = [
        ("an empty transcript", []),
        ("a line that is not a subscribers exam line", [*good, "stock ok stock-connects-open-and-deliver"]),
        ("an unrecognised kind of line", [*good, "subscribers dropped 1 2"]),
        ("a scenario line with an extra field", [*good[:-1], good[-1] + " extra"]),
        ("the first two scenarios swapped", [good[1], good[0], *good[2:]]),
        ("the last scenario before the first", [good[len(OK_SCENARIOS) - 1], *good[: len(OK_SCENARIOS) - 1], *good[len(OK_SCENARIOS) :]]),
    ]
    for index, line in enumerate(good):
        out.append((f"the line {line.split(' ', 2)[1]} {line.split(' ', 3)[2]} missing", good[:index] + good[index + 1 :]))
    for line in good:
        fields = line.split()
        if fields[1] == "ok":
            out.append((f"the scenario {fields[2]} reported twice", [*good, line]))
    for name, cls in REFUSED_SCENARIOS:
        other = next(candidate for _, candidate in REFUSED_SCENARIOS if candidate != cls)
        out.append((f"{name} refused as {other}", _replaced(good, f"subscribers refused {name} {cls}", f"subscribers refused {name} {other}")))
    for label, declaring, receiving in HELD:
        line = f"subscribers held {label} {declaring} {receiving}"
        out.append((f"{label}: the declaring side holds {declaring + 1}", _replaced(good, line, f"subscribers held {label} {declaring + 1} {receiving}")))
        out.append((f"{label}: the receiving side holds {receiving + 1}", _replaced(good, line, f"subscribers held {label} {declaring} {receiving + 1}")))
    for name, hexed in FRAMES:
        flipped = hexed[:-1] + ("0" if hexed[-1] != "0" else "1")
        out.append((f"the frame {name} with its last digit changed", _replaced(good, f"subscribers frame {name} {hexed}", f"subscribers frame {name} {flipped}")))
        out.append((f"the frame {name} in capitals", _replaced(good, f"subscribers frame {name} {hexed}", f"subscribers frame {name} {hexed.upper()}")))
    for label, sequence in DELIVERED:
        out.append((f"the sample {label} carrying sequence {sequence + 1}", _replaced(good, f"subscribers delivered {label} {sequence}", f"subscribers delivered {label} {sequence + 1}")))
    return out


def transcript_controls() -> int:
    good = good_lines()
    cases, refusals = judge(good)
    _need((cases, refusals) == (len(OK_SCENARIOS) + len(DELIVERED), len(REFUSED_SCENARIOS)), f"subscribers: the honest transcript is {cases} cases and {refusals} refusals")
    refused = 0
    for label, lines in transcript_mutations():
        if lines == good:
            raise SubscribersError(f"control: {label} did not change the transcript")
        try:
            judge(lines)
        except SubscribersError:
            refused += 1
            continue
        raise SubscribersError(f"control: the subscribers judge accepted {label}")
    return refused


def reference_agrees() -> None:
    """The host reference writes frames of distinct names, ids and sequence numbers, none of them the same twice."""
    _need(len({name for name, _ in FRAMES}) == len(FRAMES), "a frame name is used twice")
    _need(len({hexed for _, hexed in FRAMES}) == len(FRAMES), "two frames are the same bytes")
    _need(len(set(KEYS)) == len(KEYS), "a key is used twice")
    _need(len({name for name, _ in REFUSED_SCENARIOS}) == len(REFUSED_SCENARIOS), "a refused scenario name is used twice")


def reference_controls() -> int:
    """Each guard of `reference_agrees`, tripped in turn."""
    saved = (FRAMES, KEYS, REFUSED_SCENARIOS)
    cases = 0
    try:
        for label, change, reason in (
            ("a frame name used twice", {"FRAMES": (FRAMES[0], FRAMES[0], *FRAMES[2:])}, "a frame name is used twice"),
            ("two frames of the same bytes", {"FRAMES": (FRAMES[0], (FRAMES[1][0], FRAMES[0][1]), *FRAMES[2:])}, "two frames are the same bytes"),
            ("a key used twice", {"KEYS": (KEYS[0], KEYS[0], *KEYS[2:])}, "a key is used twice"),
            ("a refused scenario name used twice", {"REFUSED_SCENARIOS": (REFUSED_SCENARIOS[0], REFUSED_SCENARIOS[0], *REFUSED_SCENARIOS[2:])}, "a refused scenario name is used twice"),
        ):
            globals().update(change)
            try:
                reference_agrees()
            except SubscribersError as error:
                _need(reason in str(error), f"control: {label} was refused for another reason: {error}")
                cases += 1
                continue
            finally:
                globals().update(FRAMES=saved[0], KEYS=saved[1], REFUSED_SCENARIOS=saved[2])
            raise SubscribersError(f"control: the reference check accepted {label}")
    finally:
        globals().update(FRAMES=saved[0], KEYS=saved[1], REFUSED_SCENARIOS=saved[2])
    return cases


def all_controls() -> int:
    reference_agrees()
    return transcript_controls() + reference_controls()


# What the admitted item fixes, held here as well so the gate stays correct after the item is completed
# and retired to .tasks/terminal. While the item is active its stored body is compared with these; once
# it is gone these still bind. Both counts are exact.
PINNED_CASES = 28
PINNED_CONTROLS = 90
ADMISSION = "slime-zenoh-profile0-own-subscribers-"


def item_pins() -> tuple[int, int]:
    """The case count and the control count the admitted item fixes, read from its stored body, or (0, 0)."""
    for path in sorted((ROOT / ".tasks" / "items").glob("*.md")):
        text = path.read_text(encoding="utf-8")
        if "zenoh_subscribers_check" not in text or ADMISSION not in text or re.search(r"^state: cancelled$", text, re.M):
            continue
        cases = re.search(r'id = "all-cases-observed";.*?intValue = (\d+);', text, re.S)
        controls = re.search(r'id = "exactly-\d+-controls";.*?observation = "negativeControlsRefused";.*?intValue = (\d+);', text, re.S)
        if cases and controls:
            return int(cases.group(1)), int(controls.group(1))
    return 0, 0
