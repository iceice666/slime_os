# The seL4 boot-layout gate runs as shards

| Field | Value |
|---|---|
| Date | 2026-09-15 |
| Kind | Change |
| Status | Verified |
| Scope | `scripts/check/check-sel4-boot-layout.py`, `scripts/check/check-sel4-gate-controls.py`, `just/product.just`, `CLAUDE.md` |
| Work items | none |
| Gates | `just sel4_boot_layout_check`, `just sel4_gate_control_check` |
| Trigger | The gate is the slowest in the tree: one checker builds and boots all 33 seL4 planes in sequence, 1474 s on a quiet machine and 1694 s under load on 2026-09-15 |
| Baseline | `sel4_boot_layout_check` checks every plane in one process; `generation_v5_check` already takes `shard_index shard_count` |

## Summary

`check-sel4-boot-layout.py` takes `--shard-index I --shard-count N`, and both boot-layout recipes pass them through (`just sel4_boot_layout_check 1 4`). A shard checks every Nth plane by sorted name starting at the Ith, the same selection `generation_v5_check` uses, and refuses a shard that would check nothing. Each plane builds its own closure directory, so shards run at the same time without sharing an output. With the closures just re-rendered, four shards rebuilt and booted all 33 planes in about 14 minutes of wall time (716–861 s each, 9 + 8 + 8 + 8 planes), on a machine shared with another verification run. The single process had taken 25–28 minutes for the same work. Without shard arguments the gate behaves and reports exactly as before.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `scripts/check/check-sel4-boot-layout.py` | `select_planes(planes, index, count)` and `ShardError`; `--shard-index` / `--shard-count` (default `0` / `1`); a sharded run names its shard in the pass, fail, and bless lines | The shards of one count check every plane exactly once, and an impossible shard fails instead of passing vacuously |
| `just/product.just` | `sel4_boot_layout_check` and `sel4_boot_layout_bless` take `shard_index="0" shard_count="1"` | Existing invocations, including the `boot_layout_check` alias, are unchanged |
| `scripts/check/check-sel4-gate-controls.py` | The boot-layout controls also partition the declared planes at every count from 1 to 33 and require exact coverage, and require refusals for index −1, index = count, counts 0, −1, and 34, and an empty plane set | A selection that dropped or repeated a plane, or accepted an impossible shard, fails without a boot |
| `CLAUDE.md` | The command list names the shard parameters | Documentation matches the recipe |
| `contracts/system-image-closure/v2/closures/`, `contracts/system-test-run/v1/runs/` | Re-rendered: closures pin the Justfile recipes, which changed | Closure and run-record identities match the tree |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A shard count drops or repeats a plane | `just sel4_gate_control_check` | `boot-layout shards of N do not cover every plane exactly once` |
| An impossible shard passes having checked nothing | `just sel4_gate_control_check` | `boot-layout gate accepted the impossible shard I/N` |
| Any plane's layout drifts | `just sel4_boot_layout_check`, whole or in shards | `<plane>: layout differs from …` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_boot_layout_check 0 4` … `3 4`, run concurrently | Passed after re-rendering closures: `shard 0/4: 9 plane layouts match`, `shard 1/4: 8`, `shard 2/4: 8`, `shard 3/4: 8`; 33 distinct planes, 716–861 s each, about 14 minutes of wall time | Direct |
| `just sel4_gate_control_check` | Passed: 45 gates reject 1924 mutated transcripts and layouts (was 1918: the six shard refusals), plus the partition cases at every count | Direct |
| `just system_image_closure_check`, `just system_image_builder_check`, `just system_test_run_check`, `just system_image_closure_aggregate_check` | Passed after `python3 scripts/generate/generate-system-image-closures.py` and `just system_test_run_bless` | Direct |
| First shard attempt, before re-rendering | Every shard refused with `closure does not resolve: releaseInputs[just-recipes].artifact: identity mismatch for just`: the recipe edit changed a closure input, as designed | Direct |
| `just ruff`, `just typos`, `just devlog_check`, `just tasks_check` | Passed | Direct |

## Decisions

- Decision: shards select by sorted plane name with the slice `[index::count]`, not by contiguous ranges or build-time weight.
- Rationale: it is the selection `generation_v5_check` already uses and its controls already exercise, so both sharded gates share one rule; the slice balances counts to within one plane and does not change when a plane is appended in the middle of `PLANES`.
- Rejected alternative: balancing by recorded build time, which would make shard membership depend on evidence outside the tree.
- Decision: the selection stays in the boot-layout checker rather than moving to a shared module.
- Rationale: two checkers with a fifteen-line selection each is the first repetition, not yet a pattern; the gate controls pin the boot-layout copy's behaviour directly.

## Open risks and follow-ups

- [ ] CI does not run the boot-layout gate. A sharded job matrix, as `generation_v5_check` has, would bring it into CI at a quarter of its runtime.
- [ ] The gate's time is dominated by building closure images; booting all 33 planes from reused images took 28 s in the private clone on 2026-09-15. Shards help when closures change, which is when the gate matters most.

## Artifacts and provenance

- Run logs are local runner output, not repository evidence; the table above records the observed lines.
