# `sel4-call.zti` — the C8.6 bounded-native-call generation

A tenth seL4 generation, beside [`sel4.zti`](sel4.md),
[`sel4-channel.zti`](sel4-channel.md), [`sel4-loan.zti`](sel4-loan.md),
[`sel4-spawn.zti`](sel4-spawn.md), [`sel4-sample.zti`](sel4-sample.md),
[`sel4-stream.zti`](sel4-stream.md),
[`sel4-supervision.zti`](sel4-supervision.md),
[`sel4-crossing.zti`](sel4-crossing.md), and the frozen x86
[`valid.zti`](valid.zti). It declares the C8.6 call graph — one `ParameterCall`
route, two clients, a server, and a capability-routed clock — for P5.4.6.

The plane uses parent-vouched post-spawn introduction: `init` retains the
participant half of each minted control pair, spawns the broker and
participants, then transfers each participant's supervision handle to the
broker over that participant's authenticated channel. This is the seL4
composition that closes backlog **B25** while preserving the x86 call model.

## Why generation 18

The seL4 generations before this one are all generation 1, because each declares
its own graph and none needs a distinct boot-layout table. This one does: its
`init` holds two factories and five executables in a layout that shares nothing
with the base table, so `boot_layout.py` carries `SEL4_CALL_LAYOUT` as the
generation-18 **replacement** rather than an override.

Generation 14 is the x86 call plane and keeps its own layout. Numbering this 18
rather than reusing 14 is what stops the seL4 image from walking the x86 table —
the failure mode the first version of this plane hit.

## The control channels are declared native Endpoints

This is the fixture's one non-obvious design point, and it is the one earlier
versions got wrong in both directions.

The four `fabric-call-*-control` grants are how the components get their
endpoints: each is a native seL4 Endpoint the root materializes, installing one
half into the fabric and the other into the participant, at the slot each
instance's own binding names. Init holds no route capability and places nothing.

Two facts about that arrangement are load-bearing:

- **The grants carry identity as well as authority.** `_control_sources`
  (`scripts/build/build-generation.py`) derives `FABRIC_CALL_CLIENTS` — the table
  the broker maps a control slot to a caller *identity* with — from exactly those
  four grant names, in `FABRIC_CALL_CONTROL_GRANTS` order rather than the
  builder's `(name, source, target)` sort. Deleting them empties that table and
  trips `request_response_controls`' four-control assert before the broker reads
  a single slot.
- **The declared slots must be contiguous from the broker's base.** An earlier
  version had init mint the pairs instead, because a root-materialized grant then
  landed at the fabric's own channel cursor — which resumed *above* the factory
  grants staging installed, so the fabric received its controls at
  `[0, 3, 4, 5, 6]` while the broker addresses them as
  `FABRIC_FIRST_CONTROL_SLOT + index`. Each binding now names its slot directly,
  so the declared numbering is the numbering the broker compiles against, and the
  builder's sort order cannot rebind one client's identity to another's slot.

So the generation both names and authorizes each control, and the binding between
a control endpoint and a component identity is a manifest fact no component can
forge.

## Why four executables are `transferable`

`init-fabric-service` and the three call participant executable grants declare
`transferable = true`; the clock does not. `SpawnPlan::transferable_supervision`
reads `RIGHT_TRANSFER` off the **executable**, so the returned supervision
handle is movable only when its composition requires delegation.

The fabric handle is passed to the client and server so each can name the
fabric as a loan receiver. Each participant handle is passed by init to the
broker after that participant sends its role request, authenticating the caller
without ambient task ids or requiring a component to hold authority naming
itself. The clock's supervision handle is granted to the broker the same way,
spawned before it for the same reason (B76): `fabric-call-time` is a
separately declared instance, so nothing but its own handle reports its exit,
and the broker's exit predicate waits on that report. Its *executable* grant
is the one that stays with init and needs no transfer authority -- nothing ever
delegates the authority to spawn the clock.

The layout and the fixture agree bit for bit (B10): `SEL4_CALL_LAYOUT` gives
`fabric-service` and the three participants `0x1000c`, while
`fabric-call-time` remains `0x10008`.

## The shared-buffer budget

Three holders: `fabric-service` (24 pages, 12 buffers/mappings/loans), and the
client and server (4 pages, 2 each). The fabric's figures are the graph's
declared `limits`, which `resolve_fabric_profile` checks against the holder
quota; the participants' are what one `>MAX_INLINE_BYTES` call payload costs
each way.

`fabric-call-client-b` and `fabric-call-time` declare no quota. Neither
allocates: client B exercises duplicate, cancellation, stale-session, and
terminal-backpressure arms, all inline, and the clock sends only
`WireCallTimeAdvance`.

## The boot layout

`scripts/build/boot_layout.py`'s `SEL4_CALL_LAYOUT`, seven rows: the two
factories, then the fabric and the four participants. Frozen as
[`sel4-call.layout`](../../../boot-layout/v1/fixtures/sel4-call.layout) and
checked by `just sel4_boot_layout_check`. The layout is dumped between channel
materialization and activation, so a transferability mismatch is rejected
before runtime provisioning begins.

No control channel appears in the layout, for the reason above: they are minted
at runtime, so the table numbers only what the generation places.
