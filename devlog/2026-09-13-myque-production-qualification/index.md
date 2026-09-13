# MyQue GitHub production projection: ownership, repair, and merge independence

| Field | Value |
|---|---|
| Date | 2026-09-13 |
| Kind | Audit |
| Status | Monitoring |
| Scope | Production Issues #39-41, projection Actions, PR links, and canonical integration evidence |
| Work items | 01a09a60-86bf-7646-b782-e23fc7230249, 01a08ff0-0bae-741e-9318-331fdafe0b96, 01a08ff0-0b99-7315-99b0-7e7f340fa6f9 |
| Gates | `just tasks_check`, `just devlog_check` |
| Trigger | PR #38 merged; main push started the first automated projection |
| Baseline | Reviewed first-projection plan: three Issues, eight labels, and IO11 parent IO10 |

## Summary

The first production projection and both manual reconciliation dispatches
succeeded at canonical commit `ab82f20997ba0bafecdd3546e8b9f2a212daef1e`, using
projector `c1da385362d1eca3dc1bdf52b4c499e68d3007a4`. Automation created exactly
the expected bot-owned Issues, a stable-input rerun was a no-op, and deliberate
managed title/body/state drift was repaired without losing human labels or
comments. PR #38 appears merged while its canonical integration item is active
and its Issue open. Genuine canonical close propagation is still unobserved;
this audit does not complete the integration work item.

## Observable symptom

- Expected: canonical content and native parent relationships project through
  automation only; human data remains independent; PR merge does not close work.
- Observed: IO10 #39, IO11 #40, and integration #41 were created by
  `github-actions[bot]`; no other Issues were present in the initial inventory.
- Deliberate perturbation: title, body, and open/closed state of #41 were changed
  through the human account, leaving the UUID identity header intact. A normal
  `projection-smoke-human` label and human comment were added.
- Result: Actions restored the exact initial title/body and reopened #41; the
  human label and comment survived. No local projection `apply` was executed.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| Merged publication | PR #38's 21 checks succeeded; the canonical integration task is still active | Merge is not canonical completion |
| Initial Actions run 34754163817 | Three expected creates, eight labels, native IO10 -> IO11 parent | Bounded opt-in selection and parent closure hold |
| Initial API inspection | Exact canonical body suffixes/headings, UUID headers, main hyperlinks, states and managed labels match | Projection content and identity match the committed store |
| PR observation | #40 shows PR #32 open/ready, CI passing, review none; #41 shows PR #38 merged, CI passing, review none | Explicit UUID links and native facts render without inferred completion |
| Actions run 34754315062 | `No changes.`; all three Issue update timestamps unchanged | Stable-input reconcile performs no Issue mutations |
| Drift plan | Only #41 title, body and state require repair | Human label is outside the managed diff |
| Actions run 34754425257 | Exact title/body restored; state open; creator still bot; human label/comment unchanged | Managed repair and human ownership boundary hold |
| Final read-only plan | `No changes.` | Production converges after deliberate drift |

The initial ad-hoc inspection assertion mistakenly looked for the UUID on the
second header line; actual Issues correctly put `myque:id` first and projection
version second. Correcting the observation script resolved that assertion;
no projector or Issue change was needed.

## Changes

- Add this audit and immutable GitHub API/workflow transcripts as a new event;
  preserve the already-landed implementation devlog unchanged.
- Record observed acceptance progress in the canonical integration item's body,
  leaving its state active and exit conditions unchanged.
- Leave the human smoke label and comment on #41 as preservation evidence.
  Managed drift is fully repaired; no temporary implementation code was added.

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| PR #38 check rollup | 21 checks, all SUCCESS | Inherited: GitHub Actions |
| `gh run watch 34754163817 --repo iceice666/slime_os --exit-status` | Success; first automation-created Issues #39-41 | Direct |
| API title/body/marker/label/state/link assertions | All three Issues match canonical source; IO10's native sub-issues list is exactly #40 | Direct |
| `gh run watch 34754315062 --repo iceice666/slime_os --exit-status` | Success; `No changes.` and unchanged Issue update timestamps | Direct |
| `gh run watch 34754425257 --repo iceice666/slime_os --exit-status` | Success; title/body/state repaired on #41 | Direct |
| API preservation assertions | Human label and exact comment id/body survive; bot creator retained | Direct |
| Pinned `plan --ref ab82f20997ba0bafecdd3546e8b9f2a212daef1e --issue-author github-actions[bot] --project 'tag = github' --source-repo iceice666/slime_os --source-branch main` with `--store . --repo iceice666/slime_os` | `No changes.` after repair | Direct |
| `just tasks_check` | Passed: 298 items, 94 frozen backlog headings | Direct |
| `just devlog_check` | Passed: 325 entries, 325 indexed | Direct |
| Canonical close propagation | Not exercised: no legitimately completed projected item was closed in this campaign | Not yet observed |

No OS/runtime code changed. No QEMU or Rust runtime tests were run for this
documentation and production-service audit. Repository documentation/store gates
passed before publication of the evidence PR.

## Open risks and follow-ups

- The remaining integration exit condition is genuine canonical completion,
  `myque close`, a `.tasks` PR merge, and observed Issue closure. PR #32 was still
  open at this observation; IO11's QEMU exit evidence and IO10's board evidence
  remain independently authoritative. Do not fabricate a completion transition
  merely to finish the audit or close the integration item itself.
- Issue PR facts are snapshots at reconciliation. Continue manual dispatch when
  refreshing GitHub-only facts; no query expansion or new triggers were added.
- This evidence PR changes the integration item's canonical body; its merge
  should update #41's body, not close #41. Qualify that publication separately
  rather than treating these pre-merge file edits as already observed.

## Artifacts and provenance

- [Implementation decision and preview](../2026-09-13-myque-github-projection/index.md).
- [Canonical integration item](../../.tasks/items/01a09a60-86bf-7646-b782-e23fc7230249.md).
- [Merged PR #38](https://github.com/iceice666/slime_os/pull/38).
- [First automation run](https://github.com/iceice666/slime_os/actions/runs/34754163817), [raw log](initial-reconcile.log).
- [No-op run](https://github.com/iceice666/slime_os/actions/runs/34754315062), [raw log](noop-reconcile.log).
- [Repair run](https://github.com/iceice666/slime_os/actions/runs/34754425257), [raw log](repair-reconcile.log).
- [Initial Issues API snapshot](initial-issues.json), [native sub-issues snapshot](initial-subissues.json), [merged PR check rollup](merge-checks.json).
- [Deliberate drift and human-data snapshot](managed-drift.json), [read-only drift plan](drift-plan.txt).
- [Restored Issue and comments snapshot](restored-issue.json), [final read-only plan](restored-plan.txt).
- [IO10 #39](https://github.com/iceice666/slime_os/issues/39), [IO11 #40](https://github.com/iceice666/slime_os/issues/40), [integration #41](https://github.com/iceice666/slime_os/issues/41).
