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
    ("sel4_io_network_plane", "check/check-sel4-io-network-plane.py", 39),
    ("sel4_component_graph", "check/check-sel4-component-graph.py", 30),
    ("sel4_crossing_plane", "check/check-sel4-crossing-plane.py", 10),
    ("sel4_loan_plane", "check/check-sel4-loan-plane.py", 46),
    ("sel4_io_queue_plane", "check/check-sel4-io-queue-plane.py", 15),
    ("sel4_io_link_plane", "check/check-sel4-io-link-plane.py", 28),
    ("sel4_io_driver_authority_plane", "check/check-sel4-io-driver-authority-plane.py", 16),
    ("sel4_device_plane", "check/check-sel4-device-plane.py", 2),
    ("sel4_root_boot", "check/check-sel4-root-boot.py", 59),
    ("sel4_sample_plane", "check/check-sel4-sample-plane.py", 25),
    ("sel4_spawn_plane", "check/check-sel4-spawn-plane.py", 27),
    ("sel4_supervision_plane", "check/check-sel4-supervision-plane.py", 12),
    # 24 ceiling markers, MEM-64M's 11 reuse-cycle markers, and MEM-1G's 8
    # simultaneous-capacity plus 11 isolation markers.
    ("sel4_private_memory_plane", "check/check-sel4-private-memory-plane.py", 54),
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
    # An enumerated digit class such as `[01]`: a marker whose field is a small
    # closed set rather than an open-ended number. Matched before the ranged
    # classes below because those all contain a hyphen and cannot collide, and
    # the first member is as valid an instantiation as any other.
    text = re.sub(r"\[(\d+)\]", lambda m: m.group(1)[0], text)
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


def check_root_memory_runtime_control() -> int:
    """A larger launcher alone cannot manufacture kernel-visible high RAM.

    Driven against `check_ordinary_memory`, which is where the claim lives: the
    marker table can only say the fields are well-formed, while the validator
    is what ties the summary to the enumerated ranges, the probe to the highest
    granule-capable range, and `beyond_legacy` to the platform's own superseded
    window. Each mutation below is a way a `-m`-only change or a regressed
    validator would read as success.
    """
    gate = load_script("sel4_root_boot_memory_control", "check/check-sel4-root-boot.py")
    # An ARM inventory in the shape the kernel publishes: ascending ranges with
    # real page-sized/aligned frame-capable extents plus a final sub-page tail.
    # The addresses are the observed qemu-arm-virt ones.
    ranges = (
        (0x6000_0000, 16 * 4096),
        (0x8000_0000, 131072 * 4096),
        (0xBF94_8000, 2 * 4096),
        (0xBF94_BF00, 256),
    )

    def inventory(
        entries: tuple[tuple[int, int], ...],
        probe_paddr: int = 0xBF94_8000,
        probe_bytes: int = 4096,
        beyond_legacy: int = 1,
    ) -> str:
        records = [
            f"SLIME_ROOT ordinary range={index} paddr={paddr:#x} bytes={size}"
            for index, (paddr, size) in enumerate(entries)
        ]
        records.append(
            f"SLIME_ROOT ordinary ranges={len(entries)} "
            f"bytes={sum(size for _, size in entries)} "
            f"end={max(paddr + size for paddr, size in entries):#x}"
        )
        records.append(
            f"SLIME_ROOT ordinary probe paddr={probe_paddr:#x} bytes={probe_bytes} "
            f"beyond_legacy={beyond_legacy} verified=1"
        )
        return "\n".join(records) + "\n"

    baseline = inventory(ranges)

    def rejects_inventory(transcript: str, platform: str = "qemu-arm-virt") -> bool:
        try:
            gate.check_ordinary_memory(transcript, platform)
        except SystemExit:
            return True
        return False

    if rejects_inventory(baseline):
        fail("sel4_root_boot: rejected its synthesized ordinary-memory baseline")
    mutations = (
        (
            "a probe that never left the range the superseded platform contained",
            inventory(ranges, probe_paddr=0x6000_0000),
        ),
        (
            "a probe reporting it stayed inside the legacy window",
            inventory(ranges, beyond_legacy=0),
        ),
        (
            "a zero-byte probe placed in the inventory's sub-page tail",
            inventory(ranges, probe_paddr=0xBF94_BF00, probe_bytes=0),
        ),
        (
            "a page-sized probe with a misaligned physical address",
            inventory(ranges, probe_paddr=0xBF94_8001),
        ),
        (
            "an aligned page-sized probe extending past its selected range",
            inventory(
                (*ranges[:2], (0xBF94_8000, 6144), ranges[3]),
                probe_paddr=0xBF94_9000,
            ),
        ),
        (
            "a summary claiming an end no enumerated range reaches",
            baseline.replace("end=0xbf94c000", "end=0x100000000"),
        ),
        (
            "a summary byte total that does not sum its ranges",
            baseline.replace(
                f"bytes={sum(size for _, size in ranges)} end=", "bytes=2147483648 end="
            ),
        ),
        (
            "a summary counting ranges the transcript never enumerated",
            baseline.replace(f"ranges={len(ranges)} ", "ranges=34 "),
        ),
        (
            "the whole inventory deleted while the probe line remains",
            "\n".join(
                line for line in baseline.splitlines() if "ordinary range=" not in line
            )
            + "\n",
        ),
    )
    for description, mutated in mutations:
        if not rejects_inventory(mutated):
            fail(f"sel4_root_boot: accepted {description}")
    # The same evidence read as RISC-V, whose DRAM base *is* the ARM legacy end:
    # a platform with no superseded window must not be credited with crossing
    # one, or `beyond_legacy` would be true by arithmetic rather than by RAM.
    if not rejects_inventory(baseline, "qemu-riscv-virt"):
        fail(
            "sel4_root_boot: credited qemu-riscv-virt with passing a superseded "
            "window it never had"
        )
    print(
        "seL4 gate control check: root memory evidence rejected "
        f"{len(mutations) + 1} launcher-only and inventory mutations"
    )
    return len(mutations) + 1


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


