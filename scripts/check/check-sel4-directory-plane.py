#!/usr/bin/env python3

"""P5.4.3 gate: M6.3's directory capability mechanism (M6.3).

M6.3 is split, and this gate covers the half the *root* owns. What a directory
contains — entries, names, object identities — is a filesystem component's
business over the object store. What the root owns is the part that has to be
unforgeable: a shared namespace root, scoped views that derivation may only
narrow, and an atomic compare-and-swap commit.

The probe holds one unscoped directory capability and proves each property by
being refused where it should be:

* a stale commit is refused and leaves the live root untouched;
* scopes compose forward and cannot escape — `..`, an absolute path, a trailing
  slash, and an empty segment are all rejected by the validator, so there is no
  request that walks a scope outward;
* derivation cannot widen rights, and needs `directoryDerive` specifically;
* a *scoped* writer cannot commit, so a subtree cannot replace the namespace;
* a reader cannot commit at all;
* and a commit through the unscoped view is visible through a scoped one, which
  is what makes a directory capability a view rather than a snapshot.

No disk. This plane is about capability state, not storage — which is itself
the finding: the oracle's directory operations touch no device, no physical
memory, and no privileged register.
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
IMAGE = ROOT / "build" / "slime-sel4-directory.elf"
FIXTURE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-directory.zti"
BOOT_TIMEOUT_SECONDS = 180

REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        "the unconfigured instance parked without a run token",
        r"\[sel4-directory-probe\] idle without a run token",
    ),
    (
        # The root places the declared view in the probe's own table, and
        # `construct_child` installs it again for the spawned copy.
        "the spawned instance received its declared directory authority",
        r"SLIME_GRAPH declared placed task=\d+ child=\d+ slot=\d+ kind=directory",
    ),
    (
        "init spawned the probe",
        r"\[init\] directory probe spawned",
    ),
    (
        # The boot seeds the namespace with the directory fixture's root, so
        # what the probe asserts is the shape: unscoped, and reported
        # consistently. Which identity is live belongs to the filesystem plane.
        "the granted view is unscoped over the namespace",
        r"\[sel4-directory-probe\] unscoped view of the namespace",
    ),
    (
        "inspecting with rights outside the directory set was refused",
        r"\[sel4-directory-probe\] inspect outside the rights set refused",
    ),
    (
        "a root committed through the unscoped writer is visible",
        r"\[sel4-directory-probe\] root committed and visible",
    ),
    (
        # The compare-and-swap. A writer building on a parent that no longer
        # exists would silently discard the other writer's work.
        "a commit against a stale expected root was refused",
        r"\[sel4-directory-probe\] stale commit refused",
    ),
    (
        "a narrower scoped view was derived",
        r"\[sel4-directory-probe\] derived a scoped view",
    ),
    (
        "scopes compose forward",
        r"\[sel4-directory-probe\] scopes compose forward",
    ),
    (
        # Syntactic, not a check the caller could phrase around: `..` is not a
        # segment the validator admits.
        "no path escapes a scope",
        r"\[sel4-directory-probe\] escaping paths refused",
    ),
    (
        "derivation cannot widen rights",
        r"\[sel4-directory-probe\] widening derivation refused",
    ),
    (
        "derivation requires the derive right specifically",
        r"\[sel4-directory-probe\] derivation without the right refused",
    ),
    (
        # A subtree cannot become the namespace.
        "a scoped writer cannot commit",
        r"\[sel4-directory-probe\] scoped commit refused",
    ),
    (
        "a reader cannot commit",
        r"\[sel4-directory-probe\] read-only commit refused",
    ),
    (
        "a commit through one view is visible through another",
        r"\[sel4-directory-probe\] the namespace is shared across views",
    ),
    (
        "the probe ran every arm and exited cleanly",
        r"\[sel4-directory-probe\] directory plane complete",
    ),
    (
        "init observed the clean exit",
        r"\[init\] directory plane complete",
    ),
)

TERMINAL_MARKER = r"\[init\] directory plane complete"

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_ROOT FAIL",
    r"SLIME_GRAPH FAIL",
    r"SLIME_GRAPH wedged waiter",
    r"\[init\] directory plane fail: .*",
    r"\[sel4-directory-probe\] fail: .*",
    r"Caught cap fault",
    r"Caught vm fault",
    r"Caught user exception",
    r"panicked at ",
    r"aborted at ",
    r"\(aborted\)",
)

def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 directory plane check: {message}")


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


def build_image() -> None:
    command = [sys.executable, str(BUILD_SCRIPT), "--directory-plane"]
    print(f"[build] {' '.join(command)}", flush=True)
    try:
        process = subprocess.run(command, cwd=ROOT, check=False)
    except OSError as error:
        fail(f"cannot run the seL4 image build: {error}")
    if process.returncode != 0:
        fail(f"seL4 image build failed with exit status {process.returncode}")




def boot(profile: dict[str, object]) -> str:
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
    failures = re.compile("|".join(FAILURE_MARKERS))
    terminal = re.compile(TERMINAL_MARKER)
    lines: list[str] = []
    reached = False
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
    watchdog = threading.Timer(BOOT_TIMEOUT_SECONDS, process.kill)
    watchdog.start()
    try:
        assert process.stdout is not None
        for line in process.stdout:
            lines.append(line.rstrip("\r\n"))
            if failures.search(line):
                break
            if terminal.search(line):
                reached = True
                break
    finally:
        watchdog.cancel()
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
    transcript = "\n".join(lines)
    if not reached:
        report_transcript(transcript)
        fail(f"boot exceeded {BOOT_TIMEOUT_SECONDS}s without completing the plane")
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
    for label, pattern in REQUIRED_MARKERS:
        match = re.compile(pattern).search(transcript, position)
        if match is None:
            report_transcript(transcript)
            if re.search(pattern, transcript) is not None:
                fail(f"marker out of order: {label} ({pattern})")
            fail(f"missing marker: {label} ({pattern})")
        position = match.end()
    completions = transcript.count("[sel4-directory-probe] directory plane complete")
    if completions != 1:
        report_transcript(transcript)
        fail(f"{completions} instances ran the scenario, expected 1")
    # The root's own records corroborate the component's claims. Two commits
    # succeed and no more: a mechanism that accepted the stale one would print
    # three, and the probe's own assertions could not tell the difference
    # between "refused" and "accepted but reported refused".
    committed = re.findall(
        r"SLIME_GRAPH directory committed task=\d+ namespace=(\d+) root=(\w+)", transcript
    )
    if len(committed) != 2:
        report_transcript(transcript)
        fail(f"the root recorded {len(committed)} commits, expected exactly 2")
    if len({namespace for namespace, _ in committed}) != 1:
        fail("the commits landed in different namespaces")
    if committed[0][1] == committed[1][1]:
        fail("both commits installed the same root")
    stale = re.findall(r"SLIME_GRAPH directory commit stale task=\d+", transcript)
    if len(stale) != 1:
        fail(f"the root recorded {len(stale)} stale commits, expected exactly 1")
    scoped = re.findall(
        r"SLIME_GRAPH directory commit refused task=\d+ slot=\d+ namespace=\d+ reason=scoped",
        transcript,
    )
    if not scoped:
        fail("no scoped commit was refused by the mechanism")
    # Derivations the mechanism actually performed, each naming its scope.
    derived = re.findall(
        r"SLIME_GRAPH directory derived task=\d+ from=\d+ to=\d+ namespace=\d+ scope=(\S+)",
        transcript,
    )
    for expected in ("docs", "docs/notes", "opaque"):
        if expected not in derived:
            fail(f"the mechanism never derived the scope {expected!r}; saw {derived}")
    print(
        f"transcript: {len(REQUIRED_MARKERS)} markers observed; the root recorded "
        f"2 commits, 1 stale refusal, {len(scoped)} scoped-commit refusals, and "
        f"derivations {derived}",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 directory-plane image and assert M6.3's mechanism"
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
    if not IMAGE.is_file():
        fail(f"missing packaged image {IMAGE.relative_to(ROOT)}")
    profile = pins["qemu_arm_virt"]
    assert isinstance(profile, dict)
    check_transcript(boot(profile))
    print(
        "seL4 directory plane check: a component held one unscoped directory "
        "capability, derived narrower views that cannot escape or widen, was "
        "refused a stale commit and a scoped one, and saw its commits through "
        "every view of the shared namespace"
    )


if __name__ == "__main__":
    main()
