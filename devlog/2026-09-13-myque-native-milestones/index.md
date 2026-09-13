# Enable native GitHub milestone projection

| Field | Value |
|---|---|
| Date | 2026-09-13 |
| Kind | Change |
| Status | Proposed |
| Scope | `.github/workflows/myque-project.yml`, `CONTRIBUTING.md`, native milestone projection |
| Work items | 01a09a60-86bf-7646-b782-e23fc7230249 |
| Gates | `just tasks_check`, `just devlog_check`, `just typos` |
| Trigger | User-requested upgrade to the native-milestone projector |
| Baseline | The reusable workflow and CLI were pinned to `c1da385362d1eca3dc1bdf52b4c499e68d3007a4`, without native milestone projection |

## Summary

Upgrade both projector pins to `d4bf2aca50d3c762fa3299269b4c83ce4b0ea4fd`.
The upgraded CLI's read-only plan succeeds against committed source
`2f19911fb681089967d6b9a4747dfb00a7fac374`: 51 milestone creates, 65 Issue
body updates, and three membership updates, without warnings or conflicts.
No remote mutation was performed. Production activation requires the workflow
change to merge to `main`; local validation is not evidence of hosted apply.

## Changes

| Area | Change | Preserved invariant |
|---|---|---|
| Workflow | Update reusable-workflow and CLI pins together | Query, triggers, trusted author, canonical branch, token, and permissions remain unchanged |
| Contributor guidance | Document managed milestone fields and Issue assignment, strict milestone ancestry, title conflicts, and meaningful child work | `.tasks` remains canonical; dependencies and labels do not become hierarchy |
| Scope | Reuse the landed automatic-projection integration item | No new work item, fabricated completion, reparenting, or schema field |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Invalid workflow invocation | `actionlint .github/workflows/myque-project.yml` | Syntax or reusable-workflow call diagnostics |
| Projection conflict or unintended membership | Pinned read-only `plan` against an exact committed source | Nonzero exit, warning, conflict, or unexplained operations |
| Invalid canonical state or backlog policy | `just tasks_check` | Nonzero exit |
| Invalid evidence structure or references | `just devlog_check` | Nonzero exit |
| Documentation spelling | `just typos` | Nonzero exit |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `nix build github:mozufu/myque-gh/d4bf2aca50d3c762fa3299269b4c83ce4b0ea4fd#myque-gh --no-link --print-out-paths` | Exit 0; obtained the pinned aarch64-darwin CLI | Direct |
| Pinned CLI `plan` with the unchanged production query and trusted author | Exit 0; 51 milestone creates, 65 body updates, three membership updates; no warnings or conflicts | Direct; [raw plan](plan.txt) |
| Canonical states of the 51 planned milestone creates at the exact source SHA | 48 unfinished and three `done`; the three parents are P5.4, P5, and M5 | Direct; committed item inspection |
| `nix shell nixpkgs#actionlint --command actionlint .github/workflows/myque-project.yml` | Exit 0, no diagnostics | Direct |
| `just tasks_check` | Passed: 300 items, 94 frozen backlog headings | Direct |
| `just devlog_check` | Passed: 329 entries, 329 indexed | Direct |
| `just typos` | Exit 0, no diagnostics | Direct |

The source snapshot is newer than the user's inherited `main@961d77c` report:
that report expected 64 body updates, while this run includes Issue #110 and
expects 65. The milestone and membership counts remain 51 and three. The
three assignments are Issue #45 to P5.4, #86 to P5, and #91 to M5.

No Rust, OS, or QEMU runtime tests were run: no such implementation changed.
The actual upgraded CLI was exercised without `apply`. The workflow inputs
and permissions were checked against the pinned upstream reusable workflow.

## Decisions

- The existing integration item is already on `main` and owns automatic
  canonical GitHub projection; this implementation remains within that scope.
- Keep the existing selection of all unfinished states and retention semantics.
  Do not split IO11 or change epic/milestone kinds merely to populate progress.
- Do not pre-create same-title native milestones or run local projection apply.
  The existing workflow-only main-push trigger activates the upgraded projector.
- Keep the integration item active. Neither this upgrade nor an eventual PR
  merge proves its remaining genuine canonical-close propagation condition.

## Open risks and follow-ups

- Hosted milestone creation, field repair, membership assignment, and a
  stable-input no-op remain unobserved for this upgrade. Record production
  evidence after the implementation PR merges; do not claim activation yet.
- The 48 unfinished milestone items have no member Issues in this plan. A
  milestone directory is not task decomposition or evidence of progress.
- Canonical `done` closes the three ancestor milestones even though assigned
  descendants remain unfinished; do not reopen them to change the display.
- Concurrent canonical or GitHub changes can change the plan before apply.
  Same-title unmanaged milestones must not be introduced in the meantime.

## Artifacts and provenance

- [Read-only plan transcript](plan.txt), including the exact command and source SHA.
- [Canonical integration item](../../.tasks/items/01a09a60-86bf-7646-b782-e23fc7230249.md).
- [Pinned upstream workflow](https://github.com/mozufu/myque-gh/blob/d4bf2aca50d3c762fa3299269b4c83ce4b0ea4fd/.github/workflows/reconcile.yml).
- [Earlier automatic projection change](../2026-09-13-myque-automatic-projection/index.md).
- User-supplied pre-upgrade report at `main@961d77c` is inherited context, not
  this run's evidence; its reported production run was
  [34766390781](https://github.com/iceice666/slime_os/actions/runs/34766390781).
