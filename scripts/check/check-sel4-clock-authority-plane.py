#!/usr/bin/env python3
"""C9.1 gate: declared clock authority is independent, bounded, and deny-by-default."""
from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import threading
import tomllib
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
from closure_image import ClosureImageError, build as build_closure_image  # noqa: E402

from harness import GENERATION_COMPOSITIONS, sha256_file  # noqa: E402
from sel4_gate_markers import match_marker_contract  # noqa: E402
from zutai_cli import STDLIB, binary  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS = ROOT / "sel4" / "pins.toml"
FIXTURE = GENERATION_COMPOSITIONS / "sel4-clock-authority.zti"
# The closure identity names the build's inputs and is re-resolved from repository
# state before the build, so stale input is refused instead of silently changing the image.
CLOSURE = "sel4-clock-authority"
IMAGE: Path | None = None
TIMEOUT = 240

CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "timer cancellation and bounded expiry",
        (
            r"\[clock-probe:timer\] cancel silent=1",
            r"\[clock-probe:timer\] quota refused=1 peer-live=1",
            r"SLIME_CLOCK expired due=1 delivered=1 live=1",
            r"\[clock-probe:timer\] expired badge=0x200 once=1 peer-intact=1 teardown-live=1",
            r"SLIME_GRAPH component exit task=(\d+) status=0",
            r"SLIME_CLOCK teardown task=(\d+) before=1 live=0",
        ),
    ),
    (
        "deny by default",
        (
            r"SLIME_CLOCK malformed task=\d+ label=44 words=1 expected=Some\(0\)",
            r"\[clock-probe:denied\] monotonic=-1 timer=-1 sim-read=-1 sim-advance=-1 malformed=-4",
            r"SLIME_GRAPH component exit task=\d+ status=0",
        ),
    ),
    (
        "terminal cleanup",
        (
            r"\[init\] clock authority plane is root-launched",
            r"SLIME_GRAPH HEALTHY generation=41 required=6 live=0 completed=6 failed=0",
        ),
    ),
)

EXPECTED_UNORDERED: tuple[str, ...] = (
    r"SLIME_CLOCK authority task=(\d+) instance=clock-monotonic flags=0x4000000 timers=0 badge=0x0",
    r"SLIME_CLOCK authority task=(\d+) instance=clock-simulated-advancer flags=0x20000000 timers=0 badge=0x0",
    r"SLIME_CLOCK authority task=(\d+) instance=clock-simulated-reader flags=0x10000000 timers=0 badge=0x0",
    r"SLIME_CLOCK authority task=(\d+) instance=clock-timer flags=0x8000000 timers=2 badge=0x200",
    r"SLIME_CLOCK authority task=(\d+) instance=clock-denied flags=0x0 timers=0 badge=0x0",
    r"\[clock-probe:monotonic\] first=(\d+) second=(\d+)",
    r"\[clock-probe:sim-read\] first=(\d+) second=(\d+)",
    r"\[clock-probe:sim-advance\] before=(\d+) after=(\d+)",
)


FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_CLOCK FAIL",
    r"\[clock-probe\] FAIL",
    r"SLIME_GRAPH FAIL required instance",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 clock-authority plane check: {message}")


def build_image() -> None:
    global IMAGE
    try:
        built = build_closure_image(CLOSURE)
    except ClosureImageError as error:
        fail(str(error))
    IMAGE = built.image
    actual = sha256_file(IMAGE, fail)
    if actual != built.digest():
        fail(
            f"{IMAGE} SHA-256 is {actual}, but the build result records "
            f"{built.digest()}; the image changed after it was built"
        )


