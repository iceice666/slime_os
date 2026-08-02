#!/usr/bin/env python3

"""C8.4 fabric-stream check: bounded many-to-many streams on a live boot.

`just fabric_authority_check` proves a running system derives route *authority*
from the generation graph. This is the arm that proves it then carries *data*
under that authority: two publishers and two subscribers exchange both inline
and `>MAX_MSG` samples over two declared routes, and every bound the milestone
names is observed rather than argued.

Assertions are grouped into causal chains rather than one global order. Five
components run concurrently, so their interleaving is a scheduling detail and
pinning it would make this gate fail on an unrelated scheduler change. What is
*not* a detail is the order within each chain: a bound must be observed before
the operation that depends on it, so a regression that unbounds a queue, copies
a payload twice, or lets one participant's fault reach another route fails here
even when the happy path still delivers a sample.

The four properties that matter:

  * two publishers and two subscribers exchange inline and shared samples, and
    no participant obtains authority over another route;
  * KEEP_LAST evicts at the declared depth and a stalled BEST_EFFORT subscriber
    is told exactly what it lost, once, with no retry state;
  * one large sample incurs one fabric copy and one quota-charged
    receiver-bound loan per subscriber;
  * a stalled participant does not disturb an unrelated stream.
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
            # Every declared stream edge is provisioned before any sample moves:
            # a route that carried data before its role was minted would mean
            # the graph was not the thing granting authority.
            "[fabric] every declared stream edge provisioned",
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
        "second publisher spans two routes",
        [
            "[fabric-publisher-b] roles requested",
            "[fabric] provisioned fabric-publisher-b telemetry publish",
            "[fabric] provisioned fabric-publisher-b diagnostics publish",
            # Two declared routes arrive as two distinct capabilities; the
            # component fails if they collapse into one.
            "[fabric-publisher-b] both publish roles received",
            "[fabric-publisher-b] diagnostics sample published",
            "[fabric-publisher-b] large sample published",
            "[fabric-publisher-b] done",
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
            # Both sample forms reach a keeping-up subscriber, and it is never
            # told it lost anything.
            "[fabric-subscriber] shared sample verified",
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
        "one copy per large sample, one loan per subscriber",
        [
            # The fabric copies a >MAX_MSG payload exactly once into its own
            # sealed buffer...
            "[fabric] large sample copied once",
            # ...and each subscriber then verifies the payload through its own
            # independently accounted downstream loan.
            "[fabric-subscriber] shared sample verified",
        ],
    ),
    (
        "stalled BEST_EFFORT subscriber reports bounded loss",
        [
            "[fabric-subscriber-b] both subscribe roles received",
            # It consumes, then deliberately stops acking.
            "[fabric-subscriber-b] stalling on telemetry",
            # The stall costs a bounded number of retained samples, and resuming
            # produces exactly one loss report naming what was dropped.
            "[fabric-subscriber-b] bounded loss reported",
            "[fabric-subscriber-b] done",
        ],
    ),
    (
        "one participant's stall does not disturb an unrelated stream",
        [
            "[fabric-subscriber-b] stalling on telemetry",
            # A different route with a different interface keeps delivering
            # while telemetry is stalled.
            "[fabric-subscriber-b] diagnostics unaffected by stall",
        ],
    ),
    (
        "stream plane completes and tears down",
        [
            "[fabric] every declared stream edge provisioned",
            "[fabric] stream plane complete",
            "[init] fabric stream complete",
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
    # A malformed record must never reach a subscriber, and no component under
    # test hands the fabric one: every rejection marker below names a real
    # defect rather than a tolerated refusal. `reject:` covers the whole
    # validation surface, so a new refusal path fails this gate by default
    # rather than passing unnoticed.
    "[fabric] malformed sample rejected",
    "[fabric] malformed ack rejected",
    "[fabric] unmatched ack rejected",
    "[fabric] reject:",
]

# The most `SAMPLE_LOST` events the declared graph can justify: the two
# telemetry publishers send a fixed number of samples between them, so a report
# per drop is the ceiling. A fabric that retried instead of reporting, or that
# reported per delivery attempt, exceeds it.
MAX_LOSS_REPORTS = 16

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
    ready — an endpoint counts as ready once its peer is dead — so a loop that
    keeps waiting on a dead peer would spin while passing both greps.

    Nothing mechanical here excludes that; what does is the fabric's own loop
    shape, which retires a finished publisher and a dead subscriber before
    parking again, so no dead source is ever left in the wait set. That
    reasoning lives in `fabric-service.rs` and is reviewed, not tested.
    """
    for source in FABRIC_SOURCES:
        text = source.read_text(encoding="utf-8")
        if "yield_now" in text:
            raise SystemExit(
                f"{source.relative_to(ROOT)} busy-polls; the fabric must park in SYS_WAIT"
            )
        if "wait(" not in text:
            raise SystemExit(f"{source.relative_to(ROOT)} never parks in SYS_WAIT")


