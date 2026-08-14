# B25 and P5.4.6 — endpoint copies close the seL4 native-call plane

| Field | Value |
|---|---|
| Date | 2026-08-08 |
| Kind | Defect |
| Status | Verified |
| Scope | `slime-root/src/{channel,graph,main,transit}.rs`, `components/bins/src/bin/{init,fabric-call-client,fabric-call-time,crossing-peer}.rs`, `scripts/check/check-sel4-{call,channel,spawn,loan}-plane.py`, `scripts/check/check-sel4-gate-controls.py`, `Justfile`, `roadmap/{00-backlog,07-architecture-portability}.md` |
| Roadmap | B25, P5.4.6, P5.4, C8.6 |
| Gates | `just sel4_call_check`, `just sel4_channel_check`, `just sel4_spawn_check`, `just sel4_loan_check`, `just sel4_crossing_check`, `just sel4_gate_control_check`, `just test_sel4_root`, `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` |
| Trigger | The call plane reached parent-vouched supervision delegation only in experimental orderings; the committed endpoint-holder model could not represent the oracle's non-consuming spawn grant |
| Baseline | `sel4-call` admitted and spawned every component but deadlocked before role provisioning or any C8.6 outcome |

## Summary

The seL4 C8.6 call plane deadlocked before role provisioning because spawn
grants moved endpoint ownership into the child while the oracle copied endpoint
authority and left the parent's end usable. Endpoint capabilities now carry a
queue `Side`, spawn grants make non-consuming narrowed copies, and transit binds
attached capabilities to the receiving side. The parent-vouched call composition
now completes all bounded-call arms and every spawned task exits cleanly.

## Observable symptom

The call broker consumes two records on each authenticated participant control:
the participant's role request, then a supervision handle naming that participant
and sent by its parent. `init` held the handle, but a seL4 spawn grant moved the
service-side endpoint into the broker and deleted init's slot. The oracle copied
the endpoint, so the same composition worked there.

This was not slot numbering. Runtime minting had already made the broker's control
slots contiguous. Nor could a participant present itself safely: it held no
self-naming handle, and letting it vouch for itself would weaken the broker's trust
boundary because the descriptor does not reveal which task a supervision handle
names.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| Represent the endpoint side in `Resource::Endpoint` | Queue lookup no longer needs `Entry::producer` or `Entry::consumer` | A second task can name the same end without mutating channel state |
| Make spawn grants ordinary narrowing copies | The parent keeps its endpoint while the child receives the same side | `init` can send the post-spawn introduction after spawning the broker |
| Bind `Transit` to `(channel, receiving_side)` | Attached capabilities follow the task that wins the queue dequeue | No sender-time task prediction is needed when an end has co-holders |
| Re-run the sample plane during the spike | Task-bound transit selected init instead of the receiver and wedged | Side-bound transit, not first-holder selection, is required |
| Resolve task-naming channel authority | `unique_holder_of_endpoint_side` returns no task when the opposite end is shared | Shared queue authority cannot silently become ambiguous task authority |
| Drive the completed call image | The first clock probe could consume an already queued phase 1 | Treat a successful probe as phase 1; phase 3 is a no-time-advance completion barrier |

## Root cause

`ChannelTable::Entry` encoded each end as one task holder, and
`distribute_channel_ends` reconciled a child grant by replacing that holder and
dropping the parent's slot. That representation conflated queue direction with
task membership and violated spawn grants' derive-without-consuming invariant.
It also made a task-bound transit destination unavoidable, even though copied
receive ends permit more than one task to race for the same queued delivery.

## Changes

### Endpoint authority names a side

`Resource::Endpoint` now carries `{ channel, side }`, where `Side` is
`Producer`, `Consumer`, or `Loopback`. `ChannelTable` stores only the forward and
reverse queues and their transferability; holder identity is derived from live
capability tables when death, reclamation, or task-naming operations need it.

`preflight_spawn_grants` copies the resource with narrowed rights and leaves the
parent's slot intact. Endpoint grants therefore match the oracle's
`Arc<Endpoint>` clone semantics and every other spawn-grant kind. The obsolete
holder reassignment, rollback, and recall paths are gone.

### In-flight capabilities follow queue delivery

