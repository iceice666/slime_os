#!/usr/bin/env python3

"""P3.F: drive the resident Slisp product on the named Milk-V Duo."""

from __future__ import annotations

import argparse
import importlib.util
import json
import re
import subprocess
import sys
import time
from pathlib import Path
from types import ModuleType
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from harness import sha256_file  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
FIT_SCRIPT = ROOT / "scripts" / "build" / "build-duo-payload.py"
DUO_GATE = ROOT / "scripts" / "check" / "check-duo-boot.py"
PAYLOAD_DIR = ROOT / "build" / "duo-payload"
PLATFORM = "cv1800b-duo"
TARGET_PROFILE = "riscv64-sel4-milkv-duo"
PRODUCT_STEM = "slime-sel4-cv1800b-duo"
STEM = f"{PRODUCT_STEM}-test-terminator"
IMAGE = ROOT / "build" / "slime-sel4-graph-cv1800b-duo-test-terminator.elf"
IMAGE_IDENTITY = (
    ROOT / "build" / "slime-sel4-graph-cv1800b-duo-test-terminator.identity.json"
)
FIT = PAYLOAD_DIR / f"{STEM}.itb"
FIT_IDENTITY = PAYLOAD_DIR / f"{STEM}.identity.json"
SLISP_ELF = ROOT / "build" / "slisp-product-riscv64.elf"
BOOT_TIMEOUT_SECONDS = 180
SESSION_TIMEOUT_SECONDS = 90
VENDOR_RECOVERY_SECONDS = 180
TEST_TERMINATOR = b"\x1d"

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL .*",
    r"SLIME_GRAPH FAIL .*",
    r"SLIME_TIMER FAIL .*",
    r"KERNEL INVALID VECTOR ENTRY",
    r"Kernel init failed",
    r"seL4 called fail",
    r"Caught cap fault",
    r"Caught vm fault",
    r"Caught user exception",
    r"panicked at ",
    r"aborted at ",
    r"SLIME_GRAPH component exit task=3 ",
)

BOOT_MARKERS: tuple[tuple[str, str], ...] = (
    ("U-Boot selected the product FIT", r"Using 'config-duo-sel4' configuration"),
    ("control left U-Boot", r"Starting kernel \.\.\."),
    ("upstream seL4 reached userspace", r"Booting all finished, dropped to user space"),
    ("the Duo UART input adapter was mapped", r"SLIME_ROOT product input ready uart=0x4140000"),
    (
        "the product target generation was admitted",
        r"SLIME_ROOT generation admitted number=1 executables=6 instances=6 grants=\d+ ",
    ),
    ("only init was root-activated", r"SLIME_GRAPH activated instances=1"),
    ("init launched the declared graph", r"\[init\] launching component graph"),
    (
        "all required residents became healthy",
        r"SLIME_GRAPH healthy generation=1 instances=[0-9a-f]{16} required=4 live=4 idle=4 failed=0",
    ),
    ("the physical target reached ready", rf"SLIME_ROOT READY target_profile={TARGET_PROFILE}"),
    ("the product stayed resident", r"\[init\] product services resident"),
    ("Slisp identified itself", r"Slisp"),
    ("Slisp presented its initial prompt", r"slisp> "),
    ("empty UART RX became WouldBlock", r"\[slisp\] resident input wait"),
)

SESSION_MARKERS: tuple[tuple[str, str], ...] = (
    ("the definition command arrived intact", r"\(define answer 40\)\r?\n=> 40"),
    ("the persistent binding evaluated", r"\(\+ answer 2\)\r?\n=> 42"),
    ("Slisp requested sysinfo through spawn-service", r"sysinfo\r?\n\[spawn-service\] request"),
    ("sysinfo used the declared launch profile", r"\[sysinfo\] spawned through profile"),
    ("sysinfo exited cleanly", r"SLIME_GRAPH component exit task=\d+ status=0"),
    (
        "spawn-service collected child supervision",
        r"SLIME_GRAPH supervision collected task=2 child=\d+ kind=0",
    ),
    ("Slisp accepted the spawn", r"=> spawned sysinfo"),
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"duo Slisp check: {message}")


