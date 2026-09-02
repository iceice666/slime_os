#!/usr/bin/env python3
"""P6.C: drive the resident Slisp product on the named Novatek NT98690 H1V1.

The board's firmware handoff is P6.A's and the seL4 image shape is P6.B's;
what boots here is the resident product graph -- init, console,
spawn-service, and Slisp, with sysinfo and echo-agent in the spawn
catalogue -- built with the gate-only `0x1d` terminator compiled in. UART0
receive reaches the shell only through the root's declared `InputRead`
authority: the root maps the UART granule into its own address space, its
console dispatcher polls it, and Slisp reads input events from its declared
slot, seeing `WouldBlock` on an empty FIFO. The gate types a definition, an
arithmetic use of it, and a `sysinfo` spawn at the resident prompt, waits
for the dispatcher to cross the 32768-iteration checkpoint that bounded
planes may never reach, types again to prove the shell outlived it, and
then -- only after every assertion has passed -- sends the terminator, which
the root routes into the same watchdog reset P6.B proved. One scored boot,
one operator power-cycle, and the board ends the session back in its own
firmware.

This is its own checker because it is its own marker contract: the shared
tamper control owns one contract per module, and the P6.A and P6.B
contracts are pinned at their own counts. Unlike the Duo's P3.F gate, this
one declares `REQUIRED_MARKERS` in the `literal_for` vocabulary, so the
tamper control covers it. Multi-line session markers spell transcript
newlines as regex `\\n` and the contract is matched against a CR-stripped
view of the transcript; the raw evidence log keeps the wire's own bytes.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
import time
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from harness import load_script  # noqa: E402
from sel4_gate_markers import chains_from_gate, match_marker_contract  # noqa: E402
from uboot_console import Console, reach_uboot, report_transcript, send_command  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PLATFORM = "ns02201-h1v1"
TARGET_PROFILE = "aarch64-sel4-nt98690-h1v1"
STEM = "slime-sel4-ns02201-h1v1-test-terminator"
IMAGE_BUILDER = ROOT / "scripts" / "build" / "build-sel4.py"
PAYLOAD_BUILDER = ROOT / "scripts" / "build" / "build-nt98690-payload.py"
IMAGE = ROOT / "build" / f"slime-sel4-graph-{PLATFORM}-test-terminator.elf"
IMAGE_IDENTITY = ROOT / "build" / f"slime-sel4-graph-{PLATFORM}-test-terminator.identity.json"
OUT_DIR = ROOT / "build" / "nt98690-payload"
PAYLOAD = OUT_DIR / f"{STEM}.bin"
PAYLOAD_IDENTITY = OUT_DIR / f"{STEM}.identity.json"
EVIDENCE_DIR = ROOT / "build" / "nt98690-slisp-evidence"

#: From `booti` to the shell's first resident input wait.
BOOT_TIMEOUT_SECONDS = 180.0
#: The resident dispatcher's 32768-iteration checkpoint. The QEMU rehearsal
#: of this session crossed it 311s after boot on an emulated core; the board
#: has not been measured, so the budget is double that. A deadline is not a
#: marker: a slow pass is a pass.
RESIDENT_BOUNDARY_TIMEOUT_SECONDS = 600.0
#: From the terminator byte to the root's reset request.
RESET_TIMEOUT_SECONDS = 30.0
#: The gate-only byte the root intercepts before input dispatch; never part
#: of the ordinary product. `slime-root/src/main.rs` installs it only when
#: the image identity below says so.
TEST_TERMINATOR = b"\x1d"

#: The root's acknowledgement that the terminator byte arrived, and the same
#: reset request P6.B scored.
TERMINATOR_MARKER = r"SLIME_NT98690 test terminator accepted"
RESET_MARKER = r"SLIME_NT98690 reset request kind=wdt"

#: Ordered. The first five are the P6.A handoff and the next three the
#: loader's and kernel's own, exactly as P6.B; then the root's boot evidence
#: with the product input ready at the pinned UART0 address, the resident
#: graph's, the session's -- typed characters echoed by the shell and
#: answered -- the residency boundary, and the terminator's reset with the
#: firmware banner after it. Session markers cross a newline: the contract
#: is matched against a CR-stripped transcript, so they spell `\n`.
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
        "the root mapped UART0 for the declared input path",
        r"SLIME_ROOT product input ready uart=0x2f0130000",
    ),
    (
        "the root acquired CNTP's PPI at the frequency the board reported",
        r"SLIME_TIMER acquired irq=30 freq_hz=12000000",
    ),
    ("the startup timer interrupt was delivered", r"SLIME_TIMER delivered badge=0x1 polls=\d+"),
    ("the startup timer expiry was serviced", r"SLIME_TIMER OK"),
    (
        "the board's own product generation was admitted",
        r"SLIME_ROOT generation admitted number=1 executables=6 instances=6 grants=11 ",
    ),
    ("only init was root-activated", r"SLIME_GRAPH activated instances=1"),
    ("init began the declared graph", r"\[init\] launching component graph"),
    (
        "all four required residents were certified healthy",
        r"SLIME_GRAPH healthy generation=1 instances=[0-9a-f]{16} required=4 live=4 idle=4 failed=0",
    ),
    (
        "the root reached ready naming this board's profile",
        rf"SLIME_ROOT READY target_profile={TARGET_PROFILE}",
    ),
    ("init kept the product graph resident", r"\[init\] product services resident"),
    ("the product identified the Slisp shell", r"Slisp"),
    ("Slisp presented its prompt", r"slisp> "),
    ("empty UART0 RX became WouldBlock, once", r"\[slisp\] resident input wait"),
    ("the typed definition was echoed and bound", r"\(define answer 40\)\n=> 40"),
    ("the persistent binding evaluated", r"\(\+ answer 2\)\n=> 42"),
    ("Slisp requested sysinfo through spawn-service", r"sysinfo\n\[spawn-service\] request"),
    ("sysinfo ran through the declared launch profile", r"\[sysinfo\] spawned through profile"),
    ("sysinfo exited cleanly", r"SLIME_GRAPH component exit task=[1-9]\d* status=0"),
    (
        "spawn-service collected child supervision",
        r"SLIME_GRAPH supervision collected task=2 child=\d+ kind=0",
    ),
    ("Slisp reported the accepted spawn", r"=> spawned sysinfo"),
    (
        "the resident dispatcher crossed the former iteration ceiling",
        r"SLIME_GRAPH resident checkpoint live=4 iterations=32768",
    ),
    ("Slisp answered after the former ceiling", r"\(\+ answer 3\)\n=> 43"),
    ("the root accepted the gate-only terminator", TERMINATOR_MARKER),
    ("the root asked the watchdog to reset the board", RESET_MARKER),
    ("the board returned to its own firmware unattended", r"U-Boot 2021\.10"),
)

#: Any of these fails the session before ordered matching. P6.B's set, plus
#: the failures only a resident product can produce: a bounded plane's
#: exhaustion leaking in, the shell itself exiting (task 3 is Slisp's
#: declared spawn order), the scripted REPL's own farewell (this gate never
#: sends Escape), and the shell's input-error bailout.
FAILURE_MARKERS: tuple[str, ...] = (
    r"Moving Image from",
    r"Bad Linux ARM64 Image magic!",
    r"SLIME_ROOT FATAL .*",
    r"SLIME_GRAPH FAIL .*",
    r"SLIME_TIMER FAIL .*",
    r"SLIME_NT98690 reset failed",
    r"SLIME_GRAPH exhausted live=\d+ iterations=\d+ certified=1",
    r"SLIME_GRAPH component exit task=3 ",
    r"\[slisp\] repl done",
    r"! input",
    r"KERNEL INVALID VECTOR ENTRY",
    r"Kernel init failed",
    r"seL4 called fail",
    r"Caught cap fault",
    r"Caught vm fault",
    r"Caught user exception",
    r"panicked at ",
    r"aborted at ",
)

#: Typed exactly as the Duo session, in order; the third answer only after
#: the residency boundary.
SESSION_COMMANDS = ("(define answer 40)\n", "(+ answer 2)\n", "sysinfo\n")
SESSION_TERMINAL = r"=> spawned sysinfo"
BOUNDARY_MARKER = r"SLIME_GRAPH resident checkpoint live=4 iterations=32768"
BOUNDARY_COMMAND = "(+ answer 3)\n"
BOUNDARY_ANSWER = r"=> 43"
INPUT_WAIT_MARKER = r"\[slisp\] resident input wait"
HEALTHY_MARKER = r"SLIME_GRAPH healthy generation=1 instances=[0-9a-f]{16} required=4 live=4"


def fail(message: str) -> NoReturn:
    raise SystemExit(f"nt98690 slisp check: {message}")


def contract_view(transcript: str) -> str:
    """The transcript with carriage returns removed, for marker matching.

    The wire carries `\\r\\n` on firmware lines and the shell's own echo; the
    multi-line session markers spell plain `\\n` so the tamper control's
    `literal_for` can instantiate them. The raw evidence log is untouched.
    """
    return transcript.replace("\r", "")


def build_artifacts() -> None:
    for command, what in (
        (
            [
                sys.executable,
                str(IMAGE_BUILDER),
                "--platform",
                PLATFORM,
                "--component-graph",
                "--test-terminator",
                "--skip-pin-check",
            ],
            "the H1V1 resident product-graph seL4 image",
        ),
        (
            [
                sys.executable,
                str(PAYLOAD_BUILDER),
                "--sel4",
                "--image",
                str(IMAGE),
                "--output-stem",
                STEM,
            ],
            "the arm64 Image wrapping it",
        ),
    ):
        completed = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
        if completed.returncode != 0:
            detail = (completed.stderr or completed.stdout).strip()
            fail(f"building {what} failed, so there is nothing to boot:\n{detail[-1500:]}")


def check_identity() -> tuple[bytes, dict[str, object]]:
    """The payload on disk is this build's gate artifact, not a plane image."""
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
        if identity.get("variant") != "graph":
            fail(f"the {name} identity is not the resident product graph")
        if not identity.get("test_terminator", False):
            fail(f"the {name} identity lacks the explicit gate-only terminator")
    if not image_identity.get("component_graph"):
        fail("the image identity does not carry the component graph")
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
    return transcript


