#!/usr/bin/env python3
"""Prove seL4 gates fail closed when evidence or shared execution breaks.

Marker controls synthesize each gate's declared evidence, then delete, reorder,
or poison it. Runtime controls call ``sel4_plane`` directly with temporary images,
identity manifests, pins, and QEMU executables. Product gates retain ownership of
their concrete boot claims; this checker proves the mechanisms enforcing those
claims reject invalid inputs.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import sys as _sys
import tempfile
from collections.abc import Callable
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

from harness import ROOT, load_script  # noqa: E402
from sel4_gate_markers import chains_from_gate, match_marker_contract  # noqa: E402
from sel4_plane import run_plane, verify_image_identity  # noqa: E402

# Counts are pinned rather than derived so deleting a required marker cannot
# silently weaken a gate. Boot-layout fixture equality is controlled separately.
GATES: tuple[tuple[str, str, int], ...] = (
    ("sel4_channel_plane", "check/check-sel4-channel-plane.py", 18),
    ("sel4_io_network_plane", "check/check-sel4-io-network-plane.py", 16),
    ("sel4_component_graph", "check/check-sel4-component-graph.py", 29),
    ("sel4_crossing_plane", "check/check-sel4-crossing-plane.py", 10),
    ("sel4_loan_plane", "check/check-sel4-loan-plane.py", 46),
    ("sel4_io_queue_plane", "check/check-sel4-io-queue-plane.py", 15),
    ("sel4_io_link_plane", "check/check-sel4-io-link-plane.py", 28),
    ("sel4_io_driver_authority_plane", "check/check-sel4-io-driver-authority-plane.py", 16),
    ("sel4_device_plane", "check/check-sel4-device-plane.py", 2),
    ("sel4_root_boot", "check/check-sel4-root-boot.py", 56),
    ("sel4_sample_plane", "check/check-sel4-sample-plane.py", 25),
    ("sel4_spawn_plane", "check/check-sel4-spawn-plane.py", 27),
    ("sel4_supervision_plane", "check/check-sel4-supervision-plane.py", 12),
    ("sel4_private_memory_plane", "check/check-sel4-private-memory-plane.py", 22),
    ("sel4_clock_authority_plane", "check/check-sel4-clock-authority-plane.py", 19),
    ("sel4_wait_set_plane", "check/check-sel4-wait-set-plane.py", 15),
    ("sel4_scheduling_class_plane", "check/check-sel4-scheduling-class-plane.py", 25),
    ("sel4_lifecycle_restart_plane", "check/check-sel4-lifecycle-restart-plane.py", 55),
    ("sel4_replay_plane", "check/check-sel4-replay-plane.py", 29),
    ("sel4_robot_runtime_plane", "check/check-sel4-robot-runtime-plane.py", 45),
    ("sel4_stream_plane", "check/check-sel4-stream-plane.py", 57),
    ("sel4_qos_plane", "check/check-sel4-qos-plane.py", 14),
    # Only the demo slice arm exposes CHAINS; wrong-target and rollback use
    # separate validators owned by that checker.
    ("sel4_demo_plane", "check/check-sel4-demo-plane.py", 29),
    ("sel4_trace_plane", "check/check-sel4-trace-plane.py", 6),
    ("sel4_call_plane", "check/check-sel4-call-plane.py", 47),
    ("sel4_operation_plane", "check/check-sel4-operation-plane.py", 53),
    ("sel4_visibility_plane", "check/check-sel4-visibility-plane.py", 26),
    ("sel4_matrix_plane", "check/check-sel4-matrix-plane.py", 26),
    ("sel4_traffic_plane", "check/check-sel4-traffic-plane.py", 10),
    ("sel4_saturation_plane", "check/check-sel4-saturation-plane.py", 10),
    ("sel4_fault_plane", "check/check-sel4-fault-plane.py", 10),
    ("sel4_boot_plane", "check/check-sel4-boot-plane.py", 30),
    ("sel4_io_block_plane", "check/check-sel4-io-block-plane.py", 10),
    ("sel4_storage_plane", "check/check-sel4-storage-plane.py", 12),
    ("sel4_store_plane", "check/check-sel4-store-plane.py", 17),
    ("sel4_rollback_plane", "check/check-sel4-rollback-plane.py", 19),
    ("sel4_recovery_plane", "check/check-sel4-recovery-plane.py", 12),
    ("sel4_generation_plane", "check/check-sel4-generation-plane.py", 21),
    ("sel4_directory_plane", "check/check-sel4-directory-plane.py", 16),
    ("sel4_filesystem_plane", "check/check-sel4-filesystem-plane.py", 14),
    ("sel4_input_plane", "check/check-sel4-input-plane.py", 7),
    ("sel4_powerbox_plane", "check/check-sel4-powerbox-plane.py", 11),
    ("sel4_transfer_plane", "check/check-sel4-transfer-plane.py", 12),
    ("rpi5_boot", "check/check-rpi5-boot.py", 11),
    ("nt98690_boot", "check/check-nt98690-boot.py", 25),
    ("nt98690_sel4", "check/check-nt98690-sel4.py", 19),
    ("nt98690_slisp", "check/check-nt98690-slisp.py", 34),
)


def fail(message: str) -> None:
    raise SystemExit(f"seL4 gate control check: {message}")


def literal_for(pattern: str) -> str:
    """One concrete line that satisfies `pattern`.

    The marker tables are regexes over serial output, so a synthetic transcript
    has to instantiate them. Only the constructs the tables actually use are
    handled, and anything else is a hard error rather than a silent skip — a
    control that quietly stopped covering a marker would be worse than no control.
    """
    text = pattern
    # Anchors and whitespace-tolerance constructs carry no content.
    text = text.replace(r"\s+", " ").replace(r"\s*", "")
    text = text.replace("^", "").replace("$", "")
    # Multi-line marker contracts spell transcript newlines as regex `\n`.
    # Materialize them before escaped-literal parking so the synthetic evidence
    # is the same byte sequence the gate searches.
    text = text.replace(r"\n", "\n")
    text = text.replace("-?", "-")
    # Character classes and repetitions, narrowest first.
    #
    # Bounded repetition over the two digit classes the marker tables use, plus a
    # single literal character. It comes before the open-ended forms because
    # `[0-9a-f]+` would otherwise consume the class before its count was seen,
    # leaving a stray `{16}` no instantiation satisfies. Escaped shorthands such
    # as `\d{4}` are deliberately not handled: no marker table uses one, and the
    # round-trip check below fails loudly rather than silently if one appears.
    def repeat(match: re.Match[str]) -> str:
        return match.group(1) * int(match.group(2))

    text = re.sub(r"\[0-9a-f\]\{(\d+)\}", lambda m: "a" * int(m.group(1)), text)
    text = re.sub(r"\[0-9\]\{(\d+)\}", lambda m: "7" * int(m.group(1)), text)
    text = re.sub(r"(\w)\{(\d+)\}", repeat, text)
    # Lowercase-with-hyphen words: worker and family names in the trace tables.
    text = re.sub(r"\[a-z-\]\+", "stream", text)
    text = re.sub(r"\[0-9a-fx\]\+", "0x10", text)
    text = re.sub(r"\[0-9a-f\]\+", "abc123", text)
    text = re.sub(r"\[\^ \]\+", "value", text)
    # `[1-9]\d*` and friends: a non-zero digit followed by optional digits.
    text = re.sub(r"\[1-9\]\\d\*", "7", text)
    text = re.sub(r"\[1-9\]", "7", text)
    text = re.sub(r"\[0-9\]\+", "7", text)
    text = re.sub(r"\\d\*", "7", text)
    text = re.sub(r"\\d\+", "7", text)
    text = re.sub(r"\\d", "7", text)
    # Backreferences hold two fields equal, which is the assertion rather than
    # decoration -- `required=(\d+) live=\1` says a healthy graph has every
    # required instance live. Replaying the group's own instantiated text keeps
    # that true in the synthetic line; dropping it would emit a literal `\1`
    # and fail the round-trip check below.
    groups = re.findall(r"\((?!\?)([^()]*)\)", text)
    for index, group in enumerate(groups, start=1):
        text = text.replace(f"\\{index}", group)
    text = re.sub(r"\\w\+", "word", text)
    text = re.sub(r"\.\+", "text", text)
    text = re.sub(r"\.\*", "", text)
    # Zero-width lookarounds constrain neighbours, not content.
    text = re.sub(r"\(\?[!=][^()]*\)", "", text)
    # Alternations inside a group: take the first branch, non-capturing first so
    # its `?:` does not survive into the capturing rule below.
    text = re.sub(r"\(\?:([^()|]*)\|[^()]*\)", r"\1", text)
    text = re.sub(r"\(([^()|]*)\|[^()]*\)", r"\1", text)
    # A digit range left by that first branch, e.g. `3[3-9]`.
    text = re.sub(r"\[(\d)-\d\]", r"\1", text)
    # Escaped literals are parked as sentinels first, so `\(aborted\)` keeps its
    # parentheses instead of losing them to the group-unwrapping below.
    parked: list[str] = []

    def park(match: re.Match[str]) -> str:
        parked.append(match.group(1))
        return f"\x01{len(parked) - 1}\x02"

    text = re.sub(r"\\([-\[\]().*+?{}|^$/\\])", park, text)
    # Remaining group wrappers, now unambiguous.
    text = text.replace("(?:", "(")
    text = re.sub(r"\((.*?)\)", r"\1", text)
    for index, literal in enumerate(parked):
        text = text.replace(f"\x01{index}\x02", literal)
    if re.search(pattern, text) is None:
        fail(f"cannot instantiate marker pattern: {pattern!r} -> {text!r}")
    return text


def required_of(gate) -> tuple[tuple[str, str], ...]:
    """Flatten declarations for synthesis and count pins, not matching."""
    return tuple(
        (f"{label}: {pattern}", pattern)
        for label, chain in chains_from_gate(gate)
        for pattern in chain
    )


def transcript_for(gate) -> str:
    lines = [literal_for(pattern) for _, pattern in required_of(gate)]
    return "\n".join(lines) + "\n"


def boot_plane_transcript(gate, marker_transcript: str) -> str:
    """Add the structural composition evidence required by the boot-plane gate.

    Init is the sole spawning parent. The three chain positions divide its child
    sequence, so each required spawn marker must be expanded with the preceding
    siblings instead of emitting the full sequence at the first position.
    """
    lines = marker_transcript.splitlines()
    children = list(gate.EXPECTED_INIT_CHILDREN)
    service_at = children.index("fabric-service")
    call_worker_at = children.index("fabric-call-worker")
    slices = {
        "component=fabric-service ": children[: service_at + 1],
        "component=fabric-call-worker ": children[service_at + 1 : call_worker_at + 1],
        "component=fabric-op-worker ": children[call_worker_at + 1 :],
    }

    def spawn_line(index: int, component: str) -> str:
        return (
            f"SLIME_GRAPH spawned task=0 child={201 + index} component={component} "
            "grants=1 endpoints=1 notifications=0 handle=1"
        )

    expanded: list[str] = []
    cursor = 0
    for line in lines:
        needle = next((key for key in slices if key in line), None)
        if needle is None:
            expanded.append(line)
            continue
        for component in slices[needle]:
            expanded.append(spawn_line(cursor, component))
            cursor += 1
    # Only components the gate's *own, unmutated* CHAINS declaration does not
    # already require an idle-without-role line for: chain 4/5 name nine of
    # the ten in causal order (readiness before each participant's own
    # marker). Derived from `gate.CHAINS` rather than from `lines` — `lines`
    # is this call's possibly-*mutated* transcript, and computing the filter
    # from it would silently re-add whichever marker the deletion mutation
    # below just removed, defeating the very test that removal drives. Only
    # `fabric-proxy` is absent from every chain.
    chain_literals = {
        literal_for(pattern) for _, chain in chains_from_gate(gate) for pattern in chain
    }
    extra_idle = [
        component
        for component in gate.EXPECTED_IDLE_WITHOUT_ROLE
        if f"[{component}] boot idle without a role" not in chain_literals
    ]
    expanded.extend(
        [
            "[layout] path=init slots=1 max=64",
            "[layout] 1 endpoint control",
            *(f"[{component}] boot role provisioned" for component in gate.EXPECTED_ROLE_HOLDERS),
            *gate.EXPECTED_PROVISIONED_EDGES,
            *(f"[{component}] boot idle without a role" for component in extra_idle),
            # `check_transcript` requires exactly one healthy-supervisor
            # terminal, but `TERMINAL_MARKER` is not in `REQUIRED_MARKERS`, so
            # the marker synthesis never produces one. Instantiated from the
            # gate's own pattern rather than written out here, so a change to
            # it cannot leave this stale.
            literal_for(gate.TERMINAL_MARKER),
        ]
    )
    return "\n".join(expanded) + "\n"


def marker_check(gate, transcript: str) -> None:
    """Invoke the exact matcher used by chain-aware product gates."""
    match_marker_contract(
        transcript,
        chains_from_gate(gate),
        gate.FAILURE_MARKERS,
        lambda message: (_ for _ in ()).throw(SystemExit(message)),
    )


def rejects(gate, transcript: str) -> bool:
    """True when the product gate refuses this transcript."""
    try:
        if gate.__name__ == "sel4_boot_plane":
            gate.check_transcript(transcript)
        else:
            marker_check(gate, transcript)
    except SystemExit:
        return True
    return False


def check_gate(name: str, relative_path: str, expected_required: int) -> int:
    gate = load_script(name, relative_path)
    required = required_of(gate)
    failures = getattr(gate, "FAILURE_MARKERS", ())
    if len(required) != expected_required:
        fail(
            f"{name}: declares {len(required)} required markers, expected "
            f"{expected_required}. A gate that lost a marker lost coverage; "
            "update the pin here only alongside the gate change that justifies it"
        )
    chains = chains_from_gate(gate)
    if not any(len(patterns) >= 2 for _, patterns in chains):
        fail(f"{name}: no causal chain has two markers, nothing to transpose")
    if not failures:
        fail(f"{name}: no failure markers declared")

    marker_baseline = transcript_for(gate)

    def complete(text: str) -> str:
        return boot_plane_transcript(gate, text) if name == "sel4_boot_plane" else text

    baseline = complete(marker_baseline)
    if rejects(gate, baseline):
        fail(
            f"{name}: rejected a transcript built from its own REQUIRED_MARKERS; "
            "the control cannot distinguish a real absence from its own synthesis"
        )

    lines = marker_baseline.splitlines()

    # Delete every occurrence of the selected concrete marker. A regex shared by
    # two chains may legitimately use either occurrence, so deleting only one
    # physical line is not evidence removal; deleting them all is.
    evaluated = 0
    for index, removed in enumerate(lines):
        without = "\n".join(line for line in lines if line != removed) + "\n"
        if not rejects(gate, complete(without)):
            description = required[index][0]
            fail(f"{name}: accepted a transcript missing all evidence for {description!r}")
        evaluated += 1

    offset = 0
    for _label, patterns in chains:
        if len(patterns) >= 2:
            first = lines[offset]
            second = lines[offset + 1]
            insertion = next(
                index for index, line in enumerate(lines) if line == first or line == second
            )
            remaining = [line for line in lines if line != first and line != second]
            transposed_lines = remaining[:insertion] + [second, first] + remaining[insertion:]
            transposed = "\n".join(transposed_lines) + "\n"
            if not rejects(gate, complete(transposed)):
                fail(f"{name}: accepted the first two markers of a causal chain out of order")
            evaluated += 1
        offset += len(patterns)

    # A failure marker must veto an otherwise-complete transcript.
    for pattern in failures:
        poisoned = baseline + literal_for(pattern) + "\n"
        if not rejects(gate, poisoned):
            fail(f"{name}: accepted a transcript containing failure marker {pattern!r}")

    if name == "sel4_boot_plane":
        baseline_lines = baseline.splitlines()
        idle = literal_for(r"\[fabric\] idle: parked on control endpoints")
        terminal = literal_for(gate.TERMINAL_MARKER)
        if idle not in baseline_lines or terminal not in baseline_lines:
            fail(f"{name}: synthetic transcript lacks its idle or healthy supervisor marker")
        terminal_index = baseline_lines.index(terminal)
        early_idle_then_exit = "\n".join(
            [
                *baseline_lines[:terminal_index],
                idle,
                "SLIME_GRAPH component exit task=17 status=-9",
                *baseline_lines[terminal_index:],
            ]
        ) + "\n"
        if not rejects(gate, early_idle_then_exit):
            fail(
                f"{name}: accepted early fabric idle followed by nonzero component exit "
                "before the healthy supervisor terminal"
            )
        nonzero_exit = next(
            pattern for pattern in failures if pattern.startswith("SLIME_GRAPH component exit")
        )
        if re.search(nonzero_exit, "SLIME_GRAPH component exit task=17 status=0"):
            fail(f"{name}: generic nonzero-exit failure marker also matches status zero")
        for status in ("7", "-9"):
            if re.search(
                nonzero_exit, f"SLIME_GRAPH component exit task=17 status={status}"
            ) is None:
                fail(f"{name}: generic nonzero-exit failure marker misses status {status}")
        evaluated += 1

    return evaluated + len(failures)


def check_layout_gate() -> int:
    """The boot-layout gate's structural validator, driven with broken fixtures.

    That gate's claim is fixture *equality*, so it has no marker table to mutate.
    But it also runs `check_shape` over every captured layout before comparing,
    and that validator has properties worth guarding: a header, a terminator,
    well-formed rows, a declared count matching the rows carried, and ascending
    slot numbers. Each is driven here from a real blessed fixture, so a
    `check_shape` that stopped enforcing one would be caught without a boot.
    """
    gate = load_script("sel4_boot_layout", "check/check-sel4-boot-layout.py")
    fixture = (
        ROOT / "contracts" / "boot-layout" / "v1" / "fixtures" / "sel4-channel.layout"
    )
    if not fixture.is_file():
        fail(f"missing blessed fixture: {fixture}")
    baseline = fixture.read_text(encoding="utf-8")

    def rejects_shape(text: str) -> bool:
        try:
            gate.check_shape("control", text)
        except SystemExit:
            return True
        return False

    if rejects_shape(baseline):
        fail("boot-layout gate rejected its own blessed fixture")

    lines = baseline.splitlines()
    mutations: tuple[tuple[str, str], ...] = (
        ("header removed", "\n".join(lines[1:]) + "\n"),
        ("terminator removed", "\n".join(lines[:-1]) + "\n"),
        (
            # Derive the mutation from the fixture so a layout-size change cannot
            # turn the control into a no-op.
            "declared count disagrees with the rows carried",
            re.sub(
                r"slots=(\d+)",
                lambda match: f"slots={int(match.group(1)) + 1}",
                baseline,
                count=1,
            ),
        ),
        (
            # Removing the first row's slot number is malformed for every layout
            # shape and does not depend on a particular blessed slot.
            "row is malformed",
            re.sub(r"\[layout\] \d+ ", "[layout] ", baseline, count=1),
        ),
        (
            "slot numbers descend",
            "\n".join([lines[0], lines[2], lines[1], *lines[3:]]) + "\n",
        ),
    )
    for description, text in mutations:
        if not rejects_shape(text):
            fail(f"boot-layout gate accepted a layout whose {description}")
    return len(mutations)

class ControlRejection(Exception):
    pass


def reject_control(message: str) -> None:
    raise ControlRejection(message)


def require_rejection(
    description: str, expected_message: str, action: Callable[[], object]
) -> None:
    try:
        action()
    except ControlRejection as error:
        if expected_message not in str(error):
            fail(f"{description} was rejected for the wrong reason: {error}")
    else:
        fail(f"{description} was accepted")


def check_image_identity_controls(root: _Path) -> int:
    image = root / "plane.img"
    manifest = root / "plane.identity.json"
    image.write_bytes(b"temporary seL4 plane image\n")
    digest = hashlib.sha256(image.read_bytes()).hexdigest()

    def write_identity(identity: object) -> None:
        manifest.write_text(json.dumps(identity), encoding="utf-8")

    valid_identity = {"variant": "control", "image": {"sha256": digest}}
    write_identity(valid_identity)
    verify_image_identity(
        image=image, manifest=manifest, variant="control", fail=reject_control
    )

    missing_image = root / "missing.img"
    controls: tuple[tuple[str, str, Callable[[], object]], ...] = (
        (
            "missing image identity control",
            "image missing",
            lambda: verify_image_identity(
                image=missing_image,
                manifest=manifest,
                variant="control",
                fail=reject_control,
            ),
        ),
        (
            "missing manifest identity control",
            "identity manifest missing",
            lambda: verify_image_identity(
                image=image,
                manifest=root / "missing.identity.json",
                variant="control",
                fail=reject_control,
            ),
        ),
    )
    for description, expected, action in controls:
        require_rejection(description, expected, action)

    malformed = root / "malformed.identity.json"
    malformed.write_text("{not json", encoding="utf-8")
    require_rejection(
        "malformed JSON identity control",
        "cannot parse identity manifest",
        lambda: verify_image_identity(
            image=image, manifest=malformed, variant="control", fail=reject_control
        ),
    )

    invalid_identities: tuple[tuple[str, object, str], ...] = (
        ("non-object identity control", [], "must contain an object"),
        (
            "wrong variant identity control",
            {"variant": "wrong", "image": {"sha256": digest}},
            "wrong image variant",
        ),
        (
            "missing image record identity control",
            {"variant": "control"},
            "has no image record",
        ),
        (
            "wrong digest identity control",
            {"variant": "control", "image": {"sha256": "0" * 64}},
            "digest does not match",
        ),
    )
    for description, identity, expected in invalid_identities:
        write_identity(identity)
        require_rejection(
            description,
            expected,
            lambda: verify_image_identity(
                image=image, manifest=manifest, variant="control", fail=reject_control
            ),
        )

    print("seL4 gate control check: image identity accepted 1 valid pair and rejected 7 invalid pairs")
    return 8


def write_qemu_stub(path: _Path) -> None:
    path.write_text(
        f"""#!{_sys.executable}
