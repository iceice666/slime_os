#!/usr/bin/env python3

"""C8.7 live bounded native-operation gate.

Asserts the operation-plane transcript, then boots the retained stream and call
profiles as independent vertical slices. The operation fault cannot share one
fabric-service instance with those mutually exclusive profiles; successful
post-fault boots are therefore the observable isolation boundary: the same
generation graph still provisions and runs each unrelated route.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import os

from harness import ROOT, run_qemu

CHAINS = [
    (
        # Provisioning: every declared operation edge gets its narrowed role
        # before any goal moves.
        "role provisioning",
        [
            "[fabric] operation roles provisioned",
            "[fabric] operation goal forwarded",
            "[fabric] operation accepted",
            "[fabric] operation result routed",
            "[fabric-op-client] success correlated",
        ],
    ),
    (
        # Feedback is a stream keyed to one operation and ordered within it.
        "ordered feedback",
        [
            "[fabric-op-server] feedback streamed",
            "[fabric] operation feedback routed",
            "[fabric-op-client] feedback ordered",
        ],
    ),
    (
        "server rejection",
        ["[fabric-op-server] goal rejected", "[fabric-op-client] rejection distinct"],
    ),
    (
        # Feedback emitted after the terminal result must never reach the client.
        # The operation entry is already freed when the result settled, so the
        # sample is refused by correlation lookup — there is no live operation for
        # it to belong to. That is a stronger guarantee than a phase check: the
        # state it would need is gone, not merely flagged.
        "terminal state closes the operation",
        [
            "[fabric-op-server] post-terminal feedback emitted",
            "[fabric] stale operation reply rejected",
            "[fabric-op-client] terminal state closed",
        ],
    ),
    (
        "duplicate goal suppression",
        [
            "[fabric] duplicate operation goal rejected",
            "[fabric-op-client] duplicate goal rejected",
        ],
    ),
    (
        # One terminal per operation: the server's second result is dropped.
        "single terminal per operation",
        [
            "[fabric-op-server] duplicate result emitted",
            "[fabric-op-client] single terminal enforced",
        ],
    ),
    (
        "retained result retrieval",
        [
            "[fabric] operation result retrieved",
            "[fabric-op-client] result retrieved",
            "[fabric-op-client] retained result claimed once",
        ],
    ),
    (
        # Two concurrent operations under different authority never cross.
        "concurrent isolation",
        ["[fabric-op-client-b] concurrent operation isolated"],
    ),
    (
        # Knowing an operation identity is not authority over it.
        "authority denial",
        [
            "[fabric] unauthorized operation result denied",
            "[fabric-op-client-b] unauthorized retrieval denied",
            "[fabric] unauthorized operation cancel denied",
            "[fabric-op-client-b] unauthorized cancel denied",
            "[fabric] client role authority denied",
            "[fabric-op-client-b] forged transport record denied",
        ],
    ),
    (
        "participant restart",
        [
            "[fabric-op-client-b] restart state retained",
            "[fabric] operation participant restarted",
            "[fabric-op-client-b] participant restart deterministic",
        ],
    ),
    (
        # Cancellation is a request the server answers; the terminal lands once.
        "cancellation race",
        [
            "[fabric] operation cancel requested",
            "[fabric-op-server] cancellation honoured",
            "[fabric-op-client-b] cancellation settled once",
        ],
    ),
    (
        # Expiry and timeout are driven only by the explicit time capability, and
        # one clock step settles both: `pump_time` sweeps operation deadlines
        # before retained expiry, so the timeout is emitted first even though the
        # client observes the expiry afterwards. The order below is that real
        # sequence rather than the order the client happens to assert them in.
        "bounded expiry and timeout",
        [
            "[fabric-op-time] bounded time advanced",
            "[fabric] operation timed out",
            "[fabric] operation result expired",
            "[fabric-op-client] timeout distinct",
            "[fabric-op-client] result expiry observed",
        ],
    ),
    (
        # Peer death settles client A's active operation.
        "peer death propagation",
        [
            "[fabric-op-server] injected peer death",
            "[fabric] operation peer death propagated",
            "[fabric-op-client] peer death distinct",
        ],
    ),
    (
        # A second generation-declared operation route stays live across the
        # primary server fault and exchanges a probe through the same broker.
        "unrelated operation route liveness",
        [
            "[fabric-op-server] injected peer death",
            "[fabric] operation peer death propagated",
            "[fabric] unrelated operation route live",
            "[fabric-op-client] unrelated operation route live",
        ],
    ),
    (
        "bounded reclamation",
        [
            "[init] fabric operation complete",
            "[fabric] operation state reclaimed",
            "[fabric] operation plane complete",
            "[generation] vertical slice healthy",
        ],
    ),
]

# Any of these means a participant or the broker reported its own failure, which
# no passing run may contain.
FORBIDDEN = [
    "[fabric] fail:",
    "[fabric-op] fail:",
    "goal executed twice",
    "cross-correlated",
]


UNRELATED_CHECKS = [
    (
        "stream",
        "SLIME_FABRIC_STREAM_CHECK",
        "12",
        [
            "[fabric] stream plane complete",
            "[init] fabric stream complete",
            "[generation] vertical slice healthy",
        ],
    ),
    (
        "call",
        "SLIME_FABRIC_CALL_CHECK",
        "14",
        [
            "[fabric] call plane complete",
            "[init] fabric call complete",
            "[generation] vertical slice healthy",
        ],
    ),
]


def run_profile(flag: str, generation: str) -> str:
    environment = os.environ.copy()
    environment["SLIME_GENERATION_NUMBER"] = generation
    environment[flag] = "1"
    return run_qemu(
        ["cargo", "run", "--release", "--", "-display", "none"],
        environment=environment,
        cwd=ROOT / "kernel",
        timeout=120,
        echo="on-error",
    )

def main() -> None:
    output = run_profile("SLIME_FABRIC_OPERATION_CHECK", "15")
    for marker in FORBIDDEN:
        if marker in output:
            print(output, end="")
            raise SystemExit(f"fabric operation reported failure: {marker}")
    for label, chain in CHAINS:
        cursor = 0
        for marker in chain:
            position = output.find(marker, cursor)
            if position < 0:
                print(output, end="")
                raise SystemExit(f"{label}: missing or out of order at {marker}")
            cursor = position + len(marker)
    # A goal must execute exactly once per correlated identity. The scenario
    # declares fourteen goal sends: two repeat an existing identity (client A's
    # operation 4 and restarted client B's operation 9), so exactly twelve may
    # reach the server. Equality catches both missing work and a duplicate or
    # refused record accidentally being forwarded.
    if output.count("[fabric] operation goal forwarded") != 12:
        print(output, end="")
        raise SystemExit("operation plane forwarded an unexpected goal count")
    for label, flag, generation, markers in UNRELATED_CHECKS:
        profile_output = run_profile(flag, generation)
        for marker in FORBIDDEN:
            if marker in profile_output:
                print(profile_output, end="")
                raise SystemExit(f"unrelated {label} route reported failure: {marker}")
        cursor = 0
        for marker in markers:
            position = profile_output.find(marker, cursor)
            if position < 0:
                print(profile_output, end="")
                raise SystemExit(f"unrelated {label} route missing or out of order: {marker}")
            cursor = position + len(marker)
        print(f"[fabric-operation-check] unrelated {label} route live")
    for line in output.splitlines():
        if any(marker in line for _, chain in CHAINS for marker in chain):
            print(line)
    print("fabric operation check: ok")


if __name__ == "__main__":
    main()
