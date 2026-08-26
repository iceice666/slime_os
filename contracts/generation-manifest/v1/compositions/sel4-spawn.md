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

Holds two `exec | spawn` grants and nothing else. That is the whole authority
the scenario needs, and the fixture states it rather than letting it be
inferred:

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

- **`console-control`** and **`sysinfo-context`** are the endpoints the two
  children answer on. They are *ordinary grants*: an endpoint is a real seL4
  Endpoint the generation names, so the root materializes both halves at
  admission and installs each side into the instance that declared it. A parent
  holds no half to hand over, and there is no operation left that would create
  one — the cutover deleted `endpointCreate`, and B50 deleted the
  `mintedBindings` of kind `endpoint` that outlived it.

- **`console-view` and `sysinfo-view-1..6`** are what actually crosses at spawn:
  narrowed, transferable directory views init holds and hands on. The narrowness
  the "a spawn cannot widen its own grant" arm tests against is the declared
  rights ceiling on those views, and the six-wide array is B15's exit-condition
  width — 96 bytes of grant records, past the 64-byte message bound a narrower
  reader would apply.

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

## Why the graph declares no minted binding

It used to declare seven, all of kind `endpoint`, and every one was residue.

The retired kernel's `init` was a **broker**: it created channel objects at
runtime and handed a child one end in that child's spawn grant list. A
`MintedBinding` existed to describe exactly that — an edge whose destination is
declared but whose object identity is deferred to the minter. Post-cutover there
is no minter: an endpoint is a generation-owned seL4 Endpoint object, and the
root is the only party that can create or install one.

So a minted binding of kind `endpoint` named an object nobody could ever supply.
`preflight_spawn_grants` counted it in the total a parent must satisfy, init
could not satisfy it, and every spawn on this plane was refused with
`declared-count requested=0 … minted=7`.

What the plane's claim needed instead was authority a parent genuinely holds
*and may pass on*. That is what `transferable` marks, and a directory view is
the smallest such authority this repo has: init holds seven narrowed views and
hands one to `console` and six to `sysinfo`. The gate still asserts the grant
count at the **spawn marker**, so the property under test is intact — a parent
hands a child its capabilities at spawn — while the capabilities crossing are
ones the model can actually transfer.

## A spawn grant is a copy

The child receives a narrowed copy and the parent keeps its slot. The gate
asserts both halves: `console`'s view is granted and init's slot 5 still
inspects, and all six of `sysinfo`'s source slots still inspect afterwards.

This is a change from the retired model's endpoint *move*, and it follows the
object: a directory capability is a root-mediated view, so a narrowed copy is
the derivation the mechanism already performs. Nothing is shared that the
declaration did not name.

## What the root still launches

Both children are declared `autostart = false`, so the root launches only
`init`. That is a change this cutover made deliberately: the plane's subject is
the instances `init` **spawns**, and an autostarted copy of each executable —
which the earlier fixture declared — added two tasks that held a declared
endpoint, blocked on it forever, and appeared in every accounting line the gate
reads.

Declaring them non-autostart is not skipping a declared component: the root
still constructs whatever a graph asks it to start, and this graph asks for one
instance. The two children exist as declarations their owner activates, which is
exactly what a spawn plane is about.

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