A `Transit` entry records the channel and receiving side, not a preselected task.
`recv` collects the token only when its endpoint reaches that side. If two tasks
hold the same receive end, they already race for one queued message; the winner
receives both the bytes and their attached capabilities. Reclamation drops a
transit entry only after the last holder of its destination side dies.

A queue end is not automatically a unique task identity. Channel-routed loan
creation uses `unique_holder_of_endpoint_side` and refuses an absent or ambiguous
receiver. Loan collection still checks the declared receiver, so a co-holder may
dequeue bytes but cannot use a loan naming another task.

### Parent-vouched call composition and causal clock

`drive_call_plane` mints four participant controls, a client/time phase pair, and
a private client/client-B phase pair. It spawns the broker first, retains its
copied control ends, spawns the participants, and transfers exactly three
participant supervision handles over their authenticated controls. The time task
is spawned last.

The clock advances only for phases 1 and 2. Client A sends phase 3 after observing
the server's peer-death terminal; that phase is a completion barrier and does not
advance time. `fabric-call-time` probes slot 1 to distinguish the unconfigured
generation-launched copy from the runtime-spawned copy. A queued phase 1 consumed
by the probe counts as phase 1 rather than being discarded.

### Standing gate

`just sel4_call_check` builds `slime-sel4-call.elf`, validates its identity
manifest, boots it on the pinned qemu-arm-virt profile, and checks 50 markers
across ten causal chains. It also:

- requires exactly three parent-sent `RIGHT_SUPERVISE` introductions;
- requires the non-idempotent request marker exactly once;
- derives the five spawned child ids from root records;
- requires exactly one `status=0` exit for each child and init;
- rejects root/graph/component failures, transfer rollback, faults, panic,
  abort, unhandled paths, and wedged waiters.

The call gate is registered in the generic negative-control table with a pinned
count of 50 required markers. Its isolated control accepted its generated
baseline and rejected 71 mutations: each marker deleted in turn, the first two
transposed, and every failure marker appended.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A copied end dies when its first holder dies | `an_end_with_a_second_holder_survives_the_first_ones_death` | live queue count falls from 2 |
| Transit predicts one receiver task | `either_coholder_can_collect_from_the_same_destination_end` | token cannot arrive at the winning side holder |
| Transit leaks after destination loss | `destination_death_reclaims_only_after_the_last_holder_dies` | first holder reclaims early or last holder leaves an entry |
| Parent loses the endpoint after spawn | first causal chain in `sel4_call_check` | one of three supervision transfers or delegation marker is absent |
| C8.6 behavior regresses | remaining nine causal chains | exact missing/out-of-order semantic marker |
| A spawned task fails or never terminates | task-lifecycle check | missing, repeated, or non-zero child/init status |
| The gate loses evidence | isolated `check_gate(..., 50)` control | a mutated transcript is accepted or count drifts |

## Verification

| Command/scenario | Observed result |
|---|---|
| `just sel4_call_check` | Pass; 50 markers across 10 causal chains; five spawned tasks and init exited cleanly |
| `just test_sel4_root` | Pass; 113/113 across 13 modules |
| `just lint_all` | Pass |
| `just fmt_check_all` | Pass |
| `just ruff` | Pass |
| `just typos` | Pass |
| `just devlog_check` | Pass |
| Isolated call-gate negative control | Pass; 71 mutated transcripts rejected |
| `just sel4_gate_control_check` | Pass; 12 gates reject 535 mutated transcripts and layouts |
| `just sel4_channel_check` | Pass |
| `just sel4_spawn_check` | Pass |
| `just sel4_loan_check` | Pass |
| `just sel4_crossing_check` | Pass |
| `just sel4_root_boot_check` | Pass |
| `just sel4_component_graph_check` | Pass |
| `just sel4_sample_check` | Pass |
| `just sel4_supervision_check` | Pass |
| `just sel4_stream_check` | Pass |
| `just sel4_qos_check` | Pass |
| `just sel4_boot_layout_check` | Pass |

Every seL4 plane gate was re-run rather than only the call gate, because this
change alters marker text the other gates read. Four of them were red on the
first pass and are covered under Corrections below.

The QEMU gate rebuilt the image from the changed sources and observed the whole
composition end to end. Its terminal line was:

```text
transcript: 50 markers observed across 10 causal chains; five spawned tasks and init exited cleanly
```

## Decisions

