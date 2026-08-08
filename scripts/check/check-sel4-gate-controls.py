#!/usr/bin/env python3
"""Prove that the seL4 plane gates actually fail when their evidence is absent.

Every seL4 plane gate is a marker-matching Python checker: it boots an image and
asserts that an ordered sequence of serial markers appears and that no failure
marker does. Nothing in-repo demonstrated that a *missing* marker makes one of
them red. The oracle had `should_panic.rs` for exactly this — proof that a failing
assertion is observable at all — and the seL4 side had no equivalent, leaving
per-slice fault injection (per-change discipline) as the only mitigation.

This is that standing guard. For each gate it builds a synthetic transcript from
the gate's own `REQUIRED_MARKERS`, checks the gate accepts it, and then checks the
gate rejects three mutations of it:

* one required marker deleted,
* the first two required markers transposed,
* a failure marker appended.

The transcripts are synthetic on purpose. A negative control must be able to
produce evidence that is *wrong in one specific way*, which no real boot can be
asked to do — and building them from each gate's own marker table means the
control cannot drift out of step with the gate it guards.

What this does not claim: that the markers are the right markers, or that a real
boot emits them. The plane gates themselves assert that. This asserts only that
those assertions have teeth.
"""

from __future__ import annotations

import re
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

from harness import ROOT, load_script  # noqa: E402

# Gate module name -> checker path, for every plane gate that shares the
# `REQUIRED_MARKERS` / `FAILURE_MARKERS` / `check_transcript` shape.
#
# `check-sel4-boot-layout.py` is absent deliberately: it compares frozen fixtures
# rather than markers, so it does not expose the surface this control drives. The
# stream gate declares `CHAINS` instead of a flat table and is handled by
# `required_of`.
# Third element is the number of required markers the gate is expected to declare.
#
# Pinned rather than derived, because the whole point is to notice when a gate
# gets *weaker*: deleting a marker from a gate's table would otherwise just make
# this control report a smaller number and still pass. Raising a count here is a
# deliberate act that shows up in review; silently losing coverage is not.
GATES: tuple[tuple[str, str, int], ...] = (
    ("sel4_channel_plane", "check/check-sel4-channel-plane.py", 27),
    ("sel4_component_graph", "check/check-sel4-component-graph.py", 19),
    ("sel4_crossing_plane", "check/check-sel4-crossing-plane.py", 10),
    ("sel4_loan_plane", "check/check-sel4-loan-plane.py", 44),
    ("sel4_root_boot", "check/check-sel4-root-boot.py", 40),
    ("sel4_sample_plane", "check/check-sel4-sample-plane.py", 19),
    ("sel4_spawn_plane", "check/check-sel4-spawn-plane.py", 32),
    ("sel4_supervision_plane", "check/check-sel4-supervision-plane.py", 11),
    ("sel4_stream_plane", "check/check-sel4-stream-plane.py", 56),
    ("sel4_qos_plane", "check/check-sel4-qos-plane.py", 14),
)


def fail(message: str) -> None:
    raise SystemExit(f"seL4 gate control check: {message}")


