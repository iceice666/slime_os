# CP12 — composition derivation for the one-to-one seL4 planes

| Field | Value |
|---|---|
| Date | 2026-09-02 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/system-spec/v1`, `contracts/composition-inventory/v1`, `contracts/component-spec/v1/components/`, `scripts/lib/system_spec.py`, `scripts/check/check-system-spec.py`, `scripts/check/check-composition-inventory.py`, `scripts/generate/generate-composition-inventory-bindings.py`, 21 `contracts/generation-manifest/v1/compositions/*.zti`, `just/contracts.just` |
| Roadmap | CP12 |
| Gates | `just system_composition_closure_check`, `just system_spec_check`, `just contracts_check`, `just generation_check`, `just sel4_boot_layout_check`, `just sel4_gate_control_check` |
| Trigger | CP11's closure contract landed, so the composition corpus became the remaining hand-authored input |
| Baseline | CP1 derived `valid.zti` and `sel4-channel.zti` from system specs; the other 40 compositions were hand-authored |

## Summary

CP1 proved generation derivation on two compositions and assumed one component
spec maps to one instance. Extending that to the corpus disproved the
assumption for 17 of the remaining 40 compositions and surfaced eight
generation-manifest sections the system-spec contract could not express at all.
This entry converts every composition that genuinely fits the one-to-one model —
22 of 42, up from 2 — after extending `contracts/system-spec/v1` with those
eight sections, five declared-binding/minted-binding facts, and nine per-instance
placement overrides. Each converted composition's `generation.bin` was rebuilt
from its frozen pre-migration text and from its derived text under one toolchain
and compared: 21 of 22 are byte-identical, and the 22nd (`sel4-channel`) is a
file this change does not touch. The remaining 20 compositions are recorded, per
composition and with a closed reason, in a new `contracts/composition-inventory/v1`
record that `just system_composition_closure_check` refuses to let drift.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/system-spec/v1/schema.zt` | Added `clockAuthority`, `ioResourceBudget`, `networkDestinations`, `blockRingAuthority`, `waitSet`, `schedulingClass`, `lifecyclePolicy`, `recording` and their `*Object` presence booleans | A composition's declared authority is expressible in its own source rather than only in the manifest it should be generated from |
| `contracts/system-spec/v1/schema.zt` | Added `ExtraBinding` and `SystemMintedBinding` | A spawn broker's third-party binding and a runtime-minted capability are declared where they are decided, not implied by a grant table that cannot imply them |
| `contracts/system-spec/v1/schema.zt` | Added nine `Placement` overrides: `health`, `role`, `dependencies`, `stackBytes`, the four shared-buffer ceilings, and `privatePageQuota` | Facts that vary per composition (`supervision-child` is `required` under one plane and `optional` under another; `slisp` is an application in the reference generation and its own bootstrap in `sel4-slisp`) stop being forced into one component-wide answer |
| `scripts/lib/system_spec.py` | `derive_bindings` widened with declared pins and extras; notification validation now admits several signallers with one waiter, matching `build-generation.py`'s own rule | The derivation reproduces the corpus's real binding and notification topology instead of the two fixtures' subset |
| `scripts/check/check-system-spec.py` | Normalizes binding order, minted-binding order, and absent-vs-empty optional sections; the post-baseline private-memory assertion is now placement-aware | The comparison stays an equality test over what the builder actually reads, with each normalization justified against the builder line that makes the two forms one build |
| `contracts/component-spec/v1/components/` | 11 new specs (`crossing-peer`, `reclamation-fault`, `sample-worker`, `supervision-child`, `c-runtime-probe`, six `robot-*`); `maxSpecs` 64 → 96 | Every executable the converted compositions declare has a reviewed component record |
| `contracts/composition-inventory/v1` | New contract, record, generated bindings, and gate | "Which compositions are migrated" is one closed record instead of a Python table, a directory listing, and a roadmap paragraph |
| `scripts/check/check-sel4-io-link-plane.py` | Fixture-text assertion whitespace-normalized | The plane asserts the declaration, not the spacing a hand-authored fixture happened to use |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A derived manifest silently diverges from its pre-migration content | `just system_spec_check` | `derived manifest diverges from <fixture>` naming the first differing field |
| A composition is hand-edited instead of regenerated | `just system_spec_check` | `stale derived generation fixture(s)` |
| The migration inventory drifts from the repository | `just system_composition_closure_check` | Per-row refusal naming the composition; 7 named mutations are proven refused |
| A converted composition's resolved slots move | `just sel4_boot_layout_check` | A plane layout mismatching its frozen `*.layout` fixture |
| A gate stops being able to fail | `just sel4_gate_control_check` | A mutated transcript accepted |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just system_composition_closure_check` | Pass — 23 systems compiled, 23 manifests derived semantically identical to their committed fixtures, 21 named mutations refused; 42 compositions inventoried (22 derived, 20 hand-authored), 7 inventory mutations refused | Direct |
| Baseline-vs-derived `generation.bin`, one toolchain, 22 compositions | 21 byte-identical; `sel4-channel` is unchanged by this commit and its pre-B91 baseline predates `slotReason`, so only the derived side builds | Direct |
| `just generation_check` | Pass — two isolated builds byte-identical, admission passed, 4 mutations refused | Direct |
| `just contracts_check` | Pass | Direct |
| `just sel4_boot_layout_check` | Pass — 31 plane layouts match their fixtures | Direct |
| `sel4_channel_check`, `sel4_crossing_check`, `sel4_supervision_check`, `sel4_reclamation_check`, `sel4_sample_check`, `sel4_powerbox_check`, `slisp_core_check` | Pass | Direct |
| `sel4_boot_check`, `sel4_stream_check`, `sel4_call_check`, `sel4_operation_check`, `sel4_visibility_check`, `sel4_qos_check`, `robot_runtime_check` | Pass | Direct |
| `sel4_demo_check`, `sel4_loan_check`, `sel4_spawn_check`, `sel4_traffic_check`, `io_queue_check`, `io_link_check`, `io_network_check` | Pass | Direct |
| `just sel4_gate_control_check` | Pass — 45 gates reject 1748 mutated transcripts and layouts | Direct |
| `just test_host`, `just ruff` | Pass | Direct |
| `just typos` | Fails on `contracts/system-image-closure/v1/inputs/sel4-prefix/libsel4/include/interfaces/sel4_client.h` ("pre-empted"), a vendored seL4 header committed by CP11 and untouched here; `typos` over every path this change touches is clean | Direct, pre-existing failure |

No Rust source changed, so `just lint_all` and `just fmt_check_all` have nothing
to say about this change; `git status` shows zero `.rs` files modified.

## Decisions

- **Decision:** Convert only the compositions whose instances map one-to-one onto
  a component spec, and record the rest in a closed contract with a per-composition
  reason.
  **Rationale:** The 17 multi-instance compositions need a system-spec *instance*
  record distinct from the executable it runs, carrying its own dependencies.
  That is a model change large enough to make the byte-identity claim for the 22
  that do fit unverifiable if bundled with it.
  **Rejected alternative:** Converting all 42 by special-casing the multi-instance
  planes in the generator — which CP12's own deliverable forbids ("adding the
  narrow owning contract field rather than a fixture-name branch").

- **Decision:** Every new authority section carries a companion `*Object` boolean
  rather than deriving resource-object presence from list non-emptiness.
  **Rationale:** The two are independent in the existing corpus. `sel4-io-network`
  carries a `wait-set` resource object with no declared wait-set source at all, so
  deriving presence would silently change which payload that generation encodes —
  the same trap `sharedBufferBudgetObject` already documents.
  **Rejected alternative:** Deriving presence and re-blessing the one composition
  that disagrees.

- **Decision:** A source's own retained binding on a delegated grant is admitted
  only when `slotPins` or `extraBindings` names it.
  **Rationale:** Measured against the corpus: 23 of 24 `sharedBufferFactory`,
  12 of 12 `directory`, and 13 of 14 `device`-kind source-side bindings are
  slot-pinned, and the one unpinned case (`sel4-loan`'s
  `init-shared-buffer-factory`) grants a component authority over itself, so its
  source and target bindings are one fact. Nothing structural was left to explain.
  **Rejected alternative:** Treating every delegating kind as binding both ends,
  which would invent 19 bindings no composition declares.

## Open risks and follow-ups

- [ ] 17 compositions remain `multiInstanceExecutable`; they need a system-spec
      instance record distinct from its executable, with per-instance
      dependencies. Tracked in `roadmap/00-backlog.md`; CP15's whole-corpus
      cutover depends on it.
- [ ] `sel4-filesystem` needs the `sel4-filesystem-service` / `filesystem-service`
      naming collision resolved before it can derive.
- [ ] `sel4-matrix` needs per-composition fabric route naming (or multi-route
      component interface entries) before it can derive.
- [ ] `sel4-c-runtime` needs a committed content identity for its C implementation
      before it can be pinned as an external implementation.
- [ ] `just typos` fails on a CP11-vendored seL4 header; unrelated to this change
      and not fixed here.

## Artifacts and provenance

- Closed inventory: `contracts/composition-inventory/v1/inventory.zti`
- Frozen pre-migration baselines: `contracts/system-spec/v1/baselines/*.zti` (23 files)
- Derived sources: `contracts/system-spec/v1/systems/*.zti` (23 files)
- Related roadmap item: [CP12](../../roadmap/10-component-platform.md)
