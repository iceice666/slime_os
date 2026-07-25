# C7.5 shared-buffer loan/return lifecycle and fault reclamation

| Field | Value |
|---|---|
| Date | 2026-07-25 |
| Status | Verified |
| Scope | `kernel/src/memory/shared_buffer.rs`, `kernel/src/capability/mod.rs`, `kernel/src/syscall/mod.rs`, `boot-contracts/src/generation.rs`; `just shared_buffer_loan_check` |
| Trigger | Roadmap C7 decomposition; C7.5 adds loan/return ownership and fault reclamation on top of C7.4 sealed mappings |
| Baseline | C7.4 shared-buffer table: bounded factory allocation charged to a supervision-subtree owner with per-holder `byte_pages`/`buffer_count`/`mapping_count` quotas and irreversible read-only sealing; `loan_count` declared but unconsumed, no loan object, no cross-holder receiver authority |

## Summary

C7.5 turns the declared-but-unconsumed `loan_count` quota into a real
loan/return lifecycle. A new object-specific `RIGHT_BUFFER_LOAN` (bit 25) on a
`SharedBuffer` gates `SYS_SHARED_BUFFER_LOAN`, which mints a receiver-bound
`SharedBufferLoan` kernel object over one exact, irreversibly sealed, page-aligned
subrange. The loan names its receiver through a `RIGHT_SUPERVISE` capability
rather than an ambient task id, charges one unit against the lender's `loan_count`
quota under a fixed `MAX_LOANS`=64 ceiling, and carries a kernel-assigned,
unforgeable, single-return identity. `release_by` retains the creator's pages and
buffer charge while any loan is outstanding; the last settle finalizes the
region and frees its frames. `SYS_SHARED_BUFFER_LOAN_MAP` confines the receiver
to the loaned subrange and is always read-only; `SYS_SHARED_BUFFER_RETURN`
(receiver) and `SYS_SHARED_BUFFER_REVOKE` (lender) settle exactly one loan, and
duplicate, stale, and wrong-buffer returns fail closed without changing
accounting. `reclaim_owner` — the one path every termination funnels through —
settles every loan naming a dying task as lender or receiver, so peer death,
supervised restart, and explicit revocation deterministically restore the loan
and every resource counter to their pre-loan values. Status: verified under
`just shared_buffer_loan_check` (7 QEMU cases) and the full gate stack.

## Observable symptom

- Command: `just shared_buffer_loan_check`
- Expected: 7 QEMU cases pass; loans require sealed exact ranges; release retains
  pages until single return; stale/duplicate/wrong-buffer returns fail closed;
  receiver maps stay read-only and in range; peer death settles only the affected
  loans.
- Observed: all pass (see Verification).

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `Mapping` needed a `loan_id` tag so loan-scoped and holder-scoped mappings tear down independently on release-with-loans vs. settle | Added `Option<u64> loan_id` to `Mapping`; `#[derive(Clone, Copy)]` restored for slot-scan predicates |
| 2 | A creator that releases while a loan is outstanding must keep pages retained | Split release into `release_by(owner, region)`; `Entry` gains `released: bool`; final settle calls `finalize_region` |
| 3 | Loans must survive lock order `SCHEDULER -> SHARED_BUFFER_TABLE` | Reused the C7.3/C7.4 pattern: `task::terminate` calls `reclaim_owner(id)` once, settling loans before mappings before regions |
| 4 | Reviewer panel: a loan cap that the lender revokes/dies out from under leaves the receiver's slot orphaned (return hits `NotFound`, slot never dropped) | `sys_shared_buffer_return` drops the receiver's dead loan cap slot on `NotFound` |
| 5 | Reviewer panel: `SYS_SHARED_BUFFER_LOAN` docs said "transferable" but the loan is receiver-bound by design (authority restricted to the named receiver) | Corrected the doc; `RIGHT_TRANSFER` is retained only for the lender→receiver delivery |
| 6 | Reviewer panel: `SYS_SHARED_BUFFER_RETURN` omitted the `RIGHT_BUFFER_MAP` gate its siblings enforce | Added the gate for defense-in-depth against a future narrowing path |
| 7 | Reviewer panel: a loan minted for an already-dead receiver would never be reclaimed | Added a `task::is_live(receiver)` guard before minting; uniprocessor syscall cannot switch between check and locked insert |

## Root cause

Not a defect fix; C7.5 is new capability. The violated-until-now invariant is
that the `loan_count` quota was declared and bounded (C7.3) but no operation
consumed it, and no cross-holder authority existed to hand a bounded read-only
view of a sealed buffer to another component with a return obligation.

## Changes

