#!/usr/bin/env python3

"""P3.D: qualify the named Milk-V Duo by booting a payload on it and reading its UART.

Physical gate. It proves the deployed bytes are this build's, deploys them to the
board over its own USB-NCM link, drives the board's U-Boot over serial, starts the
payload, and requires ordered evidence on the console. A missing board, link,
serial device, or marker is a failure and never a skip, so no QEMU pass and no
absent adapter can complete this milestone.

Why this board needs its own gate rather than reusing `check-rpi5-boot.py`:

  * There is no removable-media step. The operator's laptop has no SD reader, so
    the payload is written into the board's FAT `/boot` over USB-NCM while the
    stock vendor Linux is running, and booted from U-Boot afterwards. The card
    never leaves the board.
  * The launch path is `bootm` on a FIT. This board's vendor U-Boot 2021.10 is
    built without `go`, `booti`, `bootelf`, `loadx`/`loady`, and TFTP, so a FIT
    is the only compiled-in way to transfer control. `sel4/pins.toml
    [cv1800b_duo].uboot_launch` pins that fact.
  * Recovery is in-band. Stock `/boot/boot.sd` is never modified, so letting
    autoboot run returns the board to vendor Linux and the next iteration's
    deploy step.

The gate is deliberately two-phase, because the two phases fail for different
reasons and the operator needs to know which:

  1. deploy — the board must be reachable on its USB-NCM address and accept the
     payload, verified by digest read back from the target;
  2. boot — U-Boot must be reached over serial, load the FIT, and the payload
     must print its ordered markers.
"""

from __future__ import annotations

import argparse
import errno
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
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-duo-payload.py"
PAYLOAD_DIR = ROOT / "build" / "duo-payload"
FIT = PAYLOAD_DIR / "smoke.itb"

# Wall-clock budget for the whole boot phase, from the moment the reader opens.
# It covers the vendor reboot, the autoboot window, the FIT load, and the
# payload's own output, so a board that prints nothing still fails on time.
BOOT_TIMEOUT_SECONDS = 150

# Ordered evidence, matched in this order.
#
# This is the narrowest chain that distinguishes "our payload ran in S-mode on
# this SoC" from every cheaper explanation: U-Boot echoing, a stale image, or
# the vendor kernel booting instead. What is pinned is the ordering and the
# identities; the numeric register values vary per boot and are matched loosely.
#
# `satp=0x0000000000000000` is load-bearing rather than incidental: it is the
# evidence that the payload received control with translation off, which is the
# state an seL4 elfloader requires. `sxstatus` is the T-Head S-mode extension
# mirror; its bit 21 is MAEE, which changes Sv39 PTE bits 60--63. Both the full
# register and the decoded bit are required so the platform port can select its
# page-table encoding from observed firmware state rather than a board guess.
REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        "U-Boot selected our FIT configuration",
        r"Using 'config-duo' configuration",
    ),
    (
        "the FIT's payload subimage passed its integrity hash",
        r"Trying 'kernel-1' kernel subimage\s+Verifying Hash Integrity \.\.\. crc32\+ OK",
    ),
    (
        "the FIT's device tree passed its integrity hash",
        r"Trying 'fdt-duo' fdt subimage\s+Verifying Hash Integrity \.\.\. crc32\+ OK",
    ),
    (
        "control transferred out of U-Boot",
        r"Starting kernel \.\.\.",
    ),
    (
        "the payload reached its S-mode entry",
        r"=== SLIME_DUO smoke payload: S-mode entry reached ===",
    ),
    (
        "firmware handed over a boot hart id",
        r"SLIME_DUO hart      = 0x[0-9a-f]{16}",
    ),
    (
        "firmware handed over a device tree pointer in DRAM",
        r"SLIME_DUO dtb       = 0x0{8}8[0-9a-f]{7}",
    ),
    (
        "the payload entered with translation disabled",
        r"SLIME_DUO satp      = 0x0{16}",
    ),
    (
        "the T-Head extension state is readable in S-mode",
        r"SLIME_DUO sxstatus  = 0x[0-9a-f]{16}",
    ),
    (
        "the C906 MAEE state is enabled for the seL4 page-table encoding",
        r"SLIME_DUO maee      = 0x0{15}1",
    ),
    (
        "the SBI timebase counter is readable and nonzero",
        r"SLIME_DUO rdtime    = 0x0{8}[0-9a-f]*[1-9a-f][0-9a-f]*",
    ),
    (
        "the payload completed its own checks",
        r"SLIME_DUO PAYLOAD_OK",
    ),
    (
        "the payload returned control without stranding the board",
        r"SLIME_DUO returning to U-Boot",
    ),
)

