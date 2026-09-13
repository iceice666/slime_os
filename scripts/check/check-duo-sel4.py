#!/usr/bin/env python3

"""P3.E: boot the architecture-neutral sample plane on the named Milk-V Duo."""

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
SAMPLE_GATE = ROOT / "scripts" / "check" / "check-sel4-sample-plane.py"
PAYLOAD_DIR = ROOT / "build" / "duo-payload"
PLATFORM = "cv1800b-duo"
TARGET_PROFILE = "riscv64-sel4-milkv-duo"
SAMPLE_STEM = "slime-sel4-sample-cv1800b-duo"
FAULT_STEM = f"{SAMPLE_STEM}-early-fault"
RUNS = 3
BOOT_TIMEOUT_SECONDS = 180
VENDOR_RECOVERY_SECONDS = 180

COMMON_BOOT_MARKERS: tuple[tuple[str, str], ...] = (
    ("U-Boot selected the seL4 FIT", r"Using 'config-duo-sel4' configuration"),
    (
        "the FIT payload hash passed",
        r"Trying 'kernel-1' kernel subimage\s+Verifying Hash Integrity \.\.\. crc32\+ OK",
    ),
    (
        "the FIT device-tree hash passed",
        r"Trying 'fdt-duo' fdt subimage\s+Verifying Hash Integrity \.\.\. crc32\+ OK",
    ),
    ("control left U-Boot", r"Starting kernel \.\.\."),
    (
        "the MAEE-encoded loader page tables became active",
        r"SLIME_DUO loader page tables active",
    ),
    ("upstream seL4 reached userspace", r"Booting all finished, dropped to user space"),
    (
        "the root acquired the observed RTC alarm IRQ",
        r"SLIME_TIMER acquired irq=17 freq_hz=1",
    ),
    ("the startup timer interrupt was delivered", r"SLIME_TIMER delivered badge=0x1 polls=\d+"),
    ("the startup timer expiry was serviced", r"SLIME_TIMER OK"),
)

BOOT_MARKERS: tuple[tuple[str, str], ...] = COMMON_BOOT_MARKERS + (
    (
        "the physical target generation was admitted",
        r"SLIME_ROOT generation admitted number=\d+ executables=4 instances=4 grants=6 ",
    ),
    ("the component graph activated", r"SLIME_GRAPH activated instances=2"),
    (
        "timer delivery remained live after graph activation",
        r"SLIME_TIMER phase=post-graph-start delivered badge=0x1 polls=\d+",
    ),
    (
        "the post-activation timer expiry was serviced",
        r"SLIME_TIMER phase=post-graph-start OK",
    ),
    (
        "the physical target profile reached ready",
        rf"SLIME_ROOT READY target_profile={TARGET_PROFILE}",
    ),
    ("the root requested a cold reset", r"SLIME_DUO reset request kind=cold"),
)

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
)

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
    raise SystemExit(f"duo seL4 check: {message}")


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


def artifact_paths(stem: str) -> tuple[Path, Path, Path, Path]:
    return (
        ROOT / "build" / f"{stem}.elf",
        ROOT / "build" / f"{stem}.identity.json",
        PAYLOAD_DIR / f"{stem}.itb",
        PAYLOAD_DIR / f"{stem}.identity.json",
    )


def build_artifacts(stem: str, *, early_fault: bool) -> None:
    command = [
        sys.executable,
        str(BUILD_SCRIPT),
        "--sample-plane",
        "--platform",
        PLATFORM,
    ]
    if early_fault:
        command.append("--duo-early-fault")
    run(command)
    image, manifest, _, _ = artifact_paths(stem)
    run(
        [
            sys.executable,
            str(FIT_SCRIPT),
            "--sel4",
            "--image",
            str(image),
            "--identity",
            str(manifest),
            "--output-stem",
            stem,
        ]
    )


