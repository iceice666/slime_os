#!/usr/bin/env python3

"""P5.4.2c gate: M5.4's object store, in userspace (M5.4).

The oracle keeps GPT validation, root selection, the object index, content
hashing, and commit ordering inside the kernel — `store_service` owns a global
store and `sys_store_transact` is syscall 7. This gate asserts the same
properties with none of that placement: the root mediates sectors, and a
component does the rest.

Two scenarios, because M5.4's redundancy claims are only observable when a copy
is damaged:

* `happy` — both GPT copies agree, the newest superblock is valid, and the
  component retrieves the seeded object by content hash, appends and seals a new
  one, deduplicates identical content, re-opens the store from disk to prove the
  commit durable, scrubs every payload, and is refused an oversized one.
* `superblock-newest-damaged` — the newest superblock slot fails its CRC and the
  store must open on the older root instead, at the lower sequence, seeing only
  what that root committed.

The second is what makes the first mean something. A store that ignored
superblock CRCs entirely would pass every `happy` assertion.
"""

from __future__ import annotations

import argparse
import hashlib
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
IMAGE = ROOT / "build" / "slime-sel4-store.elf"
FIXTURE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-store.zti"
BOOT_TIMEOUT_SECONDS = 240

# The store partition the fixture declares, and the roots it seeds: slot A at
# sequence 2 with one committed object, slot B at sequence 1 with none.
EXPECTED_FIRST_LBA = 40
EXPECTED_LAST_LBA = 2014
SEEDED_SEQUENCE = 2
SEEDED_OBJECTS = 1
# The older root, which `superblock-newest-damaged` must fall back to.
OLDER_SEQUENCE = 1
OLDER_OBJECTS = 0

REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        "the spawned instance received its declared device authority",
        r"SLIME_GRAPH declared placed task=\d+ child=\d+ slot=\d+ kind=block",
    ),
    (
        "init spawned the probe",
        r"\[init\] store probe spawned",
    ),
    (
        # Protective MBR, both header copies, entry-array CRCs, bounds, overlap,
        # and store selection by type GUID — all in userspace, against the real
        # device. The LBAs are the fixture's, so a validator that accepted a
        # malformed table would report a different span.
        "the store partition was selected from a validated GPT",
        rf"\[sel4-store-probe\] partition first={EXPECTED_FIRST_LBA} "
        rf"last={EXPECTED_LAST_LBA} recovery=none",
    ),
    (
        "the store opened on the newest valid root",
        rf"\[sel4-store-probe\] opened seq={SEEDED_SEQUENCE} objects={SEEDED_OBJECTS}",
    ),
    (
        # Retrieved by content hash and compared byte for byte against the
        # payload the fixture wrote. A store returning the right length with the
        # wrong bytes fails here, and `get` re-hashes before returning at all.
        "the seeded object was retrieved and its payload verified",
        r"\[sel4-store-probe\] seeded object verified",
    ),
    (
        "a hash naming no object was refused",
        r"\[sel4-store-probe\] unknown hash refused",
    ),
    (
        # Append and seal: record sectors, flush, older superblock slot, flush.
        # The sequence advances by exactly one and the object count by one.
        "a new object was appended and the root advanced",
        rf"\[sel4-store-probe\] appended seq={SEEDED_SEQUENCE + 1} "
        rf"objects={SEEDED_OBJECTS + 1}",
    ),
    (
        "identical content was deduplicated rather than appended",
        r"\[sel4-store-probe\] duplicate content deduplicated",
    ),
    (
        # Re-opened from disk. This is what makes the append durable rather than
        # in-memory: a fresh open re-reads both superblocks, picks the newest,
        # and re-indexes the records.
        "the store re-opened from disk at the committed root",
        rf"\[sel4-store-probe\] reopened seq={SEEDED_SEQUENCE + 1} "
        rf"objects={SEEDED_OBJECTS + 1}",
    ),
    (
        "every committed payload re-hashed against its record",
        r"\[sel4-store-probe\] scrub verified every object",
    ),
    (
        "a payload larger than the format admits was refused",
        r"\[sel4-store-probe\] oversized payload refused",
    ),
    (
        "the probe reported its heap footprint",
        r"\[sel4-store-probe\] heap used=\d+ capacity=\d+",
    ),
    (
        "the probe ran every arm and exited cleanly",
        r"\[sel4-store-probe\] store plane complete",
    ),
    (
        "init observed the clean exit",
        r"\[init\] store plane complete",
    ),
)

