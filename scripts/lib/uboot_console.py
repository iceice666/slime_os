"""Driving a board's U-Boot over a serial console, for physical gates.

A physical gate's job is to reach a firmware prompt it does not control, issue
commands whose exact wording belongs to the vendor, and read back evidence.
None of that is inferable from the code under test, so all of it is failure
surface: an absent adapter, a silent wire, a missed autoboot window, a command
that hangs, a prompt that never returns. Every one of those ends this module in
a named non-zero exit, because the alternative -- a gate that skips when the
hardware is missing -- is a gate that reports success for a board nobody
plugged in.

Two transports, one interface. A local tty is the ordinary case and is opened
raw with framing errors marked so they can be counted: a board wired at the
wrong baud produces plausible-looking mojibake otherwise, and counting framing
errors is what distinguishes that from a genuine transcript. A TCP endpoint
covers the board sitting on a different host, reached through a `socat` or
`ser2net` bridge and an SSH tunnel; the bridge does not forward line-discipline
state, so framing errors are reported as unobservable there rather than as
zero. Saying "unobserved" costs a sentence in a devlog; saying "zero" when
nothing was counted is a false claim about hardware.

This is a library. It contains no marker table and no board's facts; a gate
supplies those, and matching them is `scripts/lib/sel4_gate_markers.py`'s job
rather than this module's -- that is the matcher every seL4 plane gate and the
shared tamper control already share, and a second implementation of ordered
marker matching would be a second thing to keep honest.

`scripts/check/check-nt98690-boot.py` is this module's first consumer. The three
Milk-V Duo gates predate it and still carry their own copies of this machinery;
they are physically verified against a board that is not on this bench, and a
refactor whose regression test cannot be run is not an improvement. Migrating
them is a follow-up for the next session that has a Duo in front of it.
"""

from __future__ import annotations

import errno
import os
import re
import select
import socket
import termios
import time
from pathlib import Path
from typing import Callable

Reject = Callable[[str], None]

#: How a `--serial` value names a bridged endpoint rather than a device path.
TCP_PREFIX = "tcp:"


def open_serial(device: Path, baud: int, fail: Reject) -> int:
    """A raw tty at the pinned baud, or a named failure.

    `O_NONBLOCK` on open matters: a USB-serial device without carrier blocks
    `open` indefinitely otherwise, which is exactly the wedge a gate exists to
    report. `PARMRK | INPCK` is set so framing errors arrive as in-band markers
    and can be counted, instead of being indistinguishable from real NUL bytes.
    """
    if not device.exists():
        fail(f"serial device {device} does not exist; attach the adapter and pass --serial")
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
    _, _, _, _, _, _, cc = attributes
    iflag = termios.PARMRK | termios.INPCK
    oflag = 0
    lflag = 0
    cflag = termios.CS8 | termios.CREAD | termios.CLOCAL
    cc = list(cc)
    cc[termios.VMIN] = 0
    cc[termios.VTIME] = 0
    try:
        termios.tcsetattr(fd, termios.TCSANOW, [iflag, oflag, cflag, lflag, speed, speed, cc])
        termios.tcflush(fd, termios.TCIOFLUSH)
    except termios.error as error:
        os.close(fd)
        fail(f"cannot configure {device} for {baud} baud 8N1: {error}")
    return fd


class Console:
    """A board console over a local tty or a bridged TCP endpoint.

    `framing_errors` is an `int` on a tty and `None` on TCP, and callers are
    expected to report the difference rather than flatten it to a number.
    """

    def __init__(self, endpoint: str, baud: int, fail: Reject) -> None:
        self._fail = fail
        self.endpoint = endpoint
        if endpoint.startswith(TCP_PREFIX):
            host, _, port = endpoint[len(TCP_PREFIX) :].rpartition(":")
            if not host or not port.isdigit():
                fail(f"serial endpoint {endpoint!r} must look like tcp:HOST:PORT")
            try:
                self._socket = socket.create_connection((host, int(port)), timeout=10)
            except OSError as error:
                fail(
                    f"cannot reach the serial bridge at {host}:{port}: {error}; "
                    "start it on the board's host and forward the port"
                )
            self._socket.setblocking(False)
            self.fd = self._socket.fileno()
            self.framing_errors: int | None = None
        else:
            self._socket = None
            self.fd = open_serial(Path(endpoint), baud, fail)
            self.framing_errors = 0

    def describe(self) -> str:
        if self._socket is not None:
            return f"{self.endpoint} (framing errors unobservable over a TCP bridge)"
        return f"{self.endpoint} at 8N1"

    def close(self) -> None:
        if self._socket is not None:
            self._socket.close()
        else:
            os.close(self.fd)

    def _strip_markers(self, raw: bytes) -> bytes:
        """Remove PARMRK error markers (\\377\\000X), counting each one."""
        if self.framing_errors is None:
            return raw
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
        if self._socket is not None:
            self._socket.sendall(data)
        else:
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
                self._fail(f"waiting on {self.endpoint} failed: {error}")
            if not ready:
                continue
            try:
                if self._socket is not None:
                    chunk = self._socket.recv(65536)
                    if chunk == b"":
                        self._fail(f"the serial bridge at {self.endpoint} closed the connection")
                else:
                    chunk = os.read(self.fd, 65536)
            except OSError as error:
                if error.errno in (errno.EAGAIN, errno.EWOULDBLOCK):
                    continue
                self._fail(f"reading {self.endpoint} failed: {error}")
            if chunk:
                collected += self._strip_markers(chunk)
        return collected.decode("utf-8", "replace")

    def flush_input(self) -> None:
        """Discard anything already buffered, so the next read is this command's."""
        if self._socket is not None:
            self.read_for(0.2)
        else:
            try:
                termios.tcflush(self.fd, termios.TCIFLUSH)
            except termios.error:
                pass


