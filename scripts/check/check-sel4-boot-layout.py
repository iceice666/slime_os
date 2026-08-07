#!/usr/bin/env python3

"""B10 on seL4: init's resolved capability layout, per plane, against fixtures.

The boot layout is a contract between three readers -- the root task that fills
init's capability table, the component images that address slots by number, and
the gates that assert on what those components do. Nothing else fails when the
three disagree: a component reads whatever landed at the number it compiled
against, and the symptom surfaces as unrelated behaviour somewhere downstream.

`just boot_layout_check` freezes that layout for nineteen x86 profiles by
booting the retired kernel and diffing its `[layout]` block against a recorded
fixture. No seL4 equivalent existed. P5.4.1's inventory recorded B10 as covered
only *obliquely* on this side -- three gates assert specific slot numbers in
passing (`slot=0 kind=endpoint-factory`, `slot=4 component=sysinfo`) -- which
catches a layout that moved *those* slots and nothing else.

This gate closes that. `slime-root` now emits the same `[layout]` block the
oracle does, and every seL4 plane's is frozen here, so a layout change is a
reviewable diff rather than a silent renumbering.

# Why every plane rather than one

Each plane boots a different generation, and `layout_for` prunes the base table
by which components that generation declares. The pruning is the interesting
part -- it renumbers -- so freezing one plane would leave the other seven
unguarded against exactly the change most likely to break them.

# Relationship to the boot itself

This is a *layout* gate, not a behaviour one: it boots each image and reads one
block. The planes' own gates assert what their components do. Kept separate for
the reason `boot_layout_check` is separate on x86 -- a layout diff should be
readable as a layout diff, not inferred from a component failing.

Regenerate with `--bless`; the resulting diff is the evidence that a layout
change was intended.
"""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
import threading
import tomllib
from pathlib import Path
from typing import NoReturn

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
FIXTURES = ROOT / "contracts" / "boot-layout" / "v1" / "fixtures"

BOOT_TIMEOUT_SECONDS = 180

