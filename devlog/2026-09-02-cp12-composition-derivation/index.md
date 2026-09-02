# CP12/CP13/CP14 — composition derivation, the closure builder, and scenario identities

| Field | Value |
|---|---|
| Date | 2026-09-02 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/system-spec/v1`, `contracts/composition-inventory/v1`, `contracts/component-spec/v1/components/`, `scripts/lib/system_spec.py`, `scripts/check/check-system-spec.py`, `scripts/check/check-composition-inventory.py`, `scripts/generate/generate-composition-inventory-bindings.py`, 21 `contracts/generation-manifest/v1/compositions/*.zti`, `just/contracts.just` |
| Roadmap | CP12, CP13, CP14 |
| Gates | `just system_image_scenario_check`, `just system_image_builder_check`, `just system_composition_closure_check`, `just system_spec_check`, `just contracts_check`, `just generation_check`, `just sel4_boot_layout_check`, `just sel4_gate_control_check` |
| Trigger | CP11's closure contract landed, so the composition corpus became the remaining hand-authored input |
| Baseline | CP1 derived `valid.zti` and `sel4-channel.zti` from system specs; the other 40 compositions were hand-authored |

## Summary

CP1 proved generation derivation on two compositions and assumed one component
spec maps to one instance. Extending that to the corpus disproved the
assumption for 17 of the remaining 40 compositions and surfaced eight
generation-manifest sections the system-spec contract could not express at all.
This entry converts 40 of the 42 compositions, up from 2. It extends
`contracts/system-spec/v1` with those eight sections, the declared binding and
minted-binding facts the grant table cannot imply, per-instance placement
overrides, and — the change that unblocked the other 18 — a `SystemInstance`
record that separates a concrete instance from the executable it runs, so
`clock-authority-probe` can be five instances and `supervision-child` 26, each
with its own authority, quotas, and dependencies on other instances of its own
executable. Every converted composition's `generation.bin` was rebuilt from its
frozen pre-migration text and from its derived text under one toolchain and
compared: all 40 are byte-identical. The remaining 2 are recorded, per
composition and with a closed reason, in a new
`contracts/composition-inventory/v1` record that
`just system_composition_closure_check` refuses to let drift.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/system-spec/v1/schema.zt` | Added `clockAuthority`, `ioResourceBudget`, `networkDestinations`, `blockRingAuthority`, `waitSet`, `schedulingClass`, `lifecyclePolicy`, `recording` and their `*Object` presence booleans | A composition's declared authority is expressible in its own source rather than only in the manifest it should be generated from |
| `contracts/system-spec/v1/schema.zt` | Added `ExtraBinding` and `SystemMintedBinding` | A spawn broker's third-party binding and a runtime-minted capability are declared where they are decided, not implied by a grant table that cannot imply them |
| `contracts/system-spec/v1/schema.zt` | Added `SystemInstance`: a concrete instance distinct from its executable, with per-instance owner, autostart, health, dependencies, quotas, and thread/priority fields; an empty list keeps the one-instance-per-component default | One executable can run under many instance names, which is what `Instance.executable` has always allowed and what 18 compositions use |
| `contracts/system-spec/v1/schema.zt` | Added `Placement.executableName` and a declared `bootLayoutObject` | A composition whose executable is named for its implementation binary (`sel4-generation-manager`, `sel4-filesystem-service`) can emit that name without a second spec claiming the same binary; `sel4-stress` carries no boot layout at all, so its presence is declared rather than assumed |
| `contracts/system-spec/v1/schema.zt` | Added nine `Placement` overrides: `health`, `role`, `dependencies`, `stackBytes`, the four shared-buffer ceilings, and `privatePageQuota` | Facts that vary per composition (`supervision-child` is `required` under one plane and `optional` under another; `slisp` is an application in the reference generation and its own bootstrap in `sel4-slisp`) stop being forced into one component-wide answer |
| `scripts/lib/system_spec.py` | `derive_bindings` widened with declared pins and extras; notification validation now admits several signallers with one waiter, matching `build-generation.py`'s own rule | The derivation reproduces the corpus's real binding and notification topology instead of the two fixtures' subset |
| `scripts/check/check-system-spec.py` | Normalizes binding order, minted-binding order, and absent-vs-empty optional sections; the post-baseline private-memory assertion is now placement-aware | The comparison stays an equality test over what the builder actually reads, with each normalization justified against the builder line that makes the two forms one build |
| `contracts/component-spec/v1/components/` | 25 new specs (11 for the one-to-one planes, 14 for the multi-instance planes: `clock-authority-probe`, `wait-set-probe`, `scheduling-class-probe`, `lifecycle-restart-probe`, the private-memory probes, and the storage/generation probes); `maxSpecs` 64 → 96 | Every executable the converted compositions declare has a reviewed component record |
| `scripts/generate/generate-system-image-closures.py` | New: emits one closure per derived composition (38) from repository state, with `--check` refusing drift | CP11's contract is exercised by the corpus rather than by one hand-authored record, while resolution stays an independent authority that re-reads every digest |
| `contracts/system-image-closure/v1/schema.zt` | Added a closed three-name `BuildParameter` vocabulary and taught the resolver to refuse any other; `build-system-image.py` applies the parameters from the resolved closure | The three deltas that changed generation bytes through ambient `SLIME_*` variables are in the build key, so two scenarios over one composition are two identities rather than one identity an environment disambiguated |
| `contracts/system-image-closure/v1/schema.zt` | Made `ImplementationSelection.buildProfile` a closed seven-name vocabulary mapping each executable-changing scenario to one `option_env!` knob; `build-system-image.py` translates resolved profiles into those knobs and refuses two profiles setting one knob to different values | A scenario ELF is reachable only through the identity that declares it, and the knob selection is in the build key rather than the caller's environment |
| `scripts/check/check-system-image-scenario.py` | New CP14 gate: parameter and profile vocabulary closure, unadmitted-name refusals, scenario identity distinctness, field-by-field proof that a parameter changes only what it names, eight malformed-parameter refusals, and a build arm proving a profile moves the named component's ELF reproducibly while leaving unnamed components byte-identical | A scenario cannot silently change bytes outside what it declares, and a profile recorded in the identity but changing no bytes would fail |
| `scripts/check/check-system-image-builder.py` | New CP13 gate: closure coverage, distinct identities, spec-matching manifests, twice-byte-identical builds, output-collision refusal, and an AST assertion that the builder declares no plane flag or variant table | Adding a composition needs contract data and a behavioral checker, never a builder flag or output-path branch |
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
| `just system_composition_closure_check` | Pass — 41 systems compiled, 41 manifests derived semantically identical to their committed fixtures, 21 named mutations refused; 42 compositions inventoried (40 derived, 2 hand-authored), 7 inventory mutations refused | Direct |
| Baseline-vs-derived `generation.bin`, one toolchain, 40 compositions | All 40 byte-identical (39 measured directly; `sel4-channel` is unchanged by this commit and its pre-B91 baseline predates `slotReason`, so only the derived side builds) | Direct |
| `just generation_check` | Pass — two isolated builds byte-identical, admission passed, 4 mutations refused | Direct |
| `just contracts_check` | Pass | Direct |
| `just sel4_boot_layout_check` | Pass — 31 plane layouts match their fixtures | Direct |
| `sel4_channel_check`, `sel4_crossing_check`, `sel4_supervision_check`, `sel4_reclamation_check`, `sel4_sample_check`, `sel4_powerbox_check`, `slisp_core_check` | Pass | Direct |
| `sel4_boot_check`, `sel4_stream_check`, `sel4_call_check`, `sel4_operation_check`, `sel4_visibility_check`, `sel4_qos_check`, `robot_runtime_check` | Pass | Direct |
| `sel4_demo_check`, `sel4_loan_check`, `sel4_spawn_check`, `sel4_traffic_check`, `io_queue_check`, `io_link_check`, `io_network_check` | Pass | Direct |
| `clock_authority_check`, `wait_set_check`, `scheduling_class_check`, `private_memory_check`, `lifecycle_restart_check`, `sel4_stress_check` | Pass — the multi-instance planes, including the 23-instance stress graph | Direct |
| `sel4_directory_check`, `sel4_input_check`, `sel4_filesystem_check`, `sel4_storage_check`, `sel4_store_check`, `sel4_transfer_check` | Pass | Direct |
| `sel4_generation_check`, `sel4_recovery_plane_check`, `sel4_rollback_check`, `replay_check`, `io_block_check`, `io_driver_authority_check` | Pass | Direct |
| `just sel4_gate_control_check` | Pass — 45 gates reject 1748 mutated transcripts and layouts | Direct |
| `just system_image_builder_check` | Pass — 38 closures resolve with distinct identities and spec-matching manifests, 3 compositions declared closure-exempt, `sel4-channel` built twice byte-identically, a non-empty output directory refused | Direct |
| `just system_image_closure_check` | Pass against the generated `sel4-channel` closure | Direct |
| `just system_image_scenario_check` | Pass — 3 admitted parameters and a fourth refused; 3 scenario closures with identities distinct from their base and each other, each changing exactly the fields it names; 8 malformed parameters refused; the 7-name profile vocabulary closed with an unadmitted profile and a same-knob conflict both refused; `sel4-stream-death`'s profile moved `fabric-publisher.elf` `cd5cedc17aea` → `7c3f028a5226` reproducibly with `init.elf` unchanged and the two images differing | Direct |
| `just test_host`, `just ruff` | Pass | Direct |
| `just typos` | Fails on `contracts/system-image-closure/v1/inputs/sel4-prefix/libsel4/include/interfaces/sel4_client.h` ("pre-empted"), a vendored seL4 header committed by CP11 and untouched here; `typos` over every path this change touches is clean | Direct, pre-existing failure |

No Rust source changed, so `just lint_all` and `just fmt_check_all` have nothing
to say about this change; `git status` shows zero `.rs` files modified.

## Decisions

- **Decision:** Separate a concrete instance from the executable it runs, with an
  empty `instances` list preserving the one-instance-per-component default.
  **Rationale:** CP1's one-to-one assumption is not general and never was:
  `Instance.executable` has always been a distinct manifest field, and 18
  compositions use it. Landing the conversion in two steps — the one-to-one
  planes first, then the instance model — kept each byte-identity claim
  independently verifiable.
  **Rejected alternative:** Special-casing the multi-instance planes in the
  generator, which CP12's own deliverable forbids ("adding the narrow owning
  contract field rather than a fixture-name branch").

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

- [ ] `sel4-matrix` remains `routeNameVariance`: three fabric components hold
      route roles their specs do not declare, and `just component_spec_check`
      requires a spec's interface list to match `valid.zti`'s graph exactly, so
      one spec cannot describe two compositions' route sets. Needs
      per-composition interface entries or a system-level route-role override.
- [ ] `sel4-c-runtime` remains `unpinnedExternalImplementation`: its C
      implementation is built by a helper script at gate time with no committed
      content identity to pin.
- [ ] CP13's legacy surface — `build-sel4.py`'s `--*-plane` family, `VARIANT_*`
      tables, and the identity manifest's `variant` authority — is retained
      because the 36 QEMU plane checkers still call it. CP15 owns migrating
      those checkers and deleting the flags.
- [ ] CP14's remaining three deliverables: the boot selector, root fixture,
      reclamation unwind probe, and board instrumentation are still root variants
      rather than closed root roles; B40 mutations are still an ambient
      `SLIME_B40_MUTATION`; and QEMU disks, device topology, and corruption
      schedules have not moved into `system-test-run/v1`.
- [ ] The `generationCmd*`, `bootSelectionFail`, and `recoveryImage` profiles are
      declared and gated but not yet carried by a closure: their host
      compositions build through the legacy path CP15 migrates.
- [ ] `sel4`, `sel4-slisp`, and `reference` have no closure: the first two admit
      an external product Slisp ELF with no committed artifact, and the third
      targets a platform with no seL4 asset.
- [ ] `just typos` fails on a CP11-vendored seL4 header; unrelated to this change
      and not fixed here.

## Artifacts and provenance

- Closed inventory: `contracts/composition-inventory/v1/inventory.zti`
- Frozen pre-migration baselines: `contracts/system-spec/v1/baselines/*.zti` (41 files)
- Derived sources: `contracts/system-spec/v1/systems/*.zti` (41 files)
- Generated closures: `contracts/system-image-closure/v1/closures/*.zti` (38 files)
- Related roadmap items: [CP12, CP13](../../roadmap/10-component-platform.md)
