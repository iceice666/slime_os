#!/usr/bin/env python3

"""C7.7/B5 sample-plane check: the shared-buffer plane driven by real components.

`kernel/tests/sample_plane.rs` composes the same lifecycle in-harness, against
locally constructed tables and `u64` owner ids. This check is the live-path
counterpart: two separately spawned components, holding only capabilities the
generation grants them, move a payload larger than the kernel message bound
through the real `SYS_SHARED_BUFFER_*` syscalls. It is the arm that exercises
the rights gates, the loan's receiver binding, and reclamation through actual
task termination.

The transcript is order-sensitive: each denial must be observed before the
operation it guards succeeds, so a regression that silently permits a denied
operation fails here even if the happy path still completes.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import os

from harness import ROOT, run_qemu

MARKERS = [
    # Generation authority is validated before anything runs.
    "[generation] shared-buffer factory grants valid",
    # Lender: creation authority is object-specific, not a general buffer handle.
    "[sample-lender] factory is not a buffer",
    "[sample-lender] buffer created",
    "[sample-lender] payload written",
    # A loan requires an irreversibly sealed source region.
    "[sample-lender] unsealed loan denied",
    "[sample-lender] seal is irreversible",
    "[sample-lender] loan created",
    "[sample-lender] descriptor sent",
    # Receiver: exactly one control message crosses; the payload does not.
    "[sample-receiver] descriptor received",
    "[sample-receiver] malformed descriptor mapped nothing",
    "[sample-receiver] loaned bytes mapped",
    "[sample-receiver] loan stays read-only",
    "[sample-receiver] payload verified",
    "[sample-receiver] loan returned once",
    "[sample-receiver] done",
    # Lender reclaims only after the receiver settles: the creator cannot
    # reclaim pages while a valid loan is outstanding.
    "[sample-lender] receiver settled",
    "[sample-lender] released",
    "[sample-lender] done",
    "[init] sample plane complete",
]

FORBIDDEN = [
    "[sample-lender] fail:",
    "[sample-receiver] fail:",
]


def run() -> str:
    environment = os.environ.copy()
    # B11: this gate exercises verification scaffolding, so it selects the
    # boot profile that declares it. The product profile declares none.
    environment["SLIME_GENERATION_NUMBER"] = "10"
    environment["SLIME_FABRIC_PROFILE"] = "test"
    environment["SLIME_SAMPLE_PLANE_CHECK"] = "1"
    output = run_qemu(
        ["cargo", "run", "--release", "--", "-display", "none"],
        environment=environment,
        cwd=ROOT / "kernel",
        timeout=120,
        echo="on-error",
    )
    for forbidden in FORBIDDEN:
        if forbidden in output:
            print(output, end="")
            raise SystemExit(f"sample-plane component reported a failure: {forbidden}")
    cursor = 0
    for marker in MARKERS:
        position = output.find(marker, cursor)
        if position < 0:
            print(output, end="")
            raise SystemExit(f"sample-plane transcript is missing or out of order at: {marker}")
        cursor = position + len(marker)
    if "[generation] vertical slice healthy" not in output:
        print(output, end="")
        raise SystemExit("sample-plane boot did not report a healthy slice")
    return output


def main() -> None:
    output = run()
    for line in output.splitlines():
        if any(marker in line for marker in MARKERS):
            print(line)
    print("sample plane check: ok")


if __name__ == "__main__":
    main()
