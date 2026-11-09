---
name: spec-lifecycle
description: Rules for changing Slime OS behavior through the living specs under `specs/`: writing or binding a requirement, writing its exam, recording and publishing evidence, what `scripts/spec/check --base` asks a human to approve, and how status and the backlog are read. Use before editing `specs/`, a checker, or a recipe an acceptance binds; before opening a PR that changes behavior; when judging whether a change needs a spec diff or a human approval; or when a review finding cites self-grading or a missing spec diff.
---

# Spec lifecycle

`AGENTS.md` keeps the summary. This skill is the full rule set.
`CONTRIBUTING.md` is the human-facing workflow and
`docs/getting-started/06-spec-lifecycle.md` walks the helpers and the CLI;
neither overrides what is stated here. The rationale is
`docs/decisions/living-spec-devloop.md`.

## The one hard rule

A behavior change changes the spec. A PR that changes what the system does
without a diff under `specs/` does not merge. No checker can catch a PR that
changes behavior and leaves the spec untouched; the reviewer reads the spec
diff first for that reason.

## The three things

| Thing | Where | Owner |
| --- | --- | --- |
| spec | `specs/<area>.zt`, an `Area` of requirements | the area's `invariants`, `nonGoals`, `decisions` and `openQuestions` are human-owned; requirements are written by whoever carries the change |
| exam | a `just` target, run under `scripts/spec/record` | a `breaker` commit; roles and ratchets protect it |
| evidence | branch `evidence`, `<target>/<closureDigest>.zt` | the Evidence workflow for CI targets; the signer for human and operator records |

No file records a `done` state. `scripts/spec/status` computes every
acceptance from the evidence branch: enforced, failing, claimed, advisory. `specs/legacy.zt` indexes the retired `.tasks/` store by UUID;
its full text is in Git at the commit each row names as `frozenAt`.

## Work items

A work item is the unit of change
(`docs/decisions/work-item-devloop.md`). It is `.tasks/items/<slug>.zt`,
whose final value is a `task/v1` `Item`; `.tasks/items/schema.zt` defines
the types and `helpers.zt` the constructors. `just tasks_new task <slug>
--area <area>` prints a skeleton with a fresh UUIDv7.

- `h.task`, `h.bug` and `h.milestone` take the id, slug, area, title and
  problem. Extend with `with { owns = ...; requirements = ...; acceptance =
  ...; }`. A milestone must fill owns, nonGoals, requirements, acceptance,
  invariants and decisions.
- An acceptance is `h.observed <target>`, `h.observedWhere <target>
  { predicates }`, `h.human <id> <question>` or `h.clause <interface>
  <contract> <clause> <backends>`. `h.advisory` makes one non-mandatory.
  An item needs one mandatory `h.observed` or `h.clause`.
- Every requirement, non-goal, invariant, decision and known limit carries
  a provenance: `h.fromHuman <quote> <source>`, `h.fromDecision <page>` or
  `h.agentDefault`. `h.req` defaults to `h.agentDefault`.
- `.tasks/terminal/` holds closed items and byte-identical copies of
  `schema.zt` and `helpers.zt`, because Zutai refuses an import above the
  entry file's directory.

`just tasks_status` computes each item acceptance from the evidence branch.
An item is done when every mandatory acceptance is enforced. A mandatory
`h.human` gate without a signed record is claimed, so it blocks done.
`scripts/spec/tasks human <slug> <acceptance> yes|no --signer <login>`
signs an answer; the record binds to the item's requirement statements and
acceptances, not to its provenance or prose. Status also prints each item's
`agentDefault` share.

Closing an item is a move from `.tasks/items/` to `.tasks/terminal/`, in the
PR that finishes it or in a later chore commit. `tasks check --base` on CI
refuses a move unless the item is done (T1). It also refuses a PR that
touches the `owns` of an item whose mandatory acceptance is failing at its
current closure digest (T2). The PR either changes that closure, which
re-runs the exam after the merge, or adds `h.knownLimit "failing"
"<acceptance id>" ...` with an approval from `HUMAN_OWNERS`.

`just tasks_check` refuses a malformed item, a slug that is not the file
name, a repeated or non-UUIDv7 id, a broken `parent` or `depends`, an
unknown area, an undeclared target, an empty milestone list, a known limit
outside R1 to R4, and a drifted terminal copy.

## Executable contracts

