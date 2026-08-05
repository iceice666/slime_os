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
import hashlib
import json
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
IMAGE = ROOT / "build" / "slime-sel4-spawn.elf"
MANIFEST = ROOT / "build" / "slime-sel4-spawn.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
FIXTURE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-spawn.zti"
IMAGE_VARIANT = "spawn"

BOOT_TIMEOUT_SECONDS = 120

REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        "the spawn generation was admitted",
        r"SLIME_ROOT generation admitted number=\d+ components=3 grants=3 ",
    ),
    (
        "every payload is a native ELF image",
        r"SLIME_ROOT graph admitted; legacy SLIMECM images not activated "
        r"components=3 slimecm=0 elf=3 unrecognized=0",
    ),
    (
        # B10: init's factory sits at the slot the boot layout names, not at a
        # number the root invented. `init.rs` reads the same table through its
        # generated `ENDPOINT_FACTORY_SLOT`, so the two readers agree by
        # construction rather than by inspection.
        "init's endpoint factory was placed at its layout slot",
        r"SLIME_GRAPH factory placed task=\d+ component=init slot=0 "
        r"kind=endpoint-factory",
    ),
    (
        # The layout names console at 1 and sysinfo at 4. A cursor would have
        # numbered them 1 and 2, and `init.rs`'s `SYSINFO_SLOT` would then
        # resolve to whatever else landed at 4 -- the positional coupling B10
        # exists to remove, and it only became observable once init held an
        # executable grant at all.
        "init was staged holding both declared executables",
        r"SLIME_GRAPH staged task=\d+ component=init grants=\d+ executables=2 ",
    ),
    # -- required check: an ungranted or over-wide spawn is refused --
    (
        "an empty slot cannot name an executable",
        r"SLIME_GRAPH spawn refused task=\d+ slot=63 ungranted",
    ),
    (
        # A slot holding real authority of another kind. Init genuinely holds
        # its endpoint factory at slot 0, so this is a check on kind rather
        # than on possession.
        "a factory slot cannot name an executable",
        r"SLIME_GRAPH spawn refused task=\d+ slot=0 ungranted",
    ),
    ("both refusals reached the component", r"\[init\] ungranted executable refused"),
    (
        # The narrowing rule: a grant's rights must be a subset of what the
        # parent holds. Init holds the factory with `endpointCreate` alone, so
        # asking to hand on `bufferCreate` is asking the root to manufacture
        # authority no generation declared.
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
        "a channel pair was minted through the declared factory",
        r"SLIME_GRAPH endpoint minted task=\d+ key=\d+ slots=\d+,\d+",
    ),
    (
        "console was authorized from its declared executable grant",
        r"SLIME_GRAPH spawn authorized task=\d+ slot=1 component=console grants=1",
    ),
    (
        # The distribution step. The end moves to the child at *the child's*
        # slot 0, which is the only slot `console.rs` addresses -- and that
        # component never learns the number, because the order of the parent's
        # grant list is what fixes it.
        "console received its channel end at slot 0",
        r"SLIME_GRAPH channel handed parent=\d+ child=\d+ key=\d+ slot=0",
    ),
    (
        "console was constructed with its declared capabilities",
        r"SLIME_GRAPH spawned task=\d+ child=\d+ component=console grants=1 "
        r"channels=1 handle=\d+",
    ),
    ("the spawn reached the component", r"\[init\] console spawned"),
    (
        # A live child has no outcome, and the query says so rather than
        # blocking or inventing one.
        "a live child reports no outcome",
        r"\[init\] live child reports no outcome",
    ),
    (
        # An endpoint grant is a move, not a copy: a channel's queues are
        # resolved by which task holds each end, so a parent that kept a
        # working copy would hold an end the child also holds. Init keeps the
        # half it did not grant, and the gate asserts both halves of that --
        # the granted slot stops resolving, the retained one still works.
        "the granted end left the parent and the retained end still works",
        r"\[init\] handed channel end released",
    ),
    # -- required check: termination observed through a supervision handle --
    (
        "sysinfo was authorized from its layout-named executable slot",
        r"SLIME_GRAPH spawn authorized task=\d+ slot=4 component=sysinfo grants=1",
    ),
    ("sysinfo was constructed", r"\[init\] sysinfo spawned"),
    (
        # The launch context, sent down the half init kept. `sysinfo` is
        # blocked in `recv` on the half it was granted.
        "the launch context crossed the minted channel",
        r"\[init\] launch context sent",
    ),
    (
        # A parked wait. Deliberately *not* claimed as proof that the park was
        # on a supervision source: `main.rs` emits this line for any parked
        # wait, and the spawned console parks on an endpoint too. What proves
        # the supervision park is `supervision woken` below, which no channel
        # can produce. This marker's job is only to pin that a park happened
        # before the wake rather than after it.
        "a wait parked before the wake",
        r"SLIME_GRAPH parked task=\d+ reason=wait",
    ),
    (
        # The unmodified binary ran and produced its own output, which is what
        # makes "constructed from a grant-resolved executable" a fact about a
        # real component rather than about an empty task.
        "the unmodified child ran",
        r"\[sysinfo\] spawned through profile",
    ),
    (
        "the child's death woke its parent",
        r"SLIME_GRAPH supervision woken task=\d+ child=\d+",
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
        # End to end, not on the root's own bookkeeping: the unmodified
        # `console.rs` `debug_write`s whatever arrives on its slot 0, so this
        # line is the child *reading* the end it was handed. The `channel
        # handed` marker above is the root saying it moved one; this is the
        # child proving it landed somewhere it could use.
        "the spawned console read the end it was handed",
        r"\[console\] spawned child reached",
    ),
    (
        # `waits=0` is the teardown property: nobody is still registered on a
        # child's termination, which would be a wake that can never arrive.
        # `terminated` is deliberately non-zero -- one record per child that
        # ended, kept past reclamation by design -- so a zero there would mean
        # the supervision path recorded nothing at all.
        "every spawn, drop, and wait was accounted for",
        r"SLIME_GRAPH spawns served=2 drops=1 endpoints=2 terminated=[1-9]\d* waits=0",
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


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    try:
        with path.open("rb") as handle:
            while chunk := handle.read(1024 * 1024):
                digest.update(chunk)
    except OSError as error:
        fail(f"cannot hash {path.relative_to(ROOT)}: {error}")
    return digest.hexdigest()


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
    actual = sha256_file(IMAGE)
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
    check_spawned_children_are_unmodified()


# Components this fixture spawns that carry no seL4 branch at all. The claim is
# checked against the sources rather than inferred from the transcript: a
# component that grew a `SLIME_SEL4_SPAWN_CHECK` arm would still produce every
# marker above while quietly making the milestone's central claim false.
UNMODIFIED_CHILDREN = ("console", "sysinfo")


def check_spawned_children_are_unmodified() -> None:
    """Neither spawned child knows it is running on seL4.

    P5.3.3 claims that a component written against the retired kernel's spawn
    ABI is constructed by `slime-root` unchanged. The transcript shows both
    children running, but it cannot show that they are the *same* binaries the
    x86 oracle runs -- a child rewritten to suit this root would look identical
    on serial. So the sources are read directly.

    `console.rs` is allowed one guarded probe, which P5.3.2 added and which this
    gate's generation does not set; what it may not have is a spawn-specific
    branch.
    """
    for name in UNMODIFIED_CHILDREN:
        source = ROOT / "components" / "bins" / "src" / "bin" / f"{name}.rs"
        try:
            text = source.read_text(encoding="utf-8")
        except OSError as error:
            fail(f"cannot read {source.relative_to(ROOT)}: {error}")
        if "SLIME_SEL4_SPAWN_CHECK" in text:
            fail(
                f"{source.relative_to(ROOT)} branches on the spawn check flag; "
                "the milestone requires this component to run unmodified"
            )
    print(
        "components: "
        + ", ".join(UNMODIFIED_CHILDREN)
        + " carry no spawn-check branch and run as the x86 oracle builds them",
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
