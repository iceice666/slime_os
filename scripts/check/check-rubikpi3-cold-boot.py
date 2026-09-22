#!/usr/bin/env python3

"""Judge the recorded Rubik Pi 3 cold boots.

This gate cannot perform a boot. The board is power-cycled by hand, the payload
is handed over by `kexec` from the installed Linux, and the console transcript
is captured off-box; what lands here is the resulting record. So the gate first
proves it can *refuse* — a synthetic record is mutated one invariant at a time
and each mutation must be rejected — and only then reports the committed
observation. A gate that merely parsed its own evidence would assert nothing.

What the record has to establish, and what each mutation attacks:

* two boots, each a cold start, each reaching the same terminal marker;
* both boots from one image, whose hash matches the packaged image on disk;
* the ordered marker chain, so a transcript missing the timer phase or the
  terminal line is not a boot;
* semantic equality between boots, with the sampled fields -- the timer's poll
  count and its raw counter readings -- exempted, because those legitimately
  differ per boot and demanding equality would make the gate a liar.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
import copy
import hashlib
import json
import re
from collections.abc import Callable
from typing import NoReturn

from harness import ROOT

RECORD = ROOT / "evidence" / "rubikpi3-cold-boot" / "observation.json"
IMAGE = ROOT / "build" / "rubikpi3" / "slime-sel4-rubikpi3.img"
PROFILE = "aarch64-sel4-rubikpi3"
COMPOSITION = "sel4-rubikpi3"
REQUIRED_BOOTS = 2
TERMINAL = "SLIME_ROOT READY"

#: The chain every boot must emit, in order. Prefixes rather than whole lines:
#: the lines carry per-boot counters, and pinning those would forbid a second
#: boot from ever passing.
CHAIN = (
    "Booting all finished, dropped to user space",
    "SLIME_TIMER acquired",
    "SLIME_TIMER delivered",
    "SLIME_TIMER serviced",
    "SLIME_TIMER advanced",
    "SLIME_TIMER OK",
    TERMINAL,
)

#: Fields inside a record line that are sampled per boot rather than semantic.
#: `polls` counts how many times the root spun waiting for the tick, and the
#: counter readings are raw hardware time; both differ between boots on correct
#: hardware. Everything else in the chain must match exactly.
SAMPLED = re.compile(r"\b(polls|start|end|delta)=\d+")


def fail(message: str) -> NoReturn:
    raise SystemExit(f"rubikpi3 cold boot check: {message}")


def normalized(line: str) -> str:
    return SAMPLED.sub(lambda m: f"{m.group(1)}=<sampled>", line)


def validate(record: dict, image_sha: str | None) -> None:
    for key in (
        "formatVersion",
        "machine",
        "soc",
        "targetProfile",
        "composition",
        "imageSha256",
        "imageBytes",
        "boots",
        "limits",
    ):
        if key not in record:
            fail(f"record is missing {key!r}")
    if record["targetProfile"] != PROFILE:
        fail(f"record names target profile {record['targetProfile']!r}, expected {PROFILE!r}")
    if record["composition"] != COMPOSITION:
        fail(f"record names composition {record['composition']!r}, expected {COMPOSITION!r}")
    if not record["limits"]:
        fail("record states no limits; a physical claim without stated limits is not one")
    if image_sha is not None and record["imageSha256"] != image_sha:
        fail(
            f"record names image {record['imageSha256']} but the packaged image is {image_sha}; "
            "rebuild with `just sel4_rubikpi3_image_check` or re-record the boots"
        )

    boots = record["boots"]
    if len(boots) != REQUIRED_BOOTS:
        fail(f"record carries {len(boots)} boot(s), expected exactly {REQUIRED_BOOTS}")
    if sorted(boot.get("index") for boot in boots) != list(range(REQUIRED_BOOTS)):
        fail("boots are not indexed 0..n-1 exactly once")

    chains = []
    for boot in boots:
        index = boot.get("index")
        if boot.get("outcome") != "ready":
            fail(f"boot {index} did not reach ready")
        if not boot.get("coldStart"):
            fail(f"boot {index} was not a cold start")
        if boot.get("targetProfile") != PROFILE:
            fail(f"boot {index} names target profile {boot.get('targetProfile')!r}")
        if not isinstance(boot.get("secondsToReady"), (int, float)):
            fail(f"boot {index} records no time to ready")
        lines = boot.get("recordLines") or []
        if len(lines) != len(CHAIN):
            fail(f"boot {index} records {len(lines)} marker line(s), expected {len(CHAIN)}")
        for expected, actual in zip(CHAIN, lines, strict=True):
            if not actual.startswith(expected):
                fail(f"boot {index}: expected a line starting {expected!r}, found {actual!r}")
        chains.append([normalized(line) for line in lines])

    if chains[0] != chains[1]:
        differing = [a for a, b in zip(*chains, strict=True) if a != b]
        fail(f"the two boots disagree on semantic marker content: {differing}")


def command_controls() -> None:
    """Prove each invariant is checked, by mutating a known-good record."""
    good = json.loads(RECORD.read_text(encoding="utf-8"))
    validate(good, None)
    print("rubikpi3 cold boot control: the committed record is accepted")

    cases: list[tuple[str, dict]] = []

    def mutate(label: str, apply: Callable[[dict], object]) -> None:
        candidate = copy.deepcopy(good)
        apply(candidate)
        cases.append((label, candidate))

    mutate("one boot", lambda r: r["boots"].pop())
    mutate("three boots", lambda r: r["boots"].append({**r["boots"][0], "index": 2}))
    mutate("a boot that did not reach ready", lambda r: r["boots"][1].update(outcome="no-boot"))
    mutate("a warm restart", lambda r: r["boots"][1].update(coldStart=False))
    mutate("a wrong-profile boot", lambda r: r["boots"][0].update(targetProfile="aarch64-rpi5"))
    mutate("a wrong-profile record", lambda r: r.update(targetProfile="aarch64-sel4-qemu-virt"))
    mutate("a record naming another composition", lambda r: r.update(composition="sel4-sample"))
    mutate("a record stating no limits", lambda r: r.update(limits=[]))
    mutate(
        "a transcript missing the timer phase",
        lambda r: r["boots"][0].update(
            recordLines=[line for line in r["boots"][0]["recordLines"] if "SLIME_TIMER" not in line]
        ),
    )
    mutate(
        "a transcript missing the terminal marker",
        lambda r: r["boots"][1].update(
            recordLines=r["boots"][1]["recordLines"][:-1] + ["SLIME_ROOT BUSY"]
        ),
    )
    mutate(
        "two boots that disagree on a semantic field",
        lambda r: r["boots"][1].update(
            recordLines=[
                line.replace("tasks=2", "tasks=3") for line in r["boots"][1]["recordLines"]
            ]
        ),
    )
    mutate("duplicate boot indices", lambda r: r["boots"][1].update(index=0))

    for label, candidate in cases:
        try:
            validate(candidate, None)
        except SystemExit:
            print(f"rubikpi3 cold boot control: refused {label}")
        else:
            fail(f"a record with {label} was accepted")


def command_verify() -> None:
    if not RECORD.is_file():
        fail(f"no recorded observation at {RECORD.relative_to(ROOT)}")
    record = json.loads(RECORD.read_text(encoding="utf-8"))
    image_sha = hashlib.sha256(IMAGE.read_bytes()).hexdigest() if IMAGE.is_file() else None
    validate(record, image_sha)
    if image_sha is None:
        print(
            "rubikpi3 cold boot check: the packaged image is absent, so the record's image "
            "hash was not cross-checked; run `just sel4_rubikpi3_image_check` first to close that"
        )
    for boot in sorted(record["boots"], key=lambda b: b["index"]):
        print(
            f"rubikpi3 cold boot {boot['index']}: cold start, ready "
            f"{boot['secondsToReady']:.2f}s after kernel entry"
        )
    print(
        f"rubikpi3 cold boot check: {len(record['boots'])} cold boots of "
        f"{record['imageSha256'][:16]} on {record['machine']} reached {TERMINAL}; "
        f"{len(record['limits'])} stated limit(s)"
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--controls", action="store_true", help="only run the refusal controls")
    arguments = parser.parse_args()
    command_controls()
    if not arguments.controls:
        command_verify()


if __name__ == "__main__":
    main()
