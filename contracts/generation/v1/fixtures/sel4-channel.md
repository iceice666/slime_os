# `sel4-channel.zti` — the P5.3.1 channel-plane generation

A third seL4 generation, beside [`sel4.zti`](sel4.md) and the frozen x86
[`valid.zti`](valid.zti). It declares the smallest graph that can exercise
`slime-root`'s channel plane: two components and two channel grants.

## Why a third generation rather than a profile or an addition to `sel4.zti`

`init.rs` selects its behaviour with `option_env!`, which is resolved at compile
time. One component build therefore cannot serve two gates: the binary either
runs the channel scenario or it does not. A separate generation — and so a
separate image — is mechanically required, not a stylistic preference.

Keeping it separate also preserves P5.2's evidence. `sel4.zti`'s five-component
graph is asserted marker-for-marker by `just sel4_component_graph_check`, down
to task ids, grant counts, and an exact transfer-window base address. Adding
components to it would rewrite all of that for a reason unrelated to what that
gate proves.

The same reasoning as `sel4.md`'s applies to why this is not a *boot profile* of
`valid.zti`: `resolve_boot_profile` narrows by subtraction, so naming a
component in a new profile would drop it from `default` and change the frozen
45-slot product generation that `just product_boot_check` and the nineteen
`just boot_layout_check` pairs guard.

## The two grants, and what each is for

### `console-output` — the parked receive

`source = "console"`, `target = "init"`, `rights = ["send"]`.

A grant's rights describe its **target**, and its source holds the opposite end,
so `init` sends and `console` receives. That is the same sense the retired
kernel uses: `kernel/src/runtime/bootstrap.rs` places this channel's slot with
`RIGHT_RECV`, and `init.rs` hands that receive end to the spawned `console`.

`console.rs` is unmodified — it is already exactly the parked receiver this
slice needs, blocking in `recv(0)` and writing whatever arrives to the serial
port. Because the root holds the reply rather than answering `WouldBlock`,
`console` reaches that `recv` before `init` sends and is genuinely parked in the
kernel when the send lands. The message it prints is over sixteen bytes on
purpose: at or below that bound the transport packs the payload into the fast
message registers and the transfer window is never touched.

`transferable = true` because the boot layout's `console-output` entry carries
`send | transfer`, and the root requires the layout's declared rights and the
grant's to be equal before it will fill the slot.

### `service-spawn` — the queue-full and wait arms

`source = "init"`, `target = "init"`, `rights = ["send", "recv"]`.

A self-edge: `init` holds both ends of one bidirectional channel, at one slot
number. Two arms need it, and neither is reachable with a channel between two
live components.

**Queue-full** has to be deterministic. A queue whose peer is running gets
drained by that peer, so filling it is a race. Nothing drains a queue whose only
reader is the task currently filling it, so `CHANNEL_CAPACITY + 1` sends refuse
the last one on every boot.

**`wait`** needs a source that is already ready. `console.rs` only calls `wait`
after `ERR_WOULDBLOCK`, which this design never returns — the root parks the
caller instead — so the console path would leave `wait` unexercised. Waiting on
the receive direction of a channel init has just queued to is ready
immediately, which is the answer the readiness probe must produce; a `wait` that
parked there would deadlock a single-threaded component against itself.

A grant naming both `send` and `recv` is one logical channel carrying two
directed queues, addressed by one slot at each end — the same shape the retired
kernel gives `spawn-service-rpc`, which it places as the two halves
`dango-spawn`/`service-spawn`.

## What this generation deliberately does not declare

- **No `sharedBufferBudget` entries.** No component here allocates a shared
  buffer: the transfer window each one stages through is root-mapped at
  construction rather than created from a factory. The table is declared and
  empty, so "absent from the table means no quota at all" is a property this
  graph exercises rather than a rule it avoids.
- **No `spawn` grants and no `spawnBudget`.** Constructing a child from a
  resolved grant is P5.3.3. Both components are launched by the root from the
  generation, which is P5.2's mechanism.
- **No state bindings.** Nothing here persists.

## The channels this graph cannot carry, and why that is stated rather than hidden

The root places the bootstrap component's slots from the boot-layout resource,
because `init.rs` compiles against constants generated from that same layout. A
grant naming `init` whose channel the layout does not label therefore has no
slot number the root can know, and it is **skipped and counted** rather than
placed at a guessed number — the boot prints `SLIME_GRAPH channel unplaced` for
each one and a total in the `channels` marker.

Both grants here are placeable, so this generation's count is zero. `sel4.zti`'s
`spawn-service-rpc` is not: the layout labels channel *halves*
(`dango-spawn`, `service-spawn`) while the generation names the *grant*, and
nothing maps one onto the other. In the retired kernel `init` resolves that by
being a broker — it holds both halves and hands one to each child in that
child's spawn grant list, which is also what fixes the child's slot numbers.
That distribution step arrives with spawn, in P5.3.3.