import os
import signal
import sys
import time
from pathlib import Path

pid_path = Path(os.environ["SLIME_QEMU_CONTROL_PID"])
stop_path = Path(os.environ["SLIME_QEMU_CONTROL_STOP"])
pid_path.write_text(str(os.getpid()), encoding="utf-8")

def stop(_signal, _frame):
    stop_path.write_text("stopped", encoding="utf-8")
    raise SystemExit(0)

signal.signal(signal.SIGTERM, stop)
mode = os.environ["SLIME_QEMU_CONTROL_MODE"]
if mode == "terminal":
    print("SLIME CONTROL TERMINAL", flush=True)
elif mode == "failure":
    print("SLIME CONTROL EARLY FAILURE", flush=True)
    raise SystemExit(7)
while True:
    time.sleep(1)
""",
        encoding="utf-8",
    )
    path.chmod(0o755)


def with_environment(updates: dict[str, str], action: Callable[[], object]) -> object:
    previous = {key: os.environ.get(key) for key in updates}
    os.environ.update(updates)
    try:
        return action()
    finally:
        for key, value in previous.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value


def require_stopped(pid_path: _Path, stop_path: _Path, description: str) -> None:
    if not stop_path.is_file():
        fail(f"{description} left the fake QEMU process without termination evidence")
    pid = int(pid_path.read_text(encoding="utf-8"))
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return
    fail(f"{description} left fake QEMU process {pid} alive")


def check_plane_runtime_controls(root: _Path) -> int:
    executable_dir = root / "bin"
    executable_dir.mkdir()
    qemu = executable_dir / "qemu-system-aarch64"
    write_qemu_stub(qemu)
    image = root / "runtime.img"
    image.write_bytes(b"runtime control image\n")
    pins = root / "pins.toml"
    pins.write_text(
        """[qemu_arm_virt]
