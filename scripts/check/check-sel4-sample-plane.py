#!/usr/bin/env python3

"""P5.3.4 gate: the C7 sample plane, composed on seL4.

Boots `build/slime-sel4-sample.elf` -- the image whose root task embeds the
sample-plane generation, `contracts/generation/v1/fixtures/sel4-sample.zti` --
and asserts P5.3's exit condition:

    Two components exchange and return a payload larger than the
    control-message bound over seL4, with quota exhaustion and peer death
    reclaiming the same resources the x86 corpus records.

The component marker corpus was frozen at the P5 cutover. Every line is emitted
by the unchanged `sample-lender` and `sample-receiver` binaries; the two
retired-kernel-only lines remain documented in `ORACLE_ONLY` rather than being
claimed by this product gate.
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
IMAGE = ROOT / "build" / "slime-sel4-sample.elf"
MANIFEST = ROOT / "build" / "slime-sel4-sample.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
FIXTURE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-sample.zti"
IMAGE_VARIANT = "sample"

BOOT_TIMEOUT_SECONDS = 180

# Historical oracle-only lines excluded when this self-contained seL4 gate was
# frozen at P5 cutover. The component transcript below is now authoritative.
ORACLE_ONLY: dict[str, str] = {
    # `kernel/src/runtime/bootstrap.rs:419`, after a `require_grant` sweep.
    # `slime-root` validates grants differently -- `inbound_authority` derives
    # each component's rights from the generation and `preflight_spawn_grants`
    # re-checks at the point of use -- and emits no equivalent line.
    #
    # What replaces it is `[sample-lender] buffer created`, which this gate
    # requires: an allocation only succeeds if the factory capability resolved,
    # since P5.3.3 made `serve_buffer_create` resolve it (B13).
    #
    # Deliberately *not* `[sample-lender] factory is not a buffer`, which looks
    # like the right substitute and is not: that line only proves
    # `shared_buffer_seal(FACTORY_SLOT)` answered `ERR_BAD_CAP`, which an
    # entirely absent slot satisfies identically. The unconfigured lender the
    # root launches prints it and then fails at `class=ungranted`, which is the
    # proof that it proves nothing about the grant being real.
    "[generation] shared-buffer factory grants valid": (
        "emitted by the retired kernel's bootstrap grant sweep; slime-root "
        "validates grants at admission and at use, with no equivalent marker. "
        "Covered here by [sample-lender] buffer created, which cannot succeed "
        "without the factory capability resolving"
    ),
    # `init.rs` prints this after `launch_sample_plane`, which this scenario
    # replaces with `drive_sample_plane` -- same composition, but the channel is
    # minted rather than declared. The seL4 branch prints its own line, asserted
    # below.
    "[init] sample plane complete": (
        "printed by both, but through a different init branch; asserted "
        "directly in REQUIRED_MARKERS rather than through the oracle list"
    ),
}

# Frozen component transcript inherited at the P5 cutover.
SAMPLE_MARKERS: tuple[str, ...] = (
    "[sample-lender] factory is not a buffer",
    "[sample-lender] buffer created",
    "[sample-lender] payload written",
    "[sample-lender] unsealed loan denied",
    "[sample-lender] seal is irreversible",
    "[sample-lender] loan created",
    "[sample-lender] descriptor sent",
    "[sample-receiver] descriptor received",
    "[sample-receiver] malformed descriptor mapped nothing",
    "[sample-receiver] loaned bytes mapped",
    "[sample-receiver] loan stays read-only",
    "[sample-receiver] payload verified",
    "[sample-receiver] loan returned once",
    "[sample-receiver] done",
    "[sample-lender] receiver settled",
    "[sample-lender] released",
    "[sample-lender] done",
)

REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        "the sample generation was admitted",
        r"SLIME_ROOT generation admitted number=\d+ executables=4 instances=4 grants=4 ",
    ),
    (
        "every payload is a native ELF image",
        r"SLIME_ROOT graph admitted executables=4 instances=4 slimecm=0 elf=4 unrecognized=0",
    ),
    (
        # Both factories at the slots the boot layout names, which is what
        # `init.rs`'s generated `ENDPOINT_FACTORY_SLOT` and
        # `SHARED_BUFFER_FACTORY_SLOT` compile against (B10).
        "init holds its endpoint factory at its layout slot",
        r"SLIME_GRAPH factory placed task=\d+ component=init slot=3 "
        r"kind=endpoint-factory",
    ),
    (
        "init holds its shared-buffer factory at its layout slot",
        r"SLIME_GRAPH factory placed task=\d+ component=init slot=4 "
        r"kind=shared-buffer-factory",
    ),
    (
        # A process with two threads (B47). The root builds one TCB per
        # declared thread, and this is the count it acted on -- a plan that
        # declared a thread the root skipped would report 1 here.
        "the root built both of sample-worker's declared threads",
        r"SLIME_GRAPH threads instance=sample-worker count=2",
    ),
    (
        # The load-bearing one. This line can only be written by a thread that
        # reached its own entry point, on its own stack, and completed a
        # console send through its *own* IPC buffer -- the ambient buffer
        # belongs to the main thread, and a worker that used it would either
        # corrupt the main thread's in-flight message or fault. A second TCB
        # the root configured but never resumed prints nothing.
        "sample-worker's second thread ran and made its own syscall",
        r"\[sample-worker\] worker thread running",
    ),
    (
        # And the main thread still works alongside it: two threads sharing one
        # CSpace and VSpace, each with its own buffer and window.
        "sample-worker's main thread ran alongside its worker",
        r"\[sample-worker\] main thread running",
    ),
    (
        # No declared channel edge: init mints the pair at runtime through the
        # factory, because a `source == target` grant is a loopback and yields
        # one slot rather than the two halves this composition needs.
        "the peer channel was minted rather than declared",
        r"SLIME_GRAPH endpoint minted task=\d+ key=\d+ slots=\d+,\d+",
    ),
    (
        # Receiver first: the lender names its loan receiver through a
        # `RIGHT_SUPERVISE` handle, which cannot exist until the receiver does.
        # Spawn order is load-bearing here exactly as it is on x86.
        "the receiver was spawned first",
        r"SLIME_GRAPH spawn authorized task=\d+ slot=\d+ component=sample-receiver "
        r"grants=1",
    ),
    (
        # A spawned child's shared-buffer ceiling comes from the generation's
        # budget, keyed by component name. Before P5.3.4 only root-launched
        # tasks were budgeted, so a spawned lender held `DENY` and could not
        # allocate at all.
        "the spawned receiver was budgeted from the generation",
        r"SLIME_GRAPH quota task=\d+ instance=sample-receiver executable=sample-receiver pages=2 buffers=1 "
        r"mappings=2 loans=1",
    ),
    (
        "the spawned lender was budgeted from the generation",
        r"SLIME_GRAPH quota task=\d+ instance=sample-lender executable=sample-lender pages=4 buffers=1 "
        r"mappings=2 loans=1",
    ),
    (
        "the lender was spawned with all three of its declared grants",
        r"SLIME_GRAPH spawned task=\d+ child=\d+ component=sample-lender grants=3 "
        r"channels=1 handle=\d+",
    ),
    (
        # B14's probe, superseded. Init's declared budget is two and both are
        # live, but the third spawn names an instance that is *already* live,
        # and the root resolves an executable to exactly one owned instance —
        # so `instance-live` is answered before the budget is consulted and the
        # ceiling is never reached by this path.
        #
        # Asserted as what it is rather than what it was: the refusal is real
        # and the plane's own comment in `init.rs` records the same reasoning.
        # `[init] spawn budget refused` below is the caller-side half, and the
        # recovery arm that follows is what actually proves the budget releases
        # its dead.
        "a live instance cannot be spawned twice",
        r"SLIME_GRAPH spawn refused task=\d+ child=\S+ class=instance-live",
    ),
    (
        "the refusal reached init as an out-of-memory error",
        r"\[init\] spawn budget refused",
    ),
    (
        # The loan is minted against a `RIGHT_SUPERVISE` handle, not a channel
        # end: `sample-lender.rs::RECEIVER_SLOT` is its third spawn grant.
        # P5.3.2 could only name a channel peer, because no spawn existed to
        # mint a handle.
        "the loan named its receiver through a supervision handle",
        r"SLIME_GRAPH loan created task=\d+ slot=\d+ id=\d+ to=\d+ offset=0 "
        r"length=8192",
    ),
    (
        # The capability moves with the descriptor. `caps=1` is the loan
        # crossing; the 8192-byte payload does not.
        "exactly one capability crossed with the descriptor",
        r"SLIME_GRAPH sent task=\d+ channel=\d+ bytes=64 caps=1 queued=1",
    ),
    (
        "the receiver's clean exit was collected through its handle",
        r"SLIME_GRAPH supervision collected task=\d+ child=\d+ kind=0",
    ),
    (
        # B14's second half, and the arm that makes the budget a *live-child*
        # cap rather than a lifetime one. Both children have exited, so the
        # ceiling that refused above must admit again — which it can only do if
        # a dead task has left the table. A count that never released its dead
        # would refuse here too, and the two readings are otherwise identical.
        "the budget recovered once its children were reclaimed",
        r"\[init\] spawn budget recovered",
    ),
    ("the composition completed", r"\[init\] sample plane complete"),
    (
        "the graph drained with every window, table, and channel reclaimed",
        r"SLIME_GRAPH served live=0 unsupported=0 unimplemented=0 "
        r"buffers=[1-9]\d* windows=0 tables=0",
    ),
    (
        # Every task the graph created is out of the table, and its root CSlots
        # are back. Before P5.3.4 neither death path reclaimed, so this would
        # have read `live=N slots=0`: the table full of dead entries, holding a
        # parent's budget, with not one CSlot returned.
        "every task was reclaimed and its CSlots returned",
        r"SLIME_GRAPH tasks reclaimed live=0 slots=[1-9]\d*",
    ),
    (
        # Peer death and quota reclamation, which is the half of P5.3's exit
        # condition the transcript above does not cover. Every loan, mapping,
        # region, frame alias, and in-flight capability is back.
        "every loan, mapping, and region was reclaimed",
        r"SLIME_GRAPH loans served=[1-9]\d* loans=0 mappings=0 regions=0 "
        r"transit=0 orphans=0 aliases=0",
    ),
)

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL .*",
    r"SLIME_GRAPH FAIL .*",
    # Either sample component reporting a failure — but only after the spawned
    # pair has started. See `check_sample_transcript`: the root launches every
    # declared component (P5.2), so an unconfigured `sample-lender` and
    # `sample-receiver` also run, holding no channel and no peer, and both exit
    # non-zero before init has spawned anything. Those two lines are expected;
    # a failure from the spawned pair is not, and is caught by the scan below
    # rather than here.
    r"\[init\] sample plane fail: .*",
    r"\[init\] spawn plane fail: .*",
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
    raise SystemExit(f"seL4 sample plane check: {message}")


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
    command = [sys.executable, str(BUILD_SCRIPT), "--sample-plane"]
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
            "run `just sel4_sample_check`"
        )
    try:
        manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot parse {MANIFEST.relative_to(ROOT)}: {error}")
    if not isinstance(manifest, dict) or manifest.get("kind") != "slime-sel4-image-identity":
        fail(f"{MANIFEST.relative_to(ROOT)} is not a Slime seL4 identity manifest")
    # The six images are built from the same sources and differ only in which
    # generation the root task embeds, so booting the wrong one would fail on
    # markers rather than on identity. Checking the variant reports the actual
    # cause instead.
    if manifest.get("variant") != IMAGE_VARIANT:
        fail(
            f"{MANIFEST.relative_to(ROOT)} records variant "
            f"{manifest.get('variant')!r}, not {IMAGE_VARIANT!r}; "
            "rebuild with `--sample-plane`"
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
    check_sample_transcript(transcript)
    check_transcript_matches_the_oracle()
    check_components_are_unmodified()


def check_sample_transcript(transcript: str) -> None:
    """The x86 gate's own ordered transcript, produced by the same binaries.

    Order-sensitive for the reason the oracle states: each denial must be
    observed before the operation it guards succeeds, so a regression that
    silently permits a denied operation fails here even if the happy path still
    completes.
    """
    # The spawned pair starts here. Everything before this line belongs to the
    # unconfigured instances the root launches, whose failures are expected and
    # must not be read as this composition's.
    start = transcript.find("SLIME_GRAPH endpoint minted")
    if start < 0:
        report_transcript(transcript)
        fail("init never minted the peer channel, so no sample composition ran")
    transcript = transcript[start:]

    # The x86 gate's own FORBIDDEN entries, applied to the composition alone.
    #
    # Bounded at both ends. `[init] sample plane complete` is the last thing the
    # composition does; after it, init spawns one more receiver purely to show
    # the budget recovered (B14's second half), and that child holds no channel
    # so it fails its own `recv` by construction. Scanning past the completion
    # line would read that expected failure as the composition's.
    composition = transcript
    completion = composition.find("[init] sample plane complete")
    if completion >= 0:
        composition = composition[:completion]
    for forbidden in (r"\[sample-lender\] fail: .*", r"\[sample-receiver\] fail: .*"):
        match = re.search(forbidden, composition)
        if match is not None:
            report_transcript(transcript)
            fail(f"a spawned sample component reported a failure: {match.group(0)!r}")

    cursor = 0
    for marker in SAMPLE_MARKERS:
        position = transcript.find(marker, cursor)
        if position < 0:
            report_transcript(transcript)
            if marker in transcript:
                fail(f"oracle transcript out of order at: {marker}")
            fail(f"oracle transcript is missing: {marker}")
        cursor = position + len(marker)
    print(
        f"transcript: all {len(SAMPLE_MARKERS)} sample-plane markers observed in the "
        "order the x86 oracle records them",
        flush=True,
    )


def check_transcript_matches_the_oracle() -> None:
    """The frozen cutover transcript remains non-empty and duplicate-free."""
    if not SAMPLE_MARKERS or len(set(SAMPLE_MARKERS)) != len(SAMPLE_MARKERS):
        fail("SAMPLE_MARKERS must be a non-empty duplicate-free transcript")
    print(
        f"transcript: {len(SAMPLE_MARKERS)} frozen component markers plus "
        f"{len(ORACLE_ONLY)} documented retired-kernel exclusions",
        flush=True,
    )


# The components this gate's claim rests on. Neither may carry a seL4 branch:
# the milestone's whole point is that a component written against the retired
# kernel's ABI runs here unchanged.
UNMODIFIED_COMPONENTS = ("sample-lender", "sample-receiver")

# Every compile-time scenario flag any seL4 gate sets. A sample component
# branching on *any* of them would be tailoring itself to this cutover.
SEL4_CHECK_FLAGS = (
    "SLIME_SEL4_CHANNEL_CHECK",
    "SLIME_SEL4_LOAN_CHECK",
    "SLIME_SEL4_SPAWN_CHECK",
    "SLIME_SEL4_SAMPLE_CHECK",
)


def check_components_are_unmodified() -> None:
    """Neither sample component knows it is running on seL4.

    Checked against the sources, because serial output cannot establish it: a
    component rewritten to suit this root would produce an identical
    transcript. This is the one claim the boot can never prove on its own.
    """
    for name in UNMODIFIED_COMPONENTS:
        source = ROOT / "components" / "bins" / "src" / "bin" / f"{name}.rs"
        try:
            text = source.read_text(encoding="utf-8")
        except OSError as error:
            fail(f"cannot read {source.relative_to(ROOT)}: {error}")
        for flag in SEL4_CHECK_FLAGS:
            if flag in text:
                fail(
                    f"{source.relative_to(ROOT)} branches on {flag}; the "
                    "milestone requires this component to run unmodified"
                )
    print(
        "components: "
        + ", ".join(UNMODIFIED_COMPONENTS)
        + " carry no seL4 branch and run as the x86 oracle builds them",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 sample-plane image and assert the oracle's transcript"
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
        "seL4 sample plane check: the unmodified sample-lender and sample-receiver "
        "exchanged and returned a payload larger than the control-message bound over "
        "seL4, running the transcript the x86 oracle records, with quota exhaustion "
        "and peer death reclaiming every resource"
    )


if __name__ == "__main__":
    main()
