# Retiring seventeen dead profile symbols, and the limits check that could not fail

| Field | Value |
|---|---|
| Date | 2026-08-19 |
| Kind | Change |
| Status | Verified |
| Scope | `scripts/build/build-generation.py`, `scripts/check/check-data-fabric-profile.py`, `components/bins/src/default_fabric_profile.rs` |
| Roadmap | B70, CP2 |
| Gates | `just data_fabric_profile_check`, `just generation_check`, `just contracts_check`, `just sel4_boot_layout_check`, `just sel4_qos_check`, `just sel4_visibility_check`, `just sel4_call_check`, `just sel4_operation_check`, `just sel4_stream_check`, `just sel4_traffic_check`, `just sel4_matrix_check`, `just sel4_fault_check`, `just sel4_fabric_aggregate_check`, `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just test_host`, `just machete` |
| Trigger | Continuation of `dc9e9e4`; the compiled-in profile still rendered symbols no component reads |
| Baseline | `dc9e9e4` — participant assertion repaired, generation `65f60c11…`, all gates green |

## Summary

`render_fabric_profile_rust` emitted seventeen symbols that no component, root
module, or script reads: eight `FABRIC_MAX_*` limit constants, `FABRIC_ROUTES`,
`FABRIC_VISIBILITY`, `FABRIC_WORKERS`, `FABRIC_PROFILE_NAME`,
`FABRIC_DEADLINE_ABSENT`, and the `FabricWorkerRow` / `WORKER_ABSENT` /
`fabric_worker_wait_sources` / `konst_str_eq` cluster. All seventeen are gone;
the compiled-in profile drops from 184 lines to 99, and the generation hash is
byte-identical, confirming they were compile-time dead weight rather than
generation content.

Removing them was blocked by two entanglements in the gate that guards the
profile, and the second was itself a defect. The participant assertion committed
in `dc9e9e4` iterated `("FABRIC_QOS", "FABRIC_VISIBILITY")` and failed closed on
a missing table, so deleting `FABRIC_VISIBILITY` would have broken it. The
adjacent limits loop searched the whole rendered file for `f" = {value};"`, which
cannot distinguish one limit from another whenever two limits share a value — and
seven of the nineteen do. That loop was near-vacuous before this change and would
have failed spuriously after it. Both were rewritten and both rewrites were
falsification-tested.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `build-generation.py` template | Deleted seventeen dead symbols from the emitted profile | The compiled-in profile states only what a component reads |
| `build-generation.py` builders | Deleted the now-orphaned `visibility_rows`, `worker_rows`, and template-local `route_rows` accumulators | No write-only locals survive their consumer |
| `build-generation.py` docstring | `deadline()` referred to the deleted `FABRIC_DEADLINE_ABSENT`; now names `u64::MAX` | Documentation names a symbol that exists |
| `check-data-fabric-profile.py` | Participant assertion narrowed to `FABRIC_QOS` alone | The check no longer pins a table the profile is free to retire |
| `check-data-fabric-profile.py` | Limits assertion rewritten from value-substring search to a name-anchored bidirectional check | A drifted or renamed limit constant fails; a deliberate emission change does not |
| `default_fabric_profile.rs` | Regenerated through `render_fabric_profile_rust` (184 → 99 lines) | The checked-in `@generated` file is the generator's own output |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A limit constant drifts to another limit's value | `just data_fabric_profile_check` | `rendered Rust FABRIC_MAX_QUEUE_DEPTH is 9, but the canonical profile declares queueDepth = 8` |
| A limit constant is renamed | `just data_fabric_profile_check` | `rendered Rust declares FABRIC_MAX_SERVER, which is not a limit of the canonical profile` |
| The profile stops rendering limits entirely | `just data_fabric_profile_check` | `rendered Rust declared no limit constants at all` |
| A participant is dropped from `FABRIC_QOS` | `just data_fabric_profile_check` | `rendered Rust FABRIC_QOS has 14 rows for 15 declared participants` |
| A participant is duplicated over another, preserving cardinality | `just data_fabric_profile_check` | `rendered Rust FABRIC_QOS diverges from the canonical profile participants` |
| A deleted symbol turns out to have a live reader | `just lint_all`, plane gates | Compile failure in `components/bins` |

## Verification

