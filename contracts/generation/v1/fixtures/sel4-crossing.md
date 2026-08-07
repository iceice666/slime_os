# `sel4-crossing.zti` — the B22 channel-crossing generation

A ninth seL4 generation, beside [`sel4.zti`](sel4.md),
[`sel4-channel.zti`](sel4-channel.md), [`sel4-loan.zti`](sel4-loan.md),
[`sel4-spawn.zti`](sel4-spawn.md), [`sel4-sample.zti`](sel4-sample.md),
[`sel4-stream.zti`](sel4-stream.md),
[`sel4-supervision.zti`](sel4-supervision.md), and the frozen x86
[`valid.zti`](valid.zti). It declares the smallest graph that can reach backlog
**B22**'s exit condition: *a graph that mints more than `MAX_CHANNELS` channels
over its lifetime still sends and receives correctly on every live channel.*

## Why a ninth generation

The same mechanical reason as the seven before it: `init.rs` selects its
scenario with `option_env!`, resolved at compile time, so one component build
cannot serve two gates. A separate generation — and so a separate image — is
required rather than preferred.

## The two components

### `init` — the whole plane

Holds one `exec | spawn` grant and one `endpointCreate` grant. It mints 33
channel pairs and releases each before minting the next, so live occupancy never
exceeds four while the lifetime count crosses 32. It also holds the four
long-lived channels the gate asserts about: a carrier, a release gate, the pair
whose end goes in flight, and one retained pair — 37 minted in total, which is
what the root's terminal `minted=` reports.

`spawnBudget` is 2 rather than 0: the plane spawns exactly one child, and the
budget bounds children *live at once*, so 2 is headroom rather than a count.

### `crossing-peer` — why a new binary was unavoidable

This is the one design point in the fixture that is load-bearing and
non-obvious, and it is the same shape as `supervision-child` in
[`sel4-supervision.md`](sel4-supervision.md).

The gate must hold a channel end in `Transit` — the state where a capability is
held by no capability table at all — across every sweep the loop triggers. That
is the case a predicate reading only `GraphTables` frees by mistake, and the
reason `Transit::holds_endpoint` exists.

Two facts make an existing component impossible:

- **Init cannot be its own peer.** `serve_cap_transfer` refuses
  `endpoint_slot == capability_slot`, and more fundamentally a transfer needs a
  *receiving task* whose `recv` ends the in-flight window. Init runs the loop,
  so it cannot also be blocked waiting to end it.
- **Every unmodified component drains immediately.** `console`, `sysinfo`, and
  the rest either ignore the capability array or receive on their only queue as
  their first act — which closes the window before the first sweep fires. The
  window must stay open *across* the loop, so the peer needs a second edge to
  block on first.

Hence two grants at spawn: the carrier the end arrives on (slot 0) and a gate
the peer blocks reading (slot 1). Init transfers, runs the loop, then writes the
gate; only then does the peer collect.

The cost is the same one `supervision-child` records: it weakens the "no new
component binary" property the other planes have. It does not touch the frozen
oracle, which is `kernel/` rather than `components/`, but it does mean
`contracts_check` and `generation_check` carry a real duty here.

## Why init drops its own half of the in-flight pair

The arm above is decorative unless the transit entry is the **only** thing
naming that channel. Init mints the pair as a loopback, transfers one end to the
peer, and then `cap_drop`s the other. Without that drop, init's remaining
capability keeps `GraphTables::holds_endpoint` answering true and the arm passes
with `Transit::holds_endpoint` deleted — which is exactly what the first version
of this plane did, caught by fault injection rather than by review.

## Why the graph declares no channel edge

Like [`sel4-spawn.zti`](sel4-spawn.md) and
[`sel4-supervision.zti`](sel4-supervision.md), and for a sharper reason here: a
*declared* edge is materialized at boot and held for the life of the graph, so
it is exactly the thing the sweep must never reclaim. All 37 channels are minted
at runtime through the declared factory, so what distinguishes a reclaimable one
from a live one is holder state rather than provenance — which is the property
the sweep derives from.

## The boot-layout row

`scripts/build/boot_layout.py` gains row 62, `crossing-peer`, rights `0x10008`
(`exec | spawn`). Not `0x1000c` as `supervision-child` carries: that row's extra
`0x4` is `RIGHT_TRANSFER`, which its fixture declares `transferable = true` and
this one does not. The layout and the fixture must agree bit for bit (B10), so
the two must move together in either direction.

The profile prunes to 16 slots. Role rows and channel halves belong to no
component and are always kept, so this row lands renumbered at slot 15 — the
number the gate's `spawn authorized … component=crossing-peer` marker records.
Appended as the highest base-layout slot, which renumbers nothing for the eight
profiles that drop it, taking the unpruned table to 63 of
`MAX_BOOT_LAYOUT_ENTRIES` (64).
