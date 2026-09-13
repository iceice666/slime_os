# Contributor workflow separated from licensing policy

| Field | Value |
|---|---|
| Date | 2026-09-13 |
| Kind | Change |
| Status | Verified |
| Scope | CONTRIBUTING.md, README contributor route, workflow documentation only |
| Work items | 01a09aec-90c1-7aa9-b508-cadfa04fce16 |
| Gates | `just tasks_check`, `just devlog_check`, `just typos` |
| Trigger | User requested an independently mergeable documentation PR, separate from PR #107 licensing |
| Baseline | Planning PR #105 already on main; contributor guidance and licensing were bundled in PR #107 |

## Summary

Extracted the contributor workflow into a branch based directly on canonical
main, not stacked on PR #107. This change adds documentation only: it does not
introduce license grants, SPDX policy, SDK export changes, verification recipes,
or generated records. The broader work item's licensing exit conditions remain
separate and are not completed by merging this documentation.

## Changes

| Area | Change | Preserved invariant |
|---|---|---|
| `CONTRIBUTING.md` | Human intake, bot projection ownership, planning-first, machine PR linking, review and evidence-based completion | MyQue remains canonical; PR merge is not item completion |
| `README.md` | Link to the contributor workflow | README continues to describe the project |
| Documentation routing | Link existing getting-started guides, AGENTS, PR templates, and devlog | No duplication of engineering law or tutorials |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Invalid canonical references | `just tasks_check`, `just devlog_check` | Missing UUID, gate, registration, or link |
| Documentation spelling | `just typos` | Spelling finding |
| Dependency on unmerged licensing files | Local link check on a main-based checkout without LICENSE.md or LICENSES/ | Missing local target |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Branch base | `docs/contributor-workflow` created directly from fetched `origin/main`, containing planning commit `f21afb7faf31a6d279dbc65c5a4194f5d8356a56` | Direct |
| Runtime tests | Not run: this is documentation-only work | Scope |
| Documentation checks | `just tasks_check`: 299 items/94 headings; `just devlog_check`: 327 entries/indexed; `just typos`: passed | Direct |
| Local link check | 36 local targets resolved with no LICENSE.md or LICENSES/ present in this checkout | Direct |

## Decisions

- Reuse the canonical UUID already landed through PR #105. This PR implements
  only the workflow portion; no new planning item or canonical state transition
  is needed, and the broader item must not close on this subset.
- Remove the licensing section and references to unmerged license files from
  the extracted guide. Licensing and contributor-assent questions remain in
  PR #107; this document makes no new licensing or copyright-assignment claim.
- Keep SDK builders, checkers, Justfile recipes, and generated closure/run
  identities out of the documentation diff.
- Give the independently reviewable workflow change its own curated evidence
  entry, leaving the licensing investigation and SDK proof in PR #107.

## Open risks and follow-ups

- The broader canonical item still requires its separate licensing work.
  Neither this PR nor its merge provides contributor assent or completes it.
- This is contributor guidance, not new automatic enforcement of planning or
  PR-link requirements. Existing MyQue/projector semantics are unchanged.

## Artifacts and provenance

- [Contributor guide](../../CONTRIBUTING.md).
- [Canonical work item](../../.tasks/items/01a09aec-90c1-7aa9-b508-cadfa04fce16.md).
- [Merged planning PR #105](https://github.com/iceice666/slime_os/pull/105).
- [Separate licensing implementation PR #107](https://github.com/iceice666/slime_os/pull/107).
