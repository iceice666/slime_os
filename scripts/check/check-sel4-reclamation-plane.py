#!/usr/bin/env python3
"""B38 gate: exceed old task CSlot/untyped lifetime watermarks with bounded live use."""
from __future__ import annotations
import json
import re
import shutil
import subprocess
import sys
import threading
import tomllib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from harness import sha256_file  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
IMAGE = ROOT / "build" / "slime-sel4-reclamation.elf"
MANIFEST = ROOT / "build" / "slime-sel4-reclamation.identity.json"
BUILD = ROOT / "scripts" / "build" / "build-sel4.py"
PINS = ROOT / "sel4" / "pins.toml"
INIT = ROOT / "components" / "bins" / "src" / "bin" / "init.rs"
TIMEOUT = 180


def fail(message: str) -> None:
    raise SystemExit(f"seL4 reclamation plane check: {message}")


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
    build = subprocess.run([sys.executable, str(BUILD), "--reclamation-plane"], cwd=ROOT)
    if build.returncode != 0 or not IMAGE.is_file():
        fail("image build failed")
    if not MANIFEST.is_file():
        fail("identity manifest missing")
    identity = json.loads(MANIFEST.read_text(encoding="utf-8"))
    if identity.get("variant") != "reclamation":
        fail(f"wrong image variant {identity.get('variant')!r}")
    image = identity.get("image")
    if not isinstance(image, dict) or image.get("sha256") != sha256_file(IMAGE, fail):
        fail("packaged image digest does not match identity manifest")
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