# Every seL4 plane, by the build flag that selects its generation and the image
# it produces. The fixture stem is prefixed `sel4-` so these sort beside the x86
# ones without colliding: `default.layout` is the oracle's, `sel4.layout` this
# root's for the component-graph generation.
#
# `fixture` (P5.1) is absent deliberately: it embeds the retained x86 generation
# and launches no component graph, so it has no init and emits no block.
PLANES: tuple[tuple[str, str, str], ...] = (
    ("sel4", "--component-graph", "slime-sel4-graph.elf"),
    ("sel4-channel", "--channel-plane", "slime-sel4-channel.elf"),
    ("sel4-loan", "--loan-plane", "slime-sel4-loan.elf"),
    ("sel4-spawn", "--spawn-plane", "slime-sel4-spawn.elf"),
    ("sel4-sample", "--sample-plane", "slime-sel4-sample.elf"),
    ("sel4-stream", "--stream-plane", "slime-sel4-stream.elf"),
    ("sel4-supervision", "--supervision-plane", "slime-sel4-supervision.elf"),
    ("sel4-crossing", "--crossing-plane", "slime-sel4-crossing.elf"),
    # P5.4.6. Frozen even though the plane does not pass its own scenario: the
    # layout is emitted between channel materialization and activation, so it
    # is complete and observable long before the deadlock B25 records. What
    # this guards is exactly what B10 exists for — that the table
    # `SEL4_CALL_LAYOUT` declares is the table the root fills — and that claim
    # is independent of whether the broker later completes provisioning.
    ("sel4-call", "--call-plane", "slime-sel4-call.elf"),
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 boot layout check: {message}")


def load_pins() -> dict[str, object]:
    if not PINS_PATH.is_file():
        fail(f"missing pin manifest: {PINS_PATH.relative_to(ROOT)}")
    try:
        pins = tomllib.loads(PINS_PATH.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        fail(f"cannot parse {PINS_PATH.relative_to(ROOT)}: {error}")
    profile = pins.get("qemu_arm_virt")
    if not isinstance(profile, dict):
        fail("sel4/pins.toml is missing [qemu_arm_virt]")
    return profile


def profile_text(profile: dict[str, object], key: str) -> str:
    value = profile.get(key)
    if not isinstance(value, str) or not value:
        fail(f"sel4/pins.toml [qemu_arm_virt].{key} must be non-empty text")
    return value


def profile_integer(profile: dict[str, object], key: str) -> int:
    value = profile.get(key)
    if not isinstance(value, int) or isinstance(value, bool):
        fail(f"sel4/pins.toml [qemu_arm_virt].{key} must be an integer")
    return value


def build(flag: str) -> None:
    command = [sys.executable, str(BUILD_SCRIPT), flag, "--skip-pin-check"]
    print(f"[build] {' '.join(command)}", flush=True)
    process = subprocess.run(command, cwd=ROOT, check=False, capture_output=True, text=True)
    if process.returncode != 0:
        sys.stdout.write(process.stdout[-4000:])
        sys.stderr.write(process.stderr[-4000:])
        fail(f"image build failed for {flag}")


def capture(name: str, image: Path, profile: dict[str, object]) -> str:
    """Boot one image and return its `[layout]` block."""
    qemu = shutil.which("qemu-system-aarch64")
    if qemu is None:
        fail("qemu-system-aarch64 is not on PATH")
    command = [
        qemu,
        "-machine",
        profile_text(profile, "machine"),
        "-cpu",
        profile_text(profile, "cpu"),
        "-smp",
        str(profile_integer(profile, "cpus")),
        "-m",
        f"size={profile_integer(profile, 'memory_mib')}M",
        "-nographic",
        "-serial",
        "mon:stdio",
        "-kernel",
        str(image),
    ]
    lines: list[str] = []
    try:
        process = subprocess.Popen(
            command,
            cwd=ROOT,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
    except OSError as error:
        fail(f"cannot run QEMU: {error}")
    # The block is emitted before activation, so the boot is stopped as soon as
    # it closes rather than run to completion: this gate reads a layout, and the
    # planes' own gates own their behaviour.
    watchdog = threading.Timer(BOOT_TIMEOUT_SECONDS, process.kill)
    watchdog.start()
    try:
        assert process.stdout is not None
        for line in process.stdout:
            stripped = line.strip()
            lines.append(stripped)
            if stripped == "[layout] end":
                break
    finally:
        watchdog.cancel()
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
    try:
        start = next(i for i, line in enumerate(lines) if line.startswith("[layout] path="))
        end = next(i for i, line in enumerate(lines[start:], start) if line == "[layout] end")
    except StopIteration:
        sys.stdout.write("\n".join(lines[-40:]) + "\n")
        fail(f"{name}: boot emitted no complete layout block")
    return "\n".join(lines[start : end + 1]) + "\n"


# Every line a layout block may carry. Anchored so a malformed line is a
# failure rather than something the diff silently accepts.
#
# The optional `declared=` tail is B26's: a bootstrap row whose *layout* rights
# differ from the *grant* rights installed at that slot reports both, because
# the two are related by containment rather than equality and a row carrying
# only the installed value cannot show a layout that declares more authority
# than anything uses. Rows where they agree — every row in every fixture today
# — keep the retired kernel's exact four fields, so the two dumps stay
# comparable slot for slot.
HEADER = re.compile(r"^\[layout\] path=\S+ slots=\d+ max=\d+$")
ENTRY = re.compile(
    r"^\[layout\] \d+ [a-z-]+ \S+ 0x[0-9a-f]+( declared=0x[0-9a-f]+)?$"
)


def check_shape(name: str, block: str) -> None:
    """The block is well formed and its declared count matches its rows."""
    lines = block.splitlines()
    # Independently total rather than depending on `capture`'s StopIteration
    # path thirty lines away: a block too short to index is a malformed block.
    if len(lines) < 2:
        fail(f"{name}: layout block has no header and terminator")
    if not HEADER.match(lines[0]):
        fail(f"{name}: malformed header {lines[0]!r}")
    if lines[-1] != "[layout] end":
        fail(f"{name}: block does not end with the terminator")
    rows = lines[1:-1]
    for row in rows:
        if not ENTRY.match(row):
            fail(f"{name}: malformed entry {row!r}")
    declared = int(lines[0].rsplit("slots=", 1)[1].split()[0])
    if declared != len(rows):
        fail(f"{name}: header declares {declared} slots, block carries {len(rows)}")
    # Slot numbers strictly ascending: the layout is an ordered table, and a
    # repeated or out-of-order number would mean two grants resolved to one
    # slot -- exactly the collision B10 exists to make visible.
    numbers = [int(row.split()[1]) for row in rows]
    if numbers != sorted(set(numbers)):
        fail(f"{name}: slot numbers are not strictly ascending: {numbers}")


def main() -> None:
    parser = argparse.ArgumentParser(description="Freeze each seL4 plane's boot layout")
    parser.add_argument(
        "--bless",
        action="store_true",
        help="rewrite the fixtures from the observed layouts",
    )
    parser.add_argument(
        "--no-build",
        action="store_true",
        help="boot the already-built images instead of rebuilding each first",
    )
    arguments = parser.parse_args()

    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    profile = load_pins()

    failures: list[str] = []
    for name, flag, image_name in PLANES:
        if not arguments.no_build:
            build(flag)
        image = ROOT / "build" / image_name
        if not image.is_file():
            fail(f"{name}: missing image {image.relative_to(ROOT)}")
        observed = capture(name, image, profile)
        check_shape(name, observed)
        fixture = FIXTURES / f"{name}.layout"
        if arguments.bless:
            fixture.write_text(observed)
            print(f"blessed {name}: {len(observed.splitlines()) - 2} slots")
            continue
        if not fixture.exists():
            failures.append(f"{name}: no fixture; run with --bless to record it")
            continue
        expected = fixture.read_text()
        if observed == expected:
            print(f"{name}: {len(observed.splitlines()) - 2} slots match")
            continue
        failures.append(f"{name}: layout differs from {fixture.relative_to(ROOT)}")
        expected_lines = expected.splitlines()
        observed_lines = observed.splitlines()
        for index in range(max(len(expected_lines), len(observed_lines))):
            was = expected_lines[index] if index < len(expected_lines) else "<absent>"
            now = observed_lines[index] if index < len(observed_lines) else "<absent>"
            if was != now:
                failures.append(f"    was: {was}")
                failures.append(f"    now: {now}")

    if failures:
        for line in failures:
            print(line)
        raise SystemExit("seL4 boot layout check: layouts moved")
    if arguments.bless:
        print("seL4 boot layout check: blessed")
        return
    print(f"seL4 boot layout check: {len(PLANES)} plane layouts match their fixtures")


if __name__ == "__main__":
    main()