TERMINAL_MARKER = r"\[init\] store plane complete"

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_ROOT FAIL",
    r"SLIME_GRAPH FAIL",
    r"SLIME_GRAPH wedged waiter",
    r"\[init\] store plane fail: .*",
    r"\[sel4-store-probe\] fail: .*",

    r"SLIME_ROOT block bring-up failed",
    r"Caught cap fault",
    r"Caught vm fault",
    r"Caught user exception",
    r"panicked at ",
    r"aborted at ",
    r"\(aborted\)",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 store plane check: {message}")


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
    command = [sys.executable, str(BUILD_SCRIPT), "--store-plane"]
    print(f"[build] {' '.join(command)}", flush=True)
    try:
        process = subprocess.run(command, cwd=ROOT, check=False)
    except OSError as error:
        fail(f"cannot run the seL4 image build: {error}")
    if process.returncode != 0:
        fail(f"seL4 image build failed with exit status {process.returncode}")


def build_fixture(disk: Path, variant: str) -> None:
    command = [sys.executable, str(FIXTURE_SCRIPT), str(disk), variant]
    try:
        process = subprocess.run(command, cwd=ROOT, check=False, capture_output=True)
    except OSError as error:
        fail(f"cannot build the store fixture: {error}")
    if process.returncode != 0:
        fail(f"store fixture build failed for {variant}: {process.stderr.decode()}")


def boot(
    profile: dict[str, object], disk: Path, terminal: str, *, await_idle: bool = False
) -> str:
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
    stop = re.compile(terminal)
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
        # The root-owned idle instance runs concurrently with init and reports
        # holding no run token only after a bounded wait, so its line can land
        # before or after the stop marker. A caller whose scenario runs the whole
        # plane waits for both facts; the fallback and refusal scenarios stop at
        # an intermediate marker on purpose, and reading past it would run into
        # a failure the fixture is designed to produce.
        idle_seen = not await_idle
        for line in process.stdout:
            lines.append(line.rstrip("\r\n"))
            if failures.search(line):
                break
            idle_seen |= "[sel4-store-probe] idle without a run token" in line
            reached |= stop.search(line) is not None
            if reached and idle_seen:
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
        fail(f"boot exceeded {BOOT_TIMEOUT_SECONDS}s without reaching {terminal!r}")
    return transcript


def report_transcript(transcript: str) -> None:
    tail = transcript.splitlines()[-40:]
    if tail:
        sys.stdout.write("--- serial transcript (tail) ---\n")
        sys.stdout.write("\n".join(tail) + "\n")
        sys.stdout.write("--- end transcript ---\n")
        sys.stdout.flush()


def check_failures(transcript: str) -> None:
    for pattern in FAILURE_MARKERS:
        match = re.search(pattern, transcript)
        if match is not None:
            report_transcript(transcript)
            fail(f"failure marker in serial transcript: {match.group(0)!r}")


