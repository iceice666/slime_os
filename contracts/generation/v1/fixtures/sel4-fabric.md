# `sel4-fabric.zti` — the P5.5.1 typed-fabric generation

A seventh seL4 generation, beside [`sel4.zti`](sel4.md),
[`sel4-channel.zti`](sel4-channel.md), [`sel4-loan.zti`](sel4-loan.md),
[`sel4-spawn.zti`](sel4-spawn.md), [`sel4-sample.zti`](sel4-sample.md), and the
frozen x86 [`valid.zti`](valid.zti). It declares the graph that carries P5.5's
exit condition: one typed route from a publisher to a subscriber, with both
route endpoints provisioned by the fabric rather than declared as edges.

## Why a seventh generation

The same mechanical reason as the four before it: `init.rs` selects its scenario
with `option_env!`, resolved at compile time, so one component build cannot
serve two gates.

## The first seL4 generation with a fabric graph

Every earlier seL4 manifest declares no `fabricGraph`, and
`build_sel4_generation` wrote an *empty* fabric profile for them. That was fine
while no seL4 graph ran the fabric: `fabric-service` reads its route table,
participant list, and control-slot base out of the generated profile at compile
time, so an empty one is a component that does not build rather than one that
runs without routes.

This manifest declares one, and the builder now resolves it through
`resolve_fabric_profile` — the same function every x86 profile goes through, so
a seL4 route identity, QoS row, and control-slot base are folded from the same
schemas and the same validation rather than from a second implementation. The
authenticated C8.2 `fabric-graph` resource is carried in the generation too. The
root task never reads it (the fabric is userspace policy and the root knows
nothing of routes), but it is part of the object closure the root re-checks, so
a graph the builder resolved and then failed to carry would fail admission.

## The four participants, and how many are unmodified

- **`fabric-service`**, **`fabric-publisher`**, and **`fabric-intruder`** run
  **unmodified** — the same binaries the x86 oracle builds, with no seL4 branch.
  `fabric-service` was expected to need one and does not: its `ROUTE_NAMES` is
  its own constant rather than the profile's, so `main` provisions and brokers
  whatever subset of those routes the generation declares, and a route the graph
  does not name simply has no participants.
- **`fabric-subscriber`** carries exactly **one** guarded branch. It refuses to
  finish until *both* sample forms arrive, and the `>MAX_MSG` one comes from
  `fabric-publisher-b`, which this graph does not declare. The branch relaxes
  that one condition and renames the marker so the transcript cannot claim a
  shared sample arrived. P5.5.2 restores it by declaring the second publisher,
  not by editing the component.

The gate asserts those counts **exactly**, not as a ceiling: a component that
grew a second branch fails it, and so does one whose branch was removed without
the graph that made it unnecessary.

## Why the route declares no channel edge

Init holds no route capability at all. It mints one control channel per
participant through its declared `endpointCreate` grant and hands each client
one half, keeping the other for the fabric. The binding between a control
endpoint and a component identity is established exactly there, at spawn, and it
is what the fabric authenticates against — a client cannot forge, share, or
re-derive one, so "which component is asking" is a capability fact rather than a
claim in a message.

The *route* endpoints are minted by the fabric, from its own granted factory,
and moved to each participant through `cap_transfer`. That is the milestone: a
participant never holds a route endpoint directly but is provisioned one.

## The numbers, and why each

`init` declares `spawnBudget = 4` — exactly the four children this composition
needs. Unlike [`sel4-sample.zti`](sel4-sample.md) this is not also a denial arm;
B14's refusal is already observed there, and adding a fifth spawn here purely to
re-observe it would test the budget rather than the fabric.

**`fabric-subscriber` declares `historyDepth = 8`**, copied from `valid.zti`
rather than chosen. The publisher sends seven samples — two inline, four
stall-window, one terminal — and it sends them unconditionally, because on x86
the stall-window four exist to overrun a *second* subscriber's shallower ring.
A depth of 4 here made this keeping-up subscriber lose samples it had already
acked and fail its own `SAMPLE_LOST` assertion. The oracle's number is the right
one for the oracle's publisher.

The `fabric-service` shared-buffer budget (`4 / 2 / 2 / 2`) bounds the one copy
each large sample makes. This graph brokers only inline samples so it is never
charged, but `build_fabric_graph` requires the fabric holder to have a quota and
checks each `limits` entry against it — a fixture that declared none would fail
the build rather than the boot.

## Wait-set headroom, for P5.5.2

Both of the fabric's park sets clear `MAX_WAIT_SOURCES = 9` comfortably:
`park_on_controls` walks the unanswered clients (3) and `park_on_streams` walks
live publishers plus subscriber acks (2). The C8.10 worker split exists because
the full graph's three planes need 8, 7, and 9 at once; this slice is nowhere
near that, so P5.5.2 has room to grow the stream plane without partitioning it.

## What the root still launches

The root launches every component the generation declares (P5.2), so this boot
also starts one unconfigured instance of each of the four, holding no control
endpoint. Each fails its own first operation and exits non-zero.

That is expected, and the gate handles it by **identity rather than by time**.
P5.3.4's gate could use a transcript window because its unconfigured pair failed
before the composition began; here the four unconfigured instances are activated
alongside init's four children and interleave freely with them — the
unconfigured service fails its first `endpoint_create` *while* the composition
is still brokering. A window would admit that failure or exclude a real one
depending on scheduling. So the gate counts instead: each component name appears
twice per boot, the unconfigured instance contributes exactly one failure, and a
second from the same name is necessarily a participant init spawned.

## Relationship to the backlog

- **B15 is closed** by this slice, though not by this fixture: the six-grant
  spawn that observes its exit condition lives in
  [`sel4-spawn.zti`](sel4-spawn.md)'s scenario, which is where a spawn-ABI bound
  belongs. This graph's largest grant list is the fabric's five, which would
  have been refused before the fix.
- **B17 is opened** by this slice: the transfer's subset test has no coverage
  here, and no graph this cutover can declare reaches it. Recorded rather than
  papered over.
- **B16** (termination records are never reclaimed) does not bite: this graph
  creates nine tasks against `MAX_RECORDS = 32`.
