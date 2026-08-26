#!/usr/bin/env python3

"""RP2 gate: the demo-scoped AArch64 product vertical slice.

Boots `build/slime-sel4-demo.elf` -- the image whose root task embeds
`contracts/generation-manifest/v1/compositions/sel4-demo.zti` -- and asserts RP2's exit
condition on the `aarch64-sel4-qemu-virt` profile:

    One demo-scoped AArch64 generation runs the Slime component model and the
    data path the demo needs, and its rollback and wrong-target rejection arms
    are observed on that same profile rather than inherited from retired x86
    evidence.

Three arms, each a separate boot, because each asserts a different outcome.

# Arm 1: one generation, all three parts

RP2 asks for the C7 sample exchange and "the C8 route provisioning/data path
required by RP4/RP6 under one demo-scoped generation rather than across separate
plane fixtures". That is what this arm observes and it is the whole reason the
`demo` boot action exists: `sel4-sample.zti` proves the C7 half, `sel4-stream.zti`
proves the C8 half, and `sel4.zti` proves the product graph, but no generation
before this one carried all three, so "the component-launch and data path
together" was asserted only across three images.

The compositions themselves are deliberately the existing ones. What RP2 makes
new is the *generation*; reusing `drive_sample_plane`'s exchange and
`launch_fabric_graph`'s graph keeps this arm's evidence the evidence
`sel4_sample_check` and `sel4_stream_check` already froze. `fabric-service`
needed no new branch: `demo` matches none of its named boot actions and falls
through to exactly the stream composition.

# Arm 2: rollback on an AArch64 generation *pair*

A failing pending demo generation must return to a verified known-good demo
generation, across fresh QEMU processes, on this profile. This reuses the
selector image and the B35 store fixture rather than a second mechanism: that
image embeds no generation at all (`slime-root/build.rs` gates the embed on
`SLIME_BOOT_SELECTOR`), so the generations under test are exactly the disk's.

What makes this RP2 evidence rather than a restatement of
`sel4_boot_selection_check` is *which* generations: both roots here are demo
generations carrying the full slice, so the graph that rolls back is the graph
the demo runs. The existing gate pairs two `sel4` product generations.

# Arm 3: wrong-target rejection before mapping

RP2 requires a wrong-target artifact "rejected before any executable byte is
mapped" on this same admission path. Every existing wrong-target assertion in
this repository is host-side (`check-rpi5-artifacts.py`,
`check-architecture-contract.py`) or a unit test; none boots a generation whose
component image is qualified for another target, so the root's own refusal was
never observed.

`SLIME_WRONG_TARGET_EXECUTABLE` re-qualifies one declared executable for another
*admitted* profile, leaving the ELF body and the rest of the generation valid, so
the refusal cannot be an unrelated admission error wearing a wrong-target label.
The root must count it (`wrong_target=1`), exclude it from the loadable set, and
fail the spawn closed.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import tomllib
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from harness import GENERATION_COMPOSITIONS, profile_text, profile_integer, sha256_file  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
IMAGE = ROOT / "build" / "slime-sel4-demo.elf"
MANIFEST = ROOT / "build" / "slime-sel4-demo.identity.json"
SELECTOR_IMAGE = ROOT / "build" / "slime-sel4-boot-selection.elf"
SELECTOR_MANIFEST = ROOT / "build" / "slime-sel4-boot-selection.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
GENERATOR = ROOT / "scripts" / "build" / "build-generation.py"
STORE_FIXTURE = ROOT / "scripts" / "build" / "build-store-fixture.py"
FIXTURE = GENERATION_COMPOSITIONS / "sel4-demo.zti"
IMAGE_VARIANT = "demo"
MANIFEST_NAME = "sel4-demo"

BOOT_TIMEOUT_SECONDS = 420
STORE_FIRST = 40
SECTOR = 512

# The executable re-qualified for another target in arm 3, and the profile it is
# falsely qualified for. `sample-lender` is a data-path component rather than a
# root-autostart one, so the refusal lands on `init`'s spawn -- which is the
# interesting direction: the root admitted the generation, excluded the image
# from the loadable set, and then refused to construct a child from it.
WRONG_TARGET_EXECUTABLE = "sample-lender"
WRONG_TARGET_PROFILE = "aarch64-rpi5"

# Grouped into causal chains, not one global order: the fabric participants
# provision concurrently and the order they arrive in is a scheduling detail.
# What is not a detail is the order *within* each chain, and above all the order
# *between* them -- the C7 exchange completes, then the C8 graph provisions, then
# the product graph launches, all under one admitted generation.
CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "one generation declared the whole slice",
        (
            # 13 executables and 13 instances: init, the product graph's four,
            # the C7 pair, and the C8 six. This count *is* the milestone claim.
            r"SLIME_ROOT generation admitted number=\d+ executables=13 instances=13 grants=26 health=4 bootstrap=1",
            # The C8 half is real: a fabric graph the root validated against its
            # own ceilings, not an absent one. An earlier revision of this gate
            # asserted the rest of the slice over a generation with
            # `graph=absent`, which is RP2's C8 clause silently dropped.
            r"SLIME_ROOT fabric graph=admitted schemas=2 routes=2 participants=6 interpositions=0",
            r"SLIME_ROOT graph admitted executables=13 instances=13 slimecm=0 elf=13 unrecognized=0",
            r"SLIME_GRAPH staged instances=1 root_autostart=1 loadable_executables=13 slimecm=0 wrong_target=0 unrecognized=0",
        ),
    ),
    (
        "the C7 bounded data path ran and reclaimed",
        (
            r"\[init\] demo data path spawned",
            # A payload larger than the control-message bound, through real
            # frames: created, sealed irreversibly, loaned, mapped read-only,
            # verified, returned exactly once.
            r"SLIME_GRAPH buffer created task=\d+ slot=\d+ id=\d+ pages=2 writable=1",
            r"\[sample-lender\] seal is irreversible",
            r"SLIME_GRAPH loan created task=\d+ slot=\d+ id=\d+ to=\d+ offset=0 length=8192",
            r"\[sample-receiver\] loaned bytes mapped",
            r"\[sample-receiver\] loan stays read-only",
            r"\[sample-receiver\] payload verified",
            r"SLIME_GRAPH loan returned task=\d+ slot=\d+ id=\d+",
            # Both handles were collected, and a second status call on a
            # collected handle is refused -- which is what proves the slot was
            # released rather than merely reported.
            r"\[init\] demo sample exchange complete",
        ),
    ),
    (
        "the C8 route path provisioned under the same generation",
        (
            r"\[init\] demo fabric control channels minted",
            r"\[init\] demo fabric service spawned",
            # The denial arm: a component holding a real control endpoint but no
            # declared edge is refused. The denial under test is "no declared
            # edge", not "no channel".
            #
            # It precedes the sweep's completion rather than following it, and
            # that is a causal fact rather than a scheduling one: the fabric
            # refuses each request as it arrives, so the intruder's refusal lands
            # inside the provisioning sweep. An earlier revision of this chain
            # ordered it after `every declared stream edge provisioned` and the
            # gate reported it out of order on a green boot.
            r"\[fabric\] ungranted component denied: fabric-intruder",
            r"\[fabric\] every declared stream edge provisioned",
            r"\[fabric\] stream plane complete",
            r"\[init\] demo data path complete",
        ),
    ),
    (
        "the product component graph launched over that same generation",
        (
            r"\[init\] launching component graph",
            r"\[spawn-service\] ready",
            r"\[spawn-service\] complete",
            r"\[console\] channel plane complete",
            r"\[init\] component services completed",
        ),
    ),
    (
        "the slice drained and the supervisor certified it",
        (
            r"SLIME_GRAPH served live=0 unsupported=0 buffers=\d+ windows=0 tasks=0",
            r"SLIME_GRAPH tasks reclaimed live=0 slots=[1-9]\d*",
            r"SLIME_GRAPH native task_caps=0 exports=0 tickets=0",
            r"SLIME_GRAPH loans served=\d+ loans=0 mappings=0 regions=0 orphans=0 quota=0",
            r"SLIME_GRAPH HEALTHY generation=1 required=4 live=0 completed=4 failed=0",
        ),
    ),
)

TERMINAL_MARKER = CHAINS[-1][1][-1]

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL .*",
    r"SLIME_GRAPH FAIL .*",
    r"SLIME_GRAPH component exit .*status=-?[1-9]\d*",
    r"\[init\] demo plane fail: .*",
    r"\[init\] unknown boot action",
    r"SLIME_GRAPH endpoint unplaced .*",
    r"seL4 called fail",
    r"Caught cap fault",
    r"Caught vm fault",
    r"Caught user exception",
    r"panicked at ",
    r"aborted at ",
    r"\(aborted\)",
)

# Arm 3's own markers. The root must admit the generation, exclude the
# wrong-target image from the loadable set, and refuse the spawn.
WRONG_TARGET_MARKERS: tuple[tuple[str, str], ...] = (
    (
        "the generation was still admitted, so the refusal is about the target",
        r"SLIME_ROOT generation admitted number=\d+ executables=13 instances=13 ",
    ),
    (
        "one image was recognized but not admitted for this profile",
        r"SLIME_ROOT graph admitted executables=13 instances=13 slimecm=0 elf=12 unrecognized=0",
    ),
    (
        "the root counted exactly one wrong-target image and excluded it",
        r"SLIME_GRAPH staged instances=1 root_autostart=1 loadable_executables=12 slimecm=0 wrong_target=1 unrecognized=0",
    ),
    (
        "constructing a child from the wrong-target image failed closed",
        r"\[init\] demo plane fail: spawn lender",
    ),
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 demo plane check: {message}")


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


def run(command: list[str], environment: dict[str, str] | None = None) -> None:
    print(f"[run] {' '.join(command)}", flush=True)
    process = subprocess.run(command, cwd=ROOT, env=environment, check=False)
    if process.returncode:
        fail(f"command failed ({process.returncode}): {' '.join(command)}")


def build_image(*, wrong_target: bool = False) -> None:
    environment = dict(os.environ)
    if wrong_target:
        environment["SLIME_WRONG_TARGET_EXECUTABLE"] = (
            f"{WRONG_TARGET_EXECUTABLE}={WRONG_TARGET_PROFILE}"
        )
    else:
        environment.pop("SLIME_WRONG_TARGET_EXECUTABLE", None)
    run([sys.executable, str(BUILD_SCRIPT), "--demo-plane"], environment)


def check_manifest(manifest_path: Path, image: Path, variant: str, flag: str) -> None:
    if not manifest_path.is_file():
        fail(f"missing identity manifest {manifest_path.relative_to(ROOT)}")
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot parse {manifest_path.relative_to(ROOT)}: {error}")
    if not isinstance(manifest, dict) or manifest.get("kind") != "slime-sel4-image-identity":
        fail(f"{manifest_path.relative_to(ROOT)} is not a Slime seL4 identity manifest")
    # Every seL4 image is built from the same sources and differs only in which
    # generation the root task embeds, so booting the wrong one would fail on
    # markers rather than on identity. Checking the variant reports the cause.
    if manifest.get("variant") != variant:
        fail(
            f"{manifest_path.relative_to(ROOT)} records variant "
            f"{manifest.get('variant')!r}, not {variant!r}; rebuild with {flag}"
        )
    recorded = manifest.get("image")
    if not isinstance(recorded, dict) or not isinstance(recorded.get("sha256"), str):
        fail("identity manifest does not record the packaged image digest")
    if not image.is_file():
        fail(f"missing packaged image {image.relative_to(ROOT)}")
    actual = sha256_file(image, fail)
    if actual != recorded["sha256"]:
        fail(
            f"{image.relative_to(ROOT)} SHA-256 is {actual}, but the identity manifest "
            f"records {recorded['sha256']}; rebuild before booting"
        )


def qemu_command(profile: dict[str, object], image: Path, disk: Path | None) -> list[str]:
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
        str(image),
    ]
    if disk is not None:
        command += [
            "-drive",
            f"if=none,id=slimedisk,format=raw,file={disk}",
            "-device",
            "virtio-blk-device,drive=slimedisk",
        ]
    return command


def boot(
    profile: dict[str, object],
    image: Path,
    *,
    disk: Path | None = None,
    terminal: str,
    stop_on_failure: bool = True,
) -> str:
    """Boot `image` and return the serial transcript.

    The root task suspends itself once the graph has drained, so QEMU stays
    alive afterwards and waiting for an exit would always time out. Output is
    read line by line and the guest is killed as soon as the terminal marker --
    or, when the caller expects success, any failure marker -- appears.

    `stop_on_failure=False` is for the arm whose expected outcome *is* a root
    fatal: treating that as a failed run would report "did not reach" for a
    transcript containing exactly what the arm requires.
    """
    command = qemu_command(profile, image, disk)
    print(f"[boot] {' '.join(command)}", flush=True)
    terminal_pattern = re.compile(terminal)
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
            if terminal_pattern.search(line):
                break
            if stop_on_failure and failures.search(line):
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
    if terminal_pattern.search(transcript) is None:
        report_transcript(transcript)
        if timed_out:
            fail(f"boot exceeded {BOOT_TIMEOUT_SECONDS}s without reaching {terminal!r}")
        fail(f"boot ended without reaching {terminal!r}")
    return transcript


def report_transcript(transcript: str) -> None:
    # seL4's own decode diagnostics are expected on this path (the root probes
    # empty slots deliberately) and drown a 1800-line transcript, so drop them.
    tail = [line for line in transcript.splitlines() if "seL4(CPU" not in line][-40:]
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
    for label, chain in CHAINS:
        position = 0
        for pattern in chain:
            match = re.compile(pattern).search(transcript, position)
            if match is None:
                report_transcript(transcript)
                if re.search(pattern, transcript) is not None:
                    fail(f"marker out of order in {label!r}: {pattern}")
                fail(f"missing marker in {label!r}: {pattern}")
            position = match.end()
    terminals = re.findall(TERMINAL_MARKER, transcript)
    if len(terminals) != 1:
        fail(f"expected exactly one healthy supervisor terminal, saw {len(terminals)}")


def check_ordered_across_chains(transcript: str) -> None:
    """The three parts ran in one boot, in this order.

    Each chain above is internally ordered, but the milestone claim is *between*
    them: the C7 exchange completed, then the C8 graph provisioned, then the
    product graph launched -- all under one admitted generation. Asserting the
    chains alone would pass on a transcript where the product graph launched
    first, which is a different composition.
    """
    stages = (
        ("the C7 exchange completed", r"\[init\] demo sample exchange complete"),
        ("the C8 graph provisioned", r"\[fabric\] every declared stream edge provisioned"),
        ("the data path finished", r"\[init\] demo data path complete"),
        ("the product graph launched", r"\[init\] launching component graph"),
        ("the product services completed", r"\[init\] component services completed"),
    )
    position = 0
    for label, pattern in stages:
        match = re.compile(pattern).search(transcript, position)
        if match is None:
            report_transcript(transcript)
            fail(f"the slice did not reach, in order: {label} ({pattern})")
        position = match.end()


def generator_module(name: str):
    spec = importlib.util.spec_from_file_location(name, GENERATOR)
    if spec is None or spec.loader is None:
        fail("cannot import the generation builder")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def build_demo_generation(output: Path, number: int, bundle: str, *, failing: bool) -> Path:
    """Build one demo generation for the rollback pair.

    `SLIME_BOOT_SELECTION_FAIL` makes `init` report itself unhealthy before it
    composes anything, which is how a generation that *runs* and fails is
    distinguished from one that cannot start.
    """
    environment = dict(os.environ)
    environment.update(
        SLIME_TARGET_PROFILE="aarch64-sel4-qemu-virt",
        SLIME_SEL4_MANIFEST=MANIFEST_NAME,
        SLIME_GENERATION_NUMBER=str(number),
        SLIME_BOOT_BUNDLE_IDENTITY=bundle,
    )
    environment.pop("SLIME_WRONG_TARGET_EXECUTABLE", None)
    if failing:
        environment["SLIME_BOOT_SELECTION_FAIL"] = "1"
    else:
        environment.pop("SLIME_BOOT_SELECTION_FAIL", None)
    run([sys.executable, str(GENERATOR), str(output)], environment)
    generation = output / "generation.bin"
    if not generation.is_file():
        fail(f"generation builder omitted {generation}")
    return generation


def make_store(paths: list[Path], bundle: str, attempts: int) -> bytes:
    module = generator_module("slime_build_generation_rp2")
    saved = dict(os.environ)
    try:
        os.environ.update(
            SLIME_PENDING_GENERATION="1",
            SLIME_PENDING_ATTEMPTS=str(attempts),
            SLIME_KNOWN_GOOD_FIRST="1",
            SLIME_ACCEPTED_RELEASE_SEQUENCE="1",
            SLIME_PENDING_RELEASE_SEQUENCE="2",
            SLIME_BOOT_BUNDLE_IDENTITY=bundle,
        )
        return module.build_bootstore([path.read_bytes() for path in paths])
    finally:
        os.environ.clear()
        os.environ.update(saved)


def make_disk(path: Path, store: bytes) -> None:
    """Write a disk whose BootState carries `store`.

    `boot-selection` is the store fixture's only variant that accepts a supplied
    boot store; it names the *disk shape* the selector reads, not the gate using
    it, so RP2's rollback pair shares it with B35 rather than needing a variant
    of its own. The generations inside it are this gate's.
    """
    blob = path.with_suffix(".store")
    blob.write_bytes(store)
    run(
        [
            sys.executable,
            str(STORE_FIXTURE),
            str(path),
            "boot-selection",
            "--boot-store",
            str(blob),
        ]
    )


def expect_selected(transcript: str, number: int, pending: int, attempts: int) -> None:
    pattern = rf"SLIME_BOOT selected .* number={number} pending={pending} attempts={attempts}"
    if re.search(pattern, transcript) is None:
        report_transcript(transcript)
        fail(f"missing marker {pattern}")


def only_boot_state_changed(before: bytes, after: bytes) -> None:
    """Exactly the two redundant BootState slots changed, and nothing else.

    Both bounds matter. Without the upper one this would accept a boot that
    wrote anywhere from sector 40 to the end of the disk, so it could not observe
    the "only BootState sectors mutated" property RP2's exit condition records —
    a gate read as covering more than it asserts, which is the B67/B72/B73/B75
    shape. The window is two 512-byte slots because that is what
    `select_bootstate` commits, matching `check-sel4-boot-selection.py`'s
    `only_slots`.
    """
    start = STORE_FIRST * SECTOR
    end = start + 2 * SECTOR
    if (
        len(before) != len(after)
        or before[:start] != after[:start]
        or before[end:] != after[end:]
    ):
        fail("a boot mutated disk bytes outside the redundant BootState slots")
    if before[start:end] == after[start:end]:
        fail("BootState did not change")


def check_rollback_pair(profile: dict[str, object]) -> None:
    """RP2: a failing pending *demo* generation rolls back to a verified one.

    Both roots carry the full demo slice, so what rolls back is the graph the
    demo runs. The selector image embeds no generation of its own, so the two
    generations under test are exactly the ones written to this disk.
    """
    run([sys.executable, str(BUILD_SCRIPT), "--boot-selection"])
    check_manifest(SELECTOR_MANIFEST, SELECTOR_IMAGE, "boot-selection", "--boot-selection")
    bundle = str(
        json.loads(SELECTOR_MANIFEST.read_text(encoding="utf-8"))["boot_bundle_identity"]
    )
    with tempfile.TemporaryDirectory(prefix="slime-rp2-") as temporary:
        work = Path(temporary)
        known_good = build_demo_generation(work / "good", 1, bundle, failing=False)
        failing = build_demo_generation(work / "bad", 99, bundle, failing=True)
        if known_good.read_bytes() == failing.read_bytes():
            fail("the rollback pair is one generation twice")

        disk = work / "rollback.img"
        make_disk(disk, make_store([known_good, failing], bundle, 2))
        initial = disk.read_bytes()

        first = boot(
            profile,
            SELECTOR_IMAGE,
            disk=disk,
            terminal=r"SLIME_BOOT unhealthy",
            stop_on_failure=False,
        )
        expect_selected(first, 99, 1, 1)
        after_first = disk.read_bytes()
        only_boot_state_changed(initial, after_first)

        second = boot(
            profile,
            SELECTOR_IMAGE,
            disk=disk,
            terminal=r"SLIME_BOOT unhealthy",
            stop_on_failure=False,
        )
        expect_selected(second, 99, 1, 0)
        after_second = disk.read_bytes()
        only_boot_state_changed(after_first, after_second)

        # Attempts exhausted: the next boot must be the *other* generation.
        third = boot(
            profile,
            SELECTOR_IMAGE,
            disk=disk,
            terminal=r"SLIME_BOOT selected",
            stop_on_failure=False,
        )
        expect_selected(third, 1, 0, 0)
        if "number=99" in third:
            fail("the failing pending generation survived into the fallback boot")
        settled = disk.read_bytes()
        only_boot_state_changed(after_second, settled)

        # And it stays there: a rollback that re-armed the failing candidate
        # would boot 99 again here.
        stable = boot(
            profile,
            SELECTOR_IMAGE,
            disk=disk,
            terminal=r"SLIME_BOOT selected",
            stop_on_failure=False,
        )
        expect_selected(stable, 1, 0, 0)
        if disk.read_bytes() != settled:
            fail("the known-good fallback boot mutated the disk")


def check_wrong_target(profile: dict[str, object]) -> None:
    """RP2: a wrong-target artifact is refused before its bytes are mapped.

    The injected image is restored in a `finally`, not after the assertions. Every
    `fail()` below raises, so cleanup placed after them ran only on success —
    leaving a component image falsely qualified for `aarch64-rpi5` at
    `build/slime-sel4-demo.elf` exactly when something had gone wrong, for
    `--arm slice` or `check-sel4-boot-layout.py` to boot next. A cleanup that only
    runs when nothing failed is not cleanup.
    """
    try:
        check_wrong_target_arm(profile)
    finally:
        build_image()


def check_wrong_target_arm(profile: dict[str, object]) -> None:
    build_image(wrong_target=True)
    check_manifest(MANIFEST, IMAGE, IMAGE_VARIANT, "--demo-plane")
    transcript = boot(
        profile,
        IMAGE,
        terminal=r"SLIME_GRAPH FAIL required instance init exit status=1",
        stop_on_failure=False,
    )
    position = 0
    for description, pattern in WRONG_TARGET_MARKERS:
        match = re.compile(pattern).search(transcript, position)
        if match is None:
            report_transcript(transcript)
            if re.search(pattern, transcript) is not None:
                fail(f"wrong-target marker out of order: {description} ({pattern})")
            fail(f"missing wrong-target marker: {description} ({pattern})")
        position = match.end()
    # The refusal must be a refusal, not a fault: nothing may have executed from
    # the wrong-target image.
    for forbidden in (r"\[sample-lender\] ", r"Caught vm fault", r"Caught user exception"):
        match = re.search(forbidden, transcript)
        if match is not None:
            report_transcript(transcript)
            fail(f"the wrong-target image was not refused before use: {match.group(0)!r}")


def check_marker_corpus() -> None:
    if not CHAINS:
        fail("CHAINS must contain a non-empty marker corpus")
    for label, chain in CHAINS:
        if not chain or len(set(chain)) != len(chain):
            fail(f"CHAINS entry {label!r} must be non-empty and duplicate-free")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the RP2 demo-scoped AArch64 slice and assert its three arms"
    )
    parser.add_argument(
        "--no-build",
        action="store_true",
        help="boot the already-built demo image instead of rebuilding it first",
    )
    parser.add_argument(
        "--arm",
        choices=("slice", "rollback", "wrong-target", "all"),
        default="all",
        help="run one arm only (default: all three)",
    )
    arguments = parser.parse_args()

    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    if not FIXTURE.is_file():
        fail(f"missing generation fixture {FIXTURE.relative_to(ROOT)}")
    # The wrong-target arm must build: its whole subject is an image this gate
    # injects, and it must rebuild afterwards so the poisoned artifact does not
    # outlive the run. `--no-build` cannot be honored there, so the combination
    # is refused rather than silently ignored — a flag naming an expensive
    # behavior the code discards is a contract the code does not keep.
    if arguments.no_build and arguments.arm in ("wrong-target", "all"):
        fail(
            "--no-build cannot apply to the wrong-target arm, which builds the "
            "injected image and restores the clean one; use --arm slice or "
            "--arm rollback with --no-build"
        )
    check_marker_corpus()
    pins = load_pins()
    profile = pins["qemu_arm_virt"]
    assert isinstance(profile, dict)

    # Each arm reports only what it observed. `--arm` exists so a single arm can
    # be re-run while iterating, and a partial run printing the full three-arm
    # sentence would be a false evidence claim — exactly the "gate asserting a
    # property the system never had" shape this repository's backlog is full of.
    observed: list[str] = []
    if arguments.arm in ("slice", "all"):
        if not arguments.no_build:
            build_image()
        check_manifest(MANIFEST, IMAGE, IMAGE_VARIANT, "--demo-plane")
        transcript = boot(profile, IMAGE, terminal=TERMINAL_MARKER)
        check_transcript(transcript)
        check_ordered_across_chains(transcript)
        observed.append(
            "one demo-scoped AArch64 generation ran the C7 bounded data path, "
            "provisioned the C8 route graph, and launched the product component "
            "graph in a single boot"
        )
    if arguments.arm in ("rollback", "all"):
        check_rollback_pair(profile)
        observed.append(
            "a failing pending demo generation rolled back to a verified demo "
            "known-good root across fresh QEMU processes"
        )
    if arguments.arm in ("wrong-target", "all"):
        # `check_wrong_target` restores the clean image itself, in a `finally`,
        # so the poisoned artifact does not outlive a failed arm either.
        check_wrong_target(profile)
        observed.append(
            "a wrong-target component image was refused before any of its bytes "
            "were mapped"
        )

    scope = "" if arguments.arm == "all" else f" (--arm {arguments.arm})"
    print(f"seL4 demo plane check{scope}: " + "; and ".join(observed))


if __name__ == "__main__":
    main()
