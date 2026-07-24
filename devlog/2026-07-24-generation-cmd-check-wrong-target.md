# generation_cmd_check corrupted the wrong generation

| Field | Value |
|---|---|
| Date | 2026-07-24 |
| Status | Verified |
| Scope | `scripts/check-generation-commands.py`, generation staging service, `just generation_cmd_check` |
| Trigger | Component image changes shifting the bootstore directory's identity sort order |
| Baseline | `just generation_cmd_check` previously passed `success`, `bad-closure`, and `bad-release` |

## Summary

`just generation_cmd_check` failed on both negative scenarios (`bad-closure`,
`bad-release`). The backlog's stated cause (init's `spawn_and_wait` aborting on a
rejecting `Exit(1)`) was wrong: `generation-stage` already classifies a `-4`/`-3`
rejection internally and exits `0`, and init already exits cleanly after the
staged rejection. The real defect was in the test fixture builder: it corrupted
a generation by fixed directory index (`entries[1]`), but staging validates the
*candidate* generation (identity != known-good). Because the bootstore directory
is identity-sorted, a component-image change flipped the sort order so the
corruption landed on the untouched known-good generation. Staging then
*succeeded* (`status=0`), `generation-stage` hit its non-`-4`/`-3` `fail()` path,
and the boot exited `Failed`.

## Observable symptom

- Command: `just generation_cmd_check`
- Expected: `[generation-stage] rejected status=-4` / `-3`, then `generation command check: ok`
- Observed: `[generation-command] failed`, `[generation] generation-stage terminated: Some(Exit(1))`, `kernel exit: Failed (0x11)`
- Exit/fault/serial evidence: with instrumentation, `[generation-stage] unexpected status=0` on `bad-closure`

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | Baseline (clean tree) failed `bad-closure`; a stale-cache run showed `status=-4` | Behavior was layout-dependent, not a fixed init/stage bug |
| 2 | `command::fail()` printed with no preceding `rejected status=` line | Staging returned a status other than `-4`/`-3` before the classification print |
| 3 | Instrumented `generation-stage`: `unexpected status=0` | Staging *succeeded* on the "corrupted" fixture |
| 4 | Probed fixture: flipped byte at `gen_off+gen_len-1` fell inside object 20's payload of the entry whose identity == BootState known-good | Corruption hit the generation staging never reads |
| 5 | Confirmed candidate (identity != known-good) sorted to `entries[0]`, not `entries[1]` | Fixed directory index targeted the wrong generation |

## Root cause

`build_fixture` in `scripts/check-generation-commands.py` corrupted `entries[1]`
by fixed directory index. The bootstore directory is sorted by generation
identity, and staging (`SLIME_KNOWN_GOOD_FIRST=1`) targets the candidate
generation, i.e. the entry whose identity differs from the BootState known-good
baseline. The fixed index silently decoupled "corrupted entry" from "staged
entry" as soon as component images changed the identity ordering. The violated
invariant: the fixture must corrupt the generation that staging actually
validates.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `scripts/check-generation-commands.py` | Select the candidate entry by `identity != known_good` (read from BootState) instead of `entries[1]`; add `bootstate_known_good` helper | The corruption always lands on the generation staging validates, independent of directory sort order |
| `roadmap/00-backlog.md` | Move B1 to Resolved with corrected root cause | Backlog reflects the real defect and observed exit condition |
| `roadmap/README.md` | Backlog status -> `1 open (deferred)` | Index tracks remaining open items |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Fixture corrupts the wrong generation again | `just generation_cmd_check` | Absence of `[generation-stage] rejected status=-4`/`-3`; `[generation-command] failed` |
| Identity sort order changes silently | `build_fixture` raises if not exactly one candidate | `SystemExit: expected exactly one candidate generation` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just generation_cmd_check` | ok: `success` -> `staged release=3`, `bad-closure` -> `rejected status=-4`, `bad-release` -> `rejected status=-3` | Direct |
| Rejected staging leaves BootState unchanged | Checker's `before[:SLOT*2] == after[:SLOT*2]` assertion passed for both negative scenarios | Direct |

## Decisions

- Decision: Select the staged/corrupted entry by identity, not directory index.
- Rationale: Directory order is identity-sorted and shifts with any component
  image change; identity is the stable key staging keys on.
- Rejected alternative: Patch init/generation-stage per the original backlog
  diagnosis — both were already correct; that would have masked the fixture bug.

## Open risks and follow-ups

- [ ] None. B2 (`Blocked` task state) remains deferred and is unrelated.

## Artifacts and provenance

- Focused report: this entry
- Raw transcript: n/a (interactive investigation)
- Serial/debugger/model output: `just generation_cmd_check` serial log (transient)
- Related roadmap item: `roadmap/00-backlog.md` B1 (Resolved)