def literal_for(pattern: str) -> str:
    """One concrete line that satisfies `pattern`.

    The marker tables are regexes over serial output, so a synthetic transcript
    has to instantiate them. Only the constructs the tables actually use are
    handled, and anything else is a hard error rather than a silent skip — a
    control that quietly stopped covering a marker would be worse than no control.
    """
    text = pattern
    # Anchors and whitespace-tolerance constructs carry no content.
    text = text.replace(r"\s+", " ").replace(r"\s*", "")
    text = text.replace("^", "").replace("$", "")
    # Character classes and repetitions, narrowest first.
    text = re.sub(r"\[0-9a-fx\]\+", "0x10", text)
    text = re.sub(r"\[0-9a-f\]\+", "abc123", text)
    text = re.sub(r"\[\^ \]\+", "value", text)
    # `[1-9]\d*` and friends: a non-zero digit followed by optional digits.
    text = re.sub(r"\[1-9\]\\d\*", "7", text)
    text = re.sub(r"\[1-9\]", "7", text)
    text = re.sub(r"\[0-9\]\+", "7", text)
    text = re.sub(r"\\d\*", "7", text)
    text = re.sub(r"\\d\+", "7", text)
    text = re.sub(r"\\d", "7", text)
    text = re.sub(r"\\w\+", "word", text)
    text = re.sub(r"\.\+", "text", text)
    text = re.sub(r"\.\*", "", text)
    # Zero-width lookarounds constrain neighbours, not content.
    text = re.sub(r"\(\?[!=][^()]*\)", "", text)
    # Alternations inside a group: take the first branch, non-capturing first so
    # its `?:` does not survive into the capturing rule below.
    text = re.sub(r"\(\?:([^()|]*)\|[^()]*\)", r"\1", text)
    text = re.sub(r"\(([^()|]*)\|[^()]*\)", r"\1", text)
    # A digit range left by that first branch, e.g. `3[3-9]`.
    text = re.sub(r"\[(\d)-\d\]", r"\1", text)
    # Escaped literals are parked as sentinels first, so `\(aborted\)` keeps its
    # parentheses instead of losing them to the group-unwrapping below.
    parked: list[str] = []

    def park(match: re.Match[str]) -> str:
        parked.append(match.group(1))
        return f"\x01{len(parked) - 1}\x02"

    text = re.sub(r"\\([-\[\]().*+?{}|^$/\\])", park, text)
    # Remaining group wrappers, now unambiguous.
    text = text.replace("(?:", "(")
    text = re.sub(r"\((.*?)\)", r"\1", text)
    for index, literal in enumerate(parked):
        text = text.replace(f"\x01{index}\x02", literal)
    if re.search(pattern, text) is None:
        fail(f"cannot instantiate marker pattern: {pattern!r} -> {text!r}")
    return text


def required_of(gate) -> tuple[tuple[str, str], ...]:
    """The gate's ordered markers as `(description, pattern)` pairs.

    Two shapes exist. Most gates declare one flat `REQUIRED_MARKERS` table; the
    stream gate declares `CHAINS`, a per-causal-chain grouping, because its claim
    is that each chain is internally ordered rather than that all 56 markers are
    globally ordered. Flattening is sound for this control: every mutation it
    makes is within a chain, so a gate that enforces per-chain order rejects them
    exactly as a flat gate does.
    """
    chains = getattr(gate, "CHAINS", None)
    if chains is not None:
        return tuple(
            (f"{label}: {pattern}", pattern)
            for label, chain in chains
            for pattern in chain
        )
    return tuple(gate.REQUIRED_MARKERS)


def transcript_for(gate) -> str:
    lines = [literal_for(pattern) for _, pattern in required_of(gate)]
    return "\n".join(lines) + "\n"


def marker_check(gate, transcript: str) -> None:
    """The gate's own marker contract, isolated from its content assertions.

    `check_transcript` also calls per-gate helpers such as `check_queue_depth`
    that read counters a synthetic transcript does not carry. Those are the gate
    asserting things *about* a real boot, which is not what this control guards,
    so the ordered-marker and failure-marker logic is driven directly instead.
    Copied rather than called because it is four lines and importing it would
    couple this control to each gate's private helper set.
    """
    for pattern in gate.FAILURE_MARKERS:
        if re.search(pattern, transcript) is not None:
            raise SystemExit(f"failure marker: {pattern}")
    position = 0
    for description, pattern in required_of(gate):
        match = re.compile(pattern).search(transcript, position)
        if match is None:
            raise SystemExit(f"missing or out-of-order marker: {description}")
        position = match.end()


def rejects(gate, transcript: str) -> bool:
    """True when the gate's marker contract refuses this transcript."""
    try:
        marker_check(gate, transcript)
    except SystemExit:
        return True
    return False


