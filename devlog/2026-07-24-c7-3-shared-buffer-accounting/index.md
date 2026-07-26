# C7.3 — Generation quotas and supervision-subtree accounting

| Field | Value |
|---|---|
| Date | 2026-07-24 |
| Kind | Change |
| Status | Verified |
| Scope | Shared-buffer budget contract, boot-contracts decoder, kernel per-owner accounting/reclamation, syscall enforcement, generation-decode validation, host bindings/checkers |
| Roadmap | C7.3 |
| Gates | `just shared_buffer_accounting_check`, `just shared_buffer_factory_check`, `just contracts_check` |
| Trigger | Roadmap C7 decomposition; C7.3 adds per-holder quotas and supervision-subtree accounting on top of C7.2's global ceilings |
| Baseline | C7.2 shared-buffer factory: `SharedBufferTable::create(pages, writable)` bounded only by fixed global byte/object ceilings; no per-holder quota, no owner attribution, no reclamation on peer death |

## Summary

C7.3 turns the C7.2 global-only shared-buffer bounds into generation-declared
per-holder quotas charged to the creating supervision subtree. A versioned Zutai
budget contract (`contracts/shared-buffer-budget/v1/`) is stored as a generation
`KIND_RESOURCE` object, authenticated by the generation's existing per-object
digest table (no ad-hoc header fields), and declares per-holder `byte_pages`,
`buffer_count`, `mapping_count`, and `loan_count` ceilings. A present budget is
validated deterministically at generation decode — rejecting missing, malformed,
unsorted/duplicate, zero-identity, or globally-impossible limits before any
component launches. `SharedBufferTable::create` now takes `(owner, quota, pages,
writable)`, charges each allocation to the owner's account (checked before the
global ceiling and side-effect-free on rejection), and returns
`SharedBufferError::QuotaExceeded`; a holder absent from the budget receives the
deny-by-default `HolderQuota::DENY` and cannot allocate. `reclaim_owner` returns
every unloaned page and charge on release, peer death, supervised restart, and
revocation — all of which funnel through `task::terminate` — without disturbing
another subtree's account. Mapping-count and outstanding-loan quotas are present
and bounded now for the C7.4/C7.5 operations that consume them. Status: verified
under `just shared_buffer_accounting_check` (7 QEMU cases) and the full gate
stack.

## Observable symptom

Not a regression; planned foundation work. Exit condition from the roadmap
(C7.3): two holders receive distinct generation-declared budgets; one reaches
byte or buffer-count exhaustion without affecting the other, and termination of
its supervision subtree returns every unloaned page and charge.

- Command: `just shared_buffer_accounting_check`
- Expected: 7 QEMU cases pass; per-holder exhaustion isolated; subtree teardown reclaims
- Observed: all pass (see Verification)

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | C7.2 `SharedBufferTable` tracked only a global page total and object count; no owner attribution | Add a per-owner `OwnerCharge` array (bounded by the region-slot count) keyed by the creating `TaskId`; charge/uncharge on create/release |
| 2 | The spec requires quotas be generation data via a Zutai resource object, not ad-hoc record fields | Mirror the recovery-index resource pattern: new `contracts/shared-buffer-budget/v1/` schema + renderer, generated Python/Rust bindings, hand-written decoder in `boot-contracts` |
| 3 | Peer death, supervised restart, and revocation must all reclaim; the kernel routes each through `task::terminate` | One `reclaim_owner(id)` call in `terminate` covers every path; lock order `SCHEDULER -> SHARED_BUFFER_TABLE` is the only nesting direction on this uniprocessor |
| 4 | Changing `create`'s signature broke the C7.2 `shared_buffer_authority` test (2-arg callsites) | Update those callsites to a fixed owner + effectively-unbounded quota, since C7.2's invariants (global/object/size) are orthogonal to per-holder quota |

## Root cause

Not a defect. The violated constraint was capability granularity: C7.2 bounded
shared-buffer allocation only globally, so one holder could consume the entire
kernel-wide budget and starve others, and authority to allocate was implicit for
any factory holder. C7.3 adds the generation-declared, per-supervision-subtree
quota that makes allocation authority explicit, bounded per holder, and
reclaimable.

## Changes

