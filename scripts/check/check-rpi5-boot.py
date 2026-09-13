#!/usr/bin/env python3

"""P4: qualify the named Raspberry Pi 5 by booting it and reading its UART.

This is a *physical* gate. It builds the pinned bcm2712 kernel, root task, and
loader, flattens them into the removable-media boot files, proves the bytes it
is about to boot are the ones its identity manifest describes, then reads the
board's serial console and requires the same ordered evidence
`check-sel4-root-boot.py` requires of the QEMU product.

What it deliberately does not do:

  * write to a block device. Copying onto removable media is the one step that
    can destroy an unrelated disk, so the operator does it and this gate only
    proves what was written matches what was built.
  * power-cycle the board. There is no admitted reset capability, so the
    operator resets it; the gate waits for the boot it was told to expect.
  * fall back to QEMU. A QEMU pass cannot complete a physical milestone
    (roadmap invariant 8), so a missing board, missing serial device, or
    missing media is a failure and never a skip.

Serial is read with `termios` rather than `pyserial`, which the pinned shell
does not carry: a raw tty at the contract's baud is a dozen lines of POSIX and
adds no dependency to a gate whose whole purpose is reproducibility.
"""

from __future__ import annotations

import argparse
import errno
import json
import os
import re
import select
import subprocess
import sys
import termios
import time
import tomllib
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from harness import sha256_file  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
IMAGE = ROOT / "build" / "slime-sel4-bcm2712-rpi5.elf"
MANIFEST = ROOT / "build" / "slime-sel4-bcm2712-rpi5.identity.json"
MEDIA = ROOT / "build" / "rpi5-media"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
MEDIA_SCRIPT = ROOT / "scripts" / "build" / "build-rpi5-media.py"
DEMO_CONTRACT = ROOT / "contracts" / "rpi5-ros2-demo" / "v2" / "fixtures" / "valid.zti"
PLATFORM = "bcm2712-rpi5"
TARGET_PROFILE = "aarch64-rpi5"

# A board that never prints must fail loudly rather than hang the gate. This is
# wall-clock from the moment the reader opens, so it also covers the operator's
# reset: the board is expected to be reset after the gate says it is listening.
BOOT_TIMEOUT_SECONDS = 180

# Ordered evidence, matched in this order. This is deliberately the QEMU root
# gate's chain narrowed to what a board can establish without any peripheral
# this milestone has not qualified: the root task admitted its generation, took
# ownership of untyped memory, acquired the real generic-timer IRQ and observed
# one delivered and acknowledged interrupt, staged and activated its children,
# served a request, observed a clean exit and a real fault, reclaimed both, and
# reached its ready state with nothing live.
#
# Numeric fields vary per build and are matched loosely; what is pinned is the
# ordering, the identities, and the terminal `live=0`.
REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        "allocator admitted nonzero kernel resources",
        r"SLIME_ROOT allocator slots=[1-9]\d* untypeds=[1-9]\d* bytes=[1-9]\d*",
    ),
    (
        "generation admitted",
        r"SLIME_ROOT generation identity=[0-9a-f]{8}",
    ),
    (
        "timer source acquired on the board's generic timer",
        r"SLIME_TIMER acquired irq=\d+ freq_hz=[1-9]\d*",
    ),
    (
        "timer interrupt delivered",
        r"SLIME_TIMER delivered badge=0x1 polls=\d+",
    ),
    (
        "timer expiry serviced and acknowledged",
        r"SLIME_TIMER serviced events=1 programming=\S",
    ),
    (
        "first child activated",
        r"SLIME_ROOT task activated task=0",
    ),
    (
        "second child activated",
        r"SLIME_ROOT task activated task=1",
    ),
    (
        "clean child reclaimed",
        r"SLIME_ROOT task reclaimed task=0 source=generation",
    ),
    (
        "faulted child reclaimed",
        r"SLIME_ROOT task reclaimed task=1 source=generation",
    ),
    (
        "graph drained with nothing live",
        r"SLIME_ROOT cleanup tasks=2 slots=\d+ live=0",
    ),
    (
        "root reached its ready state",
        r"SLIME_ROOT READY",
    ),
)

# Any of these in the transcript fails the gate before ordered matching runs, so
# a board that reaches `READY` through a degraded path cannot pass.
FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT fatal",
    r"SLIME_ROOT generation rejected",
    r"SLIME_ROOT panic",
    r"SLIME_ROOT allocator exhausted",
    r"SLIME_TIMER timeout",
    r"SLIME_TIMER unavailable",
    r"SLIME_ROOT device page unavailable",
    r"KERNEL INVALID VECTOR ENTRY",
    r"Kernel init failed",
    r"seL4 called fail",
    r"Caught cap fault",
    r"Caught vm fault",
    r"panicked at",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"rpi5 boot check: {message}")


