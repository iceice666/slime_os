#!/usr/bin/env python3

"""C8.5 live QoS gate: bounded credit, matching, and explicit-time transitions."""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import os

from harness import ROOT, run_qemu

CHAINS: list[tuple[str, list[str]]] = [
    (
        "QoS matching precedes data",
        [
            "[fabric] QoS matched",
            "[fabric-publisher] inline samples published",
        ],
    ),
    (
        "bounded reliable retry terminates",
        [
            "[fabric] reliable retry accounted",
            "[fabric] QoS retry exhausted",
            "[fabric] stream plane complete",
        ],
    ),
    (
        "deadline boundary is explicit",
        [
            "[fabric-publisher-b] large sample published",
            "[fabric] QoS deadline missed",
            "[fabric-publisher-b] simulated time advanced",
        ],
    ),
    (
        "retained replay precedes expiry",
        [
            "[fabric] retained history offered to late subscriber",
            "[fabric] retained history replayed to late subscriber",
            "[fabric] QoS lifespan expired",
            "[fabric] retained history expired for late subscriber",
        ],
    ),
    (
        "lease drives liveliness",
        [
            "[fabric-publisher-b] large sample published",
            "[fabric] QoS liveliness lost",
            "[fabric-publisher-b] simulated time advanced",
        ],
    ),
    (
        "best effort remains loss-only",
        [
            "[fabric-subscriber-b] stalling on telemetry",
            "[fabric-subscriber-b] bounded loss reported",
            "[fabric-subscriber-b] done",
        ],
    ),
    (
        "retained history stays live through broker completion",
        [
            "[fabric] retained history expired for late subscriber",
            "[fabric] stream plane complete",
        ],
    ),
    (
        "time source closes only after acknowledgements",
        [
            "[fabric-publisher-b] large sample published",
            "[fabric-publisher-b] simulated time advanced",
            "[fabric-publisher-b] done",
            "[fabric] stream plane complete",
        ],
    ),
    (
        "QoS slice completes",
        [
            "[fabric] stream plane complete",
            "[init] fabric QoS complete",
            "[generation] vertical slice healthy",
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
    "[fabric] malformed sample rejected",
    "[fabric] malformed ack rejected",
    "[fabric] unmatched ack rejected",
    "[fabric] reject:",
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
    for source in FABRIC_SOURCES:
        text = source.read_text(encoding="utf-8")
        if "yield_now" in text:
            raise SystemExit(
                f"{source.relative_to(ROOT)} busy-polls; QoS must park in SYS_WAIT"
            )
        if "wait(" not in text:
            raise SystemExit(f"{source.relative_to(ROOT)} never parks in SYS_WAIT")


def check_bounded_retry_and_events(output: str) -> None:
    retries = output.count("[fabric] reliable retry accounted")
    if not 1 <= retries <= 8:
        print(output, end="")
        raise SystemExit(f"reliable retry accounting escaped its bound: {retries}")
    if output.count("[fabric] QoS retry exhausted") != 1:
        print(output, end="")
        raise SystemExit("retry exhaustion must be one distinct terminal event")
    for marker in (
        "[fabric] QoS matched",
        "[fabric] QoS deadline missed",
        "[fabric] QoS lifespan expired",
        "[fabric] QoS liveliness lost",
        "[fabric] QoS peer dead",
    ):
        if marker not in output:
            print(output, end="")
            raise SystemExit(f"missing distinct QoS event: {marker}")


def check_best_effort_has_no_retry_state() -> None:
    service = (ROOT / "components/bins/src/bin/fabric-service.rs").read_text(
        encoding="utf-8"
    )
    guard = "subscriber.qos.reliability as u32 != RELIABILITY_RELIABLE"
    retry = "subscriber.retry_count = subscriber.retry_count.saturating_add(1)"
    if guard not in service or service.find(guard) > service.find(retry):
        raise SystemExit("BEST_EFFORT can reach retry accounting")


def run() -> str:
    environment = os.environ.copy()
    environment["SLIME_GENERATION_NUMBER"] = "13"
    environment["SLIME_FABRIC_QOS_CHECK"] = "1"
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
            raise SystemExit(f"fabric QoS reported a failure: {forbidden}")
    for label, chain in CHAINS:
        cursor = 0
        for marker in chain:
            position = output.find(marker, cursor)
            if position < 0:
                print(output, end="")
                raise SystemExit(f"{label}: missing or out of order at: {marker}")
            cursor = position + len(marker)
    check_bounded_retry_and_events(output)
    return output


def main() -> None:
    check_no_busy_wait_shape()
    check_best_effort_has_no_retry_state()
    output = run()
    markers = [marker for _, chain in CHAINS for marker in chain]
    for line in output.splitlines():
        if any(marker in line for marker in markers):
            print(line)
    print("fabric QoS check: ok")


if __name__ == "__main__":
    main()
