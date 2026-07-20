# 25. Resource accounts as capabilities

| | |
| --- | --- |
| Status | parked |
| Route | lifecycle |
| Depends on | M6 spawn prerequisites (endpoint minting, non-consuming derive-copy, supervision handles); subsumes the matrix horizon rows for per-spawner accounting and SharedBuffer creation quota |
| Enables | declarative whole-machine resource allocation; bounded spawn authority; restart-storm bounding for [entry 8](08-declarative-supervision.md) |
| Now | Design note legal today; kernel work waits on M6 (minimal stub). |

## Motivation

The capability model governs what a component may do but not how much
it may use: `MAX_TASKS`, `MAX_CAPS`, and `CHANNEL_QUEUE` are global
constants, and the capability-matrix debt register already records
unreaped task-table entries. The matrix horizon names per-spawner
accounting and SharedBuffer creation quota without a unifying story.
Introduce a `ResourceAccount` kernel object (memory pages, task slots,
endpoint slots, queue depth) that spawn charges to the spawner's
account; `derive` splits an account for a child, exhaustion is a
structured error, and child exit returns quota to the parent.

Whole-machine resource allocation becomes generation manifest data:
declarative, auditable, and rollbackable like every other grant.

## What exists today

- The gap is measured in the repo itself: global `MAX_TASKS` /
  `MAX_CAPS` / `CHANNEL_QUEUE` constants, and the debt register's
  unreaped task-table entries — table slots leak until reboot, so even
  global accounting is currently approximate.
- The horizon lists exactly the rows this entry subsumes: per-spawner
  resource accounting (an M6 spawn prerequisite) and SharedBuffer
  creation with CREATE / quota.
- Genode's resource trading is the reference design; the Slime delta is
  carrying the account distribution as rollbackable generation data.
- M6 (stub) needs per-spawner accounting regardless; this entry is the
  unifying design rather than an additional feature.

## Design sketch

A `ResourceAccount` is a kernel object holding a vector of quantities
(memory pages, task slots, endpoint slots, queue depth). Spawn charges
the child's initial resources to the spawner's account; `derive` splits
an account — moving a bounded sub-quantity into a new account for the
child, never creating quantity — so conservation holds: the sum over
all accounts never exceeds the generation's declared total. Exhaustion
fails spawn/allocation with a structured error naming the exhausted
dimension; child exit returns its remaining quota to the parent account,
closing the debt register's leak by construction.

The generation manifest declares the initial account distribution —
which services get how much — and the builder bounds it against the
machine's actual capacities. Rollback restores the prior distribution
with the prior generation, so resource policy has the same rollback
semantics as every other grant.

Interactions worth the design note's attention: capability transfer
across components (does a transferred SharedBuffer move quota with it?),
supervision restarts ([entry 8](08-declarative-supervision.md) charges
each restart, bounding storms by quota), and the static audit story —
[entry 9](09-grant-graph-introspection.md) can answer "which components
could exhaust task slots" from the manifest.

## Open questions

- The horizon's underlying question: which resources are inside the
  account system (kernel table slots) and which stay outside (CPU time
  — entry 17's energy accounting is a separate axis)?
- Quota on capability transfer: does authority over a buffer imply its
  memory charge moves, stays with the creator, or is shared?
- Overcommit: may accounts sum beyond physical capacity with
  first-come allocation, or does the builder require hard conservation?
- Account hierarchy versus the supervision tree: same shape, or may a
  component hold accounts from multiple parents?

## Exit-condition sketch

A service holding a two-task account cannot spawn a third; a child's
quota returns to the parent account on exit; the generation manifest
declares the initial account distribution and the builder bounds it.

## Probe guidance

Paper: the design note — account object shape, split/conservation rules,
the manifest distribution format, and the builder bounding check —
evaluated against the current constants (`MAX_TASKS`, `MAX_CAPS`,
`CHANNEL_QUEUE`) as the initial quantities. Reference: Genode's resource
trading, with the delta (accounts as rollbackable generation data)
made explicit.

## References

- [Genode Foundations](https://genode.org/documentation/genode-foundations/)
