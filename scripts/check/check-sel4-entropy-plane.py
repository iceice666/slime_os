#!/usr/bin/env python3
"""Entropy authority on seL4: a userspace entropy service over virtio-rng.

The gate boots `sel4-entropy` twice with a QEMU virtio-rng device and once
without. Three probe holders draw through their declared per-holder endpoints
and a fourth holds no grant. A seeded holder's bytes must equal the host's
independent HMAC-DRBG(SHA-256) from the seed in the derived composition, in
every boot. Hardware holders' blocks must be distinct within a boot, differ
between boots, and never appear without the device. Budgets and request sizes
are refused at their declared bounds.
"""

from __future__ import annotations

import argparse
import re
import sys
from collections.abc import Callable
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
from closure_image import ClosureImageError, build as build_closure_image  # noqa: E402
from harness import GENERATION_COMPOSITIONS, sha256_file  # noqa: E402

import devloop_observations  # noqa: E402
import hmac_drbg  # noqa: E402
from sel4_gate_markers import match_marker_contract  # noqa: E402
from sel4_plane import run_plane  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS = ROOT / "sel4" / "pins.toml"
CLOSURE = "sel4-entropy"
FIXTURE = GENERATION_COMPOSITIONS / "sel4-entropy.zti"
GENERATION = 165
TIMEOUT = 240
DRAW_BYTES = 32
BUDGET = 96
HARDWARE = ("entropy-hw-a", "entropy-hw-b")
SEEDED = "entropy-seeded"
INTRUDER = "entropy-intruder"
# The draws each holder makes before its terminal line, in program order.
DRAWS = {"entropy-hw-a": 3, "entropy-hw-b": 1, SEEDED: 3}
RNG_DEVICE = ("-object", "rng-random,id=slimerng,filename=/dev/urandom", "-device", "virtio-rng-device,rng=slimerng")


def _draws(holder: str) -> tuple[str, ...]:
    return tuple(rf"\[entropy-probe\] holder={holder} draw={index} hex=([0-9a-f]{{64}})" for index in range(DRAWS[holder]))


def _exhausted(holder: str) -> str:
    return rf"\[entropy-probe\] holder={holder} exhausted=1 drawn={BUDGET}"


_ADMISSION = rf"SLIME_ROOT generation admitted number={GENERATION} executables=[0-9]+ instances=[0-9]+ grants=[0-9]+ "
_AUTHORITY = r"\[entropy-service\] authority holders=3 hardware=2 seeded=1"
_READY = r"\[entropy-service\] source=virtio-rng ready reseed_interval=64"
_UNAVAILABLE = r"\[entropy-service\] source=virtio-rng unavailable"
_INTRUDER = rf"\[entropy-probe\] holder={INTRUDER} binding_absent=1"
_HEALTHY = rf"SLIME_GRAPH HEALTHY generation={GENERATION} required=[0-9]+ live=0 completed=[0-9]+ failed=0"

# The device-present boot. The service states its table before the source is
# ready; the driver's readiness precedes the service's, but not its table.
CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    ("entropy admission", (_ADMISSION,)),
    ("entropy authority", (_AUTHORITY, _READY)),
    ("entropy driver", (r"\[virtio-rng-driver\] device ready", _READY)),
    ("entropy hardware a", (*_draws("entropy-hw-a"), _exhausted("entropy-hw-a"))),
    ("entropy hardware b", (*_draws("entropy-hw-b"), r"\[entropy-probe\] holder=entropy-hw-b malformed_refused=2")),
    ("entropy seeded", (*_draws(SEEDED), _exhausted(SEEDED))),
    ("entropy intruder", (_INTRUDER,)),
    ("entropy health", (_HEALTHY,)),
)
# The device-absent boot. Kept out of CHAINS: its hardware refusals and the
# present boot's hardware draws cannot share one transcript.
ABSENT_CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    ("absent admission", (_ADMISSION,)),
    ("absent authority", (_AUTHORITY, _UNAVAILABLE)),
    ("absent driver", (r"\[virtio-rng-driver\] device absent", _UNAVAILABLE)),
    *((f"absent {holder}", (rf"\[entropy-probe\] holder={holder} unavailable=1",)) for holder in HARDWARE),
    ("absent seeded", (*_draws(SEEDED), _exhausted(SEEDED))),
    ("absent intruder", (_INTRUDER,)),
    ("absent health", (_HEALTHY,)),
)
FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_GRAPH FAIL",
    r"\[entropy-service\] fail: ",
    r"\[virtio-rng-driver\] fail: ",
    r"\[entropy-probe\] fail: ",
    r"Caught cap fault",
    r"Caught vm fault",
    r"panicked at ",
)
# Absent from the device-less boot: any hardware draw, and any claim of readiness.
ABSENT_FORBIDDEN: tuple[str, ...] = (
    *(rf"\[entropy-probe\] holder={holder} draw=" for holder in HARDWARE),
    _READY,
    r"\[virtio-rng-driver\] device ready",
)


