#!/usr/bin/env python3
"""IO0 gate: two supervised components exchange work through the shared queue."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
from closure_image import ClosureImageError, build as build_closure_image  # noqa: E402
from harness import sha256_file  # noqa: E402

from sel4_gate_markers import match_marker_contract  # noqa: E402
from sel4_plane import run_plane  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS = ROOT / "sel4" / "pins.toml"
# The closure identity names the build's inputs and is re-resolved from repository
# state before building, so stale input is refused instead of silently changing the image.
CLOSURE = "sel4-io-queue"
IMAGE: Path | None = None
FIXTURE = ROOT / "contracts" / "generation-manifest" / "v1" / "compositions" / "sel4-io-queue.zti"
TIMEOUT = 240

CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "shared mapping and round trip",
        (
            r"SLIME_ROOT generation admitted number=49 executables=3 instances=3 grants=2 ",
            r"SLIME_GRAPH loan created task=\d+ slot=\d+ id=\d+ to=\d+ offset=0 length=4096",
            r"\[io-queue-driver\] round trip drained=4 echoed=4",
            r"\[io-queue-client\] round trip echoes=4 drained=all",
        ),
    ),
    (
        "bounded submission and late completion refusal",
        (
            r"\[io-queue-client\] backpressure full refused overwrite=0",
            r"\[io-queue-driver\] duplicate completion published",
            r"\[io-queue-client\] unknown completion refused",
        ),
    ),
    (
        "driver reset settles every lease before advancing",
        (
            r"\[io-queue-driver\] reset settled=2 leases=2",
            r"\[io-queue-driver\] fresh epoch active",
        ),
    ),
    (
        "client observes reset and refuses its old epoch",
        (
            r"\[io-queue-client\] driver resetting observed",
            r"\[io-queue-client\] fresh epoch observed old epoch refused",
        ),
    ),
    (
        "malformed slice and terminal cleanup",
        (
            r"\[io-queue-client\] malformed slice refused before submission",
            r"\[io-queue-client\] io queue plane complete",
            r"SLIME_GRAPH loans served=\d+ loans=0 mappings=0 regions=0 orphans=0 quota=0",
            r"SLIME_GRAPH HEALTHY generation=49 required=3 live=0 completed=3 failed=0",
        ),
    ),
)

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_GRAPH FAIL",
    r"\[io-queue-client\] fail: ",
    r"\[io-queue-driver\] fail: ",
    r"Caught cap fault",
    r"Caught vm fault",
    r"panicked at ",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 I/O queue plane check: {message}")


def build_image() -> None:
    global IMAGE
    try:
        built = build_closure_image(CLOSURE)
    except ClosureImageError as error:
        fail(str(error))
    IMAGE = built.image
    actual = sha256_file(IMAGE, fail)
    if actual != built.digest():
        fail(f"{IMAGE} SHA-256 is {actual}, but the build result records {built.digest()}; the image changed after it was built")


def check_transcript(transcript: str) -> None:
    match_marker_contract(transcript, CHAINS, FAILURE_MARKERS, fail)


def check_fixture() -> None:
    text = FIXTURE.read_text(encoding="utf-8")
    for declaration in (
        "generation = 49;",
        'name = "io-queue-client";',
        'name = "io-queue-driver";',
        'name = "io-queue-request-ready";',
        'name = "io-queue-completion-ready";',
        'name = "io-queue-state-changed";',
    ):
        if declaration not in text:
            fail(f"fixture is missing {declaration!r}")


def main() -> None:
    parser = argparse.ArgumentParser(description="Boot and check the seL4 I/O queue proof plane")
    parser.add_argument("--no-build", action="store_true")
    arguments = parser.parse_args()
    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    check_fixture()
    if not arguments.no_build:
        build_image()
    terminal = re.compile(CHAINS[-1][1][-1] + "|" + "|".join(FAILURE_MARKERS))
    transcript = run_plane(
        image=IMAGE,
        timeout=TIMEOUT,
        terminal_condition=terminal,
        fail=fail,
        pins_path=PINS,
    )
    check_transcript(transcript)
    print(
        "seL4 I/O queue plane check: round trip, backpressure, late completion, reset epoch, and slice refusal proved"
    )


if __name__ == "__main__":
    main()
