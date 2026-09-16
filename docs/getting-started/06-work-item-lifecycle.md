# Carrying a work item

Two ways to carry work, one identity model. An **ordinary item** closes on an
observation a person records. A **spec-driven item** closes only against
evidence bound to the exact inputs that were tested, and refuses otherwise.

Both live in the same store and reach the same end state, so read the shared
model first and then the path you need.

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

4. **Close on what you observed.** Record the exit evidence — the exact
   commands, results, scope and limits — in the item, then:

   ```sh
   myque close <ITEM>
   ```

5. **Retire it.** Terminal items leave the live tree once their closure
   evidence is recorded and their exact bytes are retained:

   ```sh
   printf '{}' | myque retire <ITEM> --evidence "<what the item recorded>"
   ```

   Quote what the item states; do not invent a summary. Push the retained
   history explicitly, because it does not travel with a branch:

   ```sh
   git push origin 'refs/myque/retained/*:refs/myque/retained/*'
   ```

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
   `just` targets; requirement text never carries a command.

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
  refspec above to travel between clones.
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
