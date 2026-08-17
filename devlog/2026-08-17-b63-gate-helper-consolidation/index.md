# B63 — 82 copies of three pure functions, and the flake that surfaced while verifying them

| Field | Value |
|---|---|
| Date | 2026-08-17 |
| Kind | Change |
| Status | Verified |
| Scope | `scripts/lib/harness.py`, `scripts/lib/sel4_gate_markers.py`, 34 `scripts/check/check-sel4-*.py` gates |
| Roadmap | B63, B55 |
| Gates | `just sel4_gate_control_check`, `just sel4_call_check`, `just sel4_boot_layout_check` |
| Trigger | The structural audit measured 30 `boot()` definitions in 23 distinct bodies and `harness.run_qemu` with zero users |
| Baseline | `scripts/lib/harness.py` existed for exactly this and was imported by almost no gate |

## Summary

39 seL4 gates each carried their own pinned-QEMU-profile readers and artifact
hasher — 82 local definitions of three pure functions, while
`scripts/lib/harness.py` sat unused. They are now consolidated: 0 local
definitions remain, 822 deletions against 387 insertions. Verifying it surfaced a
pre-existing flake in `sel4_call_check` (an independent task's completion pinned
inside a causal chain), which was fixed the way B55 fixed the boot plane's five
racy markers rather than by retrying until green. `load_pins`, `boot`, and the
blessable-marker-fixture half are deliberately not done, and the resolved entry
says why.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `harness.py` | `load_qemu_profile`, `profile_text`, `profile_integer`, `sha256_file`, each taking the caller's `fail` | One implementation of the pinned-profile contract |
| 34 gates | Local definitions deleted, call sites thread `fail`, imports added | A gate cannot accept a machine profile the others reject |
| `check-sel4-call-plane.py` | `\[fabric-call-time\] bounded time completed` moved from a causal chain to a new `EXPECTED_UNORDERED` membership check | The gate asserts what is causal, not one scheduling interleaving |
| `sel4_gate_markers.py` | `chains_from_gate` folds `EXPECTED_UNORDERED` into its chains | Moving a racy marker does not read as lost coverage |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A gate diverges on what the pinned profile must contain | Structural: one implementation, no second copy to drift | Would require editing `harness.py` |
| A gate loses marker coverage while being refactored | `just sel4_gate_control_check`'s per-gate pinned count | `declares N required markers, expected M` |
| A racy marker is reintroduced into a causal chain | The flake reproduces as `marker out of order` | Named failure with the pattern |
| A migrated gate stops booting | Each gate's own marker chain, all 33 run individually | Its own named failure |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Helper census | 0 local `profile_text`/`profile_integer`/`sha256_file` across 39 gates, from 82; 37 import from `harness` | Direct |
| Diff size | 35 files, 387 insertions, 822 deletions | Direct |
| One gate migrated and booted before fanning out | `just sel4_boot_layout_check` passed — 25 plane layouts | Direct |
| All 33 migrated gates, run individually | pass | Direct |
| `just sel4_call_check` × 3 after the flake fix | pass — 47 markers across 10 causal chains plus 1 order-independent, all three runs | Direct |
| `just sel4_gate_control_check` | pass — 32 gates reject 1227 mutated transcripts and layouts | Direct |
| Flake was pre-existing | Stashed every change; `sel4_call_check` passed at HEAD, and the failing run's diff contains only semantically identical call-site rewrites | Direct |
| `just ruff` | pass | Direct |

## Decisions

- Decision: pass each gate's `fail` into the shared helpers rather than exporting
  one. Rationale: a gate raises `SystemExit` with its own prefix, and in a suite of
  ~35 gates that prefix is how a failure is attributable. A shared `fail` would
  make every refusal read `harness: …`.

- Decision: migrate one gate, boot it, *then* fan out.
  Rationale: my first attempt did all 33 at once with a regex that mishandled
  multi-line import blocks and left 19 gates with undefined names. Reverted with
  `git checkout scripts/check/` and redone as a single atomic per-file function that
  drops the definition, threads `fail`, and adds the import — with an `ast.parse`
  check per file that auto-reverts on syntax error. Two gates (traffic, fault) hit
  that guard and were migrated by hand.

- Decision: fix the `sel4_call_check` flake rather than accept a passing retry.
  Rationale: it passed on rerun, and the temptation is to move on. But a gate that
  passes 4 times in 5 is a gate that will fail in someone else's session with no
  context. `fabric-call-time` is an independent task; nothing orders its completion
  against the broker's reclamation, so the chain was asserting a scheduling accident.
  Rejected alternative: adding a retry — that hides a wrong assertion.

- Decision: teach `chains_from_gate` about `EXPECTED_UNORDERED` rather than lowering
  the meta-gate's pinned count. Rationale: the count exists so lost coverage is
  visible, and the coverage was not lost — the marker is still required, just not
  ordered. Lowering the pin would have recorded a real regression as intentional.

- Decision: leave `load_pins` (33 copies) and `boot` (34 copies).
  Rationale: `load_pins` reads different sections and validates extra keys per gate
  family; `boot` differs in terminal marker, settling period, timeout, and disk
  arguments — the audit measured 23 distinct bodies across 30 definitions.
  Consolidating means a launcher with ~6 knobs, trading duplication for a
  configuration surface. The three that landed were pure functions with one correct
  implementation; these are not.

- Decision: do **not** attempt the blessable-marker-fixture half.
  Rationale: it needs a fixture format for regex chains, a blessing path that cannot
  bless a failing transcript into a fixture, and migration of ~35 gates. Bundling a
  design change with a mechanical one would make neither reviewable. Stated as
  deferred in the backlog rather than quietly dropped.

## Open risks and follow-ups

- [ ] The blessable marker fixtures the audit proposed remain undone; markers are
  still Python literals inside 300–1000-line gate files. `sel4_boot_layout_bless`
  remains the only blessing mechanism.
- [ ] `check-sel4-gate-controls.py` still hand-pins a marker count per gate. This
  change made that pin *work correctly* through a refactor, but it is still O(gates)
  hand maintenance — `marker_count(chains_from_gate(gate))` could derive it.
- [ ] **[INFERENCE]** The `sel4_call_check` flake is attributed to
  `fabric-call-time` being unsynchronised against the broker's reclamation, read
  from the plane's structure and the marker's position. The specific interleaving
  that produced the failure was not captured — the failing transcript was
  overwritten by the passing rerun before it was saved.
- [ ] Other gates may hold similarly racy cross-task markers inside causal chains.
  Only the one that failed was examined; no systematic audit of chain membership
  against task independence was done.

## Artifacts and provenance

- Focused report: none; the audit that opened B63 is
  [the structural audit entry](../2026-08-17-structural-audit/index.md), which
  measured the helper duplication and the 23 distinct `boot()` bodies.
- Raw transcript: none preserved. The flake's failure line is quoted above; the
  passing runs' summaries are in *Verification*.
- Serial/debugger/model output: quoted inline (`marker out of order:
  \[fabric-call-time\] bounded time completed`, and the meta-gate's
  `declares 46 required markers, expected 47`).
- Related roadmap item: [`roadmap/00-backlog.md`](../../roadmap/00-backlog.md) —
  B63 in the resolved log with both deferred halves stated; B65 open.
  [B55](../2026-08-15-b55-full-graph-boot-restoration/index.md) established the
  order-independent-membership treatment this reuses.
