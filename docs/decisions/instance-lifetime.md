# Instance lifetime

**Status:** Proposed
**Related work items:** `01a0ed72-626c-7675-af57-8510e473b774`,
`01a0ebea-7ffe-7ca1-956f-006c13dd7251` (sel4-net),
`01a0ec3a-a91f-7349-a28a-26a60d8d7f4e` (entropy authority)

## Context

The root treats every clean exit as completion. A required instance that exits
0 is marked completed in `slime-root/src/graph_runtime/services.rs`, and a graph
whose required instances have all exited certifies with
`SLIME_GRAPH HEALTHY ... live=0`. Nothing in the generation says an instance is
meant to keep running.

Each service therefore decides on its own when to stop. network-service marks a
client closed once its session closes or aborts, and releases its link and exits
when every client is closed; every single-session network plane relies on this.
init's `supervise_resident()` hard-codes a residency rule, but only for the
product components init launches itself, and only by treating their termination
as init's own failure.

Two planned compositions need a service that stays up: sel4-net, where each
spawned http-get must find network-service still serving, and sel4-entropy,
whose entropy-service has no reason ever to stop.

## Decision

Lifetime is a declared composition fact with two values:

- **resident**: the instance runs for the life of the graph. Its exit is never
  completion.
- **bounded**: the instance may finish its declared work and exit. This is
  today's behavior.

It is authored like `health`. The component spec carries the default and a
system-spec placement or instance may override it, because the same component
is resident in one composition and bounded in another. Every existing component
spec declares `bounded`.

The generation carries it as a new object defined by
`contracts/instance-lifetime/v1`, emitted only when a composition declares a
resident instance. Each row names a resident instance by holder identity. A
generation without the object has every instance bounded.

The root enforces it:

- A resident required instance that exits, with any status, ends the graph with
  a typed failure naming the instance and its status.
- A resident optional instance that exits is recorded as unhealthy. Its
  supervisor sees that outcome instead of a clean exit.
- A resident instance never counts toward `completed`.
- Faults keep their existing path.

A component reads its own lifetime with `slime_rt::lifetime()`, a root-served
query answered from the authenticated generation, as `spawn_budget()` is. A
service changes its own behavior on that answer. For example, network-service
keeps admitting new attachments instead of releasing its link.

## Qualification

`just sel4_lifetime_check` runs the `lifetime` arm of
`scripts/check/check-sel4-lifecycle-restart-plane.py` over two compositions:

- **`sel4-lifetime`** (generation 166):
  - `lifetime-holder` is resident and required, and stays live.
  - `lifetime-worker` is bounded and required, and exits 0.
  - `lifetime-owner` is bounded and required. It spawns `lifetime-quitter`,
    which is resident and optional and exits 0.
- **`sel4-lifetime-exit`** (generation 167): `lifetime-leaver` is resident and
  required, and exits 0.

The root answers each lifetime query with
`SLIME_GRAPH lifetime task=<n> instance=<name> lifetime=<value>`. It records the
quitter as
`SLIME_GRAPH resident exit instance=lifetime-quitter status=0 recorded=unhealthy`.
Once every bounded required instance has completed, it certifies
`SLIME_GRAPH HEALTHY generation=166 required=<r> live=1 completed=<r-1> failed=0`.
The exit boot ends on
`SLIME_ROOT FATAL SLIME_GRAPH FAIL resident instance lifetime-leaver exit status=0`.
The probe states its role and the lifetime it was told as
`[lifetime-probe] role=<role> lifetime=<value>`, and the owner states the
quitter's outcome as `[lifetime-probe] role=owner quitter outcome=unhealthy`.
The fixtures carry each instance's resolved `lifetime`.

## Alternatives

- **A field in each service's own contract**, such as a network-application/v4
  `sessions` column. The change would be local, but entropy-service and every
  later service would redefine the same concept, and the root could not enforce
  any of them.
- **A new instance-record field in generation/v6.** This is the most direct
  encoding, but it re-encodes every generation and every closure for one value
  that most compositions never set.
- **Inferring residency from bindings or ownership**, for example "root-owned
  services are resident". This needs no contract, but it is implicit: the same
  facts already hold for bounded plane services, so the rule would be wrong on
  day one.
- **Root-free, advisory lifetime** that components read but the root does not
  check. This is smaller, but a service that quietly exits would still certify
  the graph, which is the failure this decision exists to catch.

## Consequences

- No existing composition changes: every instance stays bounded until a
  composition declares otherwise.
- A resident instance cannot be stopped in this version. A composition that
  needs to shut an instance down, such as the product graph's spawn-service and
  console, which init shuts down after Slisp exits, keeps it bounded.
- A graph with a live resident required instance never reaches `live=0`.
  A plane that certifies on `live=0` must not declare one.
- The root gains one query label and one object decoder. Each service keeps its
  own session policy; the root states only whether exiting is allowed.

## Revisit when

- A composition needs to stop a resident instance on purpose. The expected
  mechanism is an authorized stop carried by the supervision handle, after
  which the exit counts as orderly.
- A third lifetime value is needed, for example "restart on exit", which would
  overlap the C9.4 lifecycle restart policy.
- Resident services become the common case, at which point the component-spec
  defaults for services such as network-service may flip to `resident` and the
  single-session planes declare `bounded`.

## Code references

- `slime-root/src/graph_runtime/services.rs`: exit accounting and certification.
- `components/services/network-service/src/main.rs`: session-driven exit.
- `components/system/init/src/main.rs`: `supervise_resident()`.
- `contracts/system-spec/v1/schema.zt`: placement overrides (`owner`, `health`).

