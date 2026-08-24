# B77: two readers admitted a CPU budget neither of them could honour

| Field | Value |
|---|---|
| Date | 2026-08-24 |
| Kind | Defect |
| Status | Verified |
| Scope | `scripts/check/check-generation.py`, `boot-contracts/src/generation.rs`, `scripts/check/check-generation-determinism.py`, `boot-contracts/examples/admit_generation.rs`, `roadmap/00-backlog.md` |
| Roadmap | B77, B48, C9, C9.3 |
| Gates | `just generation_check`, `just contracts_check`, `just test_sel4_root`, `just sel4_root_boot_check` |
| Trigger | Surfaced by the MCS survey in `797cb93` while answering what enabling MCS would cost |
| Baseline | `ScheduleRecord` has carried `budget_us`/`period_us` since the v5 wire format; the builder has always written zero, and B48 recorded why |

## Summary

`budget_us` and `period_us` are authenticated 64-bit fields in every generation
this repository builds, and until this change nothing constrained them and
nothing read them. The builder wrote zero out of habit, not invariant: the host
oracle unpacked both fields and tested neither, `Generation::validate` decoded
both and checked neither, and `slime-root` read only `Schedule.priority`. A
generation from any other producer could therefore declare a 50 ms budget,
satisfy both validators, boot, and be scheduled with no budget at all — the
exact "authenticated fiction" the v1 manifest schema comment says these fields
exist to avoid. Both readers now refuse a nonzero value with distinct reasons,
and `just generation_check` proves it with two resealed mutations. The fix is
five lines of predicate; most of the work was building a seam that could reach
the Rust decoder with chosen bytes at all, and then proving the new guards
actually bite.

## Observable symptom

- Command: `just generation_check`, then a forged generation declaring
  `budget_us = 50000` on the first schedule record with the identity hash
  recomputed.
- Expected: refusal naming the missing mechanism.
- Observed (before the fix): admitted by both readers. The host oracle returned
  a normal verdict dict and the Rust decoder printed `admitted`.
- Exit/fault/serial evidence: none needed — this is an admission defect, visible
  entirely on the host. Confirmed on the running system only in the negative
  sense that `just sel4_root_boot_check` still boots with the guards in place.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `grep` for `budget_us`/`period_us` across `scripts/`, `boot-contracts/src/`, `slime-root/src/`, `contracts/` returns the schema, generator, two builder zero-writes, one Python unpack, and the Rust field/decode | No comparison, no bound, no consumer anywhere. The zero is a builder convention |
| 2 | `check-generation.py:688` destructures both fields into local names, and the two `require` calls on the following lines mention neither | The host oracle reads the bytes and discards them |
| 3 | `boot-contracts/src/generation.rs:2200-2211` checks thread bounds, authority process, priority ordering, flags, and the thread back-reference | `validate` is structurally thorough and silent about the two time fields |
| 4 | `check_generation` verifies `generation_identity(data) == identity` at `:361`, well before the schedule loop at `:687` | A naive byte flip is refused as `BadGenerationHash` and never reaches the rule under test. The mutation must reseal, or the arm tests the hash instead — the failure mode `check-component-spec.py` documents in its own header |
| 5 | `Generation::decode` has exactly two production callers, `slime-root/src/main.rs:681` and `boot_selector.rs:185`, both reading bytes linked into the root image | No host gate could reach the Rust validator with *chosen* bytes, so its refusal rules were exercised only by whatever the builder emitted. A rule that rejects malformed input cannot be tested by a corpus that is never malformed |
| 6 | `boot-contracts/Cargo.toml` already declares `[[example]] verify_release` for exactly this kind of host-side seam | Follow that precedent rather than inventing a test binary or committing a fixture blob |

## Root cause

The fields were added to the wire record in advance of the mechanism that would
honour them, and the invariant that kept them harmless was never written down
anywhere a reader enforces. `build-generation.py` writes zero with a comment
explaining why, which is the correct behaviour for *this* builder and says
nothing about any other producer of v5 bytes — and the generation format is
explicitly a cross-producer contract, since admission is what separates a
trusted generation from an authenticated one. Both validators were written to
check structure (indices in range, back-references consistent, reserved bits
zero) and neither treats "a field whose mechanism does not exist" as a structural
property, because nothing in the format marks it as one. So the guarantee lived
only in the builder, while the thing that consumes generations is the pair of
validators. B48 recorded the MCS deferral in the kernel config and the schema
comment; what it did not do was make the resulting zero an admitted rule, and
that is the whole of this defect.

## Changes

| File | Change |
|---|---|
| `scripts/check/check-generation.py` | The schedule loop now requires `budget_us == 0 and period_us == 0`, failing as `UndeclarableCpuBudget`, with the reason and the MCS-era replacement point in a comment |
| `boot-contracts/src/generation.rs` | `validate`'s schedule loop refuses a nonzero value with `DecodeError::NonZeroReserved` — deliberately distinct from the `BadIndex` above it, because the record is structurally fine and merely claims authority the platform lacks |
| `boot-contracts/examples/admit_generation.rs` | New: decodes a generation from a path and prints `admitted` or `refused <DecodeError>`. The seam that lets a host gate drive the Rust validator with chosen bytes |
| `scripts/check/check-generation-determinism.py` | New `undeclarable_cpu_budget_refused` arm: forges each field independently on the real product generation, reseals the identity, and asserts both readers refuse with their own reason. Uses the generated offset constants rather than hand-computed offsets |
| `roadmap/00-backlog.md` | B77 collapsed into the resolved log |

