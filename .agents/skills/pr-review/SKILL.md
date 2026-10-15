---
name: pr-review
description: Rules that bound any review of a Slime OS pull request and any answer to one — what counts as a finding, how severity is assigned, one review per head, consistency across rounds, and how an author reproduces, fixes, or declines a finding. Use when asked to review a PR, to triage or answer review threads (including Codex findings), or to decide whether a PR is ready. For the adversarial procedure against a spec-driven item, use `spec-review` alongside this.
---

# PR review

These rules bind every reviewer of a PR — a person, a review bot such as
Codex, or an agent asked to review — and the author answering the review.
`AGENTS.md` carries the summary that review bots read; this skill is the full
text. `.agents/skills/spec-review/SKILL.md` is the procedure for an
implementation PR against a spec-driven item; the rules here bound what any
review posts.

A review's job is to find where the diff, the checker it binds, and the
landed work item disagree, and to say so with a file and line.

## The item bounds the review

Read the work item (requirements, acceptance rationale, `scope`,
`nonGoals`), the PR description, and the existing review threads before
reading the diff.

A process PR — CI workflows outside `codePaths`, `CONTRIBUTING.md`,
`AGENTS.md`, `.agents/`, `docs/` — has no item. There the bound is the rules
in `AGENTS.md` the PR claims to follow and the claims its description makes:
a finding names the rule or claim the diff contradicts, with the file and
line, and the input or reader that would be misled. The absence of an
acceptance rationale neither makes such a PR unreviewable nor excuses a
defect in it.

- A finding names the requirement, invariant, or rule in `AGENTS.md` that
  the diff breaks, the file and line, and the input that produces the wrong
  result.
- An exam's case count and control floor belong to the planning item: "this
  boundary could also be exercised" is not a finding unless the acceptance
  rationale claims that boundary, because every added case re-admits the
  item.
- A missing scenario the reviewer believes matters goes in one comment that
  proposes a follow-up item, not in one comment per case.
- A limit the PR description declares under known limits or invariants is a
  decision to weigh, not a gap to report; a reviewer who thinks the decision
  is wrong says why, once, on the thread for it.

## Severity states a consequence

A finding is **blocking** only when:

- a gate can pass while its claim is false;
- a refused input is accepted;
- an authority, lifetime, or determinism rule is broken;
- a format crossing a boundary has no Zutai contract;
- the recipe cannot pass as landed.

A real defect in the diff whose fix is local is a **non-blocking** finding.

Everything else — style, an alternative design, hardening the item does not
require — is a **suggestion**, labelled as one; it never blocks a merge and
is not raised again on a later head.

A finding that would require the PR to carry what
`scripts/check/check-pr-scope.py` refuses, or what the landed item excludes,
is addressed to the planning process in one comment, not to the PR.

## One review per head, bounded in size

Review the whole diff once at the head the review is for. A review of a
later head covers the pushed delta and the threads that delta answers; it
does not re-open the rest of the diff to look for new cases.

Post the findings that change the verdict. If a review would post more than
ten, the diff has a structural problem: name it, give the strongest findings,
and stop. Do not post the same finding twice in one round under two titles,
and do not reproduce a finding another tool already posted.

## Consistency across rounds

Before posting, read every open and resolved thread.

- A finding that restates an open thread is a reply on that thread.
- A finding that contradicts an earlier finding, or asks to undo a change an
  earlier round asked for, says so, names the earlier thread, and states
  what evidence changed; a reviewer who cannot name that evidence withholds
  the finding.
- A thread the author has answered with a question or a declared decision
  waits for the human who owns the PR; re-posting it on the next head is not
  an answer.

## Answering a review

- **Reproduce before fixing.** A finding that does not reproduce is answered
  with what was tried and stays open for the reviewer. The reply to a fixed
  finding says what reproduced, the commit that fixed it, and what control
  now refuses the case.
- **Declining is legitimate.** When a finding is outside the item's scope,
  contradicts the landed spec, or asks for what the PR cannot carry, reply
  with the reason and leave the thread for the human.
- **One push per round.** Answer every thread of a round in one push rather
  than one push per finding, since each push starts another round.
- **Re-admit once.** Do not re-admit the item to satisfy a suggestion;
  collect scope changes and re-admit once, or open a follow-up item. A
  blocking finding against an exam whose pinned count must move does
  re-admit the item; say so in the reply.
- **Know when you are done.** When a round yields nothing blocking, the PR is
  ready and the author does not request another round.
- **Threads hold the history.** The PR description describes the head as it
  is and does not accumulate a per-round changelog.
- An agent's or bot's review is review, not verification evidence; do not
  list "independent review: no blockers" in the PR's verification table.
