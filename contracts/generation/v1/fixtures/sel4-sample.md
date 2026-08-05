# `sel4-sample.zti` — the P5.3.4 sample-plane generation

A sixth seL4 generation, beside [`sel4.zti`](sel4.md),
[`sel4-channel.zti`](sel4-channel.md), [`sel4-loan.zti`](sel4-loan.md),
[`sel4-spawn.zti`](sel4-spawn.md), and the frozen x86 [`valid.zti`](valid.zti).
It declares the graph that composes P5.3's exit condition: two components
exchanging and returning a payload larger than the control-message bound, with
quota exhaustion and peer death reclaiming the same resources the x86 corpus
records.

## Why a sixth generation

The same mechanical reason as the three before it: `init.rs` selects its
scenario with `option_env!`, resolved at compile time, so one component build
cannot serve two gates.

## The two components, unmodified

`sample-lender` and `sample-receiver` are the binaries the x86 oracle builds,
with no seL4 branch in either — the gate checks that against the sources, since
a component rewritten to suit this root would produce an identical transcript.

That is the whole claim of the milestone. Everything the fixture does is in
service of letting those two binaries run as written:

- **`sample-lender.rs` compiles against three slot numbers** — `PEER_SLOT = 0`,
  `FACTORY_SLOT = 1`, `RECEIVER_SLOT = 2` — and never learns them. They are
  fixed by the *order* of the grant list init passes at spawn, which is exactly
  the list `launch_sample_plane` passes on x86.
- **`RECEIVER_SLOT` is a `RIGHT_SUPERVISE` handle**, not a channel end. The loan
  names its receiver through the capability init got when it spawned that
  receiver, which is why spawn order is load-bearing: the receiver must exist
  before the lender can be handed a handle to it.

## What changed in the root to let that happen

Two things, both recorded in the devlog:

- **`serve_buffer_loan` accepts a supervision handle** at `receiver_slot`,
  alongside the channel end P5.3.2 admitted. Neither widens the other: a
  supervision handle is authority over a task the caller *created*, a channel
  end over a task the generation *connected* it to. Both name the receiver
  through a capability rather than an ambient task id, which is what the exit
  condition asks for.
- **A spawned child is budgeted from the generation.** Before this, only
  root-launched components had a shared-buffer quota declared, so a spawned
  `sample-lender` held `HolderQuota::DENY` and its first allocation failed. The
  budget names a *component*; whether that component's task was launched by the
  root or spawned by a parent is not something the manifest says, or should have
  to.

## Why the graph declares no channel edge

Init mints the peer channel at runtime through its declared `endpointCreate`
grant, rather than receiving a declared one.

Not a preference — a `source == target` grant is a **loopback**, which
`ChannelTable::push` gives one queue and `channel::materialize` gives one slot.
This composition needs init to hold *two* halves so it can give one to each
child, and no single declared grant produces that. On x86 the two halves come
from two layout-named slots (`sample-lender-side`, `sample-receiver-side`) that
`bootstrap.rs` fills from one `ipc::channel()`, which is a correspondence this
root task cannot read from the manifest — the namespace mismatch
`channel.rs`'s module doc records.

Minting is also what the retired kernel's `spawn-service` does on every boot, so
it is the mechanism rather than a substitute for one. The components cannot
tell: each receives its half at its own slot 0 either way.

## The budgets, and why each number

`init` declares `spawnBudget = 2` — exactly the two children this composition
needs. That makes the third spawn a **denial arm** rather than an unused
allowance: `drive_sample_plane` attempts one and requires `ERR_OUT_OF_MEMORY`,
which is what closes [B14](../../../../roadmap/00-backlog.md). A budget of 3
would have left the check unexercised.

Both components declare exactly what `valid.zti` gives them on x86, copied
rather than chosen: `sample-lender` is `4 / 1 / 2 / 1` and `sample-receiver` is
`2 / 1 / 2 / 1`. The asymmetry in the first number is the oracle's, and it is
right — only the lender allocates.

- **pages** bound the region: 4 for the lender's two-page payload. The
  receiver's 2 are never charged at all, since `holder_pages` accrues at
  `shared_buffer_create` and the receiver only ever *maps* a loan someone else
  created. Kept at the oracle's number anyway: a fixture that "corrected" an
  unexercised ceiling would be diverging from the manifest this composition
  claims to reproduce.
- **1 buffer**, the region itself.
- **2 mappings** — the lender's writable mapping and, after sealing, its
  read-only one; the receiver's loan map and the read-only probe it makes.
- **1 loan**, the one that crosses.

`init` declares no shared-buffer budget. It holds a `bufferCreate` grant only to
hand on at spawn, and never allocates.

## What the root still launches

The root launches every component the generation declares (P5.2), so this boot
also starts one unconfigured `sample-lender` and one unconfigured
`sample-receiver` holding no channel and no peer. Both exit non-zero before init
has spawned anything.

That is expected, and the gate handles it precisely rather than by loosening:
`check_sample_transcript` discards everything before `SLIME_GRAPH endpoint
minted` — the first thing init does — and applies the x86 gate's own `FORBIDDEN`
patterns to what remains. A failure from a *spawned* component still fails the
gate; a failure from the unconfigured instances is not read as this
composition's.

The side effect worth stating: two `HolderId`s each hold the full declared
ceiling for one component name, because a quota is keyed by task and both the
launched and the spawned instance are tasks. No graph is mis-admitted — the
unconfigured instances allocate nothing before exiting — but the aggregate
charged against a component name is twice what the manifest declares for it.
Making the root skip a declared component would change P5.2's launch rule to
tidy a transcript, which is the worse trade.

## Relationship to the backlog

- **B14 is closed here**, with the third-spawn denial arm above. Its recorded
  deferral reason named this slice: "P5.3.4 composes the sample plane and is
  where a multi-child graph already exists."
- **B15** (a spawn carries at most four grants) does not bite: the lender takes
  three, which is 48 bytes against the 64-byte staging bound.
- **B16** (termination records are never reclaimed) does not bite: this graph
  creates five tasks against `MAX_RECORDS = 32`.

Both are re-deferred on those observations rather than by omission.
