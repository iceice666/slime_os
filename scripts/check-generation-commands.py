#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import importlib.util
import os
import shutil
import struct
import subprocess
from pathlib import Path

from boot_contracts import (
    BOOTSTATE_SLOT_BYTES,
    BOOTSTORE_DIRECTORY_OFFSET,
    BOOTSTORE_ENTRY,
    BOOTSTORE_HEADER,
    RELEASE_BYTES,
    bootstore_checksum,
)

ROOT = Path(__file__).resolve().parent.parent
KERNEL = ROOT / "kernel"
BUILD = KERNEL / "target" / "x86_64-unknown-none" / "release"
WORK = Path("/tmp/slime-os-generation-cmd")
TRACE_SPEC = importlib.util.spec_from_file_location(
    "check_bootstate_trace", ROOT / "scripts" / "check-bootstate-trace.py"
)
if TRACE_SPEC is None or TRACE_SPEC.loader is None:
    raise SystemExit("cannot load BootState trace verifier")
TRACE = importlib.util.module_from_spec(TRACE_SPEC)
TRACE_SPEC.loader.exec_module(TRACE)

SUCCESS_MARKERS = (
    "[generation-manager] update service ready",
    "[generation-list] direct boot update denied",
    "[generation-list] count=2 accepted-release=1",
    "[generation-inspect] generation=8",
    "[generation-stage] staged release=3",
    "[generation-select] pending attempts=3",
    "[bootstate-trace] v1 action=stage-pending commit=after-pending-commit",
    "[bootstate-trace] v1 action=rollback commit=rollback-update",
    "[generation-rollback] known-good restored",
    "[generation] vertical slice healthy",
)


def bootstate_sequence(image: bytes, slot: int) -> int:
    return struct.unpack_from("<Q", image, slot * BOOTSTATE_SLOT_BYTES + 24)[0]


def bootstate_pending(image: bytes, slot: int) -> bytes:
    offset = slot * BOOTSTATE_SLOT_BYTES
    return image[offset + 64 : offset + 96]


def generation_entries(image: bytes) -> list[tuple[bytes, int, int, int, int]]:
    count = struct.unpack_from("<I", image, BOOTSTORE_DIRECTORY_OFFSET + 24)[0]
    entries = []
    offset = BOOTSTORE_DIRECTORY_OFFSET + BOOTSTORE_HEADER.size
    for index in range(count):
        record = BOOTSTORE_ENTRY.unpack_from(image, offset + index * BOOTSTORE_ENTRY.size)
        entries.append((record[0], record[1], record[2], record[3], record[4]))
    return entries

def build_fixture(scenario: str) -> Path:
    environment = os.environ.copy()
    environment["SLIME_GENERATION_NUMBER"] = "8"
    environment["SLIME_GENERATION_CMD_CHECK"] = "1"
    environment["SLIME_GENERATION_CMD_SCENARIO"] = scenario
    environment["SLIME_KNOWN_GOOD_FIRST"] = "1"
    environment["SLIME_PENDING_RELEASE_SEQUENCE"] = "3"
    subprocess.run(["cargo", "build", "--release"], cwd=KERNEL, env=environment, check=True)
    generated = WORK / f"build-{scenario}"
    shutil.rmtree(generated, ignore_errors=True)
    subprocess.run(
        [str(ROOT / "scripts" / "build-generation.py"), str(BUILD / "slime_os-kernel"), str(generated)],
        cwd=ROOT,
        env=environment,
        check=True,
    )
    image = bytearray((generated / "boot-store.bin").read_bytes())
    entries = generation_entries(image)
    if len(entries) != 2:
        raise SystemExit(f"{scenario}: expected two generation entries")
    if scenario == "bad-closure":
        _, generation_offset, generation_len, _, _ = entries[1]
        image[generation_offset + generation_len - 1] ^= 0x01
    elif scenario == "bad-release":
        _, _, _, release_offset, release_len = entries[1]
        if release_len != RELEASE_BYTES:
            raise SystemExit("unexpected release record size")
        # Signature payload corruption preserves directory/checksum validity but
        # fails release authorization during staging.
        image[release_offset + 208 + 32] ^= 0x80
    checksum_start = BOOTSTORE_DIRECTORY_OFFSET + 48
    image[checksum_start : checksum_start + 32] = bytes(32)
    image[checksum_start : checksum_start + 32] = bootstore_checksum(image)
    bootstore = WORK / f"{scenario}.img"
    bootstore.write_bytes(image)
    return bootstore