# Any of these fails the gate before ordered matching runs, so a board that
# prints PAYLOAD_OK through a degraded path cannot pass.
FAILURE_MARKERS: tuple[str, ...] = (
    r"Bad Linux RISCV Image magic",
    r"Bad FIT kernel image format",
    r"No FIT subimage unit name",
    r"Bad hash value for",
    r"Bad Data Hash",
    r"Unsupported Architecture",
    r"Device tree not found or missing FDT support",
    r"ERROR: can't get kernel image",
    r"Must RESET board to recover",
    r"Unhandled exception",
    r"Oops",
    r"Kernel panic",
    r"SLIME_DUO FAULT",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"duo boot check: {message}")


def load_pins() -> dict[str, object]:
    if not PINS_PATH.is_file():
        fail(f"{PINS_PATH.relative_to(ROOT)} is missing")
    with PINS_PATH.open("rb") as handle:
        return tomllib.load(handle)


def board_profile(pins: dict[str, object]) -> dict[str, object]:
    """The board's pinned facts, each required rather than defaulted."""
    profile = pins.get("cv1800b_duo")
    if not isinstance(profile, dict):
        fail(
            "sel4/pins.toml has no [cv1800b_duo] table; the board's pinned facts "
            "are what this gate checks the payload and link against"
        )
    for key in (
        "soc",
        "board",
        "serial_baud",
        "dram_base",
        "payload_load_address",
        "fit_staging_address",
        "usb_ncm_address",
        "boot_partition",
        "uboot_prompt",
        "uboot_launch",
    ):
        if key not in profile:
            fail(f"sel4/pins.toml [cv1800b_duo] does not pin {key!r}")
    return profile


def build() -> None:
    if not BUILD_SCRIPT.is_file():
        fail(f"{BUILD_SCRIPT.relative_to(ROOT)} is missing")
    process = subprocess.run([sys.executable, str(BUILD_SCRIPT)], cwd=ROOT)
    if process.returncode != 0:
        fail(f"building the Duo payload failed with status {process.returncode}")


def check_identity(profile: dict[str, object]) -> str:
    """The FIT exists, and its load address is the one this board pins.

    A FIT linked for a different address would still load and still print
    U-Boot's own progress lines, then fault or hang. Comparing the pinned
    address to the built artifact's own metadata catches that before the board
    is touched.
    """
    if not FIT.is_file():
        fail(
            f"{FIT.relative_to(ROOT)} is missing; run the build step "
            "(`just duo_payload_check`) first"
        )
    manifest = PAYLOAD_DIR / "identity.json"
    if not manifest.is_file():
        fail(f"{manifest.relative_to(ROOT)} is missing; the build did not complete")
    import json

    identity = json.loads(manifest.read_text())
    for key, pinned in (
        ("load_address", profile["payload_load_address"]),
        ("entry_address", profile["payload_load_address"]),
    ):
        built = identity.get(key)
        if built != pinned:
            fail(
                f"the built payload's {key} is {built!r} but "
                f"sel4/pins.toml [cv1800b_duo] pins {pinned!r}"
            )
    if identity.get("board") != profile["board"]:
        fail(
            f"the built payload names board {identity.get('board')!r}, "
            f"not the pinned {profile['board']!r}"
        )
    digest = sha256_file(FIT, fail)
    if identity.get("fit_sha256") != digest:
        fail(
            "the FIT on disk does not match the digest its identity manifest "
            "records; the build is stale"
        )
    return digest


