#!/usr/bin/env python3

"""P5.3.3 gate: a component constructs children on seL4 and supervises them.

Boots `build/slime-sel4-spawn.elf` -- the image whose root task embeds the
spawn-plane generation, `contracts/generation/v1/fixtures/sel4-spawn.zti` --
and asserts ordered markers for P5.3.3's exit condition:

1. a component spawns a child from a grant-resolved executable;
2. the child receives declared capabilities at the slots its layout names;
3. the parent observes the child's termination through a supervision handle
   rather than an ambient task id.

Both children are unmodified: `console` and `sysinfo` are the same binaries the
x86 oracle runs, with no seL4 branch in either. That is the load-bearing claim
of the slice -- a component written against the retired kernel's spawn ABI is
constructed by `slime-root` because the ABI is the same one, not because the
scenario was rewritten to suit whatever the root happened to implement.

Modelled on `check-sel4-loan-plane.py`, which guards P5.3.2 against a different
image. The five seL4 images are separate artifacts on purpose: each gate boots
the one it asserts about, so none invalidates another's evidence by being built
last.

# What the root still launches, and why the transcript shows it

The root launches every component the generation declares (P5.2), so this boot
also starts one unconfigured `console` and one unconfigured `sysinfo` that no
one handed a channel to. Those exit non-zero and are *expected*: this fixture's
subject is the instances `init` spawns, which are separate tasks with separate
ids. Asserting on task id alone would confuse the two, so every marker below
names the spawn that produced the task it is about.
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
IMAGE = ROOT / "build" / "slime-sel4-spawn.elf"
MANIFEST = ROOT / "build" / "slime-sel4-spawn.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
FIXTURE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-spawn.zti"
IMAGE_VARIANT = "spawn"

BOOT_TIMEOUT_SECONDS = 120

REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        "the spawn generation was admitted",
        r"SLIME_ROOT generation admitted number=\d+ executables=3 instances=3 grants=11 ",
    ),
    (
        "every payload is a native ELF image",
        r"SLIME_ROOT graph admitted executables=3 instances=3 slimecm=0 elf=3 unrecognized=0",
    ),
    (
        # Init's executable authorities come from the generation-derived
        # layout rather than a cursor. The exact declared slot numbers are
        # private to that generated profile.
        "init was staged holding both declared executables",
        r"SLIME_GRAPH staged task=\d+ instance=init executable=init grants=11 bindings=11 ",
    ),
    (
        # The endpoints both children answer on are generation-declared seL4
        # Endpoints the root materializes and installs into each end itself.
        # A parent holds no endpoint half to hand over, so what crosses at
        # spawn is transferable directory authority instead.
        "the declared control endpoints were materialized by the root",
        r"SLIME_GRAPH peer endpoints created=2 grants=2 installed=\d+",
    ),
    # -- required check: an ungranted or over-wide spawn is refused --
    (
        "an empty slot cannot name an executable",
        r"SLIME_GRAPH spawn refused task=\d+ slot=63 ungranted",
    ),
    (
        # A live non-executable capability slot must be refused identically to
        # an empty one; the component cannot use spawn to probe capability
        # kinds.
        "a non-executable capability slot cannot name an executable",
        r"SLIME_GRAPH spawn refused task=\d+ slot=3 ungranted",
    ),
    ("both refusals reached the component", r"\[init\] ungranted executable refused"),
    (
        # The narrowing rule: a grant's rights must be a subset of what the
        # parent holds. Asking to add buffer creation authority is asking the
        # root to manufacture authority no generation declared.
        "a spawn cannot widen its own grant",
        r"\[init\] widened grant refused",
    ),
    (
        # The executable slot is authority to create this child; passing it on
        # would let the child re-spawn its own image outside its parent's
        # budget.
        "a child cannot be granted its own executable",
        r"\[init\] self-executable grant refused",
    ),
    # -- required check: a child is constructed from a grant-resolved executable --
    (
        "console was authorized from its declared executable grant",
        r"SLIME_GRAPH spawn authorized task=\d+ slot=1 component=console grants=1",
    ),
    (
        "console was constructed with its declared capabilities",
        r"SLIME_GRAPH spawned task=\d+ child=\d+ component=console grants=1 "
        r"endpoints=1 notifications=0 handle=\d+ supervision_grants=0 buffer_factory_grants=0",
    ),
    ("the spawn reached the component", r"\[init\] console spawned"),
    (
        # A live child has no outcome, and the query says so rather than
        # blocking or inventing one.
        "a live child reports no outcome",
        r"\[init\] live child reports no outcome",
    ),
    (
        # A spawn grant is a copy, matching the x86 oracle: the child receives
        # a narrowed view and the parent can still resolve the slot it granted
        # from.
        "the granted view remained usable beside the child",
        r"\[init\] granted view retained",
    ),
    # -- required check: termination observed through a supervision handle --
    (
        # B15: six grants, where this root admitted four before P5.5.1. The
        # grant array crosses the transfer window as a staged payload, and it
        # used to be read with the *message* bound -- 64 bytes, four records --
        # against the retired kernel's sixty-four. Six records are 96 bytes, so
        # this line is only reachable through the wide reader
        # (`transfer_window::read_staged_array`); a root that lost it refuses
        # the spawn outright and `init` never reaches this marker at all.
        #
        # Six is B15's own exit-condition number, so the arm is the oracle's
        # shape rather than a synthetic width. The six capabilities are narrowed
        # directory views, because an endpoint is a declared object the root
        # installs itself and a parent has none to pass on.
        "sysinfo was authorized from its layout-named executable slot with six grants",
        r"SLIME_GRAPH spawn authorized task=\d+ slot=2 component=sysinfo grants=6",
    ),
    (
        # The other half of B15's exit condition: the child holds all six at
        # the slots its numbering fixes. A root that admitted the wide array
        # but installed only the first four reaches the authorization marker
        # and fails in the component.
        "sysinfo received its six declared capabilities",
        r"SLIME_GRAPH spawned task=\d+ child=\d+ component=sysinfo grants=6 "
        r"endpoints=1 notifications=0 handle=\d+ supervision_grants=0 buffer_factory_grants=0",
    ),
    ("sysinfo was constructed", r"\[init\] sysinfo spawned"),
    (
        # The parent re-resolves every source slot after spawn: a grant is a
        # copy, so the view it granted from must still answer.
        "the parent reused all six copied views",
        r"\[init\] six grants copied",
    ),
    (
        # The unmodified binary ran and produced its own output, which is what
        # makes "constructed from a grant-resolved executable" a fact about a
        # real component rather than about an empty task. It can only run at all
        # because the launch context reached it; that init sent one is asserted
        # by presence below, since a `debug_write` is a root round trip and so
        # orders against the child it just unblocked only by scheduling accident.
        "the unmodified child ran",
        r"\[sysinfo\] spawned through profile",
    ),
    (
        # The child ended of its own accord. The cutover deleted `WaitSet` and
        # its `parked … reason=wait` / `supervision woken` pair: a supervision
        # handle is polled, so what remains observable is the exit the root
        # recorded and the outcome the parent then collected through the handle.
        "the child exited of its own accord",
        r"SLIME_GRAPH component exit task=\d+ status=0",
    ),
    (
        # `kind=0` is a clean exit. The record outlives the task itself --
        # `reclaim_dead_task` has already erased everything else about it --
        # which is the whole reason `supervision.rs` exists.
        "the outcome was collected through the handle",
        r"SLIME_GRAPH supervision collected task=\d+ child=\d+ kind=0",
    ),
    ("the parent read its child's clean exit", r"\[init\] sysinfo outcome collected"),
    (
        # Collecting consumes the handle, so the outcome is single-use rather
        # than a fact the parent can re-read forever.
        "a collected handle answers only once",
        r"\[init\] collected handle consumed",
    ),
    (
        # `cap_drop` on a live child's handle. `spawn_or_fail` does exactly
        # this on every x86 boot, so an unimplemented drop would abort the
        # product graph.
        "a live child's handle can be dropped",
        r"\[init\] dropped handle released",
    ),
    ("the scenario completed", r"\[init\] spawn plane complete"),
    (
        # The spawning component exits 0. Every arm above passed, since each
        # failure path exits 1 with a `spawn plane fail` line the failure
        # markers below catch.
        "the parent exited cleanly",
        r"SLIME_GRAPH component exit task=\d+ status=0",
    ),
    (
        # `terminated` is deliberately non-zero -- one record per child that
        # ended, kept past reclamation by design -- so a zero there would mean
        # the supervision path recorded nothing at all. Two spawns and one drop
        # is the scenario's exact shape: `console` and `sysinfo`, with the live
        # handle dropped and the collected one consumed.
        "every spawn and drop was accounted for",
        r"SLIME_GRAPH spawns served=2 drops=1 terminated=[1-9]\d*",
    ),
)

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL .*",
    r"SLIME_GRAPH FAIL .*",
    # Every arm of the scenario reports its own failure this way, so a denial
    # that stopped denying fails the gate here rather than by a missing marker.
    r"\[init\] spawn plane fail: .*",
    r"\[init\] channel plane fail: .*",
    r"\[init\] loan plane fail: .*",
    # A child constructed and then torn back down. Every unwind path in
    # `construct_child` is an exhaustion case this graph is far from reaching,
    # so one occurring means an allocation bound was hit unexpectedly.
    r"SLIME_GRAPH spawn unwound .*",
    r"SLIME_GRAPH spawn failed .*",
    # The unwind itself failing is strictly worse than the unwind happening: it
    # means a leaked VSpace, CNode, and TCB rather than a refused spawn.
    r"SLIME_GRAPH spawn unwind incomplete .*",
    # A channel left naming a task that was destroyed before it ran. Reachable
    # by nobody and reclaimed by nothing, since `reclaim_dead_task` only runs
    # for a task that actually ran.
    r"SLIME_GRAPH channel (?:recall|rollback) failed .*",
    r"\[slime-rt\] transfer window bind failed",
    r"SLIME_GRAPH window bind refused",
    r"SLIME_GRAPH park refused .*",
    # This graph declares no channel edge -- init mints both of its channels at
    # runtime -- so any unplaced channel means the fixture and the boot layout
    # have drifted apart.
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
    raise SystemExit(f"seL4 spawn plane check: {message}")


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
    command = [sys.executable, str(BUILD_SCRIPT), "--spawn-plane"]
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
            "run `just sel4_spawn_check`"
        )
    try:
        manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot parse {MANIFEST.relative_to(ROOT)}: {error}")
    if not isinstance(manifest, dict) or manifest.get("kind") != "slime-sel4-image-identity":
        fail(f"{MANIFEST.relative_to(ROOT)} is not a Slime seL4 identity manifest")
    # The five images are built from the same sources and differ only in which
    # generation the root task embeds, so booting the wrong one would fail on
    # markers rather than on identity. Checking the variant reports the actual
    # cause instead.
    if manifest.get("variant") != IMAGE_VARIANT:
        fail(
            f"{MANIFEST.relative_to(ROOT)} records variant "
            f"{manifest.get('variant')!r}, not {IMAGE_VARIANT!r}; "
            "rebuild with `--spawn-plane`"
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
    # Asserted by presence, not by position. Each of these is a `debug_write`,
    # which is a root round trip: init emits it after unblocking a child that
    # then runs concurrently, so ordering it against that child's own output
    # would assert a scheduling accident rather than the handoff.
    for description, marker in (
        ("init sent the launch context down its declared endpoint", "[init] launch context sent"),
        ("the spawned console read the endpoint it was given", "[console] spawned child reached"),
    ):
        if marker not in transcript:
            report_transcript(transcript)
            fail(f"missing marker: {description} ({marker})")
    check_spawned_children_are_unmodified()


# Product graph selection is authenticated generation data. A child rewritten
# around a private compile-time scenario selector would violate this gate even
# if it reproduced the same transcript.
FORBIDDEN_COMPONENT_SELECTORS = ("option_env!(", "cfg!(slime_")
UNMODIFIED_CHILDREN = ("console", "sysinfo")


def check_spawned_children_are_unmodified() -> None:
    """Neither spawned child knows it is running on seL4.

    P5.3.3 claims that a component written against the retired kernel's spawn
    ABI is constructed by `slime-root` unchanged. The transcript shows both
    children running, but it cannot show that they are the *same* binaries the
    x86 oracle runs -- a child rewritten to suit this root would look identical
    on serial. So the sources are read directly.

    Both sources must remain free of compile-time product selectors. Runtime
    behavior may still inspect capabilities and generation-derived profile
    data, which are the contracts this gate actually boots.
    """
    for name in UNMODIFIED_CHILDREN:
        source = ROOT / "components" / "bins" / name / "src" / "main.rs"
        try:
            text = source.read_text(encoding="utf-8")
        except OSError as error:
            fail(f"cannot read {source.relative_to(ROOT)}: {error}")
        for selector in FORBIDDEN_COMPONENT_SELECTORS:
            if selector in text:
                fail(
                    f"{source.relative_to(ROOT)} branches on compile-time selector "
                    f"{selector!r}; the milestone requires this component to run "
                    "from generation data"
                )
    print(
        "components: "
        + ", ".join(UNMODIFIED_CHILDREN)
        + " carry no compile-time product branch and run as the x86 oracle builds them",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 spawn-plane image and assert ordered markers"
    )
    parser.add_argument(
        "--no-build",
        action="store_true",
        help="boot the already-built image instead of rebuilding it first",
    )
    arguments = parser.parse_args()

    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    pins = load_pins()
    if not arguments.no_build:
        build_image()
    check_manifest()
    profile = pins["qemu_arm_virt"]
    assert isinstance(profile, dict)
    check_transcript(boot(profile))
    print(
        "seL4 spawn plane check: a component constructed two unmodified children from "
        "grant-resolved executables, handed each the capabilities its slots name, and "
        "observed a termination through a supervision handle rather than a task id"
    )


if __name__ == "__main__":
    main()
