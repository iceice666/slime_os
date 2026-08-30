#!/usr/bin/env python3
"""IO0 gate: two supervised components exchange work through the shared queue."""
from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
import threading
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from harness import load_qemu_profile, profile_integer, profile_text, sha256_file  # noqa: E402
from sel4_gate_markers import match_marker_contract  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS = ROOT / "sel4" / "pins.toml"
IMAGE = ROOT / "build" / "slime-sel4-io-queue.elf"
MANIFEST = ROOT / "build" / "slime-sel4-io-queue.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
FIXTURE = ROOT / "contracts" / "generation-manifest" / "v1" / "compositions" / "sel4-io-queue.zti"
IMAGE_VARIANT = "io-queue"
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
    process = subprocess.run(
        [sys.executable, str(BUILD_SCRIPT), "--io-queue-plane"],
        cwd=ROOT,
        check=False,
    )
    if process.returncode != 0:
        fail(f"image build failed with exit status {process.returncode}")


def check_manifest() -> None:
    if not IMAGE.is_file() or not MANIFEST.is_file():
        fail("image or identity manifest missing")
    try:
        identity = json.loads(MANIFEST.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot parse identity manifest: {error}")
    if identity.get("variant") != IMAGE_VARIANT:
        fail(f"wrong image variant {identity.get('variant')!r}")
    image = identity.get("image")
    if not isinstance(image, dict) or image.get("sha256") != sha256_file(IMAGE, fail):
        fail("packaged image digest does not match identity manifest")


def boot(profile: dict[str, object]) -> str:
    qemu = shutil.which("qemu-system-aarch64")
    if qemu is None:
        fail("qemu-system-aarch64 is not on PATH")
    command = [
        qemu,
        "-machine",
        profile_text(profile, "machine", fail),
        "-cpu",
        profile_text(profile, "cpu", fail),
        "-smp",
        str(profile_integer(profile, "cpus", fail)),
        "-m",
        f"size={profile_integer(profile, 'memory_mib', fail)}M",
        "-nographic",
        "-serial",
        "mon:stdio",
        "-kernel",
        str(IMAGE),
    ]
    process = subprocess.Popen(
        command,
        cwd=ROOT,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    watchdog = threading.Timer(TIMEOUT, process.kill)
    watchdog.start()
    lines: list[str] = []
    terminal = re.compile(CHAINS[-1][1][-1] + "|" + "|".join(FAILURE_MARKERS))
    try:
        assert process.stdout is not None
        for line in process.stdout:
            lines.append(line.rstrip("\n"))
            if terminal.search(line):
                break
    finally:
        timed_out = not watchdog.is_alive()
        watchdog.cancel()
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
    transcript = "\n".join(lines)
    if timed_out and re.search(CHAINS[-1][1][-1], transcript) is None:
        fail("QEMU timed out before terminal cleanup")
    return transcript


def check_transcript(transcript: str) -> None:
    match_marker_contract(transcript, CHAINS, FAILURE_MARKERS, fail)


def check_fixture() -> None:
    text = FIXTURE.read_text(encoding="utf-8")
    for declaration in (
        'generation = 49;',
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
    check_manifest()
    profile = load_qemu_profile(fail, PINS)
    check_transcript(boot(profile))
    print("seL4 I/O queue plane check: round trip, backpressure, late completion, reset epoch, and slice refusal proved")


if __name__ == "__main__":
    main()
