# `sel4-operation.zti` — the C8.7 bounded-native-operation generation

An eleventh seL4 generation, beside [`sel4.zti`](sel4.md),
[`sel4-channel.zti`](sel4-channel.md), [`sel4-loan.zti`](sel4-loan.md),
[`sel4-spawn.zti`](sel4-spawn.md), [`sel4-sample.zti`](sel4-sample.md),
[`sel4-stream.zti`](sel4-stream.md),
[`sel4-supervision.zti`](sel4-supervision.md),
[`sel4-crossing.zti`](sel4-crossing.md), [`sel4-call.zti`](sel4-call.md),
`sel4-qos.zti`, and the frozen x86 [`valid.zti`](valid.zti). It declares the
C8.7 operation graph — the `navigation` route with two clients, a supervised
replacement for the second, a server, and a capability-routed clock, plus client
A's private `nav-backup` route — for P5.4.7.

The plane reuses [`sel4-call.zti`](sel4-call.md)'s composition: `init` mints one
authenticated control pair per participant, keeps the participant half, spawns
the graph, and transfers each participant's supervision handle to the broker over
that participant's own channel. The parent vouches; no participant holds
authority naming itself. Everything that document says about *why the control
channels are declared but not materialized as edges* applies here verbatim, with
`FABRIC_OPERATION_CONTROL_GRANTS` in place of the call plane's table.

Recorded below is only what differs.

## Why generation 20

Generation 18 is the seL4 call plane and 19 the seL4 QoS plane. Generation 15 is
the **x86** operation plane and keeps its own layout. Numbering this 20 rather
than reusing 15 is what stops the seL4 image from walking the x86 table — the
same failure the first version of the call plane hit.

## Six executables, because the restart is a declared identity

C8.7 requires participant restart to be deterministic. That is expressed in the
graph rather than in the scenario: `fabric-op-client-b-restart` is its own
component, its own route participant, and its own control grant. The broker parks
on the replacement's authenticated control while client B's slot is vacant, and
admits the replacement there — keeping the authenticated client index, the
correlation high-water mark, and the retained results, while minting a *fresh*
non-delegable role.

So this fixture declares five control grants where the call plane declares four,
and `init` mints a fifth pair for the replacement. The broker reads that fifth
slot as the literal `FABRIC_FIRST_CONTROL_SLOT + FABRIC_OPERATION_CLIENTS.len()`,
which is why the replacement's pair is granted immediately after the four the
`FABRIC_OPERATION_CLIENTS` table names.

## Why the fabric's executable is *not* transferable

`init-fabric-service` declares `transferable = false` here and `true` on the call
plane. `SpawnPlan::transferable_supervision` reads `RIGHT_TRANSFER` off the
executable, so a handle is movable only where a composition needs it moved.

The call plane moves the fabric's handle to its client and server so each can name
the fabric as the receiver of a shared-payload loan. No operation participant does
that: every C8.7 record is inline, so no participant ever needs to name the
fabric. The four handles that *are* delegated are the participants' own — both
clients, the replacement, and the server — which is what the broker authenticates
against. The clock's handle stays with `init`.

The layout and the fixture agree bit for bit (B10): `SEL4_OPERATION_LAYOUT` gives
those four `0x1000c` and leaves `fabric-service` and `fabric-op-time` at
`0x10008`.

## The shared-buffer budget

One holder: `fabric-service`, at 4 pages and 2 buffers/mappings/loans. That is
the graph's declared `limits`, which `resolve_fabric_profile` checks against the
holder quota, and it is a floor rather than a measurement — `sampleBytes` is
8192, so the aggregate validator requires at least one buffer and enough pages to
represent one maximal sample even though this plane never sends one.

No participant declares a quota. The operation records are all inline
`WireOperationEnvelope`s at exactly `MAX_MSG`; nothing here allocates.

## The graph's operation ceilings

`inFlightOperations = 4`, `retainedSamples = 4`, `eventDepth = 8`, `retries = 2`,
and `inFlightCalls = 0`. The first three are the numbers the broker sizes its
fixed arrays from — `MAX_OPERATIONS`, `MAX_RETAINED`, and the per-client pending
share — so they are the same as the oracle's rather than tuned for this plane.
`inFlightCalls` is zero because this graph declares no call route.

`ingressSources = 5`: four route participants plus the clock, which is the
`operation` route worker's `graphDerived` count. The broker's own park set peaks
at nine and is declared in `FABRIC_WORKER_WAIT_SHAPES` rather than derived here.

## The boot layout

`scripts/build/boot_layout.py`'s `SEL4_OPERATION_LAYOUT`, eight rows: the two
factories, then the fabric and the six participants. Frozen as
[`sel4-operation.layout`](../../../boot-layout/v1/fixtures/sel4-operation.layout)
and checked by `just sel4_boot_layout_check`.

No control channel appears in the layout: they are minted at runtime, so the
table numbers only what the generation places.
