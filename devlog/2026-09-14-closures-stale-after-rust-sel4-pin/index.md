# Every closure-built plane failed after #61 moved the rust-sel4 pin

| Field | Value |
|---|---|
| Date | 2026-09-14 |
| Kind | Defect |
| Status | Fixed |
| Scope | `contracts/system-image-closure/v2/closures/*.zti`, `contracts/system-image-closure/v2/negative/*.zti` |
| Work items | 01a0a028-dc45-7a95-b25d-21e404d95983 |
| Gates | `just system_image_builder_check`, `just sel4_sample_check`, `just sel4_rollback_check` |
| Trigger | `0361bff4` (PR #61) pinned `deps/rust-sel4` at `14d98403` for the x86-64 shared-crate fixes and left the closures at `d441a880` |
| Baseline | `783cf87a`, where every closure named the same rust-sel4 commit as `sel4/pins.toml` and the rollback CI job was green |

## Summary

PR #61 bumped the rust-sel4 pin without regenerating the system-image closures, so all 60 closures still recorded the previous commit and tree identity. The closure resolver refuses a closure whose recorded pin disagrees with `sel4/pins.toml`, so every plane gate that builds through a closure failed to resolve, and CI's "Rollback runtime, release trust, and BootState trace" job went red on the PR branch and on `main`. The fix is the generator's own output: re-render from a clean tree. Nothing else moves, because the committed kernel prefix and the test-run records were unaffected by the pin.

## Observable symptom

- Command: `just sel4_rollback_check` (CI run 34808645664, job "Rollback runtime, release trust, and BootState trace")
- Expected: the rollback plane resolves its closure, builds, boots, and reports its markers.
- Observed: `seL4 rollback plane check: sel4-rollback: closure does not resolve: closure rust-sel4 commit does not match the source pin`
- Exit/fault/serial evidence: recipe `sel4_rollback_check` exit 1; the same message from `just sel4_sample_check` locally.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `git grep d441a880 -- contracts/system-image-closure` matches 54 files at `0361bff4`; `14d98403` matches none | The closures were not re-rendered with the pin bump |
| 2 | `scripts/lib/system_image_closure.py` compares `target.rustSel4Commit` to `[rust_sel4].commit` and fails on mismatch | Every closure-built plane fails before building; direct-route gates are unaffected |
| 3 | CI on `16e5ba85` and `8fee156d` (the PR branch) already failed the same job | The defect predates the merge |
| 4 | `generate-system-image-closures.py` on a clean `0361bff4` checkout changes 60 files: `rustSel4Commit` plus seven tree identities per closure; `--check` then reports all current; `generate-system-test-runs.py` changes nothing | The fix is mechanical and complete |

## Root cause

A pin bump is a closure input change. `sel4/pins.toml` and `deps/rust-sel4` moved; the closures that digest them did not, and no gate in the PR's own verification list resolves a closure. `just system_image_builder_check` (`generate-system-image-closures.py --check`) would have caught it and is not in the PR's list.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/system-image-closure/v2/{closures,negative}` | Regenerated from a clean tree at `0361bff4` | Every closure names the pinned rust-sel4 commit and the digest of the `deps/rust-sel4` tree it will build from |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A future pin bump leaves closures stale | `just system_image_builder_check` | `generate-system-image-closures.py --check` reports drift |
| A closure names a commit other than the pin | `just sel4_sample_check` (any closure-built plane) | "closure rust-sel4 commit does not match the source pin" |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `python3 scripts/generate/generate-system-image-closures.py --check` | 60 system-image closures are current | Direct |
| `just sel4_sample_check` | passed: all 17 sample-plane markers observed in order | Direct |
| `just sel4_rollback_check` | passed | Direct |
| `git diff --stat` | 60 files, 438 insertions, 438 deletions, all `identity =` and `rustSel4Commit =` lines | Direct |

## Decisions

- Decision: submit the generated output alone, without touching the generator or the pin.
- Rationale: the generator and the resolver are correct; the input changed and the output was not refreshed.
- Rejected alternative: reverting #61. It would restore green CI but discard the x86-64 lane the pin serves.

## Open risks and follow-ups

- [ ] Add `just system_image_builder_check` to the verification list of any change that touches `sel4/pins.toml` or a `deps/` gitlink; `AGENTS.md`'s verification section names `contracts_check` and `generation_check` for builder changes but not this one.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: CI run 34808645664 on `iceice666/slime_os`, job "Rollback runtime, release trust, and BootState trace".
- Serial/debugger/model output: none.
- Related work item: `01a0a028-dc45-7a95-b25d-21e404d95983`.
