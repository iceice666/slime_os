#!/usr/bin/env python3

"""RP3: qualify the named Raspberry Pi 5 by booting it and reading its UART.

This is a *physical* gate. It builds the pinned bcm2712 kernel, root task, and
loader, flattens them into the boot files, proves the bytes it is about to boot
are the ones its identity manifest describes, serves them to the board with
`rpiboot`, then reads UART10 through the Raspberry Pi Debug Probe and requires
a board subset of the ordered evidence `check-sel4-root-boot.py` requires of
the QEMU product.

The board's EEPROM holds `BOOT_ORDER=0x3`: every power-on asks the host for
`config.txt` and `kernel8.img` over the USB-C cable, which also powers it. The
gate serves exactly the verified media plus rpiboot's own BCM2712 second stage,
`bootfiles.bin`, from the pinned shell.

What it deliberately does not do:

  * write to a block device. The board boots from the host over USB, so no
    removable medium and no SD card is written or read.
  * power-cycle the board. There is no admitted reset capability, so the
    operator plugs in the USB-C cable when the gate says it is serving.
  * fall back to QEMU. A QEMU pass cannot complete a physical milestone
    (roadmap invariant 8), so a missing board, probe, or rpiboot is a failure
    and never a skip.

Serial is read with `termios` rather than `pyserial`, which the pinned shell
does not carry: a raw tty at the contract's baud is a dozen lines of POSIX and
adds no dependency to a gate whose whole purpose is reproducibility. The
reader drains UART10 from before rpiboot starts until the gate ends: the Debug
Probe's USB serial stops for good when the host leaves its input unread while
the board prints.
"""

from __future__ import annotations

import argparse
import errno
import glob
import importlib.util
import json
import os
import re
import select
import shutil
import subprocess
import sys
import termios
import time
import tomllib
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from harness import sha256_file  # noqa: E402

import devloop_observations  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
IMAGE = ROOT / "build" / "slime-sel4-bcm2712-rpi5.elf"
MANIFEST = ROOT / "build" / "slime-sel4-bcm2712-rpi5.identity.json"
MEDIA = ROOT / "build" / "rpi5-media"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
MEDIA_SCRIPT = ROOT / "scripts" / "build" / "build-rpi5-media.py"
SERVED = ROOT / "build" / "rpi5-rpiboot"
LOGS = ROOT / "build" / "rpi5-boot"
ROOT_BOOT_GATE = ROOT / "scripts" / "check" / "check-sel4-root-boot.py"
PLATFORM = "bcm2712-rpi5"
TARGET_PROFILE = "aarch64-rpi5"
# The pinned shell exports both by store path, so the gate serves the second
# stage the pin names rather than whichever rpiboot the host has on PATH.
RPIBOOT_ENVIRONMENT = "SLIME_RPIBOOT"
BOOTFILES_ENVIRONMENT = "SLIME_RPIBOOT_BOOTFILES"
SERIAL_ENVIRONMENT = "RPI5_SERIAL"
# The AArch64 EL1 physical timer: PPI 14 plus the PPI base of 16, the value
# `slime-root/src/platform_timer.rs` binds on every AArch64 profile.
TIMER_IRQ = 30

# A board that never prints must fail loudly rather than hang the gate. This is
# wall-clock from the moment rpiboot starts serving, so it also covers the
# operator plugging in the board after the gate says it is serving.
BOOT_TIMEOUT_SECONDS = 180


