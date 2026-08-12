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
from sel4_gate_markers import chains_from_gate, match_marker_contract  # noqa: E402

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
    ("sel4_component_graph", "check/check-sel4-component-graph.py", 22),
    ("sel4_crossing_plane", "check/check-sel4-crossing-plane.py", 10),
    ("sel4_loan_plane", "check/check-sel4-loan-plane.py", 44),
    ("sel4_device_plane", "check/check-sel4-device-plane.py", 7),
    ("sel4_root_boot", "check/check-sel4-root-boot.py", 43),
    # 24 since B46-B48: the sample plane also proves a process runs two
    # threads, that a busy thread declared below its peer does not starve it,
    # and that those threads exchange one message over their native seL4
    # endpoint without root channel mediation.
    ("sel4_sample_plane", "check/check-sel4-sample-plane.py", 24),
    ("sel4_spawn_plane", "check/check-sel4-spawn-plane.py", 32),
    ("sel4_supervision_plane", "check/check-sel4-supervision-plane.py", 12),
    ("sel4_stream_plane", "check/check-sel4-stream-plane.py", 55),
    ("sel4_qos_plane", "check/check-sel4-qos-plane.py", 14),
    ("sel4_call_plane", "check/check-sel4-call-plane.py", 50),
    ("sel4_operation_plane", "check/check-sel4-operation-plane.py", 53),
    ("sel4_visibility_plane", "check/check-sel4-visibility-plane.py", 25),
    ("sel4_boot_plane", "check/check-sel4-boot-plane.py", 46),
    ("sel4_storage_plane", "check/check-sel4-storage-plane.py", 10),
    ("sel4_store_plane", "check/check-sel4-store-plane.py", 15),
    ("sel4_rollback_plane", "check/check-sel4-rollback-plane.py", 17),
    ("sel4_recovery_plane", "check/check-sel4-recovery-plane.py", 12),
    ("sel4_generation_plane", "check/check-sel4-generation-plane.py", 20),
    ("sel4_directory_plane", "check/check-sel4-directory-plane.py", 17),
    ("sel4_filesystem_plane", "check/check-sel4-filesystem-plane.py", 11),
    ("sel4_input_plane", "check/check-sel4-input-plane.py", 8),
    ("sel4_powerbox_plane", "check/check-sel4-powerbox-plane.py", 11),
    ("sel4_dango_plane", "check/check-sel4-dango-plane.py", 13),
    ("sel4_transfer_plane", "check/check-sel4-transfer-plane.py", 12),
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
    text = text.replace("-?", "-")
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
    # Backreferences hold two fields equal, which is the assertion rather than
    # decoration -- `required=(\d+) live=\1` says a healthy graph has every
    # required instance live. Replaying the group's own instantiated text keeps
    # that true in the synthetic line; dropping it would emit a literal `\1`
    # and fail the round-trip check below.
    groups = re.findall(r"\((?!\?)([^()]*)\)", text)
    for index, group in enumerate(groups, start=1):
        text = text.replace(f"\\{index}", group)
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
    """Flatten declarations for synthesis and count pins, not matching."""
    return tuple(
        (f"{label}: {pattern}", pattern)
        for label, chain in chains_from_gate(gate)
        for pattern in chain
    )


def transcript_for(gate) -> str:
    lines = [literal_for(pattern) for _, pattern in required_of(gate)]
    return "\n".join(lines) + "\n"


def boot_plane_transcript(gate, marker_transcript: str) -> str:
    """Add the structural composition evidence required by the boot-plane gate."""
    lines = marker_transcript.splitlines()
    service_line = next(
        (line for line in lines if "component=fabric-service " in line), None
    )
    call_line = next((line for line in lines if "component=fabric-call-worker " in line), None)
    op_line = next((line for line in lines if "component=fabric-op-worker " in line), None)
    init_spawns = [
        f"SLIME_GRAPH spawned task=100 child={201 + index} component={component} "
        "grants=1 channels=1 handle=1"
        for index, component in enumerate(gate.EXPECTED_INIT_CHILDREN)
    ]
    expanded: list[str] = []
    for line in lines:
        if service_line is not None and line == service_line:
            expanded.extend(init_spawns)
        elif line == call_line:
            expanded.append(
                "SLIME_GRAPH spawned task=204 child=301 component=fabric-call-worker "
                "grants=1 channels=1 handle=1"
            )
        elif line == op_line:
            expanded.append(
                "SLIME_GRAPH spawned task=204 child=302 component=fabric-op-worker "
                "grants=1 channels=1 handle=1"
            )
        else:
            expanded.append(line)
    expanded.extend(
        [
            "[layout] path=init slots=1 max=64",
            "[layout] 1 endpoint control",
            *(f"[{component}] boot idle without a role" for component in gate.EXPECTED_IDLE_WITHOUT_ROLE),
            # `check_transcript` requires exactly one healthy-supervisor
            # terminal, but `TERMINAL_MARKER` is not in `REQUIRED_MARKERS`, so
            # the marker synthesis never produces one. Instantiated from the
            # gate's own pattern rather than written out here, so a change to
            # it cannot leave this stale.
            literal_for(gate.TERMINAL_MARKER),
        ]
    )
    return "\n".join(expanded) + "\n"


