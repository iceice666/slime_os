# Topic

| Field | Value |
|---|---|
| Date | YYYY-MM-DD |
| Kind | Defect / Change / Audit / Decision |
| Status | Investigating / Root-caused / Fixed / Verified / Monitoring / Proposed |
| Scope | Subsystems, files, and checks touched |
| Roadmap | Milestone and backlog ids this entry bears on, comma-separated (`C7.4, B3`), or `none` |
| Gates | The narrowest `just` targets that guard this entry's claim, or `none` |
| Trigger | Commit, change, or first observed condition |
| Baseline | Last known-good behavior or invariant |

Delete the sections your **Kind** does not require; keep the remaining ones in
the order below. Required sections per kind:

| Kind | Required sections |
|---|---|
| **Defect** | all of them |
| **Change** | Summary, Changes, Regression guards, Verification, Decisions, Open risks and follow-ups, Artifacts and provenance |
| **Audit** | Summary, Observable symptom, Investigation log, Changes, Verification, Open risks and follow-ups, Artifacts and provenance |
| **Decision** | Summary, Changes, Decisions, Open risks and follow-ups, Artifacts and provenance |

## Summary

One paragraph: symptom, root cause or current hypothesis, user-visible consequence, and status.

## Observable symptom

- Command:
- Expected:
- Observed:
- Exit/fault/serial evidence:

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | | |

Keep the decisive chain. Link the raw transcript for exploratory detail.

## Root cause

Describe the source-level mechanism and violated invariant. Distinguish the root cause from secondary symptoms and innocent crash sites.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| | | |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| | `just …` | |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| | | Direct / inherited |

## Decisions

- Decision:
- Rationale:
- Rejected alternative:

## Open risks and follow-ups

- [ ] Concrete unresolved item, owner or gate if known.

## Artifacts and provenance

- Focused report:
- Raw transcript:
- Serial/debugger/model output:
- Related roadmap item:
