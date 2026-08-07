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

The three properties a sweep could plausibly break, in the order the transcript
shows them:

1. the loop crosses the bound at all -- 33 pairs minted and released, plus the
   four held across them (carrier, gate, in-flight, retained), for a boot total
   of 37;
2. a pair **held** across the crossing still carries afterwards (too aggressive);
3. an end parked in `Transit` across the crossing still resolves to its queue.

(3) is the one a predicate over live capability tables alone would break: a
capability mid-transfer is held by no table by construction, so a sweep reading
only `GraphTables` frees its channel and the eventual receiver lands an endpoint
naming a key the table no longer has -- B22's fix reintroducing B22, exactly the
shape `Transit::holds_supervision` exists to prevent for B16. Removing
`Transit::holds_endpoint` must fail this gate; that fault injection is recorded
in the devlog entry.

A ninth image beside the eight before it, on the same rule: each gate boots the
artifact it asserts about, so none invalidates another's evidence by being built
last.
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
IMAGE = ROOT / "build" / "slime-sel4-crossing.elf"
MANIFEST = ROOT / "build" / "slime-sel4-crossing.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
FIXTURE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-crossing.zti"
IMAGE_VARIANT = "crossing"

BOOT_TIMEOUT_SECONDS = 180

# `channel::MAX_CHANNELS`, read from the source rather than restated, so a
# change to the constant either updates this gate's arithmetic or fails it
# loudly instead of silently making the crossing vacuous.
CHANNEL_SOURCE = ROOT / "slime-root" / "src" / "channel.rs"
DRIVER_SOURCE = ROOT / "components" / "bins" / "src" / "bin" / "init.rs"

REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        # Two components declared: `init` and `crossing-peer`, the purpose-built
        # peer that holds a channel end in `Transit` across the whole loop.
        # `grants=2` is the executable grant plus the endpoint factory; every
        # channel this plane uses is minted at runtime through that factory
        # rather than declared, so the sweep sees only holder state.
        "the root admitted the crossing graph",
        r"SLIME_ROOT generation admitted number=1 components=2 grants=2 "
        r"health=2 kernel=1 bootstrap=1",
    ),
    (
        "the root launched both components from native ELFs and no legacy image",
        r"SLIME_ROOT graph admitted; legacy SLIMECM images not activated "
        r"components=2 slimecm=0 elf=2 unrecognized=0",
    ),
    (
        # Parked before the loop and collected only after it. From the transfer
        # until the matching recv the end is held by no capability table at all.
        "a channel end was parked in transit before the crossing",
        r"\[init\] channel end parked in transit",
    ),
    (
        "a channel pair was retained across the crossing",
        r"\[init\] channel pair retained",
    ),
    (
        # The sweep fired at least once, and reported what it collected. This is
        # the root's own line rather than the driver's, so a loop that somehow
        # completed without the table ever filling could not match it.
        "the root swept reclaimable channels when the table filled",
        r"SLIME_GRAPH channels swept freed=[1-9]\d* live=\d+ minted=\d+",
    ),
    (
        # The whole point: 33 pairs, one more than MAX_CHANNELS. Against the
        # unfixed root the 33rd `endpoint_create` is refused and the driver
        # exits through `crossing plane fail`, which is a failure marker.
        "the graph minted more channels over its lifetime than MAX_CHANNELS holds",
        r"\[init\] channel lifetime bound crossed",
    ),
    (
        "a pair held across the crossing still carried",
        r"\[init\] retained pair carried after crossing",
    ),
    (
        # Fault injection #2 targets exactly this: drop `Transit::holds_endpoint`
        # from the sweep predicate and this marker disappears while every marker
        # above it still passes.
        "an end parked in transit across the crossing still resolved",
        r"\[init\] transit end survived crossing",
    ),
    (
        "the crossing plane ran to completion",
        r"\[init\] crossing plane complete",
    ),
    (
        # The root's *own* accounting, and the numerically strongest evidence in
        # the gate: every marker above is a string the driver chose to print,
        # whereas `minted` is counted by `ChannelTable::push` inside the root.
        # B22's exit condition is "more than MAX_CHANNELS channels over its
        # lifetime", and MAX_CHANNELS is 32, so the pattern admits 33..=99 only.
        #
        # `queues=0` and `parked=0` on the same line are teardown completeness,
        # inherited from the sibling channel gates: no task is still blocked on
        # a reply and no queue still believes it has a live peer.
        #
        # Deliberately *not* read as evidence the sweep was non-destructive.
        # `live_queues()` counts queues whose peer is alive, and `mark_dead`
        # clears that flag for every channel a dying task held — so `queues=0`
        # is reached once every task has exited, whether or not anything was
        # swept and whether or not a sweep freed something it should not have.
        # What carries non-destructiveness is the pair of positive markers above
        # (`retained pair carried`, `transit end survived`) and fault injection 2.
        #
        # Terminal, and asserted last for a second reason: `MAX_GRAPH_ITERATIONS`
        # bounds the root's dispatch loop, and a graph that reached it would
        # drain incompletely and never print this line, so iteration exhaustion
        # cannot pass as success.
        "the root's own accounting recorded more channels minted than MAX_CHANNELS",
        # `(?!\d)` so the alternation really means 33..=99: unanchored,
        # `minted=330` shares the `33` prefix and would match.
        r"SLIME_GRAPH channels served sends=\d+ receives=\d+ parks=\d+ settled=\d+ "
        r"parked=0 queues=0 replies=\d+ minted=(?:3[3-9]|[4-9]\d)(?!\d)",
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
    # This is the first seL4 gate whose driver calls `cap_transfer` on the
    # critical path, so the root's own refusal marker matters here in a way it
    # does not for the others. `[init] crossing plane fail: parking a channel
    # end in transit` would catch the same failure, but this line names the
    # cause the root saw rather than the symptom the driver reported.
    r"SLIME_GRAPH capability transfer refused .*",
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
    check_loop_crosses_the_bound()
    check_keys_are_monotonic()


def source_constant(path: Path, pattern: str, description: str) -> int:
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as error:
        fail(f"cannot read {path.relative_to(ROOT)}: {error}")
    match = re.search(pattern, text)
    if match is None:
        fail(f"cannot find {description} in {path.relative_to(ROOT)}")
    return int(match.group(1))


def check_loop_crosses_the_bound() -> None:
    """The loop must mint strictly more pairs than the table holds at once.

    The transcript cannot show this: a loop of 32 would produce every marker
    above except the root's own `minted=` count, and that count is a regex
    written against today's constant. Reading both from source is what keeps the
    *loop length* non-vacuous if either moves -- raising `MAX_CHANNELS` without
    raising the loop would leave a gate that passes while proving nothing, which
    is the exact failure mode a hardcoded number invites.

    It says nothing about key derivation; `check_keys_are_monotonic` owns that.
    """
    bound = source_constant(
        CHANNEL_SOURCE,
        r"pub const MAX_CHANNELS: usize = (\d+);",
        "`MAX_CHANNELS`",
    )
    pairs = source_constant(
        DRIVER_SOURCE,
        r"const CHANNEL_LOOP_PAIRS: u32 = (\d+);",
        "`CHANNEL_LOOP_PAIRS`",
    )
    if pairs <= bound:
        fail(
            f"the crossing loop mints {pairs} pairs against MAX_CHANNELS={bound}; "
            "it must exceed the bound or the gate proves nothing"
        )
    print(
        f"source: the loop mints {pairs} pairs against MAX_CHANNELS={bound}, "
        "so the lifetime count crosses a bound the live count never reaches",
        flush=True,
    )


def check_keys_are_monotonic() -> None:
    """`push` must derive its key from a monotonic counter, not from `self.len`.

    The B22 fix rests on two invariants, and the transcript only shows one. The
    sweep is observed by the crossing itself; this one is not observable in any
    plane, and that is not an oversight in the driver -- it is a property of the
    defect. With four channels live across the sweep and every loop pair dropped
    before the next mint, a reverted `key = self.len` reissues keys that never
    collide with the four live ones, so this exact scenario passes either way.

    What it would break is a graph that holds a *high-keyed* channel across a
    sweep that frees a lower-numbered one: the next `push` then reissues a key
    some live capability already names, and `Resource::Endpoint { channel }` is
    the only handle a component holds -- so one component's sends land in
    another's queue. That is a confused deputy, strictly worse than the
    exhaustion the sweep removes, and it is exactly the failure a plane designed
    to exercise the sweep does not produce.

    So it is checked against the source, in the same idiom as the constants
    above, rather than left to a reviewer noticing. Following the repository's
    rule that each derived invariant carries its own observation.
    """
    try:
        text = CHANNEL_SOURCE.read_text(encoding="utf-8")
    except OSError as error:
        fail(f"cannot read {CHANNEL_SOURCE.relative_to(ROOT)}: {error}")
    # Comments are stripped first: the field's own doc-comment explains why the
    # old derivation was wrong, and naming `self.len` there must not read as
    # using it.
    code = "\n".join(
        line for line in text.splitlines() if not line.lstrip().startswith(("//", "//!"))
    )
    if "let key = self.next_key;" not in code:
        fail(
            f"{CHANNEL_SOURCE.relative_to(ROOT)}::push no longer derives its key from "
            "`next_key`; a key derived from `len` aliases a live channel as soon as "
            "the sweep frees one"
        )
    if "let key = self.len as ChannelKey;" in code:
        fail(
            f"{CHANNEL_SOURCE.relative_to(ROOT)}::push derives its key from `len`, "
            "which the sweep makes non-unique (B22)"
        )
    print(
        "source: channel keys come from a monotonic counter, so a reclaimed key "
        "names nothing rather than aliasing a live channel",
        flush=True,
    )


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
        "seL4 crossing plane check: a graph minted more channels over its lifetime "
        "than MAX_CHANNELS holds at once, and still sent and received on every live "
        "channel -- including one parked in transit across the crossing"
    )


if __name__ == "__main__":
    main()
