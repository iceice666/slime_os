# Blessing the layouts found two controls that were not controlling

| Field | Value |
|---|---|
| Date | 2026-08-10 |
| Kind | Defect |
| Status | Verified |
| Scope | `contracts/boot-layout/v1/fixtures/`, `scripts/build/boot_layout.py`, `scripts/check/check-sel4-gate-controls.py` |
| Roadmap | none |
| Gates | `just sel4_boot_layout_check`, `just contracts_check`, `just sel4_gate_control_check` |
| Trigger | `sel4_boot_layout_check` was red with 204 differences across 24 planes. |
| Baseline | Fixtures frozen at `1eee295`, before the v5 wire-format cutover. |

## Summary

Every seL4 gate in the repository is green. Regenerating the frozen boot
layouts was the obvious part; the two defects it exposed were not. A second
checker cross-references the same fixtures against hand-maintained tables and
sixteen of them disagreed, and updating three stale marker-count pins let
`sel4_gate_control_check` reach two controls that had silently stopped
controlling.

## Observable symptom

- `just sel4_boot_layout_check`: 204 `was:`/`now:` differences.
- `just contracts_check` after blessing: `sel4-boot: frozen layout has 23 rows,
  resolver expects 21`.
- `just sel4_gate_control_check` after fixing the pins: `sel4_boot_plane:
  rejected a transcript built from its own REQUIRED_MARKERS`.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | Fixtures frozen before the v5 cutover; every plane's behavioural gate green | The observed layouts are the ones components run against |
| 2 | Blessing broke `contracts_check` | A second checker validates the same fixtures against tables in `boot_layout.py` |
| 3 | `sel4-boot` short by two rows | Init's own copies of the factories it hands the fabric — init is the *source* of those grants |
| 4 | Four fabric planes' tables put factories at 0 and 1 | Pre-v5 slot assignment |
| 5 | Pins fixed; boot-plane synthesis rejected | `TERMINAL_MARKER` is asserted but not in `REQUIRED_MARKERS`, and its backreference could not be instantiated |
| 6 | Layout mutation `[layout] 3 endpoint` | The channel layout reaches slot 1, so the mutation replaced nothing |

## Root cause

Three independent ones.

**The tables are hand-maintained.** `boot_layout.py` carries a literal row list
per plane, cross-checked against the fixture. Both drifted from the generation,
in different directions, so neither caught the other.

**The synthesizer could not express its own gate's assertion.**
`TERMINAL_MARKER` uses `required=(\d+) live=\1 idle=\1` — the backreference
*is* the claim. `literal_for` had no backreference handling, so the pattern
either produced a literal `\1` or was never reached.

**A mutation was pinned to a slot number.** `[layout] 3 endpoint` no longer
occurs in the channel plane's layout, so the control asserted that an
unmutated layout is accepted. The mutation directly above it carries a comment
warning of exactly this.

## Changes

- All 24 layout fixtures regenerated; all 16 resolver tables rebuilt from them.
- `literal_for` replays a captured group's instantiated text for
  backreferences.
- `boot_plane_transcript` appends the terminal marker, built from the gate's
  own pattern.
- The malformed-row mutation drops the slot number from whichever row is
  first.
- Three marker-count pins: supervision 11→12, stream 56→55, dango 14→13, boot
  47→46.

## Regression guards

- The fixture cross-check still catches a corrupted layout, verified by
  swapping two rows in `sel4-call.layout` and observing the refusal.
- `sel4_gate_control_check` reports 27 gates rejecting 1,100 mutations.

## Verification

| Check | Result |
|---|---|
| `just sel4_boot_layout_check` | pass — 24 plane layouts match |
| `just contracts_check` | pass |
| `just generation_check` | pass |
| `just sel4_gate_control_check` | pass — 27 gates, 1,100 mutations |
| All 30 seL4 plane gates | pass |
| `cargo test -p slime-root --lib` | 145 passed |
| `just test_host`, `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just machete` | clean |

## Decisions

**Blessed rather than reverted.** Every difference is a slot that moved because
a declaration moved, and each plane's behavioural gate passes against the new
layout — the call plane reports 50 markers across 10 causal chains, crossing
mints more channels than `MAX_CHANNELS` holds at once. A layout no component
could use would have failed those first. Cross-checked mechanically as well:
for every plane, the blessed slots are exactly what its generation declares for
init.

**The pins were adjusted, not deleted.** Each move traces to a specific commit
that justifies it. Three are from this run — one added coverage, two removed
assertions that no component could satisfy — and one predates it.

## Open risks and follow-ups

- `boot_layout.py`'s tables are still hand-maintained and still duplicate the
  fixtures. They agree today because they were just regenerated from them;
  nothing stops them diverging again. Deriving one from the other would remove
  the class.
- The marker-count pins have the same shape: a number that must be updated by
  hand whenever a gate legitimately changes. That is the intended friction, but
  four were stale at once, which suggests the friction is being paid in
  batches rather than per change.

## Artifacts and provenance

- Commits: `6c6fa8d` (bless), `7a13a72` (resolver tables), `54fbfe2` (controls).
