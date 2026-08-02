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
import subprocess
import tempfile
from pathlib import Path

from harness import ROOT, run_qemu

FIXTURES = ROOT / "contracts" / "boot-layout" / "v1" / "fixtures"

# A virtio-blk device at the address the storage path probes. The storage
# profiles must boot with one attached: `optional_block_function()` decides slot
# 9's object *kind* by PCI enumeration, so without a drive those profiles
# capture the no-disk `ObjectStore` fallback rather than the block capability
# their gate actually exercises.
STORAGE_DRIVE = [
    "-drive",
    "if=none,id=slime-storage,format=raw,cache=directsync,file={image}",
    "-device",
    "virtio-blk-pci,drive=slime-storage,disable-legacy=on,queue-size=8",
]

# One entry per distinct init layout a gate boots today. The name is the fixture
# stem; the environment is exactly what the corresponding check script sets, so
# a profile here resolves the same layout its gate resolves. The third element
# is extra QEMU arguments, for profiles whose layout depends on attached
# hardware.
#
# B11: every entry names its boot profile explicitly. `product` is the boot the
# product ships and declares no verification scaffolding; every other entry
# selects a profile that declares the probes its gate exercises, which is why
# their slot tables are unchanged.
PROFILES: list[tuple[str, dict[str, str], list[str]]] = [
    ("product", {"SLIME_GENERATION_NUMBER": "1", "SLIME_FABRIC_PROFILE": "default"}, []),
    ("default", {"SLIME_GENERATION_NUMBER": "1", "SLIME_FABRIC_PROFILE": "test"}, []),
    (
        "storage-read",
        {"SLIME_GENERATION_NUMBER": "1", "SLIME_FABRIC_PROFILE": "test"},
        STORAGE_DRIVE,
    ),
    (
        "storage-write",
        {"SLIME_GENERATION_NUMBER": "2", "SLIME_FABRIC_PROFILE": "test"},
        STORAGE_DRIVE,
    ),
    (
        "storage-fault",
        {"SLIME_GENERATION_NUMBER": "3", "SLIME_FABRIC_PROFILE": "test"},
        STORAGE_DRIVE,
    ),
    (
        "storage-store",
        {"SLIME_GENERATION_NUMBER": "4", "SLIME_FABRIC_PROFILE": "test"},
        STORAGE_DRIVE,
    ),
    ("directory", {"SLIME_GENERATION_NUMBER": "6", "SLIME_FABRIC_PROFILE": "test"}, []),
    (
        "dango",
        {
            "SLIME_GENERATION_NUMBER": "7",
            "SLIME_DANGO_CHECK": "1",
            "SLIME_FABRIC_PROFILE": "test",
        },
        [],
    ),
    (
        "generation-commands",
        {
            "SLIME_GENERATION_NUMBER": "8",
            "SLIME_GENERATION_CMD_CHECK": "1",
            "SLIME_FABRIC_PROFILE": "test",
        },
        [],
    ),
    (
        "powerbox",
        {
            "SLIME_GENERATION_NUMBER": "9",
            "SLIME_POWERBOX_CHECK": "1",
            "SLIME_FABRIC_PROFILE": "test",
        },
        [],
    ),
    (
        "sample-plane",
        {
            "SLIME_GENERATION_NUMBER": "10",
            "SLIME_SAMPLE_PLANE_CHECK": "1",
            "SLIME_FABRIC_PROFILE": "test",
        },
        [],
    ),
    (
        "fabric-authority",
        {
            "SLIME_GENERATION_NUMBER": "11",
            "SLIME_FABRIC_AUTHORITY_CHECK": "1",
            "SLIME_FABRIC_PROFILE": "test",
        },
        [],
    ),
    (
        "fabric-stream",
        {
            "SLIME_GENERATION_NUMBER": "12",
            "SLIME_FABRIC_STREAM_CHECK": "1",
            "SLIME_FABRIC_PROFILE": "test",
        },
        [],
    ),
    (
        "fabric-qos",
        {
            "SLIME_GENERATION_NUMBER": "13",
            "SLIME_FABRIC_QOS_CHECK": "1",
            "SLIME_FABRIC_PROFILE": "test",
        },
        [],
    ),
    (
        "fabric-call",
        {
            "SLIME_GENERATION_NUMBER": "14",
            "SLIME_FABRIC_CALL_CHECK": "1",
            "SLIME_FABRIC_PROFILE": "test",
        },
        [],
    ),
    (
        "fabric-operation",
        {
            "SLIME_GENERATION_NUMBER": "15",
            "SLIME_FABRIC_OPERATION_CHECK": "1",
            "SLIME_FABRIC_PROFILE": "test",
        },
        [],
    ),
    (
        "fabric-visibility",
        {
            "SLIME_GENERATION_NUMBER": "16",
            "SLIME_FABRIC_VISIBILITY_CHECK": "1",
            "SLIME_FABRIC_PROFILE": "visibility",
        },
        [],
    ),
    (
        "fabric-boot",
        {
            "SLIME_GENERATION_NUMBER": "17",
            "SLIME_FABRIC_BOOT_CHECK": "1",
            "SLIME_FABRIC_PROFILE": "unified",
        },
        [],
    ),
    # Gen 99 is the rollback/bootstate known-good pair. `just rollback_check`
    # and `just bootstate_trace_check` boot it, and it is the layout that
    # catches a builder emitting one generation's resource into both.
    ("bootstate", {"SLIME_GENERATION_NUMBER": "99", "SLIME_FABRIC_PROFILE": "test"}, []),
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


def capture(name: str, settings: dict[str, str], qemu_args: list[str], image: Path) -> str:
    """Boot one profile and return its `[layout]` block."""
    environment = os.environ.copy()
    for flag in GATE_FLAGS:
        environment.pop(flag, None)
    environment.update(settings)
    arguments = [value.format(image=image) for value in qemu_args]
    output = run_qemu(
        ["cargo", "run", "--release", "--", "-display", "none", *arguments],
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
        unknown = wanted - {name for name, _, _ in PROFILES}
        if unknown:
            raise SystemExit(f"unknown profile(s): {', '.join(sorted(unknown))}")
        selected = [entry for entry in PROFILES if entry[0] in wanted]

    FIXTURES.mkdir(parents=True, exist_ok=True)
    failures: list[str] = []
    with tempfile.TemporaryDirectory() as work:
        image = Path(work) / "storage.img"
        if any(qemu_args for _, _, qemu_args in selected):
            subprocess.run(
                [ROOT / "scripts" / "build" / "build-storage-fixture.py", image],
                check=True,
            )
        failures = run_profiles(selected, image, arguments.bless)

    if failures:
        for line in failures:
            print(line)
        raise SystemExit("boot layout check: layouts moved")
    if arguments.bless:
        print("boot layout check: blessed")
        return
    print("boot layout check: ok")


def run_profiles(
    selected: list[tuple[str, dict[str, str], list[str]]],
    image: Path,
    bless: bool,
) -> list[str]:
    failures: list[str] = []
    for name, settings, qemu_args in selected:
        observed = capture(name, settings, qemu_args, image)
        fixture = FIXTURES / f"{name}.layout"
        if bless:
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
    return failures


if __name__ == "__main__":
    main()