class Refusal(Exception):
    """A judged transcript set was refused; raised so controls can count refusals."""


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 entropy plane check: {message}")


def _refuse(message: str) -> NoReturn:
    raise Refusal(message)


def fixture_rows(text: str) -> list[dict[str, str]]:
    section = re.search(r"entropyAuthority\s*=\s*\[(.*?)\n  \];", text, re.S)
    if section is None:
        fail("sel4-entropy declares no entropyAuthority table")
    assert section is not None
    return [
        {key: value.strip('"') for key, value in re.findall(r'^\s*(\w+)\s*=\s*("[^"]*"|-?[0-9]+);', block, re.M)}
        for block in re.findall(r"\{(.*?)\n    \};", section.group(1), re.S)
    ]


def check_fixture() -> bytes:
    """Return the seeded holder's declared seed once the table is exactly the one the probes exercise."""
    if not FIXTURE.is_file():
        fail(f"{FIXTURE.relative_to(ROOT)} does not exist; the sel4-entropy composition has not landed")
    text = FIXTURE.read_text(encoding="utf-8")
    for pattern in (
        rf"generation\s*=\s*{GENERATION};",
        *(rf'name\s*=\s*"{name}"' for name in ("virtio-rng-driver", "entropy-service", *HARDWARE, SEEDED, INTRUDER)),
    ):
        if re.search(pattern, text) is None:
            fail(f"sel4-entropy fixture is missing {pattern!r}")
    rows = {row.get("holder"): row for row in fixture_rows(text)}
    expected = {holder: "hardware" for holder in HARDWARE} | {SEEDED: "seeded"}
    if set(rows) != set(expected):
        fail(f"sel4-entropy must declare rows for exactly {sorted(expected)}, found {sorted(map(str, rows))}")
    for holder, source in expected.items():
        row = rows[holder]
        if row.get("source") != source or row.get("byteBudget") != str(BUDGET):
            fail(f"{holder} must be a {source} row with byteBudget {BUDGET}, found {row}")
        if source == "hardware" and row.get("seed", "") != "":
            fail(f"hardware row {holder} declares a seed")
    seed = rows[SEEDED].get("seed", "")
    if re.fullmatch(r"[0-9a-f]{64}", seed) is None:
        fail("the seeded row's seed is not 64 lowercase hex digits")
    return bytes.fromhex(seed)


def build_image() -> Path:
    try:
        built = build_closure_image(CLOSURE)
    except ClosureImageError as error:
        fail(str(error))
    actual = sha256_file(built.image, fail)
    if actual != built.digest():
        fail(f"{built.image} SHA-256 is {actual}, but the build result records {built.digest()}; the image changed after it was built")
    return built.image


def boot(image: Path, device: bool) -> str:
    terminal = re.compile(_HEALTHY + "|" + "|".join(FAILURE_MARKERS))
    return run_plane(
        image=image, timeout=TIMEOUT, terminal_condition=terminal, fail=fail, pins_path=PINS,
        additional_arguments=RNG_DEVICE if device else (),
    )


def blocks(transcript: str, holder: str) -> list[bytes]:
    found = re.findall(rf"\[entropy-probe\] holder={holder} draw=(\d+) hex=([0-9a-f]{{64}})", transcript)
    if [int(index) for index, _ in found] != list(range(len(found))):
        _refuse(f"{holder} draws are missing, repeated or out of order")
    return [bytes.fromhex(value) for _, value in found]