def reach_uboot(
    console: Console,
    prompt: str,
    window: float,
    fail: Reject,
    *,
    key: bytes = b"\r",
    interval: float = 0.05,
) -> str:
    """Leave the board sitting at its U-Boot prompt, or fail saying why not.

    Returns everything the board printed on the way there. For a board that
    reset itself, that text is the recovery evidence -- its firmware banner --
    and a gate that boots repeatedly reads it there rather than spending a
    silent window it would have to interrupt anyway to stage the next boot.

    The interrupt key is sent repeatedly from before the board is powered,
    because a U-Boot built with `bootdelay=0` evaluates `tstc()` exactly once:
    the byte has to already be in the receive FIFO when that single check runs.
    Spamming it is the only thing that reliably wins that race, and it is what
    the board's own vendor tooling does.

    The prompt is then confirmed by a bare carriage return rather than trusted
    from the first sighting, because the prompt string also appears in the
    scrollback of a board that has since carried on booting.
    """
    pattern = re.compile(re.escape(prompt))

    console.write(b"\r")
    opening = console.read_for(1.5)
    if pattern.search(opening):
        print("[console] already at the prompt; resetting for a clean boot")
        console.write(b"reset\r")
    elif re.search(r"login:\s*$", opening):
        fail(
            "the board is sitting at a vendor login prompt this gate has no "
            "credentials for; power-cycle it and run again"
        )
    elif re.search(r"[#$] $", opening):
        print("[console] at a vendor shell; rebooting")
        console.write(b"reboot\r")
    elif opening.strip():
        print("[console] board is talking; waiting for the next boot to interrupt")
    else:
        print(f"[console] no output yet — power-cycle the board now (waiting up to {window:.0f}s)")

    seen_any_byte = bool(opening.strip())
    collected = opening
    deadline = time.monotonic() + window
    while time.monotonic() < deadline:
        console.write(key)
        chunk = console.read_for(interval)
        if chunk:
            seen_any_byte = True
            collected += chunk
        if pattern.search(collected[-400:]):
            break
    else:
        if not seen_any_byte:
            fail(
                f"nothing arrived on {console.endpoint} in {window:.0f}s — no bytes at "
                "all, so this is a wiring, adapter, or baud fault rather than a "
                "board that failed to boot"
            )
        fail(
            f"the board printed but never reached the {prompt!r} prompt within "
            f"{window:.0f}s; the last 400 characters were:\n{collected[-400:]}"
        )

    console.read_for(0.6)
    console.flush_input()
    for _ in range(4):
        console.write(b"\r")
        if pattern.search(console.read_for(1.0)):
            print(f"[console] at the {prompt!r} prompt")
            return collected
    fail(f"the {prompt!r} prompt appeared but does not answer a carriage return")
    raise AssertionError("unreachable")


def send_command(
    console: Console,
    command: str,
    prompt: str,
    timeout: float,
    fail: Reject,
) -> str:
    """Issue one U-Boot command and return everything it printed.

    Completion is the prompt coming back. That is the only sentinel available:
    this U-Boot has no `echo`, so a command cannot be followed by a marker of
    the gate's own choosing.
    """
    console.flush_input()
    console.write(command.encode() + b"\r")
    collected = ""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        collected += console.read_for(0.2)
        if prompt in collected[len(command) :]:
            return collected
    fail(
        f"`{command}` did not return to the {prompt!r} prompt within {timeout:.0f}s; "
        f"the board printed:\n{collected[-400:]}"
    )
    raise AssertionError("unreachable")


def report_transcript(transcript: str, lines: int = 40) -> None:
    body = transcript.replace("\r", "").splitlines()
    print(f"--- last {min(lines, len(body))} of {len(body)} transcript lines ---")
    for line in body[-lines:]:
        print(f"  {line}")
    print("--- end transcript ---")
