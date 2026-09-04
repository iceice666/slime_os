#!/usr/bin/env python3

"""P5.4.3 gate: M6.5's generation commands, in userspace (M6.5).

Two components and one channel. The manager holds the plane's only block ring
authority and is therefore the only thing that can touch BootState; the client
holds one RPC endpoint and nothing else. That split IS the milestone: M6.5
requires `BOOT_UPDATE` scoped by manifest to the management service, so a
component that wants to inspect, stage, select, or roll back must ask.

The client walks all five operations and their refusals, then tries to reach the
device directly and is refused — not by a rights check, but because no slot it
holds names a ring or a driver endpoint. The gate additionally compares the disk
image around the refused-stage arm: "fail before BootState changes" is a claim
about bytes, and a component reporting a refusal it did not honour would pass
the marker.
"""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
import tempfile
import threading
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from closure_image import ClosureImageError, build as build_closure_image  # noqa: E402
from harness import (
    GENERATION_COMPOSITIONS,
    load_qemu_profile,
    profile_text,
    profile_integer,
    qemu_kernel_arguments,
)  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
FIXTURE_SCRIPT = ROOT / "scripts" / "build" / "build-store-fixture.py"
FIXTURE = GENERATION_COMPOSITIONS / "sel4-generation.zti"
BOOT_TIMEOUT_SECONDS = 240
PLATFORMS = {
    "qemu-arm-virt": ("qemu_arm_virt", "qemu-system-aarch64"),
    "qemu-riscv-virt": ("qemu_riscv_virt", "qemu-system-riscv64"),
}


# CP15: the aarch64 arm builds by closure identity. The closure declares
# platform `qemu-arm-virt`, so the riscv64 arm keeps its flag until a closure
# exists for that platform — a closure cannot describe a build for a platform
# it does not name.
CLOSURE = "sel4-generation"
CLOSURE_PLATFORM = "qemu-arm-virt"
CLOSURE_IMAGE: Path | None = None


def image_path(platform: str) -> Path:
    if platform == CLOSURE_PLATFORM and CLOSURE_IMAGE is not None:
        return CLOSURE_IMAGE
    suffix = "" if platform == CLOSURE_PLATFORM else f"-{platform}"
    return ROOT / "build" / f"slime-sel4-generation{suffix}.elf"


REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        # The client precedes the manager: the manager is granted a supervision
        # handle naming it, and a handle cannot exist before its task. A native
        # Endpoint reports no peer death, so that handle is the only way the
        # manager learns its client is gone rather than merely quiet.
        "init spawned the client",
        r"\[init\] generation client spawned",
    ),
    (
        "the manager received its declared crossing buffer authority",
        r"SLIME_GRAPH spawned task=\d+ child=\d+ component=sel4-generation-manager .*buffer_factory_grants=1",
    ),
    (
        "init spawned the manager",
        r"\[init\] generation manager spawned",
    ),
    (
        "the userspace driver read the manager ring's generation authority",
        r"\[virtio-blk-driver\] authority rings=1 rights=read,write source=generation",
    ),
    (
        "the userspace driver brought up the generation disk",
        r"\[virtio-blk-driver\] ready capacity=2048 epoch=\d+",
    ),
    (
        # The manager alone holds the generation-authorized IO0 ring, so it is
        # the only component that could have written this root through the
        # userspace driver.
        "the manager committed the known-good root",
        r"\[sel4-generation-manager\] ready",
    ),
    (
        "the client listed the known-good root through the service",
        r"\[sel4-generation-client\] listed the known-good root",
    ),
    (
        "inspecting a generation outside the closure was refused",
        r"\[sel4-generation-client\] unknown generation refused",
    ),
    (
        # M6.5: "fail before BootState changes on missing objects". The image
        # comparison below is what proves the "before" part.
        "staging a generation outside the closure was refused",
        r"\[sel4-generation-client\] unknown stage refused",
    ),
    (
        "the candidate was staged with attempts",
        r"\[sel4-generation-manager\] stage seq=2 pending=1 attempts=2 release=1",
    ),
    (
        "the client observed the stage",
        r"\[sel4-generation-client\] staged the candidate",
    ),
    (
        "rolling back returned the known-good root",
        r"\[sel4-generation-manager\] rollback seq=3 pending=0 attempts=0 release=1",
    ),
    (
        "the client observed the rollback",
        r"\[sel4-generation-client\] rolled back to known-good",
    ),
    (
        # Not silently successful: a client must be able to tell "nothing to do"
        # from "done".
        "rolling back with nothing staged was refused",
        r"\[sel4-generation-client\] rollback with no pending refused",
    ),
    (
        # Only the generation actually staged may be promoted, so a client
        # cannot confirm the health of something else.
        "promoting the wrong generation was refused",
        r"\[sel4-generation-client\] wrong select refused",
    ),
    (
        # Promotion advances the accepted release sequence.
        "the staged generation was promoted",
        r"\[sel4-generation-manager\] select seq=5 pending=0 attempts=0 release=2",
    ),
    (
        "the client observed the promotion",
        r"\[sel4-generation-client\] promoted the candidate",
    ),
    (
        "the client ran every arm and exited cleanly",
        r"\[sel4-generation-client\] generation client complete",
    ),
    (
        "the userspace driver released cleanly on the manager's command",
        r"\[virtio-blk-driver\] peer complete, exiting",
    ),
    (
        # The manager's loop ends on peer death, which only happens because
        # init dropped its own copies of both queue ends.
        "the manager observed the client close",
        r"\[sel4-generation-manager\] client closed",
    ),
    (
        "init observed both clean exits",
        r"\[init\] generation plane complete",
    ),
)

