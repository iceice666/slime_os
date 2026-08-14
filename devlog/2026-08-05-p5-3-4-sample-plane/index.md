# P5.3.4 sample plane: the C7 composition on seL4, and P5.3 closed

| Field | Value |
|---|---|
| Date | 2026-08-05 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/{main,task}.rs`, `contracts/generation/v1/fixtures/sel4-sample.{zti,md}`, `components/bins/src/bin/init.rs`, `components/bins/build.rs`, `scripts/build/{build-generation,build-sel4}.py`, `scripts/check/check-sel4-sample-plane.py`, `Justfile` |
| Roadmap | P5.3.4, P5.3, B14, B12, B15, B16 |
| Gates | `just sel4_sample_check`, `just sel4_spawn_check`, `just sel4_loan_check`, `just sel4_channel_check`, `just sel4_component_graph_check`, `just sel4_root_boot_check` |
| Trigger | P5.3.4 opened after P5.3.3 landed; P5.3's exit condition had no composed observation |
| Baseline | P5.3.3 complete: children are constructed and supervised, but no seL4 graph runs the sample plane's own components |

## Summary

P5.3's exit condition — two components exchanging and returning a payload larger
than the control-message bound, with quota exhaustion and peer death reclaiming
what the x86 corpus records — had four sub-slices and no composed observation.
This slice supplies it: the **unmodified** `sample-lender` and `sample-receiver`
move an 8192-byte payload over seL4, 128× the 64-byte bound, running the same
ordered transcript `just sample_plane_live_check` records on x86.

Two changes in the root made that possible, and both are the milestone's own
words rather than additions to it.

`serve_buffer_loan` now accepts a `RIGHT_SUPERVISE` handle at `receiver_slot`
alongside the channel end P5.3.2 admitted. The retired kernel names a loan's
receiver that way, and `sample-lender.rs::RECEIVER_SLOT` is exactly that handle
— its third spawn grant — so accepting it is precisely what lets a component
written against that ABI run here unchanged. P5.3.2's doc deferred this question
to P5.3.4 by name.

And a **spawned** child now takes the shared-buffer ceiling the generation
declares for its component. Before this only root-launched components were
budgeted, so the spawned lender held `HolderQuota::DENY` and its first
allocation failed. The budget names a *component*; whether that component's task
was launched by the root or spawned by a parent is not something the manifest
says, or should have to.

**B14 is closed** with the denial arm its deferral reason named.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `main.rs` `serve_buffer_loan` | Accept `Resource::Supervision { task }` at `receiver_slot` beside `Endpoint` | A loan's receiver is named by capability, in either form the ABI offers |
| `main.rs` `construct_child` | Declare the child's quota from the generation, keyed by component name | A component's declared ceiling does not depend on who started it |
| `main.rs` `serve_spawn` | Refuse a spawn past the caller's declared `spawnBudget`, as `ERR_OUT_OF_MEMORY` | B14: the generation's number bounds a component, not the task table's size |
| `task.rs` `Task::{spawner,component}` | Record who spawned each task and which component it is | A per-component bound reads the same whoever started the task |
| `main.rs` `reclaim_task_objects` | Reclaim the task itself on both death paths, not only on a spawn unwind | A dead task leaves the table, freeing its parent's budget and returning its CSlots |
| `init.rs` `drive_sample_plane` | `launch_sample_plane`'s composition, with the channel minted rather than declared | The x86 spawn-grant order is what fixes the children's slots |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A supervision handle stops naming a loan receiver | `just sel4_sample_check` | `[init] sample plane fail: a sample component did not exit cleanly` |
| A spawned child is not budgeted | `just sel4_sample_check` | same — the lender cannot allocate |
| The declared spawn budget stops biting (B14) | `just sel4_sample_check` | `[init] sample plane fail: spawn budget did not bite` |
| A sample component gains a seL4 branch | `just sel4_sample_check` | `check_components_are_unmodified` fails on the source |
| The asserted transcript drifts from the oracle's | `just sel4_sample_check` | `SAMPLE_MARKERS has drifted from check-sample-plane.py` |
| Channel-peer naming regresses while widening | `just sel4_loan_check` | its own ordered markers |

## Verification

All observed on 2026-08-05. Each passed.

| Gate | Result |
|---|---|
| `just sel4_sample_check` | pass — the new gate |
| `just sel4_spawn_check` | pass — with budget enforcement live |
| `just sel4_loan_check` | pass — channel-peer naming intact after the widening |
| `just sel4_channel_check` | pass |
| `just sel4_component_graph_check` | pass |
| `just sel4_root_boot_check` | pass |
| `just contracts_check`, `just generation_check` | pass |
| `just fmt_check_all`, `just lint_all` | pass |
| `just test` (x86 oracle), `just test_host` | pass |
| `just devlog_check`, `just ruff`, `just typos`, `just machete` | pass |

The observed transcript is [`sample-plane-boot.log`](sample-plane-boot.log). Its
terminal lines are:

```
SLIME_GRAPH served live=0 unsupported=0 unimplemented=0 buffers=5 windows=0 tables=0
SLIME_GRAPH tasks reclaimed live=0 slots=373
```

The last line's `[sample-receiver] fail: recv` above it is expected and is not
this composition's: after `[init] sample plane complete`, init spawns one more
receiver purely to show the budget recovered, and that child holds no channel
by construction. The gate bounds its component-failure scan at the completion
line for exactly this reason.

### The transcript is the oracle's, and cannot quietly stop being

`SAMPLE_MARKERS` in the gate is copied verbatim from
`check-sample-plane.py::MARKERS`, and `check_transcript_matches_the_oracle`
re-reads that file at run time and fails if the two lists diverge. Copying a
marker list into a second file and asserting it there proves only that the file
agrees with itself; the claim under test is that seL4 produces the transcript
the *retired kernel's* gate requires, so that gate is the authority.

Two of its lines are exempt, each with the reason recorded in `ORACLE_ONLY`:

- `[generation] shared-buffer factory grants valid` is emitted by
  `kernel/src/runtime/bootstrap.rs`'s `require_grant` sweep. `slime-root`
  validates grants at admission and again at the point of use and has no
  equivalent line. What covers it here is `[sample-lender] buffer created`,
  which the gate requires and which cannot succeed unless the factory
  capability resolved — B13 made `serve_buffer_create` resolve it.

  The first version of this exemption cited `[sample-lender] factory is not a
  buffer` instead, which was wrong: that line only proves
  `shared_buffer_seal(FACTORY_SLOT)` answered `ERR_BAD_CAP`, and an entirely
  *absent* slot satisfies it identically. The boot log shows exactly that — the
  unconfigured lender the root launches prints it, then fails at
  `class=ungranted`. Corrected after review.
- `[init] sample plane complete` is printed by both, but through a different
  init branch, so it is asserted directly rather than through the oracle list.

The exemption list is itself checked: naming a marker the oracle no longer emits
fails the gate, so an exemption cannot outlive the line it excuses.

## Decisions

### Accepting a supervision handle, and the exact bound it carries

`serve_buffer_loan` now resolves two kinds at `receiver_slot`, and they answer
different questions. A supervision handle names its subject outright — it was
minted by the spawn that created that task and names nothing else, ever. A
channel end names its peer. In both cases the receiver is reached through a
capability rather than an ambient task id, which is what the exit condition
asks for.

The bound is **one hop**, and that is wider than the channel-end rule rather
than equal to it. `preflight_spawn_grants` permits a `Supervision` capability
as a spawn grant, so a parent may hand a child a handle naming a task the child
did not create — `launch_sample_plane` does exactly that, giving `sample-lender`
a handle to the receiver `init` spawned — and the child may then loan against
it. `Resource::is_transferable` is `false` for `Supervision`, so a handle
cannot spread further over IPC.

That is the retired kernel's rule, and tightening it to "the caller's own child"
would refuse the very composition this milestone must reproduce. The first
version of this entry claimed the handle always names a task the caller created;
review showed that is not what the code permits, and the bound is now stated in
`serve_buffer_loan`'s doc rather than left implied.

The generation's `transferable` bit applies only on the channel-end path,
because a handle names a task rather than an edge and there is no bit to read.
The delegation it rests on is the executable grant the spawn already checked:
requiring an edge bit as well would demand a manifest restate a delegation it
made by granting the executable.

Channel-peer naming is kept rather than replaced. It is a real bound in its own
right — a component can only loan to a task the generation gave it an edge to —
and `sel4-loan.zti`'s graph has no spawn in it, so there is no handle to name.

### The peer channel is minted, not declared

Init mints the pair through its declared `endpointCreate` grant. Not a
preference: a `source == target` grant is a **loopback**, which
`ChannelTable::push` gives one queue and `channel::materialize` gives one slot,
and this composition needs init to hold two halves so it can give one to each
child. No single declared grant produces that.

On x86 the halves come from two layout-named slots that `bootstrap.rs` fills
from one `ipc::channel()` — a correspondence this root cannot read from the
manifest, which is the namespace mismatch `channel.rs`'s module doc records.
Minting is what `spawn-service` does on every x86 boot, so it is the mechanism
rather than a substitute. The components cannot tell: each receives its half at
its own slot 0 either way.

### The spawn budget is derived, not counted — which exposed a leak

`Task` records the id of the task that spawned it, and `live_children` counts
the table. A counter would need decrementing on the clean-exit path, the fault
path, and every spawn unwind, and a missed decrement would silently tighten a
bound the generation declared. A reclaimed task frees its parent's budget by
ceasing to exist.

**Except that no graph task was ever reclaimed.** `TaskTable::reclaim` — the
only path to `CleanupRecord::revoke` — was called from the P5.1 fixture path and
from `release_child`, and from neither death arm in `serve_component_graph`. So
a component that exited or faulted kept its table entry for the rest of the
boot, and its root CSlots with it.

Two consequences, and this slice is what made the first one matter. The derived
budget counted *dead* children, making `spawnBudget` a lifetime cap rather than
the live-child cap the generation declares and `sys_spawn` enforces — so B14's
own recorded exit condition ("succeeds again once one is reclaimed") was
unimplemented while being claimed. And every graph component leaked its VSpace,
frame, CNode, and TCB CSlots invisibly, since no terminal marker reported the
task table at all.

Both death arms now call `reclaim_task_objects`, and a new marker reports it:

```
SLIME_GRAPH tasks reclaimed live=0 slots=311
```

373 root CSlots per boot that previously leaked. The leak predates this slice —
it is present at P5.3.3 and every seL4 milestone before it — but the budget
built on top of it is this slice's, so it is fixed here rather than deferred.

`drive_sample_plane` now spawns once more after both children have exited, which
is the arm that distinguishes the two readings: a lifetime cap refuses there
too, and only a live-child count recovers.

## Open risks and follow-ups

- **Two holders share one component's declared ceiling.** The root launches
  every declared component (P5.2), so this boot also starts unconfigured
  `sample-lender` and `sample-receiver` instances beside the spawned ones. A
  quota is keyed by task, so two `HolderId`s each hold the full ceiling for one
  component name. No graph is mis-admitted — the unconfigured instances exit
  before allocating — but the aggregate charged against a component name is
  twice what the manifest declares. Recorded rather than fixed: making the root
  skip a declared component would change P5.2's launch rule to tidy a
  transcript.
- **`shared_buffer_budget` is re-decoded per spawn.** `construct_child` walks
  the generation's objects to find the budget on every spawn rather than
  resolving it once. Bounded and small — the object table is a handful of
  entries — but it is work repeated for no reason, and the launch path already
  decodes it once.
- **B15 and B16 do not bite here** and are re-deferred on observations rather
  than by omission: the largest grant list in this graph is three (48 bytes
  against the 64-byte staging bound), and it creates five tasks against
  `MAX_RECORDS = 32`.

## Artifacts and provenance

- [`sample-plane-boot.log`](sample-plane-boot.log) — the observed serial
  transcript, captured 2026-08-05 from `build/slime-sel4-sample.elf` as built by
  `just sel4_sample_check`. Frozen; corrections are appended, never edited in.
- Fixture rationale:
  [`contracts/generation/v1/fixtures/sel4-sample.md`](../../contracts/generation/v1/fixtures/sel4-sample.md).

### Fault injection

Each change was removed in turn and the gate re-run against the injected build.

| Injection | Expected | Observed |
|---|---|---|
| A spawned child gets `HolderQuota::DENY` | gate fails | fails: `a sample component did not exit cleanly` |
| The declared spawn budget is not enforced | gate fails | fails: `spawn budget did not bite` |
| `serve_buffer_loan` rejects supervision handles | gate fails | fails: `a sample component did not exit cleanly` |
| A marker is dropped from `SAMPLE_MARKERS` | gate fails | fails: `SAMPLE_MARKERS has drifted from check-sample-plane.py` |
| Neither death path reclaims its task | gate fails | fails: `budget did not recover after a child exited` |

The fourth is the one worth stating. The first three prove the *root* changes
are load-bearing; the fourth proves the **gate** cannot be quietly weakened to
match a regression, which is the failure mode a copied marker list invites. It
fails against the oracle's own file rather than against a constant in this one.

The first injection is also the blocker that would have made boot one fail:
nothing in P5.3.3 exposed it, because no spawned child had ever allocated.

The fifth reproduces the state the code was in when review found it, and the
recovery arm catches it — which is the evidence that the arm is worth its line
rather than a restatement of the refusal above it.
