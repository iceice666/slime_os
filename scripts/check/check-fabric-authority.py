#!/usr/bin/env python3

"""C8.3 fabric-authority check: attenuated endpoint provisioning on a live boot.

`just fabric_manifest_check` proves the generation *declares* a graph. This is
the arm that proves a running system *derives* authority from it: a real
userspace fabric service mints both halves of a route, hands each participant
one narrowed, non-transferable role through the kernel's `SYS_CAP_TRANSFER`
move, and refuses an undeclared edge.

Assertions are grouped into causal chains rather than one global order. The
three clients run concurrently, so their interleaving is a scheduling detail
and pinning it would make this gate fail on an unrelated scheduler change.
What is *not* a detail is the order within each chain: every denial must be
observed before the operation it guards succeeds, so a regression that widens a
role, leaks transfer authority, or authorizes an ungranted component fails here
even when the happy path still delivers a sample.

The three denials that matter:

  * a publisher cannot receive on its own route, and a subscriber cannot send;
  * neither can re-delegate its role or widen its own rights mask;
  * a component holding a real control endpoint and supplying the exact route
    name, direction, and type identity still gets nothing, because the graph
    declares no edge for it.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import os

from harness import ROOT, run_qemu

# Each chain is an ordered sequence that must appear in this order. Chains are
# independent of one another.
CHAINS: list[tuple[str, list[str]]] = [
    (
        "control plane start-up",
        [
            # Generation authority is validated before anything runs.
            "[generation] fabric control grants valid",
            # It parks instead of spinning while its clients start. Init spawns
            # the subscribers before the service, but the publishers after, so
            # its first sweep necessarily finds an unanswered set and this line
            # is reached.
            "[fabric] idle: parked on control endpoints",
        ],
    ),
    (
        "publisher provisioning and denials",
        [
            "[fabric-publisher] role requested",
            "[fabric] provisioned fabric-publisher telemetry publish",
            "[fabric-publisher] publish role received",
            # A publish role is one direction: no receive authority came with it.
            "[fabric-publisher] route receive denied",
            # And it is terminal: it cannot be handed on or widened.
            "[fabric-publisher] re-delegation denied",
            "[fabric-publisher] widening denied",
            # Only after every denial does the role do what it is for.
            "[fabric-publisher] inline samples published",
            "[fabric-publisher] done",
        ],
    ),
    (
        "subscriber provisioning and denials",
        [
            "[fabric-subscriber] role requested",
            "[fabric] provisioned fabric-subscriber telemetry subscribe",
            "[fabric-subscriber] subscribe role received",
            "[fabric-subscriber] route publish denied",
            "[fabric-subscriber] re-delegation denied",
            "[fabric-subscriber] inline and shared received",
            "[fabric-subscriber] done",
        ],
    ),
    (
        "ungranted component denial",
        [
            # Everything a naive registry would accept is present: a real
            # control endpoint and the exact route strings.
            "[fabric-intruder] exact route strings supplied",
            # It is refused anyway, because the graph declares no edge for it.
            "[fabric] ungranted component denied: fabric-intruder",
            "[fabric-intruder] undeclared edge denied",
            "[fabric-intruder] done",
        ],
    ),
    (
        "route carries data end to end",
        [
            "[fabric-publisher] inline samples published",
            "[fabric-subscriber] inline and shared received",
        ],
    ),
    (
        "every declared edge provisioned, then teardown",
        [
            # The service provisions every declared stream edge before brokering,
            # and an unprovisioned route endpoint is a failure there.
            "[fabric] every declared stream edge provisioned",
            "[fabric] stream plane complete",
            "[init] fabric authority complete",
        ],
    ),
]

FORBIDDEN = [
    "[fabric] fail:",
    "[fabric-publisher] fail:",
    "[fabric-publisher-b] fail:",
    "[fabric-subscriber] fail:",
    "[fabric-subscriber-b] fail:",
    "[fabric-intruder] fail:",
]

FABRIC_SOURCES = [
    ROOT / "components" / "bins" / "src" / "bin" / name
    for name in (
        "fabric-service.rs",
        "fabric-publisher.rs",
        "fabric-publisher-b.rs",
        "fabric-subscriber.rs",
        "fabric-subscriber-b.rs",
        "fabric-intruder.rs",
    )
]


def check_no_busy_wait_shape() -> None:
    """Lint the two source shapes that would busy-wait, and say what that proves.

    "Consumes no CPU through a poll/yield loop" cannot be read off a transcript:
    a spinning service still reaches every marker, just slower. So it is linted
    at the source instead.

    This is a necessary condition, not a proof. `yield_now` is the obvious way
    to re-enter the run queue without blocking, and its absence rules that out.
    But `slime_rt::wait` also returns immediately whenever any source is already
    ready — `task::wait` returns without parking when `source_ready` holds, and
    an endpoint counts as ready once its peer is dead — so a loop that keeps
    waiting on a dead peer would spin while passing both greps.

    Nothing mechanical here excludes that; what does is the fabric's own loop
    shape, which retires a client on `ERR_PEER_DEAD` before parking again, so no
    dead source is ever left in the wait set. That reasoning lives in
    `fabric-service.rs` and is reviewed, not tested.
    """
    for source in FABRIC_SOURCES:
        text = source.read_text(encoding="utf-8")
        if "yield_now" in text:
            raise SystemExit(
                f"{source.relative_to(ROOT)} busy-polls; the fabric must park in SYS_WAIT"
            )
        if "wait(" not in text:
            raise SystemExit(f"{source.relative_to(ROOT)} never parks in SYS_WAIT")


def run() -> str:
    environment = os.environ.copy()
    environment["SLIME_GENERATION_NUMBER"] = "11"
    environment["SLIME_FABRIC_AUTHORITY_CHECK"] = "1"
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
            raise SystemExit(f"fabric component reported a failure: {forbidden}")
    for label, chain in CHAINS:
        cursor = 0
        for marker in chain:
            position = output.find(marker, cursor)
            if position < 0:
                print(output, end="")
                raise SystemExit(f"{label}: missing or out of order at: {marker}")
            cursor = position + len(marker)
    if "[generation] vertical slice healthy" not in output:
        print(output, end="")
        raise SystemExit("fabric-authority boot did not report a healthy slice")
    return output


def main() -> None:
    check_no_busy_wait_shape()
    output = run()
    markers = [marker for _, chain in CHAINS for marker in chain]
    for line in output.splitlines():
        if any(marker in line for marker in markers):
            print(line)
    print("fabric authority check: ok")


if __name__ == "__main__":
    main()