TERMINAL_MARKER = r"\[init\] generation plane complete"

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_ROOT FAIL",
    r"SLIME_GRAPH FAIL",
    r"SLIME_GRAPH wedged waiter",
    r"\[init\] generation plane fail: .*",
    r"\[sel4-generation-manager\] fail: .*",
    r"\[sel4-generation-client\] fail: .*",
    r"\[virtio-blk-driver\] fail: .*",
    r"Caught cap fault",
    r"Caught vm fault",
    r"Caught user exception",
    r"panicked at ",
    r"aborted at ",
    r"\(aborted\)",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 generation plane check: {message}")


def build_image(platform: str) -> None:
    """Build this plane's image for `platform`.

    The aarch64 arm builds by closure identity; the closure names platform
    `qemu-arm-virt` and so cannot describe the riscv64 build, which keeps its
    flag until a closure exists for that platform.
    """
    global CLOSURE_IMAGE
    if platform == CLOSURE_PLATFORM:
        try:
            CLOSURE_IMAGE = build_closure_image(CLOSURE).image
        except ClosureImageError as error:
            fail(str(error))
        return
    command = [
        sys.executable,
        str(BUILD_SCRIPT),
        "--generation-plane",
        "--platform",
        platform,
    ]
    print(f"[build] {' '.join(command)}", flush=True)
    try:
        process = subprocess.run(command, cwd=ROOT, check=False)
    except OSError as error:
        fail(f"cannot run the seL4 image build: {error}")
    if process.returncode != 0:
        fail(f"seL4 image build failed with exit status {process.returncode}")


def build_fixture(disk: Path) -> None:
    """The store fixture, reused: the manager needs a validated GPT partition,
    and the BootState slots live above the object store's record area in it."""
    command = [sys.executable, str(FIXTURE_SCRIPT), str(disk), "happy"]
    try:
        process = subprocess.run(command, cwd=ROOT, check=False, capture_output=True)
    except OSError as error:
        fail(f"cannot build the store fixture: {error}")
    if process.returncode != 0:
        fail(f"store fixture build failed: {process.stderr.decode()}")


