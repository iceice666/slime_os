# `sel4-boot.zti` — the C8.10 full-graph boot generation

A thirteenth seL4 generation, and the widest: every C8 role in one boot. The
stream plane, the call plane, the operation plane, an unauthorized probe, a
declared interposition proxy, and a filtered-introspection client, launched
concurrently rather than as mutually exclusive profiles of one manifest.

Twenty components, thirty-nine grants, five routes, four interface schemas. It
is the seL4 counterpart of the oracle's generation 17.

## Why generation 22

18, 19, 20, and 21 are the seL4 call, QoS, operation, and visibility planes.
Generation 17 is the **x86** full-graph boot and keeps its own 53-row layout.

## The profile is named `unified`, and that is load-bearing

`resolve_fabric_profile` selects `FABRIC_BOOT_STREAM_CONTROL_GRANTS` — the
seven-control table including the observer, probe, and proxy — when the resolved
profile name is `unified`, and `FABRIC_STREAM_CONTROL_GRANTS` (five) otherwise.

Naming this profile `sel4`, as every other seL4 fixture does, produced a
five-control table. The fabric then computed its worker executable slots from the
wrong base and both `spawn_route_worker` calls were refused on an ungranted slot.
The name is the selector; it is not decoration.

## The controls are declared, so the layout has 21 rows and not 53

This is the one structural difference from the oracle's generation 17, and it is
the same fact every seL4 plane since P5.4.6 has recorded.

The oracle's `FABRIC_BOOT_LAYOUT` numbers both halves of all sixteen control
channels, because its kernel materializes a declared channel into the bootstrap
component's layout slots. Nothing here does: a control never passes through
init, so it never occupies an init layout row.

Observed directly: an early version of this plane had init place the controls,
the fabric received its ends at cursor positions, and every worker spawn failed.

So every control is a generation-declared native seL4 Endpoint instead: the root
installs each half into the instance whose binding names it, at that binding's
own slot, before anything runs. Init therefore holds no route capability and
places nothing — `[init] fabric boot graph spawned with static endpoints`. The
grants also stay load-bearing for identity, because `_control_sources` derives
the identity tables from those names, and `SEL4_BOOT_LAYOUT` numbers only the
twenty-one things the *generation* places: two factories, three service and
worker executables, and sixteen participant executables.

The C8.10 property a boot layout can carry is intact: all three planes'
executables in disjoint slots, no profile-dependent rewrite.

## The two route workers

`fabric-service` declares `spawnBudget = 2` and receives both worker executables
as `exec`/`spawn` grants; it spawns them itself. That is C8.10's bounded-worker
half, and the reason is the wait bound: the declared peaks are stream 8, call 7,
operation 9 against `MAX_WAIT_SOURCES = 9`, so one combined task would have to
poll.

Each worker's executable grant targets **the worker**, not the fabric. Targeting
`fabric-service` — which is how the oracle's two `*-worker-executable` grants
read at a glance — makes both resolve to the fabric's own layout slot, and the
second is refused with `WaiterConflict` before the boot reaches init.

## Sixteen participants transferable, the fabric and workers not

`0x1000c` on all sixteen: init delegates the call and operation participants'
supervision handles to their workers over authenticated controls, and declaring
the whole participant set transferable keeps the table one rule rather than a
per-plane exception. The fabric and its two workers stay `0x10008` — their
handles never move.

## What this generation cost the root

Two bounds, both sized against single-plane graphs:

- `channel::MAX_CHANNELS` 32 → 48. The peak here is 37 live: sixteen participant
  controls, fourteen stream role channels, three call, four operation.
- `task::MAX_TASKS` 32 → 48. The peak here is 37 live tasks: the root launches
  all twenty declared components (P5.2) and init then spawns seventeen of them
  again as the composition's own children.

Both are recorded at their definitions and in
[`devlog/2026-08-08-p5-4-9-full-graph-boot/`](../../../../devlog/2026-08-08-p5-4-9-full-graph-boot/index.md).

## The boot layout

`scripts/build/boot_layout.py`'s `SEL4_BOOT_LAYOUT`, twenty-one rows. Frozen as
[`sel4-boot.layout`](../../../boot-layout/v1/fixtures/sel4-boot.layout) and
checked by `just sel4_boot_layout_check`.
