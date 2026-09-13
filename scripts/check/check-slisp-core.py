#!/usr/bin/env python3
"""Exercise the bounded Slisp core on the host and as an external seL4 component."""

from __future__ import annotations

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
from harness import load_qemu_profile, sha256_file  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
CLOSURE = "sel4-slisp"
IMAGE: Path | None = None
SLISP_ROOT = ROOT / "components" / "slisp"
READY = "[slisp] repl done"
TERMINAL = "SLIME_GRAPH HEALTHY generation=43 required=1 live=0 completed=1 failed=0"
TIMEOUT = 240


def fail(message: str) -> NoReturn:
    raise SystemExit(f"Slisp core check: {message}")






def run_host_vectors(output: Path) -> None:
    compiler = shutil.which("cc")
    if compiler is None:
        fail("cc is not on PATH")
    result = subprocess.run(
        [
            compiler,
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic",
            str(SLISP_ROOT / "slisp.c"),
            str(SLISP_ROOT / "host_main.c"),
            "-o",
            str(output),
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        fail(f"host build failed: {result.stderr.strip()}")
    result = subprocess.run([str(output)], cwd=ROOT, capture_output=True, text=True, check=False)
    if result.returncode != 0 or result.stdout.strip() != (
        "Slisp core: 15 behavior vectors passed\n"
        "Slisp session: persistent define passed\n"
        "Slisp effects: spawn selection passed"
    ):
        fail(f"host behavior vectors failed: {result.stderr.strip() or result.stdout.strip()}")

def build() -> None:
    global IMAGE
    with tempfile.TemporaryDirectory(prefix="slisp-core-check-") as temporary:
        run_host_vectors(Path(temporary) / "slisp-host")
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
    markers = (
        "Slisp",
        "slisp> ",
        "=> 40",
        "=> 42",
        "! arity",
        READY,
        TERMINAL,
    )
    for marker in markers:
        if marker not in transcript:
            print(transcript)
            fail(f"missing marker {marker!r}")
    for marker in ("! input", "SLIME_ROOT FATAL", "SLIME_GRAPH FAIL"):
        if marker in transcript:
            print(transcript)
            fail(f"failure marker {marker!r}")
    print(
        "Slisp core check: host vectors and the freestanding seL4 REPL proved "
        "the bounded reader, lexical evaluator, persistent definitions, and refusals"
    )


if __name__ == "__main__":
    main()
