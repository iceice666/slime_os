# Partition CI verification without dropping gates

| Field | Value |
|---|---|
| Date | 2026-09-13 |
| Kind | Change |
| Status | Monitoring |
| Scope | GitHub CI workflow, slime-env build caches, Just contract and Miri recipes, generation-v5 checker, derived closure and test-run identities |
| Work items | none |
| Gates | `just generation_v5_check`, `just contracts_source_check`, `just component_spec_validation_check`, `just bootstate_trace_check`, `just release_trust_runtime_check`, `just miri`, `just system_image_closure_aggregate_check`, `just system_test_run_check` |
| Trigger | Repeated CI feedback of approximately 30 minutes |
| Baseline | Every manifest must be built and its actual generation header checked; contract, rollback, release-trust, component-spec and Miri checks remain required |

## Summary

CI now partitions the full generation inventory across four jobs, executes contract sources once, and runs the two existing Miri package checks in separate workers. Stable historical check names aggregate the relevant workers, and a final CI gate requires every prerequisite to succeed. Local full-scope Just entrypoints remain intact. Local coverage checks and real generation builds passed; hosted cache effectiveness and end-to-end acceleration remain unmeasured.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Contract execution | Separate source, generation, release runtime and component-spec validation leaves; retain full local aggregates | CI can execute each independent gate once without weakening local entrypoints |
| Generation verification | Sorted stride partition, validated zero-based shard bounds, list mode, progress, embedded controls and bounded header read | Four shards build all 41 manifests exactly once; default invocation still checks all manifests |
| Miri | Independent boot-contracts and slime-proto recipes, matrix workers and original-name aggregate | Both original commands and feature selections remain required |
| CI aggregation | Preserve historical Miri, contracts and rollback check names; always-run exact-success gates | Failure, cancellation or skipped prerequisites cannot produce a successful aggregate |
| Build caches | Cache only Zutai, component and seL4 Cargo output trees with compatible, partitioned restore keys | Reuse compilation without reusing verification results, mutable disks or unverified closure images |
| Derived records | Regenerate closure records and their dependent test-run records with existing generators | Changed Just input identities still resolve through the normal closure admission path |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Missing or duplicated manifest shard | `python3 scripts/check/check-generation-v5.py --check-controls` in CI; real CLI partition comparison | Empty or invalid partitions rejected; union and uniqueness checks fail |
| Incorrect generated header | `just generation_v5_check` for every shard | Built magic or version mismatch fails the selected shard |
| Lost contract or runtime checks | Existing full local aggregates and exact-success historical CI aggregates | Any required leaf failure prevents aggregate success |
| Stale derived closure identities | Closure generator `--check`, `just system_image_closure_aggregate_check`, `just system_test_run_check` | Stale identity, unmapped closure or drifted test-run rejected |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `actionlint .github/workflows/ci.yml` with actionlint 1.7.12 | Passed | Direct |
| `python3 scripts/check/check-generation-v5.py --check-controls` | Passed | Direct |
| Real CLI full list and four shard lists | 41 manifests partitioned 11/10/10/10, no omissions or duplicates; invalid bounds refused | Direct |
| Aggregate shell steps with success, failure, skipped and cancelled prerequisite results | 70 cases passed; final aggregate explicitly covers all 16 prerequisite jobs | Direct |
| Just dry-runs of combined local aggregates and CI leaves | Original full gate bodies occur once; CI source/runtime leaves contain no full generation pass; Miri contains both original commands | Direct |
| Composite cache identity shell step against the current gitlinks | Passed; cache and restore prefixes share the exact compatibility boundary | Direct |
| `nix develop --command just generation_v5_check 0 4 generation_v5_check 1 4 generation_v5_check 2 4 generation_v5_check 3 4` | All 41 manifests built and encoded SLIMEG5 version 5 | Direct |
| `nix develop --command just contracts_source_check component_spec_validation_check` | Passed, including 315 boot-contract tests, 83 component records and 43 refused mutations | Direct |
| `nix develop --command just miri` | Passed: 340 boot-contracts tests and the complete slime-proto integration corpus, no failures | Direct |
| `nix develop --command just bootstate_trace_check release_trust_runtime_check` after regeneration | Passed: QEMU rollback and seven durable transitions, trace negatives, signed release and trust-root rotation/refusal cases | Direct |
| `nix develop --command python3 scripts/generate/generate-system-image-closures.py --check` | 59 closure records current | Direct |
| `just system_image_closure_aggregate_check` through the Nix shell | All 53 positive closures have owning gates; six named drift controls refused | Direct |
| `nix develop --command just system_test_run_bless system_test_run_check` | 50 derived records regenerated and checked; five named controls refused | Direct |
| `just sel4_gate_control_check` | Passed | Direct |
| `just ruff`, `just fmt_check_all typos` | Passed | Direct |

The initial rollback smoke run correctly refused the stale `just-recipes` closure identity. The existing closure generator updated 59 positive/negative records; the dependent system-test-run gate then exposed stale closure references, which its existing bless command regenerated. These were generated-data updates, not admission bypasses.

Independent review identified the closure regeneration requirement and a missing exact-length check on generation headers. The final checker rejects every truncated header length from zero through eleven bytes; its controls exercise real file reads. Controls run only in their dedicated CI step, not repeatedly on the expensive build path. The cache key no longer names a nonexistent Zutai toolchain file and includes nested component linker scripts conservatively.

## Decisions

- Keep full verification on each CI run rather than moving checks to a nightly schedule.
- Keep the established public aggregate job names so existing consumers still observe full-scope success.
- Cache Cargo intermediates, not successful checks, closure images, installed prefixes or QEMU-mutated disks.
- Run local generation shards sequentially over the complete inventory; hosted jobs have isolated filesystems and run in parallel. Local elapsed times are not an estimate of hosted performance.

## Open risks and follow-ups

- Hosted cold/warm cache timings, cache eviction and available arm64 concurrency require the first CI run on this branch; no hosted speedup is claimed.
- Local Miri execution was dominated by boot-contracts (1546.79 seconds); slime-proto cases were much shorter. Package-level splitting preserves coverage but does not promise a balanced or halved Miri critical path.
- The unchanged SDK publication workflow reports two existing shellcheck word-splitting diagnostics and an unconfigured custom `m3air` runner label under a broad actionlint invocation. The modified CI workflow passes independently; SDK publication behavior was not changed.

## Artifacts and provenance

- Baseline Actions run [34731420809](https://github.com/iceice666/slime_os/actions/runs/34731420809): rollback job 30:35, contracts job 27:39, Miri job 15:04. The generation-v5 pass inside rollback alone took 14:20. Inherited baseline evidence; not a run of this change.
- Earlier successful baseline run [34710217712](https://github.com/iceice666/slime_os/actions/runs/34710217712): rollback job 31:18, contracts job 26:47, Miri job 18:08. Inherited baseline evidence.
- Verification above was executed locally in the repository's pinned Nix environment unless the command identifies a standalone static tool. No branch was pushed and no hosted run was dispatched.