def marker_check(gate, transcript: str) -> None:
    """Invoke the exact matcher used by chain-aware product gates."""
    match_marker_contract(
        transcript,
        chains_from_gate(gate),
        gate.FAILURE_MARKERS,
        lambda message: (_ for _ in ()).throw(SystemExit(message)),
    )


def rejects(gate, transcript: str) -> bool:
    """True when the product gate refuses this transcript."""
    try:
        if gate.__name__ == "sel4_boot_plane":
            gate.check_transcript(transcript)
        else:
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
    chains = chains_from_gate(gate)
    if not any(len(patterns) >= 2 for _, patterns in chains):
        fail(f"{name}: no causal chain has two markers, nothing to transpose")
    if not failures:
        fail(f"{name}: no failure markers declared")

    marker_baseline = transcript_for(gate)

    def complete(text: str) -> str:
        return boot_plane_transcript(gate, text) if name == "sel4_boot_plane" else text

    baseline = complete(marker_baseline)
    if rejects(gate, baseline):
        fail(
            f"{name}: rejected a transcript built from its own REQUIRED_MARKERS; "
            "the control cannot distinguish a real absence from its own synthesis"
        )

    lines = marker_baseline.splitlines()

    # Delete every occurrence of the selected concrete marker. A regex shared by
    # two chains may legitimately use either occurrence, so deleting only one
    # physical line is not evidence removal; deleting them all is.
    evaluated = 0
    for index, removed in enumerate(lines):
        without = "\n".join(line for line in lines if line != removed) + "\n"
        if not rejects(gate, complete(without)):
            description = required[index][0]
            fail(f"{name}: accepted a transcript missing all evidence for {description!r}")
        evaluated += 1

    offset = 0
    for _label, patterns in chains:
        if len(patterns) >= 2:
            first = lines[offset]
            second = lines[offset + 1]
            insertion = next(
                index for index, line in enumerate(lines) if line == first or line == second
            )
            remaining = [line for line in lines if line != first and line != second]
            transposed_lines = remaining[:insertion] + [second, first] + remaining[insertion:]
            transposed = "\n".join(transposed_lines) + "\n"
            if not rejects(gate, complete(transposed)):
                fail(f"{name}: accepted the first two markers of a causal chain out of order")
            evaluated += 1
        offset += len(patterns)

    # A failure marker must veto an otherwise-complete transcript.
    for pattern in failures:
        poisoned = baseline + literal_for(pattern) + "\n"
        if not rejects(gate, poisoned):
            fail(f"{name}: accepted a transcript containing failure marker {pattern!r}")

    if name == "sel4_boot_plane":
        baseline_lines = baseline.splitlines()
        idle = literal_for(r"\[fabric\] idle: parked on control endpoints")
        terminal = literal_for(gate.TERMINAL_MARKER)
        if idle not in baseline_lines or terminal not in baseline_lines:
            fail(f"{name}: synthetic transcript lacks its idle or healthy supervisor marker")
        terminal_index = baseline_lines.index(terminal)
        early_idle_then_exit = "\n".join(
            [
                *baseline_lines[:terminal_index],
                idle,
                "SLIME_GRAPH component exit task=17 status=-9",
                *baseline_lines[terminal_index:],
            ]
        ) + "\n"
        if not rejects(gate, early_idle_then_exit):
            fail(
                f"{name}: accepted early fabric idle followed by nonzero component exit "
                "before the healthy supervisor terminal"
            )
        nonzero_exit = next(
            pattern for pattern in failures if pattern.startswith("SLIME_GRAPH component exit")
        )
        if re.search(nonzero_exit, "SLIME_GRAPH component exit task=17 status=0"):
            fail(f"{name}: generic nonzero-exit failure marker also matches status zero")
        for status in ("7", "-9"):
            if re.search(
                nonzero_exit, f"SLIME_GRAPH component exit task=17 status={status}"
            ) is None:
                fail(f"{name}: generic nonzero-exit failure marker misses status {status}")
        evaluated += 1

    return evaluated + len(failures)


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
            # Derived from the fixture rather than hardcoded: the channel
            # plane's layout has grown before and will again, and a literal
            # `slots=N` that no longer appears makes this mutation a no-op —
            # a control that silently stops controlling.
            "declared count disagrees with the rows carried",
            re.sub(
                r"slots=(\d+)",
                lambda match: f"slots={int(match.group(1)) + 1}",
                baseline,
                count=1,
            ),
        ),
        (
            # Derived, for the reason the mutation above is: this hardcoded
            # `[layout] 3 endpoint`, and the channel plane's layout no longer
            # reaches slot 3, so the mutation replaced nothing and the control
            # passed a transcript it had not mutated. Dropping the slot number
            # from whichever row is first is malformed in every layout.
            "row is malformed",
            re.sub(r"\[layout\] \d+ ", "[layout] ", baseline, count=1),
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