def boot(profile: dict[str, object]) -> str:
    qemu = shutil.which("qemu-system-aarch64")
    if qemu is None:
        fail("qemu-system-aarch64 is not on PATH")
    command = [
        qemu,
        "-machine",
        str(profile["machine"]),
        "-cpu",
        str(profile["cpu"]),
        "-smp",
        str(profile["cpus"]),
        "-m",
        f"size={profile['memory_mib']}M",
        "-nographic",
        "-serial",
        "mon:stdio",
        "-kernel",
        str(IMAGE),
    ]
    process = subprocess.Popen(
        command,
        cwd=ROOT,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    watchdog = threading.Timer(TIMEOUT, process.kill)
    watchdog.start()
    lines: list[str] = []
    terminal = re.compile(
        r"SLIME_GRAPH HEALTHY generation=41 required=6 live=0 completed=6 failed=0"
        r"|SLIME_ROOT FATAL|SLIME_CLOCK FAIL|\[clock-probe\] FAIL"
    )
    try:
        assert process.stdout is not None
        for line in process.stdout:
            lines.append(line.rstrip("\n"))
            if terminal.search(line):
                break
    finally:
        timed_out = not watchdog.is_alive()
        watchdog.cancel()
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
    if timed_out:
        fail("QEMU timed out")
    return "\n".join(lines)


def fixture_manifest() -> dict[str, object]:
    """Decode the exercised authority and notification shape through Zutai."""
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    process = subprocess.run(
        [str(binary()), "json", str(FIXTURE)],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if process.returncode != 0:
        fail(f"could not decode the fixture: {process.stdout.strip()}")
    return json.loads(process.stdout)


def check_fixture_shape() -> None:
    manifest = fixture_manifest()
    authorities = manifest.get("clockAuthority") or []
    if not isinstance(authorities, list):
        fail("fixture clockAuthority is not a list")
    holders = {entry["holder"] for entry in authorities}
    expected_holders = {
        "clock-monotonic",
        "clock-timer",
        "clock-simulated-reader",
        "clock-simulated-advancer",
    }
    if holders != expected_holders:
        fail(
            f"fixture declares clock holders {sorted(holders)}, "
            f"expected {sorted(expected_holders)}"
        )
    timer = next(entry for entry in authorities if entry["holder"] == "clock-timer")
    if (
        timer["timerQuota"] != 2
        or timer["timerBadgeBit"] != 9
        or timer["timerNotification"] != "clock-timer-expiry"
    ):
        fail(
            "fixture does not declare the timer quota, badge, and notification exercised"
        )
    instances = {entry["name"] for entry in manifest["instances"]}
    if "clock-denied" not in instances or "clock-denied" in holders:
        fail("fixture has no clock instance omitted from the authority declaration")


def check_semantics(transcript: str) -> None:
    tasks: dict[str, str] = {}
    for instance, expected_flags, expected_timers, expected_badge in (
        ("clock-monotonic", "4000000", "0", "0"),
        ("clock-simulated-advancer", "20000000", "0", "0"),
        ("clock-simulated-reader", "10000000", "0", "0"),
        ("clock-timer", "8000000", "2", "200"),
        ("clock-denied", "0", "0", "0"),
    ):
        installed = re.search(
            rf"SLIME_CLOCK authority task=(\d+) instance={instance} "
            rf"flags=0x{expected_flags} timers={expected_timers} badge=0x{expected_badge}",
            transcript,
        )
        if installed is None:
            fail(f"missing root authority installation for {instance}")
        tasks[instance] = installed.group(1)

    for instance, label, result_class in (
        ("clock-monotonic", 44, "served"),
        ("clock-timer", 45, "served"),
        ("clock-simulated-reader", 47, "served"),
        ("clock-simulated-advancer", 48, "served"),
    ):
        if re.search(
            rf"SLIME_CLOCK {result_class} task={tasks[instance]} label={label} ", transcript
        ) is None:
            fail(f"root did not serve {instance}'s declared operation")

    denied_task = tasks["clock-denied"]
    for label in (44, 45, 47, 48):
        if re.search(
            rf"SLIME_GRAPH service refused task={denied_task} label={label} class=undeclared",
            transcript,
        ) is None:
            fail(f"root did not refuse clock-denied label {label}")
    if re.search(
        rf"SLIME_CLOCK malformed task={denied_task} label=44 words=1 expected=Some\(0\)",
        transcript,
    ) is None:
        fail("malformed refusal was not bound to clock-denied")

    timer_task = tasks["clock-timer"]
    timer_cleanup = re.search(
        rf"SLIME_GRAPH component exit task={timer_task} status=0\n"
        rf"SLIME_CLOCK teardown task={timer_task} before=1 live=0",
        transcript,
    )
    if timer_cleanup is None:
        fail("live timer cleanup was not observed for the authorized timer task")

    monotonic = re.search(EXPECTED_UNORDERED[5], transcript)
    if monotonic is None or int(monotonic.group(2)) <= int(monotonic.group(1)):
        fail("monotonic holder did not observe a strictly advancing clock")
    simulated = re.search(EXPECTED_UNORDERED[6], transcript)
    if simulated is None or simulated.group(1) != simulated.group(2):
        fail("simulated clock advanced without its advancer")
    advance = re.search(EXPECTED_UNORDERED[7], transcript)
    if advance is None or int(advance.group(2)) - int(advance.group(1)) != 7:
        fail("simulated advancer did not advance by exactly seven")


def main() -> None:
    check_fixture_shape()
    build_image()
    pins = tomllib.loads(PINS.read_text(encoding="utf-8"))
    profile = pins.get("qemu_arm_virt")
    if not isinstance(profile, dict):
        fail("missing qemu profile")
    transcript = boot(profile)
    match_marker_contract(transcript, CHAINS, FAILURE_MARKERS, fail)
    for pattern in EXPECTED_UNORDERED:
        if re.search(pattern, transcript) is None:
            fail(f"missing order-independent marker: {pattern}")
    check_semantics(transcript)
    print(
        "seL4 clock-authority plane check: independent clocks, bounded timers, "
        "cancellation, expiry, and denial observed"
    )

if __name__ == "__main__":
    main()