def ssh_base(key: Path) -> list[str]:
    return [
        "-i",
        str(key),
        "-o",
        "IdentitiesOnly=yes",
        "-o",
        "StrictHostKeyChecking=no",
        "-o",
        "UserKnownHostsFile=/dev/null",
        "-o",
        "LogLevel=ERROR",
        "-o",
        "ConnectTimeout=10",
    ]


def deploy(profile: dict[str, object], key: Path, digest: str, fit: Path = FIT) -> None:
    """Write the FIT into the board's FAT boot partition and verify it landed.

    The digest is read back from the target rather than trusted from the copy,
    because a short write to a FAT partition is silent and the next phase would
    then boot a truncated image and fail for the wrong reason.
    """
    host = str(profile["usb_ncm_address"])
    if not key.is_file():
        fail(
            f"{key} is missing; this gate needs a key the board already accepts. "
            "Bootstrap it once over the serial console, then re-run."
        )
    reachable = subprocess.run(
        ["ping", "-c", "1", "-W", "3", host], capture_output=True
    )
    if reachable.returncode != 0:
        fail(
            f"the board is not reachable at {host}; it must be running its vendor "
            "Linux with the USB-NCM gadget up for the deploy phase. If a previous "
            "run left it at the U-Boot prompt, let autoboot run and retry."
        )
    name = fit.name
    # `-O` selects the legacy SCP protocol: the board's dropbear ships no
    # sftp-server, so a modern sftp-backed scp fails outright.
    copy = subprocess.run(
        ["scp", "-O", *ssh_base(key), str(fit), f"root@{host}:/boot/{name}"],
        capture_output=True,
        text=True,
    )
    if copy.returncode != 0:
        fail(f"copying the payload to the board failed: {copy.stderr.strip()}")
    readback = subprocess.run(
        ["ssh", *ssh_base(key), f"root@{host}", f"sha256sum /boot/{name}; sync"],
        capture_output=True,
        text=True,
    )
    if readback.returncode != 0:
        fail(f"reading the payload's digest back failed: {readback.stderr.strip()}")
    if digest not in readback.stdout:
        fail(
            "the payload's digest on the board does not match the built FIT; "
            f"expected {digest}, target reported {readback.stdout.strip()!r}"
        )
    print(f"[deploy] /boot/{name} on {host} matches {digest[:16]}…")


