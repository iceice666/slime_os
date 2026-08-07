#!/usr/bin/env python3

"""B11 product boot gate: the shipped profile declares no test scaffolding.

Every other QEMU gate in this repository boots a profile that declares
verification scaffolding — the storage, directory, powerbox, and sample probes,
the fabric's `-b` participants, the unauthorized probe, and the interposition
doubles. That scaffolding is what those gates exist to exercise, so none of them
can answer the question B11 asks: does the generation the *product* ships still
boot when none of it is declared?

This gate boots that profile and requires three things at once:

- the vertical slice reaches its healthy exit condition, so removing the
  scaffolding did not remove something the product needs;
- no scaffolding component appears anywhere in the transcript, so "the product
  profile declares none of it" is observed rather than inferred from the
  manifest; and
- the layout the kernel resolved holds no scaffolding slot, which is the same
  claim one level down — `boot_layout_check` freezes the slot table, and this
  checks that what filled it is scaffolding-free.

Failure here means the product profile is either unbootable or is still carrying
test participants; both are B11 regressions.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))
_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "build"))

import os
import re

from harness import ROOT, load_script, run_qemu

builder = load_script("build_generation_product", "build/build-generation.py")

# Every component assigned to a non-product boot profile is verification
# scaffolding. Derive the set from the manifest so adding another test-only
# participant automatically extends this gate.
MANIFEST = builder.load_manifest()
SCAFFOLDING = sorted(
    {
        component
        for profile in MANIFEST["bootProfiles"]
        for component in profile["components"]
    }
)

# The product boot must reach the same healthy exit condition every other
# non-scenario gate reaches.
REQUIRED = [
    "[generation] decoded generation 1",
    "[generation] bootstrap grants valid",
    "[init] spawn graph launched",
    "[generation] vertical slice healthy",
]

# A component that dies unexpectedly, or a slice that reports unhealthy, must
# fail even though the healthy marker is what we key on.
FORBIDDEN = [
    "[generation] vertical slice unhealthy",
    "[panic]",
]


def fail(message: str) -> None:
    raise SystemExit(f"product boot check: {message}")


def boot() -> str:
    environment = os.environ.copy()
    # The product profile, named explicitly. Every gate flag that would select a
    # scaffolding profile is cleared, so an inherited environment cannot quietly
    # turn this into one of the other gates.
    for flag in (
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
        "SLIME_TRANSFER_RECEIVER",
        "SLIME_TRANSFER_ACTIVATE",
        "SLIME_RECOVERY_IMAGE",
        "SLIME_INTERACTIVE",
    ):
        environment.pop(flag, None)
    environment["SLIME_GENERATION_NUMBER"] = "1"
    environment["SLIME_FABRIC_PROFILE"] = "default"
    return run_qemu(
        ["cargo", "run", "--release", "--", "-display", "none"],
        environment=environment,
        cwd=ROOT / "kernel",
        timeout=300,
        echo="on-error",
    )


def check_markers(output: str) -> None:
    cursor = 0
    for marker in REQUIRED:
        found = output.find(marker, cursor)
        if found < 0:
            fail(f"missing marker {marker!r}")
        cursor = found + len(marker)
    for marker in FORBIDDEN:
        if marker in output:
            fail(f"product boot reported {marker!r}")


def check_no_scaffolding(output: str) -> None:
    """No scaffolding component is named anywhere in the product transcript.

    The kernel's health sweep names every component it launched, and `[layout]`
    names every slot it filled, so a probe that survived into this profile shows
    up here even if it never emitted a marker of its own.
    """
    for component in SCAFFOLDING:
        if re.search(rf"\b{re.escape(component)}\b", output):
            fail(f"product boot names the scaffolding component {component!r}")


def check_layout(output: str) -> None:
    rows = [line.split() for line in output.splitlines() if line.startswith("[layout] ")]
    # `>= 5`, not `== 5`: B26 appends a sixth `declared=0x…` field to a row
    # whose layout rights differ from the installed ones, and a row carrying it
    # is still a slot this check must see — dropping it would let a scaffolding
    # component hide from the label scan below. Unreachable today, since this
    # parses an x86 boot and `dump_boot_layout` does not emit the field.
    slots = [row for row in rows if len(row) >= 5 and row[1].isdigit()]
    if not slots:
        fail("product boot emitted no layout dump")
    labels = {row[3] for row in slots}
    for component in SCAFFOLDING:
        if component in labels:
            fail(f"product layout still declares a slot for {component!r}")
    return len(slots)


def main() -> None:
    output = boot()
    check_markers(output)
    check_no_scaffolding(output)
    slots = check_layout(output)
    print(
        f"product boot: healthy vertical slice in {slots} capability slots, "
        f"none of the {len(SCAFFOLDING)} scaffolding components declared"
    )


if __name__ == "__main__":
    main()
