# All unfinished work projected with automatic GitHub reconciliation

| Field | Value |
|---|---|
| Date | 2026-09-13 |
| Kind | Change |
| Status | Proposed |
| Scope | Projection query, GitHub event triggers, PR guidance, and canonical integration acceptance |
| Work items | 01a09a60-86bf-7646-b782-e23fc7230249 |
| Gates | `just tasks_check`, `just devlog_check` |
| Trigger | User requested all unfinished work and automatic synchronization without contributor dispatch |
| Baseline | Three opt-in Issues qualified in production; canonical pushes and manual dispatch only |

## Summary

Expand first-time projection to all unfinished canonical states and automatically
refresh GitHub-native facts. The source of truth remains `.tasks/items/`; neither
PR events nor Issue edits write canonical state. The upstream workflow and CLI
remain pinned to `c1da385362d1eca3dc1bdf52b4c499e68d3007a4`. The reviewed plan
creates 60 additional Issues, yielding 63 managed projections including three
required terminal parents. Production materialization and event delivery await
this change's normal PR merge; no local projection apply was performed.

## Changes

| Area | Change | Preserved invariant |
|---|---|---|
| Query | `state = open or state = active or state = blocked or state = deferred` | Unfinished includes deferred work without activating it |
| Main pushes | `.tasks/**` and the projector workflow path | Query-only changes publish automatically |
| PR events | `pull_request_target`: opened, edited, synchronize, reopened, closed, ready_for_review, converted_to_draft, review_requested, review_request_removed | Explicit trailer and native lifecycle changes refresh from canonical main |
| Issue events | edited, closed, reopened, labeled, unlabeled; early filter for bot-created identity-header candidates | Human drift triggers repair; unmanaged Issue activity skips the writer |
| CI completion | `workflow_run` on exact `CI`, completed, all conclusions | Both success and failure refresh, including CI for fork PRs |
| Fallback | Schedule `17,47 * * * *` | Reviews, external checks, and token-suppressed/missed events converge automatically |
| Guidance | Both templates remove contributor dispatch steps | `pr link` remains explicit; merge is never canonical completion |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Invalid event types or workflow syntax | actionlint on the changed caller | Diagnostics/nonzero exit |
| Privileged fork code execution | Fixed main checkout, fixed upstream SHA, no PR artifacts or caches | Any PR-head execution or untrusted artifact consumption |
| Event recursion | Exact CI workflow filter; upstream serialization; GITHUB_TOKEN suppression | Reconciliation triggering itself repeatedly |
| Unmanaged/fork writer invocation | Actual job `if` evaluated with GitHub's expression evaluator | Spoofed Issue, unmanaged Issue, or fork repository evaluates true |
| Scope errors | Plan create UUIDs compared to canonical unfinished set plus recursive parents | Missing unfinished UUID, unrelated terminal create, wrong parent |
| Canonical/evidence integrity | `just tasks_check`, `just devlog_check` | Nonzero result |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `myque query 'state = open or state = active or state = blocked or state = deferred'` plus canonical parent closure | 60 direct items: 11 open, 3 active, 3 blocked, 43 deferred; 63 with parents | Direct |
| Pinned CLI `plan` at `d86f0d88d069265f3df39942ee2957377029ccb7` | 60 creates, 16 label creates, 9 canonical parent operations; existing #41 title/body update; no warnings | Direct |
| Canonical create-set assertions | Every planned UUID equals unfinished-plus-parent closure minus the three trusted existing Issues | Direct |
| `nix shell nixpkgs#actionlint --command actionlint .github/workflows/myque-project.yml` | Exit 0, no diagnostics | Direct |
| Actual caller job condition using `@actions/expressions@0.3.61` | Ten cases pass: five normal events and managed Issue allowed; human spoof, unmanaged bot Issue, null body, fork repository rejected | Direct |
| Event filter assertions | Exact CI completed filter, off-hour schedule, workflow-path push, no ordinary PR/review writer | Direct |
| `just tasks_check` | Passed: 298 items, 94 frozen backlog headings | Direct |
| `just devlog_check` | Passed: 326 entries, 326 indexed | Direct |
| Expanded production apply and automatic event runs | Await merge; not claimed by local expression/plan checks | Not yet observed |

The one-off expression evaluator and its dependency were installed in an OS
temporary directory, not the dev shell or repository dependency graph. Its first
harness invocation used the README's outdated array-form Dictionary constructor;
using the installed API's variadic constructor fixed the harness. No workflow
change was required. No Rust or OS runtime behavior changed; no QEMU or Rust
runtime tests were run for this workflow/documentation change.

## Decisions

- Widen the query rather than adding `github` to every item. Existing opt-in tags
  are ordinary historical metadata now; no bulk canonical tag mutations needed.
- Required terminal parents are P5 (`01a0724c-5400-7208-b5a1-7008c0e2bdc3`),
  P5.4 (`01a0724c-5400-7d48-8f10-a5383e7c21d9`), and M5
  (`01a0724c-5400-7631-b9e8-2693dc193808`), all canonically done. They project
  closed to preserve native hierarchy, not because a query completed them.
- Privileged PR-target/workflow-run events read only canonical main and the
  fixed upstream CLI. No PR head checkout/execution or CI artifacts/caches.
  GitHub fields are data, not shell expressions or authority to close work.
- Only `iceice666/slime_os` may invoke the writer. Do not filter out fork head
  repositories or failed CI conclusions: those are legitimate observed facts.
  The Issue prefix filter reduces noise; upstream still validates creator and
  canonical UUID ownership. Do not blanket-filter App/PAT bot updates.
- `GITHUB_TOKEN` writes do not recursively trigger Issue workflows; exact
  `workflows: [CI]` excludes the projector itself. Upstream concurrency serializes
  and coalesces reconciliation; later runs rediscover the latest main.
- Do not add a privileged review relay. Scheduled reconciliation covers fork
  review events without introducing an untrusted-artifact handoff. Manual
  dispatch remains available solely for operations, not routine contributors.

## Open risks and follow-ups

- Before merge, review the 60 new Issues and three terminal parents. After merge,
  observe actual bot ownership, final count/parent graph, PR edit events, CI
  completion (including failure), managed-Issue repair, and scheduled refresh.
- GitHub schedules are best-effort and may be delayed/dropped; public repository
  schedules disable after 60 days without activity. This is automatic convergence,
  not a strict 30-minute latency guarantee.
- The pinned projector is installed by each run; more triggers increase Actions
  usage. No new cache or debounce layer is introduced without measured need.
- The previous genuine canonical-close propagation acceptance remains open.
  Broad materialization of already-done parents does not prove a live close
  transition. Keep the integration item active until all exit evidence exists.
- Narrowing the query does not unmanage trusted existing Issues. Disable the
  workflow to stop writes; do not delete identity markers or human data.

## Artifacts and provenance

- [Expanded exact-commit plan](expanded-plan.txt).
- [Actual event guard evaluation results](event-guard-check.txt).
- [Prior production qualification](../2026-09-13-myque-production-qualification/index.md).
- [Canonical integration item](../../.tasks/items/01a09a60-86bf-7646-b782-e23fc7230249.md).
- [Pinned upstream workflow](https://github.com/mozufu/myque-gh/blob/c1da385362d1eca3dc1bdf52b4c499e68d3007a4/.github/workflows/reconcile.yml).
- [GitHub workflow events](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows).
- [GitHub secure-use guidance](https://docs.github.com/en/actions/reference/security/secure-use).
- [Token event suppression](https://docs.github.com/en/actions/how-tos/writing-workflows/choosing-when-your-workflow-runs/triggering-a-workflow).
