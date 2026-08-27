# Direction 24: capability rights as conservation laws, not a vacuous closure

| Field | Value |
|---|---|
| Date | 2026-08-27 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/capability-rights/model/`, generation v5 rights vocabulary, capability enforcement tests, `Justfile`, capability matrix, direction register, Authority A0 |
| Roadmap | A0 |
| Gates | `just capability_rights_model_check`, `just test_host`, `just test_sel4_root`, `just contracts_check`, `just generation_check` |
| Trigger | Direction 24 occupied the register's single probing slot without an executable authority model or a drift guard tying the rights vocabulary to enforcement |
| Baseline | Capability rights were enforced in Rust and described in the matrix, but no bounded transition model checked delegation and transfer mutations |

## Summary

Direction 24 produced a bounded capability-rights model and promoted it as
Authority A0. The probe's main result is negative but load-bearing: manifest
rights closure cannot be the primary theorem because per-component closure is
violated by honest transfer, union closure is vacuous, and edge-scoped closure
cannot detect widening derive. The landed model instead checks per-operation
conservation laws, six must-fail mutations, and a single-sourced 33-name rights
vocabulary pinned against real manifest and runtime enforcement. The partition
analysis also corrected the capability matrix's false claim that eight named
rights were unassigned.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Generation vocabulary | Moved `RightBit`, `right`, and `rightBits` verbatim to `contracts/generation/v5/vocab/rights.zt`; both the generation schema and top-level package resolve that module | One canonical rights vocabulary produces the generated bindings and is importable by other contracts |
| Manifest enforcement | Added `declared_rights_partition_into_manifest_declarable_and_root_only` over all 33 named rights | Every right is classified; changing `capability_rights_valid` cannot silently move a bit between manifest-declarable and rejected classes |
| Runtime enforcement | Added the `graph` test covering four root-only supervision rights and all eight named-but-ungated rights | Root-minted policy rights remain intentionally distinct from manifest rights, and no runtime rights type admits a dead right |
| Capability matrix | Replaced “unassigned” for bits 4–7 and 12–15 with their canonical names, ungated status, and retired-kernel provenance | Prose no longer contradicts the schema or `RIGHT_ALL` |
| Checked model | Added six operations, seven state safety properties, four reachability witnesses, one safe scenario, and six mutation scenarios | Derive and transfer narrowing, transfer authority, edge confinement, kind validity, and non-duplication are executable contracts |
| Gates and roadmap | Added `capability_rights_model_check`, made it a `contracts_check` prerequisite, promoted direction 24 to Authority A0 | Rights-algebra validation runs with generation contracts and future authority work has a canonical checked baseline |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A model mutation stops reaching its named property | `just capability_rights_model_check` | `FAILED (expected violation of "…", none found)` exits non-zero |
| A right is added or reclassified without updating enforcement documentation | `just test_host` | The two partition arrays no longer union to `RIGHT_ALL`, or a bit changes admission class |
| Root-only or ungated rights leak into a runtime capability type | `just test_sel4_root` | `CapabilityEntry::supervision` or `CapabilityEntry::block` accepts a forbidden bit |
| Vocabulary extraction changes generated ABI data | `just contracts_check`, `just generation_check` | Generated bindings drift or generation fixtures change |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `python3 scripts/generate/generate-boot-bindings.py --check` | Passed; generated bindings current | Direct |
| SHA-256 of `boot-contracts/src/generated/generation.rs` and `scripts/lib/boot_contracts.py`, before and after extraction | Byte-identical: `4876e6…e4b7` and `a9ca4c…f4f` | Direct |
| `just test_host` | Passed; 306 boot-contract tests, including the 33-right partition test, plus protocol suites | Direct |
| `just test_sel4_root` | `184/184 across 19 modules` | Direct |
| `just capability_rights_model_check` | Main: 300 states / 1436 transitions; all seven scenarios passed | Direct |
| Same model with `wideDerive.wideningDerive = false` | Exited 1 with `FAILED (expected violation of "DeriveOnlyNarrows", none found)`; change reverted | Direct |
| `just fmt_check_all` | Passed after formatting the Rust additions | Direct |
| `just lint_all` | Passed with warnings denied | Direct |

Probe measurements recorded in the direction file: seven scenarios took about
1.5 seconds on an M5 Pro during planning; right-count growth was 0.55 seconds at
three rights, 3.1 seconds at four, 19 seconds at five, and 121 seconds at six.
The current gate run reported the same 300-state/1436-transition main graph.

## Decisions

- Decision: use `DeriveOnlyNarrows`, `TransferOnlyNarrows`,
  `TransferRequiresTransferRight`, `TransferFollowsDeclaredEdge`,
  `RightsValidForKind`, and `NoTransferDuplication` as the load-bearing
  properties; retain `NoAuthorityWidening` only as a weaker corollary.
  Rationale: all three closure formulations measured by the probe were false,
  vacuous, or blind to widening derive.
  Rejected alternative: preserve the original manifest-closure exit condition
  and weaken its definition until the model passes.
- Decision: model four symbolic equivalence classes and pin the full vocabulary
  separately against enforcement.
  Rationale: transition cost grows about 6.3 times per added right, making all
  33 rights infeasible in this model shape; symbolic classes preserve the
  operation laws without creating a second vocabulary copy.
  Rejected alternative: import all canonical names into the transition state,
  which would look more direct but cannot be exhaustively checked.
- Decision: promote the probe as Authority A0 with a same-change lockstep rule.
  Rationale: changes to `rightBits`, `capability_rights_valid`, or a
  `rights_type!` mask alter the checked algebra and must move the model and
  matrix together.
  Rejected alternative: leave the direction probing or parked after landing the
  gate, which would keep the register status inconsistent with the observed
  result.

## Open risks and follow-ups

- [ ] The non-consuming `retain=true` export variant is not modelled.
- [ ] Descriptor and native-endpoint ticket movement is collapsed between
  export and import.
- [ ] Endpoint transferability is represented as a symbolic transfer right even
  though the implementation stores it in `PeerEndpointTable`.
- [ ] The four-class abstraction is documented rather than established by a
  machine-checked refinement proof.

## Artifacts and provenance

- Focused report: [direction 24 probe outcome](../../docs/directions/24-rights-algebra-model.md)
- Serial/debugger/model output: command output observed directly in the implementation session; no separate frozen artifact was added
- Related roadmap item: [Authority A0](../../roadmap/06-authority-trust.md#a0--checked-capability-rights-algebra)