**Copy endpoint grants, but put the side in authority.** A holder list would have
made queue state depend on task membership and complicated death, peer lookup,
and rollback. Putting `Side` in the capability deletes that representation:
queue meaning is intrinsic to authority, and co-holders are ordinary copied
capabilities.

**Bind transit to the queue end.** Selecting a receiver task at send time is
incorrect when co-holders race to dequeue. Binding to the receiving side keeps
bytes and capabilities under one delivery decision.

**Refuse ambiguous task derivation.** A shared channel end names a queue, not one
of its receivers. Where an operation must mint authority naming a concrete task,
ambiguity is a bounded refusal rather than table-order selection.

**Keep parent-vouched introduction.** The broker admits a participant only after
its parent sends the supervision handle over the participant's authenticated
control. No self-naming supervision authority or participant self-vouching was
introduced.

## Open risks and follow-ups

- `supervision_derive` remains a valid operation and is still guarded on the
  supervision plane, but it was not the mechanism that closed the endpoint half
  of B25.
- `ChannelTable::live_queues` now consults the same nameability predicate as
  `channel::sweep`. Both read every capability table, so the terminal accounting
  line is now O(channels × tables) once per boot rather than O(queues); it runs
  at teardown only.

## Artifacts and provenance

- Direct call-plane evidence: [`call-check.txt`](call-check.txt), captured from
  `just sel4_call_check` on 2026-08-08 with the pinned qemu-arm-virt profile.
- Root regression evidence: [`root-tests.txt`](root-tests.txt), captured from
  `just test_sel4_root`.
- Global negative-control record, wrong first reading retained beside its
  correction: [`gate-control-blocker.txt`](gate-control-blocker.txt).
- Historical diagnosis:
  [`devlog/2026-08-07-p5-4-6-call-spawn-semantics/`](../2026-08-07-p5-4-6-call-spawn-semantics/index.md).
- Earlier scaffolding diagnosis:
  [`devlog/2026-08-07-p5-4-6-call-plane/`](../2026-08-07-p5-4-6-call-plane/index.md).
- Related partial:
  [`devlog/2026-08-07-b25-supervision-derive/`](../2026-08-07-b25-supervision-derive/index.md).

## Corrections

**The `sel4_gate_control_check` failure was caused by this change, not
inherited.** It was first recorded here as pre-existing `sel4_spawn_plane`
registry drift. It was not. B25 deleted the root's `SLIME_GRAPH channel handed`
marker together with the move semantics that emitted it, and the spawn gate's
assertion that a child receives its end at *its own* slot 0 went with it instead
of being replaced — the 31-vs-32 pin was reporting exactly the lost coverage it
exists to report. `construct_child` now emits
`SLIME_GRAPH channel copied parent= child= key= side= slot=` per endpoint grant
installed, the spawn gate asserts `slot=0` again, and the pinned count is
unchanged. See [`gate-control-blocker.txt`](gate-control-blocker.txt), which
retains the wrong first reading beside its correction.

**Three further gates were red and are now fixed.** They were not re-run before
the entry was first written:

| Gate | Cause | Fix |
|---|---|---|
| `sel4_channel_check` | `channel end` gained a `side=` field mid-line; three pinned markers matched the old shape | Pins updated to `side=producer`/`consumer`/`loopback`, which asserts the new field rather than tolerating it |
| `sel4_loan_check` | `capability transfer … to=<task>` became `side=<side>`, and the loan refusal class became `absent-or-ambiguous` | Pins updated; the transfer pin now asserts the receiving side |
| `sel4_crossing_check` | Scenario defect, then a root defect. The driver treated a *minted* pair as a loopback and sent/received on one end; since B25 a mint is the oracle's `ipc::channel()` with two directed queues, so it could never deliver. With that fixed the boot completed but the terminal `queues=0` never appeared | Driver uses both ends. `live_queues` counted entries no capability table names — lazy-on-full sweep leaves them until the table refills, and their queues have no peer at all. It now filters on the same predicate `sweep` uses; regression test `an_entry_no_table_names_counts_no_live_queue` |

The last of those is a real root defect that the old representation hid: an
`Entry` cached a task per end, so `mark_dead` cleared `peer_alive` when that task
died and an unreachable entry never survived to be counted. With copied ends
there is no such task, so nameability had to become explicit.

