# Mandatory spec-driven work items after a fixed identity cutoff

**Status:** Accepted
**Related work item:** `01a0aef9-4c68-7251-8b66-291fe9f8cf4f`
**Amends:** [`spec-driven-work-item-bodies.md`](spec-driven-work-item-bodies.md)
(points 2 and 4 of its decision)

## Context

[`spec-driven-work-item-bodies.md`](spec-driven-work-item-bodies.md) established
an enforceable-completion path and made it optional: `just tasks_check` decides
which items are subject purely from the presence of a `devloop` frontmatter
record, and nothing requires a new item to carry one.

Measured on the store at the time of writing, that option went unused. It holds
310 items — 64 live, 246 retired — and **zero live items carrying a devloop
record**. Every live item is prose whose completion is a judgement call. The
mechanism is built, pinned, validated, and idle, so new work keeps closing on
evidence a reader interprets rather than evidence bound to the inputs tested.

Two facts constrain any fix.

- **MyQue cannot enforce this.** It is generic and knows nothing of Slime OS
  policy, so `myque new` will always be able to create a prose item.
  Enforcement can only be a repository check that refuses afterwards, which is
  the boundary the earlier record already drew: which items are subject is
  Slime OS's own question.
- **Gate identities do not scale by hand.** `just --summary` exposes 158
  `_check` targets, and they share no machine-readable result protocol; each is
  a bespoke script whose only contract is an exit status. The one existing
  automated gate reduces targets to booleans through `subprocess.run(["just",
  target])` and recovers its single integer observation by string-matching
  stdout. Declaring typed observations for 158 targets before the policy can
  bind is not a realistic precondition.

## Decision

Two changes, both inside the ownership the earlier record claims.

1. **A fixed identity cutoff.** An item whose canonical UUIDv7 timestamp is at
   or after `1790812800000` ms (`2026-10-01T00:00:00Z`) must carry a
   `devloop` consumer record; `just tasks_check` fails otherwise. Enforcement is
   hard — no exemption by `kind`, `tag`, or state.

   The cutoff is an announced future date rather than the identity of the
   planning item that proposed this policy. Deriving it from that item was tried
   first and was wrong: a prose planning item created 2 minutes 46 seconds later
   (`01a0aefb`, from a concurrently merged pull request) already sat after it, so
   the rule would have retroactively refused an item that was legitimate when
   written. The interval between proposing this policy and implementing it must
   stay open to ordinary prose work, and its length is not knowable in advance.

   The date leaves 13.5 days of margin over the newest identity in the store, and
   no existing item falls at or after it. Enforcement must land before the date;
   if it slips, the date moves rather than refusing work written under the old
   rule.

   The discriminator is the UUIDv7 timestamp rather than the `created`
   frontmatter because identity is what MyQue allocates and what every durable
   reference resolves through, so it cannot drift from the item it names. This
   was verified against the whole store: all 310 items are UUIDv7, and no item's
   `created` field disagrees with its embedded timestamp by more than a day. It
   also resolves from a terminal record's filename alone, so the rule survives
   retirement and stays offline.

2. **A generic target gate.** A gate identity that takes the `just` target name
   from devloop's execution `inputs` and reports one `passed` boolean from its
   exit status, so an acceptance can bind to any existing target without
   hand-declared observations. `work-item-store` and its five typed observations
   remain as they are; a check deserving richer observations earns its own gate
   identity on demand.

## Alternatives and trade-offs

- **Leave it optional and rely on convention:** no work, but the measured
  outcome of that policy is already known — zero adoption.
- **Exempt `bug`, `backlog`, and planning items by kind:** gentler, but the
  exemption is a legitimate bypass that any item can claim by choosing a kind,
  which makes the rule advisory again.
- **Retrofit the live prose items:** uniform store, but it would fabricate
  requirements for work whose context is historical, and backfilled
  observations are exactly what the evidence model exists to refuse.
- **Declare typed observations per target before enforcing:** the strongest
  evidence, but it blocks the policy behind 158 bespoke adapters and would
  likely never land.
- **A soft warning instead of a failure:** zero friction, and zero enforcement.

## Consequences

- New work costs more up front: a `dev-spec/v1` payload must be authored and
  admitted before implementation, for defects and small tasks alike.
- `just tasks_check` stops being free. Each spec-driven validation compiles and
  links a native binary, so its cost grows linearly with the number of **live**
  spec-driven items. The current live spec-driven count is zero, which makes
  this the cheapest possible moment to start; keeping retirement current is now
  a performance concern, not only hygiene.
- Every pre-cutoff prose item is grandfathered by identity. Reopening one
  never makes it subject, because its UUIDv7 timestamp is fixed. This is a
  deliberate limit, not an oversight: a reopened item's requirements are
  historical, and rewriting them as a spec would invent scope nobody agreed to.
- Gate policy no longer enumerates every command a gate may run, since the
  target arrives in operator-supplied `inputs`. Requirement text still carries
  no command, which is the property the earlier record actually asserts, but the
  weaker form should be stated plainly rather than glossed.
- A direct `myque close` without `--expected` still bypasses devloop
  eligibility. A `done` state alone remains insufficient to prove enforcement,
  unchanged by this record.

## Revisit conditions

Revisit if `just tasks_check` becomes impractical as a routine gate as the live
spec-driven count grows; if authoring overhead measurably suppresses the
recording of small defects, which would trade one honesty problem for another;
if the generic gate's pass/fail reduction proves too coarse to express real
acceptance predicates; or if a second consumer needs a different body profile.

## References

- [`spec-driven-work-item-bodies.md`](spec-driven-work-item-bodies.md) — the
  record this one amends.
- [`development-record-ownership.md`](development-record-ownership.md) — the
  underlying record split.
- `AGENTS.md`, `CONTRIBUTING.md` — work-item and contributor rules.
- [`docs/getting-started/06-work-item-lifecycle.md`](../getting-started/06-work-item-lifecycle.md)
  — both lifecycles, command by command.
- `.devloop/policy.json`, `scripts/check/devloop-gate.py` — gate identities and
  the adapter that resolves them.
- `scripts/check/check-work-items.py` — where the cutoff is enforced.
