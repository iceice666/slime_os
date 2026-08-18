# CP1 — system specification model and generation derivation

| Field | Value |
|---|---|
| Date | 2026-08-18 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/system-spec/v1/`, `contracts/generation/v1/fixtures/{valid,sel4-channel}.zti`, `scripts/lib/system_spec.py`, `scripts/check/check-system-spec.py`, `scripts/generate/generate-generation-from-spec.py`, `scripts/generate/generate-system-spec-bindings.py`, `scripts/build/build-generation.py`, `components/bins/src/default_fabric_profile.rs`, `scripts/check/check-contracts.py`, `Justfile` |
| Roadmap | CP1, B70 |
| Gates | `just system_spec_check`, `just contracts_check`, `just generation_check`, `just sel4_generation_check`, `just sel4_boot_check` |
| Trigger | CP0 landed the component model; CP1 is the milestone that makes generation manifests derive from it |
| Baseline | `valid.zti` and every `sel4-*.zti` were hand-authored in parallel with the component model, which is the coupling B70 names |

## Summary

`contracts/generation/v1/fixtures/valid.zti` and `sel4-channel.zti` are now
generated from `contracts/system-spec/v1` sources plus the CP0 component corpus.
Their `executables`, `instances`, `objects`, `sharedBufferBudget`, and
`health.requiredInstances` sections are derived rather than typed: an
instance's bindings come from the grant table, its budget and health from its
component spec, its role and stack from `runtime.resource`. What stays declared
is what no component spec can know — the grant table itself, the notification
objects, the fabric graph, the boot profiles, and the state bindings.

The evidence is byte-level rather than structural. Building `sel4-channel` from
the derived fixture produces `generation.bin` and `boot-store.bin` byte-identical
to the frozen pre-CP1 fixture, and five QEMU planes boot on the derived
manifests. Reaching that required fixing a latent order-dependency in
`build-generation.py`: `declared_spawn_grant_counts` emitted rows in raw manifest
order into `FABRIC_MINTED_GRANTS`, which is compiled into `init`, so reordering
two instances changed a component ELF and therefore the generation identity —
even though every reader looks entries up by holder name.

A reviewer then caught a real defect the gate could not see: removing the four
bindings that name no grant freed the slots they occupied, and slot assignment
silently renumbered three live `generation-manager` bindings from 2/3/4 to 1/2/3.
Those are pinned now, and the gate resolves the frozen baseline independently so
no future removal can renumber a live slot unnoticed.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/system-spec/v1/schema.zt` | New `SystemSpec` covering the requirement document's §4.2 fields, reusing `FabricRoute`/`FabricParticipant` shapes rather than a second graph representation | A composition has a formal description; manifests derive from it (B70) |
| `contracts/system-spec/v1/systems/*.zti` | Two sources: `reference` and `sel4-channel` | The two converted fixtures have a source of truth above them |
| `contracts/system-spec/v1/baselines/*.zti` | The pre-CP1 hand-authored fixtures, frozen | The derivation is checked against what it must reproduce, not against its own output |
| `scripts/lib/system_spec.py` | Compiler, validator, and `derive_manifest` | One derivation both the gate and the generator use |
| `scripts/generate/generate-generation-from-spec.py` | Emits both fixtures; `--check` fails on drift | A hand-edited fixture is a gate failure, not a silent fork |
| `scripts/check/check-system-spec.py` | Derivation, slot-preservation, byte-drift, and 20 named refusals | The model is guarded rather than trusted |
| `scripts/build/build-generation.py` | `declared_spawn_grant_counts` sorts by holder name | Generated Rust is a function of manifest content, not of the order it was typed in |
| `contracts/generation/v1/fixtures/{valid,sel4-channel}.zti` | Replaced by generator output | The exit condition: both are generated artifacts |
| `components/bins/src/default_fabric_profile.rs` | Regenerated (row reorder only) | The checked-in fallback matches its generator |
| `scripts/check/check-contracts.py`, `Justfile` | Wire the contract and add `system_spec_check` | The new contract is type-checked with every other |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A fixture is hand-edited instead of regenerated | `just system_spec_check` | `valid.zti is stale: regenerate it with …` |
| The derivation stops reproducing the pre-CP1 manifest | `just system_spec_check` | `derived manifest diverges from valid.zti: …` |
| A removed binding silently renumbers a live slot | `just system_spec_check` | `derivation moved capability slot(s) the committed fixture pinned: generation-manager/… 1->2` — observed |
| A system spec declares a component, grant, or route the corpus does not back | `just system_spec_check` | the matching named refusal, 20 of them |
| A declared bound stops being enforced | `just system_spec_check` | `slotPins: N entries exceeds the declared bound of …` |
| The generation bytes change | `just generation_check`, `just sel4_boot_check` | byte comparison / marker chain |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `check-system-spec.py` | pass: 2 systems, 2 manifests derived, 20 named mutations refused | Direct |
| `generate-generation-from-spec.py --check` | both fixtures current | Direct |
| `check-contracts.py` | pass, now naming `system-spec`; 182 boot-contract tests pass | Direct |
| `check-generation-determinism.py` | pass: two isolated builds byte-identical | Direct |
| `check-component-spec.py`, `check-data-fabric-profile.py`, `check-boot-layout-resource.py`, `check-architecture-contract.py` | pass | Direct |
| **Byte equivalence**: build `sel4-channel` from the frozen baseline and from the derived fixture | `generation.bin` and `boot-store.bin` **identical** | Direct |
| `just sel4_channel_check` on QEMU | pass — the derived fixture boots | Direct |
| `just sel4_component_graph_check` on QEMU | pass | Direct |
| `just sel4_boot_check` on QEMU | pass: full fabric graph, both route workers, 30 markers across 5 causal chains | Direct |
| `just sel4_generation_check` on QEMU | pass — exercises the three slots the reviewer found had shifted | Direct |
| `just sel4_dango_check` on QEMU | pass — exercises the derived `commandProfile` | Direct |
| `just sel4_boot_layout_check` | 25 plane layouts match their frozen fixtures | Direct |
| Non-vacuity: neutralize the dependency rule | `a dependency the system does not admit was accepted`, then reverted | Direct |
| Non-vacuity: neutralize the fabric-graph rule | `a route role declared with no fabric graph was accepted`, then reverted | Direct |
| Slot-preservation probe: drop the three corrective pins | `derived manifest diverges … bindings[2].slot: 1 != 2` | Direct |
| Fresh reviewer pass | verdict `incorrect`: one P0 slot renumbering, two vacuous arms, eleven unchecked bounds — all fixed | Direct |