def capacity_workload_transcript() -> str:
    """One synthetic run of the 1 GiB workload, in the plane's exact grammar."""
    bases = {0: 0x10000000, 1: 0x20000000, 2: 0x30000000, 3: 0x40000000}
    guard = bases[3] + 65536 * 4096
    census = "slots=96 bytes=268435456 live_objects=12 mapped_pages=196608 extent_reuses=4"
    lines: list[str] = []

    def spawn(task: int, holder: int, incarnation: int) -> None:
        name = "abcd"[holder]
        lines.extend([
            f"SLIME_MEM quota task={task} instance=private-memory-1g-holder-{name} "
            f"declared=65536 installed=65536 base=0x{bases[holder]:x}",
            f"SLIME_GRAPH spawned task=1 child={task} component=private-memory-1g-holder-{name} slots=8",
            f"SLIME_MEM grown task={task} delta=65536 previous=0 pages=65536 "
            f"base=0x{bases[holder]:x} quota=65536 total={min(holder + 1, 4) * 65536} "
            "large_frames=128 base_frames=0 leaf_tables=0",
            f"[private-memory-1g] zeroed holder={holder} incarnation={incarnation} pages=65536",
        ])

    def verify(task: int, holder: int, incarnation: int, round_index: int) -> None:
        lines.append(
            f"SLIME_MEM refused task={task} delta=1 cause=reservation "
            "detail=ReservationExceeded { pages: 65536, delta: 1, reservation: 65536 }"
        )
        lines.append(
            f"SLIME_MEM grown task={task} delta=0 previous=65536 pages=65536 "
            f"base=0x{bases[holder]:x} quota=65536 total=262144 large_frames=128 "
            "base_frames=0 leaf_tables=0"
        )
        if holder == 0:
            lines.extend([
                f"SLIME_GRAPH buffer created task={task} slot=9 id=3 pages=1 writable=1",
                f"SLIME_GRAPH buffer create refused task={task} pages=1 class=quota",
                f"SLIME_MEM mapping refused task={task} base=0x{bases[0]:x} "
                f"end=0x{bases[0] + 4096:x} window=0x{bases[0]:x}..0x{guard:x}",
            ])
        lines.append(
            f"[private-memory-1g] verified holder={holder} incarnation={incarnation} "
            f"round={round_index} pages=65536 refused=1 shared={int(holder == 0)}"
        )

    current = [2, 3, 4, 5]
    rounds = [0, 0, 0, 0]
    for holder in range(4):
        spawn(current[holder], holder, 0)
    lines.append("[private-memory-1g] resident holders=4 pages=262144")
    for cycle in range(20):
        for holder in range(4):
            verify(current[holder], holder, cycle if holder == 3 else 0, rounds[holder])
            rounds[holder] += 1
        lines.extend([
            f"SLIME_GRAPH component fault task={current[3]} kind=VirtualMemory "
            f"{{ access: Write, status: 15 }} address=Some({guard})",
            f"SLIME_MEM census retired={current[3]} {census}",
            f"SLIME_LIFECYCLE restart admitted task=1 subject={current[3]} attempt={cycle} "
            f"remaining={19 - cycle} ready_at=1000",
        ])
        current[3] = 6 + cycle
        rounds[3] = 0
        spawn(current[3], 3, cycle + 1)
        for holder in range(4):
            verify(current[holder], holder, cycle + 1 if holder == 3 else 0, rounds[holder])
            rounds[holder] += 1
        lines.append(f"[private-memory-1g] retained cycle={cycle + 1} holders=4 pages=262144")
    for task in current:
        lines.extend([
            f"SLIME_GRAPH component exit task={task} status=0",
            f"SLIME_MEM census retired={task} {census}",
        ])
    lines.extend([
        "[private-memory-1g] complete faults=20 replacements=20 exits=4",
        "SLIME_GRAPH loans served=40 loans=0 mappings=0 regions=0 orphans=0 quota=0",
        "SLIME_GRAPH HEALTHY generation=56 required=2 live=0 completed=2 failed=0",
    ])
    return "\n".join(lines)


def check_private_capacity_controls(gate) -> int:
    """The 1 GiB claim must fail closed: a peer that was not resident during a
    reclamation, a replacement that reused a dead incarnation's identity, a
    fault charged to a surviving holder, and a skipped restart admission are
    each rejected rather than absorbed as an equivalent run."""
    transcript = capacity_workload_transcript()
    gate.check_capacity_workload(transcript)
    mutations = (
        ("capacity peers not resident during reclamation", transcript.replace("mapped_pages=196608", "mapped_pages=131072", 1)),
        ("capacity census missing for a dead incarnation", transcript.replace("SLIME_MEM census retired=5 slots=96 bytes=268435456 live_objects=12 mapped_pages=196608 extent_reuses=4\n", "", 1)),
        ("capacity replacement reuses a dead task identity", transcript.replace("child=6 component=private-memory-1g-holder-d", "child=5 component=private-memory-1g-holder-d", 1)),
        ("capacity aggregate below the declared total", transcript.replace("quota=65536 total=262144", "quota=65536 total=196608", 1)),
        ("capacity fault charged to a surviving peer", transcript.replace("SLIME_GRAPH component fault task=5 ", "SLIME_GRAPH component fault task=2 ", 1)),
        ("capacity fault away from the guard page", transcript.replace(f"address=Some({0x40000000 + 65536 * 4096})", "address=Some(1074790400)", 1)),
        ("capacity restart admission skipped", transcript.replace("SLIME_LIFECYCLE restart admitted task=1 subject=5 attempt=0 remaining=19 ready_at=1000\n", "", 1)),
        ("capacity restart ordinal repeated", transcript.replace("subject=6 attempt=1 remaining=18", "subject=6 attempt=0 remaining=19", 1)),
        ("capacity replacement served stale pages", transcript.replace("[private-memory-1g] zeroed holder=3 incarnation=1 pages=65536\n", "", 1)),
        ("capacity peer verification dropped after a replacement", transcript.replace("[private-memory-1g] verified holder=1 incarnation=0 round=1 pages=65536 refused=1 shared=0\n", "", 1)),
        ("capacity holder admitted beyond its quota", transcript.replace("SLIME_MEM refused task=2 delta=1 cause=reservation detail=ReservationExceeded { pages: 65536, delta: 1, reservation: 65536 }\n", "", 1)),
        ("capacity completion claimed twice", transcript + "\n[private-memory-1g] complete faults=20 replacements=20 exits=4"),
        ("capacity holder left running at teardown", transcript.replace("SLIME_GRAPH component exit task=2 status=0\n", "", 1)),
    )
    for description, mutated in mutations:
        require_rejection(description, "capacity workload:",
            lambda mutated=mutated: gate.check_capacity_workload(mutated))
    return len(mutations)


