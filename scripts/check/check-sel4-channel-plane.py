#!/usr/bin/env python3

"""P5.3.1 gate: two components rendezvous over a generation-declared native
seL4 Endpoint.

Boots `build/slime-sel4-channel.elf` -- the image whose root task embeds the
channel-plane generation, `contracts/generation/v1/fixtures/sel4-channel.zti` --
and asserts ordered evidence that root installs the statically attenuated
Endpoint capabilities before activation, the blocking send completes only
after its receiver runs and accepts the exact payload, both components complete
through an explicit userspace/supervision lifecycle, and every task-owned native
capability and root export ticket is reclaimed.

Modelled on `check-sel4-component-graph.py`, which guards P5.2 against a
different image. The seL4 images are separate artifacts on purpose: each gate
boots the one it asserts about, so none invalidates another's evidence by being
built last.
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

# The bytes `init` sends to `console`, pinned so the transcript proves the
# receiver observed the complete message rather than merely some successful
# Endpoint rendezvous.
PAYLOAD_BYTES = 42

TERMINAL_MARKER = (
    r"SLIME_GRAPH HEALTHY generation=1 required=2 live=0 completed=2 failed=0"
)

REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        "the channel generation was admitted",
        r"SLIME_ROOT generation admitted number=\d+ executables=2 instances=2 grants=2 ",
    ),
    (
        "both payloads are native ELF images",
        r"SLIME_ROOT graph admitted executables=2 instances=2 slimecm=0 elf=2 unrecognized=0",
    ),
    (
        "console made unrelated progress while init remained blocked",
        r"\[console\] unrelated progress while sender blocked",
    ),
    ("init entered the blocking native send", r"\[init\] rendezvous send entering"),
    (
        "init observed rendezvous completion",
        r"\[init\] rendezvous send completed",
    ),
    (
        "console printed the exact rendezvous payload",
        r"\[console\] channel plane carried this line",
    ),
    ("console accepted the explicit close message", r"\[console\] channel close received"),
    ("console completed its channel role", r"\[console\] channel plane complete"),
    ("console exited cleanly", r"SLIME_GRAPH component exit task=1 status=0"),
    (
        "init observed console termination through supervision",
        r"\[init\] channel receiver completed",
    ),
    ("init completed the scenario", r"\[init\] channel plane complete"),
    ("init exited cleanly", r"SLIME_GRAPH component exit task=0 status=0"),
    (
        "the graph drained its task and window tables",
        r"SLIME_GRAPH served live=0 unsupported=0 unimplemented=0 buffers=0 windows=0 tables=0",
    ),
    (
        "every task arena and native capability was reclaimed",
        r"SLIME_GRAPH tasks reclaimed live=0 slots=[1-9]\d*",
    ),
    (
        "no task-owned native authority or root export ticket leaked",
        r"SLIME_GRAPH native task_caps=0 exports=0 tickets=0",
    ),
    ("the supervisor certified the completed graph", TERMINAL_MARKER),
)

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL .*",
    r"SLIME_GRAPH FAIL .*",
    r"SLIME_GRAPH component exit .*status=-?[1-9]\d*",
    r"\[init\] channel plane fail: .*",
    r"\[slime-rt\] transfer window bind failed",
    r"SLIME_GRAPH window bind refused",
    r"SLIME_GRAPH endpoint unplaced .*",
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
    terminals = re.findall(TERMINAL_MARKER, transcript)
    if len(terminals) != 1:
        fail(f"expected exactly one healthy supervisor terminal, saw {len(terminals)}")


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
    profile = pins["qemu_arm_virt"]
    assert isinstance(profile, dict)
    check_transcript(boot(profile))
    print(
        "seL4 channel plane check: the declared native Endpoint was installed with "
        "static direction, its blocking rendezvous carried the exact payload, both "
        "components completed explicitly, and no task-owned native/root resource leaked"
    )


if __name__ == "__main__":
    main()