Every gate below was run directly at the post-change tree.

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just data_fabric_profile_check` (baseline, before deletion) | ok | Direct |
| Perturbation: drop `participants[0]` from `qos_rows` | failed as designed — 14 rows for 15 participants | Direct |
| Perturbation: duplicate `participants[1]` over `[0]` | failed as designed — diverges from canonical participants | Direct |
| Perturbation: render `queueDepth` from `limits['ingressSources']` | failed as designed — is 9, declares 8 | Direct |
| Perturbation: rename `FABRIC_MAX_SERVERS` → `FABRIC_MAX_SERVER` | failed as designed — not a limit of the canonical profile | Direct |
| `just data_fabric_profile_check` (after deletion) | ok | Direct |
| `just generation_check` | `65f60c1163a7a7f9bcf213e88c8ab1fe3dea91072d7cf861a48738dab4d01423`, unchanged from baseline | Direct |
| `just contracts_check` | 26 declared operations documented, bindings current | Direct |
| `just sel4_boot_layout_check` | 25 plane layouts match frozen fixtures | Direct |
| `just sel4_qos_check` | 14 markers, 9 chains, six participants clean | Direct |
| `just sel4_visibility_check` | 26 markers, 7 chains, 12 view records | Direct |
| `just sel4_call_check` | 47 markers, 10 chains plus 1 order-independent | Direct |
| `just sel4_operation_check` | 53 markers, 15 chains, six tasks clean | Direct |
| `just sel4_stream_check` | 57 frozen markers plus 4 declared seL4-only | Direct |
| `just sel4_traffic_check` | 19 participants across three planes concurrently | Direct |
| `just sel4_matrix_check` | incompatible QoS pair fails closed at admission | Direct |
| `just sel4_fault_check` | 10 markers, 8 isolation markers intact | Direct |
| `just test_host`, `just machete`, `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos` | clean | Direct |

The perturbation rows are the load-bearing ones. Each was injected into the
generator, the profile regenerated, and the gate observed failing with the quoted
message; the generator was restored between every run. Two of the four — the
limit-drift and rename arms — could not fail at all before this change.

`just sel4_visibility_check` passing after `FABRIC_VISIBILITY` was deleted is the
direct evidence that the visibility broker reads its answer from the graph rather
than from the retired table.

## Decisions

- Decision: narrow the participant assertion to `FABRIC_QOS` rather than keep
  `FABRIC_VISIBILITY` alive to satisfy it.
- Rationale: `FABRIC_QOS` and `FABRIC_VISIBILITY` were rendered from the same
  participant list, so checking both proved nothing the single check does not.
  Keeping a table alive only so a gate can assert about it inverts the
  relationship — the gate exists to guard what components read, not to create
  readers.
- Rejected alternative: keep `FABRIC_VISIBILITY` as the profile's canonical
  participant statement. Rejected because no reader exists, and the plane gate
  confirms the broker resolves visibility from the graph.

- Decision: check limits by name with a bidirectional constraint, not by value.
- Rationale: `routes`, `queueDepth`, `historyDepth`, and `eventDepth` all declare
  8; `buffers`, `mappings`, and `loans` all declare 14. A value-substring search
  was satisfied for each of those by any one surviving sibling, so it could not
  fail for most limits. Requiring that every rendered `FABRIC_MAX_*` name a
  declared limit and carry its exact value catches drift and renames, while
  tolerating a profile that deliberately renders fewer constants than the graph
  declares.
- Rejected alternative: require every declared limit to be rendered. Rejected
  because that is exactly what this change stops being true, and re-asserting it
  would have forced the eight dead constants to stay.

- Decision: delete `fabric_worker_wait_sources` and its `WORKER_ABSENT` /
  `konst_str_eq` support despite a doc comment describing it as a compile-time
  drift guard.
- Rationale: the comment claims brokers bind notification arrays to it via
  `const _: () = assert!(..)`. No such assertion exists anywhere in the tree — the
  documented guard was never wired up. Deleting it removes a description of
  protection, not protection.
- Rejected alternative: wire the guard up instead of deleting it. Deferred rather
  than rejected outright; see follow-ups.

## Open risks and follow-ups

- [ ] The wake-source drift guard `fabric_worker_wait_sources` documented was
      never implemented. If per-worker park-set overflow is a real risk, it wants
      a guard that exists — reachable from the graph, not from a retired
      build-time table. Related: the 3-vs-8 `FABRIC_WORKERS` stream-peak
      discrepancy noted at
      `devlog/2026-08-19-interposition-hop-identity/index.md:127`.
- [ ] B70's exit clause is still unmet: `fabric-service.rs:107` and
      `fabric-publisher.rs:50` both `include!` the build-time profile, and both
      still read `GENERATION_BOOT_ACTION` from it (31 uses). That authority
      question is the remaining blocker.
- [ ] Doc prose in `visibility_broker.rs`, `fabric-service.rs`,
      `fabric-publisher.rs`, and `slime-root/src/ipc.rs` still names retired
      symbols as historical context; worth a consistency pass.
- [ ] B74 remains open — the aggregate gate's two unreproduced failures.

## Artifacts and provenance

- Focused report: this entry
- Raw transcript: none retained; every perturbation is reproducible from the
  Verification table by re-injecting the named edit into
  `scripts/build/build-generation.py`
- Serial/debugger/model output: gate summaries quoted inline above
- Related roadmap item: `roadmap/00-backlog.md` B70, B74