def judge(present: tuple[str, str], absent: str, seed: bytes, reject: Callable[[str], NoReturn] = _refuse) -> tuple[int, int]:
    """Return (cases judged, seeded bytes matched) over two device boots and one device-less boot."""
    expected = hmac_drbg.seeded_stream(seed, SEEDED, DRAWS[SEEDED], DRAW_BYTES)
    cases = 0
    matched = 0
    hardware: list[list[bytes]] = []
    for transcript in present:
        match_marker_contract(transcript, CHAINS, FAILURE_MARKERS, reject)
        if blocks(transcript, SEEDED) != expected:
            reject("seeded holder's bytes differ from the host HMAC-DRBG stream")
        matched += DRAW_BYTES * DRAWS[SEEDED]
        drawn = [block for holder in HARDWARE for block in blocks(transcript, holder)]
        if len(drawn) != sum(DRAWS[holder] for holder in HARDWARE):
            reject("hardware holders drew an unexpected number of blocks")
        if len(set(drawn)) != len(drawn) or any(block in expected for block in drawn) or any(block == bytes(DRAW_BYTES) for block in drawn):
            reject("a hardware block repeats within the boot, equals a seeded block, or is all zero")
        hardware.append(drawn)
    cases += 1  # seeded bytes equal the host stream
    cases += 1  # seeded bytes repeat across the two device boots
    cases += 1  # hardware blocks distinct within each boot
    if set(hardware[0]) & set(hardware[1]):
        reject("a hardware block repeats across boots")
    cases += 1  # hardware blocks differ across boots
    cases += 1  # budgets exhausted at the declared bound (chains)
    cases += 1  # malformed sizes refused (chain)
    cases += 1  # the unauthorised probe has no binding (chain)
    match_marker_contract(absent, ABSENT_CHAINS, FAILURE_MARKERS, reject)
    for pattern in ABSENT_FORBIDDEN:
        if re.search(pattern, absent) is not None:
            reject(f"device-less boot shows hardware entropy evidence: {pattern}")
    cases += 1  # hardware draws fail closed without the device
    if blocks(absent, SEEDED) != expected:
        reject("seeded holder's bytes differ from the host stream without the device")
    matched += DRAW_BYTES * DRAWS[SEEDED]
    cases += 1  # seeded draws survive the device's absence
    return cases, matched


def controls(present: tuple[str, str], absent: str, seed: bytes) -> int:
    """Mutate the accepted transcripts and require every mutation to be refused; return the count."""
    first, second = present

    def draw_line(transcript: str, holder: str, index: int) -> re.Match[str]:
        found = re.search(rf"\[entropy-probe\] holder={holder} draw={index} hex=([0-9a-f]{{64}})", transcript)
        if found is None:
            fail(f"controls need {holder} draw {index}")
        assert found is not None
        return found

    seeded = draw_line(first, SEEDED, 0)
    flipped = ("1" if seeded[1][0] == "0" else "0") + seeded[1][1:]
    hw_a_first = draw_line(first, "entropy-hw-a", 0)[1]
    hw_a_second = draw_line(second, "entropy-hw-a", 0)[1]
    hw_b_first = draw_line(first, "entropy-hw-b", 0)[1]
    exhausted = re.compile(_exhausted("entropy-hw-a")).search(first)
    if exhausted is None:
        fail("controls need the hardware holder's exhaustion refusal")
    assert exhausted is not None
    mutations = (
        ("altered seeded byte", (first.replace(seeded[1], flipped), second), absent),
        ("hardware block repeated across boots", (first, second.replace(hw_a_second, hw_a_first)), absent),
        ("hardware block repeated across holders", (first.replace(hw_b_first, hw_a_first), second), absent),
        ("missing exhaustion refusal", (first[:exhausted.start()] + first[exhausted.end():], second), absent),
        ("hardware draw without the device", present, absent + f"\n[entropy-probe] holder=entropy-hw-a draw=0 hex={hw_a_first}"),
        ("explicit failure", (first + "\n[entropy-service] fail: injected control", second), absent),
    )
    refused = 0
    for label, mutated_present, mutated_absent in mutations:
        try:
            judge(mutated_present, mutated_absent, seed)
        except Refusal:
            refused += 1
            continue
        fail(f"control accepted a mutated transcript: {label}")
    return refused


def main() -> None:
    parser = argparse.ArgumentParser(description="Boot and check the seL4 entropy-authority plane")
    parser.add_argument("--transcript", type=Path, help="write the three boots' serial transcripts to this file")
    arguments = parser.parse_args()
    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    try:
        hmac_drbg.self_test()
    except ValueError as error:
        fail(str(error))
    seed = check_fixture()
    image = build_image()
    present = (boot(image, device=True), boot(image, device=True))
    absent = boot(image, device=False)
    if arguments.transcript is not None:
        arguments.transcript.write_text("\n==== boot ====\n".join((*present, absent)) + "\n", encoding="utf-8")
    try:
        cases, matched = judge(present, absent, seed)
    except Refusal as error:
        print("\n==== boot ====\n".join((*present, absent)))
        fail(str(error))
    refused = controls(present, absent, seed)
    print(f"[entropy] cases={cases} seeded-bytes={matched} controls-refused={refused}")
    devloop_observations.record(casesObserved=cases, bytesObserved=matched, negativeControlsRefused=refused)
    print(
        "seL4 entropy plane check: seeded draws reproduced the host HMAC-DRBG in every boot, hardware draws were "
        "distinct within and across boots and failed closed without the device, budgets and request sizes were "
        "refused at their bounds, and an ungranted holder had no binding"
    )


if __name__ == "__main__":
    main()