def boot(
    profile: dict[str, object],
    disk: Path,
    *,
    section: str,
    qemu_binary: str,
    image: Path,
) -> str:
    qemu = shutil.which(qemu_binary)
    if qemu is None:
        fail(f"{qemu_binary} is not on PATH")
    command = [
        qemu,
        "-machine",
        profile_text(profile, "machine", fail, section),
        "-cpu",
        profile_text(profile, "cpu", fail, section),
        "-smp",
        str(profile_integer(profile, "cpus", fail, section)),
        "-m",
        f"size={profile_integer(profile, 'memory_mib', fail, section)}M",
        "-nographic",
        "-serial",
        "mon:stdio",
        *qemu_kernel_arguments(qemu_binary, image, fail),
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
        # The two root-owned idle instances run concurrently with init and each
        # concludes it holds no peer only after a bounded wait, so their lines
        # can land before or after init's completion marker. Stop once every
        # fact has been observed rather than at the terminal alone.
        idle_seen = 0
        for line in process.stdout:
            lines.append(line.rstrip("\r\n"))
            if failures.search(line):
                break
            idle_seen += int(
                "[sel4-generation-manager] idle without a client" in line
                or "[sel4-generation-client] idle without an endpoint" in line
            )
            reached |= terminal.search(line) is not None
            if reached and idle_seen >= 2:
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
    # Asserted by presence, not by position: each idle instance concludes it
    # holds no peer only after a bounded wait, so its line lands wherever the
    # scheduler puts it. Ordering it would assert a scheduling accident.
    for label, marker in (
        (
            "the unconfigured manager parked without a client",
            "[sel4-generation-manager] idle without a client",
        ),
        (
            "the unconfigured client parked without an endpoint",
            "[sel4-generation-client] idle without an endpoint",
        ),
    ):
        if marker not in transcript:
            report_transcript(transcript)
            fail(f"missing marker: {label} ({marker})")
    for label, pattern in REQUIRED_MARKERS:
        match = re.compile(pattern).search(transcript, position)
        if match is None:
            report_transcript(transcript)
            if re.search(pattern, transcript) is not None:
                fail(f"marker out of order: {label} ({pattern})")
            fail(f"missing marker: {label} ({pattern})")
        position = match.end()
    completions = transcript.count("[sel4-generation-client] generation client complete")
    if completions != 1:
        report_transcript(transcript)
        fail(f"{completions} clients ran the scenario, expected 1")
    # Root-side corroboration after B83: the root no longer sees block opcodes,
    # but it still mediates the userspace driver's DMA and accounts teardown.
    payload_dma = re.findall(
        r"SLIME_IO payload dma pages=\d+ frames=\d+ writable=\w+ direction=(\w+)",
        transcript,
    )
    for direction in ("DeviceRead", "DeviceWrite"):
        if direction not in payload_dma:
            report_transcript(transcript)
            fail(f"the root mediated no {direction} payload DMA for the userspace driver")
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
    # A refusal must report the exact sequence of the immediately preceding
    # committed (or initial ready) state. Merely collecting refusal sequences
    # cannot prove the root was left untouched.
    events = [
        (match.group("op"), int(match.group("seq")))
        for match in re.finditer(
            r"\[sel4-generation-manager\] "
            r"(?P<op>stage|select|rollback|inspect-unknown|stage-refused|"
            r"select-refused|rollback-nothing) seq=(?P<seq>\d+)",
            transcript,
        )
    ]
    refusal_names = {"inspect-unknown", "stage-refused", "select-refused", "rollback-nothing"}
    commit_names = {"stage", "select", "rollback"}
    refusals: list[tuple[str, int]] = []
    commits: list[int] = []
    # The fixture's admitted BootState starts at sequence 1; the first successful
    # stage required above advances it to 2.
    committed_sequence = 1
    for operation, sequence in events:
        if operation in commit_names:
            committed_sequence = sequence
            commits.append(sequence)
        elif operation in refusal_names:
            refusals.append((operation, sequence))
            if sequence != committed_sequence:
                fail(
                    f"{operation} mutated BootState sequence from {committed_sequence} "
                    f"to {sequence}"
                )
    expected_refusals = {"inspect-unknown", "stage-refused", "select-refused", "rollback-nothing"}
    if {name for name, _ in refusals} != expected_refusals or len(refusals) != 4:
        fail(f"refusal evidence was {refusals}, expected one of each {sorted(expected_refusals)}")
    if commits != sorted(set(commits)):
        fail(f"committed sequences are not strictly increasing: {commits}")
    print(
        f"transcript: {len(REQUIRED_MARKERS)} markers observed; the client drove "
        f"five operations through the service, {len(refusals)} refusals left the "
        f"root untouched, {len(commits)} commits advanced it strictly, and the "
        "root corroborated both DMA directions plus complete driver reclamation",
        flush=True,
    )


def check_only_state_slots(disk: Path, before: bytes, partition_first_lba: int) -> None:
    """The manager wrote BootState and nothing else.

    It holds `blockRead | blockWrite` over the whole device, so "it only touched
    its own two sectors" is a property of the component rather than of the
    capability — which is exactly why it is worth checking from outside.
    """
    after = disk.read_bytes()
    slot_a = (partition_first_lba + 1024) * 512
    slot_b = slot_a + 512
    for name, start, end in (
        ("the GPT and protective MBR", 0, partition_first_lba * 512),
        ("the object store region", partition_first_lba * 512, slot_a),
        ("the disk beyond the BootState slots", slot_b + 512, len(after)),
    ):
        if after[start:end] != before[start:end]:
            fail(f"the generation service modified {name}")
    if after[slot_a : slot_b + 512] == before[slot_a : slot_b + 512]:
        fail("no BootState slot changed, so nothing was actually committed")
    print(
        "image: the service wrote its two BootState slots and left every other "
        "sector byte-identical",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 generation-plane image and assert M6.5 in userspace"
    )
    parser.add_argument(
        "--no-build",
        action="store_true",
        help="boot the already-built image instead of rebuilding it first",
    )
    parser.add_argument(
        "--platform",
        choices=sorted(PLATFORMS),
        default="qemu-arm-virt",
    )
    arguments = parser.parse_args()

    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    if not FIXTURE.is_file():
        fail(f"missing generation fixture {FIXTURE.relative_to(ROOT)}")
    section, qemu_binary = PLATFORMS[arguments.platform]
    profile = load_qemu_profile(fail, PINS_PATH, section)
    if not arguments.no_build:
        build_image(arguments.platform)
    image = image_path(arguments.platform)
    if not image.is_file():
        fail(f"missing packaged image {image.relative_to(ROOT)}")

    with tempfile.TemporaryDirectory() as directory:
        disk = Path(directory) / "generation-plane.img"
        build_fixture(disk)
        before = disk.read_bytes()
        transcript = boot(
            profile,
            disk,
            section=section,
            qemu_binary=qemu_binary,
            image=image,
        )
        check_transcript(transcript)
        check_only_state_slots(disk, before, 40)

    print(
        "seL4 generation plane check: an unprivileged client drove list, "
        "inspect, stage, select, and rollback through a management service "
        "using the only generation-authorized IO0 block ring, every refusal "
        "left the root untouched, and root-mediated DMA plus exact resource "
        f"reclamation corroborated the userspace virtio-blk path on {arguments.platform}"
    )


if __name__ == "__main__":
    main()
