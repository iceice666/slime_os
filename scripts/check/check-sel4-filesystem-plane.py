#!/usr/bin/env python3

"""P5.4.3 gate: M6.3's filesystem service (M6.3).

The other half of M6.3. `sel4_directory_check` covers the capability mechanism
the root owns; this covers the service on top of it — a component that resolves
names inside a snapshot tree, reads and writes objects through the
content-addressed store, and derives subdirectory capabilities on request.

The client is the **oracle's own** `directory-probe`, unmodified. That is the
result worth stating: M6.3's userspace half is policy, and policy ports. What
changed underneath is that the object bytes come from a userspace
`ObjectStore` over a granted block capability rather than from a kernel
`store_transact` with an ambient pointer, and the client cannot tell.

The client hands the service its *own* directory view with every request, so
the service acts with the client's authority rather than its own. That transfer
is the reason `contracts/capability-transfer` gained a directory object kind.
"""

from __future__ import annotations

import argparse
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
PINS_PATH = ROOT / "sel4" / "pins.toml"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
FIXTURE_SCRIPT = ROOT / "scripts" / "build" / "build-directory-fixture.py"
IMAGE = ROOT / "build" / "slime-sel4-filesystem.elf"
FIXTURE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-filesystem.zti"
BOOT_TIMEOUT_SECONDS = 240

REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        "init spawned the service",
        r"\[init\] filesystem service spawned",
    ),
    (
        "init spawned the client",
        r"\[init\] directory probe spawned",
    ),
    (
        # The service opened the object store over its block capability. It
        # prints this only after `open_store` succeeds, so a store it could not
        # validate stops the plane here.
        "the service opened the store and is serving",
        r"\[filesystem\] ready",
    ),
    (
        # An interrupted root transition: the service put a new snapshot object
        # but the commit named a stale root, so the namespace still resolves the
        # old tree. M6.3's "preserve the previous root across interruption".
        "an interrupted transition preserved the root",
        r"\[directory-probe\] interrupted transition preserved root",
    ),
    (
        # Write, then read back: the service built a new snapshot, sealed it in
        # the store, and committed the namespace root to it.
        "a root transition committed and is visible",
        r"\[directory-probe\] root transition committed",
    ),
    (
        # A subdirectory capability, minted by the service and transferred to
        # the client — narrower in scope and in rights.
        "a narrowed subdirectory capability was derived",
        r"\[directory-probe\] derive narrowed",
    ),
    (
        "a read through a scoped view resolved",
        r"\[directory-probe\] scoped read ok",
    ),
    (
        # The derived view cannot name anything outside its subtree.
        "the derived scope's boundary is enforced",
        r"\[directory-probe\] scoped boundary enforced",
    ),
    (
        "a malformed request was rejected",
        r"\[directory-probe\] malformed rejected",
    ),
    (
        "the client ran every arm and exited cleanly",
        r"\[directory-probe\] done",
    ),
    (
        "init observed both clean exits",
        r"\[init\] filesystem plane complete",
    ),
)

TERMINAL_MARKER = r"\[init\] filesystem plane complete"

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_ROOT FAIL",
    r"SLIME_GRAPH FAIL",
    r"SLIME_GRAPH wedged waiter",
    r"\[init\] filesystem plane fail: .*",
    r"\[filesystem\] fail: .*",
    r"Caught cap fault",
    r"Caught vm fault",
    r"Caught user exception",
    r"panicked at ",
    r"aborted at ",
    r"\(aborted\)",
)

def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 filesystem plane check: {message}")


