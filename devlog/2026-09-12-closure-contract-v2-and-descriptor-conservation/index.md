# Closure contract v2, descriptor conservation, and two derived report facts

| Field | Value |
|---|---|
| Date | 2026-09-12 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/system-image-closure` v1→v2 migration and every path referencing it, `slime-root/src/graph_runtime/services.rs` baseline capacity marker, `scripts/check/check-sel4-reclamation-plane.py` descriptor assertions, `scripts/check/check-system-image-scenario.py` role-count summary, `scripts/check/check-sel4-private-memory-plane.py` comment ownership |
| Work items | 01a07a2d-003d-77fc-9df8-dda85ed9a083 |
| Gates | `just contracts_check`, `just system_image_closure_check`, `just system_image_scenario_check`, `just system_test_run_check`, `just sel4_reclamation_check`, `just private_memory_check`, `just sel4_gate_control_check`, `just sel4_root_boot_check`, `just test_sel4_root`, `just devlog_check`, `just ruff` |
| Trigger | PR #25 review of `af6c84d6` reported an unversioned breaking closure-schema change, an unread descriptor counter, a hardcoded role count contradicting its own contract, and prior-review history in a checker comment |
| Baseline | Branch head `af6c84d6`, where the closure schema declares `formatVersion = 1` without `target.sdkRelease`, the reclamation plane captures `allocation_descriptors_free` without reading it, and the scenario summary states a literal four-name root-role vocabulary against a six-entry contract |

## Summary

Four findings from one review round, none sharing a mechanism. The closure
contract had lost a required field at an unchanged `formatVersion`, so a
persisted record was undecodable in both directions while still claiming v1;
it is now `contracts/system-image-closure/v2` with `formatVersion = 2` and a
v2 identity domain, matching how every other versioned contract in this
repository handles a breaking change. The reclamation plane captured
`allocation_descriptors_free` in its terminal marker and never asserted on it,
so a release that stranded `AllocationRecord`s passed every existing check;
the root now reports descriptor pool *capacity* and the plane requires every
allocation descriptor back, which the workload's full teardown makes an exact
law rather than a threshold. The scenario gate's success line stated a
four-name root-role vocabulary against a six-entry contract. A private-memory
checker comment carried prior-review history.

## Changes

| Area | Change | Preserved invariant |
|---|---|---|
| Closure contract | Moved `contracts/system-image-closure/v1` to `v2`, set `formatVersion = 2`, and moved `identityDomain` to `slime-system-image-closure-v2:`; repointed every path, regenerated the Python bindings, all 59 closures, and 48 test-run records | A persisted record's declared version determines its shape; a record decodes exactly under the version it names |
| Identity domains | `buildResultIdentityDomain` and `negativeIdentityDomain` stay at v1 | A domain names the shape it hashes; `SystemImageBuildResult` and `NegativeBuildCase` did not change, and they move only through the closure identities they carry |
| Capacity marker | `SLIME_ROOT allocator baseline` now reports `allocation_descriptor_capacity` and `extent_descriptor_capacity` instead of post-staging free counts | A conservation law needs the pool's size, not a count already reduced by the graph the plane is about to tear down |
| Descriptor assertion | The reclamation plane requires the terminal `allocation_descriptors_free` to equal capacity exactly | Arena release clears every `AllocationRecord` it owns |
| Extent assertion | The same plane requires new extent records to stay strictly below `extent_reuses` | Extent records are deliberately retained, not returned, so retention is proved by reuse dominating new records |
| Scenario summary | Derived the root-role count from `len(CONTRACT.ROOT_ROLES)` | Captured verification output states what was actually checked |
| Checker comment | Replaced the B68 reference and former first-match description with the current invariant | Implementation comments state what must remain true |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A v1-shaped closure is accepted as v2, or vice versa | `just system_image_closure_check` | `unsupported image closure version`, or an exact-shape field mismatch on `target` |
| Arena release strands allocation descriptors | `just sel4_reclamation_check` | `N allocation descriptor(s) survived reclamation of every task` |
| Extent records stop being retained for reuse | same gate | `extent descriptors grew by N against M reuse(s)` |
| The admitted role set and the reported count diverge | `just system_image_scenario_check` | The printed vocabulary size disagrees with the contract |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just contracts_check` | Passed | Direct |
| `just system_image_closure_check` | Contracts, resolution, identity, isolation, and bytes verified; closure `e6e4a77e26f9` built | Direct |
| `just system_image_scenario_check` | 53 closures resolve; summary now reports the 6-name root-role vocabulary; 5 root-role and 5 scenario closures distinct | Direct |
| `just system_test_run_check` | 48 records correspond one-to-one with plane gates; 46 resolve a closure | Direct |
| `just sel4_reclamation_check` | Passed on AArch64 QEMU with every allocation descriptor returned (265,272 of 265,272) and 81 extent reuses against 3 new records | Direct |
| Mutation: `release_task_arena` skips clearing every seventh `AllocationRecord` | Refused — `21 allocation descriptor(s) survived reclamation of every task: 265251 free of 265272`; restored source passes | Direct |
| `just private_memory_check` | AArch64 and RV64: 23 markers across 7 causal chains each | Direct |
| `just sel4_gate_control_check` | 48 gates reject 1925 mutated transcripts and layouts | Direct |
| `just sel4_root_boot_check` | Ordered markers observed | Direct |
| `just test_sel4_root` | 235/235 across 19 modules | Direct |
| `just devlog_check`, `just ruff` | Passed | Direct |

