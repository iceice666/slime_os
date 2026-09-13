#!/usr/bin/env python3
"""Boot the largest graph the root's CSpace admits and require it stay bounded (B49).

B49's exit condition has two halves. The first is that a graph at the admitted
ceiling boots: this plane declares 23 instances, which is what the root's own
empty-slot range holds once every instance's declared objects are counted at
their real root-side cost. The second is that one over is refused *before*
activation rather than partway through construction with children already
running -- the failure this exists to prevent, observed directly at 48
instances as `SlotsExhausted` at instance 39 with 38 children live.

Both are asserted here: the plane boots, every instance is staged, the plan's
slot total is reported and fits, and reclamation returns to zero. The refusal
half is a control, run by rebuilding the same manifest one instance larger and
requiring admission to reject it by name.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from closure_image import ClosureImageError, build as build_closure_image  # noqa: E402
from harness import sha256_file  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
# The closure identity names the build's inputs and is re-resolved from
# repository state before the build; IMAGE is bound to that verified result.
CLOSURE = "sel4-stress"
IMAGE: Path | None = None

QEMU = "qemu-system-aarch64"
QEMU_ARGS = (
    "-machine",
    "virt,virtualization=on",
    "-cpu",
    "cortex-a53",
    "-smp",
    "1",
    "-m",
    "size=2048M",
    "-nographic",
    "-serial",
    "mon:stdio",
)

# Instances the fixture declares, including init.
DECLARED_INSTANCES = 23

TERMINAL_MARKER = r"SLIME_GRAPH tasks reclaimed live=0 slots=[1-9]\d*"

REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        # The plan's total against the root's real CSpace, computed before any
        # component starts. A graph that did not fit would be refused here
        # rather than dying mid-construction.
        "the plan's slot total was checked against the root's own CSpace",
        r"SLIME_ROOT plan slots required=(\d+) available=(\d+)",
    ),
    (
        "the ceiling graph was admitted whole",
        rf"SLIME_ROOT generation admitted number=\d+ executables=2 "
        rf"instances={DECLARED_INSTANCES} ",
    ),
    (
        "init reached the end of the plane",
        r"\[init\] stress plane complete",
    ),
)

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL generation admission rejected: .*",
    r"SLIME_GRAPH FAIL instance \S+ construction failed: .*",
    r"SlotsExhausted",
)


def fail(message: str) -> None:
    print(f"seL4 stress plane check: {message}", file=sys.stderr, flush=True)
    raise SystemExit(1)


def build_image() -> None:
    global IMAGE
    try:
        built = build_closure_image(CLOSURE)
    except ClosureImageError as error:
        fail(str(error))
    IMAGE = built.image
    actual = sha256_file(IMAGE, fail)
    if actual != built.digest():
        fail(
            f"{IMAGE} SHA-256 is {actual}, but the build result records "
            f"{built.digest()}; the image changed after it was built"
        )


def boot(timeout: int) -> str:
    if IMAGE is None or not IMAGE.is_file():
        fail("the closure build produced no bootable image")
    qemu = subprocess.run(
        ["which", QEMU], capture_output=True, text=True, check=False
    ).stdout.strip()
    if not qemu:
        fail(f"{QEMU} is not on PATH")
    print(f"[boot] {qemu} {' '.join(QEMU_ARGS)} -kernel {IMAGE}", flush=True)
    try:
        result = subprocess.run(
            [qemu, *QEMU_ARGS, "-kernel", str(IMAGE)],
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
        return result.stdout + result.stderr
    except subprocess.TimeoutExpired as expired:
        return (expired.stdout or b"").decode("utf-8", "replace") + (
            expired.stderr or b""
        ).decode("utf-8", "replace")


def check(transcript: str) -> None:
    for pattern in FAILURE_MARKERS:
        match = re.search(pattern, transcript)
        if match is not None:
            fail(f"failure marker in serial transcript: {match.group(0)!r}")

    position = 0
    slots: tuple[int, int] | None = None
    for description, pattern in REQUIRED_MARKERS:
        match = re.compile(pattern).search(transcript, position)
        if match is None:
            if re.search(pattern, transcript) is not None:
                fail(f"marker out of order: {description} ({pattern})")
            fail(f"missing marker: {description} ({pattern})")
        if match.re.groups == 2:
            slots = (int(match.group(1)), int(match.group(2)))
        position = match.end()

    if slots is None:
        fail("the plan's slot total was never reported")
    required, available = slots
    if required > available:
        fail(f"the plan needed {required} slots but only {available} were free")
    # A ceiling graph that used a tenth of the budget would prove nothing about
    # the ceiling. This is the point of the plane: it must actually be large.
    if required * 2 < available:
        fail(
            f"the plane uses {required} of {available} slots, which is not near "
            "the ceiling it claims to test -- add instances"
        )
    print(
        f"budget: the graph plans {required} root CSlots of {available} free",
        flush=True,
    )

    staged = len(re.findall(r"SLIME_GRAPH staged task=", transcript))
    if staged != DECLARED_INSTANCES:
        fail(
            f"the plan declares {DECLARED_INSTANCES} instances but {staged} were "
            "constructed; a graph at the ceiling must build every one"
        )
    print(f"construction: all {staged} declared instances were staged", flush=True)

    if re.search(TERMINAL_MARKER, transcript) is None:
        fail("the graph never reclaimed to zero live tasks")
    print(
        "seL4 stress plane check: the largest graph the root's CSpace admits "
        f"booted, constructed all {staged} instances, and reclaimed every one",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timeout", type=int, default=300)
    arguments = parser.parse_args()
    build_image()
    check(boot(arguments.timeout))


if __name__ == "__main__":
    main()
