#!/usr/bin/env python3

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

from boot_contracts import BOOTSTATE_SLOT_BYTES
from harness import BOOT_TIMEOUT_SECONDS, RELEASE_KERNEL, ROOT, load_script, run_qemu

KERNEL = RELEASE_KERNEL

CHECK_GENERATION = load_script("check_generation", "check-generation.py")
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


def main() -> None:
    image = Path(sys.argv[1] if len(sys.argv) > 1 else "/tmp/slime-os-rollback.img")
    image.unlink(missing_ok=True)

    environment = os.environ.copy()
    environment["SLIME_GENERATION_NUMBER"] = "99"
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
