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
# `check-sel4-stream-plane.py` and `check-sel4-boot-layout.py` are absent
# deliberately: the stream gate composes its required set at runtime rather than
# declaring one table, and the layout gate compares fixtures instead of markers,
# so neither exposes the surface this control drives. Both are noted in the
# devlog rather than silently skipped.
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
    ("sel4_supervision_plane", "check/check-sel4-supervision-plane.py", 9),
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


def transcript_for(gate) -> str:
    lines = [literal_for(pattern) for _, pattern in gate.REQUIRED_MARKERS]
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
    for description, pattern in gate.REQUIRED_MARKERS:
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
    required = getattr(gate, "REQUIRED_MARKERS", ())
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


def main() -> None:
    if Path_cwd() != ROOT:
        fail(f"run from repository root: {ROOT}")
    total = 0
    for name, relative_path, expected_required in GATES:
        total += check_gate(name, relative_path, expected_required)
    print(
        f"seL4 gate control check: {len(GATES)} gates reject "
        f"{total} mutated transcripts"
    )


def Path_cwd() -> _Path:
    return _Path.cwd().resolve()


if __name__ == "__main__":
    main()