def run_scenario(scenario: str, expected_marker: str) -> tuple[str, bytes, bytes]:
    bootstore = build_fixture(scenario)
    before = bootstore.read_bytes()
    boot_image = WORK / f"{scenario}-boot.img"
    environment = os.environ.copy()
    environment["SLIME_GENERATION_CMD_SCENARIO"] = scenario
    environment["SLIME_GENERATION_NUMBER"] = "8"
    environment["SLIME_GENERATION_CMD_CHECK"] = "1"
    environment["SLIME_GENERATION_DIR"] = str(WORK / f"build-{scenario}")
    environment["SLIME_BOOT_IMAGE"] = str(boot_image)
    process = subprocess.run(
        [
            "cargo",
            "run",
            "--release",
            "--",
            "-display",
            "none",
            "-drive",
            f"if=none,format=raw,file={bootstore},id=generation-store",
            "-device",
            "virtio-blk-pci,drive=generation-store",
        ],
        cwd=KERNEL,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        timeout=90,
    )
    print(process.stdout, end="")
    if process.returncode != 0:
        raise SystemExit(process.returncode)
    if expected_marker not in process.stdout:
        raise SystemExit(f"{scenario}: missing marker: {expected_marker}")
    return process.stdout, before, bootstore.read_bytes()


def validate_transitions(output: str, after: bytes) -> None:
    records = []
    for line in output.splitlines():
        if line.startswith("[bootstate-trace]") and (
            "action=stage-pending" in line or "action=rollback" in line
        ):
            records.append(TRACE.parse_trace_line(line))
    if [record["action"] for record in records] != ["stage-pending", "rollback"]:
        raise SystemExit("unexpected generation transition sequence")
    oracle = TRACE.Oracle()
    for record in records:
        if not oracle.reachable(
            record["action"],
            record["commit"],
            record["attempts_before"],
            record["attempts_after"],
        ):
            raise SystemExit(f"generation transition is not model-reachable: {record}")


def main() -> None:
    shutil.rmtree(WORK, ignore_errors=True)
    WORK.mkdir(parents=True)

    output, before, after = run_scenario("success", SUCCESS_MARKERS[0])
    for marker in SUCCESS_MARKERS:
        if marker not in output:
            raise SystemExit(f"success: missing generation command marker: {marker}")
    if before[BOOTSTATE_SLOT_BYTES * 2 :] != after[BOOTSTATE_SLOT_BYTES * 2 :]:
        raise SystemExit("generation commands modified bytes outside redundant BootState slots")
    sequences = [bootstate_sequence(after, slot) for slot in range(2)]
    if max(sequences) != 4:
        raise SystemExit(f"unexpected final BootState sequences: {sequences}")
    selected = sequences.index(max(sequences))
    if bootstate_pending(after, selected) != bytes(32):
        raise SystemExit("rollback left a pending generation in the newest BootState slot")
    validate_transitions(output, after)

    for scenario in ("bad-closure", "bad-release"):
        output, before, after = run_scenario(scenario, "[generation-stage] rejected status=")
        if not any(
            marker in output
            for marker in (
                "[generation-stage] rejected status=-4",
                "[generation-stage] rejected status=-3",
            )
        ):
            raise SystemExit(f"{scenario}: staging failure was not classified")
        if before[: BOOTSTATE_SLOT_BYTES * 2] != after[: BOOTSTATE_SLOT_BYTES * 2]:
            raise SystemExit(f"{scenario}: rejected staging changed BootState")

    print("generation command check: ok")


if __name__ == "__main__":
    main()
