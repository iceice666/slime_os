# Opt-in GitHub projection of canonical MyQue work

| Field | Value |
|---|---|
| Date | 2026-09-13 |
| Kind | Change |
| Status | Proposed |
| Scope | GitHub projection workflow, PR templates, and opt-in work items |
| Work items | 01a09a60-86bf-7646-b782-e23fc7230249, 01a08ff0-0bae-741e-9318-331fdafe0b96, 01a08ff0-0b99-7315-99b0-7e7f340fa6f9 |
| Gates | `just tasks_check`, `just devlog_check` |
| Trigger | Approved myque-gh integration handoff and source-link workflow inputs |
| Baseline | Canonical store validation exists; no GitHub-tagged projection items existed before this change |

## Summary

Add a separate, opt-in GitHub Actions projection without changing canonical
identity or completion semantics. Both the reusable workflow and installed CLI
pin `c1da385362d1eca3dc1bdf52b4c499e68d3007a4`. Local implementation is not
production qualification: first Issue creation and the ownership/lifecycle
scenarios remain pending the integration PR's merge.

## Changes

| Area | Change | Preserved invariant |
|---|---|---|
| Projection | Main `.tasks/**` pushes and manual dispatch call upstream reconciliation with `tag = github` | Only committed canonical state publishes |
| Trust | `GITHUB_TOKEN`, `github-actions[bot]`, least required permissions | First managed Issues are automation-authored |
| Links | Explicit `iceice666/slime_os` and `main` source-link inputs | Canonical links do not change the target or publication branch |
| Selection | IO11 and the new integration task carry `github`; IO10 is the sole expected parent addition | No historical or unrelated roadmap bulk materialization |
| Guidance | Both templates use the pinned `pr link` command and canonical UUIDs | Narrative references are not machine links; PR merge is not completion |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Invalid store or backlog ordering | `just tasks_check` | Nonzero result |
| Invalid evidence references | `just devlog_check` | Nonzero result |
| Wrong first projection | Exact-commit read-only plan | Unexpected creates, parents, labels, claims, or warnings |
| Ownership or lifecycle regression | Post-merge production scenarios in the canonical integration item | Drift remains, human data disappears, duplicate creation, or inferred completion |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Baseline `just tasks_check` | Passed: 297 items, 94 frozen backlog headings | Direct |
| `nix build github:mozufu/myque-gh/c1da385362d1eca3dc1bdf52b4c499e68d3007a4#myque-gh --no-link --print-out-paths` | Built on aarch64-darwin; bundled test suite: 40 examples, 0 failures | Direct |
| Changed-store `just tasks_check` | Passed: 298 items, 94 frozen backlog headings | Direct |
| `just devlog_check` | Passed: 324 entries, 324 indexed | Direct |
| `nix shell nixpkgs#actionlint --command actionlint .github/workflows/myque-project.yml` | Exit 0, no diagnostics | Direct |
| Pinned `plan` against `f0e3e198a01ede4929a08ef21ec375d201872db1` | Exit 0; exactly 3 Issue creates, 8 labels, IO11 parent IO10; no warnings or existing-Issue changes | Direct |
| Pinned `pr link 32 01a08ff0-0bae-741e-9318-331fdafe0b96 --store . --repo iceice666/slime_os --ref HEAD --apply` | Updated PR #32 myque links; subsequent read-only invocation at the preview SHA returned `No changes.` | Direct |
| Upstream CI run 34752592828 at the pinned SHA | Four platform checks passed; hosted-smoke skipped for this push run | Inherited: GitHub Actions result |
| Hosted automation apply and production ownership/lifecycle checks | Not run locally; wait for PR merge | Not yet observed |

No Rust or OS behavior changes; no QEMU or Rust runtime tests were run for this
integration. Existing SlimeOS CI remains unchanged and must pass before merge.

## Decisions

- Main is updated only by PR merge. Keep canonical validation in existing PR
  CI; do not add cross-workflow chaining or duplicate the dev shell in projection.
- First apply is Actions-only. Local `plan` reads GitHub but does not mutate it;
  local `pr link --apply` edits only a PR body, not Issue ownership.
- Two direct selections are IO11 (`01a08ff0-0bae-741e-9318-331fdafe0b96`) and
  the integration task (`01a09a60-86bf-7646-b782-e23fc7230249`). Expected closure
  adds IO10 (`01a08ff0-0b99-7315-99b0-7e7f340fa6f9`), not dependency items.
- PR #32 is the first relevant implementation link; do not bulk-link history.
- Issue title/body/state/parent and `myque:*` labels are managed. Discussion
  belongs in comments; non-managed labels remain human-owned. Preserve identity
  headers during drift tests so ownership remains discoverable.
- PR facts refresh only on reconciliation. Dispatch after linking or when fresh
  lifecycle/CI/review facts are needed. Do not add privileged PR event triggers.
- Query expansion requires a new reviewed plan. Narrowing the query or removing
  `github` does not unmanage existing trusted Issues. Disable the workflow to stop
  writes; preserve markers and human data rather than deleting projections.
- Completion requires observed canonical exit evidence and devlog provenance.
  IO11's QEMU lane and IO10's board lane remain independent; a merged PR closes
  neither item automatically.

## Open risks and follow-ups

- The canonical integration item owns post-merge qualification: bot creator,
  rendered fields/links, native parents, stable-input no-op, title/body/state
  drift repair, human comment/label preservation, explicit PR linking and facts,
  merge without automatic completion, and genuine canonical close propagation.
- Record the exact Source SHA from automation. The reusable workflow checks out
  main at execution time; refresh the preview if canonical changes merge before
  first apply. Canonical hyperlinks intentionally follow main, not an immutable
  snapshot.
- The integration PR's normal CI must pass before merge. The pinned upstream
  CI passed; its skipped hosted-smoke is not production qualification. No local
  PAT projection apply is authorized.

## Artifacts and provenance

- [Pinned upstream workflow](https://github.com/mozufu/myque-gh/blob/c1da385362d1eca3dc1bdf52b4c499e68d3007a4/.github/workflows/reconcile.yml).
- [Pinned upstream CI run](https://github.com/mozufu/myque-gh/actions/runs/34752592828).
- [Canonical integration item](../../.tasks/items/01a09a60-86bf-7646-b782-e23fc7230249.md).
- [IO11 implementation PR](https://github.com/iceice666/slime_os/pull/32).
- [First exact-commit plan transcript](first-plan.txt).
