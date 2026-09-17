# Contributing to Slime OS

## The short version

Slime OS uses GitHub for collaboration, but canonical project state lives in
MyQue under `.tasks/items/`. Human-created Issues are intake for bugs and
proposals; bot-created Issues carrying MyQue identity markers are projections
of accepted canonical work.

**For non-trivial work, first land a dedicated work-item PR. Only after the
item exists on `main` may you open an implementation PR.** Do not combine a new
item and its implementation, or stack implementation on unmerged planning.
A work-item proposal PR is the bootstrap exception: it needs no existing item.

**PR merged != work item done.** Completion requires the item's exit conditions
to be observed and recorded, not just an implementation to land.

## Where work lives

### Human Issues are intake

Open a [GitHub Issue](https://github.com/iceice666/slime_os/issues) for a bug,
proposal, question, or discussion. For bugs, include the target, revision,
reproduction steps, expected result, and observed output when available.
Ordinary reporters do **not** need to install MyQue or edit `.tasks`.
Accepted work is later represented as a canonical MyQue item.

### Bot Issues are projections

An Issue created by `github-actions[bot]` carrying both markers is managed:

```html
<!-- myque:id=<UUID> -->
<!-- myque:projection=github/v1 -->
```

Comment, discuss, volunteer, and add ordinary human-owned labels there.
Leave **title, body, open/closed state, `myque:*` labels, parent/sub-issue
relationships, and milestone assignment** to the projector. Propose changes to
canonical state through MyQue instead; do not edit the identity markers.

### Native milestones are projections

Projected `kind: milestone` items also have a managed GitHub milestone. Leave
its **title, description, and open/closed state** to the projector; do not
pre-create same-title milestones, which cause a title conflict. Names use the
canonical body's title without the optional human key prefix.

An Issue belongs to its nearest strict `kind: milestone` ancestor in the
canonical `parent` chain. A milestone's own Issue is not a member of itself;
it can belong to an ancestor milestone. Epics do not create native milestones,
and dependencies, tags, and body checklists do not create membership. There is
no new `milestone:` front-matter field. Canonical terminal state closes the
native milestone even when descendants remain unfinished.

Create children only for independently trackable work with observable exit
conditions. Admit one with `just devloop admit spec.zti --title "Describe the
work" --kind task`, then `myque parent <CHILD_UUID> <PARENT_UUID>` and
`just tasks_check`; follow the planning-PR process below. Do not change kinds or parent relationships merely
to populate a progress bar. Browse the repository's
[Milestones page](https://github.com/iceice666/slime_os/milestones) or filter
Issues by milestone. Until real child work exists, a milestone may be empty.

### MyQue is canonical

| Surface | Owns |
|---|---|
| `.tasks/items/<UUID>.md` | Problem, scope, state, exit conditions, hierarchy, dependencies, and observed closure evidence |
| GitHub | Intake, collaboration, reviews, projected work |
| PR | Reviewable change, claim, risk, verification, limits, and related canonical work |
| Owning docs/contracts | Current architecture, behavior, procedures, and limitations |
| [History archive](docs/history.md) | Historical investigations and raw evidence; optional exceptional reusable investigations, never a product build dependency |

UUIDs are canonical identities. Optional human keys are mutable display aliases,
not persistent references. Generated lists and `roadmap/` are not state stores.

## Proposing work

Discuss bugs and ideas in Issues first when useful. **New canonical work must be
spec-driven**: every item created on or after `2026-10-01T00:00:00Z` carries a
devloop requirements body, so propose it with `just devloop admit` as described
under [Spec-driven items](#spec-driven-items) below, not with `myque new`.
`just tasks_check` refuses a store where a post-cutoff item lacks that record,
and the rule admits no exemption by kind, tag, or state — defects included.

MyQue allocates the UUID either way; never invent one or allocate a numbered key
by scanning the store. Fill in the item's problem, scope, observable exit
conditions, and use MyQue commands for any parent/dependency relationships.
Follow [AGENTS.md](AGENTS.md)'s backlog-first and work-item rules; run
`just tasks_check`.

Items created before that instant stay valid as prose and never become subject,
even if reopened. For one of those, and only for one of those, the older form
applies:

```sh
myque new "Describe the work" --kind task   # pre-cutoff items only
```

Submit a **work-item-only planning PR**, and merge it before opening an
implementation PR. An already-landed item can cover another implementation PR
within its scope; a new planning item is not required for every PR.

### Spec-driven items

An item carries its requirements as structured data, which makes its
completion enforceable rather than asserted.
[Carrying a work item](docs/getting-started/06-work-item-lifecycle.md) walks
both lifecycles command by command; the rules that bind a pull request are
here. Author the payload as a `dev-spec/v1` `.zti` file — or as a `.zt` input
that computes it — and admit it:

```sh
just devloop admit spec.zti --title "Describe the work" --admission <token> --policy .devloop/policy.json --kind task
```

[devloop](https://github.com/mozufu/devloop) allocates nothing: MyQue assigns
the UUID, the body becomes one fenced `zti` block, and the `devloop`
frontmatter record carries the profile, pins, digest, and admission identity.
Repeating the same token is idempotent. From then on `just tasks_check`
validates that body, and work proceeds by canonical UUID:

```sh
just devloop start <ITEM> --policy .devloop/policy.json
just devloop gate <ITEM> <ACCEPTANCE> --policy .devloop/policy.json --inputs <inputs>.json --target <target> --image <image>
just devloop human <ITEM> <ACCEPTANCE> --policy .devloop/policy.json --inputs <inputs>.json --target <target> --image <image> --observations <observed>.json --observer <who>
just devloop complete <ITEM> --policy .devloop/policy.json --inputs <inputs>.json --target <target> --image <image>
```

Approval applies this repository's rules before anything starts: the item must
already be on canonical `main`, and open backlog defects come first. Gate
identities resolve through [`.devloop/policy.json`](.devloop/policy.json) to
existing `just` targets; a gate reports the typed observations that policy
declares, and the item's own predicates decide whether they satisfy an
acceptance.

Most acceptances bind the general `just-target` gate, which runs one declared
recipe and reports a single `passed` boolean. Name the recipe in the execution
inputs, never in requirement text:

```sh
echo '{"justTarget": "sel4_qos_check"}' > inputs.json
just devloop gate <ITEM> <ACCEPTANCE> --policy .devloop/policy.json --inputs inputs.json --target sel4-qos --image qemu-arm-virt
```

The gate refuses a recipe `just` does not publish, and devloop binds the inputs
digest into the evidence identity, so evidence recorded for one recipe never
transfers to another. A check that deserves richer observations gets its own
gate identity in policy instead. Evidence is recorded into the `devloop` record bound to the exact
requirements, helpers, code, policy, execution inputs, target, image, and run
epoch that were tested, and the newest entry for an obligation decides it — a
later failing run is never satisfied by an earlier pass. A human obligation
stays pending until an operator supplies a real observation; nothing here may
synthesize one, and `complete` refuses while any mandatory obligation is
unmet. Merging still does not complete an item.

Ordinary prose items need none of this and are unaffected.

## Development setup

Start with [Orientation](docs/getting-started/01-orientation.md), then
[Build and run](docs/getting-started/02-build-and-run.md) for submodules,
`nix develop`, toolchain pins, and QEMU. Continue with
[Your first change](docs/getting-started/04-first-change.md) or
[Add a component](docs/getting-started/05-add-a-component.md).
These guides own the tutorials; this document owns the contribution workflow.

## Making a change

- Read [AGENTS.md](AGENTS.md), the detailed repository engineering rules.
- Route through its task-to-file index before searching. Change the owning
  implementation, not a checker that merely observes it.
- Identify the narrowest owning gate before editing; preserve capability,
  component, generation, and hardware-safety invariants.
- Use versioned Zutai schemas for cross-process, persistence, and boot formats.
  Change the canonical schema/generator and regenerate outputs together;
  never hand-edit generated bindings.
- Keep comments for current invariants beside the implementation and stable
  rationale in owning docs.
  Put long-lived cross-module choices in `docs/decisions/`, unfinished designs
  and qualification requirements in `docs/plans/`, and exploration in
  `docs/directions/`. Expensive reusable investigations or unusual verification
  campaigns belong in the private [history repository](docs/history.md).

## Pull requests

### Canonical work-item requirement

Every non-trivial implementation PR must implement at least one canonical UUID
**already present on `main` before the PR opens**. This includes features, bug
fixes, architecture changes, behavioral refactors, CI/gate semantic changes,
new verification gates, protocols/schemas, drivers, hardware bring-up,
authority/security changes, and substantial documentation or policy changes.

Use the existing [change](.github/PULL_REQUEST_TEMPLATE/change.md) or
[system-change](.github/PULL_REQUEST_TEMPLATE/system-change.md) template. State
what changed, the claim, risks/invariants, review surface, exact verification,
evidence class, target or image identity when relevant, known limits, and
canonical related work. Link existing evidence instead of duplicating it.

### Machine-linking with myque-gh

After opening the implementation PR, run **`myque-gh pr link` using the pinned
invocation in the PR template's source comment**. It takes the PR and item,
with `--store . --repo iceice666/slime_os --ref HEAD --apply`; authenticated
`gh` and permission to edit the PR body are required. Use the canonical UUID
for durable references. The human-readable `Related` section alone is not a
machine association.

The tool manages this trailer; **do not hand-edit it**:

```html
<!-- myque:pr-links=github/v1 -->
<!-- myque:implements=<UUID> -->
<!-- myque:pr-links:end -->
```

Projection refreshes automatically; workflow dispatch is not a contributor step.

### No GitHub auto-close

Do not use `Closes #123`, `Fixes #123`, or `Resolves #123` against managed MyQue
projection Issues. GitHub does not own their completion. Explicit PR-to-work
association belongs to `myque-gh`.

### Verification

Run the narrowest gate covering the changed behavior and the checks required
by [AGENTS.md](AGENTS.md). Record exact commands, observed results, evidence
class, scope, target or image identity when relevant, and limits; separate
direct observations from inherited evidence and label unobserved conclusions
as inference. Documentation-only PRs state that no runtime tests were run.
Ordinary changes do not require a devlog entry. Run `just docs_check` for
maintained documentation, policy, links, or command-reference changes. Archived
records retain their original checker at their immutable revision; no archive
checkout or network access is required for current docs checks. Run
`just tasks_check` when applicable and `just typos` for documentation.

## Review and completion

**Merge means implementation landed, not that work is complete.** Evaluate the
item's actual exit conditions: QEMU gates, cross-architecture runs, physical
hardware evidence, failure/refusal evidence, repeated reliability runs, CI,
or other item-specific observations. QEMU output does not prove hardware
support, and a green unrelated gate does not satisfy an exit condition.

For an ordinary prose item, record the observed exit evidence, scope, identity,
and limits in exactly one unambiguous `## Closure evidence` section. Retirement
extracts that section's content verbatim, through the next heading of the same
or higher level or the body's end; an `Exit conditions` list is not observed
evidence. Only then run:

```sh
myque close <ITEM>
```

A spec-driven item closes through `just devloop complete`, which verifies its
mandatory acceptance obligations against recorded evidence and then transitions
the item under the revision it checked. A direct `myque close` bypasses that
boundary; use it only for items that carry no `devloop` record.

Commit the canonical `.tasks` transition. After it reaches `main`, `myque-gh`
subsequently closes the projected Issue. Leave unfinished work unfinished.

### Storage-only retirement PRs

The separate `myque-retire.yml` workflow proposes retirement of live `done`
items from a recorded full revision of canonical `origin/main`, with no
arbitrary source refs and no additional age threshold. MyQue can retire both
`work-item/v1` and `work-item/v2` directly; this is not envelope migration,
closure, or a new completion judgment. Cancelled, unfinished, and already
retired items are not candidates. Missing/empty/ambiguous prose evidence,
unsupported consumer versions or evidence, and unknown consumer namespaces
are reported skips. Devloop retirement references recorded evidence identities
and gate results, not today's expiry or a fabricated completion receipt; its
identity fields remain in the terminal consumer entry and full evidence remains
in exact retained history.

Preview in the pinned environment with
`python3 scripts/lib/work_item_retirement_publish.py` (the default mode).
Preview changes neither the source store nor remote refs, branches, or PRs.
`--apply` is only for the trusted isolated GitHub workflow and enforces remote
merge-protection and publication guards. It runs `just tasks_check` before and
after MyQue mutation, verifies exact history and the same-UUID storage-only
diff, and pushes/verifies this batch's retained refs before the branch or PR.
Empty batches publish nothing; an existing retirement PR stops a new batch.
Partial publication is resumed only after verifying expected bot-owned state,
never by overwriting a PR under review or deleting retained refs. A human must
review and merge after normal CI; automatic merge and direct `main` writes are
not supported.

Manual dispatch defaults to preview. The daily schedule runs only when the
repository variable `MYQUE_RETIRE_ENABLED=true` is set **after real manual
GitHub qualification**, including the retained refs, maintenance PR, actual CI
trigger/approval path, and stale-base race. Do not infer qualification from
local simulations; this document does not claim the workflow tested or enabled.
Publication prerequisites are PR-required rules with no automation bypass,
the required `Canonical work-item store` check from GitHub Actions (integration
`15368`) under strict/current-base rules,
and GitHub Actions permission to create PRs (“Allow GitHub Actions to create
and approve pull requests”). Concurrency alone cannot stop stale retirement
after an item is edited or reopened on `main`. `GITHUB_TOKEN`-created PRs may
need human approval or a supported manual check trigger; do not claim automatic
CI without observing it, replace the token, or bypass the merge rules.

The [lifecycle guide](docs/getting-started/06-work-item-lifecycle.md#retirement-maintenance-prs)
owns exact evidence selection, retained-ref transport, qualification, and recovery.
An administrator must also approve the ruleset identities using the lifecycle
guide's `--approve-rules` procedure and set `MYQUE_RETIRE_RULES_APPROVAL`;
workflow tokens cannot inspect private bypass lists. A ruleset change invalidates
that approval without granting the workflow administrative credentials.

An administrator must establish these prerequisites before qualification;
automation does not modify repository settings. A token may lack
administrative read permission, so PR creation permission is ultimately
enforced by GitHub's publication request, not promised by a local preflight.

## Trivial-change exception

A genuine typo, broken link, format-only change, tiny wording correction, or
mechanical maintenance with **no behavioral or project-state consequence** may
skip the landed-item requirement. Judge semantics, not line count. When
uncertain, create the item. This exception does not waive applicable checks.

## Continuous integration

General CI lives in `.github/workflows/ci.yml`. Pushes to `main` and `develop`,
and pull requests targeting any branch, run it; a newer run cancels the
superseded one for the same ref. The workflow's aggregate `CI` job fails unless
every gate job succeeded. Retirement publication additionally requires the
`Canonical work-item store` check with strict/current-base enforcement and
PR-required rules without an automation bypass; `CI` alone does not establish
that freshness boundary.

Host checks, Kani, and the aggregates run on `ubuntu-latest`; the seL4 image,
root tests, rollback runtime, contract sources, and the four generation shards
run on `ubuntu-24.04-arm`, which is the product's native architecture and free
for public repositories. seL4 jobs enter the repository's pinned Nix dev shell
through the local `.github/actions/slime-env` composite action, which restores
the Nix store, `~/.rustup`, `~/.cargo`, and the persistent Cargo build outputs,
then fetches both pinned toolchains' locked dependencies before the deliberately
offline product build. Jobs that need neither the seL4 prefix nor Nix use a
plain pinned toolchain and `Swatinem/rust-cache`.

Every submodule resolves from GitHub over anonymous HTTPS: `deps/zutai`, the
seL4 fork, and the three rust-sel4 checkouts, whose exact origins are pinned in
`sel4/pins.toml` and enforced by `sel4_pin_check`. QEMU runs without
hardware-device access.

SDK publication (`publish-sdk.yml`, on the dedicated signing runner) and
MyQue's issue projection (`myque-project.yml`) stay separate from ordinary CI:
they rely on GitHub-specific permissions, events, and secrets. The projector
reacts to this workflow's completion and reconciles on a schedule. SDK
publication, signing keys, and issue history retain their separate owners.

## Further reading

- [README](README.md): what Slime OS is.
- [Getting started](docs/getting-started/01-orientation.md): how to learn,
  build, run, and change it.
- [AGENTS.md](AGENTS.md): routing, invariants, schemas, gates, MyQue, and change-record rules.
- [Architecture](docs/architecture/README.md) and [plans](docs/plans/README.md): current subsystem rationale versus unimplemented requirements; [roadmap classification](roadmap/README.md) names unextracted detail and historical sources.
- [History](docs/history.md): immutable archive lookup, privacy, evidence boundaries, and restoration.
