#!/usr/bin/env python3
"""IO1 gate: generation-scoped userspace hardware authority under seL4."""
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
IMAGE = ROOT / "build" / "slime-sel4-io-driver-authority.elf"
MANIFEST = ROOT / "build" / "slime-sel4-io-driver-authority.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
FIXTURE = ROOT / "contracts" / "generation-manifest" / "v1" / "compositions" / "sel4-io-driver-authority.zti"
IMAGE_VARIANT = "io-driver-authority"
TIMEOUT = 240

# Concurrent components have independent chains. Ordering is asserted only where
# the program itself establishes it, never across scheduler-dependent streams.
CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "authority admitted and bounded",
        (
            r"SLIME_ROOT generation admitted number=50 executables=3 instances=4 grants=5 ",
            r"SLIME_IO quota task=\d+ instance=io-driver-worker devices=1 shared_granule=0",
        ),
    ),
    (
        "granted driver receives only bounded authority",
        (
            r"\[io-driver-probe\] bind exactly one device proven",
            r"\[io-driver-probe\] shared-granule direct map refused not widened",
            r"\[io-driver-probe\] qemu packed transport mediated exact range proven",
            r"\[io-driver-probe\] declared interrupt bound no-spoof proven",
            r"\[io-driver-probe\] opaque dma path exposes no physical address proven",
            r"\[io-driver-probe\] faulting with live authority",
            r"SLIME_IO reclaim task=\d+ pre_mmio_bytes=4096 pre_mmio_mappings=1 pre_irq_sources=1 pre_dma_pages=2 pre_dma_mappings=1 pre_requests=0 reclaimed_mmio_bytes=4096 reclaimed_mmio_mappings=1 reclaimed_irq_sources=1 reclaimed_dma_pages=2 reclaimed_dma_mappings=1 settled_requests=0 post_mmio_bytes=0 post_mmio_mappings=0 post_irq_sources=0 post_dma_pages=0 post_dma_mappings=0 post_requests=0 actions=3 fresh_epoch=2",
            r"\[io-driver-probe\] fresh epoch=2",
            r"\[io-driver-probe\] predecessor epoch refused=1",
            r"\[io-driver-supervisor\] replacement completed",
            r"\[io-driver-probe\] io driver authority plane complete",
        ),
    ),
    (
        "ungranted component is denied without fault",
        (r"\[io-driver-intruder\] device mmio dma interrupt denials proven",),
    ),
    (
        "terminal cleanup",
        (r"SLIME_GRAPH HEALTHY generation=50 required=3 live=0 completed=3 failed=0",),
    ),
)

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_GRAPH FAIL",
    r"SLIME_IO FAIL",
    r"\[io-driver-probe\] fail: ",
    r"\[io-driver-intruder\] fail: ",
    r"Caught cap fault",
    r"Caught vm fault",
    r"panicked at ",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 I/O driver authority plane check: {message}")


def build_image() -> None:
    process = subprocess.run(
        [sys.executable, str(BUILD_SCRIPT), "--io-driver-authority-plane"],
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
        "-machine", profile_text(profile, "machine", fail),
        "-cpu", profile_text(profile, "cpu", fail),
        "-smp", str(profile_integer(profile, "cpus", fail)),
        "-m", f"size={profile_integer(profile, 'memory_mib', fail)}M",
        "-nographic", "-serial", "mon:stdio", "-kernel", str(IMAGE),
        "-drive", "if=none,file=/dev/zero,format=raw,id=d0",
        "-device", "virtio-blk-device,drive=d0",
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


def check_fixture() -> None:
    text = FIXTURE.read_text(encoding="utf-8")
    for declaration in (
        "generation = 50;",
        'name = "io-driver-probe";',
        'name = "io-driver-intruder";',
        'capabilityKind = "device";',
        'capabilityKind = "mmioRegion";',
        'capabilityKind = "interruptSource";',
        'capabilityKind = "dmaAccount";',
    ):
        if declaration not in text:
            fail(f"fixture is missing {declaration!r}")


def main() -> None:
    parser = argparse.ArgumentParser(description="Boot and check the seL4 I/O driver authority proof plane")
    parser.add_argument("--no-build", action="store_true")
    arguments = parser.parse_args()
    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    check_fixture()
    if not arguments.no_build:
        build_image()
    check_manifest()
    profile = load_qemu_profile(fail, PINS)
    match_marker_contract(boot(profile), CHAINS, FAILURE_MARKERS, fail)
    print("seL4 I/O driver authority plane check: exact mediated MMIO, bounded IRQ authority, and ungranted denial proved")


if __name__ == "__main__":
    main()