def check_private_isolation_controls(gate) -> int:
    """MEM-1G's denial evidence must be unforgeable: a survived foreign access,
    a fault charged to the wrong task, an attacker without its own window, and
    a denial claimed before the fault that proves it each differ from isolation
    only in the transcript, never in the plane's outcome."""
    address = 0x10000000
    probe = address + 65536 * 4096 // 2
    lines = [
        f"[private-memory-isolation] victim base={address} pages=65536",
        f"SLIME_MEM quota task=1 instance=private-memory-1g-holder-a declared=65536 installed=65536 base=0x{address:x}",
        f"SLIME_MEM quota task=2 instance=private-memory-1g-holder-b declared=1 installed=1 base=0x{address:x}",
        f"SLIME_MEM quota task=3 instance=private-memory-1g-holder-c declared=1 installed=1 base=0x{address:x}",
        f"SLIME_MEM grown task=1 delta=65536 previous=0 pages=65536 base=0x{address:x} quota=65536 total=65536 large_frames=128 base_frames=0 leaf_tables=0",
        f"SLIME_MEM grown task=2 delta=1 previous=0 pages=1 base=0x{address:x} quota=1 total=65537 large_frames=0 base_frames=1 leaf_tables=1",
        f"SLIME_MEM grown task=3 delta=1 previous=0 pages=1 base=0x{address:x} quota=1 total=65538 large_frames=0 base_frames=1 leaf_tables=1",
        "SLIME_GRAPH buffer created task=1 slot=20 id=1 pages=1 writable=1",
    ]
    for source, receiver, lender, recipient, loan, exported in ((1, 2, 2, 3, 10, 30), (2, 1, 3, 2, 11, 31)):
        lines.extend([
            f"SLIME_GRAPH buffer created task={lender} slot=20 id={source + 1} pages=1 writable=1",
            f"SLIME_GRAPH loan created task={lender} slot=21 id={loan} to={recipient} offset=0 length=4096",
            f"SLIME_GRAPH capability exported task={lender} id={exported} kind=loan rights=0x200 retain=0",
            f"SLIME_GRAPH capability imported task={recipient} id={exported} kind=loan rights=0x200 retain=0",
            *[
                f"SLIME_MEM mapping refused task={recipient} base=0x{target:x} end=0x{target + 4096:x} window=0x{address:x}..0x{address + 65536 * 4096:x}"
                for target in (address, probe)
            ],
            f"SLIME_GRAPH loan mapped task={recipient} slot=22 id={loan}",
            f"SLIME_GRAPH loan returned task={recipient} slot=22 id={loan}",
            f"[private-memory-isolation] loan receiver source={source} holder={receiver} positive=1 window_denied=2 unmapped=1 returned=1 return_denied=1 pages=0 buffers=0 mappings=0 loans=0",
            f"[private-memory-isolation] loan lender holder={source} receiver={receiver} transferred=1 released=1 pages=0 buffers=0 mappings=0 loans=0",
        ])
    for holder, task, operation, access in ((1, 2, 6, "Read"), (2, 3, 7, "Write")):
        lines.extend([
            f"SLIME_GRAPH buffer created task={task} slot=20 id={holder + 3} pages=1 writable=1",
            *[
                f"SLIME_MEM mapping refused task={task} base=0x{target:x} end=0x{target + 4096:x} window=0x{address:x}..0x{address + 65536 * 4096:x}"
                for target in (address, probe)
            ],
            f"SLIME_GRAPH buffer map refused task={task} slot=20 class=write",
            f"[private-memory-isolation] attacker holder={holder} operation={operation} address={probe} own_base={address} own_pages=1 own_buffer=1 buffer_window_denied=2 unowned_denied=3 seal_denied=1 pages=0 buffers=0 mappings=0 loans=0",
            f"SLIME_GRAPH component fault task={task} kind=VirtualMemory {{ access: {access}, status: 15 }} address=Some({probe})",
            f"[private-memory-1g] verified holder=0 incarnation=0 round={holder - 1} pages=65536 refused=1 shared=1",
            f"[private-memory-isolation] denied holder={holder} operation={operation} address={probe} victim_preserved=1",
        ])
    lines.extend([
        f"[private-memory-isolation] execute address={address} pages=65536",
        f"SLIME_GRAPH component fault task=1 kind=VirtualMemory {{ access: Execute, status: 15 }} address=Some({address})",
        "[private-memory-isolation] complete read=1 write=1 execute=1 buffer_window_denied=4 loan_window_denied=4 unowned_denied=6 seal_denied=2 loan_transfer=2",
        "SLIME_GRAPH native task_caps=0 exports=0 tickets=0",
        "SLIME_GRAPH capabilities exports=2 imports=2 cancels=0 finalized=2 outstanding=0 tickets=0",
        "SLIME_GRAPH loans served=2 loans=0 mappings=0 regions=0 orphans=0 quota=0",
        "SLIME_GRAPH HEALTHY generation=57 required=2 live=0 completed=2 failed=0",
    ])
    transcript = "\n".join(lines)
    gate.check_private_isolation(transcript)
    mutations = (
        ("isolation read survived", transcript.replace("SLIME_GRAPH component fault task=2 kind=VirtualMemory { access: Read, status: 15 } " f"address=Some({probe})\n", "")),
        ("isolation fault charged to victim", transcript.replace("fault task=2 ", "fault task=1 ", 1)),
        ("isolation write reported as read", transcript.replace("access: Write", "access: Read", 1)),
        ("isolation fault outside the victim's extent", transcript.replace(f"access: Read, status: 15 }} address=Some({probe})", f"access: Read, status: 15 }} address=Some({address + 65536 * 4096})", 1)),
        ("isolation probe inside the attacker's own window", transcript.replace(f"address={probe} own_base", f"address={address} own_base", 1)),
        ("isolation stale fault status", transcript.replace("status: 15", "status: 0", 1)),
        ("isolation attacker quota differs from its declaration", transcript.replace("instance=private-memory-1g-holder-b declared=1 installed=1", "instance=private-memory-1g-holder-b declared=1 installed=4")),
        ("isolation attacker has no window of its own", transcript.replace(f"SLIME_MEM grown task=2 delta=1 previous=0 pages=1 base=0x{address:x} quota=1 total=65537 large_frames=0 base_frames=1 leaf_tables=1\n", "", 1)),
        ("isolation attacker page outlived its incarnation", transcript.replace("quota=1 total=65537", "quota=1 total=65538", 1)),
        ("isolation attacker window origin differs", transcript.replace(f"instance=private-memory-1g-holder-b declared=1 installed=1 base=0x{address:x}", "instance=private-memory-1g-holder-b declared=1 installed=1 base=0x99000000")),
        ("isolation victim pattern lost after the denial", transcript.replace("[private-memory-1g] verified holder=0 incarnation=0 round=1 pages=65536 refused=1 shared=1", "[private-memory-1g] FAIL pattern mismatch")),
        ("isolation victim unverified", transcript.replace("[private-memory-1g] verified holder=0 incarnation=0 round=0 pages=65536 refused=1 shared=1\n", "")),
        ("isolation denial claimed before the fault that proves it", transcript.replace(
            f"SLIME_GRAPH component fault task=2 kind=VirtualMemory {{ access: Read, status: 15 }} address=Some({probe})\n"
            f"[private-memory-1g] verified holder=0 incarnation=0 round=0 pages=65536 refused=1 shared=1\n"
            f"[private-memory-isolation] denied holder=1 operation=6 address={probe} victim_preserved=1\n",
            f"[private-memory-isolation] denied holder=1 operation=6 address={probe} victim_preserved=1\n"
            f"SLIME_GRAPH component fault task=2 kind=VirtualMemory {{ access: Read, status: 15 }} address=Some({probe})\n"
            f"[private-memory-1g] verified holder=0 incarnation=0 round=0 pages=65536 refused=1 shared=1\n")),
        ("isolation NX never attempted", transcript.replace(f"[private-memory-isolation] execute address={address} pages=65536\n", "")),
        ("isolation execute fault away from the victim's own base", transcript.replace(f"access: Execute, status: 15 }} address=Some({address})", f"access: Execute, status: 15 }} address=Some({probe})")),
        ("isolation victim not one large-frame set", transcript.replace("large_frames=128 base_frames=0", "large_frames=0 base_frames=65536")),
        ("isolation explicit component failure", transcript.replace("[private-memory-isolation] complete", "[private-memory-1g] FAIL pattern mismatch\n[private-memory-isolation] complete")),
        ("isolation leaked authority at exit", transcript.replace("loans=0 mappings=0 regions=0 orphans=0 quota=0", "loans=0 mappings=1 regions=1 orphans=0 quota=0")),
    )
    mutations = list(mutations)
    for source, receiver, lender, recipient, loan, exported in ((1, 2, 2, 3, 10, 30), (2, 1, 3, 2, 11, 31)):
        receiver_marker = next(line for line in lines if line.startswith(f"[private-memory-isolation] loan receiver source={source} "))
        lender_marker = next(line for line in lines if line.startswith(f"[private-memory-isolation] loan lender holder={source} "))
        created = f"SLIME_GRAPH loan created task={lender} slot=21 id={loan} to={recipient} offset=0 length=4096"
        imported = f"SLIME_GRAPH capability imported task={recipient} id={exported} kind=loan rights=0x200 retain=0"
        mapped = f"SLIME_GRAPH loan mapped task={recipient} slot=22 id={loan}"
        returned = f"SLIME_GRAPH loan returned task={recipient} slot=22 id={loan}"
        direction = f"isolation loan {source}->{receiver}: "
        for label, old, new in (
            ("source buffer absent", f"SLIME_GRAPH buffer created task={lender} slot=20 id={source + 1} pages=1 writable=1\n", ""),
            ("creation absent", created + "\n", ""),
            ("receiver binding wrong", created, created.replace(f"to={recipient}", "to=1")),
            ("import absent", imported + "\n", ""),
            ("transfer identity mismatch", imported, imported.replace(f"id={exported}", "id=999")),
            ("imported by wrong task", imported, imported.replace(f"task={recipient}", "task=1")),
            ("positive map absent", mapped + "\n", ""),
            ("positive map wrong loan", mapped, mapped.replace(f"id={loan}", "id=999")),
            ("return absent", returned + "\n", ""),
            ("return wrong slot", returned, returned.replace("slot=22", "slot=23")),
            ("return precedes map", mapped + "\n" + returned, returned + "\n" + mapped),
            ("positive read failed", receiver_marker, receiver_marker.replace("positive=1", "positive=0")),
            ("explicit unmap absent", receiver_marker, receiver_marker.replace("unmapped=1", "unmapped=0")),
            ("single return not enforced", receiver_marker, receiver_marker.replace("return_denied=1", "return_denied=0")),
            ("receiver retains mapping", receiver_marker, receiver_marker.replace("mappings=0", "mappings=1")),
            ("receiver report duplicated", receiver_marker, receiver_marker + "\n" + receiver_marker),
            ("lender source not released", lender_marker, lender_marker.replace("released=1", "released=0")),
            ("lender retains buffer", lender_marker, lender_marker.replace("buffers=0", "buffers=1")),
            ("lender release precedes return", receiver_marker + "\n" + lender_marker, lender_marker + "\n" + receiver_marker),
        ):
            mutations.append((direction + label, transcript.replace(old, new, 1)))
        for target in (address, probe):
            refusal = f"SLIME_MEM mapping refused task={recipient} base=0x{target:x} end=0x{target + 4096:x} window=0x{address:x}..0x{address + 65536 * 4096:x}"
            mutations.append((direction + f"window refusal {target:x} absent", transcript.replace(refusal + "\n", "", 1)))
            mutations.append((direction + f"window refusal {target:x} wrong task", transcript.replace(refusal, refusal.replace(f"task={recipient}", "task=1"), 1)))
    for holder, task in ((1, 2), (2, 3)):
        marker = next(line for line in lines if line.startswith(f"[private-memory-isolation] attacker holder={holder} "))
        for field in ("own_buffer=1", "buffer_window_denied=2", "unowned_denied=3", "seal_denied=1"):
            mutations.append((f"isolation attacker {holder} missing {field}", transcript.replace(marker, marker.replace(field, field.split("=")[0] + "=0"), 1)))
        sealed = f"SLIME_GRAPH buffer map refused task={task} slot=20 class=write"
        mutations.append((f"isolation attacker {holder} seal denial missing", transcript.replace(sealed + "\n", "", 1)))
        mutations.append((f"isolation attacker {holder} seal refusal wrong capability", transcript.replace(sealed, sealed.replace("slot=20", "slot=21"), 1)))
    late_growth = f"SLIME_MEM grown task=3 delta=1 previous=0 pages=1 base=0x{address:x} quota=1 total=65538 large_frames=0 base_frames=1 leaf_tables=1"
    mutations.extend([
        ("isolation receiver private page backed only after exchange", transcript.replace(late_growth + "\n", "", 1) + "\n" + late_growth),
        ("isolation export survives teardown", transcript.replace("outstanding=0 tickets=0", "outstanding=1 tickets=1")),
        ("isolation native capability survives teardown", transcript.replace("task_caps=0 exports=0", "task_caps=1 exports=0")),
        ("isolation transfer not finalized", transcript.replace("finalized=2", "finalized=1")),
        ("isolation extra loan mapping", transcript + "\nSLIME_GRAPH loan mapped task=3 slot=22 id=10"),
        ("isolation duplicate loan identity", transcript.replace("id=11", "id=10")),
        ("isolation duplicate transfer identity", transcript.replace("id=31", "id=30")),
    ])
    for description, mutated in mutations:
        if mutated == transcript:
            fail(f"{description}: mutation did not change the transcript")
        require_rejection(description, "private isolation:",
            lambda mutated=mutated: gate.check_private_isolation(mutated))
    return len(mutations)


