#!/usr/bin/env python3

"""P6.A: boot the probe on a named Novatek NT98690 H1V1 and read its evidence.

A new execution environment, which is why this is its own checker rather than a
case inside an existing one: an unmodified vendor U-Boot driven over a serial
console, a payload delivered on removable media, an AArch64 handoff at EL2, and
a console that may be a TCP bridge to another host. None of the seL4 plane
gates can reach any of that; what they share -- ordered marker matching and its
tamper control -- is imported rather than re-implemented.

What this gate proves, and the order it proves it in, matters more than the
individual assertions. Before spending a boot it establishes that the board is
at its prompt, that the SD slot answers, that the device tree U-Boot will pass
is really a device tree, and that the bytes on the card are the bytes that were
built. Only then does it launch, because a payload that prints nothing is
otherwise indistinguishable from a card that was never written.

Nothing here writes eMMC, and nothing here writes a block device at all: the
card is staged by the operator with the command the payload builder prints. The
board returns to its own firmware through the probe's PSCI reset, so a run
costs no physical intervention and a failed run costs a power cycle.

The board is physical, so its absence is a failure and never a skip. Without
`--serial` this exits non-zero, because a gate that passes with no board
attached is a gate that says nothing.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from arm64_image import parse_header  # noqa: E402
from sel4_gate_markers import chains_from_gate, match_marker_contract  # noqa: E402
from uboot_console import (  # noqa: E402
    Console,
    reach_uboot,
    report_transcript,
    send_command,
)

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
PINS_SECTION = "ns02201_h1v1"
BUILDER = ROOT / "scripts" / "build" / "build-nt98690-payload.py"
OUT_DIR = ROOT / "build" / "nt98690-payload"
IDENTITY = OUT_DIR / "identity.json"

PROMPT_WINDOW_SECONDS = 150.0
PAYLOAD_SECONDS = 30.0
RECOVERY_SECONDS = 90.0

#: The vendor U-Boot banner, which reappearing is how a completed PSCI reset is
#: observed. Pinned from `[ns02201_h1v1].uboot_version`.
BANNER_PATTERN = r"U-Boot 2021\.10"

#: Read-only questions for `--survey`, asked at the prompt before any scored
#: run. Each one settles something this gate would otherwise have to assume:
#: which slot the card is in (the eMMC answers as `mmc2`, so the SD is not
#: necessarily device 0), what `${fdtcontroladdr}` actually holds on a board
#: whose loader stages one tree at 0x100000 and whose U-Boot uses another, and
#: whether the prompt string in the pins is the real one. Nothing here writes:
#: no `saveenv`, no `mmc write`, no eMMC access at all.
SURVEY_COMMANDS: tuple[str, ...] = (
    "printenv fdtcontroladdr",
    "printenv bootcmd",
    "printenv bootdelay",
    "md.l ${fdtcontroladdr} 1",
    "mmc list",
    "mmc dev 0",
    "mmc dev 1",
    "fatls mmc 0:1",
    "bdinfo",
)

#: Ordered evidence for one probe run, from selecting the card through the
#: board's return to its own firmware.
#:
#: Every pattern here is instantiable by `check-sel4-gate-controls.py`'s
#: `literal_for`, which is what lets the shared tamper control prove this chain
#: rejects deleted, transposed, and failure-marked transcripts. That constrains
#: the regex vocabulary to literals, `\d+`, and counted character classes -- so
#: claims a regex cannot make ("this value is non-zero", "this one grew") are
#: made by the probe itself as literal `check ... = ok` lines, decided where the
#: values are. The hex readings below are measurements, reported for P6.B to
#: build on; the `check` lines are the contract.
REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        "U-Boot selected a card slot",
        r"is current device",
    ),
    (
        "the address U-Boot will pass as the device tree holds one",
        r"edfe0dd0",
    ),
    (
        "fatload read the probe off the card",
        r"\d+ bytes read in \d+ ms",
    ),
    (
        "U-Boot accepted and relocated the device-tree argument",
        r"Loading Device Tree to ",
    ),
    (
        "control left U-Boot for the payload",
        r"Starting kernel \.\.\.",
    ),
    (
        "the probe reached its entry point",
        r"=== SLIME_NT98690 probe: entry reached ===",
    ),
    (
        "the firmware handed over at EL2",
        r"SLIME_NT98690 el         = 0x0{15}2",
    ),
    (
        "booti placed the image at the pinned load address",
        r"SLIME_NT98690 base       = 0x0{8}10{7}",
    ),
    (
        "firmware passed a device-tree pointer in x0",
        r"SLIME_NT98690 x0         = 0x[0-9a-f]{16}",
    ),
    (
        "that pointer addresses a flattened device tree",
        r"SLIME_NT98690 fdt_magic  = 0x0{8}d00dfeed",
    ),
    (
        "the boot core is a Cortex-A73",
        r"SLIME_NT98690 midr_part  = 0x0{13}d09",
    ),
    (
        "the implemented physical-address range is the pinned 40-bit encoding",
        r"SLIME_NT98690 parange    = 0x0{15}2",
    ),
    (
        "the primary core's CNTFRQ_EL0 holds the pinned 12 MHz",
        r"SLIME_NT98690 cntfrq     = 0x0{10}b71b00",
    ),
    (
        "the counter's rate was estimated against the line rate",
        r"SLIME_NT98690 cnt_hz_est = 0x[0-9a-f]{16}",
    ),
    (
        "the interrupt controller answered above 4 GiB with the pinned identity",
        r"SLIME_NT98690 gicd_typer = 0x0{12}fc6a",
    ),
    (
        "its interrupt line count decoded to the pinned 352",
        r"SLIME_NT98690 gic_irqs   = 0x0{13}160",
    ),
    (
        "the image ran where it was linked",
        r"SLIME_NT98690 check placement   = ok",
    ),
    (
        "the exception level is the one the seL4 loader requires",
        r"SLIME_NT98690 check el2         = ok",
    ),
    (
        "the device-tree pointer verified",
        r"SLIME_NT98690 check fdt_magic   = ok",
    ),
    (
        "the payload was entered with the MMU off",
        r"SLIME_NT98690 check mmu_off     = ok",
    ),
    (
        "the counter advanced during the run",
        r"SLIME_NT98690 check cnt_advance = ok",
    ),
    (
        "the interrupt controller reported a plausible identity",
        r"SLIME_NT98690 check gicd        = ok",
    ),
    (
        "every check passed",
        r"SLIME_NT98690 PAYLOAD_OK",
    ),
    (
        "the probe asked firmware to reset the board",
        r"SLIME_NT98690 reset request kind=psci",
    ),
    (
        "the board returned to its own firmware unattended",
        BANNER_PATTERN,
    ),
)

#: Any of these in the transcript fails the run before ordered matching, so a
#: board that reached `PAYLOAD_OK` through a degraded path cannot pass.
#:
#: `Moving Image from` is the sharpest of them: this board's U-Boot prints it
#: only when it relocated the image away from where it was loaded, which is
#: exactly the placement contract P6.B's seL4 image inherits. Tolerating it here
#: would mean shipping that image to an address nothing verified.
FAILURE_MARKERS: tuple[str, ...] = (
    r"Moving Image from",
    r"Bad Linux ARM64 Image magic!",
    r"Wrong Image Format for booti command",
    r"Could not find a valid device tree",
    r"FDT and ATAGS support not compiled in",
    r"ERROR: can't get kernel image",
    r"MMC Device \d+ not found",
    r"Unable to read file",
    r'"Synchronous Abort" handler',
    r"Kernel panic",
    r"SLIME_NT98690 FAULT",
    r"SLIME_NT98690 PAYLOAD_FAIL",
    r"SLIME_NT98690 reset failed",
    r"= FAIL",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"nt98690 boot check: {message}")


def load_profile() -> dict[str, object]:
    if not PINS_PATH.is_file():
        fail(f"missing pins: {PINS_PATH.relative_to(ROOT)}")
    pins = tomllib.loads(PINS_PATH.read_text(encoding="utf-8"))
    profile = pins.get(PINS_SECTION)
    if not isinstance(profile, dict):
        fail(f"sel4/pins.toml has no [{PINS_SECTION}] table")
    for key in (
        "board",
        "serial_baud",
        "payload_load_address",
        "boot_partition",
        "boot_files",
        "uboot_prompt",
        "uboot_select_device",
        "uboot_launch",
        "sw18_boot_position",
    ):
        if key not in profile:
            fail(f"sel4/pins.toml [{PINS_SECTION}] must pin {key}")
    return profile


def build() -> None:
    completed = subprocess.run(
        [sys.executable, str(BUILDER)], cwd=ROOT, capture_output=True, text=True
    )
    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout).strip()
        fail(f"the payload build failed, so there is nothing to boot:\n{detail}")


def check_identity(profile: dict[str, object]) -> tuple[Path, bytes, dict[str, object]]:
    """The artifact on disk must be this build's, and agree with the pins."""
    if not IDENTITY.is_file():
        fail(f"missing {IDENTITY.relative_to(ROOT)}; run `just nt98690_payload_check`")
    identity = json.loads(IDENTITY.read_text(encoding="utf-8"))

    binary = OUT_DIR / identity["boot_file"]
    if not binary.is_file():
        fail(f"missing payload {binary.relative_to(ROOT)}")
    image = binary.read_bytes()

    digest = hashlib.sha256(image).hexdigest()
    if digest != identity["payload_sha256"]:
        fail(
            f"{binary.relative_to(ROOT)} does not match its identity manifest; "
            "rebuild rather than booting an artifact of unknown provenance"
        )

    load = int(str(profile["payload_load_address"]), 16)
    if int(str(identity["load_address"]), 16) != load:
        fail("the built payload's load address disagrees with the pinned one")
    if parse_header(image).text_offset != load:
        fail("the payload's Image header text_offset disagrees with the pinned load address")
    return binary, image, identity