`ruff check scripts/`, `typos`, and `just devlog_check` pass.

## Decisions

- **Decision:** `declared_spawn_grant_counts` sorts by holder name.
  **Rationale:** deriving `sel4-channel` in canonical order produced a semantically identical manifest whose `generation.bin` differed. The list is rendered into `FABRIC_MINTED_GRANTS` and compiled into `init`, so instance order reached a component ELF. Every reader — `init.rs`'s `declared_minted_grants` — looks entries up by holder name, so the order was never meaningful, only accidentally load-bearing. Sorting makes the generated Rust a function of manifest content. This changes generated Rust for every manifest, so the whole seL4 plane suite was re-run.
  **Rejected alternative:** preserving each fixture's hand-authored order in the system spec, which would have carried the accident forward as a declared fact.

- **Decision:** the derivation drops four bindings naming no grant, and pins three slots to compensate.
  **Rationale:** `filesystem-store`, `generation-boot-update`, `health-confirmation`, and `store-access` are named by no grant; `resolve_boot_profile` drops any such binding, so no generation byte ever carried them. But they occupied slots 0–1 and 5 in `generation-manager`, so removing them let `assign_declared_slots` renumber three live bindings. The reviewer caught this; the gate could not, because it stripped before resolving so both sides shifted together. The three are pinned, and the gate now resolves the frozen baseline independently.
  **Rejected alternative:** keeping the dead bindings to avoid the shift, which would have carried text the builder already refuses to encode.

- **Decision:** no capability agreement between a system's grants and a component spec's `provides`/`requires` is enforced, and the code says why at length.
  **Rationale:** three progressively weaker rules were written and each was disproved by the two real fixtures — per-role equality and containment both fail because `console` *provides* an endpoint under `valid.zti` and *receives* one under the channel plane; kind containment fails because `init` holds an endpoint kind in the channel plane and none in `valid.zti`. The root cause is that CP0's corpus is authored against exactly one generation, so its capability sets record what `valid.zti` grants rather than what a component supports. `just component_spec_check` keeps the exact match against `valid.zti`, where it is true.
  **Rejected alternative:** keeping a weakened rule that happens to pass today, which would assert a property the corpus cannot support.

- **Decision:** a component spec's `runtime.executionEnvironment` is not required to equal the system's target.
  **Rationale:** the same components compose into both the x86_64 reference generation and the aarch64-seL4 channel plane. Per-image target qualification is stage-0's, through `contracts/component/v2`'s header, compared by equality before mapping bytes; restating a weaker version here would be a second and wrong authority.

- **Decision:** `sharedBufferBudgetObject` is declared rather than derived from "does any component have a budget".
  **Rationale:** the object's presence is what makes the builder encode a budget payload, and the fixtures disagree about the correlation — `sel4-channel` and `sel4-crossing` carry the object with an empty budget, `sel4-directory` and `sel4-filesystem` carry neither. Deriving it would silently change which payloads four existing generations encode.

## Open risks and follow-ups

- [ ] 25 `sel4-*.zti` fixtures remain hand-authored. CP1's exit condition covers `valid.zti` and the smallest seL4 manifest; converting the rest is the deferred follow-on the track already names.
- [ ] `reference.zti` carries 44 slot pins for 76 bindings, because `contracts/boot-layout/v1/fixtures/*.layout` freeze those numbers and `components/bins/build.rs` parses several out of the manifest text. Pins are declared data, so that half of the manifest is relocated rather than derived. CP2's runtime binding resolution is what removes the need for them.
- [ ] `fabricGraph`, `bootProfiles`, and `notifications` are copied through from the system spec into the manifest unchanged. They are genuinely composition facts, but a future contract could describe the fabric once and derive both sides.
- [ ] `Object.size` is carried by the manifest and read by nothing — no builder path and no Rust decoder consume it. The derivation reproduces it faithfully; whether the field should exist is a separate question.

## Artifacts and provenance

- Contract: [`contracts/system-spec/v1/schema.zt`](../../contracts/system-spec/v1/schema.zt)
- Frozen baselines the derivation is checked against: [`contracts/system-spec/v1/baselines/`](../../contracts/system-spec/v1/baselines/)
- Predecessor milestone: [`devlog/2026-08-18-cp0-component-spec-model/`](../2026-08-18-cp0-component-spec-model/index.md)
- Related roadmap item: [CP1 in the component platform track](../../roadmap/10-component-platform.md)
- Backlog item this bears on: [B70](../../roadmap/00-backlog.md)
