# Slime OS Human Guide

Agents write most of the code here and a review bot reviews it. That loop can
move work forward with no person deciding anything, and left alone it will.
This file is what the people on the project own. [`AGENTS.md`](AGENTS.md) tells
agents when to stop and hand a question to you; this file tells you how to
answer it.

You do not need operating-systems coursework to do any of this. Most decisions
below are trade-offs about what Slime OS should be, and only this team can make
those. An agent left to choose picks a plausible default, usually whatever a
Unix-like system would do, and keeps going.

## Decisions only a person makes

- **Scope.** Does this PR support the case, or is it recorded as a known limit?
  Declaring a limit is a decision, not a gap.
- **Simple or flexible.** For example: must storage equal the declared
  configuration exactly, or may it be larger? Picking "exactly" can close a
  whole class of review findings at once.
- **Which invariants are non-negotiable.** Each subsystem has three to five
  properties that must never break (for example, "a refused input changes no
  state"). A person writes or approves them; agents and reviewers work against
  them. An invariant an agent wrote and then satisfied by itself protects
  nothing.
- **Stopping.** When the same subject comes back a third round, decide whether
  it is worth another round, needs a design decision, or becomes a known limit.
- **Process.** A new gate, exam, rule, or work-item stage is a decision about
  how much of the team's attention verification gets. It is not a fix.

### How to make one

Ask the agent for two or three options, the cost of each, and its
recommendation. Pick one and answer in the thread:

```text
Decision: <the option>. Because <one sentence>.
```

If the decision should outlive the PR, ask the agent to record it in
[`docs/decisions/`](docs/decisions/README.md). Picking among explained options
is a skill you build by doing it and looking back at which choices you regret.

## Reading a PR in five minutes

You do not need to read the diff. Read these, in order, and open the diff only
where one of them surprised you:

1. **Why.** Does the one-sentence reason match what you expected this work to
   do?
2. **Invariants / risk.** Are the properties it claims to keep the ones you care
   about?
3. **Decisions.** Did the agent make a choice that should have been yours?
4. **New tests, by name.** Test names here are sentences. Read the list: is
   anything you care about missing? Is anything there not worth having?
5. **Size.** More than roughly 500 handwritten product lines (excluding
   generated files and tests) means it should probably be split.

Merge only what you can describe in one sentence to a teammate.

## Signals to stop and look

- The same review subject appears in a third consecutive round.
- A review comment uses a term you cannot find in
  [`docs/glossary.md`](docs/glossary.md). Ask for an entry of at most two
  sentences; if it cannot be explained that briefly, the thing it names may be
  over-built.
- Over a week, more PRs touch `scripts/`, `.tasks/`, `.devloop/` or `.agents/`
  than touch `slime-root/` and `components/`.
- You merged something last week and cannot say what it does.

## Keeping the system understood

- **Each person owns a subsystem** and periodically explains it to the others.
  An agent-written walkthrough is a fine script, but the owner answers the
  questions. Whatever the owner cannot explain is where understanding is
  thinnest.
- **Ask for walkthroughs, not summaries.** "Starting from the entry point,
  explain this module in execution order, citing files and lines" produces
  something you can check against the code.
- **Check the tests occasionally.** Mutation testing (`cargo mutants`) changes
  the code on purpose and reports which changes no test noticed. A surviving
  mutant is behaviour nobody verifies.
- **Learn what the system stands on.** Slime OS runs on seL4, so the seL4
  whitepaper and reference manual are the most direct material (capabilities,
  IPC, kernel objects). For general background, *Operating Systems: Three Easy
  Pieces* is free and widely used.
