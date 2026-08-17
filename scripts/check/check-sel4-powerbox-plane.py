#!/usr/bin/env python3

"""P5.4.3 gate: M6.6's powerbox file dialog (M6.6).

A chooser holds directory authority the requester lacks. The requester holds one
RPC endpoint and nothing else — it verifies that first, by finding no directory
where its own manifest could have placed one — and the only way it can ever name
an object is for the chooser to mint a view and hand it over.

Four claims, and three of them are refusals:

* a selection gesture transfers exactly one capability, scoped to the selected
  object and carrying only the declared rights;
* a request for rights the chooser itself does not hold is denied, so the
  chooser cannot be used to launder authority upward;
* the transferred view cannot be derived past its scope;
* a cancellation mints nothing at all.

Both components are the oracle's, unmodified.
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

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from harness import profile_text, profile_integer  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
IMAGE = ROOT / "build" / "slime-sel4-powerbox.elf"
FIXTURE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-powerbox.zti"
BOOT_TIMEOUT_SECONDS = 180

REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        "init spawned the chooser",
        r"\[init\] powerbox chooser spawned",
    ),
    (
        "init spawned the probe",
        r"\[init\] powerbox probe spawned",
    ),
    (
        "the chooser is serving",
        r"\[powerbox\] chooser ready",
    ),
    (
        "the chooser prompted for a selection",
        r"\[powerbox\] request kind=file purpose=Open the selected note",
    ),
    (
        # The provenance event: which gesture, which object, which rights. A
        # mint with no record would be authority appearing from nowhere.
        "the selection produced a provenance record",
        r"\[powerbox-provenance\] event=\d+ gesture=select kind=file path=note "
        r"rights=0x[0-9a-f]+ purpose=Open the selected note",
    ),
    (
        # Exactly one capability, scoped to the selected object, carrying only
        # the rights the request declared — the probe checks the scope is
        # `note` and that `directoryWrite` is absent.
        "exactly one narrowed capability was transferred",
        r"\[powerbox-probe\] selected single object received",
    ),
    (
        # A request for more than the chooser holds. Denied by the chooser
        # rather than by the root, which is the point: a broker must not be a
        # path to authority its clients could not otherwise reach.
        "a request exceeding the chooser's own authority was denied",
        r"\[powerbox\] derive closure denied",
    ),
    (
        "the transferred view cannot be derived past its scope",
        r"\[powerbox-probe\] derive closure enforced",
    ),
    (
        "a cancelled selection minted nothing",
        r"\[powerbox-probe\] cancellation minted nothing",
    ),
    (
        "the probe ran every arm and exited cleanly",
        r"\[powerbox-probe\] done",
    ),
    (
        "init observed both clean exits",
        r"\[init\] powerbox plane complete",
    ),
)

TERMINAL_MARKER = r"\[init\] powerbox plane complete"

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_ROOT FAIL",
    r"SLIME_GRAPH FAIL",
    r"SLIME_GRAPH wedged waiter",
    r"\[init\] powerbox plane fail: .*",
    r"Caught cap fault",
    r"Caught vm fault",
    r"Caught user exception",
    r"panicked at ",
    r"aborted at ",
    r"\(aborted\)",
)

def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 powerbox plane check: {message}")


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
    command = [sys.executable, str(BUILD_SCRIPT), "--powerbox-plane"]
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
    # Asserted by count, not by order: the root launches an unconfigured copy of
    # every declared component, and that copy reaches this arm before init has
    # spawned anything. The claim is that the requester holds no directory —
    # true of both instances, which is why it is the arm's whole point.
    if "[powerbox-probe] manifest directory absent" not in transcript:
        report_transcript(transcript)
        fail("the requester was not confirmed to hold no directory of its own")
    # *Which* layer refused the derive, named rather than inferred.
    #
    # The probe requires exactly `ERR_BAD_CAP` from `directory_derive`, and two
    # unrelated refusals both answer -1: the directory mechanism finding no
    # capability at the slot, and the root's declared-service gate finding the
    # instance never declared the directory mechanism at all. The probe cannot
    # tell them apart — `IpcError::BadCapability` is one variant for all three
    # capability failures by design, so a component cannot map its own table by
    # watching which error a probe returns. The root's own marker can, so the
    # gate reads it here.
    #
    # A requester holding one RPC endpoint declares no directory-kind
    # capability, so its service set carries no directory mechanism and this is
    # the refusal that fires. Label 15 is `directory_labels::DERIVE`. Asserting
    # it keeps the arm from passing on an unrelated -1 — an absent transfer
    # window, a malformed request — that would look identical to the probe.
    if not re.search(
        r"SLIME_GRAPH service refused task=\d+ label=15 class=undeclared", transcript
    ):
        report_transcript(transcript)
        fail(
            "the requester's derive attempt was not refused as an undeclared "
            "directory mechanism; some other -1 satisfied the probe's assertion"
        )
    completions = transcript.count("[powerbox-probe] done")
    if completions != 1:
        report_transcript(transcript)
        fail(f"{completions} requesters completed the scenario, expected 1")
    # Exactly one capability crossed, and it crossed once.
    #
    # Three requests are made — select, widen, cancel — and only the first may
    # carry a capability. A chooser that answered the widening request with a
    # mint, or minted on cancellation, shows more than one here; the probe's own
    # assertions could not distinguish "denied" from "granted and discarded".
    #
    # Counted at the *import*, which is where a directory now crosses. It is a
    # root-owned logical capability with no kernel object, so it never rides in
    # the message: the chooser exports and the probe claims. The retired
    # `capability transfer … channel=N` marker named the logical channel table
    # this cutover deleted, so a pattern still expecting it matches nothing —
    # and a count of nothing is not evidence that nothing crossed.
    imports = re.findall(
        r"SLIME_GRAPH capability imported task=\d+ id=\d+ kind=(\w[\w-]*)",
        transcript,
    )
    if imports != ["directory"]:
        report_transcript(transcript)
        fail(
            f"expected exactly one directory capability to cross the powerbox "
            f"channel; saw {imports}"
        )
    # The provenance record's rights must be the ones the probe accepted, not a
    # wider set the chooser happened to hold.
    granted = re.search(r"\[powerbox-provenance\][^\n]*rights=0x([0-9a-f]+)", transcript)
    if granted is None:
        fail("the provenance record carries no rights")
    rights = int(granted.group(1), 16)
    # `directoryWrite` is 1 << 20. The chooser holds read and derive only, so a
    # record naming write would mean it minted authority it never had.
    if rights & (1 << 20):
        fail(f"the chooser granted directoryWrite it does not hold: {rights:#x}")
    print(
        f"transcript: {len(REQUIRED_MARKERS)} markers observed; exactly one "
        f"capability crossed on selection with rights {rights:#x}, a widening "
        "request was denied, and a cancellation minted nothing",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 powerbox-plane image and assert M6.6"
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
        "seL4 powerbox plane check: a selection gesture granted one otherwise "
        "unreachable object capability, scoped and narrowed, with a provenance "
        "record; a widening request was denied, the transferred view could not "
        "be derived past its scope, and a cancellation minted nothing"
    )


if __name__ == "__main__":
    main()
