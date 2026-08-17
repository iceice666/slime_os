#!/usr/bin/env python3
"""B35 persistent disk-backed selection across fresh QEMU boots."""
from __future__ import annotations

import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import tomllib
from pathlib import Path
from typing import NoReturn

ROOT = Path(__file__).resolve().parents[2]
BUILD = ROOT / "scripts/build/build-sel4.py"
GENERATOR = ROOT / "scripts/build/build-generation.py"
FIXTURE = ROOT / "scripts/build/build-store-fixture.py"
IMAGE = ROOT / "build/slime-sel4-boot-selection.elf"
IDENTITY = ROOT / "build/slime-sel4-boot-selection.identity.json"
PINS = ROOT / "sel4/pins.toml"
TIMEOUT = 240
STORE_FIRST = 40
SECTOR = 512


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 boot selection check: {message}")


def run(command: list[str], environment: dict[str, str] | None = None) -> None:
    process = subprocess.run(command, cwd=ROOT, env=environment, check=False)
    if process.returncode:
        fail(f"command failed ({process.returncode}): {' '.join(command)}")


def build_generation(output: Path, number: int, bundle: str, *, failing: bool = False) -> Path:
    environment = dict(os.environ)
    environment.update(
        SLIME_TARGET_PROFILE="aarch64-sel4-qemu-virt",
        SLIME_SEL4_MANIFEST="sel4",
        SLIME_GENERATION_NUMBER=str(number),
        SLIME_BOOT_BUNDLE_IDENTITY=bundle,
    )
    if failing:
        environment["SLIME_BOOT_SELECTION_FAIL"] = "1"
    run([sys.executable, str(GENERATOR), str(output)], environment)
    generation = output / "generation.bin"
    if not generation.is_file():
        fail(f"generation builder omitted {generation}")
    return generation


def restamp_wire_version(source: Path, destination: Path, magic: bytes, version: int) -> Path:
    """A generation whose *wire* header names a superseded format.

    B64: the repository retains `contracts/generation/v{2,3,4}` because the
    format's history is part of the contract, and `Generation::decode` refuses a
    v2/v3/v4 magic with `UnsupportedVersion` rather than `BadMagic` — a
    deliberate distinction between "an older Slime generation" and "not one".
    What no gate covered is the *consequence* for rollback: roadmap invariant 7
    requires that a failed pending generation cannot consume the last selectable
    boot root, and an undecodable pending generation is the sharpest case of a
    failed one. It never runs, so it cannot report itself unhealthy.

    Rewriting the header is the whole fixture: the selector reads the magic and
    version word before anything else, so the rest of the bytes are irrelevant to
    the refusal under test. The identity is recomputed by the caller's store
    builder, so this stays a well-formed store entry containing a generation the
    running root cannot decode.
    """
    blob = bytearray(source.read_bytes())
    if len(blob) < 12:
        fail("generation too short to restamp")
    blob[:8] = magic
    blob[8:12] = version.to_bytes(4, "little")
    destination.write_bytes(bytes(blob))
    return destination


def make_store(paths: list[Path], bundle: str, attempts: int) -> bytes:
    spec = importlib.util.spec_from_file_location("slime_build_generation", GENERATOR)
    if spec is None or spec.loader is None:
        fail("cannot import generation builder")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    saved = dict(os.environ)
    try:
        os.environ.update(
            SLIME_PENDING_GENERATION="1",
            SLIME_PENDING_ATTEMPTS=str(attempts),
            SLIME_KNOWN_GOOD_FIRST="1",
            SLIME_ACCEPTED_RELEASE_SEQUENCE="1",
            SLIME_PENDING_RELEASE_SEQUENCE="2",
            SLIME_BOOT_BUNDLE_IDENTITY=bundle,
        )
        return module.build_bootstore([path.read_bytes() for path in paths])
    finally:
        os.environ.clear()
        os.environ.update(saved)

def assert_oversize_rejected(bundle: str) -> None:
    spec = importlib.util.spec_from_file_location("slime_build_generation_oversize", GENERATOR)
    if spec is None or spec.loader is None:
        fail("cannot import generation builder for ceiling regression")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    saved = dict(os.environ)
    try:
        os.environ["SLIME_BOOT_BUNDLE_IDENTITY"] = bundle
        try:
            module.build_bootstore([bytes(module.SELECTOR_GENERATION_BYTES + 1)])
        except SystemExit as error:
            if "selector ceiling" not in str(error):
                raise
        else:
            fail("oversized selector generation was accepted")
    finally:
        os.environ.clear()
        os.environ.update(saved)


