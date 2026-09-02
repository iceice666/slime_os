#!/usr/bin/env python3
"""P5.4.5 on seL4: the C8.5 QoS arms, observed on the `sel4-qos` plane.

The stream plane proves the typed fabric moves samples. This one proves the
*declared QoS policy* is enforced: a monotonic clock is granted to the graph, the
scenario advances it through scheduled boundaries, and each boundary must produce
its own event.

Why a separate image and gate. `sel4-stream.zti` grants no time capability, so its
simulated-time clause is structurally unreachable — the arms below cannot fire
there at all. `sel4-qos.zti` is that fixture plus a runtime-minted clock and a
`retained` diagnostics route, which is what makes an inline retained head exist
independently of publisher timing.

Marker chains rather than one global order: the participants provision
concurrently, so their arms interleave by scheduling. Each chain is internally
ordered because each is one causal sequence.
"""

from __future__ import annotations

import argparse
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

from sel4_gate_markers import match_marker_contract  # noqa: E402

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from harness import GENERATION_COMPOSITIONS, profile_text, profile_integer, sha256_file  # noqa: E402
ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
FIXTURE = GENERATION_COMPOSITIONS / "sel4-qos.zti"
# The closure identity names the build's inputs and is re-resolved from repository
# state before the build, so stale input is refused instead of silently changing the image.
CLOSURE = "sel4-qos"
IMAGE: Path | None = None
SPAWN_PATTERN = re.compile(
    r"SLIME_GRAPH spawned task=(\d+) child=(\d+) component=([^ ]+) "
    r"grants=(\d+) endpoints=(\d+) notifications=(\d+) handle=(\d+)"
)
EXIT_PATTERN = re.compile(r"SLIME_GRAPH component exit task=(\d+) status=(-?\d+)")
EXPECTED_PARTICIPANTS = {
    "fabric-service",
    "fabric-publisher",
    "fabric-publisher-b",
    "fabric-subscriber",
    "fabric-subscriber-b",
    "fabric-intruder",
}

BOOT_TIMEOUT_SECONDS = 240

CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "the QoS generation was admitted and its graph validated",
        (
            r"SLIME_ROOT generation admitted number=\d+ executables=7 instances=7 grants=\d+ ",
            r"SLIME_ROOT fabric graph=admitted schemas=2 routes=2 participants=6 ",
            r"SLIME_GRAPH activated instances=\d+",
        ),
    ),
    (
        # Matching precedes the "all provisioned" line: the broker matches each
        # participant as it provisions, so the summary comes last.
        "every declared participant was matched, then the edge set was complete",
        (
            r"\[fabric\] QoS matched",
            r"\[fabric\] every declared stream edge provisioned",
        ),
    ),
    (
        "the scenario advanced the simulated clock",
        (
            r"\[fabric-publisher-b\] simulated time advanced",
        ),
    ),
    (
        "RELIABLE retry is accounted and then exhausted",
        (
            r"\[fabric\] reliable retry accounted",
            r"\[fabric\] QoS retry exhausted",
        ),
    ),
    (
        # One chain each: these are independent scheduled boundaries, and the
        # order they fire in is a function of the declared tuples rather than a
        # causal sequence. Grouping them would assert an ordering the contract
        # does not promise.
        "a lost liveliness lease fires",
        (r"\[fabric\] QoS liveliness lost",),
    ),
    (
        "a missed deadline fires",
        (r"\[fabric\] QoS deadline missed",),
    ),
    (
        "an expired lifespan fires",
        (r"\[fabric\] QoS lifespan expired",),
    ),
    (
        "a departed publisher is retired through the peer-dead path",
        (
            # The departing peer is the *publisher*: this marker's only emission
            # site is the broker's publisher supervision sweep, and it fires when
            # a publisher terminates without ending its route. The plane scripts
            # that exit rather than depending on the peer's termination racing
            # the broker's drain, which is what once produced it here.
            r"\[fabric\] QoS peer dead",
        ),
    ),
    (
        # `served live=0` prints *after* init's terminal line, because the root
        # emits its accounting once the graph has drained and init has already
        # exited. So the drain evidence is the terminal marker here, and the
        # accounting line is asserted by `sel4_component_graph_check` instead.
        "the plane reached its terminal marker",
        (
            r"\[fabric\] stream plane complete",
            r"\[init\] fabric stream complete",
        ),
    ),
)

# The last marker of the last chain: `boot` stops reading once it appears, so the
# gate does not wait out the full timeout on a healthy plane.
TERMINAL_MARKER = CHAINS[-1][1][-1]

# B28: the plane exhausted `MAX_GRAPH_ITERATIONS` at 512 and reported no component
# failure at all, which is why the wedge marker is a failure marker here. Any
# reappearance is the same defect and must be red rather than a missing arm.
FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_ROOT FAIL",
    r"SLIME_GRAPH FAIL",
    r"SLIME_GRAPH wedged waiter",
    # Scoped to init's own failure, exactly as the stream gate scopes its own.
    # Six `fail:` lines are *expected* on this plane — every participant proves a
    # denial arm (`fail: request`, `fail: role reply`, `fail: route endpoints`),
    # and treating them as failures would make this gate red on a correct boot.
    r"\[init\] stream plane fail: .*",
    r"\[fabric\] no inline retained publisher",
    r"<<seL4\(CPU 0\) \[decodeInvocation",
    r"unhandled",
)

def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 QoS plane check: {message}")


