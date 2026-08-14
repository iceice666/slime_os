#!/usr/bin/env python3
"""B40: prove every child's CSpace matches the generation's admitted plan.

The root sizes each child CNode from the plan's CNode object and installs the
child's service, TCB, and fault capabilities at the slots the plan's cap
bindings name. That is only worth anything if a deviation is caught, so this
gate does two things:

1. Boots the unmutated boot plane and requires the graph to come to rest. Every
   child in it passed `audit_child_cspace`, which asks the kernel whether each
   slot in the CNode is occupied and compares that against the plan.

2. Rebuilds the root once per injected mutation and requires the audit to
   refuse it. A mutation that still reaches the terminal is a hole in the
   audit, so here a *successful* boot is the failure condition.

The mutations cover the five deviations B40 names — a declared capability
missing, an extra one in an undeclared slot, one of the wrong type, one
carrying wrong rights, and one aliased into two slots — plus wrong-slot, which
B40 does not name but which the plan-declared destinations make possible.

This is deliberately separate from `sel4_boot_check`: that gate asserts the
graph's behaviour, this one asserts the CSpace underneath it, and a layout
defect should read as a layout defect rather than as a component failing.
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts" / "lib"))

from harness import load_script  # noqa: E402

boot_plane = load_script("boot_plane", "check/check-sel4-boot-plane.py")

BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"

# The supervisor's terminal, and the refusal the audit prints. Both are reused
# from the boot-plane gate so the two cannot drift apart.
TERMINAL = re.compile(boot_plane.TERMINAL_MARKER)
REFUSAL = re.compile(r"CSpaceMismatch")

MUTATIONS = (
    ("missing", "a capability the plan declared was deleted"),
    ("extra", "a capability was installed into an undeclared slot"),
    ("wrong_type", "a slot holds a capability of the wrong type"),
    ("wrong_slot", "a declared capability was installed at the wrong slot"),
    ("aliased", "one capability was made reachable at two slots"),
    ("wrong_rights", "a capability was installed with broader rights"),
)


def fail(message: str) -> None:
    raise SystemExit(f"capability layout check: {message}")


def build(mutation: str | None) -> None:
    environment = dict(os.environ)
    environment.pop("SLIME_B40_MUTATION", None)
    if mutation is not None:
        environment["SLIME_B40_MUTATION"] = mutation
    command = [sys.executable, str(BUILD_SCRIPT), "--boot-plane"]
    process = subprocess.run(
        command,
        cwd=ROOT,
        env=environment,
        check=False,
        capture_output=True,
        text=True,
    )
    if process.returncode != 0:
        tail = (process.stdout + process.stderr)[-2000:]
        fail(f"build failed for mutation {mutation!r}\n{tail}")


def main() -> int:
    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    pins = boot_plane.load_pins()
    profile = pins["qemu_arm_virt"]

    build(None)
    transcript = boot_plane.boot(profile)
    if not TERMINAL.search(transcript):
        fail("the unmutated graph never reached the supervisor terminal")
    if REFUSAL.search(transcript):
        fail("the unmutated graph was refused by its own CSpace audit")
    print("capability layout check: every child CSpace matches the admitted plan")

    # Whatever happens below, the last build must be unmutated: every other
    # seL4 gate boots `build/slime-sel4-boot.elf`, and leaving a mutated image
    # there would fail them for a reason that has nothing to do with them.
    try:
        check_mutations(profile)
    finally:
        build(None)
    print(f"capability layout check: all {len(MUTATIONS)} negative mutations refused")
    return 0


def check_mutations(profile: dict[str, object]) -> None:
    for mutation, description in MUTATIONS:
        build(mutation)
        # A refused CSpace is a root fatal, and the boot-plane helper turns a
        # failure marker into SystemExit. Here that outcome is the expected
        # one, so the transcript is recovered from the exception rather than
        # letting it end the gate.
        try:
            transcript = boot_plane.boot(profile)
        except SystemExit:
            # `boot` clears LAST_TRANSCRIPT on entry, so an empty one means it
            # failed before the guest produced anything — a missing QEMU or a
            # launch failure, not a refusal. Treating that as a pass would turn
            # every remaining mutation into a free one.
            transcript = boot_plane.LAST_TRANSCRIPT
            if not transcript:
                fail(
                    f"the guest produced no output for mutation {mutation!r}; "
                    "the boot never ran"
                )
        if TERMINAL.search(transcript):
            fail(
                f"the audit accepted a mutated CSpace: {description} "
                f"(--cfg slime_b40_mutate_{mutation})"
            )
        if not REFUSAL.search(transcript):
            fail(
                f"the mutation was not refused as a CSpace mismatch: {description} "
                f"(--cfg slime_b40_mutate_{mutation})"
            )
        print(f"capability layout check: refused {description}")



if __name__ == "__main__":
    raise SystemExit(main())
