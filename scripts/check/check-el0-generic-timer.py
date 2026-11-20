#!/usr/bin/env python3

"""No EL0 program of a Raspberry Pi 5 system-timer image names a generic-timer register.

The image builder takes `--system-timer` for `bcm2712-rpi5`. The root of that
image counts the SoC's system timer, and its components ask the root for the
clock. This gate builds the image and reads every EL0 program in it:

* the root ELF and the root child ELF that the image identity names, each
  held to the SHA-256 the identity records;
* the component image of every executable of the generation the image
  embeds, found by the SHA-256 the identity records for it.

The product generation holds no component that reads the clock. The gate
therefore also builds the composition `sel4-clock-authority` for the same
target profile, with `SLIME_SYSTEM_TIMER=1` as the builder sets it, and reads
its executables. `clock-authority-probe` reads the clock twice.

Every program must hold no access (`scripts/lib/el0_timer_audit.py`).

The controls come from the same two builds without the switch. The root must
then write `CNTP_CTL_EL0` and read `CNTPCT_EL0`, and `clock-authority-probe`
must read `CNTPCT_EL0`. An audit that finds nothing in those builds either
cannot tell the two apart. The synthetic controls of the reader run first.

The switch has one source. The builder must refuse `--system-timer` for
another platform and refuse the variable in its own environment, and the two
images must have identities of their own: an image that counts the system
timer under the default image's name would reach the board unnoticed.

The gate reads code and boots nothing. The kernel and the loader run above
EL0 and use these registers, so the gate does not read them. Whether the
system timer works on the board is an operator's observation; no gate here
makes it.
"""

from __future__ import annotations

import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import hashlib
import json
import os
import re
import subprocess
import sys
from typing import NoReturn

import devloop_observations
import el0_timer_audit
from harness import ROOT

PLATFORM = "bcm2712-rpi5"
TARGET_PROFILE = "aarch64-rpi5"
PREFIX = ROOT / "build" / "sel4-rpi5-prefix"
SWITCH = "SLIME_SYSTEM_TIMER"
CLOCK_MANIFEST = "sel4-clock-authority"
CLOCK_READER = "clock-authority-probe"
WORK = ROOT / "build" / "el0-generic-timer"
WROTE = re.compile(r"^seL4 image build: wrote \S+ and (\S+\.identity\.json)$", re.MULTILINE)
# The EL0 programs among the ELFs an image identity names.
EL0_ELFS = ("root", "child")


def fail(message: str) -> NoReturn:
    raise SystemExit(f"EL0 generic-timer check: {message}")


