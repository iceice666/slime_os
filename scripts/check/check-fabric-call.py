#!/usr/bin/env python3

"""C8.6 live bounded native-call gate."""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import os

from harness import ROOT, run_qemu

CHAINS = [
    (
        "successful correlation",
        [
            "[fabric] call roles provisioned",
            "[fabric] call forwarded",
            "[fabric-call-server] non-idempotent execution once",
            "[fabric] call reply correlated",
            "[fabric-call-client] success correlated",
        ],
    ),
    (
        "shared request and reply",
        [
            "[fabric-call-server] shared request verified",
            "[fabric-call-client] shared reply verified",
        ],
    ),
    (
        "server rejection",
        ["[fabric] server rejection routed", "[fabric-call-client] rejection distinct"],
    ),
    (
        "malformed reply",
        [
            "[fabric] malformed call reply rejected",
            "[fabric-call-client] malformed reply distinct",
        ],
    ),
    (
        "duplicate and cancellation",
        [
            "[fabric] duplicate call rejected",
            "[fabric-call-client-b] duplicate rejected",
            "[fabric] call cancellation forwarded",
            "[fabric-call-server] cancellation settled",
            "[fabric] call cancelled",
            "[fabric-call-client-b] cancellation observed",
        ],
    ),
    (
        "stale session",
        ["[fabric] stale call rejected", "[fabric-call-client-b] stale session observed"],
    ),
    (
        "terminal backpressure",
        [
            "[fabric] terminal delivery queued",
            "[fabric-call-client-b] terminal backpressure recovered",
        ],
    ),
    (
        "bounded terminal outcomes",
        [
            "[fabric] call timed out",
            "[fabric-call-client] timeout distinct",
            "[fabric] call retry exhausted",
            "[fabric-call-client] retry exhaustion distinct",
        ],
    ),
    (
        "peer death isolation",
        [
            "[fabric-call-client-b] unrelated route intact",
            "[fabric-call-server] injected peer death",
            "[fabric] call peer death propagated",
            "[fabric-call-client] peer death distinct",
        ],
    ),
    (
        "bounded reclamation",
        [
            "[fabric] call state reclaimed",
            "[fabric] call plane complete",
            "[init] fabric call complete",
            "[generation] vertical slice healthy",
        ],
    ),
]

FORBIDDEN = [
    "[fabric] fail:",
    "[fabric-call] fail:",
    "executed twice",
    "call route missing",
]


def main() -> None:
    environment = os.environ.copy()
    # B11: this gate exercises verification scaffolding, so it selects the
    # boot profile that declares it. The product profile declares none.
    environment["SLIME_GENERATION_NUMBER"] = "14"
    environment["SLIME_FABRIC_PROFILE"] = "test"
    environment["SLIME_FABRIC_CALL_CHECK"] = "1"
    output = run_qemu(
        ["cargo", "run", "--release", "--", "-display", "none"],
        environment=environment,
        cwd=ROOT / "kernel",
        timeout=120,
        echo="on-error",
    )
    for marker in FORBIDDEN:
        if marker in output:
            print(output, end="")
            raise SystemExit(f"fabric call reported failure: {marker}")
    for label, chain in CHAINS:
        cursor = 0
        for marker in chain:
            position = output.find(marker, cursor)
            if position < 0:
                print(output, end="")
                raise SystemExit(f"{label}: missing or out of order at {marker}")
            cursor = position + len(marker)
    if output.count("[fabric-call-server] non-idempotent execution once") != 1:
        print(output, end="")
        raise SystemExit("non-idempotent request did not execute exactly once")
    for line in output.splitlines():
        if any(marker in line for _, chain in CHAINS for marker in chain):
            print(line)
    print("fabric call check: ok")


if __name__ == "__main__":
    main()
