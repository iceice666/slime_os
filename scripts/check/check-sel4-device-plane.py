#!/usr/bin/env python3

"""B83 gate: the product root does not reclaim the userspace block path.

The old P5.4.2 gate booted the component-graph image with a disk and required
the root to negotiate virtio-blk and read sector zero. IO2 moved that behaviour
to a supervised userspace driver, so retaining those markers would require the
dead post-admission root driver this gate must now forbid.

The live device and DMA proof is `just io_block_check`. This compatibility gate
stays registered because older roadmap and devlog entries name
`just sel4_device_check`; it now enforces the cutover structurally and delegates
the observed block behaviour to IO2's gate.
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

from closure_image import ClosureImageError, build as build_closure_image  # noqa: E402
from harness import profile_text, profile_integer, sha256_file  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
CLOSURE = "sel4"
IMAGE: Path | None = None
BOOT_TIMEOUT_SECONDS = 180
DISK_BYTES = 1 << 20

# The disk carries a signature only so the byte-identity assertion has
# non-zero content; the product root must not read or modify it.
DISK_SIGNATURE = b"SLIMEDSK"

MARKERS: tuple[tuple[str, str], ...] = (
    (
        "generation admission completed without a root device probe",
        r"SLIME_ROOT generation admitted number=1 executables=6 instances=6 grants=\d+ ",
    ),
    (
        "the ordinary component graph reached its resident userspace state",
        r"\[slisp\] resident input wait",
    ),
)

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_ROOT FAIL",
    r"SLIME_ROOT block dma ",
    r"SLIME_ROOT block ready ",
    r"SLIME_ROOT block read ",
    r"SLIME_ROOT virtio irq bound ",
    r"Caught cap fault",
    r"Caught vm fault",
    r"Caught user exception",
    r"panicked at ",
    r"aborted at ",
)

TERMINAL_MARKER = MARKERS[-1][1]


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 device plane check: {message}")


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
    global IMAGE
    try:
        built = build_closure_image(CLOSURE)
    except ClosureImageError as error:
        fail(str(error))
    actual = sha256_file(built.image, fail)
    if actual != built.digest():
        fail(
            f"{built.image} SHA-256 is {actual}, but the build result records "
            f"{built.digest()}; the image changed after it was built"
        )
    IMAGE = built.image


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
        # The one difference from every other seL4 gate's command line.
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
        fail(f"boot exceeded {BOOT_TIMEOUT_SECONDS}s without reaching userspace dispatch")
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
            fail(f"failure or retired block marker in serial transcript: {match.group(0)!r}")
    position = 0
    for label, pattern in MARKERS:
        match = re.compile(pattern).search(transcript, position)
        if match is None:
            report_transcript(transcript)
            if re.search(pattern, transcript) is not None:
                fail(f"marker out of order: {label} ({pattern})")
            fail(f"missing marker: {label} ({pattern})")
        position = match.end()
    print("transcript: ordinary product boot reached userspace with no root block path", flush=True)


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the product graph with an attached disk and assert the B83 cutover"
    )
    parser.add_argument(
        "--no-build",
        action="store_true",
        help="boot the already-built image instead of rebuilding it first",
    )
    arguments = parser.parse_args()

    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    pins = load_pins()
    if not arguments.no_build:
        build_image()
    if not IMAGE.is_file():
        fail(f"missing packaged image {IMAGE.relative_to(ROOT)}")
    profile = pins["qemu_arm_virt"]
    assert isinstance(profile, dict)
    with tempfile.TemporaryDirectory() as directory:
        disk = Path(directory) / "device-plane.img"
        # Non-zero content makes accidental root writes visible even though the
        # product path must not inspect the sector.
        image = bytearray(DISK_BYTES)
        image[: len(DISK_SIGNATURE)] = DISK_SIGNATURE
        original = bytes(image)
        disk.write_bytes(original)
        transcript = boot(profile, disk)
        check_transcript(transcript)
        # No pre-admission selector is present in this image, so byte identity
        # proves the ordinary product root left the attached disk untouched.
        after = disk.read_bytes()
        if after != original:
            differing = [
                index
                for index in range(0, len(original), 512)
                if after[index : index + 512] != original[index : index + 512]
            ]
            fail(
                "the product root modified the attached disk; sectors changed: "
                + ", ".join(str(index // 512) for index in differing[:8])
            )
    print(
        "seL4 device check: the component-graph root left the attached disk "
        "byte-identical and emitted no retired block-driver marker; "
        "io_block_check owns the observed userspace device path"
    )


if __name__ == "__main__":
    main()