machine = "virt"
cpu = "cortex-a53"
cpus = 1
memory_mib = 64
""",
        encoding="utf-8",
    )
    terminal = re.compile(r"SLIME CONTROL TERMINAL")

    def run(mode: str, timeout: int) -> str:
        pid_path = root / f"{mode}.pid"
        stop_path = root / f"{mode}.stopped"
        try:
            result = with_environment(
                {
                    "PATH": str(executable_dir),
                    "SLIME_QEMU_CONTROL_MODE": mode,
                    "SLIME_QEMU_CONTROL_PID": str(pid_path),
                    "SLIME_QEMU_CONTROL_STOP": str(stop_path),
                },
                lambda: run_plane(
                    image=image,
                    timeout=timeout,
                    terminal_condition=terminal,
                    fail=reject_control,
                    pins_path=pins,
                    cwd=root,
                ),
            )
            return str(result)
        finally:
            if mode in {"terminal", "timeout"} and pid_path.is_file():
                require_stopped(pid_path, stop_path, f"{mode} runtime control")

    transcript = run("terminal", 2)
    if "SLIME CONTROL TERMINAL" not in transcript:
        fail("terminal runtime control returned no terminal evidence")

    require_rejection(
        "timeout runtime control",
        "timed out after 1s",
        lambda: run("timeout", 1),
    )
    require_rejection(
        "early process failure runtime control",
        "exited with status 7",
        lambda: run("failure", 2),
    )
    empty_path = root / "empty-path"
    empty_path.mkdir()
    require_rejection(
        "missing QEMU runtime control",
        "not on PATH",
        lambda: with_environment(
            {"PATH": str(empty_path)},
            lambda: run_plane(
                image=image,
                timeout=1,
                terminal_condition=terminal,
                fail=reject_control,
                pins_path=pins,
                cwd=root,
            ),
        ),
    )

    print(
        "seL4 gate control check: runtime returned terminal evidence and rejected "
        "timeout, early process failure, and missing QEMU"
    )
    return 4



def main() -> None:
    if Path_cwd() != ROOT:
        fail(f"run from repository root: {ROOT}")
    total = 0
    for name, relative_path, expected_required in GATES:
        total += check_gate(name, relative_path, expected_required)
    total += check_layout_gate()
    with tempfile.TemporaryDirectory(prefix="slime-sel4-gate-controls-") as temporary:
        control_root = _Path(temporary)
        identity_controls = check_image_identity_controls(control_root)
        runtime_controls = check_plane_runtime_controls(control_root)
    print(
        f"seL4 gate control check: {len(GATES) + 1} gates reject "
        f"{total} mutated transcripts and layouts; "
        f"{identity_controls} identity cases and {runtime_controls} runtime cases passed"
    )


def Path_cwd() -> _Path:
    return _Path.cwd().resolve()


if __name__ == "__main__":
    main()
