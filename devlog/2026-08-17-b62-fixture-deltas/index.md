# B62 — the proposed fix was impossible, so the delta moved to the layer that already had one

| Field | Value |
|---|---|
| Date | 2026-08-17 |
| Kind | Change |
| Status | Verified |
| Scope | `scripts/build/build-sel4.py` (`VARIANT_GENERATION_DELTAS`), `scripts/build/build-generation.py` (two overrides), `scripts/lib/fabric_graph_limits.py`, `check-sel4-{saturation,fault,matrix}-plane.py`, 3 deleted `.zti` fixtures |
| Roadmap | B62, B55 |
| Gates | `just contracts_check`, `just generation_check`, `just sel4_saturation_check`, `just sel4_matrix_check`, `just sel4_fabric_aggregate_check` |
| Trigger | The structural audit measured nine fixture pairs over 85% identical, three at 99.9% |
| Baseline | 30 `sel4-*.zti` fixtures, 16978 lines; `diff sel4-traffic.zti sel4-fault.zti` was one hunk |

## Summary

Three pairs of ~1882-line fixtures differed by one or two fields. The audit
proposed adding base-plus-delta composition to `.zti`, which the format forbids by
design — `.zti` is immediate mode with no imports or evaluation, and a probe
confirmed `import` is a syntax error there. So the delta moved to `build-sel4.py`,
which already supplied per-variant build environment for exactly this reason. Three
fixtures deleted, 3728 lines removed, pairs over 90% identical down from 3 to 1.
The one survivor is 94.4% similar across 203 changed lines — two compositions, not
a variant — and was left alone.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `build-sel4.py` | `VARIANT_GENERATION_DELTAS` declares what distinguishes a variant sharing another's manifest; `traffic`/`fault`/`saturation` and `matrix`/`matrix-unsatisfiable` now share fixtures | A variant is a declared delta, not a copied file |
| `build-generation.py` | `SLIME_FABRIC_LIMIT_OVERRIDE` (one declared limit) and `SLIME_FABRIC_QOS_OVERRIDE` (one participant field, addressed `route:component:field`), both validated against what the manifest declares | An override cannot invent a field the schema does not know |
| `fabric_graph_limits.py` | `declared_limits` gained `overrides`, refusing an override naming an undeclared limit | The gate reads the same narrowed ceiling the image was built with |
| `check-sel4-saturation-plane.py` | Reads the override out of `build-sel4.py`'s declaration instead of the deleted fixture | One source for the ceiling under test |
| 3 `.zti` fixtures | `sel4-fault.zti`, `sel4-saturation.zti`, `sel4-matrix-unsatisfiable.zti` deleted | 3728 fewer lines that could go stale independently |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A variant's delta stops matching what its gate asserts | `just sel4_saturation_check` reads the delta from the build declaration | Peak does not equal the declared ceiling |
| An override names a limit or participant the manifest does not declare | Builder validation on both overrides | `names undeclared limit`, `not a unique participant of`, `names undeclared field` |
| Two variants collapse to one generation identity, making the determinism assertion vacuous | `just generation_check` builds each twice and compares bytes | Identity collision or byte mismatch |
| The matrix negative control stops being incompatible | `just sel4_matrix_check` unsatisfiable arm | The graph is admitted instead of refused |
| A shared-fixture plane's admitted counts drift | Each plane gate's own admission chain | `generation admitted number=… executables=…` mismatch |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `.zti` composition probe | `import` in a `.zti` file is a syntax parse error, confirming the manual's "immediate mode" statement | Direct |
| Fixture census after | 27 fixtures / 12172 lines, from 30 / 16978; >85% pairs 9→3, >90% pairs 3→1 | Direct |
| `just sel4_traffic_check`, `sel4_fault_check` | pass | Direct |
| `just sel4_saturation_check` | pass — all three ceilings still land *exactly* at their declared bounds | Direct |
| `just sel4_matrix_check` | pass — both arms, including `a graph promising a reader more than its writer offers is refused before any component launches` | Direct |
| `just contracts_check`, `just generation_check` | pass — two isolated builds byte-identical | Direct |
| `just data_fabric_profile_check`, `sel4_fabric_aggregate_check`, `sel4_boot_layout_check`, `sel4_gate_control_check` | pass | Direct |
| `just ruff` | pass | Direct |

