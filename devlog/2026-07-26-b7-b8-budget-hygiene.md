# B7/B8 — manifest rights vocabulary and budget aggregate bounds

| Field | Value |
|---|---|
| Date | 2026-07-26 |
| Status | Verified |
| Scope | `scripts/build/build-generation.py` rights table; `boot-contracts/src/shared_buffer_budget.rs` validation; `kernel/src/runtime/generation.rs` caller |
| Trigger | Backlog B7 and B8, opened by the 2026-07-26 C7 audit (`devlog/2026-07-26-c7-audit/`) |
| Baseline | The manifest right for bit 9 was still spelled `map`; budget validation bounded each holder but never the aggregate |

## Summary

The last two C7 audit findings, closed together because both are hygiene on
already-working code. **B7:** C7.1 renamed the kernel constant `RIGHT_MAP` to the
object-specific `RIGHT_BUFFER_MAP`, but the host-facing manifest key stayed
`map`, so generation authors kept writing a generic name for buffer-specific
authority — a one-line rename with no wire change, since the bit value is
unchanged and no manifest referenced the key. **B8:** `validate_against` checked
each holder against the fixed kernel ceilings but never summed them, so a budget
could promise N holders `MAX_TOTAL_PAGES` each. That was over-commitment rather
than per-holder impossibility, and `SharedBufferTable::create` still enforced the
real ceiling — but the roadmap promised decode rejects "globally impossible"
limits, and first-come-first-served allocation is not what a declared quota
should mean. Chose the stricter reading: a budget that validates is now one the
kernel can honour in full, with every declared holder at its ceiling
simultaneously. The check also gained the two per-holder bounds it was missing
(`mapping_count` and `loan_count` against `MAX_MAPPINGS`/`MAX_LOANS`).

## Observable symptom

Neither was a failure; both were claims wider or looser than the code.

- **B7** — Command: `grep -rn '"map"' --include=*.py .` → one hit,
  `scripts/build/build-generation.py:112`, sitting among object-specific
  siblings `bufferWrite`, `bufferCreate`, `bufferLoan`.
- **B8** — `boot-contracts/src/shared_buffer_budget.rs:116-148` looped per entry
  with no accumulator; its own comment noted `max_buffer_pages` was retained
  only "for symmetry". Lib tests covered per-holder impossibility only.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | The `map` key had no users: no manifest fixture referenced it, and the bit value (1 << 9) is unchanged by a rename | B7 is a pure vocabulary fix with no wire or identity impact |
| 2 | B8's two options — sum-and-reject vs. reword the roadmap — are a genuine design call, not a defect fix | Chose summing: `AGENTS.md` requires generation data "deterministic, bounded, versioned, and explicitly validated", and a budget that cannot be honoured in full is none of those |
| 3 | `mapping_count` and `loan_count` were bounded per holder only against that holder's own `byte_pages`/`buffer_count`, never against `MAX_MAPPINGS`/`MAX_LOANS` | A holder could declare 200 mappings against a 64-entry table. Added those two per-holder bounds while touching the same function |
| 4 | Summing needs the two new ceilings, so `validate_against` grew from three parameters to five | Updated the kernel caller and the existing lib tests; introduced a `check()` test helper so the ceilings appear once |
| 5 | First fault injection (dango 8 → 200 pages) still passed, correctly: 200+4+4+2 = 210 ≤ 256 | My injection was too weak, not the check too lax — worth noting, since a careless read would have called this a miss |
| 6 | Second injection (also spawn-service 4 → 100, total 306 > 256) failed the boot outright | The rule bites end to end: the kernel refuses to decode an over-committed generation, so the boot fails closed rather than allocating first-come-first-served |

## Root cause

Neither is a code defect.

B7 was an incomplete rename: C7.1 changed the kernel-side constant and the
capability matrix but not the builder's manifest vocabulary, leaving the
host-facing name a generic `map` for what is now buffer-specific authority.

