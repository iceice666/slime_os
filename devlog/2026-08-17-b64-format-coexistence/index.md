# B64 — the rollback answer was already in the code; four of the five "dead" schemas were live

| Field | Value |
|---|---|
| Date | 2026-08-17 |
| Kind | Change |
| Status | Verified |
| Scope | `roadmap/README.md` (invariant 7), `scripts/check/check-sel4-boot-selection.py`, `scripts/check/check-contracts.py` |
| Roadmap | B64, B50 |
| Gates | `just sel4_boot_selection_check`, `just contracts_check` |
| Trigger | The structural audit judged the equality version gate irreconcilable with rollback, and reported five dead schema trees |
| Baseline | `Generation::decode` refuses a superseded magic and the selector spends the pending attempt before decoding — both undocumented and unexercised |

## Summary

B64 as opened had two claims and both were partly wrong. Four of the five "dead"
schema trees are live inputs to `check-contracts.py`, two of them supplying
negative controls that assert a wire-layout mismatch is rejected; deleting them
would have removed real coverage. And the format-coexistence answer already
existed in `slime-root`: the decoder distinguishes an older Slime generation from
a foreign blob, and the boot selector spends the pending attempt *before* decoding
the candidate, so an undecodable pending generation rolls back to known-good
within its declared attempts. The real defect was that this was inferable only by
reading two files and no gate observed it. Now invariant 7 states the rule and a
new boot-selection arm proves it on hardware-equivalent QEMU. `generation/v4` —
the one genuinely unguarded retained version — joined the type-check sweep.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `roadmap/README.md` invariant 7 | States that a superseded wire format counts as a failed pending generation, names both mechanisms (`UnsupportedVersion` vs `BadMagic`; attempt consumed before decode), and points at the gate | The rollback rule is written down, not inferable |
| `check-sel4-boot-selection.py` | New arm: `restamp_wire_version` stamps a pending generation to the v4 magic and version, given one declared attempt; the root must refuse it and the next boot must already be known-good, with only BootState sectors touched | An undecodable candidate cannot consume the last selectable root |
| `check-sel4-boot-selection.py` | New `boot_refused` helper — the existing `boot` treats any root fatal as a failed run, which is right for arms whose candidate is supposed to start | An expected refusal is observable |
| `check-contracts.py` | `contracts/generation/v4` added to the retained-version type-check sweep | No retained schema rots unnoticed |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A future root migrates or silently accepts a superseded generation instead of refusing it | `just sel4_boot_selection_check` stale-format arm | `an undecodable pending generation was selected as if valid`, or the refusal marker never appears |
| An undecodable pending generation consumes the known-good root or retries forever | same arm's second boot plus `only_slots` | Fallback boot is not `number=1`, or non-BootState sectors changed |
| A retained `contracts/generation/vN` stops parsing | `just contracts_check` | Zutai check failure on that version's `schema.zt`/`gen_rust.zt` |
| The v2/v3 wire-layout negative controls are lost | `just contracts_check` | `generation wire-layout mismatch was not rejected` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_boot_selection_check` | pass — summary now records the stale-format refusal | Direct |
| Arm bites: neutralize both header stamps so the candidate stays decodable | Fails — the candidate boots and dies at runtime instead: `SLIME_ROOT FATAL SLIME_GRAPH FAIL required instance init exit status=1`. Reverted | Direct |
| Arm bites: neutralize only the magic | Still passes — the version word alone suffices to refuse, which is why the arm stamps both | Direct |
| `just contracts_check` | pass — v2, v3, and now v4 type-check | Direct |
| All three retained versions checked directly with the Zutai CLI | `v2`, `v3`, `v4` × `schema.zt`/`gen_rust.zt` all OK | Direct |
| `just ruff` | pass | Direct |

## Decisions

- Decision: **do not** delete `contracts/component/v1`, `kernel-image/v1`,
  `generation/v2`, or `generation/v3`.
  Rationale: `check-contracts.py` type-checks all four, and v2/v3 additionally run
  `check-invalid-layout.zt` asserting a wire-layout mismatch is rejected. The
  audit's evidence was a `grep` for *generator* references, which does not see a
  gate consuming a schema directly. `check-generation-v5.py` also states the
  policy explicitly: format history is retained; what must not survive is a
  producer.
  Rejected alternative: deleting them as the audit proposed — it would have
  removed two negative controls and contradicted a written policy.

- Decision: document format bumps as rollback-safe **by refusal**, not
  rollback-compatible by migration.
  Rationale: this is what the code already does, and it is the safer of the two
  options the audit posed. A migration path would mean the root decoding formats it
  was not built for, which is more attack surface at the least recoverable moment
  in the boot. Refusal plus attempt-consumption already satisfies invariant 7.
  Rejected alternative: a version-dispatch registry over the retained schemas —
  strictly more mechanism for a case the refusal path already handles safely.

- Decision: add `boot_refused` rather than relaxing `boot`'s fatal check.
  Rationale: `boot` treating any `SLIME_ROOT FATAL` as failure is load-bearing for
  every other arm. Weakening it to accommodate one arm would blind the rest.

- Decision: stamp both the magic and the version word.
  Rationale: measured — neutralizing only the magic still produced a refusal, so
  the version word alone is sufficient. Stamping both makes the fixture a
  coherent "v4 generation" rather than a blob that happens to trip one check.

## Open risks and follow-ups

- [ ] The arm proves refusal for a *pending* candidate. A **known-good** slot in a
  superseded format is not covered: there is no fallback behind it, so the
  observable would be an unbootable disk rather than a rollback. That is arguably
  correct behaviour and arguably a recovery-plane concern (`sel4_recovery_plane_check`),
  but it is untested either way.
- [ ] **[INFERENCE]** No shipped generation has ever been in a superseded format on
  a real disk, so the rollback-by-refusal path has now been exercised
  synthetically but never by an actual version bump. The next `formatVersion` bump
  is the first real test.
- [ ] The retained v2/v3/v4 schemas are type-checked but nothing asserts they still
  *describe* the formats their magics name — no fixture is decoded through them.
  Type-checking catches rot, not drift.

## Artifacts and provenance

- Focused report: none; the audit that opened B64 is
  [the structural audit entry](../2026-08-17-structural-audit/index.md), whose B64
  claim is corrected in this entry and in the backlog's resolved log.
- Raw transcript: none preserved; the refusal marker and the mutation's failure
  line are quoted above and reproducible with `just sel4_boot_selection_check`.
- Serial/debugger/model output: quoted inline (`SLIME_ROOT FATAL boot selection
  rejected: Generation`, and the mutated run's `SLIME_GRAPH FAIL required instance
  init exit status=1`).
- Related roadmap item: [`roadmap/00-backlog.md`](../../roadmap/00-backlog.md) —
  B64 in the resolved log; B61, B62, B63, B65 open.
