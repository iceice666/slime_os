#!/usr/bin/env python3
"""IO1 gate: generation-scoped userspace hardware authority under seL4."""

from __future__ import annotations

import argparse
import copy
import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
import threading
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from harness import (
    GENERATION_COMPOSITIONS,
    load_qemu_profile,
    profile_integer,
    profile_text,
    sha256_file,
)  # noqa: E402
from sel4_gate_markers import match_marker_contract  # noqa: E402
from zutai_cli import STDLIB, binary  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS = ROOT / "sel4" / "pins.toml"
IMAGE = ROOT / "build" / "slime-sel4-io-driver-authority.elf"
MANIFEST = ROOT / "build" / "slime-sel4-io-driver-authority.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
GENERATOR = ROOT / "scripts" / "build" / "build-generation.py"
FIXTURE = GENERATION_COMPOSITIONS / "sel4-io-driver-authority.zti"
AUTOMATIC_BINDING_SLOTS = {
    "io-driver-worker-executable": 0,
    "probe-device": 1,
    "probe-mmio": 2,
}
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
            r"\[io-driver-probe\] stale epoch map refused=1",
            r"\[io-driver-probe\] shared-granule direct map refused not widened",
            r"\[io-driver-probe\] qemu packed transport mediated exact range proven",
            r"\[io-driver-probe\] interrupt spoof refused=1",
            r"\[io-driver-probe\] dma token differs from device physical address=1",
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
        (r"\[io-driver-intruder\] denied device=1 mmio=2 dma=2 interrupt=1",),
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


def generator_module(name: str):
    spec = importlib.util.spec_from_file_location(name, GENERATOR)
    if spec is None or spec.loader is None:
        fail("cannot import the generation builder")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def check_automatic_binding_slots() -> None:
    """Omitted supervisor bindings must resolve to the frozen authority layout."""
    environment = dict(os.environ)
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    process = subprocess.run(
        [str(binary()), "json", str(FIXTURE)],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode:
        fail(f"cannot decode {FIXTURE.relative_to(ROOT)}: {process.stderr.strip()}")
    try:
        manifest = json.loads(process.stdout)
    except json.JSONDecodeError as error:
        fail(f"cannot parse decoded {FIXTURE.relative_to(ROOT)}: {error}")
    supervisor = next(
        (
            instance
            for instance in manifest["instances"]
            if instance["name"] == "io-driver-supervisor"
        ),
        None,
    )
    if supervisor is None:
        fail("generation declares no io-driver-supervisor instance")
    bindings = {binding["grant"]: binding for binding in supervisor["bindings"]}
    for grant in AUTOMATIC_BINDING_SLOTS:
        binding = bindings.get(grant)
        if binding is None:
            fail(f"io-driver-supervisor does not bind {grant}")
        if "slot" in binding:
            fail(f"io-driver-supervisor/{grant} redundantly pins slot {binding['slot']}")

    resolved = generator_module("slime_build_generation_io_authority_slots").assign_declared_slots(
        copy.deepcopy(manifest)
    )
    resolved_supervisor = next(
        instance for instance in resolved["instances"] if instance["name"] == "io-driver-supervisor"
    )
    resolved_bindings = {
        binding["grant"]: binding["slot"] for binding in resolved_supervisor["bindings"]
    }
    for grant, expected in AUTOMATIC_BINDING_SLOTS.items():
        if resolved_bindings.get(grant) != expected:
            fail(
                f"io-driver-supervisor/{grant} resolved to slot "
                f"{resolved_bindings.get(grant)}, expected {expected}"
            )
    print(
        f"I/O authority manifest: {len(AUTOMATIC_BINDING_SLOTS)} supervisor "
        "binding slots omitted and resolved unchanged",
        flush=True,
    )


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
        "-drive",
        "if=none,file=/dev/zero,format=raw,id=d0",
        "-device",
        "virtio-blk-device,drive=d0",
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
    parser = argparse.ArgumentParser(
        description="Boot and check the seL4 I/O driver authority proof plane"
    )
    parser.add_argument("--no-build", action="store_true")
    arguments = parser.parse_args()
    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    check_fixture()
    check_automatic_binding_slots()
    if not arguments.no_build:
        build_image()
    check_manifest()
    profile = load_qemu_profile(fail, PINS)
    match_marker_contract(boot(profile), CHAINS, FAILURE_MARKERS, fail)
    print(
        "seL4 I/O driver authority plane check: exact mediated MMIO, bounded IRQ authority, and ungranted denial proved"
    )


if __name__ == "__main__":
    main()