An executable contract is a directory `contracts/<name>/v<N>/` holding
`schema.zt` (registry and trace types), `clauses.zt` (an `Interface` of
pre, post and invariant clauses with their exams), `oracle.zt`,
`fixture.zt`, `bad-traces.zt` and `mutations.zt`. Zutai refuses an import
above the entry file's directory, so the files stay together.
`contracts/spawn-arguments-dbc/v1` is the first one.

- The fixture states stimulus only. The adapter named by an exam drives the
  implementation and prints an `Observed` document. It never computes an
  expected outcome or a verdict.
- `oracle.zt` is the only judge. A violation names the clause it breaks.
  A clause marked `#unchecked` states what would decide it instead.
- `scripts/check/check-dbc.py <dir>` requires zero violations on the honest
  run. It requires every tamper, a dropped observation and every mutation
  to make the oracle report its clause. A clause is covered when the honest
  run exercised it and a negative control was refused for it.
- `h.clause <interface> <contract> <clause> <backends>` is enforced when
  each backend's exam record passed, has no violation, and sets
  `covered__<contract>__<clause>`.

## Ratchets

`scripts/spec/ratchet --base <ref>` compares exam strength at the base and
at the head. CI runs R1 and R4 in the `spec` job, and R2 and R3 in
`ratchet_mutants`.

| Id | Subject | Measure | Must not |
| --- | --- | --- | --- |
| R1 | interface | clauses in its `clauses.zt` | fall |
| R2 | interface | covered clauses over all clauses, from its exam | fall |
| R3 | crate in `specs/targets.toml` `[mutants]` | `cargo mutants` survival over the files the PR's items own in it | rise |
| R4 | just target | highest `negativeControls*` floor an area or item declares | fall |

A regression passes only when an item at the head carries a known limit
(`h.knownLimit "<R>" "<subject>" "<reason>" <provenance>`) that the base
did not carry, and a login in `HUMAN_OWNERS` approves. A known limit is new
when its ratchet, subject or reason is new. Raising a measure needs nobody.

## Writing a requirement

- Open the area file whose `owns` covers the implementation. One area per
  plane today; a requirement lives in exactly one.
- `h.req "<UUIDv7>" "<statement>" "<rationale>" { acceptances }`. The id
  comes from `uuid.uuid7()` and never changes once on `main`.
- A requirement that is not `planned` needs at least one `h.observed`
  acceptance (`h.observedWhere` with predicates `h.count`, `h.atLeast`,
  `h.atMost`, `h.flag`). `passed = true` is implicit; a list of predicates
  is their conjunction. `h.human` asks a person a question a non-expert can
  answer and is never the only acceptance.
- A promise without an exam yet is `h.planned ... "<why it waits>"
  h.noAcceptance`; it reports `advisory`.
- Replace: write the new requirement with `supersedes = { "<old id>"; }`
  and delete the old one. Cancel: delete it. Git keeps the history.
- `depends` is informational for readers; it changes no state.
- The requirement digest covers the statement and the acceptances only.
  Rewording them invalidates signed human records; rewording a rationale,
  a decision or an invariant does not.

`just spec_check` refuses: an area id that is not its file name, a
requirement id that is not a UUIDv7 or repeats, a non-planned requirement
without an automated acceptance, a `justTarget` that `just --dump` does not
declare, a `supersedes`, `depends` or `binds` that resolves to nothing, a
`# spec:` mark that names no acceptance, a `.zti` under `specs/`, a Zutai
pin that disagrees with the submodule or the flake input, and a
`scripts/spec/` over its line budget.

## Writing the exam

- An exam is an ordinary recipe. The checker reports what it counted through
  `devloop_observations.record(...)`; `record` turns the report into the
  record's `counts` and `flags`.
- `passed` is the recipe's exit status. A checker never reports it for
  itself, and a count the acceptance pins is harder to fake than an exit.
- Every positive assertion has a negative control
  (`.agents/skills/verification-checkers/SKILL.md`).
- A checker may carry `# spec: <requirement id>/<acceptance id>` (Rust:
  `// spec:`) so a reader can draw the edge without parsing the recipe.
- A recipe whose inputs the resolver cannot see (paths built at runtime,
  files read by a compiled program) declares them:
  `[metadata("inputs", "components/foo/**")]`. A target that declares no
  image closure and no inputs is `coarse`: any product change invalidates
  its record.

## Closure digest

