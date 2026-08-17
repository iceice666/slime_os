# B60 — two scoping mistakes on the way to asserting one slot number

| Field | Value |
|---|---|
| Date | 2026-08-17 |
| Kind | Change |
| Status | Verified |
| Scope | `scripts/build/build-generation.py` (`_control_sources`, `_assert_declared_control_slots`, `resolve_fabric_profile`), `contracts/generation/v1/schema.zt` (`FabricProfile`), 7 `.zti` fixtures |
| Roadmap | B60, B55, B56 |
| Gates | `just contracts_check`, `just generation_check`, `just data_fabric_profile_check`, `just sel4_boot_check`, `just sel4_boot_layout_check` |
| Trigger | The structural audit traced one control slot to two independent sources joined only by a comment |
| Baseline | B55's fix was reactive — the divergence was found by a boot failure, not by the build |

## Summary

A fabric control slot had two sources: the fixture pinned an integer per binding,
and the broker recomputed it at runtime as `FABRIC_FIRST_CONTROL_SLOT +
position(component)`. Nothing but a comment asserted they agreed — and B55's first
defect was exactly that disagreement, discovered by a boot failure. There is now a
build-time cross-check, and it took two wrong scopings to get right: the first
compared the *client's* binding against the holder's table, the second demanded
the per-plane numbering from a reference manifest whose single broker holds three
planes at once. That second mistake is B56's shape — a rule swept across profiles
only some can satisfy — and it was caught by a gate rather than by a boot. The
holder and the stream plane's membership also moved out of Python string
comparisons into the manifest and the schema.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `build-generation.py::_assert_declared_control_slots` | New: refuses a manifest whose pinned control slots disagree with the derived order, at build time | A B55-class divergence fails the build, not the boot |
| `build-generation.py::_control_sources` | Reads the holder from the grants and checks one plane terminates at one component, instead of `"fabric-call-worker" if profile == UNIFIED else "fabric-service"` | Where authority terminates is the manifest's to declare |
| `build-generation.py` | Operation and replacement controls checked to share a holder — they share one worker's table | Two grant families cannot split a table |
| `contracts/generation/v1/schema.zt` | `FabricProfile` gained `streamControls`, with order documented as authority-bearing | Plane membership is schema-declared |
| `build-generation.py` | `FABRIC_BOOT_STREAM_CONTROL_GRANTS` and `FABRIC_MATRIX_STREAM_CONTROL_GRANTS` deleted — two byte-identical tuples selected by profile name; their rationale kept as comments on the surviving default | The builder is no longer the authority on plane membership |
| 7 `.zti` fixtures | The six profile-bearing ones declare their seven-entry stream plane; `valid.zti`'s three profiles declare theirs | A profile declaring none keeps the single-broker default byte-for-byte |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A fixture's pinned control slot drifts from the order the broker indexes (B55's mechanism) | `just contracts_check`, `just generation_check` — the build itself refuses it | `<holder>'s binding for <grant> pins slot N but the plane derives M` |
| One plane's controls are split across two holders, so a worker cannot authenticate a client by the endpoint it arrived on | `_control_sources` holder check, run by every build | `control grant <name> terminates at X but its plane terminates at Y` |
| The operation plane's two grant families land on different holders while sharing one table | same, checked explicitly after resolution | `operation controls terminate at X but their replacement controls terminate at Y` |
| A profile's declared `streamControls` and its resolved controls stop being the same filter, so the cross-check silently compares a shifted pairing | `_assert_declared_control_slots` length check | `plane <name> resolved N controls from M declared grants; the control-slot cross-check cannot pair them` |
| Moving plane membership into the schema changes a resolved layout | `just data_fabric_profile_check` byte-compares the checked-in profile; `just sel4_boot_layout_check` compares 25 plane layouts | Stale-profile failure, or a layout fixture mismatch |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Guard bites: perturb `sel4-boot.zti`'s `fabric-op-time-control` 5→9 | `fabric-op-worker's binding for fabric-op-time-control pins slot 9 but the plane derives 5`; reverted | Direct |
| `just contracts_check` | pass — all 31 manifests satisfy the cross-check | Direct |
| `just generation_check` | pass — two isolated builds byte-identical | Direct |
| `just data_fabric_profile_check` | pass — and the resolved profile is byte-identical, so the membership move is a pure refactor | Direct |
| `just sel4_boot_check`, `sel4_matrix_check`, `sel4_traffic_check`, `sel4_fault_check`, `sel4_stream_check`, `sel4_visibility_check` | pass — every profile-bearing plane boots | Direct |
| `just sel4_call_check`, `sel4_operation_check` | pass — the two worker-holder planes | Direct |
| `just sel4_boot_layout_check` | pass — 25 plane layouts unchanged | Direct |
| `just ruff`, `just fmt_check_all` | pass | Direct |

## Decisions

- Decision: compare only the **holder's** binding.
  Rationale: an endpoint grant installs both ends, so it has two bindings — the
  client's, numbered in the client's own namespace (slot 0 for its single
  control), and the holder's, which is the table the broker indexes. The first
  draft compared whichever came first and reported `pins slot 0 but call derives
  2`, which is two unrelated numberings disagreeing, not a defect.
  Rejected alternative: comparing both and treating the client's as its own
  sequence — the client side has no derived order to compare against.

- Decision: compare only a holder owning **one** plane.
  Rationale: each plane numbers from `FABRIC_FIRST_CONTROL_SLOT` independently,
  because C8.10's route workers are separate tasks with separate capability
  tables. `valid.zti`'s `fabric-service` holds stream, call, and operation
  controls in one table and must lay them out consecutively. Asserting the
  per-plane rule against it demanded a contradiction.
  Rejected alternative: "fixing" `valid.zti` to match. I started down this path —
  computing corrected slots, editing two bindings, hitting a collision with
  `fabric-shared-buffer-factory` at slot 2 — before checking `sel4-call.zti`, the
  fixture that actually *boots* the call plane, which already had 2,3,4,5. That
  comparison is what showed the reference manifest was consistent and the
  assertion was over-broad. The edits were reverted.

- Decision: keep `operationReplacement` folded into `operation` for the
  one-plane test. Rationale: it is numbered as a continuation of the operation
  plane (`FIRST + len(operation) + index`), not as a plane of its own, so counting
  it separately would exempt every operation holder from the check.

- Decision: declare `streamControls` per profile rather than deriving the
  full-graph list from the default plus a delta.
  Rationale: the stream plane's supervision slots are numbered
  `FIRST_CONTROL_SLOT + len(controls) + index`, so lengthening a shared list
  renumbers supervision handles the C8.3–C8.8 gates grant positionally. Declaring
  in full lets a profile that declares nothing keep its layout byte-for-byte.
  Rejected alternative: one list with per-profile additions — the additions change
  the length, which is the thing that must not move for earlier profiles.

- Decision: leave the two Python tuples' rationale as comments on the surviving
  default rather than deleting it with them. Rationale: the reasoning (why
  `fabric-intruder` drops out of the boot plane, why `fabric-probe` must hold a
  real control endpoint in the matrix plane) explains the fixtures' contents and
  would otherwise be lost with the code it annotated.

## Open risks and follow-ups

- [ ] Two derivations the audit named remain in Python: supervision-table
  membership (the ring ∪ proxy ∪ matrix-denied-probe set comprehension) and
  notification-slot naming by `removeprefix`/`rpartition` string surgery. Both are
  now guarded from the *slot* direction — a divergence between a derived row and a
  pinned one fails the build — but neither rule is schema-declared. Recorded in
  B60's resolved entry rather than left implicit.
- [ ] The cross-check exempts multi-plane holders entirely rather than checking
  them against a consecutive-layout rule. A `valid.zti`-shaped manifest could
  still pin a wrong slot and only fail at boot. Deriving the consecutive layout
  would mean the builder owning *that* rule too, which is the thing B60 was
  removing; a per-holder declared base offset would be the schema-first version.
- [ ] **[INFERENCE]** `valid.zti`'s `fabric-service` binding table is internally
  consistent, judged by its planes being laid out consecutively without collision
  and by `data_fabric_profile_check` passing. It is not booted with a call worker
  by any gate, so no run confirms those particular slots resolve.

## Artifacts and provenance

- Focused report: none; the audit that opened B60 is
  [the structural audit entry](../2026-08-17-structural-audit/index.md).
- Raw transcript: none preserved; the guard's refusal is quoted above and
  reproducible by perturbing one pinned slot and building with
  `SLIME_TARGET_PROFILE=aarch64-sel4-qemu-virt SLIME_SEL4_MANIFEST=sel4-boot`.
- Serial/debugger/model output: none — every check here is build-time.
- Related roadmap item: [`roadmap/00-backlog.md`](../../roadmap/00-backlog.md) —
  B60 in the resolved log; B61–B65 open. [B55](../2026-08-15-b55-full-graph-boot-restoration/index.md)
  is the boot failure this now prevents, and
  [B56](../2026-08-17-c8-15-fabric-aggregate/index.md) is the over-broad-rule
  mistake this repeated and caught.