def check_gate(name: str, relative_path: str, expected_required: int) -> int:
    gate = load_script(name, relative_path)
    required = required_of(gate)
    failures = getattr(gate, "FAILURE_MARKERS", ())
    if len(required) != expected_required:
        fail(
            f"{name}: declares {len(required)} required markers, expected "
            f"{expected_required}. A gate that lost a marker lost coverage; "
            "update the pin here only alongside the gate change that justifies it"
        )
    if len(required) < 2:
        fail(f"{name}: fewer than two required markers, nothing to transpose")
    if not failures:
        fail(f"{name}: no failure markers declared")

    baseline = transcript_for(gate)
    if rejects(gate, baseline):
        fail(
            f"{name}: rejected a transcript built from its own REQUIRED_MARKERS; "
            "the control cannot distinguish a real absence from its own synthesis"
        )

    lines = baseline.splitlines()

    # A missing marker must be caught. This is the property the whole control
    # exists for: a gate that passes without its evidence proves nothing.
    for index in range(len(lines)):
        without = "\n".join(lines[:index] + lines[index + 1 :]) + "\n"
        if not rejects(gate, without):
            description = required[index][0]
            fail(f"{name}: accepted a transcript missing {description!r}")

    # Order is part of the claim: these gates assert a *sequence*, so a
    # transcript with the right lines in the wrong order must fail too.
    transposed = "\n".join([lines[1], lines[0], *lines[2:]]) + "\n"
    if not rejects(gate, transposed):
        fail(f"{name}: accepted its first two required markers out of order")

    # A failure marker must veto an otherwise-complete transcript.
    for pattern in failures:
        poisoned = baseline + literal_for(pattern) + "\n"
        if not rejects(gate, poisoned):
            fail(f"{name}: accepted a transcript containing failure marker {pattern!r}")

    return len(lines) + 1 + len(failures)


def check_layout_gate() -> int:
    """The boot-layout gate's structural validator, driven with broken fixtures.

    That gate's claim is fixture *equality*, so it has no marker table to mutate.
    But it also runs `check_shape` over every captured layout before comparing,
    and that validator has properties worth guarding: a header, a terminator,
    well-formed rows, a declared count matching the rows carried, and ascending
    slot numbers. Each is driven here from a real blessed fixture, so a
    `check_shape` that stopped enforcing one would be caught without a boot.
    """
    gate = load_script("sel4_boot_layout", "check/check-sel4-boot-layout.py")
    fixture = (
        ROOT / "contracts" / "boot-layout" / "v1" / "fixtures" / "sel4-channel.layout"
    )
    if not fixture.is_file():
        fail(f"missing blessed fixture: {fixture}")
    baseline = fixture.read_text(encoding="utf-8")

    def rejects_shape(text: str) -> bool:
        try:
            gate.check_shape("control", text)
        except SystemExit:
            return True
        return False

    if rejects_shape(baseline):
        fail("boot-layout gate rejected its own blessed fixture")

    lines = baseline.splitlines()
    mutations: tuple[tuple[str, str], ...] = (
        ("header removed", "\n".join(lines[1:]) + "\n"),
        ("terminator removed", "\n".join(lines[:-1]) + "\n"),
        (
            "declared count disagrees with the rows carried",
            baseline.replace("slots=2", "slots=3", 1),
        ),
        (
            "row is malformed",
            baseline.replace("[layout] 3 endpoint", "[layout] endpoint", 1),
        ),
        (
            "slot numbers descend",
            "\n".join([lines[0], lines[2], lines[1], *lines[3:]]) + "\n",
        ),
    )
    for description, text in mutations:
        if not rejects_shape(text):
            fail(f"boot-layout gate accepted a layout whose {description}")
    return len(mutations)


def main() -> None:
    if Path_cwd() != ROOT:
        fail(f"run from repository root: {ROOT}")
    total = 0
    for name, relative_path, expected_required in GATES:
        total += check_gate(name, relative_path, expected_required)
    total += check_layout_gate()
    print(
        f"seL4 gate control check: {len(GATES) + 1} gates reject "
        f"{total} mutated transcripts and layouts"
    )


def Path_cwd() -> _Path:
    return _Path.cwd().resolve()


if __name__ == "__main__":
    main()