def check_identity(stem: str, *, early_fault: bool) -> str:
    image, manifest_path, fit, fit_identity_path = artifact_paths(stem)
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        fit_identity = json.loads(fit_identity_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot read {stem}'s identity: {error}")
    if manifest.get("kind") != "slime-sel4-image-identity":
        fail(f"{manifest_path.relative_to(ROOT)} is not a seL4 image identity")
    if manifest.get("platform") != PLATFORM or manifest.get("target_profile") != TARGET_PROFILE:
        fail(f"{manifest_path.relative_to(ROOT)} names the wrong physical target")
    if manifest.get("variant") != "sample":
        fail(f"{manifest_path.relative_to(ROOT)} is not the sample-plane image")
    if bool(manifest.get("duo_early_fault", False)) != early_fault:
        fail(f"{manifest_path.relative_to(ROOT)} records the wrong fault mode")
    if bool(fit_identity.get("duo_early_fault", False)) != early_fault:
        fail(f"{fit_identity_path.relative_to(ROOT)} records the wrong fault mode")
    image_record = manifest.get("image")
    if not isinstance(image_record, dict) or image_record.get("sha256") != sha256_file(image, fail):
        fail(f"{manifest_path.relative_to(ROOT)} does not match {image.relative_to(ROOT)}")
    if fit_identity.get("variant") != "sample":
        fail(f"{fit_identity_path.relative_to(ROOT)} is not the sample-plane FIT")
    if fit_identity.get("elf_sha256") != sha256_file(image, fail):
        fail(f"{fit_identity_path.relative_to(ROOT)} does not bind the packaged ELF")
    digest = sha256_file(fit, fail)
    if fit_identity.get("fit_sha256") != digest:
        fail(f"{fit_identity_path.relative_to(ROOT)} does not match {fit.relative_to(ROOT)}")
    if early_fault != stem.endswith("-early-fault"):
        fail("internal artifact-mode mismatch")
    return digest


def wait_for_vendor_linux(profile: dict[str, object]) -> None:
    host = str(profile["usb_ncm_address"])
    deadline = time.monotonic() + VENDOR_RECOVERY_SECONDS
    while time.monotonic() < deadline:
        process = subprocess.run(
            ["ping", "-c", "1", "-W", "2", host],
            capture_output=True,
            check=False,
        )
        if process.returncode == 0:
            return
        time.sleep(2)
    fail(
        f"the board did not recover vendor Linux at {host} within "
        f"{VENDOR_RECOVERY_SECONDS}s after its autonomous reset"
    )


def capture_boot(
    duo: ModuleType,
    profile: dict[str, object],
    *,
    serial: Path,
    fit: Path,
    terminal: re.Pattern[str],
    evidence_path: Path | None,
) -> tuple[str, int]:
    console = duo.Console(serial, int(profile["serial_baud"]))
    try:
        duo.reach_uboot(console, str(profile["uboot_prompt"]), 90)
        staging = str(profile["fit_staging_address"])
        partition = str(profile["boot_partition"])
        console.write(f"fatload {partition} {staging} {fit.name}\r".encode())
        loaded = console.read_for(5.0)
        match = re.search(r"(\d+)\s+bytes read|Bytes transferred\s*=\s*(\d+)", loaded)
        if match is None or int(next(group for group in match.groups() if group)) == 0:
            fail(f"U-Boot did not load {fit.name} from the board's boot partition")
        console.write(
            str(profile["uboot_launch"])
            .format(staging=staging, config=str(profile["fit_config"]))
            .encode()
            + b"\r"
        )
        transcript = loaded
        deadline = time.monotonic() + BOOT_TIMEOUT_SECONDS
        failures = re.compile("|".join(FAILURE_MARKERS))
        while time.monotonic() < deadline:
            transcript += console.read_for(0.25)
            if terminal.search(transcript) or failures.search(transcript):
                break
        else:
            if evidence_path is not None:
                evidence_path.write_text(transcript)
            duo.report_transcript(transcript)
            fail(f"{fit.name} did not reach its terminal marker within {BOOT_TIMEOUT_SECONDS}s")
        return transcript, console.framing_errors
    finally:
        console.close()


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


def normalize(transcript: str) -> str:
    lines: list[str] = []
    for raw in transcript.splitlines():
        line = raw.strip()
        if not (
            line.startswith("SLIME_") or line.startswith("[init]") or line.startswith("[sample-")
        ):
            continue
        if line == "SLIME_DUO reset request kind=cold":
            break
        for pattern, replacement in DYNAMIC_FIELDS:
            line = pattern.sub(replacement, line)
        lines.append(line)
    return "\n".join(lines)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", type=Path)
    parser.add_argument(
        "--key",
        type=Path,
        default=Path.home() / ".ssh" / "slime_duo",
    )
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--evidence-dir", type=Path)
    arguments = parser.parse_args()
    if arguments.serial is None:
        fail(
            "no serial device given, so no board evidence can be observed; "
            "P3.E requires repeated seL4 boots on the named Milk-V Duo"
        )

    duo = load_module(DUO_GATE, "duo_boot_gate")
    sample = load_module(SAMPLE_GATE, "duo_sample_gate")
    pins = duo.load_pins()
    profile = duo.board_profile(pins)

    if not arguments.no_build:
        build_artifacts(SAMPLE_STEM, early_fault=False)
        build_artifacts(FAULT_STEM, early_fault=True)

    sample_digest = check_identity(SAMPLE_STEM, early_fault=False)
    fault_digest = check_identity(FAULT_STEM, early_fault=True)
    sample_fit = artifact_paths(SAMPLE_STEM)[2]
    fault_fit = artifact_paths(FAULT_STEM)[2]
    duo.deploy(profile, arguments.key, sample_digest, sample_fit)
    duo.deploy(profile, arguments.key, fault_digest, fault_fit)

    evidence_dir = arguments.evidence_dir
    if evidence_dir is not None:
        evidence_dir.mkdir(parents=True, exist_ok=True)

    normalized: list[str] = []
    total_framing_errors = 0
    terminal = re.compile(BOOT_MARKERS[-1][1])
    for run_index in range(1, RUNS + 1):
        transcript, framing_errors = capture_boot(
            duo,
            profile,
            serial=arguments.serial,
            fit=sample_fit,
            terminal=terminal,
            evidence_path=(
                evidence_dir / f"sample-run-{run_index}.log" if evidence_dir is not None else None
            ),
        )
        if evidence_dir is not None:
            (evidence_dir / f"sample-run-{run_index}.log").write_text(transcript)
        check_ordered(transcript, BOOT_MARKERS)
        sample.check_transcript(transcript)
        if framing_errors != 0:
            fail(f"sample run {run_index} observed {framing_errors} serial framing errors")
        normalized.append(normalize(transcript))
        total_framing_errors += framing_errors
        if evidence_dir is not None:
            (evidence_dir / f"sample-run-{run_index}.normalized.log").write_text(normalized[-1])
        wait_for_vendor_linux(profile)
        print(f"[physical] sample run {run_index}/{RUNS} passed and recovered", flush=True)

    if any(trace != normalized[0] for trace in normalized[1:]):
        fail("the three physical sample runs produced different normalized semantic traces")

    fault_markers = COMMON_BOOT_MARKERS + (
        (
            "the bounded early fault was diagnosed",
            r"SLIME_DUO EARLY_FAULT phase=post-timer cause=timer-range-refused bounded=1",
        ),
        ("the fault path requested a cold reset", r"SLIME_DUO reset request kind=cold"),
    )
    fault_transcript, fault_framing_errors = capture_boot(
        duo,
        profile,
        serial=arguments.serial,
        fit=fault_fit,
        terminal=re.compile(fault_markers[-1][1]),
        evidence_path=evidence_dir / "early-fault.log" if evidence_dir is not None else None,
    )
    if evidence_dir is not None:
        (evidence_dir / "early-fault.log").write_text(fault_transcript)
    check_ordered(fault_transcript, fault_markers)
    if fault_framing_errors != 0:
        fail(f"early-fault run observed {fault_framing_errors} serial framing errors")
    wait_for_vendor_linux(profile)

    print(
        "duo seL4 check: upstream seL4 admitted the riscv64-sel4-milkv-duo "
        "sample generation, delivered timer IRQs before and after graph activation, "
        f"completed {RUNS} byte-identical normalized semantic runs with "
        f"{total_framing_errors} framing errors, diagnosed a bounded early fault, "
        "and cold-reset back to vendor Linux after every boot"
    )


if __name__ == "__main__":
    main()