def open_serial(device: Path, baud: int) -> int:
    """A raw tty at the pinned baud, or a named failure.

    `O_NONBLOCK` on open matters: a USB-serial device without carrier blocks
    `open` indefinitely otherwise, which is exactly the wedge this gate exists
    to report. `PARMRK | INPCK` is set so framing errors arrive as markers and
    are counted, instead of being indistinguishable from real NUL bytes.
    """
    if not device.exists():
        fail(
            f"serial device {device} does not exist; attach the USB-UART adapter "
            "to the Duo's UART0 header (pins 16/17/18) and pass --serial"
        )
    try:
        fd = os.open(str(device), os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
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
    iflag, oflag, cflag, lflag, ispeed, ospeed, cc = attributes
    iflag = termios.PARMRK | termios.INPCK
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
        termios.tcflush(fd, termios.TCIOFLUSH)
    except termios.error as error:
        os.close(fd)
        fail(f"cannot configure {device} for {baud} baud 8N1: {error}")
    return fd


class Console:
    """A serial console that counts framing errors instead of hiding them."""

    def __init__(self, device: Path, baud: int) -> None:
        self.fd = open_serial(device, baud)
        self.device = device
        self.framing_errors = 0

    def close(self) -> None:
        os.close(self.fd)

    def _strip_markers(self, raw: bytes) -> bytes:
        """Remove PARMRK error markers (\\377\\000X), counting each one."""
        out = bytearray()
        index = 0
        while index < len(raw):
            if raw[index] == 0o377 and index + 2 < len(raw) and raw[index + 1] == 0:
                self.framing_errors += 1
                index += 3
            elif raw[index] == 0o377 and index + 1 < len(raw) and raw[index + 1] == 0o377:
                out.append(0o377)
                index += 2
            else:
                out.append(raw[index])
                index += 1
        return bytes(out)

    def write(self, data: bytes) -> None:
        os.write(self.fd, data)

    def read_for(self, seconds: float) -> str:
        collected = b""
        deadline = time.monotonic() + seconds
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break
            try:
                ready, _, _ = select.select([self.fd], [], [], min(remaining, 0.1))
            except OSError as error:
                fail(f"waiting on {self.device} failed: {error}")
            if not ready:
                continue
            try:
                chunk = os.read(self.fd, 65536)
            except OSError as error:
                if error.errno in (errno.EAGAIN, errno.EWOULDBLOCK):
                    continue
                fail(f"reading {self.device} failed: {error}")
            if chunk:
                collected += self._strip_markers(chunk)
        return collected.decode("utf-8", "replace")


def reach_uboot(console: Console, prompt: str, window: float) -> None:
    """Reboot the board and stop its autoboot, leaving it at the U-Boot prompt.

    Only SPACE is sent. Ctrl-C would also interrupt autoboot, but at the prompt
    this U-Boot echoes `<INTERRUPT>` and would fight the commands sent next.
    """
    pattern = re.compile(re.escape(prompt))
    console.write(b"\r")
    tail = console.read_for(1.5)
    if pattern.search(tail):
        console.write(b"reset\r")
    elif "#" in tail:
        console.write(b"reboot\r")
    else:
        print("[serial] no prompt yet; reset the board now if it does not respond")

    deadline = time.monotonic() + window
    seen = ""
    while time.monotonic() < deadline:
        console.write(b" ")
        seen += console.read_for(0.1)
        if pattern.search(seen[-400:]):
            break
    else:
        if not seen.strip():
            fail(
                f"no bytes arrived on {console.device} within {window:.0f}s; check the "
                "adapter wiring on pins 16/17/18 and that the board has power"
            )
        fail(
            f"the board never reached its pinned U-Boot prompt {prompt!r} within "
            f"{window:.0f}s"
        )

    # Stop poking, drain the backlog, then confirm the prompt answers a bare CR.
    console.read_for(0.6)
    termios.tcflush(console.fd, termios.TCIFLUSH)
    for _ in range(4):
        console.write(b"\r")
        if pattern.search(console.read_for(1.5)[-200:]):
            return
    fail(f"the U-Boot prompt {prompt!r} appeared but does not answer commands")


def load_and_start(
    console: Console,
    profile: dict[str, object],
    fit: Path = FIT,
    config: str = "config-duo",
    read_seconds: float = 10.0,
) -> str:
    """`fatload` the FIT and `bootm` it, returning the payload's transcript."""
    staging = str(profile["fit_staging_address"])
    partition = str(profile["boot_partition"])
    console.write(f"fatload {partition} {staging} {fit.name}\r".encode())
    loaded = console.read_for(4.0)
    # This U-Boot prints "<N> bytes read in <T> ms"; accept the older
    # "Bytes transferred = <N>" wording too, and require a nonzero count.
    match = re.search(r"(\d+)\s+bytes read|Bytes transferred\s*=\s*(\d+)", loaded)
    if match is None:
        fail(
            f"`fatload {partition} {staging} {fit.name}` reported no transfer; "
            "the payload is not on the board's boot partition"
        )
    count = int(next(group for group in match.groups() if group))
    if count == 0:
        fail("fatload reported a zero-byte transfer")
    print(f"[serial] fatload staged {count} bytes at {staging}")

    launch = str(profile["uboot_launch"]).format(staging=staging, config=config)
    console.write(launch.encode() + b"\r")
    return console.read_for(read_seconds)


def report_transcript(transcript: str) -> None:
    lines = transcript.splitlines()
    print("---- serial transcript (last 40 lines) ----")
    for line in lines[-40:]:
        print(f"  {line}")
    print("---- end of serial transcript ----")


def check_transcript(transcript: str) -> None:
    for pattern in FAILURE_MARKERS:
        if re.search(pattern, transcript) is not None:
            report_transcript(transcript)
            fail(f"the transcript contains the failure marker {pattern!r}")
    position = 0
    for description, pattern in REQUIRED_MARKERS:
        match = re.compile(pattern).search(transcript, position)
        if match is None:
            report_transcript(transcript)
            fail(
                f"the transcript never showed {description} "
                f"(expected {pattern!r} after offset {position})"
            )
        position = match.end()


def monitor(device: Path, baud: int, timeout: int) -> None:
    """Print whatever arrives, assert nothing, exit on idle or deadline.

    A bring-up aid, not a gate. When the wire is silent the useful question is
    whether any byte reaches this host, and every assertion in this file gets in
    the way of answering it.
    """
    console = Console(device, baud)
    print(f"[monitor] reading {device} at {baud} baud for up to {timeout}s")
    try:
        deadline = time.monotonic() + timeout
        idle_limit = 10.0
        last_byte = time.monotonic()
        total = 0
        while time.monotonic() < deadline:
            text = console.read_for(0.5)
            if text:
                total += len(text)
                last_byte = time.monotonic()
                sys.stdout.write(text)
                sys.stdout.flush()
            elif total and time.monotonic() - last_byte > idle_limit:
                break
        print(
            f"\n[monitor] {total} characters, "
            f"{console.framing_errors} framing errors"
        )
        if total == 0:
            print("[monitor] nothing arrived: the wire, the adapter, or the board.")
    finally:
        console.close()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--serial",
        type=Path,
        help="the board's UART0 device, e.g. /dev/ttyUSB0",
    )
    parser.add_argument(
        "--key",
        type=Path,
        default=Path.home() / ".ssh" / "slime_duo",
        help="private key the board's dropbear already accepts",
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=BOOT_TIMEOUT_SECONDS,
        help="wall-clock budget for the boot phase",
    )
    parser.add_argument(
        "--monitor",
        action="store_true",
        help="bring-up aid: print the console and assert nothing",
    )
    parser.add_argument(
        "--transcript",
        type=Path,
        help="write the raw payload transcript here",
    )
    arguments = parser.parse_args()

    pins = load_pins()
    profile = board_profile(pins)
    baud = int(profile["serial_baud"])  # type: ignore[arg-type]

    if arguments.monitor:
        if arguments.serial is None:
            fail("--monitor needs --serial")
        monitor(arguments.serial, baud, arguments.timeout)
        return

    if arguments.serial is None:
        fail(
            "no serial device given, so no board evidence can be observed; "
            "P3.D requires an observed boot on the named Milk-V Duo"
        )

    build()
    digest = check_identity(profile)
    deploy(profile, arguments.key, digest)

    console = Console(arguments.serial, baud)
    try:
        reach_uboot(console, str(profile["uboot_prompt"]), min(arguments.timeout, 90))
        transcript = load_and_start(console, profile)
    finally:
        framing_errors = console.framing_errors
        console.close()

    if arguments.transcript:
        arguments.transcript.write_text(transcript)
        print(f"[serial] transcript written to {arguments.transcript}")

    check_transcript(transcript)
    report_transcript(transcript)

    print(
        f"duo boot check: {profile['board']} booted the pinned payload in S-mode "
        f"at {profile['payload_load_address']}, {framing_errors} framing errors, "
        "every ordered marker observed"
    )


if __name__ == "__main__":
    main()
