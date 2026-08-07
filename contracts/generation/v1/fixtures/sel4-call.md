# `sel4-call.zti` — the C8.6 bounded-native-call generation

A tenth seL4 generation, beside [`sel4.zti`](sel4.md),
[`sel4-channel.zti`](sel4-channel.md), [`sel4-loan.zti`](sel4-loan.md),
[`sel4-spawn.zti`](sel4-spawn.md), [`sel4-sample.zti`](sel4-sample.md),
[`sel4-stream.zti`](sel4-stream.md),
[`sel4-supervision.zti`](sel4-supervision.md),
[`sel4-crossing.zti`](sel4-crossing.md), and the frozen x86
[`valid.zti`](valid.zti). It declares the C8.6 call graph — one `ParameterCall`
route, two clients, a server, and a capability-routed clock — for P5.4.6.

**It does not pass.** The plane builds, admits its graph, mints and binds every
control channel, spawns all five components, and delivers each role request to
the broker; it then deadlocks on backlog **B25**. The fixture is committed in
that state deliberately, and the reason is in
[`devlog/2026-08-07-p5-4-6-call-spawn-semantics/`](../../../../devlog/2026-08-07-p5-4-6-call-spawn-semantics/index.md).

## Why generation 18

The seL4 generations before this one are all generation 1, because each declares
its own graph and none needs a distinct boot-layout table. This one does: its
`init` holds two factories and five executables in a layout that shares nothing
with the base table, so `boot_layout.py` carries `SEL4_CALL_LAYOUT` as the
generation-18 **replacement** rather than an override.

Generation 14 is the x86 call plane and keeps its own layout. Numbering this 18
rather than reusing 14 is what stops the seL4 image from walking the x86 table —
the failure mode the first version of this plane hit.

## The control channels are declared but not materialized as edges

This is the fixture's one non-obvious design point, and it is the one the first
version got wrong in both directions.

The four `fabric-call-*-control` grants are present. They are **not** how the
components get their endpoints: `init.rs::drive_call_plane` mints each pair with
`endpoint_create` and hands the halves out at spawn.

Both halves of that arrangement are load-bearing:

- **The grants must stay.** `_control_sources`
  (`scripts/build/build-generation.py`) derives `FABRIC_CALL_CLIENTS` — the table
  the broker maps a control slot to a caller *identity* with — from exactly those
  four grant names, in `FABRIC_CALL_CONTROL_GRANTS` order rather than the
  builder's `(name, source, target)` sort. Deleting them emptied that table and
  tripped `request_response_controls`' four-control assert before the broker read
  a single slot.
- **Init must mint the endpoints.** A grant the root materializes gives the
  fabric its ends from the root's channel cursor, which resumes *above* the
  factory grants staging installed — so the fabric received its controls at
  `[0, 3, 4, 5, 6]` while the broker addresses them as
  `FABRIC_FIRST_CONTROL_SLOT + index`. Minting numbers them `0..count` in grant
  order instead.

So the grants name and the minted endpoints authorize. That split is the whole
reason this fixture looks like it declares edges it does not use.

## Why `fabric-service` alone is `transferable`

`init-fabric-service` declares `transferable = true`; the four participant
executables do not. `SpawnPlan::transferable_supervision` reads `RIGHT_TRANSFER`
off the **executable**, and the fabric's supervision handle is the one init
passes on — granted to the client and the server so each can name the fabric as
a loan receiver. No participant's handle is delegated, so no participant's
executable needs the bit.

The layout and the fixture must agree bit for bit (B10): `SEL4_CALL_LAYOUT` gives
`fabric-service` `0x1000c` and the four participants `0x10008`, matching these
flags. The first draft had it inverted, and nothing running objected — the dump
reported the grant's rights rather than the layout's, which is backlog **B26**,
now fixed.

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
checked by `just sel4_boot_layout_check`, which is the one gate this plane does
pass — the layout is dumped between channel materialization and activation, long
before the deadlock, so B10's property is observable even though C8.6's is not.

No control channel appears in the layout, for the reason above: they are minted
at runtime, so the table numbers only what the generation places.