def load_pins() -> dict[str, object]:
    if not PINS_PATH.is_file():
        fail(f"missing pin manifest: {PINS_PATH.relative_to(ROOT)}")
    try:
        pins = tomllib.loads(PINS_PATH.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        fail(f"cannot parse {PINS_PATH.relative_to(ROOT)}: {error}")
    if pins.get("schema") != 1:
        fail("unsupported sel4/pins.toml schema (expected 1)")
    if not isinstance(pins.get("qemu_arm_virt"), dict):
        fail("sel4/pins.toml is missing [qemu_arm_virt]")
    return pins


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
    """Boot the image and return the serial transcript.

    The root task suspends itself once the graph has drained, so QEMU stays
    alive afterwards and waiting for an exit would always time out. Serial
    output is read line by line and the guest is killed as soon as the terminal
    or any failure marker appears.
    """
    qemu = shutil.which("qemu-system-aarch64")
    if qemu is None:
        fail("qemu-system-aarch64 is not on PATH")
    command = [
        qemu,
        "-machine",
        profile_text(profile, "machine", fail),
        "-cpu",
        profile_text(profile, "cpu", fail),
        "-smp",
        str(profile_integer(profile, "cpus", fail)),
        "-m",
        f"size={profile_integer(profile, 'memory_mib', fail)}M",
        "-nographic",
        "-serial",
        "mon:stdio",
        "-kernel",
        str(IMAGE),
    ]
    print(f"[boot] {' '.join(command)}", flush=True)
    terminal = re.compile(TERMINAL_MARKER)
    failures = re.compile("|".join(FAILURE_MARKERS))
    lines: list[str] = []
    try:
        process = subprocess.Popen(
            command,
            cwd=ROOT,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
    except OSError as error:
        fail(f"cannot run QEMU: {error}")
    # A wedged guest emits nothing, so the deadline cannot live in the read
    # loop; a watchdog kills QEMU, which closes the pipe and ends the loop.
    watchdog = threading.Timer(BOOT_TIMEOUT_SECONDS, process.kill)
    watchdog.start()
    try:
        assert process.stdout is not None
        for line in process.stdout:
            lines.append(line.rstrip("\n"))
            if terminal.search(line) or failures.search(line):
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
    transcript = "\n".join(lines)
    if timed_out and terminal.search(transcript) is None:
        report_transcript(transcript)
        fail(f"boot exceeded {BOOT_TIMEOUT_SECONDS}s without reaching the final marker")
    return transcript


def report_transcript(transcript: str) -> None:
    tail = transcript.splitlines()[-40:]
    if tail:
        sys.stdout.write("--- serial transcript (tail) ---\n")
        sys.stdout.write("\n".join(tail) + "\n")
        sys.stdout.write("--- end transcript ---\n")
        sys.stdout.flush()


def check_transcript(transcript: str) -> None:
    match_marker_contract(
        transcript,
        CHAINS,
        FAILURE_MARKERS,
        fail,
        before_reject=lambda: report_transcript(transcript),
    )
    check_participant_lifecycle(transcript)
    print(
        f"transcript: {sum(len(chain) for _, chain in CHAINS)} markers observed "
        f"across {len(CHAINS)} causal chains; all six spawned participants exited "
        "cleanly and none reported a failure",
        flush=True,
    )


def check_participant_lifecycle(transcript: str) -> None:
    spawns = SPAWN_PATTERN.findall(transcript)
    spawned = {match[2] for match in spawns}
    if spawned != EXPECTED_PARTICIPANTS:
        report_transcript(transcript)
        fail(
            f"init spawned {sorted(spawned)}, expected the six QoS participants "
            f"{sorted(EXPECTED_PARTICIPANTS)}"
        )
    children = {component: child for _parent, child, component, *_ in spawns}
    exits: dict[str, list[int]] = {}
    for task, status in EXIT_PATTERN.findall(transcript):
        exits.setdefault(task, []).append(int(status))
    for component, task in children.items():
        if exits.get(task) != [0]:
            report_transcript(transcript)
            fail(
                f"{component} task {task} exit statuses were {exits.get(task, [])}, "
                "expected [0]"
            )

    # No failure from any of the six. This was a per-component budget of
    # exactly one, from the P5.2 model where the root launched every declared
    # instance and each participant therefore also ran an unconfigured copy
    # that failed its first operation. A v4 generation launches only
    # root-owned autostart instances -- this fixture declares one, init -- so
    # there are no unconfigured copies and every `fail:` line belongs to a
    # participant init spawned.
    for component, prefix in (
        ("fabric-service", r"\[fabric\]"),
        ("fabric-publisher", r"\[fabric-publisher\]"),
        ("fabric-publisher-b", r"\[fabric-publisher-b\]"),
        ("fabric-subscriber", r"\[fabric-subscriber\]"),
        ("fabric-subscriber-b", r"\[fabric-subscriber-b\]"),
        ("fabric-intruder", r"\[fabric-intruder\]"),
    ):
        failures = re.findall(rf"{prefix} fail: .*", transcript)
        if failures:
            report_transcript(transcript)
            fail(f"{component} reported {len(failures)} failures: {failures}")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 QoS-plane image and assert the C8.5 arms"
    )
    parser.add_argument(
        "--no-build",
        action="store_true",
        help="boot the already-built image instead of rebuilding it first",
    )
    arguments = parser.parse_args()

    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    if not FIXTURE.is_file():
        fail(f"missing generation fixture {FIXTURE.relative_to(ROOT)}")
    pins = load_pins()
    if not arguments.no_build:
        build_image()
    profile = pins["qemu_arm_virt"]
    assert isinstance(profile, dict)
    check_transcript(boot(profile))
    print(
        "seL4 QoS plane check: C8.5's declared QoS policy is enforced on seL4 with "
        "every participant unmodified -- RELIABLE retry accounting and exhaustion, a "
        "missed deadline, an expired lifespan, a lost liveliness lease, and peer-dead "
        "retirement -- driven by a monotonic clock the generation grants, and the "
        "plane reached its terminal marker"
    )


if __name__ == "__main__":
    main()
