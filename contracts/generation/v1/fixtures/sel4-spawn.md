# `sel4-spawn.zti` — the P5.3.3 spawn-plane generation

A fifth seL4 generation, beside [`sel4.zti`](sel4.md),
[`sel4-channel.zti`](sel4-channel.md), [`sel4-loan.zti`](sel4-loan.md), and the
frozen x86 [`valid.zti`](valid.zti). It declares the smallest graph that can
exercise P5.3.3's exit condition: a component that constructs a child from a
grant-resolved executable, hands it declared capabilities at the slots its
layout names, and observes its termination through a supervision handle rather
than an ambient task id.

## Why a fifth generation

The same mechanical reason as the two before it: `init.rs` selects its scenario
with `option_env!`, resolved at compile time, so one component build cannot
serve two gates. A separate generation — and so a separate image — is required
rather than preferred.

It also keeps the earlier evidence intact. Each of `just sel4_channel_check`
and `just sel4_loan_check` asserts its own graph marker-for-marker, down to task
ids and channel keys; adding executables and a factory to either would rewrite
all of that for reasons unrelated to what those gates prove.

## The three components

### `init` — the parent

Holds two `exec | spawn` grants and one `endpointCreate` grant, and nothing
else. That is the whole authority the scenario needs, and the fixture states it
rather than letting it be inferred:

- **`init-console`** and **`init-sysinfo`** are what make `spawn` resolve at
  all. The boot layout numbers them at slots 1 and 4, and `init.rs` addresses
  them through the generated `CONSOLE_SLOT` and `SYSINFO_SLOT` constants, so the
  number the component compiles against and the number the root fills are one
  number (B10).

  This is the first seL4 generation to grant the bootstrap component an
  executable at all, and it is what made the numbering rule observable. Until
  P5.3.3 the root numbered every component's executables `1..=N` from a cursor,
  which agreed with the layout only because no seL4 generation had ever given
  init one. A cursor puts `sysinfo` at 2 while `init.rs` reads 4 — the
  positional coupling B10 exists to remove, silently reintroduced.

- **`init-endpoint-factory`** is what lets init mint the channels it hands its
  children. Deliberately `endpointCreate` alone, with no `bufferCreate`: that
  narrowness is what the "a spawn cannot widen its own grant" arm tests against,
  since asking to hand on `bufferCreate` is asking the root to manufacture
  authority no generation declared.

### `console` and `sysinfo` — the children, unmodified

Both are the binaries the x86 oracle builds, with no seL4 branch in either. That
is the load-bearing claim of the slice, and the gate checks it against the
sources rather than inferring it from the transcript: a child rewritten to suit
this root would produce identical serial output.

`sysinfo` is the one the parent waits on, because it runs to completion and
exits 0 of its own accord — the outcome a supervision handle is for. `console`
loops until its peer dies, which makes it the right subject for the
*still-live* arm: a handle queried while its child runs must answer "no outcome"
rather than blocking or inventing one.

## Why the graph declares no channel edge

Every earlier seL4 fixture declares its channels as grants. This one declares
none, and that is the point rather than an omission.

The retired kernel's `init` is a **broker**: it mints a channel pair, keeps one
half, and hands the other to a child in that child's spawn grant list — which is
also what fixes the child's slot number. P5.3.1's `channel.rs` module doc records
that this cutover could not do it, because there was no spawn to distribute
halves through. This slice is that spawn, so the fixture exercises the real
mechanism: `endpoint_create` mints the pair, the spawn grant moves one end, and
the child finds it at slot 0.

A declared edge would test something else — the materialization P5.3.1 already
proves — and would need the boot layout to have labelled the grant, which is the
namespace mismatch `channel.rs` documents and this slice does not resolve.

## An endpoint grant is a move, not a copy

The retired kernel's spawn grant is a non-consuming derived copy. Here an
endpoint grant is a **move**: the parent gives up the slot it granted from.

The difference is real but narrow, and it falls on the side of less authority. A
channel's queues are resolved by *which task holds each end*
(`ChannelTable::send_queue`), not by anything carried in the capability, so a
child handed an endpoint capability without the holder record being updated
would resolve to no queue at all and park forever. Making it a move is what
makes the handoff work.

Every x86 caller already behaves this way. `launch_sample_plane` grants each
half to exactly one child, and `launch_fabric_graph`'s own comment states that
init "releases the control endpoint as soon as the spawn that needed them
returns". A parent that tried to keep a granted end would find it gone rather
than find it silently shared.

The parent keeps its *other* slot when it minted the pair — that is the half it
talks to the child over — and the gate asserts both halves of that: the granted
slot stops resolving, the retained one still works.

## What the root still launches

The root launches every component the generation declares (P5.2), so this boot
also starts one unconfigured `console` and one unconfigured `sysinfo` that no
one handed a channel to. Both exit non-zero, and that is expected rather than a
failure: this fixture's subject is the instances `init` **spawns**, which are
separate tasks with separate ids.

Every marker the gate asserts names the spawn that produced the task it is
about, so the two cannot be confused. Making the root skip a declared component
would be a change to P5.2's launch rule, made to tidy a transcript — a worse
trade than a boot that starts two components which promptly exit.

## Relationship to B13

This slice closes [B13](../../../../roadmap/00-backlog.md): `serve_buffer_create`
now resolves the factory capability its caller names before admitting anything.
B13's deferral reason was verbatim "the same distribution problem P5.3.3
solves", and this is that slice, so it is closed here rather than deferred
again.

The fix is asserted by `just sel4_loan_check` rather than by this gate, because
that is where a shared-buffer holder lives. Fault injection is what showed the
assertion was needed: removing the factory check left every gate passing, since
no fixture had a component that held a budget and tried to allocate without a
grant. The loan gate now names one.
