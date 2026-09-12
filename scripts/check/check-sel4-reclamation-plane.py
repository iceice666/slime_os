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
    command = [
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
    ]
    process = subprocess.Popen(
        command,
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
    terminal = re.compile(
        r"SLIME_ROOT allocator live_slots=|SLIME_ROOT FATAL|reclamation plane fail"
    )
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
        r"SLIME_ROOT allocator live_slots=(\d+) free_slots=(\d+) live_objects=(\d+) live_bytes=(\d+) "
        r"mapped_ram=(\d+) reusable_ram=([1-9]\d*) reusable_private_ram=([1-9]\d*) "
        r"allocation_descriptors_free=(\d+) extent_descriptors_free=(\d+) "
        r"slot_reuses=([1-9]\d*) extent_reuses=([1-9]\d*)",
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
    if quiescent.groups() != (terminal.group(1), terminal.group(3), terminal.group(4)):
        fail(
            f"allocator live accounting drifted: quiescent={quiescent.groups()} "
            f"terminal={(terminal.group(1), terminal.group(3), terminal.group(4))}"
        )
    if terminal.group(5) != "0":
        fail(f"private mapped RAM survived reclamation: {terminal.group(5)}")
    if int(terminal.group(6)) == 0:
        fail("no reclaimable backing extent was retained for reuse")
    # Unrestricted `reusable_ram` is satisfied by any inactive extent,
    # including the one every task carries for its static construction --
    # `reclamation-fault`'s own declared quota is what makes this composition
    # exercise a private extent at all, and this is the kind-scoped assertion
    # that it specifically was reclaimed rather than leaked.
    if int(terminal.group(7)) == 0:
        fail("no private-backing extent was retained for reuse")
    # An arena release that returned slots, extents, and the page charge but
    # stranded an `AllocationRecord` leaks descriptor capacity silently: this
    # plane's 80 iterations stay far below the pool, so every liveness and
    # reuse assertion above still passes while a longer-running system
    # eventually fails construction with `ArenaSlotTableFull`. The plane ends
    # with no live task, so the exact law holds: every allocation descriptor
    # is back.
    capacity = re.search(
        r"SLIME_ROOT allocator baseline live_slots=\d+ live_objects=\d+ live_bytes=\d+ "
        r"allocation_descriptor_capacity=(\d+) extent_descriptor_capacity=(\d+)",
        transcript,
    )
    if capacity is None:
        fail("the root reported no allocator descriptor capacity")
    if terminal.group(8) != capacity.group(1):
        fail(
            f"{int(capacity.group(1)) - int(terminal.group(8))} allocation "
            "descriptor(s) survived reclamation of every task: "
            f"{terminal.group(8)} free of {capacity.group(1)}"
        )
    # Extent descriptors are deliberately *not* returned: `release_task_arena`
    # deactivates a record rather than clearing it, so `provision_extent` can
    # re-retype into it. Retention is therefore proved by reuse dominating new
    # records, not by the free count returning: were retention to break, every
    # provision would take a fresh record and `extent_reuses` would collapse.
    extent_growth = int(capacity.group(2)) - int(terminal.group(9))
    if extent_growth >= int(terminal.group(11)):
        fail(
            f"extent descriptors grew by {extent_growth} against "
            f"{terminal.group(11)} reuse(s): released records are not being "
            "retained for reuse"
        )
    if re.search(r"SLIME_ROOT FATAL|reclamation plane fail|spawn unwound", transcript):
        fail("failure marker present")
    print(
        "seL4 reclamation plane check: segmented task extents and root CSlots were reclaimed and reused"
    )


if __name__ == "__main__":
    main()
