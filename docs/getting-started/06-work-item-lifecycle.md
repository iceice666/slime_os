# Carrying a work item

Two ways to carry work, one identity model. An **ordinary item** closes on an
observation a person records. A **spec-driven item** closes only against
evidence bound to the exact inputs that were tested, and refuses otherwise.

**New work takes the spec-driven path.** Every item created on or after
`2026-10-01T00:00:00Z` must carry a devloop record, and `just tasks_check`
refuses the store otherwise. The ordinary path below remains authoritative for
the items created before that instant, which stay valid and never become
subject — not even when reopened. Read the shared model first, then the path
your item's identity puts it on.

## Who decides what

Three programs own disjoint decisions, and none of them is reimplemented here.

| Owner | Decides |
|---|---|
| [MyQue](https://github.com/mozufu/myque) | Identity, state, hierarchy, dependencies, storage, retirement, recovery |
| [devloop](https://github.com/mozufu/devloop) | The requirements body, its helper semantics, evidence validity, completion eligibility |
| This repository | That a spec-driven body is one fenced `zti` block, which gates exist and what they observe, and the approval rules |

`just tasks_check` is where the third row is enforced: it delegates store
validity to `myque check`, keeps backlog-first, and hands every item that
carries a `devloop` record to devloop for validation.
[`docs/decisions/spec-driven-work-item-bodies.md`](../decisions/spec-driven-work-item-bodies.md)
records why the split sits there.

## The shared model

Canonical identity is the UUID MyQue allocates. Human keys such as `C9.4` are
display aliases: optional, renameable, and never resolved by a checker, a
dependency edge, or a generated view.

```text
myque new / devloop admit        →  .tasks/items/<UUID>.md   (open)
myque start / devloop start      →  active
close                            →  done | cancelled
myque retire                     →  .tasks/terminal/<UUID>.json
myque reopen                     →  .tasks/items/<UUID>.md   (open)
```

Retirement is a storage transition, not a state. After it, the item's identity,
relationships, readiness and `myque check` still resolve **offline** from the
terminal record; only the body is gone from the live tree, retained in a Git
commit under `refs/myque/retained/*`.

## An ordinary item

For pre-cutoff items only. New work cannot enter this way; `just tasks_check`
refuses a post-cutoff item that carries no devloop record.

1. **Propose.** MyQue allocates the id; never invent one or pick the next
   number.

   ```sh
   myque new "Describe the work" --kind task
   ```

   Use `--kind bug --tag backlog` for a defect. Fill in the problem, scope and
   observable exit conditions, then relate it if needed (`myque parent`,
   `myque depend`).

2. **Land the planning change.** A work-item-only PR merges into `main` before
   any implementation PR. `just tasks_check` enforces backlog-first while you
   are there; `just tasks_next` lists what is actionable.

3. **Start and implement.** `myque start <ITEM>`, then the implementation PR,
   which links the landed UUID with the pinned `myque-gh pr link` invocation in
   the PR template. Merging does not complete anything.

4. **Close on what you observed.** Record the exact commands, results, scope
   and limits in one unique `## Closure evidence` section, then:

   ```sh
   myque close <ITEM>
   ```

5. **Retire it through a maintenance PR.** A done item can leave the live tree
   after its evidence and exact history have been verified. Retirement never
   proves completion and is separate from closure. See the automated path below;
   ordinary `work-item/v1` and `work-item/v2` items retire directly through
   MyQue, without envelope migration.

## A spec-driven item

Same skeleton, with the requirements as data and completion enforced against
evidence. Everything below runs through `just devloop`, which supplies the
pinned Zutai toolchain; see [`.devloop/policy.json`](../../.devloop/policy.json)
for this repository's gates.

1. **Author the payload.** A `dev-spec/v1` document: problem, scope, non-goals,
   requirements with stable local ids, acceptance obligations, and predicates
   over declared observations. Write it as inert `.zti`, or as a `.zt` input
   that computes it — a shared constant, a helper that builds a predicate.

2. **Admit it.** MyQue still allocates the id; devloop renders the body and
   records its own frontmatter record.

   ```sh
   just devloop admit spec.zti --title "Describe the work" --admission <token> --policy .devloop/policy.json --kind task
   ```

   A `.zt` input is evaluated exactly once, here: imported constants are pinned
   into the stored payload, helpers are applied and disappear, and a result
   that is not first-order data — or an import that does not resolve — is
   refused. The `.zt` source is an authoring input, not a second spec. Re-running
   with the same `--admission` token returns the same item; the same token with
   a different payload is refused.

3. **Start it.** Approval runs first and refuses work whose item is not yet on
   canonical `main`, and refuses while open backlog defects exist.

   ```sh
   just devloop start <ITEM> --policy .devloop/policy.json
   ```

   Starting rotates a run epoch before the state transition, so an eligibility
   decision cannot be overtaken by a competing write.

4. **Record evidence.** A gate identity resolves through policy to existing
   `just` targets; requirement text never carries a command. `just-target` is
   the general gate: name the recipe in the execution inputs
   (`{"justTarget": "sel4_qos_check"}`) and it reports one `passed` boolean.
   devloop binds the inputs digest into the evidence identity, so evidence
   recorded for one target never transfers to another. Completion needs every
   acceptance's evidence under the same inputs, so acceptances that share one
   qualification recipe each gate it; `just-target` runs the recipe once per
   execution identity and answers the others from that run for up to an hour,
   failures included, from `build/devloop-gate-runs/`. Gate them one after
   another, and delete that directory to force a fresh run. A check needing richer
   observations gets its own gate identity in
   [`.devloop/policy.json`](../../.devloop/policy.json).

   ```sh
   just devloop gate <ITEM> <ACCEPTANCE> --policy .devloop/policy.json --inputs <inputs>.json --target <target> --image <image>
   ```

   A human obligation is recorded only from a real observation:

   ```sh
   just devloop human <ITEM> <ACCEPTANCE> --policy .devloop/policy.json --inputs <inputs>.json --target <target> --image <image> --observations <observed>.json --observer <who>
   ```

   Each entry is bound to the requirements digest, helper identity, code
   closure, policy, execution inputs, target, image and run epoch. Change any
   of them and that evidence no longer applies.

5. **Complete it.** Eligibility is checked, then the item closes under the
   revision that was checked:

   ```sh
   just devloop complete <ITEM> --policy .devloop/policy.json --inputs <inputs>.json --target <target> --image <image>
   ```

   Use `just devloop eligible …` to ask without closing.

6. **Retire it.** As for any item, plus the identity fields worth keeping
   outside history — the body profile, schemas, helper identity, requirements
   digest and admission identity — supplied as the retained consumer record.

## Retirement maintenance PRs

The separate `myque-retire.yml` workflow proposes storage-only changes from a
recorded full revision of canonical `origin/main`. It accepts no arbitrary
source ref or executable item content. Only live `done` items are candidates,
with no additional age threshold: unfinished, cancelled, and already retired
items are excluded. An empty batch succeeds without a branch or PR.

### Evidence is historical, not inferred

For an ordinary prose item, use exactly one unambiguous `## Closure evidence`
section outside code fences. Its content extends to the next heading of the
same or higher level, or the end of the body. The content is passed verbatim,
including whitespace and nested subsections, without summarizing or truncating
it. Record observed commands/results, revision and target identities, scope,
and limits; an `Exit conditions` list is a plan, not evidence. A missing, empty,
or ambiguous closure section is a reported skip. Do not backfill invented
observations to make historical items eligible.
Unfenced HTML blocks or comments are refused as ambiguous rather than treating
hidden/example headings as evidence; use plain Markdown or retire manually.

For a devloop item, the existing adapter uses the pinned consumer parser and
generated bindings. Retirement describes the recorded evidence identities and
gate results; it neither reruns gates nor applies today's evidence expiry.
Recorded `passed` values are not a new predicate or eligibility decision, and
there is no invented completion receipt. The retained consumer entry preserves
`recordSchema`, `schema`, `bodyProfile`, `helper`, `requirementsDigest`, and
`admission`; the full body, epoch, and evidence stay in exact retained history.
Missing records/evidence, unsupported versions or evidence types, and unknown
consumer namespaces are reported skips, never silently discarded.

### Preview and publication

In the pinned tool environment, preview with:

```sh
python3 scripts/lib/work_item_retirement_publish.py
```

Preview uses an isolated checkout and does not alter the source store or
remote refs, branches, or PRs. Both preview and publication run
`just tasks_check` before and after MyQue retirement: a terminal-only check cannot
validate a body after it leaves the live store. Only MyQue mutates canonical
records. The batch verifier checks exact retained bytes and the restricted
diff: each selected Markdown deletion plus its same-UUID terminal addition,
with envelope identity, timestamps, state, and graph preserved.

`--apply` is reserved for the trusted, isolated GitHub workflow; it is not a
local publication shortcut. Its remote guards require fixed canonical
`origin/main` and merge protection, and refuse unexpected remote state. GitHub
still enforces PR creation permission when publication is attempted. This
batch's retained refs must be pushed and verified **before**
its maintenance branch or PR is published. A failed history push blocks PR
publication. At most one retirement PR may be open, and an existing PR stops
a new batch instead of rewriting a branch under review. Interrupted publication
resumes only after verifying the existing bot-owned result; never overwrite
unexpected commits or delete retained history to undo a partial publication.

The report identifies the source revision, UUIDs, titles, evidence sources,
retained commits, verification, and skipped items. Normal CI and a human merge
remain required: no direct `main` write and no automatic merge.

### Qualification before enabling the schedule

Manual dispatch defaults to preview; publishing requires an explicit apply
choice. The daily schedule is guarded by repository variable
`MYQUE_RETIRE_ENABLED=true`. **Do not set it until a real manual GitHub run and
maintenance PR have qualified the workflow**, retained refs, restricted diff,
recovery, and actual CI approval path. Local simulations are not qualification;
this guide does not claim that the workflow has been tested or enabled.

Before publication, repository rules must require pull requests with no
automation bypass and require the `Canonical work-item store` status check
from GitHub Actions (integration `15368`) with strict/current-base enforcement.
This freshness boundary must prevent
merging a retirement result after its item is edited or reopened on `main`;
workflow concurrency or a one-time pre-push comparison does not suffice.
The ordinary `CI` aggregate remains required as appropriate to the repository's
merge policy. Observe the stale-base race on GitHub before enabling the schedule.
The workflow token cannot inspect private bypass-actor configuration. After
configuring the rules, an administrator with ruleset-write access runs:

```sh
python3 scripts/lib/work_item_retirement_publish.py --approve-rules
```

This read-only command requires visible, empty bypass lists and prints a digest
of the applicable ruleset IDs and update timestamps. Set repository variable
`MYQUE_RETIRE_RULES_APPROVAL` to that exact value before manual publication.
The workflow checks current public rules plus this approval; any ruleset change
invalidates it and requires fresh administrator review. This is separate from
`MYQUE_RETIRE_ENABLED`, which must remain unset until remote qualification passes.


GitHub Actions must be allowed to create pull requests (the repository setting
is “Allow GitHub Actions to create and approve pull requests”). The workflow
uses only required `GITHUB_TOKEN` contents/pull-request write permissions.
Token-created PRs do not imply unattended downstream CI: GitHub may suppress
events or require approval. Observe the real triggering/approval path and have
a maintainer approve or trigger checks through the supported GitHub path when
needed; do not substitute a personal token, privileged item execution, or a
merge bypass.

These prerequisites are operator-owned settings, not settings the workflow
changes. An administrator must establish them and approve the qualification
path before schedule enablement;
the workflow token may not have permission to inspect the administrative
setting in advance, so a successful protection preflight does not promise
that GitHub will accept PR creation.

### Retained history and recovery

Retained refs do not travel with an ordinary branch push or clone. For manual
retirement, push the relevant `refs/myque/retained/*` refs before publishing the
terminal records. A fresh clone that needs bodies must explicitly fetch them:

```sh
git fetch origin 'refs/myque/retained/*:refs/myque/retained/*'
myque reopen <ITEM>
```

MyQue verifies the retained digest before restoring the original body under
the same UUID. Missing history is a recovery error, not permission to recreate
an item or invent its evidence. Preserve refs during recovery and inspect a
partially published branch/PR before retrying; never prune history automatically.

## What the gate checks

`just tasks_check` runs offline and reports
`N items, M validated through devloop`:

1. `myque check` — schema, graph, aliases, identity.
2. Backlog-first ordering.
3. Terminal records readable as finished items; a corrupt or non-terminal
   record is a finding, never counted as done.
4. `devloop validate` for every **active** item carrying a `devloop` record.
5. Profile refusals: a duplicated block, a missing block, editable prose beside
   the block, an unknown profile, and a payload that no longer matches its
   digest.

`M` counts active spec-driven items only, so a store with none reads
`0 validated through devloop` rather than looking covered.

## Boundaries worth knowing before you hit them

- **Approval refuses unlanded work.** Land the planning change first; this is
  the same rule as for ordinary items, enforced by a program.
- **The newest entry decides an obligation.** An earlier passing run never
  rescues a later failing one.
- **Evidence expires.** Each gate declares a lifetime in policy.
- **A human obligation stays pending** until an operator supplies a real
  observation. Nothing here may synthesize one.
- **`myque close` bypasses eligibility.** It stays supported for items with no
  `devloop` record; do not use it to finish a spec-driven item.
- **Validation compiles.** devloop builds and links a native validator per
  call — seconds, not milliseconds, and no cached fast path.
- **Retirement is not deletion, and not a saving.** Removing active files does
  not shrink Git history, and `refs/myque/retained/*` needs the explicit
  fetch/push refspecs above to travel between clones.
- **Reopening verifies first.** `myque reopen <ITEM>` retrieves the retained
  bytes and checks their digest before restoring the item under the same UUID;
  missing history or a mismatch leaves it retired with an actionable failure.

## Projection

`myque-gh` projects canonical items into GitHub Issues from `main`. A
spec-driven description is rendered by devloop into readable Markdown — never
published as stored requirements data — and a renderer that fails refuses the
run before any remote change. Retired items keep resolving for relationships
and pull-request links, and say that their body is available only from retained
history.

Never use GitHub auto-close keywords against projected Issues, and never
hand-edit `myque:*` markers or the `myque-gh` PR trailer.
[`CONTRIBUTING.md`](../../CONTRIBUTING.md) owns the full PR workflow.