def check_one_copy_per_large_sample(output: str) -> None:
    """One admitted large sample means exactly one fabric payload copy.

    The transcript marker is emitted once per copy, so counting it is a direct
    measurement of the milestone's "at most once" rule rather than an inference
    from the delivery markers. `fabric-publisher-b` publishes exactly one large
    sample, and two subscribers are matched on its route: a fabric that copied
    per subscriber instead of per sample would print this twice.
    """
    copies = output.count("[fabric] large sample copied once")
    if copies != 1:
        print(output, end="")
        raise SystemExit(
            f"one large sample must incur exactly one fabric copy; observed {copies}"
        )
    # One quota-charged receiver-bound loan per matched subscriber. Counted at
    # creation rather than at verification: the stalled subscriber may evict
    # this sample under KEEP_LAST before reading it, which is the declared
    # BEST_EFFORT outcome — but the loan was still created, charged, and
    # settled, and that is what "one loan per subscriber" means.
    loans = output.count("[fabric] downstream loan created")
    if loans != 2:
        print(output, end="")
        raise SystemExit(
            f"one copy must be loaned to each matched subscriber; observed {loans}"
        )
    # At least one subscriber must actually read the shared payload back, or
    # the copy would be unobserved and the fan-out unproven.
    if "shared sample verified" not in output:
        print(output, end="")
        raise SystemExit("no subscriber verified the shared payload")


def check_loss_is_bounded(output: str) -> None:
    """A stall costs a bounded, reported number of samples — never a retry.

    The subscriber prints one marker per `SAMPLE_LOST` event, so counting them
    measures reporting growth directly: a fabric that reported per delivery
    attempt instead of per drain would emit a series bounded only by how long
    the stall lasted. `MAX_REPORTS` is what the two telemetry publishers could
    ever justify between them.

    At least one report is also required. Without it the stall would be free,
    and the milestone's "reports bounded loss" arm would hold vacuously.
    """
    reports = output.count("[fabric-subscriber-b] bounded loss reported")
    if reports == 0:
        print(output, end="")
        raise SystemExit("the stall produced no loss report")
    if reports > MAX_LOSS_REPORTS:
        print(output, end="")
        raise SystemExit(
            f"loss reporting grew past its bound: {reports} > {MAX_LOSS_REPORTS}"
        )


def run() -> str:
    environment = os.environ.copy()
    # B11: this gate exercises verification scaffolding, so it selects the
    # boot profile that declares it. The product profile declares none.
    environment["SLIME_GENERATION_NUMBER"] = "12"
    environment["SLIME_FABRIC_PROFILE"] = "test"
    environment["SLIME_FABRIC_STREAM_CHECK"] = "1"
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
            raise SystemExit(f"fabric stream reported a failure: {forbidden}")
    for label, chain in CHAINS:
        cursor = 0
        for marker in chain:
            position = output.find(marker, cursor)
            if position < 0:
                print(output, end="")
                raise SystemExit(f"{label}: missing or out of order at: {marker}")
            cursor = position + len(marker)
    check_one_copy_per_large_sample(output)
    check_loss_is_bounded(output)
    if "[generation] vertical slice healthy" not in output:
        print(output, end="")
        raise SystemExit("fabric-stream boot did not report a healthy slice")
    return output


def main() -> None:
    check_no_busy_wait_shape()
    output = run()
    markers = [marker for _, chain in CHAINS for marker in chain]
    for line in output.splitlines():
        if any(marker in line for marker in markers):
            print(line)
    print("fabric stream check: ok")


if __name__ == "__main__":
    main()