## Decisions

- Decision: version the closure contract to v2 rather than restoring `target.sdkRelease` at v1.
- Rationale: the field was removed because the mixed-purpose release input it referenced was itself incorrect provenance (see the 2026-09-10 host-side closure gate entry). Restoring it would reinstate a record whose own producing commit could not reproduce it. Every other versioned contract here — `generation` v2–v5, `component` v2, `block` v2, `fabric-stream` v2 — bumps the directory and `formatVersion` together for a breaking change, so v2 is the existing convention, not a new one.
- Rejected alternative: keeping v1 and documenting the shape change. That leaves `formatVersion` asserting something false, which is exactly the property the field exists to carry.

- Decision: move `identityDomain` to v2 but leave `buildResultIdentityDomain` and `negativeIdentityDomain` at v1.
- Rationale: a domain separates hashes of *different shapes*. Only `SystemImageClosure` changed shape; a build result or negative case that hashed differently for the same bytes would assert a change that did not happen. Both already move whenever the closure identity they embed moves.
- Rejected alternative: bumping all three together for tidiness. That would re-identify records whose contract is unchanged.

- Decision: report descriptor pool capacity from the root instead of comparing against a pre-workload free count.
- Rationale: the baseline marker is emitted after the graph is staged, so it already excludes the live graph's descriptors. A mutation leaking 21 descriptors still left more free at teardown than at that baseline, and the comparison passed. The plane ends with no live task, so equality against capacity is both exact and the strongest available statement.
- Rejected alternative: freezing the expected count in the checker. That is a constant needing a re-bless whenever an unrelated ceiling moves, and it would not track the linked kernel's descriptor-table selection.

## Open risks and follow-ups

- [ ] 28 prose references to `contracts/system-image-closure/v1` remain in landed devlog entries, roadmap text, and work items. Those are frozen prose describing the contract as it was, and are correct as written. Only the one dead relative *link* in `devlog/2026-09-08-private-memory-capacity-and-retry-review/index.md` was repointed, under that entry's `## Corrections`.
- [ ] The closure identities every earlier entry recorded were computed under the v1 domain. They are historical facts about a contract version that no longer exists, not values the current corpus should reproduce.

## Artifacts and provenance

- Focused report: none; each change is local to its named file.
- Raw transcript: none retained; every gate above is reproducible from the listed command.
- Serial/debugger/model output: `just sel4_reclamation_check`, `just private_memory_check`, and `just sel4_root_boot_check` serial transcripts from the pinned QEMU profiles.
- Related work item: [MEM-ARENAS](../../.tasks/items/01a07a2d-003d-77fc-9df8-dda85ed9a083.md)
- Preceding investigation: [Pre-publication construction unwinds dropped the slots they reclaimed](../2026-09-12-construction-unwind-slot-accounting/index.md)