| Area | Change | Restored/established invariant |
|---|---|---|
| `kernel/src/capability/mod.rs` | `RIGHT_BUFFER_LOAN` (bit 25); `SharedBufferLoan(BufferLoan)` object; `valid_rights`/`RIGHT_ALL` wiring; `BufferLoan` handle (id + region) | Object-specific loan authority; unforgeable single-return identity; loan cap never carries write authority |
| `kernel/src/memory/shared_buffer.rs` | `MAX_LOANS`=64; `Loan` records; `NotSealed`/`LoansExhausted` errors; `loan`/`map_loan`/`return_loan`/`revoke_loan`; `release_by` with retained-`released` regions; per-owner `loans` charge; `reclaim_owner` settles loans first | Retained-while-loaned pages; exact read-only receiver range; single-return; deterministic fault reclamation |
| `kernel/src/syscall/mod.rs` | Syscalls 26–29 with `RIGHT_BUFFER_LOAN`/`RIGHT_SUPERVISE`/`RIGHT_BUFFER_MAP` gates, receiver liveness guard, orphaned-slot reclamation on `NotFound` | Capability-gated loan/map/return/revoke; no ambient receiver; no stranded receiver slot |
| `boot-contracts/src/generation.rs`, `scripts/check/check-generation.py` | `RIGHT_ALL` widened to bit 26; `RIGHT_ALL_V2` retains the 24-bit v2 mask; v2-rejects-v3-rights test | Manifests may grant `bufferLoan`; retained v2 generations still reject v3-only bits |
| `scripts/build/build-generation.py` | `"bufferLoan": 1 << 25` | Manifest grants the loan right 1:1 to the bit name |
| `scripts/check/check-no-storage-authority.py` | Allowlist adds four syscalls, the `SharedBufferLoan` object, and `RIGHT_BUFFER_LOAN` | Framework safety allowlist matches the real surface |
| `docs/capability-matrix.md` | `BUFFER_LOAN` and `SharedBufferLoan` rows; free-bits line to 26–63; `MAX_LOANS` bound; loan reclamation semantics | Matrix tracks the object/rights/bounds surface in the same change |
| `kernel/tests/shared_buffer_loan.rs`, `Justfile` | `just shared_buffer_loan_check` (7 cases) | Independently reviewable C7.5 gate |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Loan of unsealed or malformed range accepted | `just shared_buffer_loan_check` | `loans_require_a_sealed_exact_region` fails |
| Creator reclaims pages while a loan is outstanding | `just shared_buffer_loan_check` | `release_retains_pages_until_single_return` fails |
| Duplicate/stale/wrong-buffer return corrupts accounting | `just shared_buffer_loan_check` | `stale_duplicate_and_wrong_buffer_returns_are_side_effect_free` fails |
| Receiver map escapes range or gains write | `just shared_buffer_loan_check` | `receiver_mapping_cannot_escape_or_gain_write_access` fails |
| Peer death does not settle / disturbs unrelated owner | `just shared_buffer_loan_check` | `receiver_death_settles_only_its_loan` / `lender_death_revokes_receiver_and_preserves_unrelated_owner` fail |
| v2 generation accepts v3-only rights | `just contracts_check` | `retained_v2_rejects_v3_only_rights` fails |
| Syscall/rights/object surface drift | `just framework_safety_check` | allowlist mismatch |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just shared_buffer_loan_check` | pass (7/7 QEMU cases) | Direct |
| `just shared_buffer_mapping_check` | pass (8/8; C7.4 unaffected) | Direct |
| `just shared_buffer_accounting_check` | pass (7/7; C7.3 unaffected) | Direct |
| `just shared_buffer_factory_check` | pass (8/8; C7.2 unaffected) | Direct |
| `just test` (QEMU) | pass (151 test assertions across all suites) | Direct |
| `just contracts_check` | pass (incl. 19 boot-contracts lib tests) | Direct |
| `just generation_check` | pass (byte-identical two builds) | Direct |
| `just fmt_check` / `just lint` | clean | Direct |
| `just fmt_check_components` / `just lint_components` | clean | Direct |
| `just framework_safety_check` | pass | Direct |

## Decisions

- Decision: The loan is bound to the named receiver task, not to whoever holds
  the `SharedBufferLoan` capability slot.
- Rationale: "Receiver authority restricted to the loaned region" (roadmap C7.5)
  is a security property; a chained onward transfer must not let a third task map
  or return the loan. `RIGHT_TRANSFER` is retained only so the lender can deliver
  the freshly minted cap to the receiver over IPC.
- Rejected alternative: rebinding the receiver on transfer (holder-bound loans)
  — it would let any transferee exercise the loan, defeating the receiver
  restriction.

- Decision: Buffer/loan reclamation stays bound to the creating supervision-subtree
  owner (`Entry::owner` / `Loan::lender`), not to current capability possession.
- Rationale: This preserves the C7.2/C7.3 per-subtree accounting model unchanged;
  whether accounting ownership should travel with a transferred `SharedBuffer`
  capability is the open horizon question in `docs/directions/25-resource-accounts.md`,
  out of scope for C7.5.
- Rejected alternative: current-holder-bound release/revoke — would silently move
  charges across subtrees and reopen the wrong-holder release path C7.5 closed.

## Open risks and follow-ups

- [ ] C7.6 defines the versioned sample-descriptor contract that references a
  transferred loan identity, offset, length, and type over the C7.4/C7.5
  lifecycle; the loan mechanism is exercised directly by the gate until then.
- [ ] Accounting-ownership transfer semantics for a handed-off `SharedBuffer`
  capability remain a C7 horizon question (`docs/directions/25-resource-accounts.md`).

## Artifacts and provenance

- Reviewer verdicts: `history://CanonicalLoanReview`, `history://CorrectnessLoanReview`,
  `history://SecurityLoanReview`, `history://LifecycleLoanReview`,
  `history://ConventionLoanReview` (panel; findings applied or recorded not-applicable).
- Kernel gate: `kernel/tests/shared_buffer_loan.rs`; `just shared_buffer_loan_check`.
- Capability surface: `docs/capability-matrix.md`.
- Related roadmap item: `roadmap/02-core-runtime.md` C7.5.
