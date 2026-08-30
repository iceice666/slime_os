#!/usr/bin/env python3
"""IO4 gate: destination-scoped network authority under seL4."""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from sel4_gate_markers import match_marker_contract  # noqa: E402
from sel4_plane import run_plane, verify_image_identity  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS = ROOT / "sel4" / "pins.toml"
IMAGE = ROOT / "build" / "slime-sel4-io-network.elf"
MANIFEST = ROOT / "build" / "slime-sel4-io-network.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
FIXTURE = ROOT / "contracts" / "generation-manifest" / "v1" / "compositions" / "sel4-io-network.zti"
IMAGE_VARIANT = "io-network"
TIMEOUT = 240

CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "admission",
        (
            r"SLIME_ROOT generation admitted number=53 executables=5 instances=5 grants=3 ",
            r"\[network-service\] authority destinations=5 rights=connect,send,recv",
            r"\[network-service\] declared socket_limit=7 listener_limit=0 dns_record_limit=2",
        ),
    ),
    (
        "loopback honesty",
        (r"\[io-link-loopback\] declared endpoint bindings=1 protocol operations=0",),
    ),
    (
        "granted path",
        (
            r"\[io-network-probe\] tcp capabilities=1 rights=connect,send,recv",
            r"\[io-network-probe\] successful capability operations=2",
            r"\[io-network-probe\] exact destination refusals=1",
            r"\[io-network-probe\] dns records=1 budget_refusals=1",
            r"\[io-network-probe\] socket charges=2 budget_refusals=1",
            r"\[io-network-probe\] closed capabilities=4 shutdown=1",
        ),
    ),
    (
        "denials",
        (
            r"\[io-network-intruder\] exact authority refusals=8",
            r"\[io-network-intruder\] cross-holder capability refusals=4",
            r"\[io-network-intruder\] rights-mask refusals=2",
            r"\[io-network-intruder\] structured denials=14 shutdown=1",
        ),
    ),
    (
        "service close",
        (
            r"\[network-service\] observed requests=33 packets=7 socket_refusals=1 listener_refusals=0 dns_refusals=1 cross_holder_refusals=4",
            r"SLIME_GRAPH HEALTHY generation=53 required=5 live=0 completed=5 failed=0",
        ),
    ),
)
FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_GRAPH FAIL",
    r"\[network-service\] fail: ",
    r"\[io-network-probe\] fail: ",
    r"\[io-network-intruder\] fail: ",
    r"Caught cap fault",
    r"Caught vm fault",
    r"panicked at ",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 I/O network plane check: {message}")


def build_image() -> None:
    process = subprocess.run(
        [sys.executable, str(BUILD_SCRIPT), "--io-network-plane"],
        cwd=ROOT,
        check=False,
    )
    if process.returncode != 0:
        fail(f"image build failed with exit status {process.returncode}")


def check_fixture() -> None:
    text = FIXTURE.read_text(encoding="utf-8")
    for pattern in (
        r"generation\s*=\s*53;",
        r"networkDestinations\s*=\s*\[",
        r'name\s*=\s*"network-service"',
        r'name\s*=\s*"io-network-probe"',
        r'name\s*=\s*"io-network-intruder"',
        r'name\s*=\s*"io-link-loopback"',
    ):
        if re.search(pattern, text) is None:
            fail(f"fixture is missing {pattern!r}")


def main() -> None:
    parser = argparse.ArgumentParser(description="Boot and check the seL4 I/O network proof plane")
    parser.add_argument("--no-build", action="store_true")
    arguments = parser.parse_args()
    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    check_fixture()
    if not arguments.no_build:
        build_image()
    verify_image_identity(
        image=IMAGE,
        manifest=MANIFEST,
        variant=IMAGE_VARIANT,
        fail=fail,
    )
    terminal = re.compile(CHAINS[-1][1][-1] + "|" + "|".join(FAILURE_MARKERS))
    transcript = run_plane(
        image=IMAGE,
        timeout=TIMEOUT,
        terminal_condition=terminal,
        fail=fail,
        pins_path=PINS,
    )
    match_marker_contract(transcript, CHAINS, FAILURE_MARKERS, fail)
    print(
        "seL4 I/O network plane check: exact authority, per-destination budgets, "
        "structured denials, and honest backend absence proved"
    )


if __name__ == "__main__":
    main()
