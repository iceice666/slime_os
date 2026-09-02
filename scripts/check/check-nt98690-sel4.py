#!/usr/bin/env python3
"""P6.B: boot seL4 and slime-root on the named Novatek NT98690 H1V1, three times.

The board's firmware handoff is P6.A's, unchanged: the vendor U-Boot, driven
over UART0, loads a flat arm64 `Image` from the SD card and starts it with
`booti`. What it starts here is the seL4 kernel loader carrying the kernel,
the root task, and the sample-plane generation, wrapped by
`scripts/build/build-nt98690-payload.py --sel4` in the same header the probe
qualified. The evidence required is the evidence the reference planes
require -- the root's allocator, its timer before and after the component
graph starts, the admitted generation, the graph, and READY naming this
board's target profile -- plus three things only a board can show: that
every run produced the same normalized semantic trace, that the wire carried
it without framing errors, and that the board returned to its own firmware
each time without an operator, because the root resets it through the SoC
watchdog when the plane completes.

This is its own checker rather than a mode of `check-nt98690-boot.py`
because it is a different marker contract: the shared tamper control owns
one contract per module and pins its count, and the P6.A probe's contract is
pinned at its own. Everything below the contract is borrowed -- the console,
the staging sequence, the banner recovery, and the sample plane's own
transcript assertions -- and nothing is duplicated.

The operator power-cycles once, when told. Runs two and three begin while
the board is already rebooting from the previous run's watchdog reset; the
carriage-return spam that wins the `bootdelay=0` race catches the prompt of
that reboot, and the text collected on the way there -- the firmware banner
-- is the previous run's recovery evidence. The last run's recovery is
watched in silence, so it is observed without a keystroke and the board is
left booting its vendor Linux, as P6.A leaves it.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from harness import load_script  # noqa: E402
from sel4_gate_markers import chains_from_gate, match_marker_contract  # noqa: E402
from uboot_console import Console, reach_uboot, report_transcript, send_command  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PLATFORM = "ns02201-h1v1"
TARGET_PROFILE = "aarch64-sel4-nt98690-h1v1"
SAMPLE_STEM = "slime-sel4-sample-ns02201-h1v1"
IMAGE_BUILDER = ROOT / "scripts" / "build" / "build-sel4.py"
PAYLOAD_BUILDER = ROOT / "scripts" / "build" / "build-nt98690-payload.py"
IMAGE = ROOT / "build" / f"{SAMPLE_STEM}.elf"
IMAGE_IDENTITY = ROOT / "build" / f"{SAMPLE_STEM}.identity.json"
OUT_DIR = ROOT / "build" / "nt98690-payload"
PAYLOAD = OUT_DIR / f"{SAMPLE_STEM}.bin"
PAYLOAD_IDENTITY = OUT_DIR / f"{SAMPLE_STEM}.identity.json"
EVIDENCE_DIR = ROOT / "build" / "nt98690-sel4-evidence"

RUNS = 3
#: From `booti` to the root's reset request. The Duo's seL4 boots take about
#: fifteen seconds; the budget covers a 1.2 MiB image over a 115200 line
#: plus the kernel's own boot printing.
BOOT_TIMEOUT_SECONDS = 180.0

#: The root's last line: it has asked the watchdog to reset the SoC.
RESET_MARKER = r"SLIME_NT98690 reset request kind=wdt"

#: Ordered. The first five are the P6.A handoff; the next three are the
#: loader's and kernel's own; the rest are the root's, each also required of
#: the reference planes except the two that name this board. Every regex is
#: instantiable by `check-sel4-gate-controls.py::literal_for`, so run-varying
#: values are matched by shape here and normalized before runs are compared.
REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    ("U-Boot selected the card slot", r"is current device"),
    ("the address U-Boot will pass as the device tree holds one", r"edfe0dd0"),
    ("fatload read the seL4 image off the card", r"\d+ bytes read in \d+ ms"),
    ("U-Boot accepted and relocated the device-tree argument", r"Loading Device Tree to "),
    ("control left U-Boot for the loader", r"Starting kernel \.\.\."),
    ("the seL4 kernel loader ran on the board's console", r"Starting loader"),
    ("the loader handed over to the kernel", r"Entering kernel"),
    ("upstream seL4 reached userspace", r"Booting all finished, dropped to user space"),
    (
        "the root's allocator admitted nonzero kernel resources",
        r"SLIME_ROOT allocator slots=[1-9]\d* untypeds=[1-9]\d* bytes=[1-9]\d*",
    ),
    (
        "the root acquired CNTP's PPI at the frequency the board reported",
        r"SLIME_TIMER acquired irq=30 freq_hz=12000000",
    ),
    ("the startup timer interrupt was delivered", r"SLIME_TIMER delivered badge=0x1 polls=\d+"),
    ("the startup timer expiry was serviced", r"SLIME_TIMER OK"),
    (
        "the board's own target generation was admitted",
        r"SLIME_ROOT generation admitted number=\d+ executables=4 instances=4 grants=6 ",
    ),
    ("the component graph activated", r"SLIME_GRAPH activated instances=2"),
    (
        "timer delivery remained live after graph activation",
        r"SLIME_TIMER phase=post-graph-start delivered badge=0x1 polls=\d+",
    ),
    ("the post-activation timer expiry was serviced", r"SLIME_TIMER phase=post-graph-start OK"),
    (
        "the root reached ready naming this board's profile",
        rf"SLIME_ROOT READY target_profile={TARGET_PROFILE}",
    ),
    ("the root asked the watchdog to reset the board", RESET_MARKER),
    ("the board returned to its own firmware unattended", r"U-Boot 2021\.10"),
)

#: Any of these fails the run before ordered matching.
FAILURE_MARKERS: tuple[str, ...] = (
    r"Moving Image from",
    r"Bad Linux ARM64 Image magic!",
    r"SLIME_ROOT FATAL .*",
    r"SLIME_GRAPH FAIL .*",
    r"SLIME_TIMER FAIL .*",
    r"SLIME_NT98690 reset failed",
    r"KERNEL INVALID VECTOR ENTRY",
    r"Kernel init failed",
    r"seL4 called fail",
    r"Caught cap fault",
    r"Caught vm fault",
    r"Caught user exception",
    r"panicked at ",
    r"aborted at ",
)

#: Fields that legitimately differ between boots of the same image, collapsed
#: before traces are compared. The Duo's list, which a physical seL4 boot
#: shares: identities and counters, never markers.
DYNAMIC_FIELDS: tuple[tuple[re.Pattern[str], str], ...] = (
    (re.compile(r"generation=\d+"), "generation=<n>"),
    (re.compile(r"number=\d+"), "number=<n>"),
    (re.compile(r"task=\d+"), "task=<n>"),
    (re.compile(r"child=\d+"), "child=<n>"),
    (re.compile(r"id=\d+"), "id=<n>"),
    (re.compile(r"slot=\d+"), "slot=<n>"),
    (re.compile(r"polls=\d+"), "polls=<n>"),
    (re.compile(r"start=\d+ end=\d+ delta=\d+"), "start=<n> end=<n> delta=<n>"),
    (re.compile(r"instances=[0-9a-f]{16}"), "instances=<identity>"),
    (re.compile(r"\[[0-9a-f]{2}(?:, [0-9a-f]{2}){31}\]"), "<identity>"),
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"nt98690 sel4 check: {message}")


def build_artifacts() -> None:
    for command, what in (
        (
            [sys.executable, str(IMAGE_BUILDER), "--platform", PLATFORM, "--sample-plane", "--skip-pin-check"],
            "the H1V1 sample-plane seL4 image",
        ),
        ([sys.executable, str(PAYLOAD_BUILDER), "--sel4"], "the arm64 Image wrapping it"),
    ):
        completed = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
        if completed.returncode != 0:
            detail = (completed.stderr or completed.stdout).strip()
            fail(f"building {what} failed, so there is nothing to boot:\n{detail[-1500:]}")


def check_identity() -> tuple[bytes, dict[str, object]]:
    """The payload on disk is this build's, from this profile's image."""
    # The ELF itself is not needed here: the payload identity binds the ELF's
    # digest, so a board host carrying only the wrapped image and the two
    # identity files can verify what it is about to boot.
    for path in (IMAGE_IDENTITY, PAYLOAD, PAYLOAD_IDENTITY):
        if not path.is_file():
            fail(f"missing {path.relative_to(ROOT)}; build first or drop --no-build")
    image_identity = json.loads(IMAGE_IDENTITY.read_text(encoding="utf-8"))
    payload_identity = json.loads(PAYLOAD_IDENTITY.read_text(encoding="utf-8"))
    for identity, name in ((image_identity, "image"), (payload_identity, "payload")):
        if identity.get("target_profile") != TARGET_PROFILE:
            fail(f"the {name} identity names the wrong target profile")
        if identity.get("platform") != PLATFORM:
            fail(f"the {name} identity names the wrong platform")
        if identity.get("variant") != "sample":
            fail(f"the {name} identity is not the sample-plane variant")
    if payload_identity.get("elf_sha256") != image_identity["image"]["sha256"]:
        fail("the payload was wrapped from a different seL4 image than the one identified")
    payload = PAYLOAD.read_bytes()
    if hashlib.sha256(payload).hexdigest() != payload_identity["payload_sha256"]:
        fail("the payload on disk does not match its identity; rebuild rather than booting it")
    if payload_identity.get("boot_file") != PAYLOAD.name:
        fail("the payload identity names a different boot file")
    return payload, payload_identity


def stage_and_launch(
    console: Console,
    profile: dict[str, object],
    boot: object,
    payload: bytes,
    load: int,
) -> str:
    """From the prompt: probe the slot, check the tree, load, verify, `booti`.

    Exactly P6.A's sequence, with P6.A's helpers; the transcript begins here
    so the vendor's own autoboot noise is never part of what is scored.
    """
    prompt = str(profile["uboot_prompt"])
    transcript = send_command(console, str(profile["uboot_select_device"]), prompt, 15.0, fail)
    tree = send_command(console, "md.l ${fdtcontroladdr} 1", prompt, 10.0, fail)
    transcript += tree
    if "edfe0dd0" not in tree:
        fail(
            "${fdtcontroladdr} does not point at a device tree; `booti` panics on a "
            f"missing tree, so this run stops before spending a boot on it:\n{tree[-300:]}"
        )
    partition = str(profile["boot_partition"])
    loaded = send_command(
        console, f"fatload {partition} {load:#x} {PAYLOAD.name}", prompt, 120.0, fail
    )
    transcript += loaded
    match = re.search(r"(\d+) bytes read", loaded)
    if match is None:
        fail(f"`fatload` did not report a byte count; is {PAYLOAD.name} on the card?\n{loaded[-300:]}")
    if int(match.group(1)) != len(payload):
        fail(
            f"the board read {match.group(1)} bytes but the payload is {len(payload)}; "
            "the card is carrying a different build"
        )
    boot.check_deployed_bytes(console, prompt, load, payload)

    launch = str(profile["uboot_launch"]).replace("{load}", f"{load:#x}")
    print(f"[gate]   launching: {launch}")
    console.flush_input()
    console.write(launch.encode() + b"\r")

    output = ""
    remaining = BOOT_TIMEOUT_SECONDS
    stop = re.compile("|".join((RESET_MARKER, *FAILURE_MARKERS)))
    while remaining > 0:
        chunk = console.read_for(0.5)
        remaining -= 0.5
        output += chunk
        if stop.search(output):
            break
    return transcript + output


def banner_line(text: str, banner: str) -> str:
    """Up to and including the firmware banner's line, and nothing after it."""
    match = re.search(banner, text)
    if match is None:
        return ""
    line_end = text.find("\n", match.end())
    start = text.rfind("\n", 0, match.start()) + 1
    return text[start : line_end if line_end != -1 else len(text)]