def _root_boot_gate():
    spec = importlib.util.spec_from_file_location("sel4_root_boot_for_rpi5", ROOT_BOOT_GATE)
    if spec is None or spec.loader is None:
        raise SystemExit(f"rpi5 boot check: cannot load {ROOT_BOOT_GATE.relative_to(ROOT)}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


# The QEMU root gate's chain narrowed to what RP3 claims of a board without any
# peripheral it has not qualified: the root took ownership of untyped memory,
# acquired the generic timer and observed one delivered and acknowledged
# interrupt, admitted its generation, activated a child that printed and exited
# cleanly and one that faulted, reclaimed both, and reached its ready state with
# nothing live. Each entry names a description of `check-sel4-root-boot.py`'s
# chain, so a marker the QEMU product changes changes here too; the patterns
# and their order come from that chain and nowhere else.
BOARD_SUBSET: tuple[str, ...] = (
    "allocator admitted nonzero kernel resources",
    "timer source acquired",
    "timer interrupt delivered",
    "timer expiry serviced and acknowledged",
    "generation admitted",
    "clean-exit task activated",
    "child exited cleanly",
    "root observed the clean exit",
    "deliberate-fault task activated",
    "root observed the child fault",
    "the clean-exit task's arena-owned slots are accounted for",
    "the faulted task's arena-owned slots are accounted for",
    "both tasks reclaimed",
    "root reached ready",
)


def _board_markers(chain: tuple[tuple[str, str], ...]) -> tuple[tuple[str, str], ...]:
    named = {description for description, _ in chain}
    missing = [description for description in BOARD_SUBSET if description not in named]
    if missing:
        raise SystemExit(
            "rpi5 boot check: the QEMU root gate no longer names "
            + ", ".join(repr(description) for description in missing)
        )
    return tuple(entry for entry in chain if entry[0] in BOARD_SUBSET)


_ROOT_BOOT = _root_boot_gate()
REQUIRED_MARKERS: tuple[tuple[str, str], ...] = _board_markers(_ROOT_BOOT.REQUIRED_MARKERS)

# Any of these in the transcript fails the gate before ordered matching runs, so
# a board that reaches `READY` through a degraded path cannot pass. The QEMU
# root gate's list, plus the kernel's own early failures that QEMU never shows.
FAILURE_MARKERS: tuple[str, ...] = (
    *_ROOT_BOOT.FAILURE_MARKERS,
    r"KERNEL INVALID VECTOR ENTRY",
    r"Kernel init failed",
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
    for key in ("board", "soc", "serial", "serial_baud", "boot_files", "timer_frequency_hz"):
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
            f"serial device {device} does not exist; attach the Debug Probe "
            "to the Pi 5 UART10 header"
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


def serial_device(given: Path | None) -> Path:
    """--serial, then RPI5_SERIAL, then the only Debug Probe serial device present."""
    if given is not None:
        return given
    if named := os.environ.get(SERIAL_ENVIRONMENT):
        return Path(named)
    found = sorted(glob.glob("/dev/cu.usbmodem*")) + sorted(glob.glob("/dev/ttyACM*"))
    if len(found) != 1:
        fail(
            f"cannot choose the Debug Probe's UART device; found {found or 'none'}. "
            f"Attach the probe to UART10, or set {SERIAL_ENVIRONMENT} or pass --serial"
        )
    return Path(found[0])


def rpiboot_inputs() -> tuple[Path, Path]:
    """The pinned rpiboot binary and its BCM2712 second stage, from the shell."""
    paths = []
    for name in (RPIBOOT_ENVIRONMENT, BOOTFILES_ENVIRONMENT):
        value = os.environ.get(name, "")
        if not value:
            fail(f"{name} is unset; run the gate from `nix develop`, which pins rpiboot")
        path = Path(value)
        if not path.is_file():
            fail(f"{name} names {path}, which does not exist")
        paths.append(path)
    return paths[0], paths[1]


def stage(profile: dict[str, object], media_digest: str, bootfiles: Path) -> dict[str, str]:
    """The directory rpiboot serves: the verified boot files and the second stage.

    Copied, then hashed again from the copy, so the bytes rpiboot serves are
    the bytes the identity checks above approved.
    """
    if SERVED.exists():
        shutil.rmtree(SERVED)
    SERVED.mkdir(parents=True)
    for name in profile["boot_files"]:
        shutil.copyfile(MEDIA / name, SERVED / name)
    shutil.copyfile(bootfiles, SERVED / "bootfiles.bin")
    digests = {path.name: sha256_file(path, fail) for path in sorted(SERVED.iterdir())}
    if digests["kernel8.img"] != media_digest:
        fail("the staged kernel8.img differs from the verified media")
    return digests


def capture(device: Path, baud: int, timeout: int, rpiboot: Path) -> str:
    """Serve the staged files and read UART10 until terminal evidence, failure, or timeout.

    The deadline is enforced by `select`, not checked after a blocking read, so
    a board that prints nothing at all still fails on time. rpiboot runs with
    `-l`, so a board that resets is served again rather than left waiting; it
    exiting before the board finishes is a failure.
    """
    fd = open_serial(device, baud)
    terminal = re.compile(REQUIRED_MARKERS[-1][1])
    failures = [re.compile(pattern) for pattern in FAILURE_MARKERS]
    LOGS.mkdir(parents=True, exist_ok=True)
    served_log = LOGS / "rpiboot.log"
    serial_log = LOGS / "serial.log"
    pending = b""
    lines: list[str] = []
    process = None
    try:
        with served_log.open("w") as log:
            try:
                process = subprocess.Popen(
                    [str(rpiboot), "-l", "-v", "-d", str(SERVED)],
                    stdout=log,
                    stderr=subprocess.STDOUT,
                    start_new_session=True,
                )
            except OSError as error:
                fail(f"cannot start {rpiboot}: {error}")
        deadline = time.monotonic() + timeout
        print(
            f"[rpiboot] serving {SERVED.relative_to(ROOT)}; power the RPi 5 through "
            f"its USB-C cable now. Reading {device} at {baud} baud",
            flush=True,
        )
        with serial_log.open("wb") as raw_log:
            while True:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    break
                if process.poll() is not None:
                    fail(f"rpiboot exited with status {process.returncode}; see {served_log.relative_to(ROOT)}")
                try:
                    ready, _, _ = select.select([fd], [], [], min(remaining, 1.0))
                except OSError as error:
                    fail(f"waiting on {device} failed: {error}")
                if not ready:
                    continue
                try:
                    chunk = os.read(fd, 65536)
                except OSError as error:
                    if error.errno in (errno.EAGAIN, errno.EWOULDBLOCK):
                        continue
                    fail(f"reading {device} failed: {error}")
                if not chunk:
                    continue
                raw_log.write(chunk)
                raw_log.flush()
                pending += chunk
                *complete, pending = pending.split(b"\n")
                for raw in complete:
                    line = raw.decode("utf-8", "replace").rstrip("\r")
                    print(f"  {line}", flush=True)
                    lines.append(line)
                transcript = "\n".join(lines)
                if terminal.search(transcript) is not None:
                    return transcript
                if any(pattern.search(transcript) is not None for pattern in failures):
                    return transcript
    finally:
        os.close(fd)
        if process is not None and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
    transcript = "\n".join(lines)
    if not lines:
        fail(
            f"no bytes arrived on {device} within {timeout}s; check the Debug Probe "
            "on UART10, that the board's EEPROM has BOOT_ORDER=0x3, and that its "
            f"USB-C cable reaches this host (see {served_log.relative_to(ROOT)})"
        )
    report_transcript(transcript)
    fail(f"the board did not reach its terminal marker within {timeout}s")


def check_served() -> None:
    """rpiboot handed the board this image, not only some earlier boot's files."""
    served = (LOGS / "rpiboot.log").read_text(encoding="utf-8", errors="replace")
    for name in ("config.txt", "kernel8.img"):
        if f"File read: {name}" not in served:
            fail(f"the board finished, but rpiboot never served {name}; see build/rpi5-boot/rpiboot.log")


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
        print("  1. The Debug Probe's UART cable is not on the Pi 5's UART10")
        print("     header (the 3-pin JST between the micro-HDMI ports).")
        print("  2. The board is not running: nothing served it, or its EEPROM")
        print("     does not hold BOOT_ORDER=0x3. The EEPROM's BOOT_UART=1 and")
        print("     `config.txt`'s `uart_2ndstage=1` make the *firmware* print")
        print("     before Slime does; silence means the link, not the image.")


def report_transcript(transcript: str) -> None:
    lines = transcript.splitlines()
    print("---- serial transcript (last 40 lines) ----")
    for line in lines[-40:]:
        print(line)
    print("---- end of serial transcript ----")


def check_transcript(transcript: str, profile: dict[str, object]) -> tuple[int, int]:
    """The ordered board subset, then the facts that tell this board from QEMU.

    The generic timer's frequency is the board's own crystal, 54 MHz on the
    BCM2712, where QEMU `virt` reports 62.5 MHz; a transcript replayed from a
    QEMU boot fails here. Returns the markers matched and the observed frequency.
    """
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
    timer = re.search(r"SLIME_TIMER acquired irq=(\d+) freq_hz=(\d+)", transcript)
    if timer is None:
        fail("the timer marker disappeared after marker matching")
    if int(timer.group(1)) != TIMER_IRQ:
        fail(f"the root bound timer IRQ {timer.group(1)}, expected {TIMER_IRQ}")
    if int(timer.group(2)) != profile["timer_frequency_hz"]:
        fail(
            f"the generic timer runs at {timer.group(2)} Hz, but [bcm2712_rpi5] pins "
            f"{profile['timer_frequency_hz']} Hz; this transcript is not from the board"
        )
    return len(REQUIRED_MARKERS), int(timer.group(2))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--serial",
        type=Path,
        help=(
            f"the Debug Probe's UART device on UART10; defaults to ${SERIAL_ENVIRONMENT}, "
            "then the only /dev/cu.usbmodem* or /dev/ttyACM* present"
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
            "bring-up aid that builds no artifacts, serves nothing, and "
            "qualifies no board"
        ),
    )
    arguments = parser.parse_args()

    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    pins = load_pins()
    profile = board_profile(pins)
    device = serial_device(arguments.serial)

    if arguments.monitor:
        # Deliberately before the build and identity checks: when the wire is
        # silent, the question is whether anything reaches this host at all, and
        # rebuilding the image cannot answer it. Qualifies nothing.
        monitor(device, profile["serial_baud"], arguments.timeout)
        return

    rpiboot, bootfiles = rpiboot_inputs()
    if not arguments.no_build:
        build()
    media_digest = check_identity(profile)
    check_media_matches_image(media_digest)
    digests = stage(profile, media_digest, bootfiles)
    for name, digest in digests.items():
        print(f"[served] {name} sha256={digest}")

    transcript = capture(device, profile["serial_baud"], arguments.timeout, rpiboot)
    matched, frequency = check_transcript(transcript, profile)
    check_served()
    devloop_observations.record(casesObserved=matched, timerFrequencyHz=frequency)
    print(
        f"rpi5 boot check: {profile['board']} ({profile['soc']}) booted the "
        f"pinned {PLATFORM} image (kernel8.img sha256 {media_digest[:16]}) served by "
        f"rpiboot and produced {matched} ordered generation, timer, task, fault, and "
        f"ready markers on {profile['serial']} at the board's "
        f"{frequency} Hz generic timer"
    )


if __name__ == "__main__":
    main()