def load_module(path: Path, name: str) -> ModuleType:
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        fail(f"cannot load {path.relative_to(ROOT)}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def run(command: list[str]) -> None:
    print(f"[run] {' '.join(command)}", flush=True)
    process = subprocess.run(command, cwd=ROOT, check=False)
    if process.returncode != 0:
        fail(f"command failed with exit status {process.returncode}: {' '.join(command)}")


def build_artifacts() -> None:
    run(
        [
            sys.executable,
            str(BUILD_SCRIPT),
            "--component-graph",
            "--platform",
            PLATFORM,
            "--duo-test-terminator",
        ]
    )
    run(
        [
            sys.executable,
            str(FIT_SCRIPT),
            "--sel4",
            "--image",
            str(IMAGE),
            "--identity",
            str(IMAGE_IDENTITY),
            "--output-stem",
            STEM,
        ]
    )


def check_identity() -> tuple[str, dict[str, object]]:
    try:
        image_identity = json.loads(IMAGE_IDENTITY.read_text(encoding="utf-8"))
        fit_identity = json.loads(FIT_IDENTITY.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot read product identity: {error}")
    if image_identity.get("kind") != "slime-sel4-image-identity":
        fail("the product image identity has the wrong kind")
    if image_identity.get("platform") != PLATFORM:
        fail("the product image identity names the wrong platform")
    if image_identity.get("target_profile") != TARGET_PROFILE:
        fail("the product image identity names the wrong target profile")
    if image_identity.get("variant") != "graph" or not image_identity.get("component_graph"):
        fail("the product image identity is not the resident graph")
    if image_identity.get("duo_early_fault", False):
        fail("the product image unexpectedly enables the P3.E early-fault control")
    if not image_identity.get("duo_test_terminator", False):
        fail("the physical gate image lacks its explicit test-only terminator")
    image_record = image_identity.get("image")
    if not isinstance(image_record, dict) or image_record.get("sha256") != sha256_file(IMAGE, fail):
        fail("the product image does not match its identity")
    if fit_identity.get("variant") != "graph" or fit_identity.get("target_profile") != TARGET_PROFILE:
        fail("the FIT identity is not the Duo resident product")
    if not fit_identity.get("duo_test_terminator", False):
        fail("the physical gate FIT lacks its explicit test-only terminator")
    if fit_identity.get("elf_sha256") != sha256_file(IMAGE, fail):
        fail("the FIT identity does not bind the product ELF")
    digest = sha256_file(FIT, fail)
    if fit_identity.get("fit_sha256") != digest:
        fail("the FIT does not match its identity")
    if not SLISP_ELF.is_file():
        fail("the RV64 Slisp ELF is missing")
    return digest, {
        "fit_sha256": digest,
        "elf_sha256": sha256_file(IMAGE, fail),
        "generation_identity": fit_identity.get("generation_identity"),
        "generation_sha256": fit_identity.get("generation_sha256"),
        "slisp_sha256": sha256_file(SLISP_ELF, fail),
        "target_profile": TARGET_PROFILE,
    }


def check_ordered(transcript: str, markers: tuple[tuple[str, str], ...]) -> None:
    for pattern in FAILURE_MARKERS:
        match = re.search(pattern, transcript)
        if match is not None:
            fail(f"serial transcript contains failure marker {match.group(0)!r}")
    position = 0
    for description, pattern in markers:
        match = re.compile(pattern).search(transcript, position)
        if match is None:
            fail(f"serial transcript is missing or reorders {description}: {pattern}")
        position = match.end()


def wait_for(profile: dict[str, object], description: str) -> None:
    host = str(profile["usb_ncm_address"])
    deadline = time.monotonic() + VENDOR_RECOVERY_SECONDS
    while time.monotonic() < deadline:
        process = subprocess.run(
            ["ping", "-c", "1", "-W", "2", host], capture_output=True, check=False
        )
        if process.returncode == 0:
            return
        time.sleep(2)
    fail(f"{description}: vendor Linux did not return at {host}")


def drive_session(
    duo: ModuleType,
    profile: dict[str, object],
    *,
    serial: Path,
    evidence_dir: Path | None,
) -> tuple[str, int]:
    console = duo.Console(serial, int(profile["serial_baud"]))
    transcript = ""

    def persist_transcript() -> None:
        if evidence_dir is not None:
            evidence_dir.mkdir(parents=True, exist_ok=True)
            (evidence_dir / "duo-slisp-session.log").write_text(
                transcript, encoding="utf-8"
            )

    try:
        duo.reach_uboot(console, str(profile["uboot_prompt"]), 90)
        staging = str(profile["fit_staging_address"])
        partition = str(profile["boot_partition"])
        console.write(f"fatload {partition} {staging} {FIT.name}\r".encode())
        transcript += console.read_for(5.0)
        if re.search(r"(\d+)\s+bytes read|Bytes transferred\s*=\s*(\d+)", transcript) is None:
            fail(f"U-Boot did not load {FIT.name}")
        console.write(
            str(profile["uboot_launch"])
            .format(staging=staging, config=str(profile["fit_config"]))
            .encode()
            + b"\r"
        )
        boot_terminal = re.compile(BOOT_MARKERS[-1][1])
        deadline = time.monotonic() + BOOT_TIMEOUT_SECONDS
        while time.monotonic() < deadline:
            transcript += console.read_for(0.25)
            if boot_terminal.search(transcript):
                break
        else:
            persist_transcript()
            duo.report_transcript(transcript)
            fail("the product did not reach resident input wait")
        persist_transcript()
        check_ordered(transcript, BOOT_MARKERS)
        if len(re.findall(r"\[slisp\] resident input wait", transcript)) != 1:
            fail("the session did not report exactly one resident empty-FIFO interval")
        time.sleep(0.75)
        transcript += console.read_for(0.25)
        commands = ("(define answer 40)\n", "(+ answer 2)\n", "sysinfo\n")
        for command in commands:
            for character in command:
                console.write(character.encode())
                transcript += console.read_for(0.05)
            transcript += console.read_for(0.75)
        terminal = re.compile(SESSION_MARKERS[-1][1])
        deadline = time.monotonic() + SESSION_TIMEOUT_SECONDS
        while time.monotonic() < deadline and terminal.search(transcript) is None:
            transcript += console.read_for(0.25)
        check_ordered(transcript, SESSION_MARKERS)
        if len(re.findall(r"SLIME_GRAPH healthy .*required=4 live=4", transcript)) != 1:
            fail("the resident graph was restarted or recertified during the session")
        console.write(TEST_TERMINATOR)
        reset_terminal = re.compile(r"SLIME_DUO reset request kind=cold")
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline and reset_terminal.search(transcript) is None:
            transcript += console.read_for(0.25)
        if re.search(r"SLIME_DUO test terminator accepted", transcript) is None:
            fail("the explicit test-only terminator was not accepted")
        if reset_terminal.search(transcript) is None:
            fail("the test terminator did not request the qualified cold reset")
        persist_transcript()
        return transcript, console.framing_errors
    except BaseException:
        persist_transcript()
        raise
    finally:
        console.close()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", type=Path)
    parser.add_argument("--key", type=Path, default=Path.home() / ".ssh" / "slime_duo")
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--evidence-dir", type=Path)
    arguments = parser.parse_args()
    if arguments.serial is None:
        fail("no serial device given; P3.F requires the named physical Duo")

    duo = load_module(DUO_GATE, "duo_boot_gate")
    profile = duo.board_profile(duo.load_pins())
    if not arguments.no_build:
        build_artifacts()
    digest, identity = check_identity()
    duo.deploy(profile, arguments.key, digest, FIT)
    transcript, framing_errors = drive_session(
        duo, profile, serial=arguments.serial, evidence_dir=arguments.evidence_dir
    )
    if framing_errors != 0:
        fail(f"the physical session observed {framing_errors} serial framing errors")
    wait_for(profile, "after the P3.F cold reset")
    if arguments.evidence_dir is not None:
        identity.update(
            {
                "board": profile["board"],
                "soc": profile["soc"],
                "firmware": profile["uboot_version"],
                "serial": profile["serial"],
                "serial_baud": profile["serial_baud"],
                "framing_errors": framing_errors,
                "transcript_sha256": sha256_file(
                    arguments.evidence_dir / "duo-slisp-session.log", fail
                ),
            }
        )
        (arguments.evidence_dir / "duo-slisp-identities.json").write_text(
            json.dumps(identity, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
    if re.search(r"SLIME_GRAPH component exit task=3 ", transcript):
        fail("Slisp exited during the physical session")
    print(
        "duo Slisp check: the target-qualified resident graph accepted real UART0 input "
        "only through InputRead, preserved Slisp state across three commands, launched "
        "sysinfo through its declared profile, observed zero framing errors, and returned "
        "to vendor Linux through the explicit gate-only cold-reset terminator"
    )


if __name__ == "__main__":
    main()
