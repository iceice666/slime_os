"""Shared image and QEMU process mechanisms for seL4 plane gates."""

from __future__ import annotations

import json
import shutil
import subprocess
import threading
from collections.abc import Callable, Sequence
from pathlib import Path
from re import Pattern
from typing import NoReturn

from harness import ROOT, load_qemu_profile, profile_integer, profile_text, sha256_file

Reject = Callable[[str], NoReturn]


def verify_image_identity(*, image: Path, manifest: Path, variant: str, fail: Reject) -> None:
    """Verify that a built image matches its declared variant and digest."""
    if not image.is_file():
        fail(f"image missing: {image}")
    if not manifest.is_file():
        fail(f"identity manifest missing: {manifest}")
    try:
        identity = json.loads(manifest.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot parse identity manifest: {error}")
    if not isinstance(identity, dict):
        fail("identity manifest must contain an object")
    if identity.get("variant") != variant:
        fail(f"wrong image variant {identity.get('variant')!r}")
    image_identity = identity.get("image")
    if not isinstance(image_identity, dict):
        fail("identity manifest has no image record")
    if image_identity.get("sha256") != sha256_file(image, fail):
        fail("packaged image digest does not match identity manifest")


def qemu_base_command(*, image: Path, fail: Reject, pins_path: Path | None = None) -> list[str]:
    """Build the pinned qemu-arm-virt command shared by plane gates."""
    qemu = shutil.which("qemu-system-aarch64")
    if qemu is None:
        fail("qemu-system-aarch64 is not on PATH")
    profile = load_qemu_profile(fail, pins_path)
    return [
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
        str(image),
    ]


def run_plane(
    *,
    image: Path,
    timeout: int,
    terminal_condition: Pattern[str],
    fail: Reject,
    additional_arguments: Sequence[str] = (),
    pins_path: Path | None = None,
    cwd: Path = ROOT,
) -> str:
    """Run QEMU until terminal evidence, exit, or the bounded timeout."""
    command = qemu_base_command(image=image, fail=fail, pins_path=pins_path)
    command.extend(additional_arguments)
    try:
        process = subprocess.Popen(
            command,
            cwd=cwd,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
    except OSError as error:
        fail(f"cannot run QEMU: {error}")

    timed_out = threading.Event()

    def stop_on_timeout() -> None:
        timed_out.set()
        if process.poll() is not None:
            return
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()

    watchdog = threading.Timer(timeout, stop_on_timeout)
    watchdog.start()
    lines: list[str] = []
    terminal_reached = False
    try:
        assert process.stdout is not None
        for line in process.stdout:
            lines.append(line.rstrip("\r\n"))
            if terminal_condition.search(line):
                terminal_reached = True
                break
    finally:
        watchdog.cancel()
        watchdog.join()
        if process.poll() is None:
            process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()

    transcript = "\n".join(lines)
    if timed_out.is_set():
        fail(f"QEMU timed out after {timeout}s before terminal condition")
    if not terminal_reached and process.returncode not in (0, None):
        fail(f"QEMU exited with status {process.returncode} before terminal condition")
    return transcript
