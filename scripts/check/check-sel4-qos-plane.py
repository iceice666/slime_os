#!/usr/bin/env python3
"""P5.4.5 on seL4: the C8.5 QoS arms, observed on the `sel4-qos` plane.

The stream plane proves the typed fabric moves samples. This one proves the
*declared QoS policy* is enforced: a monotonic clock is granted to the graph, the
scenario advances it through scheduled boundaries, and each boundary must produce
its own event.

Why a separate image and gate. `sel4-stream.zti` grants no time capability, so its
simulated-time clause is structurally unreachable — the arms below cannot fire
there at all. `sel4-qos.zti` is that fixture plus a runtime-minted clock and a
`retained` diagnostics route, which is what makes an inline retained head exist
independently of publisher timing.

Marker chains rather than one global order: the participants provision
concurrently, so their arms interleave by scheduling. Each chain is internally
ordered because each is one causal sequence.
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
IMAGE = ROOT / "build" / "slime-sel4-qos.elf"
MANIFEST = ROOT / "build" / "slime-sel4-qos.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
FIXTURE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-qos.zti"
IMAGE_VARIANT = "qos"

BOOT_TIMEOUT_SECONDS = 240

CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "the QoS generation was admitted and its graph validated",
        (
            r"SLIME_ROOT generation admitted number=\d+ components=7 grants=\d+ ",
            r"SLIME_ROOT fabric graph=admitted schemas=2 routes=2 participants=6 ",
            r"SLIME_GRAPH activated components=7",
        ),
    ),
    (
        # Matching precedes the "all provisioned" line: the broker matches each
        # participant as it provisions, so the summary comes last.
        "every declared participant was matched, then the edge set was complete",
        (
            r"\[fabric\] QoS matched",
            r"\[fabric\] every declared stream edge provisioned",
        ),
    ),
    (
        "the scenario advanced the simulated clock",
        (
            r"\[fabric-publisher-b\] simulated time advanced",
        ),
    ),
    (
        "RELIABLE retry is accounted and then exhausted",
        (
            r"\[fabric\] reliable retry accounted",
            r"\[fabric\] QoS retry exhausted",
        ),
    ),
    (
        # One chain each: these are independent scheduled boundaries, and the
        # order they fire in is a function of the declared tuples rather than a
        # causal sequence. Grouping them would assert an ordering the contract
        # does not promise.
        "a lost liveliness lease fires",
        (r"\[fabric\] QoS liveliness lost",),
    ),
    (
        "a missed deadline fires",
        (r"\[fabric\] QoS deadline missed",),
    ),
    (
        "an expired lifespan fires",
        (r"\[fabric\] QoS lifespan expired",),
    ),
    (
        "a departed subscriber is retired through the peer-dead path",
        (
            r"\[fabric\] QoS peer dead",
        ),
    ),
    (
        # `served live=0` prints *after* init's terminal line, because the root
        # emits its accounting once the graph has drained and init has already
        # exited. So the drain evidence is the terminal marker here, and the
        # accounting line is asserted by `sel4_component_graph_check` instead.
        "the plane reached its terminal marker",
        (
            r"\[fabric\] stream plane complete",
            r"\[init\] fabric stream complete",
        ),
    ),
)

# The last marker of the last chain: `boot` stops reading once it appears, so the
# gate does not wait out the full timeout on a healthy plane.
TERMINAL_MARKER = CHAINS[-1][1][-1]

# B28: the plane exhausted `MAX_GRAPH_ITERATIONS` at 512 and reported no component
# failure at all, which is why the wedge marker is a failure marker here. Any
# reappearance is the same defect and must be red rather than a missing arm.
FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_ROOT FAIL",
    r"SLIME_GRAPH FAIL",
    r"SLIME_GRAPH wedged waiter",
    # Scoped to init's own failure, exactly as the stream gate scopes its own.
    # Six `fail:` lines are *expected* on this plane — every participant proves a
    # denial arm (`fail: request`, `fail: role reply`, `fail: route endpoints`),
    # and treating them as failures would make this gate red on a correct boot.
    r"\[init\] stream plane fail: .*",
    r"\[fabric\] no inline retained publisher",
    r"<<seL4\(CPU 0\) \[decodeInvocation",
    r"unhandled",
)

def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 QoS plane check: {message}")


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
    command = [sys.executable, str(BUILD_SCRIPT), "--stream-plane"]
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
            "run `just sel4_stream_check`"
        )
    try:
        manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot parse {MANIFEST.relative_to(ROOT)}: {error}")
    if not isinstance(manifest, dict) or manifest.get("kind") != "slime-sel4-image-identity":
        fail(f"{MANIFEST.relative_to(ROOT)} is not a Slime seL4 identity manifest")
    # The seven images are built from the same sources and differ only in which
    # generation the root task embeds, so booting the wrong one would fail on
    # markers rather than on identity. Checking the variant reports the actual
    # cause instead.
    if manifest.get("variant") != IMAGE_VARIANT:
        fail(
            f"{MANIFEST.relative_to(ROOT)} records variant "
            f"{manifest.get('variant')!r}, not {IMAGE_VARIANT!r}; "
            "rebuild with `--stream-plane`"
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
    terminal = re.compile(TERMINAL_MARKER)
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
    for label, chain in CHAINS:
        position = 0
        for pattern in chain:
            match = re.compile(pattern).search(transcript, position)
            if match is None:
                report_transcript(transcript)
                if re.search(pattern, transcript) is not None:
                    fail(f"{label}: marker out of order: {pattern}")
                fail(f"{label}: missing marker: {pattern}")
            position = match.end()
    print(
        f"transcript: {sum(len(chain) for _, chain in CHAINS)} markers observed "
        f"across {len(CHAINS)} causal chains",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 QoS-plane image and assert the C8.5 arms"
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
        fail(f"missing generation fixture {FIXTURE.relative_to(ROOT)}")
    pins = load_pins()
    if not arguments.no_build:
        build_image()
    check_manifest()
    profile = pins["qemu_arm_virt"]
    assert isinstance(profile, dict)
    check_transcript(boot(profile))
    print(
        "seL4 QoS plane check: C8.5's declared QoS policy is enforced on seL4 with "
        "every participant unmodified -- RELIABLE retry accounting and exhaustion, a "
        "missed deadline, an expired lifespan, a lost liveliness lease, and peer-dead "
        "retirement -- driven by a monotonic clock the generation grants, and the "
        "plane reached its terminal marker"
    )


if __name__ == "__main__":
    main()