B8 was a scope mismatch between the roadmap deliverable ("reject … globally
impossible limits") and the implementation (per-holder impossibility only). The
implementation was safe — the kernel's own ceilings still bound allocation — but
it deferred the failure from decode to runtime, and only for whichever component
started last.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `scripts/build/build-generation.py` | `"map"` → `"bufferMap"` in the `RIGHT` table | The manifest vocabulary matches the object-specific kernel right |
| `boot-contracts/src/shared_buffer_budget.rs` | `validate_against` sums `byte_pages`, `buffer_count`, `mapping_count`, and `loan_count` with saturating adds and rejects a total past any kernel ceiling | A validating budget is one the kernel can honour in full |
| `boot-contracts/src/shared_buffer_budget.rs` | Per-holder `mapping_count`/`loan_count` are now bounded by `MAX_MAPPINGS`/`MAX_LOANS` | A holder cannot declare more mappings or loans than the fixed tables hold |
| `kernel/src/runtime/generation.rs` | Passes `MAX_MAPPINGS` and `MAX_LOANS` to the widened validator | Decode enforces the full rule |
| `boot-contracts/src/shared_buffer_budget.rs` (tests) | `check()` helper plus three new cases: aggregate pages, aggregate buffers/mappings/loans, and per-holder mapping/loan ceilings | Each new rule has a case that fails without it |

Saturating adds are deliberate: a budget whose totals overflow `u32` is
over-committed by construction, and saturating keeps the comparison honest
instead of wrapping into a passing value.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A budget over-commits pages in aggregate | `aggregate_over_commitment_is_rejected` | 5 holders x 64 pages (320 > 256) is accepted |
| A budget over-commits buffers, mappings, or loans | `aggregate_buffer_mapping_and_loan_ceilings_are_enforced` | Any of the three totals is accepted past its ceiling |
| A single holder exceeds the fixed mapping or loan table | `per_holder_mapping_and_loan_ceilings_are_enforced` | 100 mappings or 100 loans is accepted |
| The rule stops applying on the live path | Fault injection, observed: raising the manifest to 306 aggregate pages made the boot fail closed | An over-committed generation boots to a healthy slice |
| The generic `map` key returns | `just generation_check` builds the manifest against the renamed table | Unknown-right failure at build time |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `cargo test -p boot-contracts --lib` | 24 passed (21 prior + 3 new) | Direct |
| Fault injection: manifest aggregate raised to 306 pages (> 256) | Boot failed closed; restored, and the real budget (18/256 pages, 5/32 buffers, 10/64 mappings, 5/64 loans) passes | Direct |
| `just generation_check` | pass, two byte-identical builds | Direct |
| `just contracts_check` | pass | Direct |
| `just spawn_service_check` | pass — `vertical slice healthy`, both quota probes live | Direct |
| `just sample_plane_live_check` | pass | Direct |
| `just test` | pass, full kernel suite | Direct |
| `just framework_safety_check`, `just fmt_check`, `just lint` | clean | Direct |

## Decisions

- Decision (B8): sum across holders and reject an over-committed budget, rather
  than rewording the roadmap to promise less.
- Rationale: `AGENTS.md` requires generation data to be "deterministic, bounded,
  versioned, and explicitly validated". A budget that cannot be honoured in full
  is not bounded in any useful sense — it degrades a declared quota into a race,
  where a late-starting component fails with `BytesExhausted` despite holding a
  quota the generation promised it. Failing at decode is deterministic and
  explicit; failing at runtime is neither.
- Rejected alternative: keep over-commitment legal and narrow the roadmap
  wording. Smaller change, and it preserves deliberate over-subscription — but
  nothing in the system wants over-subscription today, and the looser rule is
  the one that produces confusing runtime failures.

- Decision: also add the missing per-holder `mapping_count`/`loan_count` bounds.
- Rationale: found while adding the aggregate sums — a holder could declare 200
  mappings against a 64-entry table and validate. Same function, same class of
  bug, and leaving it for a future item would be artificial.
- Rejected alternative: file it separately — it is one line of the same check.

- Decision: introduce a `check()` test helper rather than repeating five
  ceilings at each call site.
- Rationale: the widened signature would otherwise appear a dozen times in the
  tests, and the ceilings are a property of the kernel, not of each case.

## Open risks and follow-ups

- [ ] The host builder does not validate the aggregate; only the kernel does at
  decode. An over-committed manifest builds successfully and fails at boot. That
  is fail-closed and matches how other generation invariants are enforced, but a
  builder-side check would move the error earlier. Not done here to keep one
  source of truth for the rule.
- [ ] Current headroom is large (18/256 pages, 5/32 buffers, 10/64 mappings,
  5/64 loans), so the aggregate rule is not yet binding in practice. C8 should
  revisit the quota values when the sample plane carries real traffic.
- [ ] With B7 and B8 closed, the C7 audit backlog is empty.

## Artifacts and provenance

- Focused report: this entry.
- Raw evidence for the original findings: `devlog/2026-07-26-c7-audit/transcript.txt` §8–§9.
- Related roadmap items: `roadmap/00-backlog.md` B7 and B8 (resolved by this
  entry); `roadmap/02-core-runtime.md` C7.1 and C7.3.
- Related prior entries: `devlog/2026-07-26-c7-audit/` (opened both);
  `devlog/2026-07-26-b4-live-shared-buffer-budget/` (declared the budget these
  rules now bound).