def normalize(transcript: str) -> str:
    """Only the semantic lines, with the fields that vary per boot collapsed."""
    lines: list[str] = []
    for raw in transcript.splitlines():
        line = raw.strip()
        if not (
            line.startswith("SLIME_") or line.startswith("[init]") or line.startswith("[sample-")
        ):
            continue
        if re.fullmatch(RESET_MARKER, line):
            break
        for pattern, replacement in DYNAMIC_FIELDS:
            line = pattern.sub(replacement, line)
        lines.append(line)
    return "\n".join(lines)


def check_run(run: int, transcript: str) -> None:
    """The ordered contract and the failure markers, naming the run that failed."""

    def reject(message: str) -> NoReturn:
        fail(f"run {run}: {message}")

    match_marker_contract(
        transcript,
        chains_from_gate(sys.modules[__name__]),
        FAILURE_MARKERS,
        reject,
        before_reject=lambda: report_transcript(transcript),
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--serial",
        help="the board's console: a tty path such as /dev/ttyUSB0, or tcp:HOST:PORT for a bridge",
    )
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--evidence-dir", type=Path, default=EVIDENCE_DIR)
    arguments = parser.parse_args()

    boot = load_script("nt98690_boot_gate", "check/check-nt98690-boot.py")
    sample = load_script("nt98690_sample_gate", "check/check-sel4-sample-plane.py")
    profile = boot.load_profile()
    prompt = str(profile["uboot_prompt"])
    baud = int(str(profile["serial_baud"]))

    if arguments.serial is None:
        fail(
            "no serial endpoint given, so no board evidence can be observed; "
            f"P6.B requires {RUNS} seL4 boots on the named Novatek NT98690 H1V1. "
            "Pass --serial /dev/ttyUSB0, or --serial tcp:HOST:PORT for a bridge"
        )

    if not arguments.no_build:
        build_artifacts()
    payload, identity = check_identity()
    load = int(str(identity["load_address"]), 16)
    print(f"[gate]   payload {PAYLOAD.name}, {len(payload)} bytes, sha {identity['payload_sha256'][:16]}…")
    print(f"[gate]   generation {identity['generation_identity'][:16]}…, load {load:#x}")

    console = Console(arguments.serial, baud, fail)
    print(f"[gate]   console {console.describe()}")
    print(
        f"[gate]   the card must carry {PAYLOAD.name} at its root, and SW18 must be "
        f"{profile['sw18_boot_position']} (never the loader's rescue position)"
    )
    arguments.evidence_dir.mkdir(parents=True, exist_ok=True)
    normalized: list[str] = []
    framing_total = 0
    transcript = ""
    try:
        reach_uboot(console, prompt, boot.PROMPT_WINDOW_SECONDS, fail)
        for run in range(1, RUNS + 1):
            print(f"[gate]   run {run} of {RUNS}")
            transcript = stage_and_launch(console, profile, boot, payload, load)
            if run < RUNS:
                # The board is resetting itself; catching the next prompt both
                # proves it came back and stages the next run.
                recovery = reach_uboot(console, prompt, boot.PROMPT_WINDOW_SECONDS, fail)
            else:
                print(f"[gate]   waiting up to {boot.RECOVERY_SECONDS:.0f}s for the vendor firmware to return")
                recovery = boot.wait_for_banner(console, boot.RECOVERY_SECONDS)
            transcript += "\n" + banner_line(recovery, boot.BANNER_PATTERN)
            (arguments.evidence_dir / f"sample-run-{run}.log").write_text(transcript, encoding="utf-8")

            check_run(run, transcript)
            sample.check_transcript(transcript)
            if console.framing_errors:
                fail(f"run {run}: {console.framing_errors} framing errors on the wire")
            framing_total += console.framing_errors or 0
            trace = normalize(transcript)
            (arguments.evidence_dir / f"sample-run-{run}.normalized.log").write_text(
                trace, encoding="utf-8"
            )
            normalized.append(trace)
            print(f"[gate]   run {run}: contract, sample plane, and wire all clean")
    finally:
        console.close()
        if transcript:
            print(f"[gate]   evidence in {arguments.evidence_dir}")

    if any(trace != normalized[0] for trace in normalized[1:]):
        fail(f"the {RUNS} physical sample runs produced different normalized semantic traces")

    framing = (
        "framing errors unobservable over a TCP bridge"
        if console.framing_errors is None
        else f"{framing_total} framing errors"
    )
    print(
        f"nt98690 sel4 check: PASS on the named {profile['board']}: {RUNS} seL4 boots of "
        f"{TARGET_PROFILE} with identical normalized semantic traces, {framing}, and the "
        "board's own return to its firmware after each"
    )


if __name__ == "__main__":
    main()