def make_disk(path: Path, store: bytes) -> None:
    blob = path.with_suffix(".store")
    blob.write_bytes(store)
    run([sys.executable, str(FIXTURE), str(path), "boot-selection", "--boot-store", str(blob)])


def qemu_profile() -> dict[str, object]:
    profile = tomllib.loads(PINS.read_text(encoding="utf-8"))["qemu_arm_virt"]
    if not isinstance(profile, dict):
        fail("qemu profile is not a table")
    return profile


def boot(disk: Path, terminal: str) -> str:
    qemu = shutil.which("qemu-system-aarch64")
    if qemu is None:
        fail("qemu-system-aarch64 is not on PATH")
    profile = qemu_profile()
    command = [
        qemu,
        "-machine", str(profile["machine"]),
        "-cpu", str(profile["cpu"]),
        "-smp", str(profile["cpus"]),
        "-m", f"size={profile['memory_mib']}M",
        "-nographic", "-serial", "mon:stdio", "-kernel", str(IMAGE),
        "-drive", f"if=none,id=slimedisk,format=raw,file={disk}",
        "-device", "virtio-blk-device,drive=slimedisk",
    ]
    process = subprocess.Popen(
        command, cwd=ROOT, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT, text=True, bufsize=1,
    )
    timer = threading.Timer(TIMEOUT, process.kill)
    timer.start()
    lines: list[str] = []
    reached = False
    try:
        assert process.stdout is not None
        for line in process.stdout:
            lines.append(line.rstrip())
            if "SLIME_ROOT FATAL" in line or "panicked at" in line:
                break
            if terminal in line:
                reached = True
                break
    finally:
        timer.cancel()
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
    transcript = "\n".join(lines)
    if not reached:
        sys.stderr.write("\n".join(lines[-50:]) + "\n")
        fail(f"boot did not reach {terminal}")
    return transcript


def boot_refused(disk: Path, refusal: str) -> str:
    """Boot once where a root *fatal* is the expected outcome.

    `boot` treats any `SLIME_ROOT FATAL` as a failed run, which is right for
    every arm whose candidate is supposed to start. B64's stale-format candidate
    cannot be decoded, so the correct observable is the refusal itself; reusing
    `boot` would report "did not reach" for a transcript that contains exactly
    what the arm requires.
    """
    qemu = shutil.which("qemu-system-aarch64")
    if qemu is None:
        fail("qemu-system-aarch64 is not on PATH")
    profile = qemu_profile()
    command = [
        qemu,
        "-machine", str(profile["machine"]),
        "-cpu", str(profile["cpu"]),
        "-smp", str(profile["cpus"]),
        "-m", f"size={profile['memory_mib']}M",
        "-nographic", "-serial", "mon:stdio", "-kernel", str(IMAGE),
        "-drive", f"if=none,id=slimedisk,format=raw,file={disk}",
        "-device", "virtio-blk-device,drive=slimedisk",
    ]
    process = subprocess.Popen(
        command, cwd=ROOT, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT, text=True, bufsize=1,
    )
    timer = threading.Timer(TIMEOUT, process.kill)
    timer.start()
    lines: list[str] = []
    reached = False
    try:
        assert process.stdout is not None
        for line in process.stdout:
            lines.append(line.rstrip())
            if refusal in line:
                reached = True
                break
            if "panicked at" in line:
                break
    finally:
        timer.cancel()
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
    transcript = "\n".join(lines)
    if not reached:
        sys.stderr.write("\n".join(lines[-50:]) + "\n")
        fail(f"boot did not refuse with {refusal}")
    return transcript


def expect(transcript: str, number: int, pending: int, attempts: int) -> None:
    pattern = rf"SLIME_BOOT selected .* number={number} pending={pending} attempts={attempts}"
    if re.search(pattern, transcript) is None:
        fail(f"missing marker {pattern}")


def only_slots(before: bytes, after: bytes) -> None:
    start = STORE_FIRST * SECTOR
    end = start + 2 * SECTOR
    if len(before) != len(after) or before[:start] != after[:start] or before[end:] != after[end:]:
        fail("bytes outside the redundant BootState slots changed")
    if before[start:end] == after[start:end]:
        fail("BootState did not change")


