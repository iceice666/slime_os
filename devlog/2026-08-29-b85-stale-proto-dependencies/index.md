# B85: three stale `slime-proto` dependencies left `just machete` red

| Field | Value |
|---|---|
| Date | 2026-08-29 |
| Kind | Defect |
| Status | Fixed |
| Scope | `components/testkit/sel4-store-probe/Cargo.toml`, `components/testkit/sel4-rollback-probe/Cargo.toml`, `components/testkit/io-link-intruder/Cargo.toml`, `roadmap/00-backlog.md` |
| Roadmap | B85 |
| Gates | `just machete`, `just lint_all`, `just generation_check`, `just sel4_store_check`, `just sel4_rollback_check`, `just io_link_check` |
| Trigger | Observed while validating IO6; `just machete` exited 1 on a working tree whose changes did not touch any testkit crate |
| Baseline | `just machete` is meant to exit 0, reporting no unused dependencies across `boot-contracts`, `components`, and `slime-root` |

## Summary

Three testkit crates declared a dependency on `slime-proto` that none of
them used, so `just machete` exited 1. The gate exists to catch the *next*
stale dependency someone adds; while it is already red it cannot do that, and
a permanently-failing gate trains everyone to ignore it. Removing the three
declarations returns the gate to exit 0. No source referenced the crate, so
nothing else changed — the generation hash is byte-identical across the fix.

## Observable symptom

- Command: `just machete`
- Expected: exit 0, "didn't find any unused dependencies" for all three roots
- Observed: exit 1, three crates listed under "unused dependencies in components"
- Exit/fault/serial evidence:

```
cargo-machete found the following unused dependencies in components:
slime-component-sel4-store-probe -- components/testkit/sel4-store-probe/Cargo.toml:
	slime-proto
slime-component-sel4-rollback-probe -- components/testkit/sel4-rollback-probe/Cargo.toml:
	slime-proto
slime-component-io-link-intruder -- components/testkit/io-link-intruder/Cargo.toml:
	slime-proto
error: recipe `machete` failed on line 214 with exit code 1
```

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `just machete` failed during IO6 validation, on a tree that touched only `components/proto`, `verification/`, `just/`, and docs | Either IO6 caused it or it predated IO6; guessing was not acceptable |
| 2 | `git stash push -u`, re-run on clean `HEAD`: same three crates, same exit code | Pre-existing. Not attributable to IO6, and not to be fixed inside an IO6 commit |
| 3 | `grep -rn "slime_proto\|slime-proto"` across all three crate directories returned only the `Cargo.toml` lines themselves | Genuinely unused — including in each crate's `build.rs`. `[package.metadata.cargo-machete] ignored` would have been the wrong remedy, since these are real stale declarations rather than false positives |
| 4 | Filed as B85 with the evidence, deferred the fix | Kept the IO6 commits scoped to IO6 |

## Root cause

Dependency declarations outlived the code that used them. `cargo-machete`
reports a declared dependency no source references; all three crates compile
without it, so nothing forced the declarations to be removed when the last use
went away. Nothing in the build objects to an unused path dependency, which is
precisely why a dedicated gate exists.

The secondary failure is the more important one: the gate had been red long
enough to be normal. A gate whose failure carries no information is worse than
no gate, because it consumes attention and reports nothing.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `components/testkit/sel4-store-probe/Cargo.toml` | Removed `slime-proto = { path = "../../proto" }` | Declared dependencies are used dependencies |
| `components/testkit/sel4-rollback-probe/Cargo.toml` | Removed the same line | Same |
| `components/testkit/io-link-intruder/Cargo.toml` | Removed the same line | Same |
| `roadmap/00-backlog.md` | B85 collapsed into the resolved log | Backlog records how the item closed, with the narrative here |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A new stale dependency is added | `just machete` | Now green, so the next addition is visible instead of buried in three standing failures |
| A crate actually needed the dependency | `just lint_all` | Unresolved import naming the crate — a compile error, not a judgement call |
| Removal perturbed the shipped images | `just generation_check` | Generation hash mismatch or non-determinism |
| The three probes no longer boot | `just sel4_store_check`, `just sel4_rollback_check`, `just io_link_check` | Missing or reordered serial markers on the owning plane |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just machete` | exit 0 — no unused dependencies in `boot-contracts`, `components`, `slime-root` | Direct |
| `just lint_all` | exit 0 — all three crates compile without the dependency | Direct |
| `just generation_check` | exit 0, generation `197c86bc97a57a23051f8681bce453771f662c65a7129cc23e16d3ec2d2e6298` — byte-identical to the pre-change run | Direct |
| `just sel4_store_check` | exit 0 — GPT validation, object retrieval, durable commit, older-root fallback | Direct |
| `just sel4_rollback_check` | exit 0 — 19 markers, 7 durable transitions, rollback idempotent, promotion advanced | Direct |
| `just io_link_check` | exit 0 — duplex readiness, replenishment, reset, restart, authority | Direct |

Each of the three touched crates has its own QEMU plane gate, and all three
were run rather than inferred from a host compile: `cargo-machete` is a static
scan and `lint_all` only proves the crates still typecheck. The plane gates
prove they still boot and still produce their expected serial evidence.

The identical generation hash before and after is the strongest single piece of
evidence here — the shipped bytes did not move, which is what an unused
dependency removal should mean.

### A note on two transient gate failures

`just io_link_check` and `just sel4_store_check` first failed at CMake
configure after 1.98 s each. Both were run concurrently with
`just generation_check`, which owns `build/sel4-qemu`; the concurrent configure
collided with it. Re-run serially, both passed. This was build-tree contention
from how the checks were invoked, not a property of the change, and not a
defect in the gates — they are not documented as concurrency-safe, and nothing
here suggests they should be.

## Decisions

- **Decision:** Delete the three declarations rather than add them to
  `[package.metadata.cargo-machete] ignored`.
  **Rationale:** `grep` found no reference in any of the three crates, so these
  are stale declarations, not false positives. The `ignored` list is for
  dependencies a static scan cannot see — proc-macro or link-only crates — and
  using it here would silence a true report.
  **Rejected alternative:** Ignoring the gate, which is what had been happening.

- **Decision:** File B85 during IO6 and fix it separately.
  **Rationale:** Deleting dependency lines from three unrelated testkit crates
  is not IO6's intent, and folding unrelated repairs into a feature commit makes
  both harder to revert. The backlog exists for exactly this.
  **Rejected alternative:** Fixing it inline while validating IO6.

- **Decision:** Run all three plane gates, not just `lint_all`.
  **Rationale:** The claim being made is "these crates are unaffected". A host
  typecheck cannot support that for components whose purpose is to boot under
  seL4 and emit serial evidence.

## Open risks and follow-ups

- [ ] No guard prevents the *next* unused dependency from sitting unnoticed
  between manual `just machete` runs; the gate is not part of any aggregate
  target. Whether it should join one is a separate question from this fix.
- [ ] Neither `just machete` nor the seL4 plane checks are documented as safe to
  run concurrently, and the plane checks share `build/sel4-qemu`. Serializing
  them, or giving each plane its own build directory, would remove a real
  footgun. Filed here as an observation rather than a backlog item because no
  gate is currently wrong.

## Artifacts and provenance

- Backlog entry: [`roadmap/00-backlog.md`](../../roadmap/00-backlog.md), B85
- Surfaced while validating: [`devlog/2026-08-29-io6-kani-wire-proofs/`](../2026-08-29-io6-kani-wire-proofs/index.md)
- Generation hash, before and after: `197c86bc97a57a23051f8681bce453771f662c65a7129cc23e16d3ec2d2e6298`