## Decisions

- Decision: express the delta in `build-sel4.py`, not in `.zti`.
  Rationale: the format forbids composition by design, and the documented pattern
  is inert data plus a typed transformation — these fixtures are the inert data.
  `build-sel4.py` already supplied per-variant environment
  (`SLIME_FABRIC_PROXY_EARLY_EXIT`) and the builder already took
  `SLIME_GENERATION_NUMBER`, so this made an existing ad-hoc mechanism declarative
  rather than inventing one.
  Rejected alternative: a `.zt` transformation emitting each variant's `.zti`.
  That is the manual's blessed shape, but it would put a second generator between
  the fixtures and the builder for a two-field delta — more machinery than the
  duplication cost.

- Decision: keep per-variant generation numbers rather than deriving them.
  Rationale: `generation_check` builds each variant twice and compares bytes. Two
  variants resolving to one identity would make that assertion vacuous, so the
  numbers must stay distinct — and reusing the deleted fixtures' exact numbers
  means every plane's generation identity is unchanged by the collapse.

- Decision: make the overrides addressable and validated rather than a generic
  patch mechanism. Rationale: a general "override any manifest path" facility would
  let a variant express an arbitrary different graph, which is the duplication
  problem wearing a different hat. `route:component:field` with every part required
  to resolve keeps a delta a delta.

- Decision: teach the saturation gate to read the delta from the build rather than
  restating the narrowed ceiling.
  Rationale: the gate deliberately reads declared limits so loosening the fixture
  moves the assertion with it — a property worth preserving through the collapse.
  It failed the honest way first (`peak was 2, expected exactly 4`) when it read
  the shared fixture's headroom.
  Rejected alternative: hardcoding `2` in the gate — that reintroduces exactly the
  restated constant the gate's own comment argues against.

- Decision: leave `sel4-boot.zti` vs `sel4-traffic.zti` alone despite 94.4%
  similarity. Rationale: measured 203 changed lines across grants, bindings,
  budgets, trace depth, and `bootAction`. That is two compositions sharing most of a
  component set, not one with a variant; expressing it as overrides would encode a
  different graph as a patch list.

## Open risks and follow-ups

- [ ] The two overrides are build-environment strings, not schema-declared. A typo
  in `VARIANT_GENERATION_DELTAS` is caught (both overrides validate against the
  manifest), but *which* deltas a variant may declare is not itself a contract.
  Same class as B60's remaining Python derivations, one layer out.
- [ ] `sel4-qos.zti` vs `sel4-stream.zti` (88.3%) and `sel4-reclamation.zti` vs
  `sel4-supervision.zti` (86.7%) were not examined for collapsibility. Both are
  under the 90% line B62's exit condition drew, so they are out of scope here, not
  proven irreducible.
- [ ] **[INFERENCE]** Deleting the fixtures cannot have changed any plane's
  generation identity, because the delta reuses each deleted fixture's exact
  generation number and `generation_check` plus every plane's own admission chain
  pass unchanged. No byte-level before/after comparison of the built generations was
  taken across the change.

## Artifacts and provenance

- Focused report: none; the audit that opened B62 is
  [the structural audit entry](../2026-08-17-structural-audit/index.md).
- Raw transcript: none preserved; the fixture census is reproducible with a
  pairwise `difflib` ratio over `contracts/generation/v1/fixtures/sel4-*.zti`, and
  each gate result from its named `just` target.
- Serial/debugger/model output: the matrix refusal line is quoted in
  *Verification*; full transcripts regenerable from the named gates.
- Related roadmap item: [`roadmap/00-backlog.md`](../../roadmap/00-backlog.md) —
  B62 in the resolved log; B61, B63, B65 open.
  [B55](../2026-08-15-b55-full-graph-boot-restoration/index.md) is the staleness
  failure the duplication invited.
