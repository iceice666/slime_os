# Zutai's field-pun shorthand, and why `schemaFields` cannot reach these contracts

| Field | Value |
|---|---|
| Date | 2026-08-14 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/**/*.zt` (78 files), `deps/zutai` pin `f232532` → `9352235` |
| Roadmap | none |
| Gates | `just contracts_check`, `just generation_check`, `just interface_schema_check`, `just bootstate_model_check`, `just architecture_contract_check`, `just data_fabric_profile_check`, `just sample_descriptor_check`, `just rpi5_ros2_demo_contract_check`, `just test_host`, `just fmt_check_all`, `just ruff`, `just typos` |
| Trigger | `deps/zutai` submodule advanced two commits past its pin with `feat(general/syntax): extend field-pun shorthand to patterns` and `feat(stdlib/reflect): add erased schemaFields accessor` |
| Baseline | 42 generated artifacts reproduced byte-identically by all 18 `scripts/generate/*.py` at pin `f232532` |

## Summary

The Zutai submodule carried two unadopted features. Only one of them is usable
here. The field-pun shorthand (`name =;` for `name = name;`) applies to 785
record-literal fields across 78 contract files and is now adopted repo-wide,
with every one of the 42 generated artifacts proven byte-identical afterward.
The new `stdlib.reflect.schemaFields` accessor, which exists precisely to
remove the `(schema T).fields ?? {;}` spelling this repository writes 128 times,
**cannot be adopted**: the TLC fold that lets reflection coexist with effects
only recognizes the *ambient* `schema` builtin applied to a literal type, and
every contract schema owns an effectful `main`. Substituting the module-qualified
wrapper turns all 128 sites into a hard refusal. The 128 `?? {;}` sites
therefore stay, deliberately, and the pin bump is still mandatory because the
new stdlib does not load on the old compiler.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/**/*.zt` | 785 `name = name;` record fields rewritten to `name =;` across 78 files | Contract sources use the same field-pun spelling the upstream stdlib migrated to; generated output unchanged |
| `deps/zutai` | Pin advanced `f232532` → `9352235` | The stdlib shipped in the submodule tree actually loads on the pinned compiler |
| `contracts/.../schema.zt` (128 sites) | **Unchanged** — `(schema T).fields ?? {;}` kept | Reflection keeps folding before effect lowering, so every contract still runs |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Pun transform corrupts a field name or wire layout | `just contracts_check` | `<family> schema reflection/layout validation failed` |
| Rewritten schema silently changes emitted bindings | all 18 `scripts/generate/*.py` re-run, then `git status` | any generated file appears modified |
| Generation bytes drift | `just generation_check` | non-identical `generation.bin` / `boot-store.bin` across two isolated builds |
| Transform touches comment or string text | comment/string-span-aware rewriter, unit-tested on 11 adversarial cases | punned text inside `--` comment or `"…"` literal |
| Negative fixtures stop rejecting | `check-invalid-layout.zt` sentinel + injected-drift control test | `INVALID_GENERATION_SCHEMA` absent, or bad layout name accepted |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Baseline: 18 generators at old pin, then `git status` | clean — committed output is reproducible | Direct |
| 18 generators after rewrite, then `git status` | only `contracts/` and `deps/zutai` modified; **all 42 generated artifacts byte-identical** | Direct |
| `just contracts_check` | pass — 26 contract families validated | Direct |
| `just generation_check` | pass — two isolated builds byte-identical, `d4d5dcc7…` | Direct |
| `just bootstate_model_check` | pass — 4 scenarios, 5416 states, 3 expected violations | Direct |
| `just interface_schema_check` | pass | Direct |
| `just architecture_contract_check` | pass — 181 boot-contracts tests | Direct |
| `just data_fabric_profile_check`, `just sample_descriptor_check`, `just rpi5_ros2_demo_contract_check` | pass | Direct |
| `just test_host` | pass | Direct |
| `just fmt_check_all`, `just ruff`, `just typos` | pass | Direct |
| Control: inject `op` → `op_WRONG` in `block` layout | generator refuses (`block schema reflection/layout validation failed`) | Direct |
| Control: old compiler (`f232532`) + new stdlib | `zutai::import::module_has_errors` on `stdlib.result` — pin bump is mandatory | Direct |
| Control: `refl.schemaFields` in an effectful module | `EffectfulNotExecutable` refusal, at every spelling tried | Direct |

## Decisions

- **Decision:** Adopt the field-pun shorthand in record literals only; keep all
  128 `(schema T).fields ?? {;}` sites.
- **Rationale:** `deps/zutai/crates/general/tlc/src/lower/expr/mod.rs` folds a
  `schema` builtin application whose argument is a *literal* `TypeValue`.
  `refl.schemaFields` is a closure application through a module field, so
  lowering leaves a residual `Type`, `residual_type_values` stays set,
  `tlc_reflection_folded()` returns false, and `analysis_eval.rs:73` refuses any
  module that also has effect syntax. Every contract schema ends in an effectful
  `main` writing bindings through `FsWrite`, so the wrapper is refused at all
  128 sites. Confirmed against four spellings — `refl.schemaFields T`,
  destructured `{ schemaFields; } ::= import`, a `::=` compile-time binding, and
  the reflection isolated in a pure imported module consumed by an effectful
  root. Only the ambient `schema T` form survives contact with effects.
- **Rejected alternative:** `zutai-cli format` in place. It does not apply puns
  and reflows the repository's `main` indentation, producing unrelated churn.
- **Rejected alternative:** Pattern-position field puns, the other new feature.
  The contracts contain no punnable record patterns — the recursive `match`
  arms are *list* patterns (`{ field; ...rest }`), and the only three
  tagged-record payload patterns, in `contracts/bootstate/model/bootstate.zt`
  lines 472–474, rebind to different names (`{ value = generation; }`), where a
  pun is not applicable.
- **Rejected alternative:** Leaving the pin unbumped. The new stdlib uses
  pattern puns internally, so it fails to load on the old compiler; stdlib and
  compiler advance atomically.

## Open risks and follow-ups

- [ ] The 128 `?? {;}` sites remain until the Zutai TLC fold recognizes a
  reflection call reached through a module field, or `schemaFields` is exposed
  as an ambient builtin. Upstream tests for it
  (`crates/general/eval/src/tests/imports.rs:671-686`) only cover pure
  programs, so the effectful case is unguarded upstream; this is a Zutai-side
  change, not a contract-side one.

## Artifacts and provenance

- Focused report: this entry
- Raw transcript: none retained; every command above is reproducible from the listed gates
- Serial/debugger/model output: `just bootstate_model_check` — 4 scenarios, 5416 states
- Related roadmap item: none
