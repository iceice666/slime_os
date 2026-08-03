#!/usr/bin/env python3
# P2.2: AArch64 exception vectors, synchronous-fault decoding, and `svc` entry.
#
# Boots the verified `aarch64-qemu-virt` generation through the same pinned
# launcher as P2.1, then requires live evidence that EL1 and EL0 synchronous
# exceptions entered the installed vector table, an `svc #0` used the documented
# register mapping and the shared syscall dispatcher, the complete mutable frame
# survived `eret`, and DAIF masking restored its entry state.

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import re
import sys
import tempfile

from harness import load_script

BOOT = load_script("aarch64_trap_boot", "check/check-aarch64-boot.py")

REQUIRED_MARKERS = (
    r"\[bootstate-trace\] v1 action=boot-known-good",
    r"\[stage0\] generation and kernel verified",
    r"\[serial\] Slime OS aarch64-qemu-virt bring-up",
    r"\[serial\] exception level EL1",
    r"\[bringup\] aarch64 EL1 vertical slice reached",
    r"\[aarch64-trap\] vectors installed vbar=0x[0-9a-f]+",
    r"\[aarch64-trap\] daif entry_masked=true enabled_window=true masked_inside=true "
    r"restored_enabled=true final_masked=true",
    r"\[aarch64-trap\] el1 sync ec=0x3c reason=UndefinedOp elr=0x[0-9a-f]+",
    r"\[aarch64-trap\] svc nr=1 args=0x1111111111111111,0x2222222222222222,"
    r"0x3333333333333333,0x4444444444444444,5 result=-4",
    r"\[aarch64-trap\] el0 sync ec=0x3c reason=UndefinedOp elr=0x400004",
    r"\[aarch64-trap\] frame restored gprs=31 sp=0x402000 "
    r"handler_mutation=0x4d55544154454432",
    r"\[aarch64-trap\] complete",
)

FAILURE_MARKERS = (
    r"\[stage0\] boot failed",
    r"Synchronous Exception",
    r"\[kernel fault\]",
    r"\[aarch64-trap\] failed",
    r"\[panic\]",
)


def fail(message: str) -> None:
    raise SystemExit(f"aarch64 trap check: {message}")


def check_transcript(transcript: str) -> None:
    for pattern in FAILURE_MARKERS:
        match = re.search(pattern, transcript)
        if match:
            line = next(
                (line for line in transcript.splitlines() if match.group(0) in line),
                match.group(0),
            )
            fail(f"boot reported a failure: {line.strip()}")

    position = 0
    for pattern in REQUIRED_MARKERS:
        match = re.compile(pattern).search(transcript, position)
        if match is None:
            if re.search(pattern, transcript):
                fail(f"marker out of order: {pattern}")
            fail(f"missing expected trap marker: {pattern}")
        position = match.end()


def main() -> None:
    code, variables = BOOT.firmware()
    for path in (code, variables):
        if not path.is_file():
            fail(f"AArch64 UEFI firmware missing: {path}")

    with tempfile.TemporaryDirectory(prefix="slime-aarch64-trap.") as directory:
        work = _Path(directory)
        generation_dir = work / "generation"
        image = work / "slime-aarch64.img"
        BOOT.os.environ["SLIME_AARCH64_TRAP_CHECK"] = "1"
        BOOT.build_artifacts(generation_dir, image)
        transcript, returncode = BOOT.boot(image, code, variables, work / "boot")

        sys.stdout.write(transcript)
        check_transcript(transcript)
        if returncode < 0:
            fail(f"QEMU terminated on signal {-returncode}")
        if returncode != BOOT.EXPECTED_EXIT_STATUS:
            fail(
                f"boot exited with status {returncode}, expected "
                f"{BOOT.EXPECTED_EXIT_STATUS} from a semihosting exit"
            )

    print(
        "aarch64 trap check: vectors installed; EL1/EL0 faults decoded; svc register "
        "mapping, shared dispatch, mutable frame restore, and DAIF state observed"
    )


if __name__ == "__main__":
    main()
