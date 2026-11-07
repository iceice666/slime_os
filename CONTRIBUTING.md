# Contributing to Slime OS

## The short version

Slime OS uses GitHub for collaboration, but canonical project state lives in
the repository: `specs/<area>.zt` says what the system promises, and the
`evidence` branch says what has been observed. GitHub Issues are intake for
bugs and proposals; nothing projects into them. The rationale is
[Living specs replace work items](docs/decisions/living-spec-devloop.md).

**A behavior change changes the spec.** One pull request carries the spec
diff, the exam and the implementation together. A PR that changes what the
system does without a diff under `specs/` does not merge.

**A grader change needs a human approval.** CI runs
`scripts/spec/check --base` on every pull request and asks for an approval
from a login in `HUMAN_OWNERS` when the diff changes an area's `invariants`,
an existing `contracts/**/schema.zt`, or a file in the grader set of any
bound target. New checker files enter freely.

**PR merged != requirement enforced.** After the merge, CI on `main` runs
each bound target whose closure digest has no record yet and writes the
record to the `evidence` branch; `just spec_status` reads it. No person and
no bot writes "done".

If those rules read as ceremony, start with
[How work flows](docs/getting-started/00-how-work-flows.md): the same process
as a timeline, with the reason and the enforcing check beside each step.
[Carrying a change through the living specs](docs/getting-started/06-spec-lifecycle.md)
walks the helpers and the CLI. This file owns the exact rules.

## Where work lives

### Human Issues are intake

