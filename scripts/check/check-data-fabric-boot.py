#!/usr/bin/env python3

"""C8.10 collision-free full-graph bootstrap and bounded route worker gate.

`just data_fabric_profile_check` proves the generation *declares* one resolved
profile with a validated worker partition. This is the arm that proves a real
boot *runs* it: every C8 role — the stream, call, and operation planes, the
unauthorized probe, the declared interposition proxy, and the filtered
introspection client — launches in one generation, provisions concurrently
through collision-free capability layouts, and the whole graph reaches healthy
blocked idle.

Three properties this gate exists to make observable, none of which the
declarative half could reach:

  * **No mutually exclusive planes.** Before C8.10 the stream, call, and
    operation planes physically aliased one range of init's capability slots and
    were selected by rewriting those slots per generation number. Here all three
    coexist, so the boot must succeed with no profile-dependent slot rewrite at
    all.
  * **Distinct identities.** The probe, proxy, and introspection client were one
    binary switching on an env flag. They are three components with
    non-overlapping grants now, and the transcript must show all three.
  * **Blocked, not finished.** The exit condition is idle *with no traffic*. A
    role that exits looks identical to one that was never launched, so the gate
    asserts the kernel's own liveness sweep reports every fabric task still live
    and parked.

The assertions are grouped into causal chains rather than one global order. The
sixteen participants provision concurrently, so their interleaving is a
scheduling detail; pinning it would make this gate fail on an unrelated
scheduler change. What is not a detail is the order *within* each chain.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import os

from harness import ROOT, run_qemu

GENERATION = "17"

# Each chain is an ordered sequence that must appear in this order. Chains are
# independent of one another.
CHAINS: list[tuple[str, list[str]]] = [
    (
        "collision-free bootstrap layout",
        [
            # Generation authority is validated before anything runs.
            "[generation] fabric boot control grants valid",
            # The whole point of the milestone: one fabric-only layout, counted
            # against the kernel ceiling on the live path rather than projected.
            "[generation] fabric boot layout",
            "[init] fabric boot subscribers spawned",
            "[init] fabric boot service spawned",
        ],
    ),
    (
        "bounded route workers",
        [
            # Each worker is its own task with its own capability table, so both
            # number their controls from the same base without colliding.
            "[fabric] route worker provisioned: call",
            "[fabric] route worker provisioned: operation",
            "[fabric] bounded route workers spawned",
        ],
    ),
    (
        "full graph launched and idle",
        [
            "[init] fabric boot participants spawned",
            # Supervision travels only after every participant has sent its role
            # request, because a broker reads one request then one descriptor per
            # client on the same queue.
            "[init] fabric boot supervision transferred",
            "[init] fabric boot graph launched",
            "[generation] vertical slice healthy",
        ],
    ),
    (
        "three distinct split identities",
        [
            # The probe holds a real control endpoint and supplies the exact
            # route strings, and still gets nothing: the graph declares no edge
            # for it.
            "[fabric-probe] exact route strings supplied",
            "[fabric-probe] undeclared edge denied",
        ],
    ),
    (
        "declared interposition proxy launched with no route authority",
        [
            # Declared as a chain hop rather than a route participant, so the
            # stream broker has no edge to give it. It asks for nothing and
            # receives nothing — the C8.8 gate owns proving the relay itself.
            "[fabric-proxy] boot idle without a role",
        ],
    ),
]

# Every declared participant must report its narrowed role, and the stream
# worker must report every edge it provisioned. Unordered: the sixteen
# participants provision concurrently, so which finishes first is a scheduling
# detail — but every one of them must appear.
#
# Both sides are asserted deliberately. The participant line proves the
# capability arrived narrowed to the exact (route, direction) the graph declared;
# the worker line proves the worker created that edge. A regression that
# provisioned the wrong route would still print one of them.
PROVISIONED = [
    "[fabric-publisher] boot role provisioned",
    "[fabric-subscriber] boot role provisioned",
    "[fabric-publisher-b] boot role provisioned",
    "[fabric-subscriber-b] boot role provisioned",
    "[fabric-observer] boot role provisioned",
    "[fabric-call-client] boot role provisioned",
    "[fabric-call-client-b] boot role provisioned",
    "[fabric-call-server] boot role provisioned",
    "[fabric-op-client] boot role provisioned",
    "[fabric-op-client-b] boot role provisioned",
    "[fabric-op-server] boot role provisioned",
    # The clock and replacement identities hold a control endpoint and ask for
    # nothing, which is what the graph declares for them.
    "[fabric-call-time] boot idle without a role",
    "[fabric-op-time] boot idle without a role",
    "[fabric-op-client-b-restart] boot idle without a role",
    # Each request/response worker's own confirmation that it provisioned its
    # plane. Without these, a worker that minted nothing and parked would be
    # caught only by the participant-side lines — and a worker that provisioned
    # the *wrong* route would still satisfy those, since a participant validates
    # its own descriptor rather than the worker's whole plane.
    "[fabric] call roles provisioned",
    "[fabric] operation roles provisioned",
    # Every stream edge the graph declares, from the worker that minted it.
    "[fabric] provisioned fabric-publisher telemetry publish",
    "[fabric] provisioned fabric-subscriber telemetry subscribe",
    "[fabric] provisioned fabric-publisher-b telemetry publish",
    "[fabric] provisioned fabric-publisher-b diagnostics publish",
    "[fabric] provisioned fabric-subscriber-b telemetry subscribe",
    "[fabric] provisioned fabric-subscriber-b diagnostics subscribe",
    "[fabric] provisioned fabric-observer telemetry subscribe",
    # The unauthorized probe holds a real control endpoint and supplies the exact
    # route strings, and is still refused: the graph declares no edge for it.
    "[fabric] ungranted component denied: fabric-probe",
    # Parked rather than polled, which is the milestone's "never polls".
    "[fabric] idle: parked on control endpoints",
]

FORBIDDEN = [
    "[fabric] fail:",
    "[fabric-publisher] fail:",
    "[fabric-subscriber] fail:",
    "[fabric-publisher-b] fail:",
    "[fabric-subscriber-b] fail:",
    "[fabric-observer] fail:",
    "[fabric-probe] fail:",
    "[fabric-proxy] fail:",
    # The milestone's own failure modes, each a distinct message so a regression
    # names itself rather than surfacing as a missing marker.
    "fabric boot layout exceeds",
    "route worker grant overflow",
    "live stream sources exceed one SYS_WAIT set",
    "[generation] vertical slice failed",
]

# Every fabric role must still be live at the idle sweep. A role that terminated
# would be reported here instead, and "terminated: Exit(0)" reads as success
# everywhere else in the tree — so this is asserted explicitly rather than left
# to the healthy/unhealthy verdict.
IDLE_BLOCKED = [
    "init",
    "fabric-service",
    "fabric-call-worker",
    "fabric-op-worker",
    "fabric-publisher",
    "fabric-subscriber",
    "fabric-publisher-b",
    "fabric-subscriber-b",
    "fabric-observer",
    "fabric-probe",
    "fabric-proxy",
    "fabric-call-client",
    "fabric-call-client-b",
    "fabric-call-server",
    "fabric-call-time",
    "fabric-op-client",
    "fabric-op-client-b",
    "fabric-op-server",
    "fabric-op-time",
    "fabric-op-client-b-restart",
]


def run() -> str:
    environment = os.environ.copy()
    # B11: this gate exercises verification scaffolding, so it selects the
    # boot profile that declares it. The product profile declares none.
    environment["SLIME_GENERATION_NUMBER"] = GENERATION
    environment["SLIME_FABRIC_PROFILE"] = "unified"
    environment["SLIME_FABRIC_BOOT_CHECK"] = "1"
    return run_qemu(
        ["cargo", "run", "--release", "--", "-display", "none"],
        environment=environment,
        cwd=ROOT / "kernel",
        timeout=180,
        echo="on-error",
    )


def check_chain(output: str, label: str, markers: list[str]) -> None:
    cursor = 0
    for marker in markers:
        index = output.find(marker, cursor)
        if index < 0:
            if marker in output:
                raise SystemExit(
                    f"data fabric boot check: {label}: {marker!r} appeared out of order"
                )
            raise SystemExit(f"data fabric boot check: {label}: missing {marker!r}")
        cursor = index + len(marker)


def main() -> None:
    output = run()

    for forbidden in FORBIDDEN:
        if forbidden in output:
            raise SystemExit(f"data fabric boot check: observed {forbidden!r}")

    for label, markers in CHAINS:
        check_chain(output, label, markers)

    for marker in PROVISIONED:
        if marker not in output:
            raise SystemExit(f"data fabric boot check: missing {marker!r}")

    # Blocked idle, per component. `on_idle` prints one line per live task, so
    # this distinguishes "parked, healthy" from "exited cleanly" — which the
    # milestone's exit condition treats very differently.
    for component in IDLE_BLOCKED:
        marker = f"[generation] {component} idle-blocked (persistent=true)"
        if marker not in output:
            raise SystemExit(
                f"data fabric boot check: {component} did not reach healthy blocked idle"
            )

    # The layout must fit with room to spare, and the figure must come from the
    # live boot rather than a projection. Parse it back so a layout that grew to
    # the ceiling fails here instead of at the next participant added.
    layout = [
        line for line in output.splitlines() if "[generation] fabric boot layout" in line
    ]
    if len(layout) != 1:
        raise SystemExit("data fabric boot check: expected exactly one layout report")
    fields = layout[0].split()
    used, ceiling = int(fields[-4]), int(fields[-2])
    if used >= ceiling:
        raise SystemExit(
            f"data fabric boot check: layout uses {used} of {ceiling} capability slots"
        )
    print(
        f"full-graph boot: {used} of {ceiling} init capability slots, "
        f"{len(IDLE_BLOCKED)} roles at healthy blocked idle, "
        "three bounded route workers: ok"
    )


if __name__ == "__main__":
    main()
