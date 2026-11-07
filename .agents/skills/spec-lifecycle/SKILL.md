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
| exam | a `just` target, run under `scripts/spec/record` | the checker's author; the grader-independence rule protects it |
| evidence | branch `evidence`, `<target>/<closureDigest>.zt` | the Evidence workflow for CI targets; the signer for human and operator records |

There is no work item and no `done` state. `scripts/spec/status` computes
every acceptance from the evidence branch: enforced, failing, claimed,
advisory. `specs/legacy.zt` indexes the retired `.tasks/` store by UUID;
its full text is in Git at the commit each row names as `frozenAt`.

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

`scripts/spec/check --base <base> --approvers <logins>` requires an approval
from a login in `HUMAN_OWNERS` when the diff:

1. changes any area's evaluated `invariants`;
2. modifies or deletes a `contracts/**/schema.zt` that exists at the base;
3. modifies, deletes or renames a file in the grader set of any target an
   acceptance binds at the base, or changes such a target's recipe closure.
   New checker files enter freely.

Rule 4 — every touched product file under some area's `owns` — is reported,
not enforced, until every plane has an area. The rule judges every bound
target at the base, enforced or not, and ignores whether the PR carries a
spec diff: splitting a grader change into its own PR triggers it twice. A
chore that touches no area takes the same approval exit. An agent account
never belongs in `HUMAN_OWNERS`; an approval from one satisfies nothing.

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