def check_happy(transcript: str) -> None:
    check_failures(transcript)
    position = 0
    for label, pattern in REQUIRED_MARKERS:
        match = re.compile(pattern).search(transcript, position)
        if match is None:
            report_transcript(transcript)
            if re.search(pattern, transcript) is not None:
                fail(f"marker out of order: {label} ({pattern})")
            fail(f"missing marker: {label} ({pattern})")
        position = match.end()
    # Asserted by presence, not by position: the idle instance concludes it
    # holds no run token only after a bounded wait, so its line lands wherever
    # the scheduler puts it. Ordering it would assert a scheduling accident.
    if "[sel4-store-probe] idle without a run token" not in transcript:
        report_transcript(transcript)
        fail("the unconfigured instance did not report parking without a run token")
    # Exactly one instance ran the scenario. Two would race on the append.
    completions = transcript.count("[sel4-store-probe] store plane complete")
    if completions != 1:
        report_transcript(transcript)
        fail(f"{completions} instances ran the scenario, expected 1")
    # The heap bound is declared in the runtime; the component reports what it
    # actually used. A footprint at or above the bound means the next object
    # would fail to allocate, which is worth catching before it does.
    used, capacity = (
        int(value)
        for value in re.search(
            r"\[sel4-store-probe\] heap used=(\d+) capacity=(\d+)", transcript
        ).groups()
    )
    if used >= capacity:
        fail(f"the probe used {used} of {capacity} heap bytes, leaving no headroom")
    print(
        f"happy: {len(REQUIRED_MARKERS)} markers observed; GPT validated, store "
        f"opened at sequence {SEEDED_SEQUENCE}, object retrieved by content hash, "
        f"append committed at sequence {SEEDED_SEQUENCE + 1} and durable across a "
        f"re-open, scrub verified every payload, heap {used}/{capacity} bytes",
        flush=True,
    )


def check_older_root(transcript: str) -> None:
    """The newest superblock is damaged, so the store must open on the older one.

    M5.4 requires an older valid root to be preserved and used, not merely for a
    damaged slot to be detected. Opening at the seeded sequence here would mean
    the CRC was never checked; failing to open at all would mean the redundancy
    is not redundancy.
    """
    check_failures(transcript)
    expected = (
        rf"\[sel4-store-probe\] opened seq={OLDER_SEQUENCE} objects={OLDER_OBJECTS}"
    )
    if re.search(expected, transcript) is None:
        report_transcript(transcript)
        fail(
            f"the store did not fall back to the older root "
            f"(expected sequence {OLDER_SEQUENCE} with {OLDER_OBJECTS} objects)"
        )
    # The seeded object belongs to the damaged newer root, so the older root must
    # not see it. A store that fell back but kept the newer index would.
    if "[sel4-store-probe] seeded object verified" in transcript:
        report_transcript(transcript)
        fail("the older root exposed an object only the newer root committed")
    print(
        f"superblock-newest-damaged: the store fell back to sequence "
        f"{OLDER_SEQUENCE} and saw none of the newer root's objects",
        flush=True,
    )


# The refusal scenarios: a fixture variant, the class the component must report,
# and why the store is required to fail closed rather than recover.
#
# Each one is a *correct rejection*, so the component exits 0 after reporting the
# class. What the gate pins is which class — a store that refused everything for
# the wrong reason would otherwise pass.
REFUSALS: tuple[tuple[str, str, str], ...] = (
    (
        "gpt-conflict",
        r"\[sel4-store-probe\] gpt error=conflicting-copies",
        # Both copies validate but disagree on disk identity. Picking either
        # would be a guess about which one is authoritative, so M5.4 requires a
        # hard reject rather than a false recovery.
        "two valid but disagreeing GPT copies are rejected, not silently resolved",
    ),
    (
        "superblock-both-damaged",
        r"\[sel4-store-probe\] store error=no-valid-superblock",
        # Neither root decodes. There is no older root to fall back to, and
        # inventing one would expose records no commit ever sealed.
        "a store with no valid root fails closed",
    ),
)