def load_pins() -> dict[str, object]:
    if not PINS_PATH.is_file():
        fail(f"missing pins: {PINS_PATH.relative_to(ROOT)}")
    try:
        pins = tomllib.loads(PINS_PATH.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        fail(f"cannot read {PINS_PATH.relative_to(ROOT)}: {error}")
    if "bcm2712_rpi5" not in pins:
        fail("sel4/pins.toml declares no [bcm2712_rpi5] board profile")
    return pins


def board_profile(pins: dict[str, object]) -> dict[str, object]:
    """The board's pinned facts, each required rather than defaulted."""
    profile = pins["bcm2712_rpi5"]
    for key in ("board", "soc", "serial", "serial_baud", "boot_files"):
        if key not in profile:
            fail(f"[bcm2712_rpi5] declares no {key}")
    return profile


def build() -> None:
    for script, arguments, description in (
        (BUILD_SCRIPT, ["--platform", PLATFORM, "--skip-pin-check"], "board image"),
        (MEDIA_SCRIPT, [], "removable-media boot files"),
    ):
        command = [sys.executable, str(script), *arguments]
        print(f"[build {description}] {' '.join(command)}")
        try:
            process = subprocess.run(command, cwd=ROOT, check=False)
        except OSError as error:
            fail(f"cannot run {script.relative_to(ROOT)}: {error}")
        if process.returncode != 0:
            fail(f"building the {description} failed with status {process.returncode}")


def check_identity(profile: dict[str, object]) -> str:
    """The image, its manifest, and the media agree, and name this board.

    The media digest is what makes this more than a build check: the bytes the
    firmware loads are a flattened *copy* of the ELF, so a stale `kernel8.img`
    beside a freshly built ELF would otherwise boot silently and be reported as
    this build's evidence.
    """
    if not MANIFEST.is_file():
        fail(f"missing identity manifest: {MANIFEST.relative_to(ROOT)}")
    try:
        identity = json.loads(MANIFEST.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot read {MANIFEST.relative_to(ROOT)}: {error}")
    if identity.get("kind") != "slime-sel4-image-identity":
        fail(f"{MANIFEST.relative_to(ROOT)} is not a Slime seL4 image identity")
    if identity.get("platform") != PLATFORM:
        fail(
            f"{MANIFEST.relative_to(ROOT)} describes platform "
            f"{identity.get('platform')!r}, not {PLATFORM!r}"
        )
    if identity.get("target_profile") != TARGET_PROFILE:
        fail(
            f"{MANIFEST.relative_to(ROOT)} names target profile "
            f"{identity.get('target_profile')!r}, not {TARGET_PROFILE!r}"
        )
    if "qemu" in identity:
        fail(
            f"{MANIFEST.relative_to(ROOT)} carries QEMU launch facts, so it was "
            "not built for a physical board"
        )
    recorded = identity.get("image", {}).get("sha256")
    observed = sha256_file(IMAGE, fail)
    if recorded != observed:
        fail(
            f"{IMAGE.relative_to(ROOT)} is {observed}, but its manifest records "
            f"{recorded}; rebuild both together"
        )
    for name in profile["boot_files"]:
        path = MEDIA / name
        if not path.is_file():
            fail(
                f"missing boot file {path.relative_to(ROOT)}; run "
                f"`python3 {MEDIA_SCRIPT.relative_to(ROOT)}`"
            )
    return sha256_file(MEDIA / "kernel8.img", fail)


def check_media_matches_image(media_digest: str) -> None:
    """The flat image is this ELF's payload, not a leftover from another build.

    Rebuilt rather than trusted: the media script is deterministic, so
    re-flattening the current ELF and comparing digests proves the file on disk
    came from it. Without this the gate could boot an old `kernel8.img` and
    attribute its transcript to today's kernel.

    The scratch directory sits outside `build/rpi5-media` on purpose: that
    directory must contain exactly the pinned `boot_files` and nothing else, so
    the operator can copy all of it onto the boot partition. Writing scratch
    inside it made the media builder's own check fail.
    """
    verify = ROOT / "build" / "rpi5-media-verify"
    command = [sys.executable, str(MEDIA_SCRIPT), "--output", str(verify)]
    try:
        process = subprocess.run(command, cwd=ROOT, check=False, capture_output=True)
    except OSError as error:
        fail(f"cannot re-run the media builder: {error}")
    if process.returncode != 0:
        fail("re-running the media builder failed, so the media cannot be verified")
    rebuilt = sha256_file(verify / "kernel8.img", fail)
    if rebuilt != media_digest:
        fail(
            f"build/rpi5-media/kernel8.img is {media_digest}, but flattening the "
            f"current ELF yields {rebuilt}; the media is stale"
        )


def open_serial(device: Path, baud: int) -> int:
    """A raw tty at the pinned baud, or a named failure.

    `O_NONBLOCK` on open matters: a USB-serial device without carrier blocks
    `open` indefinitely otherwise, which is exactly the wedge this gate exists
    to report.
    """
    if not device.exists():
        fail(
            f"serial device {device} does not exist; attach the USB-UART adapter "
            "to the Pi 5 debug header and pass --serial"
        )
    try:
        fd = os.open(str(device), os.O_RDONLY | os.O_NOCTTY | os.O_NONBLOCK)
    except OSError as error:
        if error.errno in (errno.EACCES, errno.EPERM):
            fail(f"cannot open {device}: {error.strerror}; check device permissions")
        fail(f"cannot open {device}: {error.strerror}")
    try:
        attributes = termios.tcgetattr(fd)
    except termios.error as error:
        os.close(fd)
        fail(f"{device} is not a tty: {error}")
    speed = getattr(termios, f"B{baud}", None)
    if speed is None:
        os.close(fd)
        fail(f"the platform's termios has no constant for {baud} baud")
    # 8N1, no flow control, fully raw: every translation off, so the transcript
    # is the bytes the board sent.
    iflag, oflag, cflag, lflag, ispeed, ospeed, cc = attributes
    iflag = 0
    oflag = 0
    lflag = 0
    cflag = termios.CS8 | termios.CREAD | termios.CLOCAL
    cc = list(cc)
    cc[termios.VMIN] = 0
    cc[termios.VTIME] = 0
    try:
        termios.tcsetattr(
            fd, termios.TCSANOW, [iflag, oflag, cflag, lflag, speed, speed, cc]
        )
        termios.tcflush(fd, termios.TCIFLUSH)
    except termios.error as error:
        os.close(fd)
        fail(f"cannot configure {device} for {baud} baud 8N1: {error}")
    return fd


def capture(device: Path, baud: int, timeout: int) -> str:
    """Read the board's console until terminal evidence, failure, or timeout.

    The deadline is enforced by `select`, not checked after a blocking read, so
    a board that prints nothing at all still fails on time.
    """
    fd = open_serial(device, baud)
    terminal = re.compile(REQUIRED_MARKERS[-1][1])
    failures = [re.compile(pattern) for pattern in FAILURE_MARKERS]
    deadline = time.monotonic() + timeout
    pending = b""
    lines: list[str] = []
    print(f"[serial] reading {device} at {baud} baud; reset the board now")
    try:
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break
            try:
                ready, _, _ = select.select([fd], [], [], min(remaining, 1.0))
            except OSError as error:
                fail(f"waiting on {device} failed: {error}")
            if not ready:
                continue
            try:
                chunk = os.read(fd, 4096)
            except OSError as error:
                if error.errno in (errno.EAGAIN, errno.EWOULDBLOCK):
                    continue
                fail(f"reading {device} failed: {error}")
            if not chunk:
                continue
            pending += chunk
            *complete, pending = pending.split(b"\n")
            for raw in complete:
                line = raw.decode("utf-8", "replace").rstrip("\r")
                print(f"  {line}")
                lines.append(line)
            transcript = "\n".join(lines)
            if terminal.search(transcript) is not None:
                return transcript
            if any(pattern.search(transcript) is not None for pattern in failures):
                return transcript
    finally:
        os.close(fd)
    transcript = "\n".join(lines)
    if not lines:
        fail(
            f"no bytes arrived on {device} within {timeout}s; check the adapter "
            "wiring, that the board was reset, and that the media was written"
        )
    report_transcript(transcript)
    fail(f"the board did not reach its terminal marker within {timeout}s")


def monitor(device: Path, baud: int, timeout: int) -> None:
    """Print whatever arrives, assert nothing, exit on idle or deadline.

    A bring-up tool, not a gate. When the board is silent the useful question is
    "does *anything* come out of this wire", and every assertion in this file
    gets in the way of answering it — a firmware banner with no Slime markers is
    a pass here and a failure two functions up.

    Bounded rather than interactive on purpose: this repository's operator
    cannot drive `screen` or a serial console, so this returns on its own after
    `timeout` seconds, or after 10s of silence once bytes have started, instead
    of holding a session open.

    Bytes are printed with `repr`-style escaping for anything unprintable, since
    the first symptom of a baud mismatch is framing garbage rather than silence,
    and that must be visible rather than swallowed by a UTF-8 replacement char.
    """
    fd = open_serial(device, baud)
    deadline = time.monotonic() + timeout
    idle_grace = 10.0
    total = 0
    last = None
    pending = b""
    print(f"[monitor] {device} at {baud} baud 8N1, up to {timeout}s")
    print("[monitor] power-cycle or reset the board now; Ctrl-C to stop early")
    try:
        while time.monotonic() < deadline:
            if last is not None and time.monotonic() - last > idle_grace:
                print(f"[monitor] {idle_grace:.0f}s idle after {total} bytes; stopping")
                break
            try:
                ready, _, _ = select.select([fd], [], [], 1.0)
            except OSError as error:
                fail(f"waiting on {device} failed: {error}")
            if not ready:
                continue
            try:
                chunk = os.read(fd, 4096)
            except OSError as error:
                if error.errno in (errno.EAGAIN, errno.EWOULDBLOCK):
                    continue
                fail(f"reading {device} failed: {error}")
            if not chunk:
                continue
            total += len(chunk)
            last = time.monotonic()
            pending += chunk
            *complete, pending = pending.split(b"\n")
            for raw in complete:
                text = raw.rstrip(b"\r").decode("utf-8", "backslashreplace")
                print(f"  {text}", flush=True)
    except KeyboardInterrupt:
        print("\n[monitor] interrupted")
    finally:
        if pending:
            print(f"  {pending.decode('utf-8', 'backslashreplace')}", flush=True)
        os.close(fd)
    print(f"[monitor] {total} bytes received")
    if total == 0:
        print()
        print("Nothing arrived. In order of likelihood:")
        print("  1. TX/RX swapped. The Pi 5 debug header is pin 1 = board TX,")
        print("     pin 2 = GND, pin 3 = board RX, with pin 1 nearest the")
        print("     micro-HDMI ports. The adapter's RX goes to pin 1.")
        print("  2. GND not connected between adapter and board.")
        print("  3. The board is not running: no media, or wrong partition.")
        print("     `config.txt` sets `uart_2ndstage=1`, so the *firmware*")
        print("     should print before Slime does; silence means the link,")
        print("     not the image.")


def report_transcript(transcript: str) -> None:
    lines = transcript.splitlines()
    print("---- serial transcript (last 40 lines) ----")
    for line in lines[-40:]:
        print(line)
    print("---- end of serial transcript ----")


def check_transcript(transcript: str) -> None:
    for pattern in FAILURE_MARKERS:
        match = re.search(pattern, transcript)
        if match is not None:
            report_transcript(transcript)
            fail(f"failure marker in serial transcript: {match.group(0)!r}")
    position = 0
    for description, pattern in REQUIRED_MARKERS:
        match = re.compile(pattern).search(transcript, position)
        if match is None:
            report_transcript(transcript)
            if re.search(pattern, transcript) is not None:
                fail(f"marker out of order: {description} ({pattern})")
            fail(f"missing marker: {description} ({pattern})")
        position = match.end()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--serial",
        type=Path,
        help=(
            "the USB-UART device attached to the Pi 5 debug header, "
            "e.g. /dev/cu.usbserial-0001"
        ),
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=BOOT_TIMEOUT_SECONDS,
        help="seconds to wait for the board's terminal marker",
    )
    parser.add_argument(
        "--no-build",
        action="store_true",
        help="assert against artifacts already built",
    )
    parser.add_argument(
        "--monitor",
        action="store_true",
        help=(
            "print whatever the serial device emits and assert nothing; a "
            "bring-up aid that builds no artifacts and qualifies no board"
        ),
    )
    arguments = parser.parse_args()

    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    pins = load_pins()
    profile = board_profile(pins)

    if arguments.monitor:
        # Deliberately before the build and identity checks: when the wire is
        # silent, the question is whether anything reaches this host at all, and
        # rebuilding the image cannot answer it. Qualifies nothing.
        if arguments.serial is None:
            fail("--monitor needs --serial naming the USB-UART device")
        monitor(arguments.serial, profile["serial_baud"], arguments.timeout)
        return

    if not arguments.no_build:
        build()
    media_digest = check_identity(profile)
    check_media_matches_image(media_digest)

    if arguments.serial is None:
        fail(
            "no serial device given, so no board evidence can be observed; P4 "
            "requires an observed boot on the named Raspberry Pi 5. Attach the "
            "USB-UART adapter to the debug header and pass "
            "`--serial /dev/cu.usbserial-XXXX`. The artifacts are built and "
            "verified: copy build/rpi5-media/* onto the FAT32 boot partition first"
        )
    baud = profile["serial_baud"]
    transcript = capture(arguments.serial, baud, arguments.timeout)
    check_transcript(transcript)
    print(
        f"rpi5 boot check: {profile['board']} ({profile['soc']}) booted the "
        f"pinned {PLATFORM} image (media sha256 {media_digest[:16]}) and produced "
        f"ordered generation, timer, task, fault, and ready evidence on "
        f"{profile['serial']}"
    )


if __name__ == "__main__":
    main()
