# 32. Scheduling class and QoS authority

| | |
| --- | --- |
| Status | parked |
| Route | lifecycle |
| Depends on | M6 spawn service and per-spawner resource accounting ([entry 25](25-resource-accounts.md)); shares the scheduler attribution point with [entry 17](17-energy-accounting.md) |
| Enables | interactive foreground responsiveness under mixed foreground/background load; latency isolation between daily-use tools, containers, and agents |
| Now | Design note legal today; kernel work waits on M6 and the resource-account object. |

## Motivation

The capability model governs *what* a component may do and
[entry 25](25-resource-accounts.md) governs *how much* it may hold, but
neither governs *when* it runs. On a daily-driver running native tools,
containers, and background agents at once, this is a real gap:
`MAX_TASKS` is a flat count and the scheduler treats every task alike,
so a busy-looping container or a batch-inference agent competes for CPU
with the interactive foreground on equal terms. Latency isolation
cannot be a runtime accident; on a mixed-workload machine it must be a
declared, auditable, rollbackable property — a scheduling class carried
as generation data like every other resource decision.

Entry 17 makes *energy* a budget; this entry makes *responsiveness* a
class. They share the scheduler as attribution point but answer
different questions: "which component drained the battery" versus "why
did the UI stutter while the agent ran."

## What exists today

- The scheduler exists (M1/M2: preemptible tasks, APIC timer) and
  already tracks per-task run time as a scheduling concept
  ([entry 17](17-energy-accounting.md) notes this), but exposes no
  priority, deadline, or class — every task is equal.
- [entry 25](25-resource-accounts.md) introduces the ResourceAccount
  object whose split/conservation rules a scheduling-share dimension
  naturally extends.
- [entry 17](17-energy-accounting.md) already establishes the
  policy-in-userspace boundary for throttling; QoS enforcement must
  respect the same policy-free-kernel invariant.
- Nothing today lets a manifest say "this component is foreground" or
  "this agent is best-effort." [INFERENCE: from the absence of any
  scheduling-class row in the capability matrix.]

## Design sketch

A scheduling class is manifest data attached per component (or per
supervision subtree): at minimum a coarse foreground / normal /
best-effort tier, possibly a share weight and an optional latency
target. The kernel owns only the mechanism — it enforces the ordering
the class implies — while the *assignment* of classes is generation
policy, and any dynamic re-classification (a background container
promoted to foreground when focused) is a userspace policy service
decision, not a kernel one.

Whether a scheduling class is *authority* is the design's central
question, mirroring entry 17's authority-versus-telemetry split. If
declaring yourself foreground lets you starve peers, the class is
authority and belongs in the rights grammar (a component cannot widen
its own class beyond its grant); if the class only biases a userspace
policy service that the kernel consults, it stays declarative data
outside the matrix. The mixed-workload exit condition — foreground stays
responsive while a background container saturates CPU — implies the
authority reading: the container must not be *able* to claim foreground
share it was not granted.

Interaction to design carefully: the resource account
([entry 25](25-resource-accounts.md)) bounds *quantity* of CPU share
while the class bounds *ordering*; a component might hold a large share
account but a best-effort class, or vice versa. Supervision restarts
([entry 8](08-declarative-supervision.md)) must preserve class across
restart. The guest VM / personality of
[entry 31](31-compat-personality.md) is the primary consumer: a
container's class is how the daily-driver keeps it from harming the
interactive foreground.

## Open questions

- Is scheduling class authority (rights-gated, non-wideable) or
  declarative policy data consulted by a userspace service?
- Class granularity: coarse tiers, share weights, explicit deadlines,
  or a combination — and how much of that belongs in the kernel
  mechanism versus a userspace policy service?
- Relationship to the resource account: is CPU share an account
  dimension ([entry 25](25-resource-accounts.md)) with the class as an
  ordering hint, or two independent axes?
- Dynamic re-classification (focus follows the user): who holds the
  authority to promote a component's class at runtime, and is that a
  capability?
- Does a container/guest ([entry 31](31-compat-personality.md)) get a
  single class for the whole workload, or may it subdivide internally?

## Exit-condition sketch

On the Framework target, an interactive foreground component keeps its
declared latency while a background container saturates every CPU; the
container cannot claim foreground scheduling share it was not granted,
and each component's class is visible in the manifest.

## Probe guidance

Paper: define the scheduling-class schema in the manifest, decide the
authority-versus-policy question against the mixed-workload scenario,
and specify the kernel mechanism (what ordering guarantee the class
buys) versus the userspace policy surface (assignment and dynamic
re-classification). Evaluate the class dimension against
[entry 25](25-resource-accounts.md)'s account model so CPU quantity and
CPU ordering compose rather than collide. Hardware validation waits for
M7.