def load_pins() -> dict[str, object]:
    if not PINS_PATH.is_file():
        fail(f"missing pin manifest: {PINS_PATH.relative_to(ROOT)}")
    try:
        pins = tomllib.loads(PINS_PATH.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        fail(f"cannot parse {PINS_PATH.relative_to(ROOT)}: {error}")
    if pins.get("schema") != 1:
        fail("unsupported sel4/pins.toml schema (expected 1)")
    if not isinstance(pins.get("qemu_arm_virt"), dict):
        fail("sel4/pins.toml is missing [qemu_arm_virt]")
    return pins


def profile_text(profile: dict[str, object], key: str) -> str:
    value = profile.get(key)
    if not isinstance(value, str) or not value:
        fail(f"sel4/pins.toml [qemu_arm_virt].{key} must be non-empty text")
    return value


def profile_integer(profile: dict[str, object], key: str) -> int:
    value = profile.get(key)
    if not isinstance(value, int) or isinstance(value, bool):
        fail(f"sel4/pins.toml [qemu_arm_virt].{key} must be an integer")
    return value


def build_image() -> None:
    command = [sys.executable, str(BUILD_SCRIPT), "--filesystem-plane"]
    print(f"[build] {' '.join(command)}", flush=True)
    try:
        process = subprocess.run(command, cwd=ROOT, check=False)
    except OSError as error:
        fail(f"cannot run the seL4 image build: {error}")
    if process.returncode != 0:
        fail(f"seL4 image build failed with exit status {process.returncode}")




def build_fixture(disk: Path) -> None:
    """The directory fixture: the store's happy image plus a committed snapshot
    tree — `docs/` and two `note` objects — whose root the boot seeds."""
    command = [sys.executable, str(FIXTURE_SCRIPT), str(disk)]
    try:
        process = subprocess.run(command, cwd=ROOT, check=False, capture_output=True)
    except OSError as error:
        fail(f"cannot build the directory fixture: {error}")
    if process.returncode != 0:
        fail(f"directory fixture build failed: {process.stderr.decode()}")


def boot(profile: dict[str, object], disk: Path) -> str:
    qemu = shutil.which("qemu-system-aarch64")
    if qemu is None:
        fail("qemu-system-aarch64 is not on PATH")
    command = [
        qemu,
        "-machine",
        profile_text(profile, "machine"),
        "-cpu",
        profile_text(profile, "cpu"),
        "-smp",
        str(profile_integer(profile, "cpus")),
        "-m",
        f"size={profile_integer(profile, 'memory_mib')}M",
        "-nographic",
        "-serial",
        "mon:stdio",
        "-kernel",
        str(IMAGE),
        "-drive",
        f"if=none,id=slimedisk,format=raw,file={disk}",
        "-device",
        "virtio-blk-device,drive=slimedisk",
    ]
    print(f"[boot] {' '.join(command)}", flush=True)
    failures = re.compile("|".join(FAILURE_MARKERS))
    terminal = re.compile(TERMINAL_MARKER)
    lines: list[str] = []
    reached = False
    try:
        process = subprocess.Popen(
            command,
            cwd=ROOT,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
    except OSError as error:
        fail(f"cannot run QEMU: {error}")
    watchdog = threading.Timer(BOOT_TIMEOUT_SECONDS, process.kill)
    watchdog.start()
    try:
        assert process.stdout is not None
        for line in process.stdout:
            lines.append(line.rstrip("\r\n"))
            if failures.search(line):
                break
            if terminal.search(line):
                reached = True
                break
    finally:
        watchdog.cancel()
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
    transcript = "\n".join(lines)
    if not reached:
        report_transcript(transcript)
        fail(f"boot exceeded {BOOT_TIMEOUT_SECONDS}s without completing the plane")
    return transcript


def report_transcript(transcript: str) -> None:
    tail = transcript.splitlines()[-40:]
    if tail:
        sys.stdout.write("--- serial transcript (tail) ---\n")
        sys.stdout.write("\n".join(tail) + "\n")
        sys.stdout.write("--- end transcript ---\n")
        sys.stdout.flush()


def check_transcript(transcript: str) -> None:
    for pattern in FAILURE_MARKERS:
        match = re.search(pattern, transcript)
        if match is not None:
            report_transcript(transcript)
            fail(f"failure marker in serial transcript: {match.group(0)!r}")
    position = 0
    for label, pattern in REQUIRED_MARKERS:
        match = re.compile(pattern).search(transcript, position)
        if match is None:
            report_transcript(transcript)
            if re.search(pattern, transcript) is not None:
                fail(f"marker out of order: {label} ({pattern})")
            fail(f"missing marker: {label} ({pattern})")
        position = match.end()
    # Exactly one client ran the scenario. The root also launches an
    # unconfigured copy of `directory-probe` — the oracle's own binary, which
    # carries no seL4 authority probe and exits nonzero when it finds no
    # capability. That copy is not this plane's subject, so its exit is
    # tolerated; what is *not* tolerated is a second completion, which would
    # mean two clients raced on one namespace.
    completions = transcript.count("[directory-probe] done")
    if completions != 1:
        report_transcript(transcript)
        fail(f"{completions} clients completed the scenario, expected 1")
    # The first arm, asserted by count rather than by order: the root-launched
    # copy reaches it too — deriving from a slot holding no directory is refused
    # for *both* instances, which is the arm's whole point — and it runs before
    # init has spawned anything, so an ordered pin would place it wrongly.
    denials = transcript.count("[directory-probe] no-cap denied")
    if denials < 1:
        report_transcript(transcript)
        fail("a derive without a directory capability was not denied")
    # The client's directory view crossed to the service on every request. This
    # is the property the new `objectKindDirectory` exists for, and a service
    # that answered from its own authority instead would show none.
    transfers = re.findall(
        r"SLIME_GRAPH capability transfer task=\d+ channel=\d+ side=(\w+) caps=1", transcript
    )
    if transfers.count("producer") < 4 or transfers.count("consumer") < 4:
        fail(
            "the client's directory view did not reach the service on each "
            f"request; observed {transfers}"
        )
    print(
        f"transcript: {len(REQUIRED_MARKERS)} markers observed; the oracle's own "
        f"directory-probe drove read, interrupted-write, write, derive, and "
        f"boundary arms through the seL4 filesystem service, handing its view "
        f"across {transfers.count('producer')} times",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 filesystem-plane image and assert M6.3's service half"
    )
    parser.add_argument(
        "--no-build",
        action="store_true",
        help="boot the already-built image instead of rebuilding it first",
    )
    arguments = parser.parse_args()

    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    if not FIXTURE.is_file():
        fail(f"missing generation fixture {FIXTURE.relative_to(ROOT)}")
    pins = load_pins()
    if not arguments.no_build:
        build_image()
    if not IMAGE.is_file():
        fail(f"missing packaged image {IMAGE.relative_to(ROOT)}")
    profile = pins["qemu_arm_virt"]
    assert isinstance(profile, dict)

    with tempfile.TemporaryDirectory() as directory:
        disk = Path(directory) / "filesystem-plane.img"
        build_fixture(disk)
        before = disk.read_bytes()
        transcript = boot(profile, disk)
        check_transcript(transcript)
        # The store is append-only: a committed snapshot adds records and moves
        # a superblock, and never rewrites what was already sealed. The GPT is
        # not the service's to touch at all.
        after = disk.read_bytes()
        if after[: 40 * 512] != before[: 40 * 512]:
            fail("the filesystem service modified the GPT or protective MBR")
        if after == before:
            fail("no snapshot was committed, so the write arms did nothing")

    print(
        "seL4 filesystem plane check: the oracle's own directory-probe resolved "
        "names, committed a root transition, and derived a narrowed "
        "subdirectory through a seL4 filesystem service backed by a userspace "
        "object store — unmodified, because M6.3's service half is policy"
    )


if __name__ == "__main__":
    main()
