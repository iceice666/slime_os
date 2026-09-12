# Comment ownership across four files, and closure decoding through the shared evaluator

| Field | Value |
|---|---|
| Date | 2026-09-12 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/system-image-closure/v2/schema.zt` header, `scripts/check/check-system-spec.py` exemption comment, `sel4/config/qemu-arm-virt.cmake` `KernelMaxNumNodes` comment, `just/quality.just` `test_sel4_root` comment, `scripts/lib/system_image_closure.py` Zutai invocation |
| Work items | 01a07a2d-003d-77fc-9df8-dda85ed9a083 |
| Gates | `just system_image_closure_check`, `just system_spec_check`, `just system_test_run_check`, `just contracts_check`, `just system_image_scenario_check`, `just ruff`, `just typos`, `just devlog_check` |
| Trigger | PR #25 review of `b319720d` reported four implementation comments carrying history or campaign inventory, and one closure decoder bypassing the shared cached evaluator |
| Baseline | Branch head `b319720d`, where the v2 closure schema's opening line still says "version 1", three comments narrate prior decisions or verification campaigns, and `system_image_closure._run_zutai` launches `zutai-cli` directly |

## Summary

Five findings from one review round. Four are comment-ownership defects of the
same shape: an implementation comment stating its current invariant and then
going on to record how the repository arrived there — a superseded contract
version in the v2 schema header, a former global exemption and the regression
it caused in the system-spec checker, upstream SMP support history and a future
audit plan in the AArch64 kernel config, and a milestone-by-milestone test
inventory in the `test_sel4_root` recipe. Each is now the invariant alone. The
fifth is a performance defect with a correctness-shaped cause: closure and
test-run decoding ran `zutai-cli` through its own `subprocess` calls rather than
`zutai_cli.evaluate`, so every record in the closure and test-run gates redid
two full Zutai evaluations per run, bypassing the content-addressed cache and
`SLIME_ZUTAI_CACHE` entirely.

## Changes

| Area | Change | Preserved invariant |
|---|---|---|
| Closure schema header | The opening line names contract version 2 | A schema's stated version matches the `formatVersion` it exports and the directory it lives in |
| System-spec exemption comment | States that the exemption is per system and that `check_post_baseline` compares the named sections independently; the former flat-tuple behavior and the regression it permitted are dropped | Notification sections are excused from one system's frozen baseline, never from comparison |
| Kernel config comment | Three lines beside `KernelMaxNumNodes`: the assurance departure, the root's single-core table assumption, the `KernelIsMCS` affinity coupling, and the direction document carrying the terms | Raising the core count is an assurance decision with a recorded cost, not a config edit |
| `test_sel4_root` recipe comment | States why the count is pinned and that it moves in the same change that adds a test | A dropped or unregistered `#[test]` fails the gate rather than passing as a smaller run |
| Closure decoding | `_run_zutai` calls `zutai_cli.evaluate` for both the `run` and `json` passes; `os` and `subprocess` imports dropped with their last use | A record is decoded by the same binary, stdlib, and contract closure the cache key names, so a cached result and a fresh run are the same result |

The comment edits move `just/` and `sel4/config/` bytes, which the closures
pin, so all 59 closures were regenerated and the 48 test-run records carrying
their identities re-blessed.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Closure decoding diverges from the cached evaluator's result | `just system_image_closure_check` | Identity mismatch or an exact-shape field failure on a record that resolved before |
| A test-run record stops naming its resolved closure | `just system_test_run_check` | `test run does not name the resolved image closure` |
| A system-spec baseline divergence hides inside the scoped exemption | `just system_spec_check` | The independent `check_post_baseline` comparison reports the differing notification field |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just system_image_closure_check` | Contracts, resolution, identity, isolation, and bytes verified; closure `b11e67d64022` built | Direct |
| `just system_spec_check` | Passed; every composition current | Direct |
| `just system_test_run_check` | 48 records correspond one-to-one with plane gates; 46 resolve a closure, 2 closure-exempt; 5 named controls refused | Direct |
| `just contracts_check` | Passed | Direct |
| `just system_image_scenario_check` | Passed | Direct |
| `just ruff` | Passed | Direct |
| `just typos` | Passed | Direct |
| Stale-closure refusal after the comment edits | `just system_image_closure_check` refused with `releaseInputs[just-recipes].artifact: identity mismatch for just` before regeneration, and with `test run does not name the resolved image closure` before the re-bless | Direct |

The closure gate's refusal before regeneration is the evidence that these
comment edits are inside the identity boundary: a comment in `just/quality.just`
is a closure input, and the gate said so by name.

## Decisions

- Decision: correct the four comments in place rather than relocating their prose to `devlog/`.
- Rationale: each narrative is already recorded where it belongs — the SMP assurance terms in `docs/directions/34-capacity-ceilings.md`, the system-spec exemption regression in the 2026-09-10 checker-coverage entry, the closure versioning in the 2026-09-12 v2 entry. Copying it again would duplicate a fact rather than move it.
- Rejected alternative: appending the removed text to this entry for completeness. That makes the devlog the second copy instead of the single one.

- Decision: route both the `run` and `json` passes through `evaluate` rather than caching only the schema check.
- Rationale: the two passes share a cache key input — the record's own bytes — and the JSON projection is the more expensive of the two. Caching one and not the other would halve a benefit that costs nothing extra to take whole.
- Rejected alternative: keeping the direct `subprocess` calls and documenting that this path is uncached. That leaves two ways to run the same binary in one repository, which is the condition that let the cache be bypassed silently.

## Open risks and follow-ups

- [ ] No new open risk. The two risks the 2026-09-12 closure-contract entry records — landed prose referencing `system-image-closure/v1`, and closure identities computed under the v1 domain — are unchanged by this entry.

## Artifacts and provenance

- Focused report: none; each change is local to its named file.
- Raw transcript: none retained; every gate above is reproducible from the listed command.
- Serial/debugger/model output: none. No plane boots for this change; the affected surfaces are contract text, checker comments, and host-side decoding.
- Related work item: [MEM-ARENAS](../../.tasks/items/01a07a2d-003d-77fc-9df8-dda85ed9a083.md)
- Preceding investigation: [Closure contract v2, descriptor conservation, and two derived report facts](../2026-09-12-closure-contract-v2-and-descriptor-conservation/index.md)