def check_private_stress_controls(gate) -> int:
    address = 0x10000000
    lines = []
    for index, name in enumerate("abc"):
        task = index + 2
        lines += [
            f"SLIME_GRAPH spawned task=1 child={task} component=private-memory-1g-holder-{name} slots=8",
            f"SLIME_MEM grown task={task} delta=65536 previous=0 pages=65536 base=0x{address:x} quota=65536 total={(index + 1) * 65536} large_frames=128 base_frames=0 leaf_tables=0",
        ]
    for attempt, case in enumerate(("construction", "slots", "descriptors")):
        lines += [
            f"SLIME_MEM stress census attempt={attempt} phase=before slots=1000 descriptors=1000 extents=100 objects=100 bytes=1000 reusable_anchors=0 reusable_bytes=0 ordinary_bytes=10000 preserved_bytes=0 preserved_anchors=0",
            f"SLIME_MEM stress construction case={case} attempt={attempt} actual=1000 effective=1 required=2 reserved={0 if case == 'construction' else 999}",
            f"SLIME_MEM stress census attempt={attempt} phase=after slots=1000 descriptors=1000 extents=100 objects=100 bytes=1000 reusable_anchors=0 reusable_bytes=0 ordinary_bytes=10000 preserved_bytes=0 preserved_anchors=0",
            *[f"[private-memory-1g] verified holder={holder} incarnation=0 round=0 pages=65536 refused=1 shared={int(holder == 0)}" for holder in range(3)],
            f"[private-memory-stress] spawn_refused attempt={attempt + 1} peers_preserved=3",
        ]
    for incarnation, task in enumerate((8, 9)):
        lines += [
            f"SLIME_MEM quota task={task} instance=private-memory-1g-holder-d declared=65536 installed=65536 base=0x{address:x}",
            f"SLIME_GRAPH spawned task=1 child={task} component=private-memory-1g-holder-d slots=8",
        ]
        schedule = [(12, 1), (12, 511)]
        if incarnation == 0:
            schedule += [(13, 1024)]
        schedule += [(12, 1024), (12, 63998)]
        if incarnation == 0:
            schedule += [(13, 2)]
        schedule += [(12, 2), (14, 0)]
        pages = 0
        for stage, (operation, delta) in enumerate(schedule):
            previous = pages
            if operation == 13:
                case, backed = ("map", 512) if previous == 512 else ("allocation", 1)
                lines += [
                    f"SLIME_MEM stress growth case={case} previous={previous} delta={delta} backed={backed}",
                    f"SLIME_MEM refused task={task} delta={delta} cause=frames detail=Frames {{ allocated: {backed}, error: NotEnoughMemory }}",
                ]
            elif operation == 12:
                pages += delta
                large = max(0, pages // 512 - 1)
                base = pages - large * 512
                lines += [f"SLIME_MEM grown task={task} delta={delta} previous={previous} pages={pages} base=0x{address:x} quota=65536 total={196608 + pages} large_frames={large} base_frames={base} leaf_tables=2"]
            lines += [f"[private-memory-stress] stage incarnation={incarnation} operation={operation} delta={delta} previous={previous} pages={pages} zeroed={int(operation == 12)} preserved=1"]
            if operation != 14:
                lines += [f"[private-memory-1g] verified holder={holder} incarnation=0 round={stage} pages=65536 refused=1 shared={int(holder == 0)}" for holder in range(3)]
                lines += [f"[private-memory-stress] retained incarnation={incarnation} stage={stage} peers=3 pages=196608"]
        if incarnation == 0:
            lines += [f"SLIME_GRAPH component exit task={task} status=0"]
        else:
            lines += [f"SLIME_GRAPH component fault task={task} kind=VirtualMemory {{ access: Write, status: 15 }} address=Some({address + 65536 * 4096})"]
        lines += [f"SLIME_MEM census retired={task} mapped_pages=196608 free_slots=1000"]
        lines += [f"SLIME_ROOT reclaim census task={task} slots=1000 bytes=1000 live_objects=100 extent_reuses={incarnation + 1}"]
        lines += [f"[private-memory-1g] verified holder={holder} incarnation=0 round=0 pages=65536 refused=1 shared={int(holder == 0)}" for holder in range(3)]
    lines += [f"SLIME_GRAPH component exit task={task} status=0" for task in (2, 3, 4)]
    lines += [
        "[private-memory-stress] complete spawn_refused=3 growth_refused=2 retries=2 replacements=1 faults=1 exits=4",
        "SLIME_GRAPH native task_caps=0 exports=0 tickets=0",
        "SLIME_GRAPH loans served=0 loans=0 mappings=0 regions=0 orphans=0 quota=0",
        "SLIME_GRAPH HEALTHY generation=58 required=2 live=0 completed=2 failed=0",
    ]
    lines += ["SLIME_MEM stress fragmented guards=4 bytes=16777216 released=1"]
    lines += [f"SLIME_MEM stress backing attempt={attempt} data_extents=128 discontinuities=3 reused={0 if attempt == 0 else 256}" for attempt in (0, 1, 3, 4)]
    transcript = "\n".join(lines)
    gate.check_stress_workload(transcript)
    mutations = []
    for index, line in enumerate(lines):
        if line.startswith(("SLIME_MEM stress ", "[private-memory-stress]", "[private-memory-1g] verified", "SLIME_MEM grown", "SLIME_GRAPH component")):
            mutations.append((f"stress missing evidence {index}", "\n".join(lines[:index] + lines[index + 1:])))
    for description, old, new in (
        ("slot limit not binding", "case=slots attempt=1 actual=1000 effective=1 required=2", "case=slots attempt=1 actual=1000 effective=2 required=2"),
        ("unwind leaks slots", "phase=after slots=1000", "phase=after slots=999"),
        ("unwind leaks descriptors", "phase=after slots=1000 descriptors=1000", "phase=after slots=1000 descriptors=999"),
        ("wrong failed subject", "refused task=8 delta=1024", "refused task=2 delta=1024"),
        ("mapping failure before partial work", "case=map previous=512 delta=1024 backed=512", "case=map previous=512 delta=1024 backed=0"),
        ("wrong retry population", "quota=65536 total=198144", "quota=65536 total=198143"),
        ("peer reclaimed with subject", "retired=8 mapped_pages=196608", "retired=8 mapped_pages=131072"),
        ("foreign fault", "fault task=9", "fault task=2"),
        ("backing never reused", "extent_reuses=2", "extent_reuses=1"),
        ("reclamation drift", "retired=9 mapped_pages=196608 free_slots=1000", "retired=9 mapped_pages=196608 free_slots=999"),
        ("pressure not reserved", "required=2 reserved=999", "required=2 reserved=0"),
        ("backing contiguous", "discontinuities=3", "discontinuities=0"),
        ("fragmented backing not reused", "reused=256", "reused=0"),
        ("only base pages", "large_frames=127 base_frames=512", "large_frames=0 base_frames=65536"),
        ("leaked export", "task_caps=0 exports=0", "task_caps=0 exports=1"),
    ):
        mutations.append((description, transcript.replace(old, new, 1)))
    for description, mutated in mutations:
        if mutated == transcript:
            fail(f"{description}: stress mutation did not change evidence")
        require_rejection(description, "private stress:", lambda mutated=mutated: gate.check_stress_workload(mutated))
    peer_checks = [f"[private-memory-1g] verified holder={holder} incarnation=0 round=0 pages=65536 refused=1 shared={int(holder == 0)}" for holder in range(3)]
    heap_lines = []
    for index, name in enumerate("abc"):
        heap_lines += [
            f"SLIME_GRAPH spawned task=1 child={index + 2} component=private-memory-1g-holder-{name} slots=8",
            f"SLIME_MEM grown task={index + 2} delta=65536 previous=0 pages=65536 base=0x10000000 quota=65536 total={(index + 1) * 65536} large_frames=128 base_frames=0 leaf_tables=0",
        ]
    heap_lines += [
        "SLIME_MEM quota task=5 instance=private-heap-probe declared=65536 installed=65536 base=0x10000000",
        "SLIME_GRAPH spawned task=1 child=5 component=private-heap-probe slots=8",
        *peer_checks,
        "SLIME_MEM grown task=5 delta=61457 previous=0 pages=61457 base=0x10000000 quota=65536 total=258065 large_frames=119 base_frames=529 leaf_tables=2",
        "[private-heap-probe:stress] capacity payload=251723776 overhead=4096 backed=251727872 pages=61457 touched=1 vecs=120 boxes=120 small=256",
        "[private-heap-probe:stress] holes reused=1 growths=1 pages=61457",
        "[private-heap-probe:stress] exhaustion requested=16777216 refused=1 intact=1 pages=61457",
        *peer_checks,
        "[private-heap-probe:stress] verified payload=251723776 intact=1",
        *peer_checks,
        "[private-heap-probe:stress] released live=0 reused=1 growths=1 pages=61457",
        "SLIME_GRAPH component exit task=5 status=0",
        *peer_checks,
        *[f"SLIME_GRAPH component exit task={task} status=0" for task in (2, 3, 4)],
        "[private-memory-stress] heap_complete peers=3 exits=4",
        "SLIME_GRAPH native task_caps=0 exports=0 tickets=0",
        "SLIME_GRAPH loans served=0 loans=0 mappings=0 regions=0 orphans=0 quota=0",
        "SLIME_GRAPH HEALTHY generation=59 required=2 live=0 completed=2 failed=0",
    ]
    heap = "\n".join(heap_lines)
    gate.check_heap_stress_workload(heap)
    for index in range(len(heap_lines)):
        mutated = "\n".join(heap_lines[:index] + heap_lines[index + 1:])
        require_rejection(f"heap stress missing evidence {index}", "heap stress:", lambda mutated=mutated: gate.check_heap_stress_workload(mutated))
    heap_mutations = (
        ("heap wrong quota", heap.replace("installed=65536", "installed=65535", 1)),
        ("heap wrong root charge", heap.replace("total=258065", "total=258064", 1)),
        ("heap reuse grew backing", heap.replace("released live=0 reused=1 growths=1", "released live=0 reused=1 growths=2", 1)),
        ("heap wrong task", heap.replace("grown task=5", "grown task=6", 1)),
        ("heap explicit failure", heap + "\n[private-heap-probe:stress] FAIL injected"),
    )
    for description, mutated in heap_mutations:
        require_rejection(description, "heap stress:", lambda mutated=mutated: gate.check_heap_stress_workload(mutated))
    return len(mutations) + len(heap_lines) + len(heap_mutations)


def check_private_memory_capacity_controls() -> int:
    gate = load_script(
        "sel4_private_memory_semantic_controls", "check/check-sel4-private-memory-plane.py"
    )
    gate.fail = reject_control
    profile = {"memory_mib": 2048}
    section = "control"
    target = "aarch64-sel4-qemu-virt"
    qualification = (
        "SLIME_MEM qualification scope=contract-aggregate-headroom holders=4 "
        "pages=65536 private_allocations=263168 private_extents=1028 private_cslots=264196 "
        "private_reserved=1075838976 payload=1073741824 tables=2097152 alignment=0 "
        "static_allocations=8 static_reserved=16384 required_allocations=263200 "
        "required_extents=1028 required_cslots=264228 required_reserved=1075904512 "
        "allocation_capacity=400000 allocations_available=380000 extent_capacity=2048 "
        "extents_available=1500 cslots_available=500000 ordinary_available=2013265920 "
        "ordinary_layout=1 root_image=8388608 root_metadata=1048576 root_stack=1048576 "
        "root_heap=524288 fit=1"
    )
    gate.check_segmented_capacity_report(qualification, profile, section, target, 0)
    # The headroom the report claims must follow the quotas the generation
    # already declares, not the whole aggregate a fresh image would admit.
    require_rejection(
        "capacity headroom ignores already-declared quotas",
        "capacity qualification:",
        lambda: gate.check_segmented_capacity_report(qualification, profile, section, target, 65536),
    )
    capacity_mutations = (
        ("capacity false refusal", qualification[:-1] + "0"),
        (
            "capacity missing static descriptors",
            qualification.replace("required_allocations=263200", "required_allocations=263168"),
        ),
        (
            "capacity missing static RAM",
            qualification.replace("required_reserved=1075904512", "required_reserved=1075838976"),
        ),
        (
            "capacity ignores impossible ordinary layout",
            qualification.replace("ordinary_layout=1", "ordinary_layout=0")[:-1] + "1",
        ),
        ("capacity allocation exhaustion", qualification.replace("allocations_available=380000", "allocations_available=263199")),
        ("capacity extent exhaustion", qualification.replace("extents_available=1500", "extents_available=1027")),
        ("capacity slot exhaustion", qualification.replace("cslots_available=500000", "cslots_available=264227")),
        ("capacity ordinary exhaustion", qualification.replace("ordinary_available=2013265920", "ordinary_available=1075904511")),
        ("capacity small tables", qualification.replace("allocation_capacity=400000 allocations_available=380000", "allocation_capacity=4096 allocations_available=3000")),
        ("capacity envelope below the declared row", qualification.replace("holders=4 pages=65536", "holders=2 pages=65536")),
        ("capacity duplicate report", qualification + "\n" + qualification),
        (
            "capacity missing static field",
            qualification.replace(" static_allocations=8", ""),
        ),
    )
    for description, transcript in capacity_mutations:
        require_rejection(
            description,
            "capacity qualification:",
            lambda transcript=transcript: gate.check_segmented_capacity_report(
                transcript, profile, section, target, 0
            ),
        )
    conversion_lines = [
        "SLIME_MEM refused task=5 delta=16384 cause=frames detail=Frames { allocated: 0, error: Retype }",
        "SLIME_MEM grown task=5 delta=1 previous=0 pages=1 base=0x400000 quota=16384 total=1 large_frames=0 base_frames=1 leaf_tables=1",
        "SLIME_MEM grown task=5 delta=16383 previous=1 pages=16384 base=0x400000 quota=16384 total=16384 large_frames=31 base_frames=512 leaf_tables=1",
        "[private-memory-probe] granted pages=16384 base=0x400000 zeroed=1 survived=1 refused=1 worker_rpc_once=1 worker_grow_refused=1 retries=1",
    ]
    conversion = "\n".join(conversion_lines)
    gate.check_large_map_retry(conversion)
    conversion_mutations = (
        ("conversion missing first page", "\n".join(conversion_lines[:1] + conversion_lines[2:])),
        ("conversion reported no base pages", conversion.replace("base_frames=512", "base_frames=0")),
        ("conversion demoted every span", conversion.replace("large_frames=31 ", "large_frames=0 ")),
        ("conversion changed task", conversion.replace("task=5 delta=16383", "task=6 delta=16383")),
        ("conversion changed base", conversion.replace("previous=1 pages=16384 base=0x400000", "previous=1 pages=16384 base=0x600000")),
        ("conversion wrong order", "\n".join([conversion_lines[0], conversion_lines[2], conversion_lines[1], conversion_lines[3]])),
    )
    for description, transcript in conversion_mutations:
        require_rejection(
            description,
            "large-map",
            lambda transcript=transcript: gate.check_large_map_retry(transcript),
        )


    rollback_lines = [
        "SLIME_MEM refused task=0 delta=2 cause=frames detail=Frames { allocated: 1, error: Retype }",
        "SLIME_CHILD mem rollback preserved pages=1 base=0x40000000 survived=0x4d454d5f42415345",
        "SLIME_MEM refused task=1 delta=2 cause=frames detail=Frames { allocated: 1, error: Retype }",
        "SLIME_CHILD mem zero rollback result=-5 pages=0 base=0x50000000",
        "SLIME_MEM grown task=1 delta=512 previous=0 pages=512 base=0x50000000 quota=512 total=513 large_frames=0 base_frames=512 leaf_tables=1",
        "SLIME_CHILD mem whole retry pages=512 base=0x50000000 zeroed=1 preserved=1 survived=0x4d454d5f42415345",
        "SLIME_CHILD fault requested addr=0x0",
        "SLIME_ROOT child fault observed task=1 role=deliberate-fault kind=VirtualMemory { access: Write, level: 0 } instruction=Some(0) address=Some(0)",
        "SLIME_MEM enforced clean_quota=4 retry_quota=512 pages=513 grants=2 grown=513 reclaimed=0 flags=0x7f",
        "SLIME_MEM teardown grown=513 reclaimed=513 pages=0",
        "SLIME_ROOT READY tasks=2 grants=2 declared_grants=2 reclaimed_slots=600",
    ]
    rollback = "\n".join(rollback_lines)
    gate.check_incremental_rollback(rollback)
    reordered = rollback_lines.copy()
    reordered[3], reordered[4] = reordered[4], reordered[3]
    rollback_mutations = (
        ("rollback missing zero marker", "\n".join(line for index, line in enumerate(rollback_lines) if index != 3)),
        ("rollback reordered zero and bulk", "\n".join(reordered)),
        ("rollback used large frame", rollback.replace("large_frames=0", "large_frames=1")),
        ("rollback explicit failure", rollback + "\nSLIME_CHILD mem zero retry failed"),
        ("rollback missing ready", "\n".join(rollback_lines[:-1])),
    )
    for description, transcript in rollback_mutations:
        require_rejection(
            description,
            "private rollback:",
            lambda transcript=transcript: gate.check_incremental_rollback(transcript),
        )
    quota_lines = [
        "SLIME_MEM quota task=3 instance=private-heap-granted declared=15872 "
        "installed=15872 base=0x4000000",
        "[private-heap-probe:granted] capacity payload=62914560 overhead=16 "
        "backed=63008768 pages=15383 touched=1",
        "SLIME_MEM refused task=3 delta=50153 cause=quota "
        "detail=QuotaExceeded { pages: 15383, delta: 50153, quota: 15872 }",
    ]
    quota_refusal = "\n".join(quota_lines)
    declared = {"private-heap-granted": 15872}
    gate.check_heap_refusal_is_the_declared_quota(
        quota_refusal, declared, "qemu-arm-virt"
    )
    quota_mutations = (
        (
            "heap quota refusal names reservation instead",
            quota_refusal.replace(
                "cause=quota detail=QuotaExceeded",
                "cause=reservation detail=ReservationExceeded",
            ),
        ),
        (
            "heap quota refusal names a different quota",
            quota_refusal.replace("quota: 15872", "quota: 16384"),
        ),
        (
            "heap quota refusal request stops short of the reservation",
            quota_refusal.replace("delta=50153", "delta=50152"),
        ),
        (
            "heap quota installation differs from the declaration",
            quota_refusal.replace("installed=15872", "installed=16384"),
        ),
        (
            "heap quota refusal is attributed to a different task",
            quota_refusal.replace("refused task=3", "refused task=4"),
        ),
    )
    for description, transcript in quota_mutations:
        require_rejection(
            description,
            "private-heap-granted:",
            lambda transcript=transcript: gate.check_heap_refusal_is_the_declared_quota(
                transcript, declared, "qemu-arm-virt"
            ),
        )
    cold = (
        "SLIME_MEM census retired=0 untyped=10000 reusable=0 shared_reusable=0 "
        "preserved_bytes=0 active_extent_bytes=100 mapped_pages=0 free_slots=100 "
        "anchors=0 shared_anchors=0 preserved_anchors=0 allocations_free=100\n"
        "SLIME_ALLOC preserved parent=1 slot=2 paddr=4096 bytes=16\n"
        "SLIME_MEM census retired=1 untyped=9000 reusable=1000 shared_reusable=16 "
        "preserved_bytes=84 active_extent_bytes=0 mapped_pages=0 free_slots=96 "
        "anchors=2 shared_anchors=1 preserved_anchors=1 allocations_free=102"
    )
    gate.check_capacity_conservation(cold)
    conservation_mutations = (
        ("cold backing lost", cold.replace("untyped=9000", "untyped=8999")),
        ("cold backing double counted", cold.replace("preserved_bytes=84", "preserved_bytes=85")),
        ("cold slots lost", cold.replace("free_slots=96", "free_slots=95")),
        ("cold mapping remains", cold.replace("active_extent_bytes=0 mapped_pages=0", "active_extent_bytes=0 mapped_pages=1")),
        ("cold descriptor lost", cold.replace("allocations_free=102", "allocations_free=99")),
        ("cold missing initial backing", cold.replace("active_extent_bytes=100", "active_extent_bytes=0")),
        ("cold no real reuse", "\n".join(line for line in cold.splitlines() if not line.startswith("SLIME_ALLOC"))),
    )
    for description, transcript in conservation_mutations:
        require_rejection(description, "capacity conservation:",
            lambda transcript=transcript: gate.check_capacity_conservation(transcript))
    ledger = "\n".join([
        "SLIME_BACKING inventory parent=1 paddr=4096 bytes=4096",
        "SLIME_BACKING preserve parent=1 child=2 paddr=4096 bytes=16",
        "SLIME_ROOT ordinary range=0 paddr=0x1000 bytes=4096",
        "SLIME_MEM census untyped=4080 preserved_bytes=16 preserved_anchors=1 active_extent_bytes=0 reusable=0 anchors=0 shared_reusable=0 shared_retained=0 shared_anchors=0",
        "SLIME_BACKING snapshot phase=initial begin",
        "SLIME_BACKING ordinary parent=1 paddr=4096 bytes=4096 used=16",
        "SLIME_BACKING retained parent=2 paddr=4096 bytes=16 used=0",
        "SLIME_BACKING snapshot phase=initial end",
        "SLIME_BACKING preserve parent=1 child=3 paddr=4112 bytes=16",
        "SLIME_BACKING consume source=ordinary parent=1 slot=4 paddr=4128 bytes=32 count=1",
        "SLIME_BACKING consume source=preserved parent=3 slot=5 paddr=4112 bytes=16 count=1",
        "SLIME_MEM census untyped=4032 preserved_bytes=16 preserved_anchors=2 active_extent_bytes=0 reusable=32 anchors=1 shared_reusable=16 shared_retained=16 shared_anchors=1",
        "SLIME_BACKING snapshot phase=final begin",
        "SLIME_BACKING ordinary parent=1 paddr=4096 bytes=4096 used=64",
        "SLIME_BACKING retained parent=2 paddr=4096 bytes=16 used=0",
        "SLIME_BACKING retained parent=3 paddr=4112 bytes=16 used=16",
        "SLIME_BACKING task_extent parent=4 bytes=32 active=0",
        "SLIME_BACKING shared_node cap=5 parent=0 paddr=4112 bytes=16 state=Free",
        "SLIME_BACKING snapshot phase=final end",
    ])
    gate.check_backing_ledger(ledger)
    ledger_mutations = (
        ("ledger omitted prefix", ledger.replace("SLIME_BACKING preserve parent=1 child=3 paddr=4112 bytes=16\n", "")),
        ("ledger overlapping ownership", ledger.replace("child=3 paddr=4112", "child=3 paddr=4096")),
        ("ledger duplicate parent", ledger.replace("child=3 paddr=4112", "child=2 paddr=4112")),
        ("ledger false reusable consumed leaf", ledger.replace("parent=3 paddr=4112 bytes=16 used=16", "parent=3 paddr=4112 bytes=16 used=0")),
        ("ledger duplicate extent", ledger.replace("SLIME_BACKING task_extent parent=4 bytes=32 active=0", "SLIME_BACKING task_extent parent=4 bytes=32 active=0\nSLIME_BACKING task_extent parent=4 bytes=32 active=0")),
        ("ledger incomplete snapshot", ledger.replace("SLIME_BACKING snapshot phase=final end", "")),
        ("ledger wrong inventory", ledger.replace("ordinary range=0 paddr=0x1000", "ordinary range=0 paddr=0x2000")),
        ("ledger invalid shared ancestry", ledger.replace("shared_node cap=5 parent=0", "shared_node cap=5 parent=99")),
    )
    for description, transcript in ledger_mutations:
        require_rejection(description, "backing ledger:",
            lambda transcript=transcript: gate.check_backing_ledger(transcript))
    fault_lines = []
    for cycle in range(gate.CYCLE_COUNT):
        task = cycle + 1
        base = 0x10000000
        fault_lines.extend([
            f"SLIME_MEM quota task={task} instance=private-cycle-probe declared=16384 installed=16384 base=0x{base:x}",
            f"SLIME_MEM grown task={task} delta=16384 previous=0 pages=16384 base=0x{base:x} quota=16384 total=16384 large_frames=32 base_frames=0 leaf_tables=0",
            f"[private-cycle-probe] cycle={cycle} pages=16384 base=0x{base:x} stamp=0x{cycle + 1:x} zeroed=1 verified=1 end={'fault' if cycle % 2 else 'exit'}",
        ])
        if cycle % 2:
            access = "Write" if cycle % 4 == 1 else "Execute"
            address = base + (16384 * 4096 if access == "Write" else 0)
            fault_lines.append(f"SLIME_GRAPH component fault task={task} kind=VirtualMemory {{ access: {access}, status: 15 }} address=Some({address})")
    fault_trace = "\n".join(fault_lines)
    gate.check_cycle_fault_attribution(fault_trace, 16384)
    fault_mutations = (
        ("fault wrong task", fault_trace.replace("fault task=2 ", "fault task=99 ")),
        ("fault wrong access", fault_trace.replace("access: Write", "access: Read", 1)),
        ("fault stale status", fault_trace.replace("status: 15", "status: 0", 1)),
        ("fault stale address", fault_trace.replace("address=Some(335544320)", "address=Some(9)", 1)),
        ("fault NX at guard", fault_trace.replace("access: Execute, status: 15 } address=Some(268435456)", "access: Execute, status: 15 } address=Some(335544320)", 1)),
        ("fault missing record", "\n".join(line for line in fault_lines if not line.startswith("SLIME_GRAPH component fault task=2 "))),
        ("fault duplicate record", fault_trace + "\n" + fault_lines[6]),
        ("fault missing backed region", fault_trace.replace("grown task=4 delta=16384", "grown task=99 delta=16384", 1)),
    )
    for description, transcript in fault_mutations:
        require_rejection(description, "cycle fault attribution:",
            lambda transcript=transcript: gate.check_cycle_fault_attribution(transcript, 16384))
    isolation_mutations = check_private_isolation_controls(gate)
    stress_mutations = check_private_stress_controls(gate)
    workload_mutations = check_private_capacity_controls(gate)
    return (
        workload_mutations
        + isolation_mutations
        + stress_mutations
        + len(fault_mutations)
        + len(ledger_mutations)
        + len(conservation_mutations)
        + len(capacity_mutations)
        + len(rollback_mutations)
        + len(conversion_mutations)
        + len(quota_mutations)
    )



def main() -> None:
    if Path_cwd() != ROOT:
        fail(f"run from repository root: {ROOT}")
    total = 0
    for name, relative_path, expected_required in GATES:
        total += check_gate(name, relative_path, expected_required)
    total += check_root_memory_runtime_control()
    total += check_layout_gate()
    total += check_private_memory_capacity_controls()
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