def check_refusal(transcript: str, pattern: str, why: str) -> None:
    """A fixture the component must reject, for the stated reason."""
    if re.search(pattern, transcript) is None:
        report_transcript(transcript)
        fail(f"expected refusal not observed ({pattern}): {why}")
    # Rejected, and rejected *cleanly*: a panic or a fault would also stop the
    # boot, and would also technically not open the store.
    for marker in (r"\[sel4-store-probe\] fail: ", r"panicked at ", r"Caught vm fault"):
        if re.search(marker, transcript) is not None:
            report_transcript(transcript)
            fail(f"the refusal was not clean: {marker!r}")
    print(f"refusal: {why}", flush=True)


def check_seeded_untouched(disk: Path, before: bytes) -> None:
    """The append wrote only where the store says it may.

    Sectors 0 and 1 of the partition are the two superblock slots, and one of
    them is the committed root the append must preserve. Everything below the
    partition — the GPT, the protective MBR — has no business changing at all.
    """
    after = disk.read_bytes()
    if after[: EXPECTED_FIRST_LBA * 512] != before[: EXPECTED_FIRST_LBA * 512]:
        fail("the append modified the GPT or protective MBR")
    # Exactly one superblock slot changes per commit, and it is the older one.
    slot_a = EXPECTED_FIRST_LBA * 512
    slot_b = slot_a + 512
    changed = [
        name
        for name, start in (("A", slot_a), ("B", slot_b))
        if after[start : start + 512] != before[start : start + 512]
    ]
    if changed != ["B"]:
        fail(
            "the commit wrote superblock slots "
            f"{changed or ['none']}, expected exactly the older slot B"
        )
    print(
        "image: the GPT is unchanged and the commit wrote exactly the older "
        "superblock slot, so the previously committed root survived",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 store-plane image and assert M5.4 in userspace"
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
        disk = Path(directory) / "store-happy.img"
        build_fixture(disk, "happy")
        before = disk.read_bytes()
        transcript = boot(profile, disk, TERMINAL_MARKER, await_idle=True)
        check_happy(transcript)
        check_seeded_untouched(disk, before)

        damaged = Path(directory) / "store-damaged.img"
        build_fixture(damaged, "superblock-newest-damaged")
        digest_before = hashlib.sha256(damaged.read_bytes()).hexdigest()
        # This scenario cannot complete the plane: the older root has no seeded
        # object, so the probe fails its retrieval arm by design. Stop at the
        # open marker, which is the property under test.
        transcript = boot(
            profile,
            damaged,
            rf"\[sel4-store-probe\] opened seq={OLDER_SEQUENCE}",
        )
        check_older_root(transcript)
        if hashlib.sha256(damaged.read_bytes()).hexdigest() != digest_before:
            fail("the fallback scenario modified the disk before its first read")

        # An uncommitted record past the committed append point. The fixture
        # writes a valid-magic truncated record there; the committed `append_lba`
        # excludes it, so the index must not carry it.
        interrupted = Path(directory) / "store-interrupted.img"
        build_fixture(interrupted, "interrupted-append")
        transcript = boot(profile, interrupted, TERMINAL_MARKER, await_idle=True)
        check_happy(transcript)
        print(
            "interrupted-append: an uncommitted record past the committed append "
            "point was ignored and the scenario ran unchanged",
            flush=True,
        )

        # The two fixtures the store must reject outright.
        for variant, pattern, why in REFUSALS:
            refused = Path(directory) / f"store-{variant}.img"
            build_fixture(refused, variant)
            digest = hashlib.sha256(refused.read_bytes()).hexdigest()
            transcript = boot(
                profile, refused, r"\[sel4-store-probe\] store plane refused"
            )
            check_refusal(transcript, pattern, why)
            if hashlib.sha256(refused.read_bytes()).hexdigest() != digest:
                fail(f"the {variant} scenario wrote to a disk it had rejected")

    print(
        "seL4 store plane check: a component validated a GPT, opened a "
        "content-addressed object store, retrieved and verified an object by "
        "hash, appended a durable commit preserving the previous root, and fell "
        "back to the older root when the newest superblock was damaged — with "
        "M5.4 policy entirely in userspace"
    )


if __name__ == "__main__":
    main()
