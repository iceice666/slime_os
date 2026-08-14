# P5.3.3 spawn plane: child construction and supervision on seL4

| Field | Value |
|---|---|
| Date | 2026-08-05 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/{main,supervision,channel,graph,fault}.rs`, `contracts/generation/v1/fixtures/sel4-spawn.{zti,md}`, `components/bins/src/bin/{init,console}.rs`, `components/bins/build.rs`, `scripts/build/{build-generation,build-sel4}.py`, `scripts/check/check-sel4-{spawn-plane,component-graph,loan-plane}.py`, `Justfile` |
| Roadmap | P5.3.3, B13, B10, B14, B15, B16 |
| Gates | `just sel4_spawn_check`, `just sel4_loan_check`, `just sel4_channel_check`, `just sel4_component_graph_check`, `just sel4_root_boot_check` |
| Trigger | P5.3.3 opened after P5.3.2 landed; `Operation::Spawn` resolved its authority and then refused |
| Baseline | P5.3.2 complete: loans cross between components against generation-declared quotas, but no component can start another |

## Summary

`slime-root` could resolve a spawn's authority and then refused to act on it,
so no component could start another. `SupervisionStatus`, `CapDrop`, and
`EndpointCreate` had no handler at all, and `WaitSource::Supervision` resolved
to `Unmediated` — a wait naming only it was refused outright, because no spawn
existed to mint a handle for it to name. This slice implements all four, so
`init` constructs two children from grant-resolved executables, hands each the
capabilities its slots name, and collects one child's clean exit through a
supervision handle after being woken by that death.

Both children are the **unmodified** `console` and `sysinfo` binaries the x86
oracle builds. That is the load-bearing claim, and the gate checks it against
the sources rather than inferring it from the transcript.

Two things landed here that were not on the milestone's face but that its own
words required. The bootstrap component's executable and factory slots are now
placed from the **boot layout** rather than from a running cursor — this is the
first seL4 generation to grant `init` an executable, which is what made the
coupling observable. And **B13 is closed**: `serve_buffer_create` resolves the
caller's factory capability before admitting anything, whose recorded deferral
reason was verbatim "the same distribution problem P5.3.3 solves".

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `main.rs` `serve_spawn` | Construct the child, install its table, distribute its channel ends, mint the parent a supervision handle, activate | A component starts exactly the executables its generation granted it |
| `main.rs` `preflight_spawn_grants` | Validate the whole grant list before allocating: executable needs `RIGHT_EXEC \| RIGHT_SPAWN`; no duplicate or self-executable slot; every grant narrows | A parent cannot hand on authority it does not hold |
| `main.rs` `construct_child` / `release_child` | Allocate only after every check passes; reclaim the child through `TaskTable::reclaim` on each later failure | A refused spawn leaks nothing |
| `main.rs` `distribute_channel_ends` | Move each granted endpoint to the child and drop the parent's granted slot, all-or-nothing | A channel end resolves to a queue for exactly one holder |
| `supervision.rs` (new) | `Terminations` records how each child ended, outliving the task; `SupervisionWaits` records who is parked on whose death | An outcome is answerable after the task and its table are gone |
| `main.rs` `serve_supervision_status` | Answer through a `RIGHT_SUPERVISE` handle; consume it on collection | A child's fate is named by capability, never by an ambient task id |
| `channel.rs` `WaitTarget::Supervision` | Resolve a supervision wait source through the caller's table | A wait on a child's death parks and is woken, rather than being refused |
| `main.rs` `EndpointCreate` | Mint a channel pair through a declared factory | `init` can broker a channel the generation could not have named |
| `channel.rs` `reassign` / `mint` | Split a minted loopback into a real pair when one end moves | A child's `recv` finds the queue it was granted |
| `main.rs` `CapDrop` | Release a slot the caller holds | `spawn_or_fail`'s handle drop works, as on every x86 boot |
| `main.rs` executable/factory placement | Bootstrap component takes the boot layout's slot; others keep the cursor | B10: the slot a component compiles against is the slot the root fills |
| `main.rs` `SharedBufferCreate` | Resolve the named factory capability; honour the `writable` flag | B13: the grant authorizes and the budget bounds, independently |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A spawn widens its own grant | `just sel4_spawn_check` | `[init] spawn plane fail: a spawn widened its own grant` |
| An endpoint grant copies rather than moves | `just sel4_spawn_check` | `[init] spawn plane fail: a handed-off channel end still resolved` |
| A collected outcome is re-readable | `just sel4_spawn_check` | `[init] spawn plane fail: a collected handle answered twice` |
| A child's death does not wake its parent | `just sel4_spawn_check` | Boot wedges; gate times out |
| A budget entry alone admits an allocation (B13) | `just sel4_loan_check` | `[init] loan plane fail: an empty slot named a buffer factory` |
| A child gains its own executable | `just sel4_spawn_check` | `[init] spawn plane fail: a child was granted its own executable` |
| A spawned child is quietly given a seL4 branch | `just sel4_spawn_check` | `check_spawned_children_are_unmodified` fails on the source |

## Verification

All observed on 2026-08-05, in this order. Each passed.

| Gate | Result |
|---|---|
| `just sel4_spawn_check` | pass — the new gate |
| `just sel4_loan_check` | pass — with the new B13 denial arm |
| `just sel4_channel_check` | pass — unchanged markers |
| `just sel4_component_graph_check` | pass — two assertions updated, see below |
| `just sel4_root_boot_check` | pass |
| `just contracts_check` | pass — 19 fixtures agree with the resource |
| `just generation_check` | pass |
| `just fmt_check_all`, `just lint_all` | pass |
| `just test` (x86 oracle), `just test_host` | pass |
| `just devlog_check`, `just ruff`, `just typos`, `just machete` | pass |

The observed transcript is [`spawn-plane-boot.log`](spawn-plane-boot.log),
captured from the gate's own image. Its terminal line is:

```
SLIME_GRAPH spawns served=2 drops=1 endpoints=2 terminated=5 waits=0
```

`waits=0` is the teardown property: nobody is still registered on a child's
termination, which would be a wake that can never arrive. `terminated=5` is
deliberately non-zero — one record per child that ended, kept past reclamation
by design — so a zero there would mean the supervision path recorded nothing.

### What changed in P5.2's gate, and why

`check-sel4-component-graph.py` has two edits, and neither relaxes anything.

The refusal assertion moved from `error=-4` to `error=-1`. Until this slice the
spawn arm answered `InvalidOperation` because nothing resolved the slot; now
`preflight_spawn_grants` resolves it and refuses, which is a *capability*
answer — and it is the code `kernel/src/syscall/mod.rs::sys_spawn` returns for
every `SpawnError` but the two exhaustion cases. That agreement is load-bearing
rather than cosmetic: `init.rs::spawn_optional_storage` matches on exactly
`Err(slime_rt::ERR_BAD_CAP)` to distinguish "no block device" from a real
failure, so a seL4 root answering `-4` there would abort a graph the retired
kernel launches.

`check-sel4-loan-plane.py`'s grant count moved from 3 to 5, because that
fixture now declares the two `bufferCreate` grants B13's fix requires, and
gained the `class=ungranted` assertion described below.

## Decisions

### An endpoint spawn grant is a move, not a copy

The retired kernel's spawn grant is a non-consuming derived copy. Here an
endpoint grant is a **move**: the parent gives up the slot it granted from.

This is forced rather than chosen. A channel's queues are resolved by *which
task holds each end* (`ChannelTable::send_queue`), not by anything carried in
the capability, so a child handed an endpoint capability without the holder
record moving would resolve to no queue at all and park forever.

The difference falls on the side of less authority, and every x86 caller already
behaves this way: `launch_sample_plane` grants each half to exactly one child,
and `launch_fabric_graph`'s own comment states that init "releases the control
endpoint as soon as the spawn that needed them returns". The parent keeps its
*other* slot when it minted the pair — that is the half it talks to the child
over — and the gate asserts both halves: the granted slot stops resolving, the
retained one still works.

`graph.rs::Resource::is_transferable` still answers `false` for an endpoint, and
its doc now says why that is not a contradiction. A **spawn grant** is bounded
to rights the parent holds and lands in a table that did not exist until the
parent made it; a **send attachment** moves a capability to a task chosen at
runtime by whoever is at the other end of a channel. The second is
redistribution of a declared graph; the first is not.

### The spawn fixture declares no channel edge

Every earlier seL4 fixture declares its channels as grants. This one declares
none, and that is the point.

The retired kernel's `init` is a broker: it mints a pair, keeps one half, hands
the other to a child in that child's spawn grant list. P5.3.1's `channel.rs`
module doc records that this cutover could not do that, because there was no
spawn to distribute halves through. This slice *is* that spawn, so the fixture
exercises the real mechanism. A declared edge would test the materialization
P5.3.1 already proves, and would need the boot layout to have labelled the
grant — the namespace mismatch `channel.rs` documents and this slice does not
resolve.

### Three defects on the spawn failure paths, found by re-reading rather than by booting

`release_child` first suspended the child's thread and dropped its capability
table, which returns nothing: the VSpace, image frames, CNode, and TCB are all
already allocated by then, and the task-table entry stays occupied. Enough spawn
failures would fill a table nothing could empty. It now goes through
`TaskTable::reclaim`, which suspends, revokes every capability derived from the
task's objects, empties its CSlots, and frees the entry — the same path
`reclaim_dead_task` uses for a task that actually ran.

The two failure paths *after* distribution — a parent table too full for the
supervision handle, and a failed activation — additionally left the child's
channel ends assigned to a task about to be destroyed. `recall_channel_ends` now
puts them back, restoring both the holder record and the parent's capability.

All three are on paths no fixture reaches: every spawn in the spawn-plane graph
succeeds, and the exhaustion cases that trigger these unwinds need a table far
fuller than any declared seL4 generation makes it. That is precisely why they
were read rather than tested, and it is the same coverage shape the fault
injection section keeps surfacing — the untested code is where the feature does
not go.

### Channel distribution is all-or-nothing

`distribute_channel_ends` moves one end per endpoint grant, and a grant part way
down the list can be refused. The first version returned immediately, leaving
the ends it had already moved assigned to a child the caller was about to tear
down — and those channels would then name a dead task, reachable by nobody and
reclaimed by nothing, since `reclaim_dead_task` never runs for a child that was
never activated.

It now records each move and unwinds them newest-first on failure, restoring
both halves of what a move changes: the holder record in the channel table and
the capability in the parent's own table. Found by re-reading the failure paths
rather than by a boot — no fixture refuses a grant after accepting one, which is
the same coverage shape the fault injection below keeps surfacing.

### B10's coupling was silently reintroduced, and this slice caught it

P5.2 numbered every component's executables `1..=N` from a cursor. That agreed
with the boot layout only because no seL4 generation had ever granted `init` an
executable. This fixture is the first that does — and the layout puts `console`
at 1 and `sysinfo` at **4**, while a cursor puts them at 1 and 2. `init.rs`
reads `SYSINFO_SLOT` from the generated table, so the spawn would have resolved
to whatever else landed at 4.

This was found by booting, not by reading: the first spawn-plane boot refused
every executable. It is exactly the positional ambiguity B10 exists to remove,
and it was invisible until a generation exercised it.

## Open risks and follow-ups

- **The root still launches every declared component.** This boot therefore also
  starts one unconfigured `console` and one unconfigured `sysinfo` that no one
  handed a channel to, and both exit non-zero. That is expected — the fixture's
  subject is the instances `init` *spawns*, which are separate tasks — and every
  marker names the spawn that produced the task it is about. Making the root
  skip a declared component would change P5.2's launch rule to tidy a
  transcript, which is a worse trade.
A read-only review of the finished diff raised three further divergences from
the retired kernel, each now recorded rather than fixed here, plus five smaller
corrections applied in place: the `unimplemented` catch-all comment overclaimed
(three `RootService` operations — `HealthConfirm`, `Unhealthy`, `CapTransfer` —
still land there, and every gate's `unimplemented=0` is a fact about the
fixtures rather than about the dispatcher); `EndpointCreate` and the
buffer-slot-exhausted arm answered `-5` where the oracle answers `-1`, which
matters for the same reason the spawn refusal did; `preflight_spawn_grants`
checked that a grant narrows but not that its rights are meaningful for the
resource kind, which the oracle enforces at insert; granting both halves of one
minted pair would have made the child a self-loopback, since the dedupe is by
slot and a pair is two slots naming one channel; and two comments in `init.rs`
still described an earlier draft's slots.

Two gate gaps were closed in the same pass. Three failure markers this slice
added — `spawn unwind incomplete`, `channel recall failed`, `channel rollback
failed` — were caught by no gate, though each names a strictly worse outcome
than the `spawn unwound` line that *was* caught. And the spawned `console`'s
receipt of its channel end rested entirely on the root's own bookkeeping marker:
the unmodified `console.rs` prints what it receives, so the bytes were already
on the wire and the gate simply did not look. It does now.

- **Spawn budget is not enforced**, recorded as **B14** in
  [`00-backlog.md`](../../roadmap/00-backlog.md). The generation declares
  `spawnBudget` per component and this slice does not read it, so a component
  can spawn until the task table fills rather than until its declared budget is
  spent. The retired kernel checks `live_children >= spawn_budget` in
  `spawn_from_cap`. Not a hole in the exit condition, which is about *which*
  executables resolve rather than how many times — but it is authority the
  generation declares and the root ignores, which is the same shape B13 had.
  Deferred to P5.3.4, where a multi-child graph already exists to prove it
  against.
- **A spawn carries at most four grants**, recorded as **B15**. The grant array
  crosses the transfer window through `read_staged`, whose bound is the 64-byte
  control message, so four 16-byte records is the ceiling — against the oracle's
  sixty-four. Real x86 callers already exceed it: `GENERATION_MANAGER_CAPS` and
  `dango_caps()` are six. No seL4 fixture spawns with more than one grant, so
  nothing observes it, but a component that launches its children on the retired
  kernel would fail to on the cutover, and P5.4 has to be able to claim
  otherwise. `MAX_SPAWN_GRANTS` is now *derived* from the staging bound rather
  than asserted to match the kernel's, so the real ceiling is in the source
  instead of reachable only as a length error.
- **A termination record is never reclaimed**, recorded as **B16**. `MAX_RECORDS`
  bounds tasks alive at once, but `TaskTable::reclaim` frees entries while
  `next_id` keeps counting, so a graph that spawns and reaps repeatedly can
  exceed it while holding few tasks. Past the bound the record drops silently
  and `supervision_status` answers `WouldBlock` forever — the
  parent-waits-forever failure `supervision.rs` exists to prevent, arriving
  through its own bookkeeping. The module doc claimed this was bounded; it is
  not, and now says so.
- **`Termination::Timeout`, `PeerLoss`, and `Unhealthy`** are in the component
  ABI and this root never produces them. Only `Exit` and `Fault` are recorded,
  which is what the two death paths can observe.

## Artifacts and provenance

- [`spawn-plane-boot.log`](spawn-plane-boot.log) — the observed serial
  transcript, captured 2026-08-05 from `build/slime-sel4-spawn.elf` as built by
  `just sel4_spawn_check`. Frozen; corrections are appended below, never edited
  in.
- Fixture rationale: [`contracts/generation/v1/fixtures/sel4-spawn.md`](../../contracts/generation/v1/fixtures/sel4-spawn.md).

### Fault injection

A passing gate does not by itself prove a denial arm fires, so each was removed
from `slime-root` in turn and the gate re-run against the injected build.

| Injection | Expected | Observed |
|---|---|---|
| Remove the narrowing check in `preflight_spawn_grants` | gate fails | fails: `a spawn widened its own grant` |
| Make the endpoint grant a copy (parent keeps its slot) | gate fails | fails: `a handed-off channel end still resolved` |
| Stop consuming the handle on collection | gate fails | fails: `a collected handle answered twice` |
| Drop the supervision wake on exit | gate fails | **wedges**: boot exceeds 120s without the terminal marker |
| Remove B13's factory resolution | gate fails | **passed** — see below |

The fourth is worth stating plainly: the wake is load-bearing, and its absence
is a hang rather than a wrong answer, which the gate's watchdog catches.

The fifth is the finding of this slice. With the factory check removed **every
gate still passed**, because no fixture had a component that held a budget and
tried to allocate without a grant — the fix was uncovered by construction, and
would have shipped as untested code justified only by its own comment. The loan
fixture's `init` now names an empty slot and a wrong-kind slot deliberately, and
`check-sel4-loan-plane.py` asserts `class=ungranted` *before* any ceiling is
grazed, so the refusal cannot be a quota answer wearing another name. Re-running
the injection against the new arm fails as it should.

This is the third slice running where fault injection found something the gate
could not. It is not a coincidence: a gate written alongside a feature tends to
assert the paths the feature takes, and the paths it does not take are exactly
where the untested code is.
