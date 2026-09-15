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
conditions. Use `myque new "Describe the work" --kind task`, then
`myque parent <CHILD_UUID> <PARENT_UUID>` and `just tasks_check`; follow the
planning-PR process below. Do not change kinds or parent relationships merely
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

Discuss bugs and ideas in Issues first when useful. To propose canonical work,
use [MyQue](https://github.com/mozufu/myque) in the repository dev shell:

```sh
myque new "Describe the work" --kind task
```

Use `--kind bug --tag backlog` for a defect. MyQue allocates the UUID; never
invent one or allocate a numbered key by scanning the store. Fill in the item's
problem, scope, observable exit conditions, and use MyQue commands for any
parent/dependency relationships. Follow [AGENTS.md](AGENTS.md)'s backlog-first
and work-item rules; run `just tasks_check`.

Submit a **work-item-only planning PR**, and merge it before opening an
implementation PR. An already-landed item can cover another implementation PR
within its scope; a new planning item is not required for every PR.

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

Record the observed exit evidence, its scope, identity, and limits in the item.
Only then run:

```sh
myque close <ITEM>
```

Commit the canonical `.tasks` transition. After it reaches `main`, `myque-gh`
subsequently closes the projected Issue. Leave unfinished work unfinished.

## Trivial-change exception

A genuine typo, broken link, format-only change, tiny wording correction, or
mechanical maintenance with **no behavioral or project-state consequence** may
skip the landed-item requirement. Judge semantics, not line count. When
uncertain, create the item. This exception does not waive applicable checks.

## Continuous integration

General CI lives in `.github/workflows/ci.yml`. Pushes to `main` and `develop`,
and pull requests targeting any branch, run it; a newer run cancels the
superseded one for the same ref. The workflow's aggregate `CI` job is the
single status to require in branch protection — it fails unless every gate
job succeeded.

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
