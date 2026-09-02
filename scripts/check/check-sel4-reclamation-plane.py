#!/usr/bin/env python3
"""B38 gate: exceed old task CSlot/untyped lifetime watermarks with bounded live use."""
from __future__ import annotations
import re
import shutil
import subprocess
import sys
import threading
import tomllib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from closure_image import ClosureImageError, build as build_closure_image  # noqa: E402

from component_paths import source_path  # noqa: E402
from harness import sha256_file  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
# CP15: the closure identity names the build's inputs and is re-resolved from
# repository state before the build, so stale input is refused rather than
# silently producing a different image. This checker exercises the forced
# construction-unwind arm carried by the reclamation-unwind root role.
CLOSURE = "sel4-reclamation-unwind"
IMAGE: Path | None = None
PINS = ROOT / "sel4" / "pins.toml"
INIT = source_path("init")
TIMEOUT = 180


def fail(message: str) -> None:
    raise SystemExit(f"seL4 reclamation plane check: {message}")


def build_image() -> None:
    global IMAGE
    try:
        built = build_closure_image(CLOSURE)
    except ClosureImageError as error:
        fail(str(error))
    IMAGE = built.image
    actual = sha256_file(IMAGE, fail)
    if actual != built.digest():
        fail(
            f"{IMAGE} SHA-256 is {actual}, but the build result records "
            f"{built.digest()}; the image changed after it was built"
        )

def boot(profile: dict[str, object]) -> str:
    qemu = shutil.which("qemu-system-aarch64")
    if qemu is None:
        fail("qemu-system-aarch64 is not on PATH")
    command = [qemu, "-machine", str(profile["machine"]), "-cpu", str(profile["cpu"]),
               "-smp", str(profile["cpus"]), "-m", f"size={profile['memory_mib']}M",
               "-nographic", "-serial", "mon:stdio", "-kernel", str(IMAGE)]
    process = subprocess.Popen(command, cwd=ROOT, stdin=subprocess.DEVNULL,
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                               text=True, bufsize=1)
    watchdog = threading.Timer(TIMEOUT, process.kill)
    watchdog.start()
    lines: list[str] = []
    terminal = re.compile(r"SLIME_ROOT allocator live_slots=|SLIME_ROOT FATAL|reclamation plane fail")
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
    source = INIT.read_text(encoding="utf-8")
    match = re.search(r"const RECLAMATION_LOOP_CHILDREN: u32 = (\d+);", source)
    if match is None or int(match.group(1)) <= 64:
        fail("lifetime loop does not exceed the old monotonic ceiling")
    build_image()
    pins = tomllib.loads(PINS.read_text(encoding="utf-8"))
    profile = pins.get("qemu_arm_virt")
    if not isinstance(profile, dict):
        fail("missing qemu profile")
    transcript = boot(profile)
    required = (
        r"\[init\] reclamation construction unwind returned",
        r"SLIME_GRAPH component exit task=\d+ status=0",
        r"\[init\] reclamation lifetime bound crossed",
        r"SLIME_GRAPH component fault task=\d+ kind=",
        r"\[init\] reclamation fault path reused",
        r"\[init\] reclamation plane complete",
        r"SLIME_ROOT allocator quiescent live_slots=(\d+) live_objects=(\d+) live_bytes=(\d+)",
        r"SLIME_ROOT allocator live_slots=(\d+) live_objects=(\d+) live_bytes=(\d+) slot_reuses=([1-9]\d*) arena_reuses=([1-9]\d*)",
    )
    cursor = 0
    for marker in required:
        match = re.search(marker, transcript[cursor:])
        if match is None:
            fail(f"missing ordered marker {marker!r}\n{transcript}")
        cursor += match.end()
    quiescent = re.search(
        r"SLIME_ROOT allocator quiescent live_slots=(\d+) live_objects=(\d+) live_bytes=(\d+)",
        transcript,
    )
    terminal = re.search(required[-1], transcript)
    if quiescent is None or terminal is None:
        fail("allocator quiescent or terminal accounting missing")
    if quiescent.groups() != terminal.groups()[:3]:
        fail(f"allocator live accounting drifted: quiescent={quiescent.groups()} terminal={terminal.groups()[:3]}")
    if re.search(r"SLIME_ROOT FATAL|reclamation plane fail|spawn unwound", transcript):
        fail("failure marker present")
    print("seL4 reclamation plane check: reusable task arenas and root CSlots observed")


if __name__ == "__main__":
    main()