def read_words(output: str, count: int) -> list[int]:
    """The 32-bit words in a `md.l` dump, in address order.

    U-Boot prints `<address>: <four words>  <ascii>`, so each line is split at
    the colon and only whole 8-digit tokens from the word column are taken --
    the trailing ASCII rendering can contain anything, including something that
    looks like a hex word.
    """
    words: list[int] = []
    for line in output.splitlines():
        head, separator, tail = line.strip().partition(":")
        if not separator or not re.fullmatch(r"[0-9a-f]+", head):
            continue
        for token in tail.split()[:4]:
            if re.fullmatch(r"[0-9a-f]{8}", token):
                words.append(int(token, 16))
    return words[:count]


def check_deployed_bytes(console: Console, prompt: str, load: int, image: bytes) -> None:
    """Compare what the board loaded against what was built.

    This U-Boot has no `crc32`, so a digest of the loaded span is not available
    and `md.l` at both ends of the image is what remains. It is a sampling
    rather than a proof, and it is here for one specific and common failure: a
    card carrying a *previous* build, which produces a transcript that looks
    almost right and wastes a bench session. The `fatload` byte count above
    already rules out a truncated read.
    """
    for label, offset in (("head", 0), ("tail", (len(image) // 64 - 1) * 64)):
        output = send_command(console, f"md.l {load + offset:#x} 0x10", prompt, 10.0, fail)
        words = read_words(output, 16)
        expected = [
            int.from_bytes(image[offset + n * 4 : offset + n * 4 + 4], "little") for n in range(16)
        ]
        if len(words) < 16:
            fail(
                f"could not read 16 words back from {load + offset:#x}; `md.l` printed:\n"
                f"{output[-400:]}"
            )
        if words != expected:
            fail(
                f"the {label} of the image in board memory does not match the built "
                "payload; the card is probably carrying an older build.\n"
                f"  expected {[f'{w:08x}' for w in expected[:4]]} ...\n"
                f"  board    {[f'{w:08x}' for w in words[:4]]} ..."
            )


def report_facts(transcript: str) -> None:
    """Print what the board measured. These are readings, not assertions.

    P6.A's job is to produce them; P6.B's is to build a kernel configuration
    from them. The one that decides something here is `cntfrq`, and it decides
    it for the next milestone rather than for this gate: a zero or implausible
    value means the seL4 platform's timer frequency has to come from a pin and
    an override rather than from the register, which is a design consequence and
    not a boot failure.
    """
    facts = dict(re.findall(r"^SLIME_NT98690 (\w+)\s+= (0x[0-9a-f]{16})", transcript, re.MULTILINE))
    if not facts:
        return
    print("--- board facts observed (inputs to P6.B) ---")
    for name, value in facts.items():
        print(f"  {name:<11} = {value}")

    cntfrq = int(facts.get("cntfrq", "0x0"), 16)
    estimate = int(facts.get("cnt_hz_est", "0x0"), 16)
    if cntfrq == 0:
        print(
            "  note: CNTFRQ_EL0 reads zero on the primary core, as this board's "
            "TF-A programs it on secondaries only. P6.B must pin the timer "
            f"frequency (the measured estimate is ~{estimate} Hz) rather than "
            "read it."
        )
    elif estimate and not 0.8 <= estimate / cntfrq <= 1.25:
        print(
            f"  note: CNTFRQ_EL0 says {cntfrq} Hz but the line-rate estimate is "
            f"~{estimate} Hz. One of them is wrong; P6.B must resolve this before "
            "pinning a timer frequency."
        )
    print("--- end board facts ---")


def survey(console: Console, prompt: str, timeout: float) -> str:
    """Ask the board the read-only questions, asserting nothing.

    The scored run needs a slot number, a device-tree address, and a prompt
    string that the vendor's autoboot transcript cannot supply, because it
    never stops at the prompt. Getting them wrong costs a power cycle each;
    asking for them costs one.
    """
    reach_uboot(console, prompt, min(timeout, PROMPT_WINDOW_SECONDS), fail)
    collected = ""
    for command in SURVEY_COMMANDS:
        print(f"[survey] {command}")
        output = send_command(console, command, prompt, 15.0, fail)
        collected += output
        # Drop the command's own echo by matching it, not by position: whether
        # a line comes back at all depends on the console's echo behaviour, and
        # skipping index 0 blindly eats the first line of the answer when it
        # does not.
        for line in output.replace("\r", "").splitlines():
            stripped = line.strip()
            if not stripped or stripped == prompt.strip():
                continue
            if stripped == command or stripped == f"{prompt}{command}".strip():
                continue
            print(f"    {line}")
    return collected


def monitor(console: Console, timeout: float) -> None:
    """Print whatever the board says, asserting nothing.

    Separating a link fault from an image fault is the first thing a bench
    session needs, and a gate that asserts cannot do it.
    """
    print(f"[monitor] reading {console.describe()} for up to {timeout:.0f}s; Ctrl-C to stop")
    seen = 0
    idle = 0.0
    while idle < 10.0 and seen < timeout:
        chunk = console.read_for(1.0)
        seen += 1
        if chunk:
            idle = 0.0
            sys.stdout.write(chunk)
            sys.stdout.flush()
        else:
            idle += 1.0
    print()
    if console.framing_errors is None:
        print("[monitor] framing errors unobservable over a TCP bridge")
    else:
        print(f"[monitor] framing errors: {console.framing_errors}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--serial",
        help=(
            "the board's console: a tty path such as /dev/ttyUSB0, or "
            "tcp:HOST:PORT for a socat/ser2net bridge when the board is on "
            "another machine"
        ),
    )
    parser.add_argument("--timeout", type=float, default=300.0)
    parser.add_argument("--monitor", action="store_true", help="print the console, assert nothing")
    parser.add_argument(
        "--survey",
        action="store_true",
        help="ask the board the read-only questions a scored run depends on",
    )
    parser.add_argument("--transcript", type=Path, help="write the captured transcript here")
    parser.add_argument("--no-build", action="store_true")
    arguments = parser.parse_args()

    profile = load_profile()
    baud = int(str(profile["serial_baud"]))

    if arguments.serial is None:
        fail(
            "no serial endpoint given, so no board evidence can be observed; "
            "P6.A requires an observed boot on the named Novatek NT98690 H1V1. "
            "Pass --serial /dev/ttyUSB0, or --serial tcp:HOST:PORT for a bridge"
        )

    console = Console(arguments.serial, baud, fail)
    # Accumulated outside the try so a failed run still writes what it saw. A
    # transcript is most valuable exactly when the gate rejects: that is the run
    # somebody has to diagnose, and on a physical board it cost a power cycle.
    transcript = ""
    try:
        if arguments.monitor:
            monitor(console, arguments.timeout)
            return

        if arguments.survey:
            transcript += survey(console, str(profile["uboot_prompt"]), arguments.timeout)
            return

        if not arguments.no_build:
            build()
        binary, image, identity = check_identity(profile)
        load = int(str(profile["payload_load_address"]), 16)
        prompt = str(profile["uboot_prompt"])

        print(f"[gate]   payload {binary.name}, {len(image)} bytes, sha {identity['payload_sha256'][:16]}…")
        print(f"[gate]   console {console.describe()}")
        print(
            f"[gate]   the card must carry {binary.name} at its root, and SW18 must be "
            f"{profile['sw18_boot_position']} (never the loader's rescue position)"
        )

        reach_uboot(console, prompt, min(arguments.timeout, PROMPT_WINDOW_SECONDS), fail)

        # Probe the slot before loading from it: `fatload` against an
        # un-probed card can hang this U-Boot outright, where `mmc dev` fails
        # fast and says so.
        transcript += send_command(console, str(profile["uboot_select_device"]), prompt, 15.0, fail)

        # Confirm the device tree exists before spending a boot on it. This
        # U-Boot panics rather than warns when `booti` is given no tree, and
        # `${fdtcontroladdr}` is supplied by the vendor loader rather than by us.
        tree = send_command(console, "md.l ${fdtcontroladdr} 1", prompt, 10.0, fail)
        transcript += tree
        # The word is the FDT's big-endian d00dfeed magic read back as a
        # little-endian long, so this is the tree's own header and not an
        # address that merely reads.
        if "edfe0dd0" not in tree:
            fail(
                "${fdtcontroladdr} does not point at a device tree — `md.l` read "
                f"no d00dfeed magic there. `booti` panics rather than warns on a "
                f"missing tree, so this run stops before spending a boot on it:\n{tree[-400:]}"
            )

        partition = str(profile["boot_partition"])
        loaded = send_command(
            console, f"fatload {partition} {load:#x} {binary.name}", prompt, 60.0, fail
        )
        transcript += loaded
        match = re.search(r"(\d+) bytes read", loaded)
        if match is None:
            fail(
                f"`fatload` did not report a byte count; is {binary.name} at the root "
                f"of the card in {partition}?\n{loaded[-400:]}"
            )
        if int(match.group(1)) != len(image):
            fail(
                f"the board read {match.group(1)} bytes but the payload is "
                f"{len(image)}; the card is carrying a different build"
            )

        check_deployed_bytes(console, prompt, load, image)

        launch = str(profile["uboot_launch"]).replace("{load}", f"{load:#x}")
        print(f"[gate]   launching: {launch}")
        console.flush_input()
        console.write(launch.encode() + b"\r")

        payload = ""
        deadline = PAYLOAD_SECONDS
        while deadline > 0:
            chunk = console.read_for(0.5)
            deadline -= 0.5
            payload += chunk
            if re.search(r"SLIME_NT98690 (reset request kind=psci|PAYLOAD_FAIL|FAULT)", payload):
                break
        transcript += payload

        # Say nothing from here on. The board is resetting into its own
        # firmware, and a keystroke would interrupt the autoboot this step
        # exists to observe -- leaving the board at a prompt and the recovery
        # unproven.
        print(f"[gate]   waiting up to {RECOVERY_SECONDS:.0f}s for the vendor firmware to return")
        recovery = ""
        remaining = RECOVERY_SECONDS
        while remaining > 0:
            recovery += console.read_for(1.0)
            remaining -= 1.0
            if re.search(BANNER_PATTERN, recovery):
                # Let the banner line arrive whole, then keep only up to its
                # end. What follows is the vendor's own next boot, which reaches
                # its kernel handoff about 700 characters later and prints
                # `Moving Image from` -- a failure marker about where *this*
                # gate's payload was placed, not the vendor's. Retained, a
                # correct autonomous recovery would reject the run it proves.
                recovery += console.read_for(1.0)
                end = re.search(BANNER_PATTERN, recovery).end()
                line_end = recovery.find("\n", end)
                recovery = recovery[: line_end if line_end != -1 else len(recovery)]
                break
        transcript += recovery
    finally:
        console.close()
        if arguments.transcript and transcript:
            arguments.transcript.parent.mkdir(parents=True, exist_ok=True)
            arguments.transcript.write_text(transcript, encoding="utf-8")
            print(f"[gate]   transcript written to {arguments.transcript}")

    report_facts(transcript)
    match_marker_contract(
        transcript,
        chains_from_gate(sys.modules[__name__]),
        FAILURE_MARKERS,
        fail,
        before_reject=lambda: report_transcript(transcript),
    )

    if console.framing_errors is None:
        print("[gate]   framing errors unobservable over a TCP bridge")
    elif console.framing_errors:
        fail(
            f"{console.framing_errors} framing errors on the wire; the transcript "
            "is not trustworthy evidence about this board"
        )
    else:
        print("[gate]   framing errors: 0")

    print(f"nt98690 boot check: PASS on the named {profile['board']}")


if __name__ == "__main__":
    main()
