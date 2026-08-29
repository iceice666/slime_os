#!/usr/bin/env python3

"""P5.4.2c gate: a userspace component reaches a real disk (M5.2, M5.3).

`just sel4_device_check` proves the product root does *not* touch an attached
disk. This gate proves the layer that matters: a *component* moving sectors
through nothing but a capability its generation granted it, served by the
supervised userspace `virtio-blk-driver` over an IO0 ring whose rights the
generation declares (B83).

Six arms, and each fails differently:

* a read returns the fixture's own signature, so the sector crossed rather than
  being fabricated;
* a write, a flush, and a read-back agree byte for byte, and the write is
  confirmed durable in the host image after the boot;
* a sector past the device's capacity is refused;
* a malformed request is refused before any sector moves;
* a slot holding no block capability is refused;
* the root-launched instance of the same component, holding the same device
  capability, parks — so the arms above are the spawned instance's.

That last one is the authority claim. Both copies of the component hold the
block capability, because a generation grant names a component rather than a
task; what distinguishes them is a run token only `init` hands to the instance
it spawns.
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

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from harness import GENERATION_COMPOSITIONS, profile_text, profile_integer  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
IMAGE = ROOT / "build" / "slime-sel4-storage.elf"
MANIFEST = ROOT / "build" / "slime-sel4-storage.identity.json"
FIXTURE = GENERATION_COMPOSITIONS / "sel4-storage.zti"
IMAGE_VARIANT = "storage"
BOOT_TIMEOUT_SECONDS = 180

DISK_BYTES = 1 << 20
# Written at sector 0 by the fixture, and required back from the read.
DISK_SIGNATURE = b"SLIMEDSK"
# Written at sector 1 by the probe, and required in the image afterwards.
SCRATCH_LBA = 1
SCRATCH_SIGNATURE = b"SLIMEWR1"
SECTOR_BYTES = 512

REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        # The spawned instance's crossing authority, placed by the root above
        # the parent's grants. Post-B83 that authority is the shared-buffer
        # factory it builds its IO0 ring from, which is what
        # `buffer_factory_grants` counts; before P5.4.2c a spawned child
        # received only what its parent handed it, so this evidence did not
        # exist and the arms below could not run.
        "the spawned instance received its declared crossing authority",
        r"SLIME_GRAPH spawned task=\d+ child=\d+ component=sel4-storage-probe .*buffer_factory_grants=1",
    ),
    (
        "init spawned the probe",
        r"\[init\] storage probe spawned",
    ),
    (
        # B83: the probe's device authority is no longer a root-placed `block`
        # capability. It is a declared row in the generation's
        # `block-ring-authority` table, read by the userspace driver through the
        # root's identity-gated paged path. This line is that read: a driver
        # serving zero rings would refuse every request, and one that never read
        # the table would not have started. It follows the spawn because the
        # driver reads the table after receiving its client's ring loan.
        "the userspace driver read its generation-declared per-ring authority",
        r"\[virtio-blk-driver\] authority rings=1 rights=read,write source=generation",
    ),
    (
        # The device is reached by the driver, not the root: this capacity is
        # read out of virtio config space through IO1's mediated MMIO.
        "the userspace driver brought up the device and announced its capacity",
        r"\[virtio-blk-driver\] ready capacity=\d+ epoch=\d+",
    ),
    (
        # M5.2: a read through the ring, returning the fixture's bytes.
        "a sector was read through the capability and carries the fixture's bytes",
        r"\[sel4-storage-probe\] sector 0 verified",
    ),
    (
        # M5.3: write, flush, and a read-back compared byte for byte.
        "a sector was written, flushed, and read back identical",
        r"\[sel4-storage-probe\] write flushed and verified",
    ),
    (
        "a sector past the device's capacity was refused",
        r"\[sel4-storage-probe\] out-of-range refused",
    ),
    (
        "a malformed request was refused",
        r"\[sel4-storage-probe\] malformed refused",
    ),
    (
        # B83's replacement for the root's rights refusal. The root used to gate
        # each request on the badge-derived caller's own `BlockDevice`; a ring
        # carries no rights identity, so the gate is now the generation's
        # declared per-ring authority and this is the `STATUS_BAD_RIGHTS` it
        # produces. Before B83 that status existed in `io-queue/v1` and nothing
        # ever emitted it.
        "a request outside the ring's declared authority was refused",
        r"\[sel4-storage-probe\] ungranted slot refused",
    ),
    (
        "the driver released cleanly on its peer's command",
        r"\[virtio-blk-driver\] peer complete, exiting",
    ),
    (
        "the probe ran every arm and exited cleanly",
        r"\[sel4-storage-probe\] storage plane complete",
    ),
    (
        "init observed the clean exit",
        r"\[init\] storage plane complete",
    ),
)

TERMINAL_MARKER = r"\[init\] storage plane complete"

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_ROOT FAIL",
    r"SLIME_GRAPH FAIL",
    r"SLIME_GRAPH wedged waiter",
    r"\[init\] storage plane fail: .*",
    r"\[sel4-storage-probe\] fail: .*",
    # The root's `SLIME_ROOT block *` failures are gone with its driver. The
    # userspace driver's own refusals take their place, and the plane must fail
    # on them for the same reason: a driver that cannot serve must not be read
    # as a plane that had nothing to serve.
    r"\[virtio-blk-driver\] fail: .*",
    r"Caught cap fault",
    r"Caught vm fault",
    r"Caught user exception",
    r"panicked at ",
    r"aborted at ",
    r"\(aborted\)",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 storage plane check: {message}")


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


def build_image() -> None:
    command = [sys.executable, str(BUILD_SCRIPT), "--storage-plane"]
    print(f"[build] {' '.join(command)}", flush=True)
    try:
        process = subprocess.run(command, cwd=ROOT, check=False)
    except OSError as error:
        fail(f"cannot run the seL4 image build: {error}")
    if process.returncode != 0:
        fail(f"seL4 image build failed with exit status {process.returncode}")


def boot(profile: dict[str, object], disk: Path) -> str:
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
        # The root-owned idle instance runs concurrently with init and reports
        # holding no run token only after a bounded wait, so its line can land
        # before or after init's completion marker. Stop once *both* facts have
        # been observed rather than at the terminal alone.
        idle_seen = False
        for line in process.stdout:
            lines.append(line.rstrip("\r\n"))
            if failures.search(line):
                break
            idle_seen |= "[sel4-storage-probe] idle without a run token" in line
            reached |= terminal.search(line) is not None
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
        fail(f"boot exceeded {BOOT_TIMEOUT_SECONDS}s without completing the storage plane")
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
    # Asserted by presence, not by position: the idle instance concludes it
    # holds no run token only after a bounded wait, so its line lands wherever
    # the scheduler puts it. Ordering it would assert a scheduling accident.
    if "[sel4-storage-probe] idle without a run token" not in transcript:
        report_transcript(transcript)
        fail("the unconfigured instance did not report parking without a run token")
    # Exactly one instance ran the scenario. Two would mean the run-token
    # discrimination failed and both copies raced on the scratch sector.
    completions = transcript.count("[sel4-storage-probe] storage plane complete")
    if completions != 1:
        report_transcript(transcript)
        fail(f"{completions} instances ran the scenario, expected 1")
    # Corroboration by the root, so the component's claims are not merely
    # self-reported. B83 moved the driver out of the root, so the root no longer
    # records `block served` lines -- it never sees an opcode. What it does
    # record is the mediation it still owns, and that is what a fabricated
    # transcript could not produce: the DMA mappings the driver's transfers went
    # through, and their reclamation.
    #
    # Both directions are required. A driver that mapped only `DeviceRead` could
    # not have served the read whose bytes the probe verified, and one that
    # mapped only `DeviceWrite` could not have served the write the host-side
    # durability check finds on the disk.
    payload_dma = re.findall(
        r"SLIME_IO payload dma pages=\d+ frames=\d+ writable=\w+ direction=(\w+)",
        transcript,
    )
    for direction in ("DeviceRead", "DeviceWrite"):
        if direction not in payload_dma:
            report_transcript(transcript)
            fail(f"the root mediated no {direction} payload DMA for the userspace driver")
    # The root's numeric account of the driver's hardware charges, taken at
    # driver teardown. This is the strongest root-side evidence available after
    # the cutover: it names how many DMA pages and mappings the driver held and
    # how many came back, and it is produced by the root's own resource table
    # rather than by any component's claim.
    #
    # Nonzero before, zero after: a driver that mapped nothing could not have
    # moved the bytes the probe verified, and one that leaked a mapping would
    # leave a nonzero `post_`. Both readings must fail the plane.
    reclaim = re.search(
        r"SLIME_IO reclaim task=\d+ .*pre_dma_pages=(\d+) pre_dma_mappings=(\d+) .*"
        r"reclaimed_dma_pages=(\d+) reclaimed_dma_mappings=(\d+) .*"
        r"post_dma_pages=(\d+) post_dma_mappings=(\d+) post_requests=(\d+)",
        transcript,
    )
    if reclaim is None:
        report_transcript(transcript)
        fail("the root recorded no IO-resource reclamation for the userspace driver")
    pre_pages, pre_mappings, back_pages, back_mappings, post_pages, post_mappings, post_requests = (
        int(value) for value in reclaim.groups()
    )
    if pre_pages == 0 or pre_mappings == 0:
        report_transcript(transcript)
        fail("the driver held no DMA pages or mappings, so it moved no bytes")
    if (back_pages, back_mappings) != (pre_pages, pre_mappings):
        report_transcript(transcript)
        fail(
            f"the root reclaimed {back_pages}/{back_mappings} of "
            f"{pre_pages}/{pre_mappings} DMA pages/mappings"
        )
    if (post_pages, post_mappings, post_requests) != (0, 0, 0):
        report_transcript(transcript)
        fail(
            f"the driver left {post_pages} DMA pages, {post_mappings} mappings, "
            f"and {post_requests} requests outstanding"
        )
    print(
        f"transcript: {len(REQUIRED_MARKERS)} markers observed; a component read, wrote, "
        "flushed, and verified a sector through a userspace virtio-blk driver over "
        "IO0 rings, a request outside its ring's declared authority was refused, "
        "and two further refusal arms held",
        flush=True,
    )


def check_durability(disk: Path) -> None:
    """The write reached the image, not just the device's cache.

    Read after the boot, from the host side. A flush the device acknowledged but
    never honoured would pass every in-boot assertion and fail here.
    """
    image = disk.read_bytes()
    start = SCRATCH_LBA * SECTOR_BYTES
    written = image[start : start + len(SCRATCH_SIGNATURE)]
    if written != SCRATCH_SIGNATURE:
        fail(
            f"sector {SCRATCH_LBA} holds {written!r} after the boot, "
            f"expected {SCRATCH_SIGNATURE!r}"
        )
    if image[: len(DISK_SIGNATURE)] != DISK_SIGNATURE:
        fail("the fixture's own sector 0 was modified")
    print(
        f"image: sector 0 still holds {DISK_SIGNATURE!r} and sector {SCRATCH_LBA} "
        f"holds {SCRATCH_SIGNATURE!r}, so the flushed write is durable",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 storage-plane image and assert M5.2/M5.3"
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
        disk = Path(directory) / "storage-plane.img"
        image = bytearray(DISK_BYTES)
        image[: len(DISK_SIGNATURE)] = DISK_SIGNATURE
        disk.write_bytes(bytes(image))
        transcript = boot(profile, disk)
        check_transcript(transcript)
        check_durability(disk)
    print(
        "seL4 storage plane check: a userspace component read, wrote, flushed, and "
        "verified sectors on a real device through a capability its generation "
        "granted, three refusal arms held, and the write survived to the image"
    )


if __name__ == "__main__":
    main()