def read_until(console: Console, transcript: str, pattern: str, seconds: float, what: str) -> str:
    """Append serial output until `pattern` or a failure marker appears."""
    stop = re.compile("|".join((pattern, *FAILURE_MARKERS)))
    remaining = seconds
    while remaining > 0:
        chunk = console.read_for(0.5)
        remaining -= 0.5
        transcript += chunk
        if stop.search(contract_view(transcript)):
            return transcript
    report_transcript(transcript)
    fail(f"no {what} within {seconds:.0f}s")


def type_command(console: Console, transcript: str, command: str) -> str:
    """One command, character by character, the Duo session's pacing.

    Each byte is followed by a short read so the shell's echo lands in the
    transcript between keystrokes, and each command by a drain interval so
    one-time service startup lines cannot interleave the next command.
    """
    for character in command:
        console.write(character.encode())
        transcript += console.read_for(0.05)
    transcript += console.read_for(0.75)
    return transcript


def drive_session(
    console: Console,
    profile: dict[str, object],
    boot: object,
    payload: bytes,
    load: int,
) -> str:
    transcript = stage_and_launch(console, profile, boot, payload, load)
    transcript = read_until(
        console, transcript, INPUT_WAIT_MARKER, BOOT_TIMEOUT_SECONDS, "resident input wait"
    )
    view = contract_view(transcript)
    if len(re.findall(INPUT_WAIT_MARKER, view)) != 1:
        report_transcript(transcript)
        fail("the resident input wait was not reported exactly once before the session")

    time.sleep(0.5)
    for command in SESSION_COMMANDS:
        transcript = type_command(console, transcript, command)
    transcript = read_until(
        console, transcript, SESSION_TERMINAL, BOOT_TIMEOUT_SECONDS, "accepted sysinfo spawn"
    )
    transcript = read_until(
        console,
        transcript,
        BOUNDARY_MARKER,
        RESIDENT_BOUNDARY_TIMEOUT_SECONDS,
        "resident 32768-iteration checkpoint",
    )
    transcript = type_command(console, transcript, BOUNDARY_COMMAND)
    transcript = read_until(
        console, transcript, BOUNDARY_ANSWER, BOOT_TIMEOUT_SECONDS, "post-boundary answer"
    )

    view = contract_view(transcript)
    if len(re.findall(HEALTHY_MARKER, view)) != 1:
        report_transcript(transcript)
        fail("the graph was certified healthy more than once; a resident restarted")
    if console.framing_errors:
        fail(f"{console.framing_errors} framing errors on the wire before the terminator")

    # Only after every assertion has passed does the gate end the session.
    print("[gate]   session complete; sending the gate-only terminator")
    console.write(TEST_TERMINATOR)
    transcript = read_until(
        console, transcript, RESET_MARKER, RESET_TIMEOUT_SECONDS, "watchdog reset request"
    )
    print(f"[gate]   waiting up to {boot.RECOVERY_SECONDS:.0f}s for the vendor firmware to return")
    recovery = boot.wait_for_banner(console, boot.RECOVERY_SECONDS)
    banner = boot.BANNER_PATTERN
    match = re.search(banner, recovery)
    if match is None:
        report_transcript(transcript)
        fail("the vendor firmware banner did not return after the watchdog reset")
    line_end = recovery.find("\n", match.end())
    start = recovery.rfind("\n", 0, match.start()) + 1
    return transcript + "\n" + recovery[start : line_end if line_end != -1 else len(recovery)]


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
    profile = boot.load_profile()
    prompt = str(profile["uboot_prompt"])
    baud = int(str(profile["serial_baud"]))

    if arguments.serial is None:
        fail(
            "no serial endpoint given, so no board evidence can be observed; "
            "P6.C requires one interactive Slisp session on the named Novatek "
            "NT98690 H1V1. Pass --serial /dev/ttyUSB0, or --serial tcp:HOST:PORT"
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
    transcript = ""
    try:
        reach_uboot(console, prompt, boot.PROMPT_WINDOW_SECONDS, fail)
        transcript = drive_session(console, profile, boot, payload, load)
    finally:
        console.close()
        if transcript:
            (arguments.evidence_dir / "slisp-session.log").write_text(transcript, encoding="utf-8")
            print(f"[gate]   evidence in {arguments.evidence_dir}")

    match_marker_contract(
        contract_view(transcript),
        chains_from_gate(sys.modules[__name__]),
        FAILURE_MARKERS,
        fail,
        before_reject=lambda: report_transcript(transcript),
    )

    framing = (
        "framing errors unobservable over a TCP bridge"
        if console.framing_errors is None
        else f"{console.framing_errors} framing errors"
    )
    identities = {
        "board": profile["board"],
        "soc": profile["soc"],
        "serial": profile["serial"],
        "serial_baud": baud,
        "framing_errors": console.framing_errors,
        "transcript_sha256": hashlib.sha256(transcript.encode()).hexdigest(),
        "payload_sha256": identity["payload_sha256"],
        "elf_sha256": identity["elf_sha256"],
        "generation_identity": identity["generation_identity"],
        "generation_sha256": identity.get("generation_sha256"),
        "target_profile": TARGET_PROFILE,
    }
    (arguments.evidence_dir / "slisp-identities.json").write_text(
        json.dumps(identities, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(
        f"nt98690 slisp check: PASS on the named {profile['board']}: one resident Slisp "
        f"session of {TARGET_PROFILE} answered at the prompt, outlived the 32768-iteration "
        f"checkpoint, and returned to the vendor firmware only through the gate-only "
        f"terminator, with {framing}"
    )


if __name__ == "__main__":
    main()
