#!/usr/bin/env python3
"""Build and boot one external C component against the generated runtime ABI."""

from __future__ import annotations

import re
import shutil
import subprocess
import sys
import threading
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from closure_image import ClosureImageError, build as build_closure_image  # noqa: E402
from harness import load_qemu_profile, sha256_file  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
CLOSURE = "sel4-c-runtime"
IMAGE: Path | None = None
READY = "[c-runtime-probe] C component ready"
TERMINAL = "SLIME_GRAPH HEALTHY generation=42 required=1 live=0 completed=1 failed=0"
TIMEOUT = 240


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 C runtime check: {message}")




def build() -> None:
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


def boot(profile: dict[str, object]) -> str:
    qemu = shutil.which("qemu-system-aarch64")
    if qemu is None:
        fail("qemu-system-aarch64 is not on PATH")
    process = subprocess.Popen(
        [
            qemu,
            "-machine",
            str(profile["machine"]),
            "-cpu",
            str(profile["cpu"]),
            "-smp",
            str(profile["cpus"]),
            "-m",
            f"size={profile['memory_mib']}M",
            "-nographic",
            "-serial",
            "mon:stdio",
            "-kernel",
            str(IMAGE),
        ],
        cwd=ROOT,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    watchdog = threading.Timer(TIMEOUT, process.kill)
    watchdog.start()
    lines: list[str] = []
    terminal = re.compile(re.escape(TERMINAL) + r"|SLIME_ROOT FATAL|SLIME_GRAPH FAIL")
    try:
        assert process.stdout is not None
        for line in process.stdout:
            lines.append(line.rstrip("\n"))
            if terminal.search(line):
                break
    finally:
        timed_out = not watchdog.is_alive()
        watchdog.cancel()
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
    if timed_out:
        fail("QEMU timed out")
    return "\n".join(lines)


def main() -> None:
    build()
    transcript = boot(load_qemu_profile(fail))
    for marker in (READY, TERMINAL):
        if marker not in transcript:
            print(transcript)
            fail(f"missing marker {marker!r}")
    for marker in ("SLIME_ROOT FATAL", "SLIME_GRAPH FAIL"):
        if marker in transcript:
            print(transcript)
            fail(f"failure marker {marker!r}")
    print(
        "seL4 C runtime check: an external freestanding C ELF entered through the "
        "generated component ABI, wrote through the console endpoint, and exited cleanly"
    )


if __name__ == "__main__":
    main()
