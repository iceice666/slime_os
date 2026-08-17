#!/usr/bin/env python3

"""B22 gate: a graph outlives `MAX_CHANNELS` and still sends on every live one.

Boots `build/slime-sel4-crossing.elf` -- the image whose root task embeds the
channel-crossing generation,
`contracts/generation/v1/fixtures/sel4-crossing.zti` -- and asserts ordered
markers for backlog B22's exit condition: *a graph that mints more than
`MAX_CHANNELS` channels over its lifetime still sends and receives correctly on
every live channel.*

Before the fix, `channel::ChannelTable` never freed an entry: `push` derived its
key as `self.len`, `mark_dead` marked both queues of a dying task's channels
dead but released nothing, and `reassign` only rewrote the holder fields. So
`MAX_CHANNELS` (32, from `MAX_TASKS`) bounded the channels a boot could **ever**
mint rather than those live at once, and a long-running graph spent one
permanently per `endpoint_create` however short-lived the pair.

# What distinguishes this from B16's gate

B16's defect dropped a record *silently* and hung the parent, so converting the
failure into a reported one was part of its fix and its fault injection could
assert a new failure marker. B22's was already a bounded refusal --
`ChannelError::TableFull` becomes `IpcError::DestinationSlotsExhausted`, wire
`-5` -- so "the failure became reportable" proves nothing here. This gate can
only be satisfied by the graph *succeeding* past 32, which is why the loop's
completion marker is unreachable against the unfixed root.

The three properties a reclaim crossing could plausibly break, in transcript
order, are:

1. the loop crosses the historical live-object bound;
2. a retained endpoint capability still carries afterwards;
3. an endpoint exported before the crossing still imports with the same kind
   and rights afterwards.

The third property is the native bridge invariant. The export ticket owns the
reservation while no receiver CSpace contains the capability, and terminal
accounting proves no reservation or ticket leaks after import or cancellation.

A ninth image beside the eight before it, on the same rule: each gate boots the
artifact it asserts about, so none invalidates another's evidence by being built
last.
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
import threading
import tomllib
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from harness import profile_text, profile_integer, sha256_file  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
IMAGE = ROOT / "build" / "slime-sel4-crossing.elf"
MANIFEST = ROOT / "build" / "slime-sel4-crossing.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
FIXTURE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-crossing.zti"
IMAGE_VARIANT = "crossing"

BOOT_TIMEOUT_SECONDS = 180

REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        "the root admitted the crossing graph",
        r"SLIME_ROOT generation admitted number=1 executables=2 instances=2 grants=\d+ "
        r"health=2 bootstrap=1",
    ),
    (
        "the root launched both components from native ELFs and no legacy image",
        r"SLIME_ROOT graph admitted executables=2 instances=2 slimecm=0 elf=2 unrecognized=0",
    ),
    (
        # Root records the export before returning success to the component.
        "the root recorded an endpoint capability export",
        r"SLIME_GRAPH capability exported task=\d+ id=\d+ kind=endpoint "
        r"rights=0x[0-9a-f]+ retain=1",
    ),
    (
        # The receive installs the kernel capability before the receiver can use it.
        "the root recorded the matching endpoint import",
        r"SLIME_GRAPH capability imported task=\d+ id=\d+ kind=endpoint "
        r"rights=0x[0-9a-f]+ retain=1",
    ),
    (
        "an endpoint capability was exported before the crossing",
        r"\[init\] endpoint capability exported before crossing",
    ),
    (
        "the sender retained its narrowed endpoint authority across the crossing",
        r"\[init\] sender retained delegated authority",
    ),
    (
        "the exported endpoint still imported and carried after the crossing",
        r"\[init\] imported endpoint survived crossing",
    ),
    (
        "the graph sustained more exchanges than the retired channel lifetime bound",
        r"\[init\] channel lifetime bound crossed",
    ),
    (
        "the crossing plane ran to completion",
        r"\[init\] crossing plane complete",
    ),
    (
        # Native terminal accounting replaces queue/park/reply internals. Every
        # export ticket must have landed, been cancelled, or been finalized.
        "the root finalized every native capability export",
        r"SLIME_GRAPH capabilities exports=[1-9]\d* imports=[1-9]\d* "
        r"cancels=\d+ finalized=\d+ outstanding=0 tickets=0",
    ),
)

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL .*",
    r"SLIME_GRAPH FAIL .*",
    # The driver's own refusal. Against the unfixed root this is what appears
    # instead of the crossing marker: `loop pair mint` at the 33rd iteration.
    r"\[init\] crossing plane fail: .*",
    # The peer names its own cause before exiting, so a wrong-cause failure
    # cannot impersonate the transit-predicate one that init reports.
    r"\[crossing-peer\] fail: .*",
    # Native capability bridge failures must be explicit and terminal.
    r"SLIME_GRAPH capability (?:export|import|cancel) (?:failed|refused) .*",
    r"SLIME_GRAPH spawn unwound .*",
    r"SLIME_GRAPH spawn failed .*",
    r"SLIME_GRAPH spawn unwind incomplete .*",
    r"SLIME_GRAPH channel (?:recall|rollback) failed .*",
    r"\[slime-rt\] transfer window bind failed",
    r"SLIME_GRAPH window bind refused",
    r"SLIME_GRAPH park refused .*",
    r"SLIME_GRAPH channel unplaced .*",
    r"SLIME_GRAPH service budget exhausted",
    r"Attempted to invoke a read-only endpoint",
    r"seL4 called fail",
    r"Caught cap fault",
    r"Caught vm fault",
    r"Caught user exception",
    r"panicked at ",
    r"aborted at ",
    r"\(aborted\)",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 crossing plane check: {message}")


def load_pins() -> dict[str, object]:
    if not PINS_PATH.is_file():
        fail(f"missing pin manifest: {PINS_PATH.relative_to(ROOT)}")
    try:
        pins = tomllib.loads(PINS_PATH.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        fail(f"cannot parse {PINS_PATH.relative_to(ROOT)}: {error}")
    if pins.get("schema") != 1:
        fail("unsupported sel4/pins.toml schema (expected 1)")
    if not isinstance(pins.get("qemu_arm_virt"), dict):
        fail("sel4/pins.toml is missing [qemu_arm_virt]")
    return pins


def build_image() -> None:
    command = [sys.executable, str(BUILD_SCRIPT), "--crossing-plane"]
    print(f"[build] {' '.join(command)}", flush=True)
    try:
        process = subprocess.run(command, cwd=ROOT, check=False)
    except OSError as error:
        fail(f"cannot run the seL4 image build: {error}")
    if process.returncode != 0:
        fail(f"seL4 image build failed with exit status {process.returncode}")


def check_manifest() -> None:
    if not MANIFEST.is_file():
        fail(
            f"missing identity manifest {MANIFEST.relative_to(ROOT)}; "
            "run `just sel4_crossing_check`"
        )
    try:
        manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot parse {MANIFEST.relative_to(ROOT)}: {error}")
    if not isinstance(manifest, dict) or manifest.get("kind") != "slime-sel4-image-identity":
        fail(f"{MANIFEST.relative_to(ROOT)} is not a Slime seL4 identity manifest")
    # Every seL4 image is built from the same sources and differs only in which
    # generation the root task embeds, so booting the wrong one would fail on
    # markers rather than on identity. Checking the variant reports the actual
    # cause instead.
    if manifest.get("variant") != IMAGE_VARIANT:
        fail(
            f"{MANIFEST.relative_to(ROOT)} records variant "
            f"{manifest.get('variant')!r}, not {IMAGE_VARIANT!r}; "
            "rebuild with `--crossing-plane`"
        )
    image = manifest.get("image")
    if not isinstance(image, dict) or not isinstance(image.get("sha256"), str):
        fail("identity manifest does not record the packaged image digest")
    if not IMAGE.is_file():
        fail(f"missing packaged image {IMAGE.relative_to(ROOT)}")
    actual = sha256_file(IMAGE, fail)
    if actual != image["sha256"]:
        fail(
            f"{IMAGE.relative_to(ROOT)} SHA-256 is {actual}, but the identity manifest "
            f"records {image['sha256']}; rebuild before booting"
        )


def boot(profile: dict[str, object]) -> str:
    """Boot the image and return the serial transcript.

    The root task suspends itself once the graph has drained, so QEMU stays
    alive afterwards and waiting for an exit would always time out. Serial
    output is read line by line and the guest is killed as soon as the terminal
    or any failure marker appears.
    """
    qemu = shutil.which("qemu-system-aarch64")
    if qemu is None:
        fail("qemu-system-aarch64 is not on PATH")
    command = [
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
        str(IMAGE),
    ]
    print(f"[boot] {' '.join(command)}", flush=True)
    terminal = re.compile(REQUIRED_MARKERS[-1][1])
    failures = re.compile("|".join(FAILURE_MARKERS))
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
    # A wedged guest emits nothing, so the deadline cannot live in the read
    # loop; a watchdog kills QEMU, which closes the pipe and ends the loop.
    watchdog = threading.Timer(BOOT_TIMEOUT_SECONDS, process.kill)
    watchdog.start()
    try:
        assert process.stdout is not None
        for line in process.stdout:
            lines.append(line.rstrip("\n"))
            if terminal.search(line) or failures.search(line):
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
    transcript = "\n".join(lines)
    if timed_out and terminal.search(transcript) is None:
        report_transcript(transcript)
        fail(f"boot exceeded {BOOT_TIMEOUT_SECONDS}s without reaching the final marker")
    return transcript


def report_transcript(transcript: str) -> None:
    tail = transcript.splitlines()[-40:]
    if tail:
        sys.stdout.write("--- serial transcript (tail) ---\n")
        sys.stdout.write("\n".join(tail) + "\n")
        sys.stdout.write("--- end transcript ---\n")
        sys.stdout.flush()


def check_transcript(transcript: str) -> None:
    for pattern in FAILURE_MARKERS:
        match = re.search(pattern, transcript)
        if match is not None:
            report_transcript(transcript)
            fail(f"failure marker in serial transcript: {match.group(0)!r}")
    position = 0
    for description, pattern in REQUIRED_MARKERS:
        match = re.compile(pattern).search(transcript, position)
        if match is None:
            report_transcript(transcript)
            if re.search(pattern, transcript) is not None:
                fail(f"marker out of order: {description} ({pattern})")
            fail(f"missing marker: {description} ({pattern})")
        position = match.end()
    exports = re.findall(
        r"SLIME_GRAPH capability exported task=\d+ id=(\d+) kind=endpoint "
        r"rights=(0x[0-9a-f]+) retain=1",
        transcript,
    )
    imports = re.findall(
        r"SLIME_GRAPH capability imported task=\d+ id=(\d+) kind=endpoint "
        r"rights=(0x[0-9a-f]+) retain=1",
        transcript,
    )
    if len(exports) != 1 or exports != imports:
        fail(f"endpoint export/import evidence was {exports!r}/{imports!r}, expected one exact pair")




def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 channel-crossing image and assert ordered markers"
    )
    parser.add_argument(
        "--no-build",
        action="store_true",
        help="boot the already-built image instead of rebuilding it first",
    )
    arguments = parser.parse_args()

    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    if not FIXTURE.is_file():
        fail(f"missing generation fixture: {FIXTURE.relative_to(ROOT)}")
    pins = load_pins()
    if not arguments.no_build:
        build_image()
    check_manifest()
    profile = pins["qemu_arm_virt"]
    assert isinstance(profile, dict)
    check_transcript(boot(profile))
    print(
        "seL4 crossing plane check: native endpoint authority survived an "
        "allocation crossing both while retained and while held by an export ticket"
    )


if __name__ == "__main__":
    main()
