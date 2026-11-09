# Work items return on top of living specs, with machine ratchets

**Status:** Accepted
**Decided by:** the project's human owner, 2026-11-10, accepting every agent
default in `proposal/devloop-v1-redesign/README.md` §9 and choosing the
derived-file scope below.
**Amends:** [Living specs replace work items](living-spec-devloop.md). The
evidence branch, closure digests and computed status stay. The work item
returns as the unit of change, and human approval of grader changes ends.

## Context

Living specs removed the V1 bookkeeping cost. They kept one premise that the
v2 experiment and 33 recorded sessions did not support: that a person writes
invariants and approves grader diffs. In those sessions the human wrote 163
messages with a median length of 12 characters. No message stated a formal
invariant. Agents wrote every invariant, and the human answered "agreed".

An approval of a checker diff the approver cannot read tells the machine
"someone looked". That signal is false. The redesign replaces it with checks
a machine can run and leaves people the questions they can answer.

## Decision

- A work item is `.tasks/items/<slug>.zt`, a Zutai source whose final value
  is a `task/v1` `Item`. Its UUIDv7 `id` never changes. Its `slug` is the
  file name.
- One pull request carries the item, the implementation, the exam and, when
  the item is done, the move to `.tasks/terminal/`. `scripts/spec/tasks
  check` is the admission check. No planning PR exists.
- Every requirement, non-goal, invariant, decision and known limit carries a
  `provenance`: a quoted human statement, a decision page, or
  `agentDefault`. `tasks status` reports the `agentDefault` share.
- An item has no `state` field. `tasks status` computes each acceptance from
  the `evidence` branch. An item is done when every mandatory acceptance is
  enforced.
- `specs/<area>.zt` stays. It holds the area's invariants, open questions and
  the requirements promised before this decision. Each item names its area.
- Four ratchets replace human approval of grader changes. R1 counts contract
  clauses per interface. R2 is the share of clauses with an oracle check and
  a failing negative control. R3 is the mutant survival rate in owned
  crates. R4 is the declared negative-control floor per target.
- A ratchet regression passes only when the pull request adds a known limit
  that names the ratchet and subject, and a login in `HUMAN_OWNERS` approves.
  This is the only remaining human approval.
- Each commit declares a `Devloop-Role:` trailer: `spec-writer`,
  `implementer` or `breaker`. CI refuses a commit whose diff leaves its
  role's path set.
- Two open items must not own overlapping paths. At most three item trees
  are open at once.
- System-image closure sources are regenerated after the merge by a
  workflow, not inside the pull request. Generated bindings stay in the pull
  request because they are compile inputs.

## Open choices settled

Each row uses `Decision: <option>. Because <reason>. Reopen if <signal>.`
Rows Q1 to Q8 are agent defaults the human accepted as a set. Row D1 is the
human's own choice.

| # | Decision | Because | Reopen if |
| --- | --- | --- | --- |
| Q1 | Change `slime_os` directly. | v2 has no CI, and a gate without an execution environment is only form. | a change here breaks the Evidence workflow for more than one day |
| Q2 | R3 measures only crates inside an item's `owns`. | A whole-workspace mutation run takes hours. | a regression lands in a crate that no item owns |
| Q3 | Ratchet approval is a pull-request approval by a `HUMAN_OWNERS` login. | It is already implemented and a non-expert can answer it. | an approval is given without reading the known limit |
| Q4 | Roles use commit trailers only, without separate accounts. | Measure whether two roles collude before paying for accounts. | one commit is found to carry a role it did not act as |
| Q5 | Area files stay as the home of invariants and open questions, and index items. | Area invariants and open questions have no other home. | items and area requirements disagree about one behavior |
| Q6 | A failed post-merge regeneration reports a failing check and waits for the next pull request. | A revert interrupts the parallel tracks. | `main` stays stale for more than one day |
| Q7 | The file name is `<slug>.zt` and the UUID is inside the file. | A readable name helps, and `tasks check` refuses repeated ids and broken links. | a rename breaks a `parent` or `depends` that the check did not catch |
| Q8 | `schema.zt` and `helpers.zt` are copied from v2 into `.tasks/items/` and evolve here. | v2 is an experiment, and a separate repository repeats the three V1 pins. | v2 and this repository both still edit the schema after three months |
| D1 | Only closure sources move to post-merge regeneration. Generated bindings stay in the pull request. | Bindings are compile inputs, so `main` would not build between the merge and the regeneration. | two parallel pull requests conflict on generated bindings more than once a month |
| D2 | `.tasks/terminal/` holds byte-identical copies of `schema.zt` and `helpers.zt`. | Zutai refuses an import above the entry file's directory. | Zutai gains a project root that spans both directories |

## Alternatives considered

- **Fold the new fields into `specs/<area>.zt` instead of items.** Rejected by
  the human owner. An area is a ledger of promises, and the proposal needs a
  unit of change with `owns`, a parent and a closing move.
- **Keep `HUMAN_OWNERS` approval for grader changes.** Rejected. A person
  approving a diff they cannot read gives a false signal, and the ratchets
  catch a weakened exam by measurement.
- **Regenerate generated bindings after the merge as well.** Rejected (D1).

## Consequences

- An agent account can change a checker without a person. A weaker checker
  must still keep R1 to R4 from falling, or carry an approved known limit.
- Role separation is context separation, not permission separation. Two
  agents can still agree to cheat. Separate accounts and branch protection
  are the reopen path.
- A `#human` provenance is not signed. A reader can compare its quote with
  the session record afterwards.
- A ratchet blocks a decrease and allows no change. `tasks status --weekly`
  shows a measure that does not improve.
- A finite trace does not prove liveness or every input.

## Revisit when

- more than one pull request in ten needs a `HUMAN_OWNERS` approval;
- the `agentDefault` share of invariants rises for four consecutive weeks;
- the median time from `claimed` to `enforced` exceeds one push to `main`.
