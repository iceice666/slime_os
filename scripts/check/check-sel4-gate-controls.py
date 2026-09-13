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
    ("sel4_component_graph", "check/check-sel4-component-graph.py", 29),
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
    # 24 ceiling markers plus MEM-64M's 11 reuse-cycle markers.
    ("sel4_private_memory_plane", "check/check-sel4-private-memory-plane.py", 35),
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


def check_private_memory_capacity_controls() -> int:
    gate = load_script(
        "sel4_private_memory_semantic_controls", "check/check-sel4-private-memory-plane.py"
    )
    gate.fail = reject_control
    profile = {"memory_mib": 2048}
    section = "control"
    qualification = (
        "SLIME_MEM qualification scope=staged-graph-plus-admitted-holder-clones holders=2 "
        "pages=16384 private_allocations=32896 private_extents=130 private_cslots=33026 "
        "private_reserved=134479872 payload=134217728 tables=262144 alignment=0 "
        "static_allocations=8 static_reserved=16384 required_allocations=32912 "
        "required_extents=130 required_cslots=33042 required_reserved=134512640 "
        "allocation_capacity=288416 allocations_available=280000 extent_capacity=1072 "
        "extents_available=1050 cslots_available=500000 ordinary_available=2013265920 "
        "ordinary_layout=1 root_image=8388608 root_metadata=1048576 root_stack=1048576 "
        "root_heap=524288 fit=1"
    )
    gate.check_segmented_capacity_report(qualification, profile, section)
    capacity_mutations = (
        ("capacity false refusal", qualification[:-1] + "0"),
        (
            "capacity missing static descriptors",
            qualification.replace("required_allocations=32912", "required_allocations=32896"),
        ),
        (
            "capacity missing static RAM",
            qualification.replace("required_reserved=134512640", "required_reserved=134479872"),
        ),
        (
            "capacity ignores impossible ordinary layout",
            qualification.replace("ordinary_layout=1", "ordinary_layout=0")[:-1] + "1",
        ),
        ("capacity allocation exhaustion", qualification.replace("allocations_available=280000", "allocations_available=32911")),
        ("capacity extent exhaustion", qualification.replace("extents_available=1050", "extents_available=129")),
        ("capacity slot exhaustion", qualification.replace("cslots_available=500000", "cslots_available=33041")),
        ("capacity ordinary exhaustion", qualification.replace("ordinary_available=2013265920", "ordinary_available=134512639")),
        ("capacity small tables", qualification.replace("allocation_capacity=288416 allocations_available=280000", "allocation_capacity=4096 allocations_available=3000")),
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
                transcript, profile, section
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
        "SLIME_MEM refused task=3 delta=1001 cause=quota "
        "detail=QuotaExceeded { pages: 15383, delta: 1001, quota: 15872 }",
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
            quota_refusal.replace("delta=1001", "delta=1000"),
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
    return (
        len(capacity_mutations)
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
