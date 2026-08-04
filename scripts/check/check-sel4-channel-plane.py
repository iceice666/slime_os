#!/usr/bin/env python3

"""P5.3.1 gate: two components exchange bounded messages over declared channels
on seL4.

Boots `build/slime-sel4-channel.elf` -- the image whose root task embeds the
channel-plane generation, `contracts/generation/v1/fixtures/sel4-channel.zti` --
and asserts ordered markers for each of P5.3.1's required checks:

1. every channel the generation's send/recv grants declare is materialized
   before any component runs, with each end installed at the slot that end's
   component addresses and with the rights that end actually holds;
2. a component blocked in `recv` is parked in the kernel and woken by its
   peer's send, receiving a payload too large for the fast message registers
   through its transfer window;
3. a bounded channel refuses a send past its depth, and a capability-carrying
   send is refused outright, both as ordinary Slime errors with the caller
   still running;
4. a `wait` on a source that is already ready is answered rather than parked;
5. every channel, held reply, and window is reclaimed when its components exit.

Modelled on `check-sel4-component-graph.py`, which guards P5.2 against a
different image. The three seL4 images are separate artifacts on purpose: each
gate boots the one it asserts about, so none invalidates another's evidence by
being built last. They differ only in which generation the root task embeds --
the root chooses its startup path by what the generation carries, not by a flag
it was built with.
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
IMAGE = ROOT / "build" / "slime-sel4-channel.elf"
MANIFEST = ROOT / "build" / "slime-sel4-channel.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
IMAGE_VARIANT = "channel"

BOOT_TIMEOUT_SECONDS = 120

# Depth of one directed logical channel, mirroring
# `slime-root/src/ipc.rs::CHANNEL_CAPACITY` and `init.rs::CHANNEL_DEPTH`. The
# queue-full arm asserts the exact count rather than "some refusal happened",
# because a channel that refused early would also produce a refusal.
CHANNEL_DEPTH = 16

# The bytes `init` sends to `console`. Over the sixteen the two inline payload
# registers carry, so the message must cross through the transfer window: a
# shorter line would ride in the fast registers and leave the whole staging
# path -- the root mapping a child's window frame at its scratch address --
# unexercised while still producing a delivered message.
PAYLOAD_BYTES = 42
INLINE_BYTES = 16

REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        "the channel generation was admitted",
        r"SLIME_ROOT generation admitted number=\d+ components=2 grants=2 ",
    ),
    (
        "both payloads are native ELF and no legacy image was activated",
        r"SLIME_ROOT graph admitted; legacy SLIMECM images not activated "
        r"components=2 slimecm=0 elf=2 unrecognized=0",
    ),
    ("console was staged", r"SLIME_GRAPH staged task=0 component=console grants=1 "),
    ("init was staged", r"SLIME_GRAPH staged task=1 component=init grants=1 "),
    (
        # Required check 1. The direction is the claim: the generation declares
        # `dango-output` with `console` as the grant's target and `recv` as its
        # right, so `console` is the consumer and `init` the producer.
        # Asserting the producer and consumer task ids is what makes this a
        # statement about direction rather than about a channel existing -- a
        # graph materialized backwards would still report one channel, and
        # would then deadlock with both ends waiting to receive.
        "the declared channel was materialized with init producing to console",
        r"SLIME_GRAPH channel grant=dango-output key=0 producer=1 consumer=0 queues=1",
    ),
    (
        # Each end holds only what that end can do. `init` holds send (0x1) at
        # the slot the boot layout numbers for it; `console` holds recv (0x2) at
        # slot 0, which is the slot `console.rs` compiles against.
        "init holds the send end at its layout slot",
        r"SLIME_GRAPH channel end task=1 slot=3 key=0 rights=0x1",
    ),
    (
        "console holds the receive end at slot 0",
        r"SLIME_GRAPH channel end task=0 slot=0 key=0 rights=0x2",
    ),
    (
        # A bidirectional grant whose two ends are the same component is a
        # loopback: the task sends to itself and receives what it sent, so it is
        # one queue, not two. Asserting `queues=1` here is what keeps that
        # honest -- allocating a second queue nothing could name would report
        # two and prove neither.
        #
        # The two-party bidirectional case (one channel, two directed queues,
        # one slot number at each end) is implemented but not exercised by this
        # graph; it arrives with the spawn-time capability distribution in
        # P5.3.3, which is what gives a second component a channel to reply on.
        "the bidirectional self-edge became one loopback queue",
        r"SLIME_GRAPH channel grant=service-spawn key=1 producer=1 consumer=1 queues=1",
    ),
    (
        "init holds both directions at one slot",
        r"SLIME_GRAPH channel end task=1 slot=7 key=1 rights=0x3",
    ),
    (
        # Every declared channel placed, nothing left unplaced. A generation
        # naming a channel the boot layout does not label would report it here
        # rather than installing it at a guessed slot; this graph has none.
        "every declared channel was placed before any component ran",
        r"SLIME_GRAPH channels grants=2 channels=2 queues=2 slots=3 unplaced=0",
    ),
    ("both components were activated", r"SLIME_GRAPH activated components=2"),
    (
        # Required check 2, first half. `console` reaches its `recv` on an empty
        # queue and is parked: the root holds its reply authority rather than
        # answering `ERR_WOULDBLOCK`, so the component is blocked in the kernel
        # rather than spinning. This marker appearing *before* the send below is
        # the whole claim -- a send that arrived first would be a fast-path
        # enqueue onto a queue nobody was waiting on, and the wake path would
        # never run.
        "console parked on an empty channel",
        r"SLIME_GRAPH parked task=0 channel=0 reason=recv",
    ),
    (
        # Required check 2, second half. The payload exceeds the inline
        # registers, so it crossed through the transfer window.
        "init sent a windowed payload to the parked receiver",
        rf"SLIME_GRAPH sent task=1 channel=0 bytes={PAYLOAD_BYTES} queued=1",
    ),
    ("init observed its own send succeed", r"\[init\] parked receiver sent"),
    (
        # Required check 3, first arm. This slice mediates no transferable
        # logical resource -- loans are P5.3.2 -- so a send naming a capability
        # is refused before the queue is touched.
        "a capability-carrying send was refused",
        r"SLIME_GRAPH capability transfer refused task=1 channel=0 caps=1",
    ),
    (
        "the refusal reached the component as an ordinary Slime error",
        r"\[init\] capability transfer denied",
    ),
    (
        # Required check 3, second arm. Deterministic because the channel is a
        # self-edge: nothing drains a queue whose only reader is the task
        # filling it, so the refusal lands on exactly the depth-plus-first send.
        "a full channel refused the send past its depth",
        r"\[init\] queue full refused",
    ),
    (
        # Required check 4. The queue filled above is non-empty, so the wait
        # must be answered rather than parked; parking here would deadlock a
        # single-threaded component against itself.
        "a wait on a ready source was answered rather than parked",
        r"\[init\] ready wait answered",
    ),
    (
        "the filled channel drained back through recv",
        r"\[init\] queue drained",
    ),
    (
        # The bytes themselves, written by `console` to the serial port. This is
        # the end-to-end claim: the exact payload init staged into its window
        # came back out of console's.
        "console printed the exact bytes it was sent",
        r"\[console\] channel plane carried this line",
    ),
    (
        # Console loops back and blocks again on an empty channel, so its reply
        # is owed at the moment its peer dies. This second park is what makes
        # the death-wake arm below observable rather than vacuous.
        "console parked again on an empty channel",
        r"SLIME_GRAPH parked task=0 channel=0 reason=recv",
    ),
    ("init completed the scenario", r"\[init\] channel plane complete"),
    ("init exited cleanly", r"SLIME_GRAPH component exit task=1 status=0"),
    (
        # Required check 5, first half. `woken=1` is the load-bearing number: it
        # says a component blocked in a call was answered by its peer's death
        # rather than left waiting for a message that can never arrive. Settling
        # the channels without waking anyone would drain the graph just as well
        # and say nothing about the component that was parked on it.
        "init's death settled both channels and woke the parked receiver",
        r"SLIME_GRAPH peer death task=1 channels=2 woken=1",
    ),
    ("console exited cleanly", r"SLIME_GRAPH component exit task=0 status=0"),
    (
        "the graph drained with every window and table reclaimed",
        r"SLIME_GRAPH served live=0 unsupported=0 unimplemented=0 buffers=0 "
        r"windows=0 tables=0",
    ),
    (
        # Required check 5, second half, and the terminal marker. `parked=0`
        # means no component is still blocked on a reply the root owes it, and
        # `queues=0` means no queue still believes it has a live peer -- either
        # would be a graph that only appeared to drain. The send and receive
        # counts are pinned exactly: one windowed message to console plus the
        # depth-many that filled the self-edge, and the depth-many drained back.
        "every channel and held reply was reclaimed",
        rf"SLIME_GRAPH channels served sends={CHANNEL_DEPTH + 1} "
        rf"receives={CHANNEL_DEPTH + 1} parks=2 settled=3 parked=0 queues=0 "
        r"replies=\d+",
    ),
)

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL .*",
    r"SLIME_GRAPH FAIL .*",
    # The scenario's own assertions. Every one of these means a channel
    # operation returned something other than what the plane promises, and the
    # component says so rather than exiting quietly.
    r"\[init\] channel plane fail: .*",
    # A component that could not bind its transfer window would issue no
    # windowed operation at all, and the graph would look quiet rather than
    # broken.
    r"\[slime-rt\] transfer window bind failed",
    r"SLIME_GRAPH window bind refused",
    # A park the root could not record leaves a caller blocked; it is answered
    # with a bounded error, but it is never expected on this path.
    r"SLIME_GRAPH park refused .*",
    # A channel the generation declared that the root could not place. This
    # graph declares none, so any occurrence means the fixture and the boot
    # layout have drifted apart.
    r"SLIME_GRAPH channel unplaced .*",
    r"SLIME_GRAPH service budget exhausted",
    # seL4's own complaints. `read-only endpoint` in particular means a
    # component cannot invoke the root at all, which is silent from the Slime
    # side: the component simply never speaks.
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
    raise SystemExit(f"seL4 channel plane check: {message}")


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
    command = [sys.executable, str(BUILD_SCRIPT), "--channel-plane"]
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
            "run `just sel4_channel_check`"
        )
    try:
        manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot parse {MANIFEST.relative_to(ROOT)}: {error}")
    if not isinstance(manifest, dict) or manifest.get("kind") != "slime-sel4-image-identity":
        fail(f"{MANIFEST.relative_to(ROOT)} is not a Slime seL4 identity manifest")
    # The three images are built from the same sources and differ only in which
    # generation the root task embeds, so booting the wrong one would fail on
    # markers rather than on identity. Checking the variant reports the actual
    # cause instead.
    if manifest.get("variant") != IMAGE_VARIANT:
        fail(
            f"{MANIFEST.relative_to(ROOT)} records variant "
            f"{manifest.get('variant')!r}, not {IMAGE_VARIANT!r}; "
            "rebuild with `--channel-plane`"
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


def check_payload_crosses_the_window() -> None:
    """The payload the scenario sends must exceed the inline registers.

    Asserted against the source rather than inferred from the transcript,
    because a payload that shrank below the inline bound would still be
    delivered and still print -- the gate would pass while the transfer-window
    staging path it exists to cover went unexercised.
    """
    if PAYLOAD_BYTES <= INLINE_BYTES:
        fail(
            f"the scenario's payload is {PAYLOAD_BYTES} bytes, which rides in the "
            f"{INLINE_BYTES} inline register bytes and never reaches the transfer window"
        )
    print(
        f"payload: {PAYLOAD_BYTES} bytes exceeds the {INLINE_BYTES} inline bytes "
        "and must cross the transfer window",
        flush=True,
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


def check_queue_depth(transcript: str) -> None:
    """The channel refused its send at exactly its declared depth.

    The ordered markers assert that a refusal happened; this asserts it happened
    in the right place. A channel that accepted one message and then refused
    would satisfy `[init] queue full refused` just as well, and would be a
    bounded channel of the wrong bound.
    """
    queued = re.findall(r"SLIME_GRAPH sent task=1 channel=1 bytes=\d+ queued=(\d+)", transcript)
    if not queued:
        fail("the transcript records no sends on the bounded channel")
    depth = max(int(value) for value in queued)
    if depth != CHANNEL_DEPTH:
        fail(
            f"the bounded channel accepted {depth} messages, not its declared "
            f"depth of {CHANNEL_DEPTH}"
        )
    if len(queued) != CHANNEL_DEPTH:
        fail(
            f"the bounded channel accepted {len(queued)} sends before refusing, "
            f"not {CHANNEL_DEPTH}"
        )
    print(
        f"bounded channel: accepted exactly {CHANNEL_DEPTH} messages and refused the next",
        flush=True,
    )


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
    check_queue_depth(transcript)


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 channel-plane image and assert ordered markers"
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
    check_payload_crosses_the_window()
    profile = pins["qemu_arm_virt"]
    assert isinstance(profile, dict)
    check_transcript(boot(profile))
    print(
        "seL4 channel plane check: two components exchanged bounded messages over "
        "declared channels, a parked receiver was woken by its peer's send, the "
        "queue-full and capability-transfer refusals were observed, and every "
        "channel, held reply, and window was reclaimed"
    )


if __name__ == "__main__":
    main()
