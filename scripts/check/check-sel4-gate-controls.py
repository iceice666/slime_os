#!/usr/bin/env python3
"""Prove that the seL4 plane gates actually fail when their evidence is absent.

Every seL4 plane gate is a marker-matching Python checker: it boots an image and
asserts that an ordered sequence of serial markers appears and that no failure
marker does. Nothing in-repo demonstrated that a *missing* marker makes one of
them red. The oracle had `should_panic.rs` for exactly this — proof that a failing
assertion is observable at all — and the seL4 side had no equivalent, leaving
per-slice fault injection (per-change discipline) as the only mitigation.

This is that standing guard. For each gate it builds a synthetic transcript from
the gate's own `REQUIRED_MARKERS`, checks the gate accepts it, and then checks the
gate rejects three mutations of it:

* one required marker deleted,
* the first two required markers transposed,
* a failure marker appended.

The transcripts are synthetic on purpose. A negative control must be able to
produce evidence that is *wrong in one specific way*, which no real boot can be
asked to do — and building them from each gate's own marker table means the
control cannot drift out of step with the gate it guards.

What this does not claim: that the markers are the right markers, or that a real
boot emits them. The plane gates themselves assert that. This asserts only that
those assertions have teeth.
"""

from __future__ import annotations

import re
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

from harness import ROOT, load_script  # noqa: E402
from sel4_gate_markers import chains_from_gate, match_marker_contract  # noqa: E402

