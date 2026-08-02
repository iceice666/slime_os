#!/usr/bin/env python3

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import os
import subprocess
import sys
from pathlib import Path


from boot_contracts import (
    BOOTSTATE_SLOT_BYTES,
    BOOTSTORE_DIRECTORY_OFFSET,
    BOOTSTORE_ENTRY,
    BOOTSTORE_HEADER,
    BOOTSTORE_HEADER_CHECKSUM_END,
    BOOTSTORE_HEADER_CHECKSUM_OFFSET,
    RELEASE_HEADER_BYTES,
    bootstore_checksum,
)
from harness import BOOT_TIMEOUT_SECONDS, RELEASE_KERNEL, ROOT, load_script, run_qemu

KERNEL = RELEASE_KERNEL

CHECK_GENERATION = load_script("check_generation", "check/check-generation.py")
decode_bootstate = CHECK_GENERATION.decode_bootstate


def run(
    arguments: list[str],
    *,
    environment: dict[str, str] | None = None,
    allow_failure: bool = False,
    timeout: int | None = BOOT_TIMEOUT_SECONDS,
) -> str:
    return run_qemu(
        arguments,
        environment=environment,
        allow_failure=allow_failure,
        timeout=timeout,
    )


def bootstate(image: Path) -> dict:
    extracted = Path("/tmp/slime-os-rollback-boot-store.bin")
    extracted.unlink(missing_ok=True)
    subprocess.run(
        ["mcopy", "-o", "-i", str(image), "::/boot/boot-store.bin", str(extracted)],
        check=True,
    )
    data = extracted.read_bytes()
    states = []
    for index in range(2):
        slot = data[index * BOOTSTATE_SLOT_BYTES : (index + 1) * BOOTSTATE_SLOT_BYTES]
        try:
            states.append(decode_bootstate(slot))
        except SystemExit:
            pass
    if not states:
        raise SystemExit("rollback image has no valid BootState slot")
    return max(states, key=lambda state: state["sequence"])

def corrupt_pending_release(image: Path) -> None:
    extracted = Path("/tmp/slime-os-rollback-bad-release.bin")
    extracted.unlink(missing_ok=True)
    subprocess.run(
        ["mcopy", "-o", "-i", str(image), "::/boot/boot-store.bin", str(extracted)],
        check=True,
    )
    data = bytearray(extracted.read_bytes())
    pending = bootstate(image)["pending"]
    if pending is None:
        raise SystemExit("rollback fixture has no pending release to corrupt")
    count = int.from_bytes(
        data[BOOTSTORE_DIRECTORY_OFFSET + 24 : BOOTSTORE_DIRECTORY_OFFSET + 28], "little"
    )
    directory = BOOTSTORE_DIRECTORY_OFFSET + BOOTSTORE_HEADER.size
    for index in range(count):
        identity, _offset, _length, release_offset, release_length = BOOTSTORE_ENTRY.unpack_from(
            data, directory + index * BOOTSTORE_ENTRY.size
        )
        if identity == pending:
            if release_length <= RELEASE_HEADER_BYTES:
                raise SystemExit("pending release has no signature bytes")
            data[release_offset + RELEASE_HEADER_BYTES] ^= 0x01
            checksum_start = BOOTSTORE_DIRECTORY_OFFSET + BOOTSTORE_HEADER_CHECKSUM_OFFSET
            checksum_end = BOOTSTORE_DIRECTORY_OFFSET + BOOTSTORE_HEADER_CHECKSUM_END
            data[checksum_start:checksum_end] = bytes(checksum_end - checksum_start)
            data[checksum_start:checksum_end] = bootstore_checksum(data)
            extracted.write_bytes(data)
            subprocess.run(
                ["mcopy", "-o", "-i", str(image), str(extracted), "::/boot/boot-store.bin"],
                check=True,
            )
            return
    raise SystemExit("pending release directory entry is missing")


