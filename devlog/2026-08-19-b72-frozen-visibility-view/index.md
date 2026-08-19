# B72: the visibility plane's QoS records are decoded and frozen

| Field | Value |
|---|---|
| Date | 2026-08-19 |
| Kind | Defect |
| Status | Verified |
| Scope | `scripts/check/check-sel4-visibility-plane.py`, `contracts/fabric-visibility/v1/{schema,gen_rust}.zt`, `scripts/generate/generate-fabric-visibility-bindings.py`, `scripts/lib/fabric_visibility_contract.py`, `contracts/fabric-visibility/v1/fixtures/sel4-visibility.view`, `Justfile` |
| Roadmap | B72 |
| Gates | `just sel4_visibility_check`, `just sel4_visibility_bless`, `just contracts_check`, `just sel4_gate_control_check` |
| Trigger | Observed 2026-08-19 while migrating `visibility_broker` off the generated tables (B70/CP2) |
| Baseline | The plane's twelve view records were counted; no field of any record was compared to anything |

## Summary

`check-sel4-visibility-plane.py` required the composition to emit exactly twelve
view records and exactly two distinct interposition traces, but never decoded a
record. Every field the broker copies out of the declared graph — each route's
name and its transport QoS — was unconstrained, so a mutation that swapped the
two routes' declared QoS left the gate green. The gate now decodes all twelve
records using offsets the visibility contract renders itself, and compares every
field against a frozen fixture. The exit-condition mutation now fails the gate,
naming the two records that moved.

## Observable symptom

- Command: `just sel4_visibility_check`, after swapping the declared QoS of
  `telemetry` and `diagnostics` in `contracts/generation/v1/fixtures/sel4-visibility.zti`.
- Expected: the gate fails — the broker is reporting each route the other's policy.
- Observed: the gate passed unchanged, on both pre- and post-migration code.
- Evidence: `check_records` compared `len(views)` to `EXPECTED_VIEW_RECORDS` and
  `len(set(traces))` to `EXPECTED_TRACE_RECORDS`, and nothing else.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `check_records` compares counts only; `VIEW_PATTERN` captures the hex but discards it | The gate constrains record *count*, not content |
| 2 | `fabric-publisher` checks route names, `contract_kind`, and `matched`; `fabric-subscriber` counts `routes != 1 \|\| qos != 1` and reads no field | No component covers the QoS payload either |
| 3 | `visibility_broker.rs:782` hex-dumps all 64 bytes of every record behind `[fabric-view] ` | The transcript already carries enough to decode gate-side |
| 4 | Booted `slime-sel4-visibility.elf` twice and diffed the twelve records | Byte-identical across boots: the view is stable enough to freeze |
| 5 | `qos_for` falls back to a route's first row when the component declares none | `fabric-publisher`'s diagnostics QoS is `fabric-publisher-b`'s declaration |
| 6 | Grepped every consumer of the QoS record's `route_name` | Checked *nowhere*: a record naming the wrong route was undetectable |

## Root cause

The gate asserted a structural property (twelve records) and treated it as if it
covered a semantic one (the right twelve records). Counting is invariant under
any permutation or corruption of record contents, so every field the broker
copies out of the graph sat outside the verified surface. The broker was
correct; the gate simply never looked.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/fabric-visibility/v1/gen_rust.zt` | Added a `pythonBindings` renderer via `wire.python`; `render` now returns `{ rust; python }`, and `valid` gained `py.recordsValid` | One schema authors both halves of the 64-byte layout |
| `contracts/fabric-visibility/v1/schema.zt` | `main` writes `scripts/lib/fabric_visibility_contract.py` beside the Rust binding | The gate's offsets are generated, not hand-copied |
| `scripts/generate/generate-fabric-visibility-bindings.py` | Dual-output emit and `--check`, on the `fabric-trace` generator's shape | `just contracts_check` catches either half going stale |
| `scripts/check/check-sel4-visibility-plane.py` | `render_view` decodes all twelve records; `check_view` compares every field against a frozen fixture; `--bless` records it | Route names and declared QoS are now verified |
| `Justfile` | Added `sel4_visibility_bless` and `fabric_visibility_gen` | The bless diff is the evidence a view change was intended |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A route's declared QoS reaches the wrong route's record | `just sel4_visibility_check` | `was:`/`now:` lines naming the differing records |
| A record names the wrong route | `just sel4_visibility_check` | Same; `route_name` is part of the frozen line |
| The 64-byte layout moves without regenerating | `just contracts_check` | `generated fabric_visibility_contract.py is stale` |
| The gate stops rejecting mutated transcripts | `just sel4_gate_control_check` | The `sel4_visibility_plane` control fails |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_visibility_bless` | Blessed 12 records; 26 markers, 7 chains | Direct |
| `just sel4_visibility_check` (unmutated) | Passed | Direct |
| **Exit condition**: swapped `telemetry`/`diagnostics` `historyDepth` (4↔2) | **Failed**, naming records 1 and 3 | Direct |
| Restore fixture, re-run `just sel4_visibility_check` | Passed | Direct |
| Two independent boots of `slime-sel4-visibility.elf` | Twelve records byte-identical | Direct |
| Appended a line to the generated Python binding | `--check` reported it stale | Direct |
| `just contracts_check` | Passed; fabric-visibility bindings current | Direct |
| `just sel4_gate_control_check` | 32 gates reject 1231 mutated transcripts | Direct |
| `just ruff` | Passed | Direct |

## Decisions

- **Decision:** the expectation is *frozen* in a fixture, not re-derived from
  the generation manifest.
- **Rationale:** B72's own exit condition mutates
  `contracts/generation/v1/fixtures/sel4-visibility.zti`. A gate parsing its
  expectation out of that fixture moves both sides of the comparison together
  and stays green — the self-confirming shape of the `FABRIC_INTERPOSITIONS`
  check deleted under B70. Freezing follows `sel4_boot_layout_bless`: the bless
  diff is the evidence a change was intended.
- **Rejected alternative:** asserting in the components against source literals.
  `fabric-publisher` declares no row on `diagnostics` — `qos_for`'s fallback
  hands it `fabric-publisher-b`'s declaration — so covering the second route
  means writing another component's manifest-derived QoS into a component
  source, the class of constant B70 has been removing.
- **Decision:** the fixture is rendered field-by-field as text, not as raw hex.
- **Rationale:** a policy change should read as a policy diff. The observed
  failure printed `history=4` against `history=2` rather than two 128-character
  hex strings.

## Open risks and follow-ups

- [ ] The exit condition's *full* QoS swap is rejected earlier, at admission
      (`UnsatisfiableFabricGraph`), because `offer_satisfies` constrains
      reliability, durability, liveliness, and the three deadlines. The
      admission-neutral `historyDepth` swap was used to isolate this gate: it is
      the strictly harder case, since `historyDepth` is not part of
      `offer_satisfies` and so reaches the view unmediated.
- [ ] B73 is the same shape on the matrix plane, but `matrix_broker.rs`'s
      `answer_view` emits no QoS record and no hex dump at all, so this
      approach does not transfer until the broker emits comparable evidence.

## Artifacts and provenance

- Frozen fixture: `contracts/fabric-visibility/v1/fixtures/sel4-visibility.view`
- Generated offsets: `scripts/lib/fabric_visibility_contract.py`
- Related roadmap item: [B72](../../roadmap/00-backlog.md)