| Area | Change | Restored/established invariant |
|---|---|---|
| `contracts/shared-buffer-budget/v1/` (new) | Zutai budget schema + renderer; 32-byte header, 48-byte per-holder entry, `MAX_HOLDERS=32` | Zutai remains the single source of truth for the on-disk budget layout |
| `boot-contracts/src/shared_buffer_budget.rs` (new) | `SharedBufferBudget` decoder, fail-closed validation, `validate_against` global-impossibility check, `holder_identity`, 6 lib tests | Deterministic, bounded, digest-authenticated budget; malformed input rejected |
| `boot-contracts/src/generated/shared_buffer_budget.rs`, `scripts/lib/boot_contracts.py` | Generated Rust/Python bindings | Host and kernel agree on layout 1:1 |
| `kernel/src/memory/shared_buffer.rs` | `HolderQuota` (+`DENY`); per-owner `OwnerCharge`; `create(owner, quota, …)` with `QuotaExceeded`; `release`/`reclaim_owner` uncharge; owner accessors | Per-holder quota enforced before global ceiling; reclamation returns all charges without double-free |
| `kernel/src/task/mod.rs` | `Task.shared_buffer_quota` (default DENY); `set_shared_buffer_quota`; `terminate` calls `reclaim_owner(id)` | Every termination path reclaims the subtree's buffers |
| `kernel/src/syscall/mod.rs` | `SYS_SHARED_BUFFER_CREATE` reads the current task's owner id + quota; `QuotaExceeded -> ERR_OUT_OF_MEMORY` | Allocation charged to the caller's account |
| `kernel/src/runtime/generation.rs` | Validate a present budget at decode (fail-closed before launch); `shared_buffer_quota(gen, name)` lookup | Impossible/malformed budget fails the generation, not silently at runtime |
| `kernel/src/runtime/bootstrap.rs` | Apply init's and each recorded child's declared quota at launch | Quota-application path is live, not dead code; deny-by-default when absent |
| `scripts/generate/generate-boot-bindings.py`, `scripts/check/check-contracts.py` | Register the new contract and its schema/gen_rust checks | Bindings stay current; schema reflection validated in CI |
| `kernel/tests/shared_buffer_accounting.rs` (new) + `Justfile` | `just shared_buffer_accounting_check` (7 cases) | Independently reviewable C7.3 gate |
| `docs/capability-matrix.md` | Per-holder quota bounds rows + C7.3 accounting/reclamation semantics | Matrix tracks the quota surface in the same change |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Quota not enforced / cross-holder disturbance | `just shared_buffer_accounting_check` | `byte_quota_is_enforced_per_holder` / `one_holder_exhaustion_does_not_disturb_another` fail |
| Reclamation leak or double-free | `just shared_buffer_accounting_check` | `subtree_teardown_reclaims_only_its_own_charges` fails (bad total_pages or a spurious release success) |
| Malformed/impossible budget accepted | `boot-contracts` lib tests via `just contracts_check` | `impossible_quotas_rejected_against_ceilings` / `unsorted_or_duplicate_holders_fail_closed` fail |
| C7.2 global bounds regressed by the signature change | `just shared_buffer_factory_check` | any of the 8 C7.2 cases fail |
| Budget-layout / binding drift | `just contracts_check` (`--check` on boot bindings) | "generated boot bindings are stale" |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just shared_buffer_accounting_check` | pass (7/7 QEMU cases) | Direct |
| `just shared_buffer_factory_check` | pass (8/8; C7.2 unaffected by signature change) | Direct |
| `just contracts_check` (incl. 18 boot-contracts lib tests) | pass | Direct |
| `just generation_check` | pass (byte-identical two builds; kernel budget-decode path exercised) | Direct |
| `just test` (QEMU) | pass (spawn_authority, storage_capability, full slice boots) | Direct |
| `just fmt_check` / `just lint` | pass (`-D warnings`) | Direct |
| `just framework_safety_check` | pass (no new syscall/object/right surface) | Direct |
| Reviewer verdict | `correct`, confidence 0.83, no findings | Direct (`history://C73Review`) |

## Decisions

- Decision: The per-owner quota check precedes the global ceiling and pulls no
  frame on rejection.
- Rationale: A holder hitting its manifest ceiling is a `QuotaExceeded`, not a
  global-exhaustion signal another holder could observe; ordering it first keeps
  rejection scoped and side-effect-free.
- Decision: Reclamation is one `reclaim_owner` call in `task::terminate`.
- Rationale: Peer death, supervised restart, and revocation all terminate the
  owning task, so a single hook covers every path without a separate revocation
  syscall in this slice.
- Decision: Charge by creating `TaskId`, not by a persistent holder identity.
- Rationale: The supervision subtree owner is the live task; the budget's
  `holder_identity` maps a component name to its quota at launch, after which the
  running account is the task. Keeps the kernel free of name policy.
- Rejected alternative: Add quota fields to the generation component record.
  Rejected because AGENTS.md requires cross-boundary formats be versioned Zutai
  schemas; a resource object authenticated by the existing digest table is the
  in-band, no-new-header-field route.

## Open risks and follow-ups

- [ ] The `SharedBufferFactory` and a budget resource are not yet wired into the
  live generation manifest (deferred to C7.7 with the two-component
  integration), so the live boot path currently declares no budget and every
  holder is deny-by-default. The mechanism is proven by the kernel gate and
  boot-contracts lib tests. Tracked in `roadmap/02-core-runtime.md` (C7.7).
- [ ] `mapping_count` and `loan_count` quotas are declared, decoded, and
  validated but not yet consumed; the map and loan operations that charge
  against them land in C7.4 and C7.5.

## Artifacts and provenance

- Related roadmap item: `roadmap/02-core-runtime.md` (C7.3)
- Reviewer verdict: `history://C73Review` (overall_correctness: correct, no findings)
- Budget contract: `contracts/shared-buffer-budget/v1/`
- Kernel gate: `kernel/tests/shared_buffer_accounting.rs`; `just shared_buffer_accounting_check`
- Capability surface: `docs/capability-matrix.md`
