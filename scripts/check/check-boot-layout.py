#!/usr/bin/env python3

"""B10 boot-layout equivalence check.

Init's capability layout is a contract between three parties that cannot see
each other: the kernel builds the table, the component images address slots by
number, and the gates assert on what the components then do. Nothing in the
build fails when those three disagree — the boot simply does the wrong thing,
and the gate that notices reports it as a component fault far from the cause.

This check makes the layout itself observable. Each profile below boots the
generation a gate boots, captures the `[layout]` lines the kernel emits for
init's resolved table, and compares them against a frozen fixture. A change
that moves a slot fails here, naming the slot, rather than downstream.

The fixtures are regenerated with `--bless`. Blessing is a deliberate act:
the diff it produces is the evidence that a layout change was intended, and it
belongs in the review that changes the layout.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
import os

from harness import ROOT, run_qemu

FIXTURES = ROOT / "contracts" / "boot-layout" / "v1" / "fixtures"

# One entry per distinct init layout a gate boots today. The name is the fixture
# stem; the environment is exactly what the corresponding check script sets, so
# a profile here resolves the same layout its gate resolves.
PROFILES: list[tuple[str, dict[str, str]]] = [
    ("default", {"SLIME_GENERATION_NUMBER": "1"}),
    ("storage-write", {"SLIME_GENERATION_NUMBER": "2"}),
    ("storage-fault", {"SLIME_GENERATION_NUMBER": "3"}),
    ("storage-store", {"SLIME_GENERATION_NUMBER": "4"}),
    ("directory", {"SLIME_GENERATION_NUMBER": "6"}),
    ("dango", {"SLIME_GENERATION_NUMBER": "7", "SLIME_DANGO_CHECK": "1"}),
    (
        "generation-commands",
        {"SLIME_GENERATION_NUMBER": "8", "SLIME_GENERATION_CMD_CHECK": "1"},
    ),
    ("powerbox", {"SLIME_GENERATION_NUMBER": "9", "SLIME_POWERBOX_CHECK": "1"}),
    (
        "sample-plane",
        {"SLIME_GENERATION_NUMBER": "10", "SLIME_SAMPLE_PLANE_CHECK": "1"},
    ),
    (
        "fabric-authority",
        {"SLIME_GENERATION_NUMBER": "11", "SLIME_FABRIC_AUTHORITY_CHECK": "1"},
    ),
    (
        "fabric-stream",
        {"SLIME_GENERATION_NUMBER": "12", "SLIME_FABRIC_STREAM_CHECK": "1"},
    ),
    ("fabric-qos", {"SLIME_GENERATION_NUMBER": "13", "SLIME_FABRIC_QOS_CHECK": "1"}),
    ("fabric-call", {"SLIME_GENERATION_NUMBER": "14", "SLIME_FABRIC_CALL_CHECK": "1"}),
    (
        "fabric-operation",
        {"SLIME_GENERATION_NUMBER": "15", "SLIME_FABRIC_OPERATION_CHECK": "1"},
    ),
    (
        "fabric-visibility",
        {"SLIME_GENERATION_NUMBER": "16", "SLIME_FABRIC_VISIBILITY_CHECK": "1"},
    ),
    (
        "fabric-boot",
        {"SLIME_GENERATION_NUMBER": "17", "SLIME_FABRIC_BOOT_CHECK": "1"},
    ),
]

# Flags that select a boot path. A profile that does not set one must not
# inherit it from the caller's environment, or the layout it captures is not the
# layout its gate boots.
GATE_FLAGS = [
    "SLIME_DANGO_CHECK",
    "SLIME_GENERATION_CMD_CHECK",
    "SLIME_GENERATION_CMD_SCENARIO",
    "SLIME_POWERBOX_CHECK",
    "SLIME_SAMPLE_PLANE_CHECK",
    "SLIME_FABRIC_AUTHORITY_CHECK",
    "SLIME_FABRIC_STREAM_CHECK",
    "SLIME_FABRIC_QOS_CHECK",
    "SLIME_FABRIC_CALL_CHECK",
    "SLIME_FABRIC_OPERATION_CHECK",
    "SLIME_FABRIC_VISIBILITY_CHECK",
    "SLIME_FABRIC_BOOT_CHECK",
    "SLIME_FABRIC_PROXY_EARLY_EXIT",
    "SLIME_FABRIC_PROFILE",
    "SLIME_TRANSFER_RECEIVER",
    "SLIME_TRANSFER_ACTIVATE",
    "SLIME_RECOVERY_IMAGE",
    "SLIME_INTERACTIVE",
]


def capture(name: str, settings: dict[str, str]) -> str:
    """Boot one profile and return its `[layout]` block."""
    environment = os.environ.copy()
    for flag in GATE_FLAGS:
        environment.pop(flag, None)
    environment.update(settings)
    output = run_qemu(
        ["cargo", "run", "--release", "--", "-display", "none"],
        environment=environment,
        cwd=ROOT / "kernel",
        timeout=180,
        echo="never",
        allow_failure=True,
    )
    lines = [line.strip() for line in output.splitlines()]
    # A boot emits one block per launched init. Take the first: later blocks
    # would come from a re-launch, which no profile here performs.
    try:
        start = next(i for i, line in enumerate(lines) if line.startswith("[layout] path="))
        end = next(i for i, line in enumerate(lines[start:], start) if line == "[layout] end")
    except StopIteration:
        print(output, end="")
        raise SystemExit(f"{name}: boot emitted no complete layout block") from None
    return "\n".join(lines[start : end + 1]) + "\n"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--bless",
        action="store_true",
        help="rewrite the fixtures from the observed layouts",
    )
    parser.add_argument(
        "--profile",
        action="append",
        help="capture only the named profile (repeatable)",
    )
    arguments = parser.parse_args()

    selected = PROFILES
    if arguments.profile:
        wanted = set(arguments.profile)
        unknown = wanted - {name for name, _ in PROFILES}
        if unknown:
            raise SystemExit(f"unknown profile(s): {', '.join(sorted(unknown))}")
        selected = [entry for entry in PROFILES if entry[0] in wanted]

    FIXTURES.mkdir(parents=True, exist_ok=True)
    failures: list[str] = []
    for name, settings in selected:
        observed = capture(name, settings)
        fixture = FIXTURES / f"{name}.layout"
        if arguments.bless:
            fixture.write_text(observed)
            print(f"blessed {name}: {len(observed.splitlines()) - 2} slots")
            continue
        if not fixture.exists():
            failures.append(f"{name}: no fixture; run with --bless to record it")
            continue
        expected = fixture.read_text()
        if observed == expected:
            print(f"{name}: {len(observed.splitlines()) - 2} slots match")
            continue
        failures.append(f"{name}: layout differs from {fixture.relative_to(ROOT)}")
        expected_lines = expected.splitlines()
        observed_lines = observed.splitlines()
        for index in range(max(len(expected_lines), len(observed_lines))):
            was = expected_lines[index] if index < len(expected_lines) else "<absent>"
            now = observed_lines[index] if index < len(observed_lines) else "<absent>"
            if was != now:
                failures.append(f"    was: {was}")
                failures.append(f"    now: {now}")

    if failures:
        for line in failures:
            print(line)
        raise SystemExit("boot layout check: layouts moved")
    if arguments.bless:
        print("boot layout check: blessed")
        return
    print("boot layout check: ok")


if __name__ == "__main__":
    main()
