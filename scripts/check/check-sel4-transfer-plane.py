#!/usr/bin/env python3

"""P5.4.3 gate: M6.7's generation transfer (M6.7).

A generation crosses a persistence boundary. Two devices are attached: a
*source* the component may only read, carrying the transfer manifest, and a
*receiver* it may write, holding the BootState.

The separation is the milestone. M6.7 requires that a transfer "leave every
ungranted device byte-identical", and here the source is granted — read-only —
so the claim is sharper: a device granted `blockRead` alone is byte-identical
even though the component reached it, repeatedly, and wanted to write it.

The arms:

* the source refuses a write, checked before anything else;
* the manifest decodes, which validates bounds, ordering, and a self-excluding
  SHA-256 over the whole record;
* a tampered byte fails that digest specifically, not some field it landed in;
* the object closure re-hashes to the identities the manifest declares, before
  any BootState write — so an incomplete transfer costs the receiver nothing;
* state travels only where the source declared it may;
* the generation stages **pending**, leaving the known-good root intact, and
  only health confirmation promotes it.

This plane exists because B29 was fixed: two QEMU virtio transports share one
4 KiB granule, and the root now maps that granule once and hands each driver a
borrowed view at its own offset.
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
FIXTURE_SCRIPT = ROOT / "scripts" / "build" / "build-store-fixture.py"
IMAGE = ROOT / "build" / "slime-sel4-transfer.elf"
FIXTURE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-transfer.zti"
BOOT_TIMEOUT_SECONDS = 240

# The receiver's BootState slots, partition-relative.
STATE_SLOT_A = 1024

# Sequence 2 is the higher of the two slots recovery writes, so it is what a
# fresh boot selects. Release 3 is the fixture's accepted release sequence.
RECONSTRUCTED_SEQUENCE = 2
RECONSTRUCTED_RELEASE = 3
# The fixture's state closure: the store's seeded object.
CLOSURE_OBJECTS = 1

REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        "the unconfigured instance parked without a run token",
        r"\[sel4-transfer-probe\] idle without a run token",
    ),
    (
        "init spawned the probe",
        r"\[init\] transfer probe spawned",
    ),
    (
        # Checked before anything else: every later claim about the source
        # being untouched rests on the capability refusing the write.
        "the source device refuses writes",
        r"\[sel4-transfer-probe\] the source device refuses writes",
    ),
    (
        "the receiver starts from a known-good root",
        r"\[sel4-transfer-probe\] receiver holds a known-good root",
    ),
    (
        "the manifest decoded with its declared closure",
        r"\[sel4-transfer-probe\] manifest objects=1 states=1",
    ),
    (
        # On the digest specifically. A flip in the metadata is covered by no
        # bound, so only the self-excluding hash catches it.
        "a tampered manifest was refused on its digest",
        r"\[sel4-transfer-probe\] tampered manifest refused",
    ),
    (
        # Before any BootState write, so an incomplete transfer consumes no
        # attempt and leaves the receiver as it was.
        "every object in the closure re-hashed to its declared identity",
        r"\[sel4-transfer-probe\] closure verified objects=1 of=1",
    ),
    (
        # `ephemeral` state does not travel; `immutable` travels read-only.
        "state travelled only where the source declared it may",
        r"\[sel4-transfer-probe\] state travels entries=1 read-only=1",
    ),
    (
        # Pending, not promoted: the known-good root is intact, which is what
        # makes a failed activation recoverable.
        "the transferred generation staged pending",
        r"\[sel4-transfer-probe\] staged seq=2 pending=1 release=1",
    ),
    (
        # Only now does the known-good root become the transferred generation,
        # and the accepted release advances to the manifest's.
        "health confirmation promoted it and advanced the release",
        r"\[sel4-transfer-probe\] promoted seq=3 pending=0 release=5",
    ),
    (
        "the probe ran every arm and exited cleanly",
        r"\[sel4-transfer-probe\] transfer plane complete",
    ),
    (
        "init observed the clean exit",
        r"\[init\] transfer plane complete",
    ),
)

TERMINAL_MARKER = r"\[init\] transfer plane complete"

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_ROOT FAIL",
    r"SLIME_GRAPH FAIL",
    r"SLIME_GRAPH wedged waiter",
    r"\[init\] transfer plane fail: .*",
    r"\[sel4-transfer-probe\] fail: .*",
    r"SLIME_ROOT block bring-up failed",
    r"Caught cap fault",
    r"Caught vm fault",
    r"Caught user exception",
    r"panicked at ",
    r"aborted at ",
    r"\(aborted\)",
)

def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 transfer plane check: {message}")


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
    command = [sys.executable, str(BUILD_SCRIPT), "--transfer-plane"]
    print(f"[build] {' '.join(command)}", flush=True)
    try:
        process = subprocess.run(command, cwd=ROOT, check=False)
    except OSError as error:
        fail(f"cannot run the seL4 image build: {error}")
    if process.returncode != 0:
        fail(f"seL4 image build failed with exit status {process.returncode}")


def build_fixture(disk: Path, variant: str) -> None:
    """One of the two fixtures: `happy` for the receiver, `transfer` for the
    source — the same image plus a transfer manifest above the record area."""
    command = [sys.executable, str(FIXTURE_SCRIPT), str(disk), variant]
    try:
        process = subprocess.run(command, cwd=ROOT, check=False, capture_output=True)
    except OSError as error:
        fail(f"cannot build the store fixture: {error}")
    if process.returncode != 0:
        fail(f"store fixture build failed: {process.stderr.decode()}")


def boot(profile: dict[str, object], receiver: Path, source: Path) -> str:
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
        f"if=none,id=slimedisk,format=raw,file={receiver}",
        "-device",
        "virtio-blk-device,drive=slimedisk",
        # The source, attached second so QEMU gives it the lower transport —
        # the root sorts highest-address-first, so device 0 is the receiver.
        "-drive",
        f"if=none,id=sourcedisk,format=raw,file={source}",
        "-device",
        "virtio-blk-device,drive=sourcedisk",
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
    completions = transcript.count("[sel4-transfer-probe] transfer plane complete")
    if completions != 1:
        report_transcript(transcript)
        fail(f"{completions} instances ran the scenario, expected 1")
    # Both devices came up. B29 was that only one did — two QEMU transports
    # share a granule and the frame maps once — so a regression there leaves
    # this plane with one device and no source to read.
    ready = re.findall(r"SLIME_ROOT block ready transport=(\S+) sectors=\d+", transcript)
    if len(ready) != 2:
        report_transcript(transcript)
        fail(f"{len(ready)} block devices were brought up, expected 2; saw {ready}")
    # The root refused the write the probe attempted on its read-only source.
    if not re.search(r"SLIME_GRAPH block refused task=\d+ op=2 class=rights", transcript):
        fail("the root recorded no rights refusal on the read-only source")
    print(
        f"transcript: {len(REQUIRED_MARKERS)} markers observed; both devices came "
        "up from one shared granule, the read-only source refused a write, a "
        "tampered manifest failed its digest, and the transferred generation "
        "staged pending before health confirmation promoted it",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 transfer-plane image and assert M6.7"
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
        receiver = Path(directory) / "receiver.img"
        source = Path(directory) / "source.img"
        build_fixture(receiver, "happy")
        build_fixture(source, "transfer")
        source_before = source.read_bytes()
        receiver_before = receiver.read_bytes()
        transcript = boot(profile, receiver, source)
        check_transcript(transcript)
        # M6.7's central claim, checked from the host: the source is granted
        # `blockRead` and nothing else, and it is byte-identical even though the
        # component read it repeatedly and tried to write it.
        if source.read_bytes() != source_before:
            fail("the read-only source device changed during the transfer")
        # The receiver changed, and only in its BootState slots: the transfer
        # installs a root, and nothing else on that device is its business.
        after = receiver.read_bytes()
        if after == receiver_before:
            fail("the receiver was not written, so nothing was transferred")
        slot_a = (40 + STATE_SLOT_A) * 512
        for name, start, end in (
            ("the GPT and protective MBR", 0, 40 * 512),
            ("the object store region", 40 * 512, slot_a),
            ("the disk beyond the BootState slots", slot_a + 1024, len(after)),
        ):
            if after[start:end] != receiver_before[start:end]:
                fail(f"the transfer modified {name} on the receiver")

    print(
        "seL4 transfer plane check: a generation crossed from a read-only "
        "source device to a writable receiver — manifest digest, object "
        "closure, and travel policy all verified before any write — staged "
        "pending without disturbing the known-good root, and was promoted only "
        "on health confirmation, leaving the source byte-identical"
    )


if __name__ == "__main__":
    main()
