# System image closures digested a submodule's `.git` gitlink, so every closure was stale in a git worktree

| Field | Value |
|---|---|
| Date | 2026-09-12 |
| Kind | Defect |
| Status | Verified |
| Scope | `scripts/lib/system_image_closure.py`, `scripts/generate/generate-system-image-closures.py`, `scripts/check/check-system-image-closure.py`, `contracts/system-image-closure/v2/closures/`, `contracts/system-image-closure/v2/negative/`, `contracts/system-test-run/v1/runs/`, `.tasks/items/` |
| Work items | 01a09118-1086-78e6-b8aa-f25920b8992d |
| Gates | `just system_image_closure_check`, `just system_image_builder_check`, `just system_test_run_check` |
| Trigger | `python3 scripts/generate/generate-system-image-closures.py --check` reporting all 50 closures stale from a fresh `git worktree` of `main` a1119ca6 while verifying the zutai cache lane on 2026-09-11 |
| Baseline | A closure identity depends on the bytes a clone reproduces; the same tree at the same commit resolves to the same identity in every checkout |

## Summary

Every system image closure names a kernel-loader implementation by the digest of a
`deps/rust-sel4*` submodule tree, and that digest included the submodule's `.git` gitlink
file. A gitlink's content is a relative path into the superproject's object store, which
differs between the primary checkout and every `git worktree`, so the loader identity moved
with the checkout and all 50 closures reported stale in any worktree while nothing in the
tree's own bytes had changed. Tree identities now exclude repository metadata, the
closures and the test-run records that name them were regenerated once, and the closure
gate carries a control: two trees differing only in their `.git` file share an identity.
The staleness check passes from a worktree.

## Observable symptom

- Command: `python3 scripts/generate/generate-system-image-closures.py --check` in a `git
  worktree` of `main` a1119ca6 with every submodule initialised.
- Expected: `50 system-image closures are current`, as in the primary checkout of the same
  commit.
- Observed: all 50 closures reported stale, through both the serial path and the zutai
  content cache, while every zutai evaluation's stdout was byte-identical to the primary
  checkout's.
- Exit/fault/serial evidence: the three digests per checkout in
  [`loader-tree-digests.txt`](loader-tree-digests.txt).

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | The zutai evaluations were identical between checkouts, so the drift was not in any spec's compilation | The moving input was a tree or file digest, not a compiled value |
| 2 | Every closure names its platform's kernel loader as a `tree` artifact rooted at a `deps/rust-sel4*` submodule; `identity_of` in the generator called `tree_digest` with no exclusion | The submodule's `.git` gitlink file was part of every closure's inputs |
| 3 | `deps/rust-sel4/.git` reads `gitdir: ../../.git/modules/deps/rust-sel4` in the primary checkout and `gitdir: ../../../slime_os/.git/worktrees/<name>/modules/deps/rust-sel4` in a worktree ([`loader-tree-digests.txt`](loader-tree-digests.txt)) | The same commit yields different loader digests per checkout |
| 4 | With `.git` excluded, both checkouts digest to `d6ac5a3b…`; the primary's full digest `5dab11d1…` was the committed identity | Excluding repository metadata restores one identity per commit without changing anything a clone reproduces |
| 5 | `tree_digest` already took an `exclude` argument, and the SDK record path already excludes `.git` through it | The fix is one definition of a tree input's identity, shared by the generator and the resolver |

## Root cause

