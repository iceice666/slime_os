# Carrying a change through the living specs

One rule: a behavior change changes the spec. A PR that changes what the
system does without a diff under `specs/` does not merge.

A feature is one PR carrying three things together: the spec change (a work
item under `.tasks/items/`, or requirements added, rewritten or removed in
the owning area file), the exam (a `just` target), and the implementation.
[`docs/decisions/work-item-devloop.md`](../decisions/work-item-devloop.md)
describes the work item. There is no `done`
state either: `scripts/spec/status` computes each requirement from records
on the `evidence` branch after the PR lands. The rationale is in
[`docs/decisions/living-spec-devloop.md`](../decisions/living-spec-devloop.md).

## The three things

| Thing | Where | What it is |
| --- | --- | --- |
| spec | `specs/<area>.zt` | an `Area`: what one plane or subsystem promises today, as requirements with acceptances |
| exam | a `just` target | an ordinary checker; `scripts/spec/record` wraps it and writes an `observation/v1` record |
| evidence | branch `evidence` | `<target>/<closureDigest>.zt`, written by CI after each push to `main`; never in the tree |

`specs/schema.zt` defines the types, `specs/helpers.zt` the constructors.
`specs/legacy.zt` indexes the retired `.tasks/` store by UUID; the full text
of those items is in Git at the commit each row names as `frozenAt`.

## Write the requirement

Open the area file that owns the behavior (one per plane today; see each
file's `owns`). Add a requirement:

```text
h.req "<fresh UUIDv7>"
  "<one sentence: what the system promises>"
  "<why>"
  {
    h.observedWhere "<acceptance id>" "<just target>" { h.count "casesObserved" 4; } "<what the count proves>";
  };
```

- `python3 -c 'import uuid; print(uuid.uuid7())'` prints the id. It never
  changes once on `main`.
- Every requirement that is not `planned` needs at least one `h.observed`
  acceptance. `h.human` asks a person a question a non-expert can answer and
  is never the only acceptance.
- A promise without an exam yet is `h.planned ... "<why it waits>"
  h.noAcceptance`. It reports `advisory` until bound.
- Replacing a requirement: write the new one with `supersedes = { "<old>"; }`
  and delete the old. Git keeps the history.
- `invariants`, `nonGoals`, `decisions` and `openQuestions` are area-level and
  human-owned. Changing `invariants` needs an approval, like a grader.

## Write the exam

An exam is a recipe. A checker reports typed observations with
`devloop_observations.record(casesObserved=..., negativeControlsRefused=...)`;
`record` turns them into the record's `counts` and `flags`. `passed` is the
recipe's exit status, never something the checker reports. A checker may
carry `# spec: <requirement id>/<acceptance id>` so readers can draw the
edge without parsing the recipe; `spec_check` verifies the mark resolves.

[Add a gate](07-add-a-gate.md) explains a plane checker; the
[verification-checkers skill](../../.agents/skills/verification-checkers/SKILL.md)
owns negative controls and judge independence.

## Check locally

```sh
just spec_check                      # schema, ids, bound targets, marks, Zutai pin
just spec_record <target>            # run the exam, write build/observations/<target>.zt
just spec_status --local             # enforced / failing / claimed / advisory, with local records
python3 scripts/spec/closure <target> --explain   # what the record binds to
```

A record from a dirty tree says `dirty` and cannot be published; commit
first. A `coarse` closure means the target declares neither an image closure
nor `[metadata("inputs", ...)]`, so any product change invalidates it.

## What CI judges

On a pull request four checks run against the base. No person approves a
grader change.

- `scripts/spec/roles --base` refuses a commit that touches a checker, an
  oracle, a fixture, product code, a recipe or an item without exactly one
  `Devloop-Role:` trailer, or that touches a path its role does not admit.
- `scripts/spec/ratchet --base` refuses fewer contract clauses (R1), fewer
  covered clauses (R2), higher mutant survival (R3) or a lower
  negative-control floor (R4), unless the PR adds a known limit and a login
  in `HUMAN_OWNERS` approves.
- `scripts/spec/tasks check --base` refuses closing an item that is not
  done, and touching a failing item's `owns` without a fix or a known limit.
- `scripts/spec/check --base --enforce-owns` refuses a touched product file
  under no area's `owns`. Add the glob to the area that promises its
  behavior.

After the merge, the Evidence workflow resolves every bound target's closure
digest, runs those without a record at that digest (in shards, by observed
wall time from `specs/targets.toml`), and commits the records to `evidence`.
Nothing lands on `main`. `just spec_status` then reads the branch.

## Human gates and hardware

`scripts/spec/human <area> <requirement> <acceptance> yes|no --signer <login>`
writes a signed answer bound to the requirement's digest; rewording the
statement or an acceptance asks again. A target CI cannot run (listed under
`operator` in `specs/targets.toml`) is recorded by its operator with
`scripts/spec/record <target> --signer <login>` and published with
`scripts/spec/publish`. The signature, not the pusher, is what status trusts.

## Status and the backlog

| State | Meaning |
| --- | --- |
| enforced | a passing record at the current closure digest; every predicate holds |
| failing | a record exists and failed, a predicate does not hold, or a person signed no |
| claimed | bound to a target, no record at the current digest yet |
| advisory | planned, or a human gate without a verified answer |

`just spec_status --unenforced` is the backlog. A `failing` requirement on
`main` is fixed, or marked `planned` with a note, by the next PR that touches
its area.

## Next

[Add a gate](07-add-a-gate.md) — how a plane checker is built.