A record is keyed by the target's closure digest. The digest covers, in
this order: the definitions of the target and every recipe it depends on;
the grader set — every `scripts/` file those recipes invoke and the
`scripts/lib` modules those import statically, plus `scripts/spec/lib.py`,
`scripts/spec/record` and `contracts/observation/v1/schema.zt`; declared
inputs; the derivation id of each system image a checker in the closure
names as `CLOSURE = "..."`; the toolchain pins `rust-toolchain.toml`,
`sel4/pins.toml`, `flake.lock`, `Cargo.lock`; and the coarse product
fallback when neither an image nor inputs exist. Zutai is not in the digest;
`specs/zutai.toml` is record metadata.

`python3 scripts/spec/closure <target> --explain` prints the composition.
A record from a tree that differs from `HEAD` says `dirty` and cannot be
published.

## What CI judges on a pull request

No person approves a grader change. Four machine checks run instead
(`docs/decisions/work-item-devloop.md`):

| Check | Refuses |
| --- | --- |
| `scripts/spec/roles --base` | a commit that touches a classified path without exactly one `Devloop-Role:` trailer, or a path its role does not admit |
| `scripts/spec/ratchet --base` | a fall in R1, R2 or R4, or a rise in R3, without a new known limit and a `HUMAN_OWNERS` approval |
| `scripts/spec/tasks check --base` | an item entering `terminal/` while not done (T1); a touch on a failing item's `owns` (T2) |
| `scripts/spec/check --base --enforce-owns` | a touched product file under no area's `owns` (rule 4) |

Role path classes (`scripts/spec/roles --classify <path>` prints one):

| Class | Paths | Roles |
| --- | --- | --- |
| clause | `clauses.zt`, `oracle.zt`, `fixture.zt`, `schema.zt` of an executable contract | spec-writer |
| control | `bad-traces.zt`, `mutations.zt` of an executable contract; `scripts/check/**` | breaker |
| product | `components/`, `slime-root/`, `boot-contracts/`, other `contracts/**`, `verification/dbc/**` | implementer |
| recipe | `Justfile`, `just/**` | implementer, breaker |
| item | `.tasks/**`, `specs/*.zt` | spec-writer, implementer |

A commit that touches only unclassified paths (`docs/`, `scripts/spec/`,
`scripts/lib/`, workflows) needs no trailer. One pull request may carry
commits of every role. The only human approval left is a ratchet regression
or a failing known limit. An agent account never belongs in `HUMAN_OWNERS`;
an approval from one satisfies nothing.

A new product path needs an `owns` glob in the area that promises its
behavior, in the same PR.

## Evidence

- After each push to `main`, `.github/workflows/evidence.yml` resolves every
  bound target's closure digest, runs the targets with no record at that
  digest (shards balanced by `specs/targets.toml` wall times, operator
  targets excluded), and commits the records to `evidence`. It never writes
  `main`.
- `scripts/spec/publish` refuses a record whose digest is not the target's
  in the checkout, a `dirty` record, and a human record without a verifying
  signature. Publishing by hand is for human and operator records only.
- `scripts/spec/human <area> <requirement> <acceptance> yes|no --signer
  <login>` and `scripts/spec/record <target> --signer <login>` sign with
  `ssh-keygen -Y sign -n slime-spec`; `specs/allowed_signers` lists who may.
  The signature, not the pusher, is what status trusts.
- A failing target still yields a record; the requirement reports `failing`.
  Flakiness shows as `passed` alternating at one path in the branch's log.

## Status and the backlog

| State | Meaning |
| --- | --- |
| enforced | a passing record at the current digest; every predicate holds; human gate signed yes |
| failing | a record failed, a predicate does not hold, or a person signed no |
| claimed | bound to a target, no record at the current digest |
| advisory | planned, or a human gate without a verified answer |

`just spec_status --unenforced` is the backlog, with `roadmap/` for
directions not yet written as requirements. A `failing` requirement is
fixed, or marked `planned` with a note, by the next PR that touches its
area; a `claimed` one waits for CI. Hardware targets (`operator` in
`specs/targets.toml`) stay `claimed` until their operator records them.
`status --json` is the interface for readers; nothing reads the evidence
branch's paths directly.

## History and roadmap

Historical evidence uses the repository identity, full commit, and original
path described in `docs/history.md`; archived snapshots never participate
in current validation. `roadmap/` retains explicitly identified unextracted
requirements and their surrounding historical context; its classification
index routes extracted subjects to `docs/architecture/` and `docs/plans/`.
Promises belong only to `specs/`.