A closure's tree inputs were digested as raw directory contents, so their identity captured
how the checkout was made and not only what it contained. The violated invariant is the one
the closure model exists for: a closure identity depends on the bytes a clone reproduces.
The gitlink is the only file in a submodule tree that git itself writes differently per
checkout, and it is not part of the tree's committed content.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `scripts/lib/system_image_closure.py` | `tree_identity` digests with `REPOSITORY_METADATA = (".git",)` excluded | A tree input's identity is its content in every checkout |
| `scripts/generate/generate-system-image-closures.py` | `identity_of` calls `tree_identity` instead of `tree_digest` directly | The generator and the resolver share one definition of a tree identity |
| `contracts/system-image-closure/v2/closures/`, `contracts/system-image-closure/v2/negative/` | Every closure regenerated (50 at the fix's base; 59 once rebased onto `main` fc86cc7d, whose v2 corpus holds 53 canonical and 6 negative closures); each moves only its loader identity | Committed identities equal the recomputed ones |
| `contracts/system-test-run/v1/runs/` | All 50 records re-frozen with `--bless`; 46 move one closure identity and the 4 closure-exempt runs name none | Each run names the closure identity its regenerated closure compiles to |
| `scripts/check/check-system-image-closure.py` | `check_repository_metadata`: trees differing only in `.git` share an identity, and a content edit changes it | The exclusion cannot silently widen or vanish |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A tree identity depends on the gitlink again | `just system_image_closure_check` | `a tree input's identity depends on its .git gitlink` |
| The exclusion swallows a content change | `just system_image_closure_check` | `a tree input's identity ignored a content change` |
| Closures drift from their inputs | `just system_image_builder_check` | `generate-system-image-closures.py --check` reports stale closures |
| A run names a closure that no longer compiles to that identity | `just system_test_run_check` | the run check reports drift |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `python3 scripts/generate/generate-system-image-closures.py --check` from `git worktree add --detach /space/slime_os-gitlink 5353e4aa` with submodules initialised ([`worktree-check.log`](worktree-check.log)) | `50 system-image closures are current` | Direct |
| `deps/rust-sel4` digests in both checkouts ([`loader-tree-digests.txt`](loader-tree-digests.txt)) | full digests differ (`5dab11d1…` primary, `2734af09…` worktree); both `d6ac5a3b…` with `.git` excluded, which is every regenerated closure's loader identity | Direct |
| `python3 scripts/generate/generate-system-image-closures.py --check` in the primary checkout before the change | `50 system-image closures are current` | Direct |
| `just system_image_closure_check` ([`gates.log`](gates.log)) | `system image closure check: contracts, resolution, identity, isolation, and bytes verified`, with the new `check_repository_metadata` control in the run | Direct |
| `just system_image_builder_check` ([`gates.log`](gates.log)) | `50 system-image closures are current`; builder check passed | Direct |
| `just system_test_run_check` ([`gates.log`](gates.log)) | passed on the 45 re-frozen records | Direct |
| `just system_image_closure_aggregate_check` ([`gates.log`](gates.log)) | all 50 closures exercised by an owning gate; 6 drift controls refused | Direct |
| `just ruff`, `typos` on the touched files | clean | Direct |
| Rebased onto `main` fc86cc7d (after `#30` and `#25` re-identified every closure and moved the corpus to `v2`): the generated trees reset to `main`'s, the generator's two edits re-applied onto `main`'s version, closures regenerated, runs re-blessed, then `python3 scripts/generate/generate-system-image-closures.py --check` | `59 system-image closures are current`; the diff against `main` is 105 generated files at one line each, and the `scripts/` hunks are the ones 5353e4aa carried | Direct |
| `just system_image_closure_check`, `just system_image_builder_check`, `just system_test_run_check`, `just system_image_closure_aggregate_check` on the rebased tree ([`gates-rebase.log`](gates-rebase.log)) | all pass: `53 closures resolve with distinct identities`; `46 name a closure that resolves to their own plane and 4 name none because their image is declared closure-exempt`; `all 53 closures exercised by an owning build or boot gate`; `6 named drift control(s) refused` | Direct |
| `just devlog_check`, `just tasks_check`, `just ruff`, `just typos` on the rebased tree ([`gates-rebase.log`](gates-rebase.log)) | all pass | Direct |

## Decisions

- Decision: exclude only `.git`, at the tree root or anywhere below it, and place the
  exclusion in the resolver's `tree_identity` rather than in each caller.
- Rationale: the gitlink is the one file git writes per checkout; every other byte in a
  submodule tree is committed content a clone reproduces, so nothing else should be
  excluded. One definition keeps the generator and the resolver's recomputation from
  disagreeing.
- Rejected alternative: digesting `git ls-files` output or the submodule's commit hash.
  Either would blind the identity to an uncommitted edit in the submodule, which the
  current content digest deliberately catches.

## Open risks and follow-ups

- [ ] No other tree input carries repository metadata today; a future tree input rooted at
  a nested repository is covered by the same exclusion.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [`gates.log`](gates.log), [`gates-rebase.log`](gates-rebase.log), [`worktree-check.log`](worktree-check.log).
- Serial/debugger/model output: [`loader-tree-digests.txt`](loader-tree-digests.txt).
- Related work item: `01a09118-1086-78e6-b8aa-f25920b8992d`.
