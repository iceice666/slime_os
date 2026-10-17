"""Shared image and QEMU process mechanisms for seL4 plane gates."""

from __future__ import annotations

import json
import shutil
import subprocess
import threading
import time
from collections.abc import Callable, Sequence
from pathlib import Path
from re import Pattern
from typing import BinaryIO, NoReturn

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
    input_trigger: Pattern[str] | None = None,
    input_text: str | None = None,
    input_character_delay: float = 0.001,
    input_steps: Sequence[tuple[Sequence[Pattern[str]], str]] = (),
    raw_transcript: Path | None = None,
) -> str:
    """Run QEMU until terminal evidence, exit, or the bounded timeout.

    Optional launch input is sent once after the declared readiness marker.
    `input_steps` instead sends each text only after every one of its patterns
    has matched a line printed since the previous step's text was sent, so a
    step never races the guest output it depends on. The harness does not log
    input; callers supplying secrets must use a guest input path that does not
    echo them to the serial output. Explicit `raw_transcript` capture exclusively
    creates a file before launch and retains the original consumed stdout/stderr
    bytes, including CRLF, through the terminal line (not a post-terminal drain).
    Captured runs decode UTF-8 with replacement for matching; uncaptured runs
    retain the existing text-mode behavior. Raw files also survive failed runs,
    and may contain guest-echoed input, so callers must opt in deliberately.
    """
    if (input_trigger is None) != (input_text is None):
        fail("QEMU launch input requires both a readiness trigger and input text")
    if input_trigger is not None and input_steps:
        fail("QEMU input is either one triggered launch input or ordered steps, not both")
    if any(not patterns for patterns, _text in input_steps):
        fail("every QEMU input step needs at least one readiness pattern")
    if input_character_delay < 0:
        fail("QEMU input pacing delay must be nonnegative")
    steps: list[tuple[Sequence[Pattern[str]], str]] = list(input_steps)
    if input_trigger is not None and input_text is not None:
        steps.append(((input_trigger,), input_text))
    command = qemu_base_command(image=image, fail=fail, pins_path=pins_path)
    command.extend(additional_arguments)
    raw_output: BinaryIO | None = None
    if raw_transcript is not None:
        try:
            raw_output = raw_transcript.open("xb")
        except OSError as error:
            fail(f"cannot exclusively create raw QEMU transcript {raw_transcript}: {error}")
    try:
        process = subprocess.Popen(
            command,
            cwd=cwd,
            stdin=subprocess.PIPE if steps else subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=raw_output is None,
            bufsize=1 if raw_output is None else -1,
        )
    except OSError as error:
        if raw_output is not None:
            raw_output.close()
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
    sent_steps = 0
    matched: set[int] = set()
    try:
        assert process.stdout is not None
        for output in process.stdout:
            if raw_output is not None:
                raw_output.write(output)
                raw_output.flush()
                line = output.decode("utf-8", errors="replace").replace("\r\n", "\n").replace("\r", "\n")
            else:
                line = output
            lines.append(line.rstrip("\r\n"))
            if sent_steps < len(steps):
                patterns, text = steps[sent_steps]
                matched.update(index for index, pattern in enumerate(patterns) if pattern.search(line))
                if len(matched) == len(patterns):
                    assert process.stdin is not None
                    try:
                        for character in text:
                            process.stdin.write(character.encode("utf-8") if raw_output is not None else character)
                            process.stdin.flush()
                            if input_character_delay:
                                time.sleep(input_character_delay)
                    except (BrokenPipeError, OSError):
                        fail("QEMU closed the launch-input stream")
                    sent_steps += 1
                    matched = set()
            if terminal_condition.search(line):
                terminal_reached = True
                break
    except OSError as error:
        if raw_output is None:
            raise
        fail(f"cannot collect QEMU serial output: {error}")
    finally:
        watchdog.cancel()
        watchdog.join()
        if process.poll() is None:
            process.terminate()
        try:
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        finally:
            if raw_output is not None:
                try:
                    raw_output.close()
                except OSError as error:
                    fail(f"cannot close raw QEMU transcript {raw_transcript}: {error}")

    transcript = "\n".join(lines)
    diagnostic_tail = "\n".join(lines[-80:])
    if timed_out.is_set():
        fail(f"QEMU timed out after {timeout}s before terminal condition\nLast serial output:\n{diagnostic_tail}")
    if not terminal_reached:
        fail(f"QEMU exited with status {process.returncode} before terminal condition\nLast serial output:\n{diagnostic_tail}")
    if sent_steps != len(steps):
        fail(f"QEMU reached terminal evidence without accepting launch input ({sent_steps} of {len(steps)} input steps sent)")
    return transcript