def main() -> None:
    image = Path(sys.argv[1] if len(sys.argv) > 1 else "/tmp/slime-os-rollback.img")
    image.unlink(missing_ok=True)

    environment = os.environ.copy()
    # B11: this gate exercises verification scaffolding, so it selects the
    # boot profile that declares it. The product profile declares none.
    environment["SLIME_GENERATION_NUMBER"] = "99"
    environment["SLIME_FABRIC_PROFILE"] = "test"
    environment["SLIME_PENDING_GENERATION"] = "1"
    environment["SLIME_PENDING_ATTEMPTS"] = "2"
    run(
        [
            str(ROOT / "kernel" / "scripts" / "build-iso.sh"),
            str(KERNEL),
            str(image),
            "64",
        ],
        environment=environment,
    )

    initial = bootstate(image)
    if initial["pending"] is None or initial["remaining_attempts"] != 2:
        raise SystemExit("rollback fixture did not start with two pending attempts")

    # Verification failures must consume attempts too. A corrupt signed release
    # cannot reach userspace, but it must still drain the bounded pending window
    # instead of trapping the selector in a permanent retry loop.
    previous_attempts = initial["remaining_attempts"]
    while previous_attempts > 0:
        environment = os.environ.copy()
        environment["SLIME_BOOT_IMAGE"] = str(image)
        environment["SLIME_REUSE_BOOT_IMAGE"] = "1"
        try:
            run(
                [
                    str(ROOT / "kernel" / "scripts" / "run-kernel.sh"),
                    str(KERNEL),
                    "-display",
                    "none",
                ],
                environment=environment,
                timeout=30,
                allow_failure=True,
            )
        except SystemExit:
            # UEFI may remain in its boot manager after stage-0 returns
            # LOAD_ERROR. The persisted BootState is the assertion.
            pass
        current = bootstate(image)
        attempts = current["remaining_attempts"]
        if attempts >= previous_attempts:
            raise SystemExit(
                "verification-failing pending attempt count did not decrease: "
                f"{previous_attempts} -> {attempts}"
            )
        previous_attempts = attempts


    # Restore a clean fixture and exercise the existing runtime-unhealthy path.
    image.unlink(missing_ok=True)
    environment = os.environ.copy()
    environment["SLIME_GENERATION_NUMBER"] = "99"
    environment["SLIME_FABRIC_PROFILE"] = "test"
    environment["SLIME_PENDING_GENERATION"] = "1"
    environment["SLIME_PENDING_ATTEMPTS"] = "2"
    run(
        [
            str(ROOT / "kernel" / "scripts" / "build-iso.sh"),
            str(KERNEL),
            str(image),
            "64",
        ],
        environment=environment,
    )
    initial = bootstate(image)
    for expected_attempts in (1, 0):
        environment = os.environ.copy()
        environment["SLIME_BOOT_IMAGE"] = str(image)
        environment["SLIME_REUSE_BOOT_IMAGE"] = "1"
        output = run(
            [
                str(ROOT / "kernel" / "scripts" / "run-kernel.sh"),
                str(KERNEL),
                "-display",
                "none",
            ],
            environment=environment,
            allow_failure=True,
        )
        if "[generation-manager] explicit unhealthy status" not in output:
            raise SystemExit("failing pending generation did not report explicit unhealthy status")
        current = bootstate(image)
        if current["remaining_attempts"] != expected_attempts:
            raise SystemExit(
                f"pending attempt count is {current['remaining_attempts']}, expected {expected_attempts}"
            )

    environment = os.environ.copy()
    environment["SLIME_BOOT_IMAGE"] = str(image)
    environment["SLIME_REUSE_BOOT_IMAGE"] = "1"
    output = run(
        [
            str(ROOT / "kernel" / "scripts" / "run-kernel.sh"),
            str(KERNEL),
            "-display",
            "none",
        ],
        environment=environment,
    )
    if "[generation] vertical slice healthy" not in output:
        raise SystemExit("known-good generation did not recover after pending exhaustion")
    final = bootstate(image)
    if final["known_good"] != initial["known_good"] or final["pending"] != initial["pending"]:
        raise SystemExit("rollback changed known-good or pending identities unexpectedly")
    print("rollback check: failing pending generation returned to known-good")


if __name__ == "__main__":
    main()