def main() -> None:
    if Path.cwd().resolve() != ROOT:
        fail("run from repository root")
    run([sys.executable, str(BUILD), "--boot-selection"])
    bundle = str(json.loads(IDENTITY.read_text(encoding="utf-8"))["boot_bundle_identity"])
    assert_oversize_rejected(bundle)
    with tempfile.TemporaryDirectory(prefix="slime-b35-") as temporary:
        work = Path(temporary)
        known_good = build_generation(work / "a", 1, bundle)
        failing = build_generation(work / "bad", 99, bundle, failing=True)
        healthy = build_generation(work / "good", 2, bundle)

        rollback = work / "rollback.img"
        make_disk(rollback, make_store([known_good, failing], bundle, 2))
        initial = rollback.read_bytes()
        first = boot(rollback, "SLIME_BOOT unhealthy")
        expect(first, 99, 1, 1)
        after_first = rollback.read_bytes()
        only_slots(initial, after_first)
        second = boot(rollback, "SLIME_BOOT unhealthy")
        expect(second, 99, 1, 0)
        after_second = rollback.read_bytes()
        only_slots(after_first, after_second)
        third = boot(rollback, "SLIME_BOOT selected")
        expect(third, 1, 0, 0)
        fallback = rollback.read_bytes()
        only_slots(after_second, fallback)
        stable = boot(rollback, "SLIME_BOOT selected")
        expect(stable, 1, 0, 0)
        if rollback.read_bytes() != fallback:
            fail("known-good fallback boot mutated disk")

        # B64: a pending generation the running root cannot decode. It never
        # runs, so it can never report itself unhealthy — the arm above proves
        # retry exhaustion for a generation that *does* run and fails, and this
        # one proves the same protection for one that cannot start. The attempt
        # is spent before the bytes are decoded (`boot_selector::select` consumes
        # it, commits the state, and only then reads the entry), which is what
        # bounds an undecodable candidate to its declared attempts instead of
        # letting it retry forever or take the known-good root with it.
        stale = restamp_wire_version(
            failing, work / "stale-generation.bin", b"SLIMEG4\0", 4
        )
        stale_disk = work / "stale.img"
        make_disk(stale_disk, make_store([known_good, stale], bundle, 1))
        stale_initial = stale_disk.read_bytes()
        # One declared attempt, so the pending candidate is refused and the very
        # next boot must already be the known-good root. The refusal is a root
        # fatal rather than a `SLIME_BOOT` record: the selector cannot report
        # which generation it selected when it could not decode one.
        stale_first = boot_refused(stale_disk, "boot selection rejected")
        if "SLIME_BOOT selected" in stale_first:
            fail("an undecodable pending generation was selected as if valid")
        stale_after = stale_disk.read_bytes()
        only_slots(stale_initial, stale_after)
        stale_fallback = boot(stale_disk, "SLIME_BOOT selected")
        expect(stale_fallback, 1, 0, 0)
        if "number=99" in stale_fallback:
            fail("the undecodable pending generation survived into the fallback boot")
        settled = stale_disk.read_bytes()
        only_slots(stale_after, settled)

        promotion = work / "promotion.img"
        make_disk(promotion, make_store([known_good, healthy], bundle, 2))
        before = promotion.read_bytes()
        candidate = boot(promotion, "SLIME_BOOT promoted")
        expect(candidate, 2, 1, 1)
        after = promotion.read_bytes()
        only_slots(before, after)
        confirmed = boot(promotion, "SLIME_BOOT selected")
        expect(confirmed, 2, 0, 0)
        if promotion.read_bytes() != after:
            fail("promoted boot mutated disk")

        # Structural proof: selector builds intentionally ignore the ambient
        # compile-time generation path, and packaging supplies only one app.
        build_source = (ROOT / "scripts/build/build-sel4.py").read_text(encoding="utf-8")
        root_build = (ROOT / "slime-root/build.rs").read_text(encoding="utf-8")
        if "if variant == BOOT_SELECTION_VARIANT" not in build_source \
                or 'root_environment["SLIME_BOOT_SELECTOR"] = "1"' not in build_source \
                or "package_image(payload_tool, loader, root_elf, image)" not in build_source \
                or 'if std::env::var("SLIME_BOOT_SELECTOR").as_deref() != Ok("1")' not in root_build:
            fail("selector build does not structurally exclude embedded generation bytes")

    print(
        "seL4 boot selection check: attempts persisted across fresh QEMU processes, "
        "exhaustion rolled back, a pending generation in a superseded wire format "
        "was refused without consuming the known-good root, health promoted, and "
        "only BootState sectors changed"
    )


if __name__ == "__main__":
    main()