def run(command: list[str], environment: dict[str, str], what: str) -> str:
    process = subprocess.run(command, cwd=ROOT, env=environment, check=False, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    if process.returncode != 0:
        sys.stderr.write(process.stdout[-4000:])
        fail(f"{what} failed with exit status {process.returncode}")
    return process.stdout


def held(path: _Path, digest: str, what: str) -> bytes:
    data = path.read_bytes()
    if hashlib.sha256(data).hexdigest() != digest:
        fail(f"{what} at {path.relative_to(ROOT)} does not match the SHA-256 its image identity records")
    return data


def read(what: str, function, data: bytes) -> list[str]:
    try:
        return el0_timer_audit.program_accesses(function(data))
    except el0_timer_audit.AuditError as error:
        fail(f"{what}: {error}")


def components(generation: bytes, what: str) -> dict[str, list[str]]:
    try:
        images = el0_timer_audit.generation_images(generation)
    except el0_timer_audit.AuditError as error:
        fail(f"{what}: {error}")
    return {f"component {name}": read(f"{what}, component {name}", el0_timer_audit.image_code, image) for name, image in images}


def refused(command: list[str], environment: dict[str, str], what: str) -> None:
    """The builder must stop, and name the flag, before it builds anything."""
    process = subprocess.run(command, cwd=ROOT, env=environment, check=False, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    if process.returncode == 0 or "--system-timer" not in process.stdout or WROTE.search(process.stdout):
        sys.stderr.write(process.stdout[-2000:])
        fail(f"the builder did not refuse {what}")


def image_programs(system_timer: bool) -> tuple[str, dict[str, list[str]]]:
    """Build the product image; return its identity path and every EL0 program it holds."""
    environment = {name: value for name, value in os.environ.items() if name != SWITCH}
    command = [sys.executable, str(ROOT / "scripts" / "build" / "build-sel4.py"), "--platform", PLATFORM, "--component-graph", "--skip-pin-check"]
    what = f"the {PLATFORM} image{' with --system-timer' if system_timer else ''}"
    output = run(command + (["--system-timer"] if system_timer else []), environment, f"the build of {what}")
    wrote = WROTE.findall(output)
    if len(wrote) != 1:
        fail(f"the build of {what} named {len(wrote)} image identities, expected one")
    identity = json.loads((ROOT / wrote[0]).read_text(encoding="utf-8"))
    if identity.get("platform") != PLATFORM or identity.get("target_profile") != TARGET_PROFILE:
        fail(f"{wrote[0]} is not an identity of {PLATFORM} for {TARGET_PROFILE}")
    if identity.get("system_timer", False) is not system_timer:
        fail(f"{wrote[0]} records system_timer={identity.get('system_timer')!r} for {what}")
    programs = {}
    for role in EL0_ELFS:
        record = identity["elf"][role]
        programs[role] = read(f"{what}, {role} ELF", el0_timer_audit.elf_code, held(ROOT / record["path"], record["sha256"], f"the {role} ELF"))
    digest = identity["generation"]["sha256"]
    generations = [path for path in sorted((ROOT / "build" / PLATFORM).glob("*/generation.bin")) if hashlib.sha256(path.read_bytes()).hexdigest() == digest]
    if not generations:
        fail(f"no generation under build/{PLATFORM}/ has the SHA-256 that {wrote[0]} records")
    programs.update(components(generations[0].read_bytes(), what))
    return wrote[0], programs


def clock_programs(system_timer: bool) -> dict[str, list[str]]:
    """Build the clock composition for the same profile and read its executables."""
    environment = {name: value for name, value in os.environ.items() if name != SWITCH}
    environment.update(SEL4_PREFIX=str(PREFIX), SLIME_TARGET_PROFILE=TARGET_PROFILE, SLIME_SEL4_MANIFEST=CLOCK_MANIFEST)
    if system_timer:
        environment[SWITCH] = "1"
    output = WORK / f"{CLOCK_MANIFEST}-{'system-timer' if system_timer else 'generic-timer'}"
    output.mkdir(parents=True, exist_ok=True)
    generation = output / "generation.bin"
    generation.unlink(missing_ok=True)
    what = f"{CLOCK_MANIFEST} for {TARGET_PROFILE}{f' with {SWITCH}=1' if system_timer else ''}"
    run([sys.executable, str(ROOT / "scripts" / "build" / "build-generation.py"), "--generation-only", str(output)], environment, f"the build of {what}")
    if not generation.is_file():
        fail(f"the build of {what} wrote no generation")
    return {f"{CLOCK_MANIFEST} {name}": found for name, found in components(generation.read_bytes(), what).items()}


def main() -> int:
    if sys.argv[1:] not in ([], ["--controls-only"]):
        fail(f"unknown arguments {sys.argv[1:]}; the script takes --controls-only or nothing")
    try:
        synthetic = el0_timer_audit.controls()
    except el0_timer_audit.AuditError as error:
        fail(str(error))
    print(f"EL0 generic-timer check: {synthetic} synthetic controls refused or found what they must")
    if sys.argv[1:]:
        return 0

    builder = [sys.executable, str(ROOT / "scripts" / "build" / "build-sel4.py"), "--component-graph", "--skip-pin-check"]
    clean = {name: value for name, value in os.environ.items() if name != SWITCH}
    refused(builder + ["--system-timer"], clean, "--system-timer for the default platform")
    refused(builder + ["--platform", PLATFORM], clean | {SWITCH: "1"}, f"{SWITCH} in its environment")

    generic_identity, generic = image_programs(False)
    system_identity, system = image_programs(True)
    if generic_identity == system_identity:
        fail(f"both builds wrote the image identity {system_identity}")
    generic |= clock_programs(False)
    system |= clock_programs(True)

    # Controls on real builds: without the switch the reader finds the accesses the sources hold.
    clock_reader = f"{CLOCK_MANIFEST} component {CLOCK_READER}"
    for program, access in (("root", "msr CNTP_CTL_EL0"), ("root", "mrs CNTPCT_EL0"), (clock_reader, "mrs CNTPCT_EL0")):
        if access not in generic.get(program, []):
            fail(f"control: {program} of the build without the switch holds no '{access}', so the audit cannot tell the two builds apart")
    if set(system) != set(generic):
        fail(f"the two builds hold different programs: {sorted(set(system) ^ set(generic))}")

    problems = [f"{program}: {len(found)} access(es), {sorted(set(found))}" for program, found in sorted(system.items()) if found]
    for program, found in sorted(system.items()):
        print(f"[el0-timer] {program:<52} generic={len(generic[program]):>3} system={len(found):>3}")
    if problems:
        fail("the system-timer image names a generic-timer register from EL0:\n  " + "\n  ".join(problems))
    devloop_observations.record(
        el0ProgramsAudited=len(system),
        el0TimerAccesses=sum(len(found) for found in system.values()),
        el0TimerAccessesWithoutSwitch=sum(len(found) for found in generic.values()),
        el0TimerSyntheticControls=synthetic,
    )
    print(f"EL0 generic-timer check: {len(system)} EL0 programs of the {PLATFORM} system-timer build hold no generic-timer access")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