# Gate module name -> checker path, for every plane gate that shares the
# `REQUIRED_MARKERS` / `FAILURE_MARKERS` / `check_transcript` shape.
#
# `check-sel4-boot-layout.py` is absent deliberately: it compares frozen fixtures
# rather than markers, so it does not expose the surface this control drives. The
# stream gate declares `CHAINS` instead of a flat table and is handled by
# `required_of`.
# Third element is the number of required markers the gate is expected to declare.
#
# Pinned rather than derived, because the whole point is to notice when a gate
# gets *weaker*: deleting a marker from a gate's table would otherwise just make
# this control report a smaller number and still pass. Raising a count here is a
# deliberate act that shows up in review; silently losing coverage is not.
#
# B46 deliberately shortened the channel and call contracts when the logical
# ChannelTable/WaitSet paths disappeared. The replacement contracts assert the
# native endpoint lifecycle instead; their lower counts are therefore pinned
# here rather than mistaken for accidental coverage loss. The larger counts on
# the other affected gates pin the additional native-authority evidence added
# during the same cutover.
#
# B50's minted-endpoint deletion lowers ten gates. Every one of them asserted at
# least one marker the cutover left with no subject: an `endpoint minted` /
# `channel copied` pair no operation produces, a `capability transfer …
# channel=… side=…` line replaced by an export/import pair, a `parked …
# reason=wait` / `supervision woken` pair `WaitSet` used to emit, or an
# `idle root copy` a `startup_arg` discriminator the root no longer feeds.
#
# None of the compositions lost coverage. The spawn plane's wide array is now
# six narrowed directory views instead of six endpoint halves, and its grant
# counts are still pinned at the spawn marker; the supervision plane's B25
# derive scenario is restored and its two-collections-per-child check is
# stronger than the export/import pair it replaced; each probe plane's
# `idle without a run token` line moved from an ordered marker to a presence
# assertion, because the idle instance concludes it holds no peer only after a
# bounded wait and so lands wherever the scheduler puts it.
GATES: tuple[tuple[str, str, int], ...] = (
    # 16 -> 18 for CP2's two runtime-binding markers: the root's own
    # `SLIME_GRAPH binding unresolved` line and console's `ungranted binding
    # denied`. The pin exists to make a marker-count change deliberate, and this
    # one is: the channel plane now also guards that a component cannot resolve a
    # binding its instance was never granted.
    ("sel4_channel_plane", "check/check-sel4-channel-plane.py", 18),
    # C10.2: 30 -> 31. This generation declares no `privateMemoryBudget`, which
    # is the case 31 of the 32 fixtures are in and the private-memory plane
    # cannot state — it exists to carry a budget. The new marker is the root
    # reporting it found none, paired with two failure markers that make "and
    # therefore every component is denied" an assertion rather than an
    # inference.
    ("sel4_component_graph", "check/check-sel4-component-graph.py", 31),
    ("sel4_crossing_plane", "check/check-sel4-crossing-plane.py", 10),
    ("sel4_loan_plane", "check/check-sel4-loan-plane.py", 46),
    ("sel4_device_plane", "check/check-sel4-device-plane.py", 7),
    # C10.1: 43 -> 58. Fifteen private-memory markers, each a root record paired
    # with the child observation it cannot itself make: the size query, both
    # growths, the zeroed pages read at the reported base, the surviving pattern
    # and the address it was read back from, the quota refusal and its named
    # cause, the refusal having had no effect, the child's full report, the
    # root's adjudication against its own page accounting, and the teardown
    # returning every page. Raised deliberately: the count going *up* is the
    # milestone's evidence, and the pin is what makes a later reduction visible.
    ("sel4_root_boot", "check/check-sel4-root-boot.py", 58),
    ("sel4_sample_plane", "check/check-sel4-sample-plane.py", 25),
    ("sel4_spawn_plane", "check/check-sel4-spawn-plane.py", 27),
    ("sel4_supervision_plane", "check/check-sel4-supervision-plane.py", 12),
    # C10.2: eleven markers, pairing what the root enforced with what the two
    # probes observed — the admitted budget, all three installed ceilings
    # (including init's zero), the granted holder's size query, its quota
    # refusal and named cause, its measured ceiling with the zeroed reads and
    # surviving pattern, the omitted holder's reservation refusal, and the
    # unchanged region afterwards.
    ("sel4_private_memory_plane", "check/check-sel4-private-memory-plane.py", 11),
    ("sel4_stream_plane", "check/check-sel4-stream-plane.py", 57),
    ("sel4_qos_plane", "check/check-sel4-qos-plane.py", 14),
    # RP2. 29 markers over five causal chains: the generation's declared shape,
    # the C7 exchange, the C8 provisioning, the product graph, and the drain.
    # That is the slice arm only, and the count pins exactly it.
    #
    # The gate's other two arms are *uncovered by this control*, stated plainly
    # rather than implied covered. Both expect a root fatal instead of the
    # healthy terminal: the wrong-target arm matches its own
    # `WRONG_TARGET_MARKERS` table, and the rollback arm declares no table at all
    # — its assertions are an inline regex in `expect_selected` plus terminal
    # strings passed to `boot`. Neither is a `CHAINS`-shaped surface, which is
    # the only surface this control drives.
    ("sel4_demo_plane", "check/check-sel4-demo-plane.py", 29),
    # C8.11. Six rather than seven: the peer-death chain dropped its trailing
    # "and then a clock advance" marker when the gate grew from one plane to all
    # three. That marker asserted a *scenario* shape -- on the call plane the
    # death is at the final instant and nothing follows it -- while the
    # arrangement of records within an instant is checked structurally by the
    # gate's own `check_order` on every plane. Coverage went up, not down: the
    # gate now reads three workers instead of one.
    ("sel4_trace_plane", "check/check-sel4-trace-plane.py", 6),
    ("sel4_call_plane", "check/check-sel4-call-plane.py", 47),
    ("sel4_operation_plane", "check/check-sel4-operation-plane.py", 53),
    # B70: +1 each. Both gates gained "SLIME_ROOT fabric interposition
    # hop=<name>", the root's own resolution of the declared chain's hop
    # identity back to a generation instance name. It replaces an
    # `assert_declared_chain` inside each broker that compared a
    # build-time table against a constant compiled beside it -- a check
    # that stayed green when the fixture named a different proxy.
    ("sel4_visibility_plane", "check/check-sel4-visibility-plane.py", 26),
    # B73 raised this from 25: the graph-wide view's route order is now
    # asserted by `fabric-publisher`, the half the plane never read.
    ("sel4_matrix_plane", "check/check-sel4-matrix-plane.py", 26),
    # C8.13: three chains -- admission, init's single-threaded spawn order, and
    # the close -- deliberately short. Everything a genuinely concurrent
    # schedule cannot guarantee an order for (per-plane traffic markers,
    # resource evidence, which of three workers settles first) is checked as
    # membership by `check_resources`/`check_concurrency`/`check_task_lifecycle`
    # instead, on B55's rule: a chain that pinned a scheduling accident would
    # be a flaky gate, not a stronger one.
    ("sel4_traffic_plane", "check/check-sel4-traffic-plane.py", 10),
    # C8.13's saturation fixture reuses the traffic plane's exact `CHAINS`
    # shape (declared ceilings tightened, not the admitted structure), so it
    # is pinned at the same count.
    ("sel4_saturation_plane", "check/check-sel4-saturation-plane.py", 10),
    # C8.14's fault fixture likewise reuses the traffic plane's exact `CHAINS`
    # shape -- it is the same graph with the interposition hop compiled to die,
    # not a restructured composition -- so it is pinned at the same count. Its
    # fault-specific tables are asserted outside `CHAINS`, since a concurrent
    # schedule fixes no order among them.
    ("sel4_fault_plane", "check/check-sel4-fault-plane.py", 10),
    # B55: the full-graph boot restoration moved the seven racy cross-task
    # stream markers (a broker per-edge print racing a participant's own
    # summary print differently for one-route vs two-route participants) out
    # of CHAINS and into `EXPECTED_ROLE_HOLDERS`/`EXPECTED_PROVISIONED_EDGES`,
    # order-independent membership checks exactly like the pre-existing
    # `EXPECTED_IDLE_WITHOUT_ROLE`. Real coverage did not shrink: every one of
    # those markers is still required by `check_composition`, just no longer
    # asserted as a fixed scheduling interleaving that was never true.
    ("sel4_boot_plane", "check/check-sel4-boot-plane.py", 30),
    ("sel4_storage_plane", "check/check-sel4-storage-plane.py", 9),
    ("sel4_store_plane", "check/check-sel4-store-plane.py", 14),
    ("sel4_rollback_plane", "check/check-sel4-rollback-plane.py", 16),
    ("sel4_recovery_plane", "check/check-sel4-recovery-plane.py", 11),
    ("sel4_generation_plane", "check/check-sel4-generation-plane.py", 18),
    ("sel4_directory_plane", "check/check-sel4-directory-plane.py", 16),
    ("sel4_filesystem_plane", "check/check-sel4-filesystem-plane.py", 11),
    ("sel4_input_plane", "check/check-sel4-input-plane.py", 7),
    ("sel4_powerbox_plane", "check/check-sel4-powerbox-plane.py", 11),
    ("sel4_dango_plane", "check/check-sel4-dango-plane.py", 13),
    ("sel4_transfer_plane", "check/check-sel4-transfer-plane.py", 11),
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

    Init is the sole spawning parent (B55): the stream broker and both bounded
    route workers are among its nineteen children, not spawned by a second
    parent. The chain still names three positions in that one sequence —
    `fabric-service`, `fabric-call-worker`, `fabric-op-worker` — each preceded
    by every sibling init spawns before it, so the synthesis slices
    `EXPECTED_INIT_CHILDREN` at those same two names and drops each slice in
    at its own chain-required line rather than bundling all nineteen at the
    first one.
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
            # Derived from the fixture rather than hardcoded: the channel
            # plane's layout has grown before and will again, and a literal
            # `slots=N` that no longer appears makes this mutation a no-op —
            # a control that silently stops controlling.
            "declared count disagrees with the rows carried",
            re.sub(
                r"slots=(\d+)",
                lambda match: f"slots={int(match.group(1)) + 1}",
                baseline,
                count=1,
            ),
        ),
        (
            # Derived, for the reason the mutation above is: this hardcoded
            # `[layout] 3 endpoint`, and the channel plane's layout no longer
            # reaches slot 3, so the mutation replaced nothing and the control
            # passed a transcript it had not mutated. Dropping the slot number
            # from whichever row is first is malformed in every layout.
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


def main() -> None:
    if Path_cwd() != ROOT:
        fail(f"run from repository root: {ROOT}")
    total = 0
    for name, relative_path, expected_required in GATES:
        total += check_gate(name, relative_path, expected_required)
    total += check_layout_gate()
    print(
        f"seL4 gate control check: {len(GATES) + 1} gates reject "
        f"{total} mutated transcripts and layouts"
    )


def Path_cwd() -> _Path:
    return _Path.cwd().resolve()


if __name__ == "__main__":
    main()