Open a [GitHub Issue](https://github.com/iceice666/slime_os/issues) for a bug,
proposal, question, or discussion. For bugs, include the target, revision,
reproduction steps, expected result, and observed output when available.
Accepted work becomes a requirement in the owning area file; the Issue does
not track it.

### What owns what

| Surface | Owns |
|---|---|
| `specs/<area>.zt` | What one plane or subsystem promises: requirements, acceptances, invariants, non-goals, decisions, open questions |
| `evidence` branch | Observation records keyed by closure digest; signed human and operator records |
| `specs/legacy.zt` | The identity and frozen state of every retired `.tasks/` work item |
| GitHub | Intake, collaboration, reviews |
| PR | Reviewable change, claim, risk, verification, limits, the requirement ids it adds or changes |
| Owning docs/contracts | Current architecture, behavior, procedures, and limitations |
| [History archive](docs/history.md) | Historical investigations and raw evidence; never a product build dependency |

Requirement ids are UUIDv7 and never change once on `main`. A legacy item's
UUID resolves through `specs/legacy.zt`; its full text is in Git at the
commit the row names as `frozenAt`.

## Proposing work

Discuss bugs and ideas in Issues first when useful. Then write the
requirement in the area file that owns the behavior:

```text
h.req "<fresh UUIDv7>" "<one sentence of promise>" "<why>" {
  h.observedWhere "<acceptance>" "<just target>" { h.count "casesObserved" 4; } "<what the count proves>";
};
```

- A promise without an exam yet is `h.planned ... "<why it waits>"
  h.noAcceptance`; it reports `advisory`.
- A requirement that is not planned needs at least one `h.observed`
  acceptance; `h.human` is never the only one.
- Replace a requirement by writing the new one with `supersedes` and deleting
  the old; Git keeps the history.
- `just spec_check` refuses an id that is not a UUIDv7, a target `just` does
  not declare, a `# spec:` mark that names no acceptance, a `.zti` under
  `specs/`, and a Zutai pin that disagrees with the submodule or the flake.

### Exams and records

An exam is an ordinary `just` target. Its checker reports what it counted
through `scripts/lib/devloop_observations.py`:

```python
from devloop_observations import record
record(staleHandlesRejected=refusals, negativeControlsRefused=len(controls))
```

`scripts/spec/record <target>` runs the recipe with that report enabled,
takes `passed` from the exit status — a checker never reports it for itself —
and writes `build/observations/<target>.zt`, an `observation/v1` record
keyed by the target's closure digest. The digest covers the recipe
definitions in the target's dependency chain, the scripts they invoke and
the `scripts/lib` modules those import, the derivation id of each system
image the checkers build, `[metadata("inputs", ...)]` the recipes declare,
and the toolchain pins. Change any of them and the record no longer applies.

A record taken from a tree that differs from `HEAD` says `dirty` and cannot
be published. A target that declares neither an image closure nor inputs is
`coarse`: any product change invalidates it.

## Development setup

Start with [Orientation](docs/getting-started/01-orientation.md), then
[Build and run](docs/getting-started/02-build-and-run.md) for submodules,
`nix develop`, toolchain pins, and QEMU. Continue with
[Your first change](docs/getting-started/04-first-change.md) or
[Add a component](docs/getting-started/05-add-a-component.md).
These guides own the tutorials; this document owns the contribution workflow.

## Making a change

- Read [AGENTS.md](AGENTS.md), the detailed repository engineering rules.
- Route through the [task-to-file index](docs/task-to-file.md) before searching. Change the owning
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

### Grader independence

`scripts/spec/check --base <base> --approvers <logins>` judges every pull
request. The workflow passes the logins that approved the head; the check
requires one of them to be in `HUMAN_OWNERS` when the diff:

1. changes any area's evaluated `invariants`;
2. modifies or deletes a `contracts/**/schema.zt` that exists at the base;
3. modifies, deletes or renames a file in the grader set of any target an
   acceptance binds at the base, or changes such a target's recipe closure.

The rule looks at every bound target at the base, enforced or not, and not at
whether the PR carries a spec diff. Splitting the grader change and the
implementation into two PRs triggers it on both. Rule 4 — every touched
product file under some area's `owns` — refuses the PR; a new product path
needs an `owns` glob in the area that promises its behavior. A chore that touches no area (rename, format, pure
refactor) takes the same approval exit; there is no separate class.

Every `contracts/**/schema.zt` is compiled by `just contracts_check` whether
or not a generator or checker names it yet, so a schema landed ahead of its
implementation is still checked
([decision](docs/decisions/planning-pr-schema-contracts.md)).

### The PR itself

Use the [change](.github/PULL_REQUEST_TEMPLATE/change.md) or
[system-change](.github/PULL_REQUEST_TEMPLATE/system-change.md) template. State
what changed, the claim, risks/invariants, review surface, exact verification,
evidence class, target or image identity when relevant, known limits, and the
requirement ids the PR adds, binds or changes. Link existing evidence instead
of duplicating it.

Review reads the area's `openQuestions` first, then the spec diff, then the
code diff ([spec-review skill](.agents/skills/spec-review/SKILL.md)). Review
conduct — what a reviewer may post, how severity is assigned, and how an
author answers or declines a finding — is set in [AGENTS.md](AGENTS.md)
under "Code review". When an author stops and asks, [HUMAN.md](HUMAN.md)
says how to answer.

### No GitHub auto-close

Do not use `Closes #123`, `Fixes #123`, or `Resolves #123` to complete work.
GitHub does not own completion; the evidence branch does.

### Verification

Run the narrowest gate covering the changed behavior and the checks required
by [AGENTS.md](AGENTS.md). Record exact commands, observed results, evidence
class, scope, target or image identity when relevant, and limits; separate
direct observations from inherited evidence and label unobserved conclusions
as inference. Documentation-only PRs state that no runtime tests were run.
Run `just docs_check` for maintained documentation, links, or
command-reference changes, `just spec_check` when `specs/` changed, and
`just typos` for documentation. `just spec_status --local` shows the
requirement enforced from your own record before CI records it.

## After the merge

**Merge means implementation landed, not that the requirement is enforced.**
The Evidence workflow (`.github/workflows/evidence.yml`) runs after every
push to `main`: it resolves every bound target's closure digest, balances the
targets with no record at that digest over shards by observed wall time
(`specs/targets.toml`), runs them, and commits the records to the `evidence`
branch. It never writes `main`.

`just spec_status` then reports each acceptance as enforced, failing, claimed
or advisory. `just spec_status --unenforced` is the backlog: a `failing`
requirement is fixed, or marked `planned` with a note, by the next PR that
touches its area. QEMU output does not prove hardware support, and a green
unrelated gate does not satisfy a requirement.

### Human gates and hardware targets

An `h.human` acceptance waits for a signed answer:

```sh
scripts/spec/human <area> <requirement> <acceptance> yes|no --signer <login>
```

A target CI cannot run — listed under `operator` in `specs/targets.toml` — is
recorded by its operator and published by hand:

```sh
scripts/spec/record <target> --signer <login>
scripts/spec/publish && git push origin refs/heads/evidence
```

Both sign with `ssh-keygen -Y sign` against `specs/allowed_signers`; the
signature, not the pusher, is what status trusts. Rewording a requirement's
statement or acceptance invalidates its human records; rewording a decision
or an invariant does not.

## Trivial-change exception

A genuine typo, broken link, format-only change, tiny wording correction, or
mechanical maintenance with **no behavioral or project-state consequence**
needs no spec diff. Judge semantics, not line count. When uncertain, write
the requirement. This exception does not waive applicable checks or the
grader-independence approval.

## Continuous integration

General CI lives in `.github/workflows/ci.yml`. Pushes to `main` and `develop`,
and pull requests targeting any branch, run it; a newer run cancels the
superseded one for the same ref. The workflow's aggregate `CI` job fails unless
every gate job succeeded.

Host checks, Kani, and the aggregates run on `ubuntu-latest`; the seL4 image,
root tests, rollback runtime, contract sources, and the four generation shards
run on `ubuntu-24.04-arm`, which is the product's native architecture and free
for public repositories. seL4 jobs enter the repository's pinned Nix dev shell
through the local `.github/actions/slime-env` composite action, which restores
the Nix store, `~/.rustup`, `~/.cargo`, and the persistent Cargo build outputs,
then fetches both pinned toolchains' locked dependencies before the deliberately
offline product build. Jobs that need neither the seL4 prefix nor Nix use a
plain pinned toolchain and `Swatinem/rust-cache`.

The spec job uses `nix develop .#spec`: a checkout plus `just`, Python,
`zutai-cli`, `gh` and `ssh-keygen`, with no product toolchain. It runs
`just spec_check`, the CLI's own end-to-end run, and on a pull request the
grader-independence judgement with the head's approvers.

Every submodule resolves from GitHub over anonymous HTTPS: `deps/zutai`, the
seL4 fork, and the three rust-sel4 checkouts, whose exact origins are pinned in
`sel4/pins.toml` and enforced by `sel4_pin_check`. QEMU runs without
hardware-device access.

SDK publication (`publish-sdk.yml`, on the dedicated signing runner) stays
separate from ordinary CI: it relies on GitHub-specific permissions, events,
and secrets. SDK publication and signing keys retain their separate owners.

## Further reading

- [README](README.md): what Slime OS is.
- [Getting started](docs/getting-started/01-orientation.md): how to learn,
  build, run, and change it.
- [AGENTS.md](AGENTS.md): routing, invariants, schemas, gates, specs, and change-record rules.
- [Architecture](docs/architecture/README.md) and [plans](docs/plans/README.md): current subsystem rationale versus unimplemented requirements; [roadmap classification](roadmap/README.md) names unextracted detail and historical sources.
- [History](docs/history.md): immutable archive lookup, privacy, evidence boundaries, and restoration.