The refusal is deliberately placed where real admission would go. If MCS is ever
enabled, these two predicates are what range and aggregate admission replace — a
deliberate edit at a named line, not a gap someone has to rediscover.

## Regression guards

`just generation_check` now carries the mutation arm. Two properties make it
real rather than decorative:

- **The baseline is asserted admitted first**, by both readers, so an arm cannot
  pass by tripping a guard the unmutated generation also trips.
- **The mutation is resealed**, so it reaches the schedule rule instead of dying
  on the identity hash. The reseal itself is re-verified before the assertion.

Both fields are mutated separately, because a single predicate covering both
would still pass if only one were ever checked.

## Verification

Every gate below was run at the final state of the tree.

| Gate | Result |
|---|---|
| `just generation_check` | pass — "2 resealed nonzero-CPU-budget mutations were refused as UndeclarableCpuBudget" |
| `just contracts_check` | pass — 30 declared syscall operations documented, bindings current |
| `just test_sel4_root` | pass — 152/152 across 16 modules |
| `just sel4_root_boot_check` | pass — ordered generation, timer, task, IPC, fault, and ready markers on qemu-arm-virt |
| `just fmt_check_all` | pass |
| `just lint_all` | pass; `cargo clippy -p boot-contracts --example admit_generation -- -D warnings` also clean, since `lint_all` does not cover examples |
| `just ruff` | pass |
| `just typos` | pass |

**Both guards were proven load-bearing by removing them one at a time**, which
is the only way to know the new arm can fail:

- Host guard neutralized to `require(True, ...)`, Rust guard intact →
  `generation determinism check: a generation declaring a nonzero budget_us was
  admitted by the host oracle`.
- Rust guard neutralized to `if false`, host guard restored →
  `generation determinism check: the Rust decoder answered 'admitted' for a
  nonzero budget_us, not 'refused NonZeroReserved'; the two readers disagree
  about B77`.

Both guards were then restored and the gate re-run green.

## Decisions

- Decision: refuse the nonzero value rather than start honouring it.
- Rationale: honouring it means MCS, and `797cb93` records why that is a
  per-target assurance decision rather than a config flip. Between "silently
  ignored" and "enforced" there is a third state that is strictly better than the
  first and available now: explicitly refused, with the refusal naming the
  missing mechanism. That converts a field a foreign producer could lie in into a
  field the contract states is unusable.

- Decision: distinct reasons from the two readers, not one shared name.
- Rationale: B77's exit condition asked for it, and the reason is diagnostic. The
  host oracle's `UndeclarableCpuBudget` names the policy; the decoder's
  `NonZeroReserved` names the wire property, and reuses a variant that already
  means exactly this across six other `boot-contracts` modules. Had I reused
  `BadIndex` on the Rust side, a genuine index bug and a forged budget would
  produce the same verdict.

- Decision: reach the Rust decoder through an `[[example]]`, not a committed
  fixture blob or a new test binary.
- Rationale: no generation blob is committed, by design — generations are built,
  and a committed one would rot against the format. The crate already uses the
  example seam for `verify_release`, and the gate needs to pass a path chosen at
  run time. This also closes a real gap that predates B77: until now no host gate
  ran `Generation::decode` over bytes it chose, so *every* refusal rule in that
  decoder was untested. This adds one arm; the seam makes the rest reachable.

- Decision: mutation coverage lives in `check-generation-determinism.py`.
- Rationale: `check-generation.py` is a library imported by seven other gates,
  not a gate itself. The determinism gate already builds the real product
  generation and holds an admitted blob at exactly the point a mutation needs
  one, so the arm costs no additional build.

## Open risks and follow-ups

- The guard is correct only while the kernel is built non-MCS. It is not
  conditional on the kernel config, because nothing in the generation format
  carries that config — if MCS is ever enabled, this predicate must be edited in
  the same change or every generation will be refused. That is the intended
  failure direction (loud, immediate, at a named line) but it is a coupling worth
  naming.
- `Schedule.budget_us`/`period_us` remain in the wire record and in the semantic
  `Schedule` struct. Removing them would be the stronger fix and is not
  available: the wire layout is frozen by `SCHEDULE_LEN = 48` and the generated
  bindings, and a format change is a v6 question.
- The new example makes the Rust decoder's other refusal rules reachable from a
  host gate for the first time. None of them are covered yet. That is a real
  testing gap this change only samples — it is not B77's to close, but it is now
  cheap to close.

## Artifacts and provenance

- The defect was found by the read-only MCS survey recorded in
  [`devlog/2026-08-24-mcs-cost-reassessed/`](../2026-08-24-mcs-cost-reassessed/index.md),
  not by a failing gate. Nothing was broken; something was absent.
- Raw transcript: not retained. Every claim is reproducible from the tree — the
  two guards are five lines at `scripts/check/check-generation.py:698` and
  `boot-contracts/src/generation.rs:2219`, and the neutralization experiment is
  two edits described verbatim under *Verification*.
- Related: [B48](../../roadmap/00-backlog.md#b48--all-child-execution-shares-one-fixed-priority-and-no-scheduling-authority)
  established declared priority and deferred the MCS half; this closes the
  smaller hole that deferral left behind.
- Predecessors: [`devlog/2026-08-24-mcs-cost-reassessed/`](../2026-08-24-mcs-cost-reassessed/index.md)
  (which surfaced it),
  [`devlog/2026-08-12-b48-mcs-assurance/`](../2026-08-12-b48-mcs-assurance/index.md)
  (the deferral that created the gap).

## Corrections

None.
