# Backlog (defects and unmasked debt)

**Purpose:** Track concrete defects, regressions, and latent bugs found in
implemented code that must be resolved before starting new roadmap-track
milestones. Backlog items are not new capability; they restore an already
claimed exit condition or remove debt that would compound under new work.

**Priority:** Backlog items are handled before roadmap-track milestones. A green
verification suite is a precondition for milestone work, not a milestone itself.
Clear or explicitly defer every open item here before opening a new track gate.

**Entry shape:** Each item states the problem, the evidence (how it was
observed), the proposed fix, and the exit condition that closes it. Close an
item only when its exit condition is observed, then move it to the resolved log
at the bottom rather than deleting it.

## Open

### B28 — a `retained` second route on one publisher stops a *different* publisher's parked role reply from ever being taken

**Problem:** On the P5.4.5 QoS plane, `fabric-publisher` parks once in `recv`
waiting for its role reply and never runs again, although the fabric delivers
both role capabilities to it — the transcript carries
`SLIME_GRAPH capability transferred task=9 channel=5 to=10 kind=endpoint` twice,
and `serve_cap_transfer` calls `deliver_wake` for each. It produces *zero*
further log lines and is still live at teardown, so the plane never reaches
`[init] fabric stream complete`.

**Bisected to one fixture field.** The trigger is `fabric-publisher-b`'s
*diagnostics* participant being `durability = retained` with
`retainedDepth = 2`. Flipping that one participant back to `volatile`/`0` and
rebuilding, with nothing else changed, makes `fabric-publisher` wake and print
`publish role received`. Flipping it to `retained` makes it park forever. The
affected task is a different component on a different route, which is what makes
this a defect rather than a scenario limitation.

**Two earlier readings, both ruled out by experiment.**

* *Starvation behind the clock driver.* `fabric-publisher-b` performs seven
  `advance_time`/`await_time_credit` round-trips, each re-waking the fabric, so
  the obvious reading was that task 10 is woken and never selected. Reducing the
  advance to a **single** step changes nothing — both transfers still land, the
  task still parks once, and it still never runs. Clock volume is not the
  variable.
* *Slow progress.* Extending the boot window from 200s to 700s changes nothing.

**Evidence:** `devlog/2026-08-07-p5-4-5-qos-clock/boot.log` for the retained
case. The stream plane, which is the same graph without the clock or the retained
diagnostics route, runs a byte-comparable transfer sequence and wakes the same
task at the same point.

**Not diagnosed to a line**, but the search is narrowed. What a second retained
route changes inside `fabric-service` is the untraced step: it adds a retained
history the broker maintains, and `create_late_subscriber` now finds a satisfying
publisher where it previously failed — so the broker takes a path it did not take
before, between the transfer and the point where the parked task would be served.

Two resource-exhaustion readings are also ruled out, so a bound is not the cause:

* *`retainedSamples` too small.* The graph declares `2` while two publishers now
  retain depth 2 each, which looks like the obvious ceiling. Raising it to `4` and
  rebuilding changes nothing — the task still parks forever.
* *Frame-table exhaustion.* `FABRIC_FRAME_CAPACITY` is 32 against a retained
  demand of 4, and the transcript carries no frame-exhaustion marker.

A fifth reading is ruled out too, and it moves the suspicion off the broker: the
late-subscriber path **works**. With the diagnostics route retained the transcript
carries `retained history offered to late subscriber`,
`retained history replayed to late subscriber`, and
`retained history expired for late subscriber` in order — and that replay is what
produces the `QoS lifespan expired` arm. The fabric's capability slots peak at 23
of 32, so it is not out of slots either. The broker is healthy and simply parks on
its stream sources with `fabric-publisher`'s request never served.

**The reply is not lost.** `ParkedReplies` is now instrumented: the root emits
`SLIME_GRAPH replies owed count=` and one `reply owed task=` per still-parked task
at teardown, and only when the set is non-empty, so every healthy plane gains no
line. On this boot the answer is a single owed reply belonging to task **6**,
which is `init` waiting on its children — expected and correct. `fabric-publisher`
is **not** in the list, although the transcript shows `parked task=10 reason=wait`
and no later activity from it.

So its wake *was* delivered: 33 park events across the boot, one owed at
teardown. The task resumed, consumed its reply, and then blocked inside seL4
without issuing another root call — which is why it emits nothing further and why
no root-side accounting shows it as outstanding. That excludes every lost-wake and
lost-reply reading, including the two this entry previously carried.

**The precise state, from the root's own accounting.** `parked=1` at teardown and
the owed list names task 6 alone, so task 10 left the parked table — it was woken.
It then issued **no further root call at all**: the transcript carries zero
`received task=10` lines after the wake, and `recv` is the only thing
`receive_role` does between waking and returning.

That is the contradiction to resolve. `receive_role` loops `recv` then
`wait(Endpoint(CONTROL_SLOT))`, both root operations, so a woken task must either
call `recv` again or park again. Task 10 does neither, and it prints nothing — not
even `publish role received`, which is the next statement after the two-capability
loop completes. A task that returns from `wait` and then makes no syscall is
either faulting silently or looping in userspace on a path with no root call in
it.

**The fault check is done and the path is found.** No fault marker appears — the
root reports them (`SLIME_GRAPH component fault`) — so task 10 did not fault. And
there *is* a userspace loop with no root-visible call in it, in the runtime rather
than the component:

`sel4_transport::wait` (`components/runtime/src/syscall/sel4_transport.rs:264`)
stages its source set through the transfer window, and on a staging failure it
calls `yield_now()` and **returns silently** — no `SYS_WAIT`, no error to the
caller, because `wait` returns `()`. `yield_now` is `sel4::r#yield()`, a kernel
primitive that never reaches the root. `receive_role` then loops back to `recv`,
and a caller that keeps failing to stage spins between the two forever while the
root sees nothing at all. That is exactly task 10's signature: woken, no further
root call, no fault, no output.

The comment there says "the caller re-polls either way", which is true only if the
next poll can succeed. When it cannot, the silent return converts a bounded error
into an invisible hang — and `wait`'s `()` return type is what makes it
unreportable.

**That arm is not the cause — refuted by instrumentation.** A temporary
`debug_write` on the staging-failure branch, rebuilt and booted, produces **zero**
lines. `wait` stages successfully every time on this plane, so the silent-yield
path is never taken and task 10 is not spinning there.

The park accounting is also self-consistent, which removes the last root-side
suspicion: 33 park events, one owed reply at teardown (task 6), and task 10 never
appears in a reclaim or peer-death line. Its park entry was therefore *consumed by
a wake* rather than abandoned. It resumed, and then made no root call by any path
the root or the runtime can report.

Seven readings are now excluded: lost wake, lost reply, starvation, clock volume,
boot duration, the `retainedSamples` bound, the frame table, a component fault, and
the runtime's silent-yield arm.

**Localized to the first `receive_role` iteration.** A marker compiled into
`fabric-publisher` between `role requested` and its two-capability loop prints
`awaiting role cap` and then nothing: the task blocks inside the *first* iteration,
never reaching `role cap arrived`. It is not stuck on the second capability, and it
is not past the loop.

The wiring is right, which is what makes this narrow. Task 10 holds the control
channel at the slot it reads — `channel handed parent=6 child=10 key=5 slot=0`
against `CONTROL_SLOT = 0` — and the fabric transferred both capabilities to that
exact channel (`capability transferred task=9 channel=5 to=10`, twice, rights
`0x1` then `0x2`). So the transfers targeted the queue the receiver polls, the
receiver polled it, and it saw nothing.

That points at `serve_cap_transfer`'s enqueue-plus-wake against a receiver that is
parked *at that moment*: the fabric's two transfers land back to back while task 10
is parked from its `wait`, and the second finds `deliver_wake` a no-op because the
first already un-parked it — but the first wake races the enqueue of the second
capability. On the stream plane the same pair lands and the receiver drains both,
so the ordering that breaks is graph-dependent, which is consistent with the
retained bisect.

That ordering was then read, and it is **correct**, so this reading is refuted too.
`Channel::commit_send` enqueues and `take()`s `recv_waiter`, so the first transfer
carries the wake and the second correctly returns `None`; the receiver is expected
to drain both messages once awake. The transcript confirms the order is favourable:
`parked task=10 reason=wait` precedes both transfers, so `deliver_wake`'s
`parked.reason(task).is_none()` guard cannot have skipped the first wake.

So: the receiver parks, two messages are enqueued on the queue it polls, the wake
is delivered to a task the root agrees is parked, its park entry is consumed, and
it never runs again. Every step is individually correct and the composition
deadlocks.

Two further readings were tested and refuted, which is worth recording because both
look compelling from the transcript:

* *Both ends of the loopback given away.* Init mints `key=5` as a loopback and the
  log shows it handed to child 9 *and* child 10, so init keeps neither end — which
  would leave the queue's `producer`/`consumer` naming only the two children. That
  is exactly what `reassign`'s loopback split is for, and the **stream plane does
  the identical thing** (`channel handed parent=6 child=9 key=5` then
  `… child=10 key=5`, same line shape, same order) and drains both messages. Not
  the cause.
* *Round-robin starvation.* On the stream plane task 10 runs only after the fabric
  parks and every other task blocks, so it is plainly last in the queue — and on
  the QoS plane the clock keeps the fabric busy. But the QoS fabric still parks
  **eight** times after the transfers, so task 10 has scheduling opportunities and
  does not take them. Not the cause either.

Ten readings excluded from the boot log. Both planes are byte-comparable through the
park and the two transfers, the rights on the control end are `send|recv` so
`WAIT_KIND_ENDPOINT` resolves, and every root-side structure reports consistent
state.

**The debugger settles it.** Booting under `-gdb tcp::1234`, letting the plane reach
its deadlock, and attaching `lldb` shows the CPU parked at `0x8060011190`, inside a
`b .` self-loop. Resolving that against the kernel's symbol table puts it in
`idle_thread` (`0x806001118c`, the symbol immediately below). **seL4 has no runnable
thread at all** — so task 10 is not spinning in userspace and not starving behind a
peer: it is blocked in the kernel on an endpoint nothing will signal.

That inverts the remaining suspicion back onto the root, with one specific
candidate. `parked::send_reply` ends with `slot.cap().send(info)` and **discards the
result**. Every accounting structure the root keeps is updated as though the reply
was delivered — the entry is removed from `ParkedReplies`, `recycled` is bumped, the
`reply owed` list is correspondingly empty for task 10 — while an `seL4_Send` that
failed would leave the child blocked forever with no trace anywhere. That is exactly
the observed combination: consistent root bookkeeping, an idle CPU, and a child that
never resumes.

**The send is not the loss either.** `sel4::cap::Unspecified::send` returns `()` —
seL4's `Send` reports nothing, so there was no discarded error to find. Bracketing
the call with markers shows `SLIME_DBG wake replying task=10` followed by
`wake replied task=10`: the root reaches the send, performs it, and returns. The
same bracket fires and *works* for tasks 4, 5, 7, 8, and 9 in the same boot, so the
save/park/wake/reply path is sound in general.

**So the defect is narrower than any structure the root can inspect.** Task 10's
reply is sent over its saved capability, the send returns, the CPU then goes idle
with no runnable thread, and the child never resumes. Every layer reports success
and the thread stays blocked. Eleven readings are now excluded, including the
debugger-motivated one.

**The reply capability is live — measured, not assumed.** `KernelDebugBuild` is
already `ON` in `sel4/config/qemu-arm-virt.cmake`, so `seL4_DebugCapIdentify` is
available; `Cap::debug_identify` was called on the saved slot immediately before the
send. Task 10's slot reports `kind=8`, which is `cap_reply_cap` in
`build/sel4-qemu/generated/arch/object/structures_gen.h:635` — and it is the *same*
kind reported for tasks 4, 5, 7, 8, 9, 11, and 12, every one of which wakes
correctly in the same boot.

So every root-side link is now measured and sound: the task parks, its reply is
saved as a genuine `cap_reply_cap`, two messages are enqueued on the queue it polls,
`deliver_wake` fires while the root agrees it is parked, the send is performed over a
capability the kernel confirms is a live reply cap, and the send returns. The CPU
then idles with no runnable thread and the child never resumes. **Twelve readings
excluded.**

**The kernel's own scheduler state confirms a true deadlock, not starvation.**
Reading it through the gdbstub with the kernel ELF loaded as a symbol target:

* `ksCurThread = 0x8060030c00`, the idle TCB — matching the `idle_thread` PC.
* `ksSchedulerAction = 0`, i.e. `SchedulerAction_ResumeCurrentThread`: the kernel
  has decided there is nothing to switch to.
* `ksReadyQueues[0]`, `[1]`, `[254]`, and `[255]` all have `head = NULL`. Priority
  254 is `CHILD_PRIORITY` and 255 is the root's, so **no thread at any priority is
  runnable**.

That closes the starvation question for good: every thread in the graph is blocked,
including the root. It also means the missing wake is not a scheduling artifact — a
runnable-but-never-selected thread would sit in `ksReadyQueues[254]`, and it does
not.

So the state is fully characterized and internally contradictory at the seL4
boundary: the root sent a reply over a capability the kernel identifies as a live
`cap_reply_cap` naming a blocked thread, and that thread did not become runnable.
Thirteen readings excluded.

**A TCB state read deepens the contradiction rather than resolving it.**
`ksDebugTCBs` (the kernel's debug thread list, available because
`KernelDebugBuild` is on) heads at `0x80604f6c00`, and `tcbState` is the first field
of `tcb_t`. Reading it there gives word 0 = `0x1`, which is
`ThreadState_Running` in `deps/sel4/include/object/structures.h:160`.

So at the deadlock there is a thread the kernel considers **Running** while
`ksReadyQueues` is empty at every priority and `ksCurThread` is the idle TCB. Those
three facts cannot all be consistent with a healthy scheduler: a Running thread
belongs in a ready queue or is current, and this one is neither.

That is the sharpest statement available and it is worth stopping on rather than
guessing past. Fourteen readings excluded. Two candidates remain, and they are
different bugs:

* the thread was made Running and then never enqueued — a missing
  `SCHED_ENQUEUE`, which on this path would be inside seL4's own reply handling;
* or the TCB at the head of `ksDebugTCBs` is not the thread this concerns, and the
  Running state belongs to something else entirely — in which case walking
  `tcbDebugNext` to identify each thread is the remaining read.

**The second candidate is now settled: it is a real child thread.** This build has
no `tcbDebugNext` field, so `ksDebugTCBs` is not a walkable list here — but the TCB it
points at can be identified directly. At `0x80604f6c00`, `tcbPriority` (offset 920)
reads **254**, which is `task::CHILD_PRIORITY`. The idle thread is a different object
at `0x8060270000`-ish with `tcbPriority = 0`. So the Running TCB is one of the
graph's own components, not the idle thread and not the root.

**The inconsistency is therefore confirmed at the kernel level:** a child thread at
priority 254 in `ThreadState_Running`, absent from `ksReadyQueues[254]` (and from
every other priority's queue), while `ksCurThread` is the idle TCB and
`ksSchedulerAction` is `ResumeCurrentThread`. A Running thread that is neither
current nor enqueued cannot be scheduled again, which is exactly the observed hang.

Fifteen readings excluded, and the defect is now located to one transition rather
than a subsystem: something set a child's state to Running without enqueuing it, on
the path a reply to a parked task takes. That is either seL4's own
`setThreadState`/`possibleSwitchTo` sequence for a reply-send to a thread blocked in
`Recv`, or a root-side invocation that leaves the thread in Running without the
kernel completing the switch.

**Thread naming was tried and does not help on this build.** `seL4_DebugNameThread`
is exposed by `rust-sel4` as `cap::Tcb::debug_name`, and calling it at spawn compiles
and boots cleanly — but this kernel has no `tcbName` field at all, so nothing stores
the label and no dump can report it. `KernelDebugBuild ON` gives `DebugCapIdentify`
and the `ksDebugTCBs` pointer without the naming storage that
`CONFIG_DEBUG_BUILD`'s thread-name support would add. The change was reverted rather
than left as a call whose effect is unobservable.

**The thread is identified: it is task 10, `fabric-publisher` itself.** Matching was
done through the IPC buffer rather than the VSpace, because the root already prints
the derived address. The Running TCB reports
`tcbIPCBuffer = 0x237000`; `child_vspace.rs` sets `ipc_buffer_addr = footprint.end`
and places the transfer window one page above it, so that TCB's window is
`0x238000` — and the transcript's `window bound task=10 base=0x238000` names exactly
one spawned task with that address. Task 10 is the `fabric-publisher` instance init
spawned, which is the thread that never wakes.

Its saved context is consistent with a live component rather than a fresh one:
`registers[31]` (PC) is `0x2366f0`, far above the `entry=0x211e78`
`fabric-publisher` was started at.

**So the defect is now stated exactly.** `fabric-publisher`'s thread is in
`ThreadState_Running` with a plausible mid-execution PC, absent from
`ksReadyQueues` at every priority, while `ksCurThread` is the idle TCB and
`ksSchedulerAction` is `ResumeCurrentThread`. The root has sent it a reply over a
capability the kernel identifies as a live `cap_reply_cap`. Sixteen readings
excluded; every layer above the scheduler checks out.

**The kernel's reply path was read and it explains the state without being wrong.**
Non-MCS `doReplyTransfer` (`deps/sel4/src/kernel/thread.c:133`) opens with
`assert(thread_state_get_tsType(receiver->tcbState) == ThreadState_BlockedOnReply)`.
On success it does `cteDeleteOne(slot)`, `setThreadState(receiver, Running)`, then
`possibleSwitchTo(receiver)` — so the enqueue is not missing from the kernel.

`possibleSwitchTo` is where a Running thread can legitimately end up in no queue:
when the target shares the current domain and `ksSchedulerAction` is
`ResumeCurrentThread`, it takes neither `SCHED_ENQUEUE` branch and instead sets
`ksSchedulerAction = target` — a *pending switch* held outside the ready queues.
`schedule()` consumes that correctly, so the design is sound; but the measured state
at the deadlock is `ksSchedulerAction = 0` (`ResumeCurrentThread`) with the target
Running and unqueued, which is that pending switch having been **cleared without
being honoured**.

**So the shape of the bug is now pinned even though the culprit is not.** Something
between `possibleSwitchTo` recording the switch and `schedule()` acting on it reset
`ksSchedulerAction` to `ResumeCurrentThread` — plausibly a second
`possibleSwitchTo`/`rescheduleRequired` interleaving from another root operation in
the same kernel entry, which the root's single-threaded dispatch makes possible when
one syscall replies to two different tasks. That is consistent with B28 appearing
only when the retained diagnostics route adds a second reply-bearing path.

**The multi-reply interleaving exists and is observable.** `reclaim_dead_task`
(`slime-root/src/main.rs:4281`) loops over `DeathWakes` and calls `deliver_wake` — so
`send_reply` — once per wake, all inside one kernel entry. The QoS transcript records
`peer death task=3 channels=5 woken=2`: two tasks replied to in a single root
operation. Each `seL4_Send` on a reply cap runs `possibleSwitchTo` for its receiver,
and the second one's call sees `ksSchedulerAction` already holding the first target
rather than `ResumeCurrentThread` — the branch that then fires is
`rescheduleRequired()` plus `SCHED_ENQUEUE`, which enqueues the *first* target and
requests a reschedule.

**But the timing refutes it as task 10's cause.** The only `woken=2` line in the
transcript is at boot-log line 184, and task 10 does not park until line 283. A
pending switch that never existed cannot have been cleared, so this interleaving —
real as it is — is not what strands `fabric-publisher`. Every later wake in that boot
is `woken=0` or `woken=1`, i.e. one reply per kernel entry.

Seventeen readings excluded. That leaves the contradiction fully measured and
unexplained by any mechanism inspected so far: a `Running` child at priority 254,
absent from every ready queue, `ksSchedulerAction = ResumeCurrentThread`,
`ksCurThread` idle, reached after a single reply send over a live `cap_reply_cap`.

**Exit condition unchanged.** The remaining approaches are both heavier than anything
tried so far, and choosing between them is the next decision rather than the next
command: single-step the kernel from the reply send that targets task 10 (a gdbstub
watchpoint on `ksSchedulerAction` plus a breakpoint on `possibleSwitchTo` would show
the write and the caller), or rebuild seL4 with thread-name support so a
`DebugDumpScheduler` names every thread and its state in one shot. The first needs no
rebuild; the second makes every future seL4 investigation cheaper.

**One wider finding stands regardless of B28**, and it is worth its own slice:
`sel4_transport::wait` returns `()`, so its staging-failure branch can only
`yield_now()` and return silently. It is unreachable on every current plane — hence
the zero lines above — but if it were ever reached it would convert a bounded error
into an invisible hang, exactly the signature that made this defect take seven
attempts to characterize. It should either report or be made impossible by
construction.

**Severity:** Blocks P5.4.5's exit condition and nothing else. Latent for every
other plane: no other seL4 graph declares two retained routes on one publisher.
The tradeoff is quantified — `retained` yields five observed C8.5 arms with
`fabric-publisher` parked, `volatile` yields three with it running, and neither
reaches the final marker — so the committed fixture keeps `retained` as strictly
more coverage.

**Exit condition:** With the diagnostics route `retained`, `fabric-publisher`
takes its role reply and the plane reaches `[init] fabric stream complete`,
asserted by a gate, with a fault injection showing the parked case caught.

### B25 — a spawn-granted endpoint moves on seL4 and copies on x86, so a parent cannot broker a later introduction

**Problem:** `slime-root`'s `distribute_channel_ends` (`slime-root/src/main.rs`)
treats an endpoint named by a spawn grant as a **move**: it reassigns the
channel's holder to the child and calls `table.drop_slot` on the parent's slot.
The retired kernel copies: `preflight_spawn_grant`
(`kernel/src/task/mod.rs:286`) performs `cap.derive(grant.rights)` at `:320`
into a fresh vector that `spawn_with_caps_for` (`:402`) installs into the
child, and neither reads nor mutates the parent's table — so the parent keeps
its end.

That difference is invisible to every component that hands an end away and
never touches it again — which is every component in the nine passing planes —
and fatal to any composition where a parent grants one end at spawn and then
*uses* that channel itself. The x86 call plane is exactly such a composition:
`init.rs::launch_fabric_calls` spawns `fabric-service` with all four service
halves, keeps them, and afterwards moves each participant's supervision handle
to the broker with `cap_transfer` over the matching half.

**Not a slot-numbering defect.** Two earlier versions of this entry blamed
`SlotCursors::take`'s `used_slot_zero`, first as a slot *collision* and then as
a slot *gap*. The gap is real, but it was a consequence of declaring the
control channels as **generation grants** — the root then numbers a launched
component's ends from its own cursor, which resumes above the factory grants
staging installed. Having `init` mint the pairs and hand them out at spawn
removes it, because `construct_child` installs a child's grants at `0..count`
in the requested order. Observed with the pairs minted: the fabric's four
controls arrive as `channel handed parent=5 child=6 … slot=2,3,4,5`,
contiguous above the two factory grants at the head of its grant array.

The grants themselves stay in the manifest, which the first attempt at this got
wrong by deleting them. `_control_sources`
(`scripts/build/build-generation.py:833`) derives `FABRIC_CALL_CLIENTS` — the
table the broker maps a control slot to a caller identity with — from exactly
those four grant *names*, and in `FABRIC_CALL_CONTROL_GRANTS` order rather than
the builder's `(name, source, target)` sort. Removing them emptied the table
and tripped `request_response_controls`' four-control assert before the broker
read a slot. They are the naming source; the minted endpoints are the
authority.

**Evidence:** `devlog/2026-08-07-p5-4-6-call-spawn-semantics/`. With the plane
rebuilt to mint its control pairs, the boot reaches
`SLIME_GRAPH channel handed parent=5 child=6 key=4 slot=2` — the fabric's end
arriving *and* init's slot being dropped in one step. Every participant's role
request then reaches the broker (`SLIME_GRAPH received task=4 channel=2`) and
is never answered: `Broker::provision` blocks in `consume_supervision` awaiting
a handle no one on this plane can send, and the graph ends `live=10`,
`parked=8`, `transfers served=0`.

The obvious alternative — each participant sending a handle naming itself — is
not constructible. `serve_spawn` installs a supervision handle only into the
**parent's** table, and only after `construct_child` has built the child's
(`slime-root/src/main.rs:3586-3603`), so no component ever holds a handle
naming itself.

**Narrowed by experiment, 2026-08-07.** Inverting the call plane's spawn order
*does* carry the supervision handoff, so the endpoint-move semantics alone are
not the whole blocker. Spawning the participants first with the *participant*
half of each control pair, keeping the *service* half in init, transferring each
participant's handle over it, and spawning the fabric last with the service
halves reached `[init] call supervision delegated` — the step this entry was
filed for. Both halves of a pair are still granted exactly once, so no
`drop_slot` takes anything init needs later.

What that order cannot then deliver is the **fabric's own** handle, and for a
second, independent reason. Two participants lend to the broker
(`fabric_call_scenario`'s `send_large_request` and `send_large_reply`), so both
need a `RIGHT_SUPERVISE` capability naming the fabric at their
`FABRIC_SUPERVISION_SLOT`. A *spawn grant* copies (`preflight_spawn_grants`
installs `held.resource` and leaves the parent's slot), which is how
`drive_sample_plane` hands one handle to a lender — but it requires the fabric
to exist first, which is the order this experiment inverted. A *transfer* moves
(`serve_cap_transfer` calls `table.drop_slot` on the source), and
`FLAG_RETAIN_TRANSFER` keeps the delegation bit at the destination without
making the move a copy — so one handle reaches one receiver. Init cannot obtain
a second, because `bootstrap_executable_slot` resolves an executable by
component identity to exactly one slot and each spawn returns one handle.

So the two requirements are order-incompatible as the components are written:
the control ends want the fabric spawned last, the fabric handle wants it
spawned first. That is a sharper statement than "the grant moves", and it means
the fix is still a model decision rather than a composition detail. Observed
directly; the experiment was reverted and the tree is back to the committed
plane.

**Severity:** Latent for every current plane, and a hard blocker for any plane
whose parent must broker an introduction after spawning. It is a genuine
*semantic* divergence from the frozen oracle, not a numbering accident, so it
cannot be resolved by re-blessing a fixture.

**Proposed fix:** Decide which semantics the model wants and make both
implementations agree, rather than working around it per plane. A copy matches
the oracle and keeps `init.rs` portable, but it means two tasks name one
channel end and `ChannelTable` resolves queues by holder — so the copy needs a
holder model that admits more than one. A move is the cheaper invariant and is
arguably the more capability-honest one, but then the oracle's own call plane
is not portable as written and `launch_fabric_calls` needs restructuring.

The experiment above adds a third option, cheaper than either and worth
weighing first: let a component obtain a **second** handle naming a task it
already supervises, so a broker's handle can reach both of its lenders without
the fabric having to be spawned before the participants. The narrow form is a
`supervision_derive`-style operation returning a fresh capability naming the
same task, which is a copy of authority the caller already holds and widens
nothing. That would make the inverted order carry the whole plane, leaving the
endpoint move/copy question a real but no longer blocking difference.

**Exit condition:** A parent grants one end of a minted pair at spawn, uses the
other end afterwards to deliver a capability to that child, and the child
observes it — asserted on a plane that declares such a composition, with a
fault injection showing the parent's end going missing is caught. The call
plane's `[init] call supervision delegated` marker is that composition, already
observed; what remains is for the plane to get past it.

### B12 — the component build's `--remap-path-prefix` names a path that does not exist

**Problem:** `components/.cargo/config.toml` passes
`--remap-path-prefix /home/iceice666/projects/slime_os=.` for both the
`x86_64-unknown-none` and `aarch64-unknown-none` targets. The current checkout is
`/home/iceice666/projects/slime_os-sel4-cutover`. Because the stale literal is a
*prefix* of the real path, the flag does not simply miss: it rewrites the leading
portion and leaves `-sel4-cutover/...` behind, so recorded paths are mangled
rather than normalized, and a checkout at a different directory still produces
different bytes.

The determinism claim this flag exists to support is therefore weaker than it
reads. `just generation_check` still passes, because it builds twice from *one*
checkout — the property it verifies is reproducibility across runs, not across
source paths. `build-sel4.py` closes the same leak properly for the kernel with
`-ffile-prefix-map` onto fixed logical roots (`/slime/sel4`, `/slime/build`), and
P5.1's devlog records two builds from different source paths as byte-identical
on that path.

**Evidence:** `components/.cargo/config.toml:11` and `:21` against `pwd`. Noted
while adding the seL4 target in P5.2; see
`devlog/2026-08-04-p5-2-native-component-images/`.

**Proposed fix:** remap from the repository root as computed at build time rather
than from a hardcoded literal — the builder already knows it (`ROOT` in
`scripts/build/build-generation.py`), and the seL4 path passes
`--remap-path-prefix={ROOT}=.` explicitly for exactly this reason. Deciding
whether the mapped-to token should match `build-sel4.py`'s `/slime/...`
convention is part of the fix.

**Why deferred rather than fixed in P5.2:** changing the frozen x86 oracle's
build inputs alters every component ELF it produces, and therefore the
authenticated identity of every generation the oracle's gates assert against.
That is a larger blast radius than the defect, and it is orthogonal to native
seL4 component images. The seL4 target is unaffected: it inherits none of these
rustflags (they are keyed by triple) and passes its own.

**Exit condition:** two builds of the same generation from two different
checkout directories produce byte-identical component images and the same
generation identity, with `just generation_check`, `just product_boot_check`,
and `just test` unchanged.

**Deferral re-reviewed 2026-08-05, before opening P5.5.2's gate**, on the same
reasoning: that slice replaces the seventh seL4 generation through the same
build path, whose rustflags are keyed by triple and match none of the stale
literal's. See `devlog/2026-08-05-p5-5-2-stream-plane/`.

**Deferral re-reviewed 2026-08-05, before opening P5.5.1's gate**, on the same
reasoning: that slice adds a seventh seL4 generation through the same build
path. See `devlog/2026-08-05-p5-5-1-typed-fabric/`.

**Deferral re-reviewed 2026-08-05, before opening P5.3.4's gate**, on the same
reasoning: that slice adds a sixth seL4 generation through the same build path,
whose rustflags are keyed by triple and match none of the stale literal's. See
`devlog/2026-08-05-p5-3-4-sample-plane/`.

**Deferral re-reviewed 2026-08-05, before opening P5.3.3's gate**, on the
reasoning recorded below: that slice adds a fifth seL4 generation through the
same build path, whose rustflags are keyed by triple and match none of the stale
literal's, so it neither touches the defect nor extends its reach. See
`devlog/2026-08-05-p5-3-3-spawn-plane/`.

**Deferral re-reviewed 2026-08-04, before opening P5.3.2's gate** on the same
reasoning: that slice adds a fourth seL4 generation through the same build path,
so it neither touches the defect nor extends its reach. See
`devlog/2026-08-04-p5-3-2-loan-plane/`.

**Deferral reviewed 2026-08-04, before opening P5.3.1's gate.** Still deferred,
on the reason recorded above rather than by omission. B12's own analysis
establishes that the seL4 target is unaffected: `components/.cargo/config.toml`
keys its rustflags by triple, the seL4 component build matches none of them
(it uses a JSON target specification), and `build-generation.py` passes
`--remap-path-prefix={ROOT}=.` explicitly on that path for exactly this reason.
P5.3.1 adds a second seL4 generation built through that same path, so it neither
touches the defect nor extends its reach. Fixing it still means rebuilding every
frozen x86 component image and re-authenticating every generation identity the
x86 gates assert against — a blast radius larger than the defect, and orthogonal
to the seL4 cutover. It should be scheduled against the x86 oracle deliberately,
not folded into a portability slice.

**Deferral re-reviewed 2026-08-07, before opening P5.4.1's gate.** Still
deferred, on the same reasoning. B16's fix adds an eighth seL4 generation and a
new component binary built through the same JSON-target path, which the
rustflags this defect concerns do not match, so the reach is unchanged once
again. `just generation_check` and `just contracts_check` were run to confirm
the new binary perturbed neither contract validation nor generation identity.
See `devlog/2026-08-07-b16-supervision-records/`.

**Deferral re-reviewed 2026-08-07, before opening P5.4.1's own gate.** Still
deferred, on the same reasoning once more. B22's fix adds a ninth seL4
generation and a new component binary through the same JSON-target path, whose
rustflags this defect does not match, so the reach is unchanged.
`just generation_check` and `just contracts_check` were run to confirm the new
binary perturbed neither contract validation nor generation identity. See
`devlog/2026-08-07-p5-4-1-oracle-inventory/`.

## Resolved

### B29 — `ParkedReplies::wake` never deleted the reply CSlot it counted as recycled — **resolved 2026-08-07**

**Problem:** `slime-root/src/parked.rs` has three paths that finish with a saved
reply capability, and only two released it. `answer_saved` and `discard` both go
through `release_slot`, which calls `delete_slot` *and* bumps `recycled`. `wake`
— the path every parked task takes — called `send_reply` and then bumped
`recycled` directly, with no `delete_slot`. So each parked wake left a root CSlot
holding a spent reply capability while reporting it as recycled.

**Found by** reading the three paths side by side while chasing B28. Not by a
failure: the boot's own counters cannot see it. `recycled` was already
incremented, so the terminal `replies=` figure is identical before and after the
fix (323 on the QoS plane both ways), and `tasks reclaimed … slots=` is unchanged
too (517). That is exactly what makes it worth recording — the accounting said
"recycled" and the CSlot was still occupied, so the number that exists to prove
the save path is not a leak was the number hiding one.

**Severity:** Latent, and bounded per boot rather than per operation only because
the graphs are short-lived. A long-running graph that parks and wakes repeatedly
consumes one root CSlot per wake with nothing reclaiming it; the QoS plane alone
parks 33 times. It is the same shape as B22, B23, and B24 — a table with no free
path — one level down, in the allocator rather than a table.

**Resolved by** `wake` calling `release_slot(held.slot)` after `send_reply`,
which is the path the other two already took. `recycled` is bumped by
`release_slot`, so the counter's meaning is now uniform across all three.

**Exit condition observed.** All nine seL4 plane gates, `sel4_boot_layout_check`,
and `test_sel4_root` (109/109) pass with the fix; the five C8.5 arms on the QoS
plane are unchanged. The counters are identical by construction, so the guard
against regression is that all three paths now call one function — a future
fourth path leaks only by not calling it.

### B27 — the manifest→flag table set and scrubbed in one pass, so two manifests could not share a flag — **resolved 2026-08-07**

**Problem:** `build_sel4_generation`'s manifest→flag loop
(`scripts/build/build-generation.py`) set the selected manifest's flag and
popped every other manifest's in the same iteration. With one flag per manifest
that is correct. The moment two manifests declare the same flag it is not: a row
later in the table pops what an earlier row set, and which one wins depends on
table order rather than on the selection.

**Found by** P5.4.5's QoS plane, which is the stream driver plus a clock and so
declares `SLIME_SEL4_STREAM_CHECK` alongside the oracle's
`SLIME_FABRIC_QOS_CHECK`. Adding the `sel4-qos` row *after* `sel4-stream`
cleared the stream plane's own flag, and `just sel4_stream_check` failed with
`boot exceeded 180s without reaching the final marker` — init fell through to
`[init] launching component graph` and spawned nothing. Observed directly, and
worth recording because the failure is a timeout rather than an error: nothing
said "flag missing", and the plane simply ran a different composition.

**Resolved by** collecting the selected manifest's flags into one set and every
flag the table declares into another, then setting the first and removing the
rest. A flag two manifests share now survives for whichever asked for it,
independent of row order.

**Exit condition observed.** `just sel4_stream_check` passes with the
`sel4-qos` row present, and the QoS plane's own boot shows both flags in effect
— it runs `drive_stream_plane` and its components take the QoS path. All nine
seL4 plane gates pass with every image rebuilt. See
`devlog/2026-08-07-p5-4-5-qos-clock/`.

### B26 — the `[layout]` dump reported the grant's rights, so a too-permissive layout row was unobservable — **resolved 2026-08-07**

**Problem:** `slime-root/src/main.rs` printed each layout row's rights from the
*installed capability*, which `launch_component_graph` fills from the
**generation grant**, rather than from the boot-layout entry the row exists to
freeze. `bootstrap_executable_slot` and `bootstrap_slot` test *containment*
(`rights & !entry.rights != 0`) rather than equality, deliberately and
correctly — a layout marks a channel half `RIGHT_TRANSFER` because init hands
it on, while the grant is not about delegation at all, and requiring equality
rejected a well-formed graph once already. So the two legitimately differ, and
a dump carrying only one of them could not show a layout declaring strictly
more authority than anything uses. B10 exists to keep the table that declares a
slot and the table that fills it in agreement; this was the one direction of
disagreement the gate was blind to.

**Found by** fault-injecting P5.4.6's call plane: changing
`SEL4_CALL_LAYOUT`'s `fabric-call-server` row from `0x10008` to `0x1000c`
rebuilt the generation to different bytes (verified by md5) and the gate still
passed, while swapping two slot *numbers* in the same table was caught
immediately. That contrast is what localized the gap to rights.

**Resolved by** `declared_layout_rights`, which resolves the layout entry
behind a bootstrap row — by identity for an executable, by role for the two
singular factories — and appends `declared=0x…` when it differs from the
installed value. Appended and only on disagreement, so every row that agrees
keeps the retired kernel's exact four fields and stays comparable to
`dump_boot_layout`'s output slot for slot. `check-sel4-boot-layout.py`'s
`ENTRY` pattern admits the optional tail.

A channel end is deliberately not covered: it is named by its *grant*, and one
capability can be reached by more than one grant name, so reporting a declared
value would mean picking one. Executables and the two factories are where a
layout row's rights are unambiguous, and they are the rows a layout edit
touches.

**Exit condition observed.** The previously-invisible `0x10008`→`0x1000c`
injection now fails the gate, reporting
`now: [layout] 5 executable fabric-call-server 0x10008 declared=0x1000c`
against the frozen row. Restored and re-verified green.

The fix immediately earned itself: re-blessing surfaced three *pre-existing*
disagreements nothing had ever reported — `sel4-loan`, `sel4-sample`, and
`sel4-stream` each declare `0x1000004` on their shared-buffer-factory row while
the root installs `0x1000000`. Those are legitimate containment differences,
now recorded rather than invisible. See
`devlog/2026-08-07-b26-layout-declared-rights/`.

### B24 — `SharedBufferTable::quotas` never reclaimed, so `MAX_CHARGE_HOLDERS` was a lifetime bound — **resolved 2026-08-07**

**Problem:** B16's and B22's defect shape in a third table, and the one B16's
sweep implicitly cleared. `slime-root/src/shared_buffer.rs:502` declares
`quotas` one line below `charges`, which B16 named among the correct tables.
`charges` **is** correct — `uncharge` frees it at `:1782-1784`. `quotas` had no
free path anywhere: `declare_quota` reuses a slot only for the same `HolderId`
and otherwise takes a fresh one, while `commit_teardown`, `reclaim_holder`, and
`advance_epoch` never mentioned it. Because `construct_child` keys it by task id
and `TaskTable::next_id` never rewinds, a spawn/reap graph presented a fresh
holder every time and the 96 slots bounded the holders a boot could **ever**
construct.

Found by P5.4.1's lifetime-vs-live class audit rather than one at a time, which
is the reason that audit was scoped as a class: `quotas` is *keyed* per-task but
*declared* per-component at boot, so it does not read as a per-task table at a
glance and B16's per-task sweep passed over it.

**Resolved by** `release_quota`, called from `reclaim_dead_task` after charge
settlement — the ceiling outlives every charge made against it and is dropped
only once nothing can be charged again. A **direct release rather than a derived
sweep**, unlike B16 and B22: a quota has exactly one holder and that holder is a
task, so "the task is gone" is complete information. Those two needed predicates
because a supervision handle or a channel end can be named by a capability that
outlives the task; a quota cannot.

**Exit condition amended, and why.** The condition recorded when this item was
opened asked for a graph constructing more than `MAX_CHARGE_HOLDERS` holders.
That is unreachable: root CSlots are deliberately never returned
(`task.rs:165-167`), and the supervision plane's 35 spawns consume 2321 of 3457,
so a boot exhausts CSlots near 52 tasks and cannot reach 97. Stretching the
evidence to fit the original wording would have been the wrong move; the
condition is restated to what the platform can carry.

**Exit condition (observed 2026-08-07):** every constructed holder releases its
declared ceiling when its task dies, observed under `just sel4_supervision_check`
— 38 holders constructed over one boot, 38 `SLIME_GRAPH quota released` lines,
and `quotas=0` on the terminal accounting — and fault-injected to show that
disabling the release leaves `quotas=38`. Asserted on that existing plane rather
than a tenth image, since it is already the deepest spawn/reap loop in the
corpus. See
[`devlog/2026-08-07-b24-shared-buffer-quotas/`](../devlog/2026-08-07-b24-shared-buffer-quotas/index.md).

**Follow-up recorded, not opened:** root CSlot non-reuse is now the binding
lifetime constraint on graph longevity, ahead of every table this class audit
examined. Deliberate and documented rather than a defect, but P5.4.1 classified
it as acceptable-monotonic without quantifying it.

### B23 — `slime-root`'s unit tests were run by no gate — **resolved 2026-08-07**

**Problem:** 102 `#[test]` functions across 13 modules were compiled by nothing
and run by nothing, while `slime-root/src/main.rs` described those modules as
"bounded, pure, and unit-tested in place". Two independent blockers: no Justfile
target named the crate, and it could not have run anyway — `main.rs` is
unconditionally `#![no_std]`/`#![no_main]`, the package declared no lib target,
and the crate built only for a seL4 JSON target with no `libtest`.

**Resolved by** splitting the mechanism modules into a `slime_root` library the
binary links, rather than a `cfg(test)` escape (which neither blocker admits) or
a separate test crate (whose passing tests would be evidence about a copy). The
`sel4` crate builds for a host target given `SEL4_PREFIX`, so nothing had to be
excluded: all 13 covered modules run, including the seL4-touching ones.
`sel4-root-task` is scoped to `cfg(target_os = "none")` because it pulls
`sel4-alloca`, whose inline ELF section directive will not assemble on Mach-O;
only the binary needs it and the seL4 build is unchanged.

**What the first run found, which is the point:** three latent defects, every
one a test silently wrong since something changed under it. Nine `push` call
sites had been stale since P5.3.2 added a `transferable` parameter. An
`elf_header` fixture was 20 bytes against `LEGACY_HEADER_LEN`'s 32, so it had
been asserting `Unrecognized` rather than the bare-ELF arm ever since
`component_image::target` gained its length guard. A `qualified` fixture sized
its tail with a literal that no longer matched. All three are test bugs rather
than production bugs — the good case, but not evidence that nothing was hiding.

**Exit condition (observed 2026-08-07):** `just test_sel4_root` runs 102 tests
across 13 modules and asserts the count, so a module that stops being covered is
visible. It is a gate of its own rather than a `test_host` arm, because it needs
the installed seL4 prefix that `test_host`'s CI runner does not build — the same
reason `lint_sel4_root` stands apart. Fault-injected by removing one `transit`
test: the gate fails with `ran 101 tests, expected 102`. The nine seL4 gates,
`just generation_check`, and `just contracts_check` are unchanged, so the lib
split did not disturb the image. See
[`devlog/2026-08-07-b23-slime-root-host-tests/`](../devlog/2026-08-07-b23-slime-root-host-tests/index.md).

**Noted, not fixed:** `just test_host`'s `slime-proto` arm pins
`x86_64-unknown-linux-gnu` and therefore fails on an `aarch64-apple-darwin`
host, which was true before this change and is confirmed by stashing it.
`test_host` is left untouched — this fix adds no arm to it, and
`test_sel4_root` uses the host triple.

### B22 — `ChannelTable` never reclaimed, so `MAX_CHANNELS` was a lifetime bound — **resolved 2026-08-07**

**Problem:** B16's exact defect shape in a second table.
`slime-root/src/channel.rs` never freed an entry: `push` derived its key as
`self.len` (`:446`), `mark_dead` (`:339-354`) marked both queues of a dying
task's channels dead but freed nothing, and `reassign` only rewrote the holder
fields. So `MAX_CHANNELS` (32) bounded the channels a boot could **ever** mint,
not those live at once, and every channel a long-running graph minted was spent
permanently.

**How it differed from B16, and why that changed the fix's evidence:** B16
dropped a record *silently* and hung the parent, so converting the failure into
a reported one was part of its fix. B22's was already a bounded refusal —
`ChannelError::TableFull` becomes `IpcError::DestinationSlotsExhausted` — so
"the failure became reportable" proves nothing here. The gate could only be
satisfied by the graph *succeeding* past 32. The downstream symptom was the real
cost: a refused `mint` surfaces in the component, and at `MAX_CHANNELS = 16` the
stream plane's exhaustion "read as four broken components rather than one
exhausted table" (`channel.rs:107-111`). The bound had already been crossed once
and raised rather than fixed.

**Resolved by** `channel::sweep(&mut ChannelTable, &GraphTables, &Transit)`,
which frees every entry no live holder can name — derived from state that
already exists, exactly as `supervision::sweep` is. Two predicates, not one:
`GraphTables::holds_endpoint` for the live half and `Transit::holds_endpoint`
for the in-flight half, because `serve_cap_transfer` drops the capability from
the sender's table *before* parking it, so a sweep reading only the graph would
free the channel a transfer is mid-way through moving.

A precondition came with it: `key = self.len` had to become a monotonic
`next_key`. That derivation is unique only while `len` never decreases — once
the sweep frees an entry, the next `push` would reissue a key some live
capability already names, and `Resource::Endpoint { channel }` is the only
handle a component holds. That would have converted an exhaustion bug into
confused-deputy redirection, which is strictly worse.

The sweep is lazy, firing on `TableFull` and retrying, for B16's reason: one
trigger condition is one thing to keep correct, and a channel that stays is a
channel that still works.

**Exit condition (observed 2026-08-07):** `just sel4_crossing_check` boots a
graph that mints 33 pairs against a 32-entry table and still sends and receives
on every live channel, including a pair held across the crossing and an end
parked in `Transit` across it. The transcript records the first sweep as
`freed=28 live=4 minted=32` and the terminal line as `minted=37`; what the gate
*asserts* is looser and deliberately so — a nonzero `freed` on the sweep line
and a terminal `minted` in 33..=99, since pinning exact counts would break on
unrelated allocator changes while the loop-vs-bound arithmetic is enforced
separately from source. Three fault injections confirmed failing:
removing the sweep dies at the 33rd mint, removing the `Transit` half of the
predicate loses the in-flight end, and restoring `key = self.len` trips the
gate's key-derivation source check. The other eight seL4 gates,
`just generation_check`, and `just contracts_check` are unchanged. See
[`devlog/2026-08-07-p5-4-1-oracle-inventory/`](../devlog/2026-08-07-p5-4-1-oracle-inventory/index.md).

**Follow-up opened:** [B24](#b24--sharedbuffertablequotas-never-reclaims-so-max_charge_holders-is-a-lifetime-bound),
a third table of the same shape found by the same class audit.

### B21 — the toolchain was pinned by name, so each host resolved a different binary — **resolved 2026-08-06**

**Problem:** `flake.nix` pinned the seL4 cross toolchain by *name*
(`CROSS_COMPILER_PREFIX = crossCC.targetPrefix`), and `build-sel4.py` passed
that bare prefix to CMake, which resolves `${prefix}gcc` through `PATH`. A name
is not an identity. `pkgsCross.aarch64-multiplatform.stdenv.cc` is a *cross*
wrapper on `aarch64-darwin` and `x86_64-linux` but a *native* wrapper on
`aarch64-linux`, where `targetPrefix` is empty and `bin/` contains no
`aarch64-unknown-linux-gnu-`-prefixed entry. The prefixed lookup therefore
skipped that wrapper and found the **unwrapped** GCC its own `setup-hook` had
put on `PATH` — a different compiler driver *and* a different assembler,
selected by `PATH` order rather than by anything pinned.

**This corrects B20's recorded root cause.** B20 attributed the divergence to
Darwin's wrapper injecting `-fno-omit-frame-pointer` where `aarch64-linux`
"forces neither". Both wrappers ship a byte-identical
`nix-support/cc-cflags-before`; nixpkgs emits it for every non-x86-32,
non-s390 target. B20's two pre-fix hashes, `e8cbab4f…` and `f2d316e1…`, differ
by *driver*, not by host: both are reproducible on one machine by choosing the
wrapped or unwrapped compiler.

**Resolved by** exporting `CROSS_COMPILER_PREFIX` as an absolute
`"${crossCC}/bin/${crossCC.targetPrefix}"` store path, so every host runs the
same driver and assembler. This is the fix B20 proposed and rejected as
"larger, with a worse failure mode"; that rejection rested on a false premise.
`crossCC` is the same derivation each platform already evaluates and installs,
so nothing new is fetched and no pinned hash moves. `just sel4_pin_check` now
fails if the bare form returns — the prefix pin cannot catch this itself, since
it reports "toolchain drift" without naming which host is odd.

B20's `-fomit-frame-pointer -momit-leaf-frame-pointer` are **kept**. Fault
injection shows they close a *different* leak than the one B20 recorded: with
the toolchain pinned but the flags removed, the hosts still diverge in
`.debug_line` alone (`e8cbab4f…` vs `4c694979…`, both 982208 bytes, every ALLOC
section equal), because GAS's DWARF-5 view numbering for the extra prologue row
is not host-independent. That binutils behavior is masked, not fixed.

**Exit condition (observed 2026-08-06):** `kernel.elf` rebuilt from scratch on
`aarch64-darwin` and `aarch64-linux` is `97dcb029…`, 973184 bytes on both —
**unchanged** from the recorded pin, now depending on the toolchain rather than
on `PATH`. `CROSS_COMPILER_PREFIX` resolves to the wrapper on `aarch64-linux`
instead of being empty. `just sel4_qemu_image_check` passes on `aarch64-darwin`,
and the new guard is fault-injected: reverting to `crossCC.targetPrefix` fails
`just sel4_pin_check`. `x86_64-linux` was not re-observed; its prefix was
already the cross form, so the change is expected to be a no-op there
(**[INFERENCE]**). Both hosts are on one machine, one virtualized — the right
test for toolchain and `PATH` independence and no evidence about physical
boards. See `devlog/2026-08-06-b21-cross-toolchain-binary-selection/`.

### B16 — a supervision termination record was never reclaimed, so a long-lived graph exhausted the table — **resolved 2026-08-07**

**Problem:** `slime-root/src/supervision.rs::Terminations` records how each child
ended and never removes the record, because two parents may hold handles to one
child and each is owed the answer. `MAX_RECORDS` is `MAX_TASKS` (32), which
bounds the tasks *alive at once* — but `TaskTable::reclaim` frees its entries
while `TaskId`'s `next_id` keeps counting, so a graph that spawns and reaps
repeatedly creates far more than 32 tasks while never holding more than a few.

Past the bound, `record` drops silently and every later
`supervision_status` on that child answers `WouldBlock` forever: the
parent-waits-forever failure the module exists to prevent, arriving by the
module's own bookkeeping rather than by a missed wake. The retired kernel's
`sched.terminated` is an unbounded `Vec` and has no equivalent limit.

Not reachable by any declared seL4 generation — each creates a handful of tasks
and exits — so it is a latent bound rather than an observed defect.

**Evidence:** `supervision.rs::MAX_RECORDS` against `task.rs::TaskTable::reclaim`,
which decrements `len` but not `next_id`. Noted in the P5.3.3 review; see
`devlog/2026-08-05-p5-3-3-spawn-plane/`.

**Proposed fix:** reclaim a record once every holder of a handle naming that
child has collected or dropped it, which needs a reference count incremented at
each `Supervision` capability install and decremented at each collect, drop, and
table release. Alternatively fail the *spawn* when the record table is full,
which turns a silent wrong answer into a bounded refusal at the point of
allocation — the same shape `construct_child` already uses for `MAX_GRAPH_TASKS`.

**Deferral re-reviewed 2026-08-05, before opening P5.5.2's gate.** Still
deferred, on the same observation, and this is the largest graph the cutover
declares: P5.5.2's stream plane creates thirteen tasks — seven launched, six
spawned — against `MAX_RECORDS = 32`. The bound is approached more closely than
by any earlier slice and still not reached. See
`devlog/2026-08-05-p5-5-2-stream-plane/`.

Worth stating plainly, since the margin is now under 3×: this stays a latent
bound rather than a defect only because every declared generation runs to
completion and exits. A long-lived graph that spawns and reaps repeatedly is
what makes it bite, and P5.4 — which retires the oracle — is the point at which
"every declared generation" stops being a safe quantifier.

**Why deferred rather than fixed in P5.3.3:** the counting version touches every
path that installs or releases a capability, and the refusal version needs a
gate whose graph spawns past the record table to prove it. Neither is a line;
both want the multi-child graph P5.3.4 composes.

**Exit condition (observed 2026-08-07):** a graph that creates more than
`MAX_RECORDS` tasks over its lifetime still answers `supervision_status`
correctly for every live handle, observed under `just sel4_supervision_check`,
with the nine existing seL4 gates passing. (The entry said *five*; there were
nine by the time it was closed.)

**Fix: a derived sweep, which is neither option this entry proposed.** The
refusal was rejected on the entry's own terms — refusing the spawn makes the
graph the exit condition requires impossible to observe, so choosing it would
mean amending the condition in the same change that claimed to meet it. The
reference count was unnecessary: the live-holder set is already represented, so
`supervision::sweep` derives it, reclaiming every record no live holder can
name. Same choice, same reason, as `TaskTable::live_children`, and it fails
safe — a sweep that does not run leaves a record that still answers correctly,
whereas a missed decrement loses one forever.

The predicate reads **two** holders. A supervision handle in flight is held by
no capability table at all, so a sweep consulting only `GraphTables` would free
a record mid-transfer and leave the receiver waiting forever: this defect,
reintroduced by its own fix. `Transit::holds_supervision` is the second half,
and fault injection #2 below is what proves it is load-bearing.

The residual case is now reported rather than silent: if every record has a live
holder, `record_termination` emits
`SLIME_GRAPH FAIL termination lost task={} reason=records-full`, matching
`unland_caps`'s convention. That is what closes the *silent*-loss defect rather
than merely raising the bound.

**Observed:** 35 tasks created over one boot, `terminated=38` against
`MAX_RECORDS = 32`, with `freed=30 live=3` at the sweep — the retained handle,
the in-flight handle, and the current record all preserved. Two fault
injections, both confirmed failing: removing the sweep fails at
`termination lost task=33 reason=records-full`; removing only the `Transit` half
of the predicate fails at `a handle parked across the crossing lost its
outcome`, with every earlier marker still passing. See
`devlog/2026-08-07-b16-supervision-records/`.

### B20 — the prefix pin held for one platform at a time — **resolved 2026-08-06**

**Problem:** B19 made `kernel_sha256` independent of the dev *shell*; it was
still per-*platform*. `aarch64-darwin` produced `e8cbab4f…` and `aarch64-linux`
produced `f2d316e1…` from the same checkout, the same `flake.nix`, and the same
pinned seL4 source and config.

The cause was the toolchain, not a leak. `flake.nix` names
`pkgsCross.aarch64-multiplatform.stdenv.cc`, which resolves to a **cross**
`gcc-wrapper` on Darwin and a **native** `gcc` on `aarch64-linux` — the
empty-`targetPrefix` fact B19's analysis recorded, seen from the other side.
Darwin's `nix-support/cc-cflags-before` forces
`-fno-omit-frame-pointer -mno-omit-leaf-frame-pointer`, so every function
prologue differed. Because that file lives inside the wrapper derivation rather
than the environment, B19's scrub could not reach it.

**Resolved by** having the build state its own frame-pointer policy:
`-fomit-frame-pointer -momit-leaf-frame-pointer` joins the prefix maps and the
fixed seed in `CMAKE_C_FLAGS`/`CMAKE_ASM_FLAGS`. This is a policy the build
**chooses**, not a compiler default it restores, and it moves *both* platforms:
GCC's aarch64 backend disables `-fomit-frame-pointer` at every `-O` level, so an
aarch64 kernel keeps its frame pointers at `-O2` unless the flag is explicit.
(`-Q --help=optimizers` claims otherwise at `-O2`; that is a reporting trap, and
it is what an earlier draft of this entry got wrong.) The choice is sound because
seL4 states no frame-pointer preference and nothing walks one — the AArch64 trap
path's `x29` uses are full register-context saves indexed off `sp`, and
`Arch_userStackTrace` scans `SP_EL0` linearly. `-momit-leaf-frame-pointer` is
belt and braces: under `-fomit-frame-pointer` it changes no emitted code, and it
is kept only because it names the second of the wrapper's two injections.

Darwin's two other injections need no counter-flag: `-march=armv8-a` is what seL4
passes itself and what both compilers default to, and the glibc/gcc
`-idirafter`/`-B` paths reach nothing in a `-nostdinc -ffreestanding -nostdlib`
build.

Naming one cross toolchain for every system — B20's own proposed fix — was
rejected as larger, with a worse failure mode, and moving the pin for a reason
unrelated to the defect. It remains the stronger fix and is now optional.

`kernel_sha256` is re-observed as `97dcb029…` on **all three platforms tested**.

**Exit condition (observed 2026-08-06):** `kernel.elf` built on
`aarch64-darwin`, `aarch64-linux`, and `x86_64-linux` are **byte-identical** by
`cmp`, each 973184 bytes at `97dcb029…`, from three different dev-shell seeds
(`r279wlb3cq`, `65gzz0x3v8`, `6ckb6q72lb`), with all nine `sel4_*` Justfile gates
passing. `x86_64-linux` is the case that matters most:
there `pkgsCross.aarch64-multiplatform.stdenv.cc` is a genuine *cross* wrapper as
on Darwin, rather than the native `gcc` `aarch64-linux` resolves, so both wrapper
shapes agree. B19's property still holds on each: a real-shell build and a
hostile-environment build are byte-identical. Fault-injected symmetrically —
replacing the flag string with `""` reverts Darwin to `e8cbab4f…` and
`aarch64-linux` to `f2d316e1…`, the exact pre-B20 divergence. Both Linux hosts
are containers under a macOS hypervisor, one of them emulated, not separate
hardware — the right test for toolchain independence and no evidence about
physical boards. See `devlog/2026-08-06-b20-cross-platform-kernel-identity/`.

**Root cause superseded by B21 (2026-08-06).** The mechanism recorded above is
wrong. Both wrappers ship a byte-identical `cc-cflags-before`; the divergence
was `PATH`-order *binary* selection, not a per-platform wrapper policy, and the
two pre-fix hashes differ by driver rather than by host. The "stronger fix …
now optional" is implemented and moved no hash. The frame-pointer flags are
kept, for a residual `.debug_line` leak this entry did not identify. See the
B21 entry above and
`devlog/2026-08-06-b20-cross-platform-kernel-identity/index.md`'s
`## Corrections`.

### B19 — the seL4 prefix pins bound the dev-shell derivation hash, not the toolchain — **resolved 2026-08-06**

**Problem:** `sel4/pins.toml`'s `[observed_prefix]` is the gate that would
notice a change of seL4 compiler, and it pinned the **dev shell's own derivation
hash** instead. `configure_and_install_sel4` inherited `os.environ`, and nixpkgs
puts `-frandom-seed=<first 10 chars of the devShell derivation hash>` into
`NIX_CFLAGS_COMPILE`; GCC seeds symbol and section naming from it, so adding a
tool to `flake.nix` — or reordering the list — changed `kernel.elf` byte-for-byte
and was reported as toolchain drift. The same variable carried per-package
`-isystem` store paths, and `NIX_HARDENING_ENABLE` imposed
`-fstack-protector-strong`, `-fzero-call-used-regs`, and `_FORTIFY_SOURCE=3` on a
freestanding kernel whose own `CMakeLists.txt` asks for `-fno-stack-protector`.

**Resolved by** making the kernel build independent of the shell rather than by
re-pinning per host. `sel4_build_environment` builds the environment from
`os.environ` minus every flag-carrying `NIX_*` variable, the `CFLAGS`-family
names CMake seeds `CMAKE_<LANG>_FLAGS_INIT` from, the bintools wrapper's
`NIX_SET_BUILD_ID`/`NIX_BUILD_ID_STYLE` switches, and
`CMAKE_INCLUDE_PATH`/`CMAKE_LIBRARY_PATH`/`CMAKE_PREFIX_PATH`; a fixed
`-frandom-seed=slime-sel4-qemu-arm-virt` replaces the shell's seed. The scrub
matches by *prefix* because the cc-wrapper reads target- and role-mangled
spellings (`NIX_CFLAGS_COMPILE_aarch64_unknown_linux_gnu`, `_FOR_BUILD`,
`_FOR_TARGET`) rather than the base names.

Of the exact-name groups, only the search paths were a live route:
`CMAKE_INCLUDE_PATH` is prepended to `find_file` order, which no `-D` protects,
and seL4 resolves `KERNEL_HELPERS_PATH` that way. The rest are defense in depth
and are labelled so in the code rather than described as leaks.

`kernel_sha256` was re-observed as `e8cbab4f…` on `aarch64-darwin` — **since
superseded by B20's `97dcb029…`**, which is the same kernel built with the
frame-pointer policy stated rather than inherited. The other four
pinned artifacts were already reproducible and are unchanged. The hash still
binds `cmake`, `ninja`, and the host Python generators, which this file does not
pin — recorded as a residual in the devlog, not claimed as closed.

**Exit condition (observed 2026-08-06):** `just sel4_qemu_image_check` passes,
and adding `hexdump` to `flake.nix`'s `packages` moves the shell's seed from
`r279wlb3cq` to `rhl1f441df` while leaving `kernel_sha256` byte-identical. A
third build with a fabricated seed, fake `-isystem` store paths, a narrowed
hardening set, and an ambient `CFLAGS` is byte-identical too. Fault-injected:
one nibble changed in `kernel_sha256` makes the gate exit 1.

**A second host was then observed, on `aarch64-linux` under OrbStack** (shell
seed `65gzz0x3v8` against Darwin's `r279wlb3cq`). B19's property holds there —
a real-shell build and a hostile-environment build are byte-identical — but at
`f2d316e1…` rather than `e8cbab4f…`, because Darwin resolves a *cross*
`gcc-wrapper` that forces `-fno-omit-frame-pointer` while `aarch64-linux`
resolves a *native* `gcc` that does not. That is a genuine toolchain difference,
which is what the gate exists to catch, so the pin stands as recorded. It does
mean `[observed_prefix]` is **per-platform**; that was opened as B20 rather than
folded in here, and B20 is now resolved — both platforms produce a
byte-identical `97dcb029…`. See
`devlog/2026-08-06-b19-sel4-prefix-pin-shell-coupling/`.

### B18 — the seL4 stream gate was scheduling-dependent — **resolved 2026-08-06**

**Problem:** `just sel4_stream_check` passed roughly one run in three. Two
independent causes, both invisible on x86 because the retired kernel's
cooperative scheduler orders the events favourably every time.

**Cause 1 — a publisher writing to a route it had already retired.**
`fabric-publisher-b` sent its first `diagnostics` sample with `FLAG_LAST` and
then published on that route again after the large telemetry sample. That second
send was **dead code**: `FLAG_LAST` sets `publisher.finished`, and both the
broker loop and `park_on_streams` skip a finished publisher, so nothing ever
read it. Worse than inert — once `diagnostics` retired, only `telemetry` kept
the fabric alive, so after that drained the send answered `ERR_PEER_DEAD`, which
`publish` treats as fatal. Deleted.

**Cause 2 — `debug_write` was one syscall per byte.** Under `PRINTING` the
component-side implementation called `seL4_DebugPutChar` per character,
bypassing the root entirely. The root's own `debug_println!`, or another
component's line, could land mid-string: the transcript showed ` QoS matched`
where `[fabric] QoS matched` was written, and whichever gate required the
destroyed marker failed on a boot that was otherwise correct.

This was the larger cause, and it masqueraded as several different bugs — a
missing `re-delegation denied`, a missing `large sample published`, and (because
a corrupted `QoS matched` changes what the transcript appears to say about
matching) an apparent provisioning race. Diagnosing it as one defect rather than
three took reading full transcripts rather than the gate's 40-line tail.

`Operation::DebugWrite` is now served by the root's graph loop, which is
single-threaded and answers one request at a time, so a line printed inside that
arm cannot interleave with anything. Atomicity is structural rather than a
matter of timing. The cost is that printing now needs a bound transfer window;
every launched component binds one before it runs.

**Two fixes were tried and reverted**, both recorded because each looked
plausible and each made things worse:

- **Moving `FLAG_LAST` to the second diagnostics sample**, where the route
  genuinely ends. Wedges `just fabric_qos_check`, whose subscriber waits for the
  terminal event the early flag produces.
- **Making the stall stop acking.** `receive_large_sample` acks the inline
  samples it passes over, which does drain the ring the stall is supposed to
  overrun — but removing the ack wedges the fabric outright, because it waits
  for a delivery slot that never frees. The ack is load-bearing.
- Narrowing `fabric-subscriber-b`'s declared `historyDepth` from 4 to 2 also
  failed, and for the same underlying reason as everything else: the failures
  were marker corruption, not ring arithmetic.

**Exit condition (observed):** ten consecutive `just sel4_stream_check` runs
pass, with all six other seL4 gates, `just fabric_stream_check`,
`just fabric_qos_check`, `just fabric_visibility_check`, and
`just data_fabric_boot_check` unchanged. See
`devlog/2026-08-05-p5-5-2-stream-plane/`.

### B17 — the capability transfer's subset test had no coverage — **resolved 2026-08-05**

**Problem:** `slime-root/src/main.rs::serve_cap_transfer` enforces four rules,
and P5.5.1's gate observed three. The fourth — the **subset test**,
`rights & !source.rights != 0`, which is what makes the move narrow-only
against *what the holder actually has* — was not observed: deleting it left
every marker in that gate intact.

**The entry's stated reason was wrong, and that is the interesting part.** It
argued the property was unreachable from any graph this cutover could declare,
because reaching it needs a capability holding transfer authority while being
strictly narrower than its kind admits, and `cap_transfer` with
`FLAG_RETAIN_TRANSFER` was "the only thing that produces one" — which a
component cannot use on itself, since the two ends of a channel it holds alone
are a loopback the root refuses to split.

A plain **spawn grant** produces one. `preflight_spawn_grants` installs the
requested mask verbatim, so `grant(endpoint, RIGHT_SEND | RIGHT_TRANSFER)`
yields exactly send+transfer where `Endpoint` admits send+recv+transfer.
Init already does this on x86 for `DANGO_OUTPUT_SLOT` — the shape existed in the
tree the whole time; nobody had asked to widen one. The gap was a missing arm,
not an unreachable property, and the analysis that said otherwise was checking
`cap_transfer`'s own outputs rather than every path that installs a mask.

**Resolution:** `sel4-stream.zti` grants `fabric-publisher` a second endpoint
end at send+transfer, carrying no traffic and belonging to no route. It goes to
the publisher because that component already carries the other two
transfer-rule denials, so all three sit together and each states which rule it
proves. The component asks to move it with `recv` restored: that passes the transfer-authority rule,
passes the descriptor/kind rule, and computes zero against the per-kind mask, so
only the subset test can refuse it.

The arm is guarded on **holding** the subject rather than on a check flag,
because an empty slot answers the same `ERR_BAD_CAP` the subset test does — a
bare widening arm would pass identically in a graph that never granted the
endpoint, which is the "looks like coverage and is not" failure this item was
opened for. It establishes possession by *using* the granted end first, so a
graph without one skips silently and claims nothing.

**Exit condition (observed):** `just sel4_stream_check` observes the refusal,
and removing `rights & !source.rights` from `serve_cap_transfer` fails that gate
— the fault injection P5.5.1 ran and could not make fail. See
`devlog/2026-08-05-p5-5-2-stream-plane/`.

### B15 — a spawn carries at most four grants on seL4, against the oracle's sixty-four — **resolved 2026-08-05**

**Was:** `slime-root`'s spawn read its grant array through
`transfer_window::read_staged`, whose bound is `ipc::MAX_MESSAGE_BYTES` (64). At
`SPAWN_GRANT_RECORD_BYTES` = 16 that is **four** records, against the retired
kernel's sixty-four. Real x86 callers already exceeded it —
`init.rs::GENERATION_MANAGER_CAPS` and `dango_caps()` are six each, and
`launch_fabric_graph` hands the fabric nine — so a component that runs on the
retired kernel would have failed to launch its children on the cutover, which is
the one property P5.4 must be able to claim.

**Fixed by** a second staged bound rather than a wider message.
`transfer_window::MAX_STAGED_ARRAY_BYTES` (1024) bounds an *array* staged
through a window, where `MAX_STAGED_BYTES` bounds a *message*; the two stay
separate numbers because a `send` payload becomes an `ipc::Message` and is that
wide by construction, while a grant array becomes no message at all. The
component side needed no change: `sel4_transport::spawn` already encoded into a
`MAX_SPAWN_GRANTS * GRANT_RECORD_BYTES` buffer and staged it into a 4096-byte
window, so the refusal was entirely root-side.

**Exit condition observed 2026-08-05** under `just sel4_spawn_check`: `init`
spawns `sysinfo` with **six** grants — B15's own number, and the size of this
repository's largest real grant lists — and all six ends move, each granted slot
leaving init's table while each retained half still sends. Fault-injected: with
the narrow reader restored the spawn is refused outright and the gate fails. See
`devlog/2026-08-05-p5-5-1-typed-fabric/`.

### B14 — `slime-root` ignores the generation's declared spawn budget

**Problem:** the generation declares `spawnBudget` per component, and
`slime-root/src/main.rs::serve_spawn` never reads it. A component with a
declared budget of 1 can spawn until `MAX_TASKS` fills. The retired kernel
checks it first thing in `spawn_from_cap`
(`kernel/src/task/mod.rs`: `if task.live_children >= task.spawn_budget`), and
refuses with `ERR_OUT_OF_MEMORY`.

This is the same shape B13 had, and it is why it is recorded rather than left
in a devlog note: the generation declares a bound and the root does not enforce
it, so the only thing limiting a component is a global table size no generation
named. Authority to spawn comes from the executable grant, which *is* checked;
what goes unchecked is how many times it may be used.

The blast radius is currently small — no seL4 fixture spawns near its declared
budget, and `boot_contracts` already clamps the decoded value to
`MAX_SPAWN_BUDGET` — so it is a latent hole rather than an observed defect.

**Evidence:** `Component::spawn_budget` is decoded in
`boot-contracts/src/generation.rs` and read nowhere in `slime-root/`;
`contracts/generation/v1/fixtures/sel4-spawn.zti` declares `spawnBudget = 4`
for `init`, which spawns twice, so no boot currently reaches the bound. Noted
while implementing spawn in P5.3.3; see
`devlog/2026-08-05-p5-3-3-spawn-plane/`.

**Proposed fix:** count live children per task in `TaskTable`, decremented when
a child is reclaimed, and refuse a spawn past the declared budget with
`ERR_OUT_OF_MEMORY` — matching the retired kernel's code, since
`init.rs::spawn_optional_storage` already distinguishes that from `ERR_BAD_CAP`.
The count must be decremented on both death paths, not only on clean exit.

**Why deferred rather than fixed in P5.3.3:** the exit condition that slice
carries is about *which* executables resolve and how a child's fate is
observed, not how many children may exist. Adding a counter would be
straightforward, but the arm that proves it needs a fixture whose component
spawns past its declared budget, which is a scenario rather than a line —
P5.3.4 composes the sample plane and is where a multi-child graph already
exists.

**Exit condition:** a component whose generation declares `spawnBudget = N` is
refused `ERR_OUT_OF_MEMORY` on its `N+1`th live child and succeeds again once
one is reclaimed, observed under a named seL4 gate, with the five existing seL4
gates still passing.

**Resolved 2026-08-05** by P5.3.4; see
[`devlog/2026-08-05-p5-3-4-sample-plane/`](../devlog/2026-08-05-p5-3-4-sample-plane/index.md).

`slime-root/src/main.rs::serve_spawn` now reads the caller's declared
`spawnBudget` and refuses a spawn past it, before anything is allocated. The
count is *derived* rather than tracked: `Task` records the id of the task that
spawned it, and `TaskTable::live_children` counts the table. A counter would
need decrementing on the clean-exit path, the fault path, and every spawn
unwind, and a missed decrement would silently tighten a bound the generation
declared — whereas a reclaimed task frees its parent's budget by ceasing to
exist.

The refusal is `ERR_OUT_OF_MEMORY`, matching `sys_spawn`, which maps
`BudgetExhausted` and `TooManyTasks` alike to that code and everything else to
`ERR_BAD_CAP`. That distinction is the caller's business in a way the preflight
refusals are not: a component at its ceiling learns something true about itself
and can wait for a child to exit.

The deferral reason was "P5.3.4 composes the sample plane and is where a
multi-child graph already exists," and that is this slice.

**Observed exit condition, both clauses.**
`contracts/generation/v1/fixtures/sel4-sample.zti` declares `init` a budget of
exactly two — the two children the composition needs — so the third spawn is a
denial arm rather than an unused allowance. `just sel4_sample_check` asserts
`SLIME_GRAPH spawn refused task=N child=... class=budget live=2 budget=2` and
`[init] spawn budget refused`, which `drive_sample_plane` prints only after
requiring exactly `ERR_OUT_OF_MEMORY`.

The second clause — "succeeds again once one is reclaimed" — is asserted too,
and getting it required a real fix. `TaskTable::reclaim` was reachable from the
P5.1 fixture path and from `release_child`, but from neither death arm in
`serve_component_graph`, so a dead child kept its table entry and the derived
count made the budget a *lifetime* cap. Both arms now reclaim, and init spawns
once more after both children exit; a lifetime cap would refuse there too, so
that arm is what distinguishes the two readings. All six seL4 gates pass.

**Fault injection.** With the budget check disabled the gate fails on
`spawn budget did not bite`; with task reclamation removed from the death paths
it fails on `budget did not recover after a child exited`. Both arms are covered
rather than merely present.

### B13 — `slime-root` admits a shared-buffer allocation without resolving a factory capability

**Problem:** `slime-root/src/main.rs::serve_buffer_create` ignores the factory
slot its caller names and admits the allocation against the holder's declared
quota alone. The retired kernel resolves a `RIGHT_BUFFER_CREATE` capability
first (`kernel/src/syscall/mod.rs::sys_shared_buffer_create`), so a component
the generation grants no factory allocates nothing there whatever its budget
says. On seL4 the budget is the only bound: a component with a non-zero ceiling
and no factory grant still allocates.

That inverts the intended relationship between the two. The grant authorizes
the operation and the budget bounds it; they are independent by design, and
`components/bins/src/shared_buffer_probe.rs` documents exactly that. With the
grant unchecked, authority to allocate follows from a budget entry — which is
ambient authority arriving through the back door, against the invariant that
`slime-root`'s whole capability model exists to hold.

The blast radius is currently small: every seL4 generation that declares a
budget holder also intends it to allocate, so no live graph is mis-admitted.
It is a latent hole rather than an observed defect.

The same discarded word carries the caller's `writable` flag
(`slot_with_flag(factory_slot, writable)` in
`components/runtime/src/syscall/wire.rs`), so every region is created writable
whatever the caller asked for. That is permissive in the same direction and
belongs to the same fix.

**Evidence:** `slime-root/src/main.rs::serve_buffer_create` takes no slot
argument and the `SharedBufferCreate` arm reads only `words[1]`, against
`kernel/src/syscall/mod.rs::sys_shared_buffer_create`'s capability resolution.
`graph::Resource::SharedBufferFactory` is defined and never installed or
resolved anywhere in the crate. Noted while adding the loan plane in P5.3.2 and
confirmed by that slice's review; see `devlog/2026-08-04-p5-3-2-loan-plane/`.

**Proposed fix:** materialize the boot layout's `shared-buffer-factory` role and
the generation's `bufferCreate` grants into the holding components' capability
tables, the way `channel::materialize` already does for send/recv grants, and
resolve the slot in `serve_buffer_create` before admitting anything — reading
the `writable` flag from the same word while it is being decoded.

P5.3.2 made this sharper rather than causing it: replacing the uniform
`SHARED_QUOTA` with the generation's declared ceilings means the budget now
carries the weight the factory grant used to. Authority to allocate currently
follows from a budget entry alone, which is why the entry moved to the top of
the open list.

**Why deferred rather than fixed in P5.3.2:** installing non-channel grants
changes what occupies each component's capability table, and therefore the slot
numbers `channel::materialize`'s cursor hands out for channel ends. Those
numbers are asserted marker-for-marker by `just sel4_component_graph_check` and
`just sel4_channel_check`. Renumbering them is the same distribution problem
P5.3.3 solves for spawn grants, and doing it twice — once here and once there —
would rewrite two gates' evidence for one change.

**Exit condition:** a component holding a budget entry but no `bufferCreate`
grant is refused `ERR_BAD_CAP` by `shared_buffer_create`, observed under a named
seL4 gate, with `just sel4_component_graph_check`, `just sel4_channel_check`, and
`just sel4_loan_check` still passing.

**Resolved 2026-08-05** by P5.3.3; see
[`devlog/2026-08-05-p5-3-3-spawn-plane/`](../devlog/2026-08-05-p5-3-3-spawn-plane/index.md).

`slime-root/src/main.rs`'s `SharedBufferCreate` arm now resolves the factory
slot the caller names, requiring `RIGHT_BUFFER_CREATE`, before admitting
anything — and reads the `writable` flag out of the same word while it is being
decoded, so a region created read-only no longer carries write rights. The
generation's `bufferCreate` grants are materialized into the holding
components' capability tables beside the channel ends: at the boot layout's
role slot for the bootstrap component, and above the executables for every
other, which is the same split `channel::materialize` already made.

The deferral reason was verbatim "the same distribution problem P5.3.3 solves",
and that is this slice, so it was closed here rather than deferred again.

**Observed exit condition.** `just sel4_loan_check` asserts
`SLIME_GRAPH buffer create refused task=N class=ungranted` before any ceiling is
grazed, so the refusal is a capability answer rather than a quota answer wearing
another name. Two arms in one marker pair: an empty slot and a slot holding real
authority of another kind are refused identically, which is what stops a
component probing its table by watching which error comes back.
`just sel4_component_graph_check`, `just sel4_channel_check`,
`just sel4_loan_check`, and `just sel4_spawn_check` all pass.

**Fault injection is what made this real.** Removing the factory check left
*every* gate passing: no fixture had a component that held a budget and tried to
allocate without a grant, so the fix was uncovered by construction. The loan
fixture's `init` now names one deliberately. Recorded because a gate that passes
against an injected build is evidence of nothing, and this one nearly shipped
that way.

### B11 — test scaffolding is declared in the product boot generation

**Resolved:** 2026-08-01. See
`devlog/2026-08-01-b11-product-boot-profiles/`.

**Problem:** The source manifest had one global component graph and health
policy. It declared the sixteen probes and scenario doubles originally named by
B11, plus the test-only `storage-writer`, as peers of product services with
real capability grants. Selecting a fabric profile changed interposition only;
it could not remove a component, its executable object, authority, budget, or
health edge from the authenticated generation.

**Fix:** Added a versioned Zutai `BootProfile` to the existing profile mechanism.
The builder resolves one profile to a closed component/object/grant/state/budget/
health/fabric graph before encoding. `default` is the scaffolding-free product
profile; `test`, `visibility`, and `unified` explicitly declare the verification
participants their gates use. The boot-layout emitter and kernel placer accept
profile-absent scaffolding while retaining exact rights and filled-slot checks,
and init consumes the same generated labels for every scenario executable and
authority role.

**Exit condition (observed):** `just product_boot_check` boots a healthy 45-slot
product generation that names none of the seventeen test-only components. `just
boot_layout_check` passes all nineteen profile/layout pairs while preserving all
eighteen pre-B11 fixtures. Every probe-dependent gate explicitly selects its
profile and passes, including all five storage gates, directory, powerbox,
sample-plane, fabric authority/stream/QoS/call/operation/visibility/full-graph,
generation commands, rollback, bootstate trace, and transfer. `just test` passes
189 assertions; contracts, generation determinism, formatting, lint, Python
lint, spelling, devlog, and Framework safety checks are clean.

### B10 — init's capability layout is a positional convention, so boot paths are selected at kernel compile time

**Resolved:** 2026-08-01. See `devlog/2026-07-31-boot-layout-baseline/` for the
equivalence baseline and `devlog/2026-08-01-boot-layout-resolution/` for the
change.

**Problem:** `launch_init` builds init's capability vector by writing fixed
indices (`caps[46] = ...`) rather than resolving named grants the generation
declares. `MAX_CAPS = 64`, and the vector was 61 occupied before C8.10, so a new
participant set cannot be appended — it must squat on another profile's slots or
fork a whole `launch_*_init`. Both happened. The gates that read those slots read
them positionally, which is why the layout cannot simply be renumbered.

The escape hatch chosen instead was compile-time selection: `option_env!` reads a
`SLIME_*_CHECK` flag and compares `generation.number` against a literal. Because
`option_env!` is evaluated at compile time and Cargo tracks these as build inputs
(the kernel's dep-info records `env-dep:SLIME_DANGO_CHECK`,
`env-dep:SLIME_GENERATION_CMD_CHECK`, `env-dep:SLIME_POWERBOX_CHECK` and
siblings), each gate builds a *different kernel binary*. There is no single
kernel artifact that passes the gate suite.

This blocks P1. That milestone requires that "architecture-neutral code can be
type-checked for AArch64 without importing x86-only modules", which cannot hold
while the boot path is selected by x86-gate build flags and hardcoded generation
numbers.

**Evidence:** `kernel/src/runtime/bootstrap.rs:176-182` states the constraint
outright — the vector is "61 of `MAX_CAPS = 64` before this milestone adds
anything", the three new C8.10 roles "need nine slots against three free", and
the vector "is also the layout six passing QEMU gates read positionally — the
`caps[46] = ...` blocks below rewrite it per generation number — so renumbering
it to fit would rewrite C8.3-C8.8's evidence rather than extend it".

Counted at the commit that opened this item:

- 26 positional writes over 13 distinct slots (46-59) in `bootstrap.rs`;
- 3 `launch_*_init` forks: `launch_init` (168), `launch_fabric_boot_init` (964),
  `launch_recovery_init` (1087);
- 9 `generation.number ==` branches in `launch_init`, including
  `generation.number == 14` reassigning slots 46/47/49 under the comment that
  "the call gate reuses the executable/control slots occupied by three stream
  participants in every other generation profile", and the mutually exclusive
  call/operation profiles at lines 793 and 828 sharing one slot range;
- 21 distinct `option_env!("SLIME_*")` flags over 70 sites (18 in `kernel/src`,
  52 in `components/`);
- 11 distinct generation numbers driven by check scripts (6, 7, 8, 9, 10, 11,
  12, 13, 14, 16, 99), e.g. `check-fabric-stream.py` sets
  `SLIME_FABRIC_STREAM_CHECK=1` with number 12, `check-fabric-qos.py` sets
  `SLIME_FABRIC_QOS_CHECK=1` with 13, and `check-data-fabric-boot.py` sets
  `SLIME_FABRIC_BOOT_CHECK=1` against the kernel's `generation.number == 17`.

**Fix as proposed when the item opened:** Resolve init's grants by name from
the generation instead of by index in kernel source, so a profile's participant
set is generation data. The hard constraint is that every profile in use today
must resolve to **the same slot numbers it occupies now** — a naming layer over
the existing
layout, not a renumbering, because renumbering rewrites six gates' evidence
rather than extending it. With grants named, the `option_env!` and
`generation.number` branches in `launch_init` lose their purpose and the
`launch_*_init` forks collapse.

Storage identity selection at `bootstrap.rs:571` and `bootstrap.rs:595`
(generation numbers 2, 3, 4 selecting different capabilities and a different
storage component) is the same pattern on a different axis. Decide explicitly
whether it is in scope before starting; do not leave it undecided.

Component-side flags are not assumed to fall out of this: 52 `option_env!` sites
in `components/` (9 reading `SLIME_FABRIC_VISIBILITY_CHECK` alone) make their own
build-time decisions independent of the kernel layout, and may need their own
pass.

**Fix:** A `contracts/boot-layout/v1` resource declares which capability slot
holds which role, under which name, with which rights, per generation number.
`launch_init` offers each capability it mints to a placer under the name the
layout knows it by, and the layout decides where it lands; a capability the
layout does not name, or a declared slot nothing fills, stops the boot. The
storage `generation.number` matches disappear by construction rather than by a
separate fix, because the layout names the component and declares the rights.
Profile branches ask what the layout declares instead of comparing against a
literal, and the C8.10 fork keys on the layout declaring the fabric's own route
workers — putting it in the same category as the `component_named("recovery")`
fork beside it. The script-install and idle-exit gates were each `flag &&
number == N` with a unique number per gate, so the flag was redundant in all
ten. `init.rs` reads the same table, rendered as Rust at component build time,
dropping 84 lines of constants that previously agreed with the kernel only by
inspection.

An entry declares a *role*, not a concrete object: the storage slot resolves to
a block device when the platform enumerates one and an object store when it
does not, which is decided by PCI enumeration at boot and is not knowable to
the host builder.

**Exit condition (observed):** `just boot_layout_check` — a new gate, since
P0/P1's `architecture_contract_check` and `x86_portability_check` do not exist
— boots all eighteen distinct profiles and finds every slot, label, and rights
value identical to the pre-change fixtures. `launch_init` contains no
`option_env!` and no `generation.number` branch. One kernel binary now serves
every gate: built with no flags and with `SLIME_FABRIC_BOOT_CHECK`,
`SLIME_DANGO_CHECK`, `SLIME_FABRIC_CALL_CHECK`, `SLIME_POWERBOX_CHECK` and
`SLIME_GENERATION_CMD_CHECK` all set, it hashes identically, where the same
comparison previously gave three distinct binaries. The named gates observe
their existing results: `dango_check`, `sample_plane_live_check`,
`fabric_stream_check`, `fabric_call_check`, `fabric_operation_check`,
`fabric_visibility_check`, `data_fabric_boot_check`, plus `fabric_qos_check`,
`fabric_authority_check`, `generation_cmd_check`, `powerbox_check`,
`directory_check`, `transfer_check`, `rollback_check`, `bootstate_trace_check`,
`test`, `contracts_check`, `generation_check`.

**Fault injection:** three defects surfaced during the change, each caught by a
fixture rather than by reading code. Generation 4 declares two identical
object-store entries, so resolving a role by first-match filled one slot twice;
generation 14 leaves `fabric-subscriber-b` in slot 50 because the call profile
rewrote 46-49 and stopped; generation 15 takes slot 50 but leaves the same
component's control channel at 55 and 60. The last two are the argument for the
change — which slots a profile overwrote was implied by the index range a
rewrite block happened to cover, stated nowhere and checked by nothing. The
emitter's own guards were fault-injected too: a duplicate slot, a named role
without a label, an unnamed role carrying one, and a stale component fallback
table are each rejected.

**Follow-up:** `launch_fabric_boot_init` still builds its 53-slot table
positionally while the layout declares those same slots, so the C8.10 path
keeps the one-sided-authority property `init.rs` shed; `boot_layout_check`
covers it, but by inspection rather than construction. `launch_recovery_init`
is unchanged and was decided out of scope: its trigger is already
generation-data-driven, and no layout fixture covers its four-slot table.
`SLIME_INTERACTIVE` remains in `on_idle` — a user-facing mode from `just run`,
not a gate, and it does not divide the kernel binary across the suite. 52
`option_env!` sites remain in `components/`, which B10's text anticipated; the
component images are per-generation artifacts by design.

### B9 — terminated tasks are never reaped, so their frames never return

**Resolved:** 2026-07-28. See `devlog/2026-07-28-b9-task-frame-reclamation/`.

**Problem:** `task::terminate` marked a task `Terminated`, drained its
capabilities, and reclaimed its shared buffers, but never removed the `Task`
from the scheduler. The `Task` — and the `AddressSpace` it owns — therefore
lived for the rest of the boot, so `AddressSpace::drop` never ran. Even when it
did, that `Drop` freed only the PML4 frame and deliberately leaked every
user-half page table; the image and stack frames mapped by
`spawn_with_caps_for` had no release path at all. Every spawn permanently
consumed its image pages plus its stack pages, so a repeated spawn/exit
workload drained the frame allocator monotonically.

**Evidence:** `kernel/src/task/mod.rs` — `terminate` pushed to
`sched.terminated` and left the task in `sched.tasks`; `remove_task` was called
only from the `spawn_from_cap` capability-insert failure path.
`kernel/src/memory/address_space.rs` — `Drop` dealloc'd `self.pml4` alone, with
the comment that intermediate user-half tables "intentionally leak for the
small M2 isolation test". The per-cycle delta is no longer an inference: a boot
probe running four real spawn/release cycles before `launch_init` reported
`spawn/exit leaked: 52 frame(s) over 4 cycles` — 13 frames per cycle.

**Fix:** two gaps on one path, closed together. `vmm::free_user_half` walks
PML4 entries 0..256, freeing leaf pages then the tables that held them, and
`AddressSpace::drop` now calls it before releasing the PML4 — so every frame an
address space owns has a release path, including on the `spawn_with_caps_for`
early-return paths, which hold it as a local. `reap_terminated` gives the
scheduler a reclamation point, removing every terminated task except the one
the CPU is standing on; it runs from `schedule_next` after the switch target is
chosen. Reaping is deferred rather than immediate because `terminate` executes
on the terminating task's own kernel stack and address space. `sched.terminated`
stays a separate log, so `supervision_status` and `SYS_WAIT` still answer for a
reaped child. The kernel half (entries 256..512, shared aliases of the one
kernel hierarchy) is never touched.

**Exit condition (observed):** the boot probe reports `spawn/exit conserves
frames: 14 per cycle, 0 drift`, asserted by `just dango_check`. `just test`
passes 185 assertions including five new `task_reclamation` cases — eight-cycle
conservation, release scaling with image size, a task holding capabilities, a
rejected spawn, and the shared-buffer double-free ordering. Supervision results
stay observable after reaping, proven by `just spawn_service_check` and `just
dango_check`, whose components spawn and exit through `terminate` and the
reaper and still report a healthy slice; `just sample_plane_live_check` and
`just fabric_stream_check` are unaffected. Fault injection confirms the guards
bite: removing the `free_user_half` call makes both the harness tests and the
live probe fail, and inverting the reclaim/release order fails the double-free
test.

**Follow-up:** a task that terminates when nothing else is runnable is reaped by
the *next* scheduling event, which on the non-interactive path never comes —
`on_idle` exits QEMU. One task's frames are therefore returned to an allocator
that is about to stop existing, which is harmless today but is the residual
lag C10.4's spawn/exit measurement should quantify. The live probe covers the
release path rather than the reaper; a gate counting frames across a full
spawn/exit/reap cycle needs a userspace loop and belongs with that milestone.

### B8 — budget validation bounded each holder but never the aggregate

**Resolved:** 2026-07-26. See `devlog/2026-07-26-b7-b8-budget-hygiene/`.

**Problem:** `SharedBufferBudget::validate_against` checked each holder's quota
against the fixed kernel ceilings but never summed holders, so a budget could
promise N holders `MAX_TOTAL_PAGES` each. Not exploitable —
`SharedBufferTable::create` still enforced the real global ceiling — but the
roadmap said decode rejects "globally impossible" limits, and an aggregate
over-commit degraded a declared quota into first-come-first-served: a
late-starting component failed with `BytesExhausted` despite holding a quota the
generation promised it.

**Evidence:** `boot-contracts/src/shared_buffer_budget.rs:116-148` looped per
entry with no accumulator; its comment noted `max_buffer_pages` was retained
only "for symmetry". Lib tests covered per-holder impossibility only.

**Fix:** Chose the stricter reading, since `AGENTS.md` requires generation data
to be deterministic, bounded, and explicitly validated: `validate_against` now
sums `byte_pages`, `buffer_count`, `mapping_count`, and `loan_count` with
saturating adds and rejects any total past its kernel ceiling, so a budget that
validates is one the kernel can honour with every holder at its ceiling at once.
Also added the two per-holder bounds the check was missing — `mapping_count` and
`loan_count` against `MAX_MAPPINGS`/`MAX_LOANS`, without which a holder could
declare 200 mappings against a 64-entry table. `validate_against` grew to five
parameters; the kernel caller passes the new ceilings.

**Exit condition (observed):** `cargo test -p boot-contracts --lib` passes 24
tests, including `aggregate_over_commitment_is_rejected`,
`aggregate_buffer_mapping_and_loan_ceilings_are_enforced`, and
`per_holder_mapping_and_loan_ceilings_are_enforced`. Fault injection confirms it
bites on the live path: raising the manifest to 306 aggregate pages (> 256) made
the boot fail closed, and the real budget (18/256 pages, 5/32 buffers, 10/64
mappings, 5/64 loans) passes. `just generation_check` (two byte-identical
builds), `just contracts_check`, `just spawn_service_check`, `just
sample_plane_live_check`, `just test`, and fmt/lint are clean.

**Follow-up:** The host builder does not validate the aggregate; only the kernel
does at decode, so an over-committed manifest builds and fails at boot. That is
fail-closed and keeps one source of truth for the rule.

### B7 — the `RIGHT_MAP` rename never reached the manifest vocabulary

**Resolved:** 2026-07-26. See `devlog/2026-07-26-b7-b8-budget-hygiene/`.

**Problem:** C7.1's deliverable was to replace the grandfathered generic
`RIGHT_MAP` name with an object-specific shared-buffer map right. The kernel
constant became `RIGHT_BUFFER_MAP`, but the manifest key stayed `map`, so
generation authors kept writing a generic name for buffer-specific authority.

**Evidence:** `scripts/build/build-generation.py:112` mapped `"map": 1 << 9`
alongside object-specific siblings `bufferWrite`, `bufferCreate`, `bufferLoan`;
`kernel/src/capability/mod.rs:39` defined the same bit as `RIGHT_BUFFER_MAP`.

**Fix:** Renamed the builder key to `bufferMap`. No wire or identity change —
the bit value is unchanged and no manifest fixture referenced the old key.

**Exit condition (observed):** No `"map"` key remains in the builder rights
table; `just generation_check` produces two byte-identical builds and `just
framework_safety_check` stays clean.

### B6 — the retained-v2 "still boots" claim was proven only as decode

**Resolved:** 2026-07-26 (scope corrected + admission covered). See
`devlog/2026-07-26-b6-retained-v2-rollback-scope/`.

**Problem:** C7.1's exit condition stated that a retained v2 known-good artifact
"still decodes **and boots**". Only decode was proven; no v2 generation was ever
booted.

**Evidence:** `scripts/lib/boot_contracts.py:7-8` pins `GENERATION_MAGIC =
b"SLIMEG3\0"` / version 3, so the builder emits v3 only. The sole v2 artifacts
were hand-built in memory (`boot-contracts/src/generation.rs`,
`kernel/tests/sample_plane.rs:564`).

**Resolution:** The boot arm is not merely unproven, it is unconstructible from
this tree, and investigating why closed a more interesting question.
`stage0::verify_kernel` (`stage0/src/lib.rs:320-325`) resolves
`generation.kernel_object`, so each generation embeds and boots its **own**
kernel. A retained v2 generation therefore runs its v2-era kernel — which is
also why this tree's v3-only rights cannot break the rollback window, despite
`bufferCreate` (bit 24) lying outside v2's 24-bit rights space and
`require_grant` being unconditional. Any "v2 boot" staged today would pair a v2
manifest with a v3-era kernel: a configuration that has never existed.

Covered the provable and load-bearing part instead — the stage-0 admission
chain, which had no coverage. Two `boot-contracts` tests were added:
`retained_v2_generation_passes_stage0_admission` (identity seal, kernel object,
bootstrap component, tamper detection) and
`retained_v2_authority_manifest_is_width_stable`, which pins the 32-bit v2
authority hash. That second one guards a real hazard: `release.rs:163` binds a
signed release to `authority_manifest_identity`, so losing the version branch
would fail every retained v2 release while every gate stayed green. C7.1's
status and exit condition now claim decode + release authorization + admission,
and state why the boot arm cannot be staged.

**Exit condition (observed):** `cargo test -p boot-contracts --lib` passes 21
tests (19 prior + 2 new). Fault injection confirms the guard bites: removing the
v2 branch from `authority_manifest_identity` so it hashes at 64-bit made
`retained_v2_authority_manifest_is_width_stable` fail, and the branch was
restored. `just contracts_check`, `just generation_check`, and `just
transfer_check` all pass.

**Follow-up:** If a real v2 generation is ever recovered from history, booting
it under QEMU would upgrade this from admission to a true rollback boot. The
rollback window also remains unlimited in code — v2 retention is unconditional
decode support, noted since C7.1.

### B5 — no C7 gate exercised the syscall layer or real components

**Resolved:** 2026-07-26. See `devlog/2026-07-26-b5-live-sample-plane/`.

**Problem:** No test or component reached any `SYS_SHARED_BUFFER_*` syscall. The
gates called `SharedBufferTable` methods on locally constructed tables and never
touched the global `SHARED_BUFFER_TABLE`, so the rights gates, the loan receiver
binding, and reclamation through real termination were unproven. C7.7's "two
isolated components" were the `u64` constants `0x71`/`0x72`, and its "peer death"
was a direct `reclaim_owner` call. This is the blind spot B3's boot wedge shipped
through.

**Evidence:** `grep 'dispatch|UserFrame|sys_'` and `grep SHARED_BUFFER_TABLE`
over `kernel/tests/` both returned no matches, while `SharedBufferTable::new()`
appeared 33 times. `kernel/tests/sample_plane.rs:57-58` defined its holders as
bare integers; `:462` stood in for peer death with `reclaim_owner`.

**Fix:** Added the four missing loan wrappers (`loan`/`loan_map`/`return`/
`revoke`) to `slime_rt`, completing the nine-syscall surface begun in B4. Added
two real components, `sample-lender` and `sample-receiver`, that the generation
grants a factory, a channel, and a `supervise` handle; init spawns the receiver
first so the lender names its loan receiver by capability rather than ambient
task id. `just sample_plane_live_check` asserts an ordered transcript covering
the happy path plus six denial arms, and rejects any component `fail:` line.
A first draft exposed a real ordering property: a lender that exits before the
receiver maps has its loan settled by its own termination, so the lender now
waits for a settle message — the C7.5 retention rule, asserted rather than raced.

**Exit condition (observed):** `just sample_plane_live_check` passes: two
separately spawned components move a two-page payload — larger than `MAX_MSG` —
through the real syscalls, with only the 64-byte descriptor crossing the IPC
channel, and every denial arm observed before the operation it guards.
`just sample_plane_check` (5/5), `just test`, all shared-buffer gates
(8/8/8/7/4), `just spawn_service_check`, `just dango_check`, `just
powerbox_check`, `just transfer_check` (exercising the renumbered slots 45/46),
`just generation_cmd_check`, `just generation_check`, `just
framework_safety_check`, and fmt/lint with `_components` are all clean.

**Follow-up:** `SYS_SHARED_BUFFER_REVOKE` has a wrapper and in-harness coverage
but no live caller, since the lender settles by return. The two insert-failure
rollback paths still need a full capability table at the moment of insert, which
neither gate stages.

### B4 — the C7 shared-buffer plane was dormant on the live boot path

**Resolved:** 2026-07-26. See `devlog/2026-07-26-b4-live-shared-buffer-budget/`.

**Problem:** Nothing in a running system could allocate a shared buffer. No
generation declared a `shared-buffer-budget/v1` resource, so every component
launched with `HolderQuota::DENY`; no manifest granted `bufferCreate`; the
kernel never minted a `SharedBufferFactory`; and `slime_rt` had no wrapper for
any shared-buffer syscall. C7.3's exit condition ("two holders receive distinct
generation-declared budgets") therefore held only inside the kernel test
harness. C7.2/C7.3/C7.4 each deferred this wiring to C7.7, which closed without
doing it.

**Evidence:** The built `generation-1.bin` held 21 objects and zero of kind
`KIND_RESOURCE`; the one `SLIMESB` match sat inside the kernel object's byte
range, not an object payload. No `bufferCreate` grant in the manifest fixture;
`bootstrap.rs` minted `EndpointFactory` and `Input` but never
`SharedBufferFactory`.

**Fix:** Emit the budget as a digest-authenticated `KIND_RESOURCE` object from
`build-generation.py` (entries sorted by `holder_identity` and duplicate-checked,
as `SharedBufferBudget::decode` requires); declare per-holder quotas and two
`bufferCreate` grants in the manifest; mint one transferable
`SharedBufferFactory` in `bootstrap.rs` at a fixed slot ahead of the optional
transfer block (renumbering the transfer slots to 41/42) and validate both
grants with `require_grant`; add the five missing `slime_rt` wrappers; and run a
bounded create/map/write/seal/unmap/release self-check at dango and
spawn-service startup so a normal boot proves its own quota.

**Exit condition (observed):** A built generation contains exactly one
`KIND_RESOURCE` budget object (128 bytes, digest verified, magic `SLIMESB\0`,
two holders sorted by identity) that `crate::generation::decode` validates.
A normal boot prints `[generation] shared-buffer factory grants valid`,
`[dango] shared-buffer quota live`, and `[spawn-service] shared-buffer quota
live`, then `vertical slice healthy`. The new
`booted_generation_declares_distinct_holder_budgets` case decodes the booted
generation and asserts two distinct non-`DENY` quotas with an absent component
denied. `just generation_check` produces two byte-identical builds; `just
test`, all six C7 sub-slice gates (8/8/8/7/4/5), `just dango_check`, `just
transfer_check`, `just generation_cmd_check`, `just contracts_check`, `just
framework_safety_check`, and fmt/lint (with `_components`) are clean.

**Follow-up:** B5 is partly addressed — five syscalls are now exercised on a
live boot, but the four loan syscalls still have no wrapper and no test drives
any syscall.

### B3 — C7.5 wedged every full-graph boot (kernel-stack overflow)

**Resolved:** 2026-07-26. See
`devlog/2026-07-26-b3-shared-buffer-table-stack-overflow/`.

**Problem:** From C7.5 onward every boot that launched the full component graph
hung instead of draining its ready queue. `transfer_check` stalled after
`[init] generation transfer installed`; `spawn_service_check` and `dango_check`
stalled after `[init] spawn graph launched`. `on_idle` is the only path to
`exit_qemu`, so the guest never exited and each gate died on its timeout — the
same observable class as B2, but an unrelated cause.

**Evidence:** Bisected one gate per worktree: `just transfer_check` passed at
C7.2 `991dcbb`, C7.3 `ed49fb5`, and C7.4 `928389e`, and wedged at C7.5
`ca15764` and HEAD; `just spawn_service_check` passed at `928389e` and wedged
at `ca15764` and HEAD. Not timeout tuning: raising the inner QEMU timeout from
60 s to 600 s still wedged. `git diff --stat ca15764 HEAD -- kernel/src` is
empty, so C7.6/C7.7 were not implicated. Full transcript in
`devlog/2026-07-26-c7-audit/transcript.txt` §3–§4.

**Root cause:** Kernel-stack overflow, not the reclamation logic first
suspected. C7.5 grew `SharedBufferTable` to 10520 bytes of fixed arrays
(`loans: [Option<Loan>; 64]` plus a widened `Mapping`), and the table was
published through a `LazyLock`, whose initializer builds the value on whichever
stack first touches the static. Because no `SharedBufferFactory` is minted on
the live path (B4), the first touch is `SHARED_BUFFER_TABLE.lock()` inside
`task::terminate` (`kernel/src/task/mod.rs:832`) — on a 32 KiB task kernel stack
allocated as a plain boxed slice with no guard page. The 10 KiB temporary
overflowed it while `SCHEDULER` was held, corrupting adjacent memory silently
rather than faulting, so the boot wedged with no panic. Confirmed by raising
`KERNEL_STACK_SIZE` to 128 KiB with no other change, which made the gate pass.

**Fix:** Replaced the `LazyLock` with a plain `const`-initialized
`Mutex<SharedBufferTable>` static, matching `FRAME_ALLOCATOR` and the
`drivers/input.rs` tables. `SharedBufferTable::new()` was already a `const fn`,
so the laziness bought nothing; const-initializing places the table in `.bss`
and removes the stack temporary. The diagnostic stack bump was reverted. Added
a compile-time assertion that `size_of::<SharedBufferTable>() * 2 <
KERNEL_STACK_SIZE`, verified to fire by temporarily setting `MAX_LOANS = 1024`.

**Exit condition (observed):** `just transfer_check` (install, pending boot,
promotion, rollback retention), `just spawn_service_check`, and `just
dango_check` all reach their success lines and exit QEMU `Success` at the stock
32 KiB stack. `just test` (160 assertions), all six C7 sub-slice gates (8/7/8/7/
4/5), `just generation_cmd_check`, `just contracts_check`, `just
generation_check`, `just framework_safety_check`, `just fmt_check`, `just
lint`, `just fmt_check_components`, and `just lint_components` are clean.

**Follow-up:** Task kernel stacks still have no guard page, so a future
overflow will again corrupt memory silently instead of faulting. This fix
removes the trigger, not the class.

### B2 — scheduler has no `Blocked` task state (busy-poll pathology)

**Resolved:** 2026-07-24. See `devlog/2026-07-24-b2-blocked-task-state/`.

**Problem:** `TaskState` had only `Ready`/`Running`/`Terminated`. A task waiting
on input or IPC poll-and-yielded, staying `Ready`, keeping the ready queue
non-empty, so `on_idle` (the only path to `exit_qemu`) never fired and every
non-scripted full-graph boot wedged at `dango>`. A default Escape input script
masked the wedge without removing the pathology.

**Fix (design A — wait-set, not blocking recv):** Added
`TaskState::Blocked(BlockReason{Endpoint,Input,Supervision})` and a multi-source
`SYS_WAIT` syscall (max 8 sources, descriptors pack `kind<<32|slot`). `recv`/
`send`/`input_read`/`supervision_status` stay non-blocking; userspace sweeps its
sources then calls `wait` instead of `yield_now`. Waiter registration lives on
each wake source — `recv_waiter` in a new `ipc::Channel`, a global `INPUT_WAITER`
in `drivers/input.rs`, and `wake_on_terminate` on the child `Task`. Wakes are
deferred through a `PENDING_WAKES` queue drained inside `schedule_next` under
`SCHEDULER` (strict order `SCHEDULER → Channel/QUEUE/INPUT_WAITER →
PENDING_WAKES`), fed by `ipc::send`, the keyboard IRQ, `pump_script`,
`task::terminate`, and `Endpoint::Drop`. `wait` re-checks readiness under
IF-clear before parking to close the lost-wakeup race. The default-Escape hack
is removed; `on_idle` now treats an alive, cleanly-blocked persistent service as
healthy while one-shot probes must still `Exit(0)`, and `SLIME_INTERACTIVE`
routes into a new `task::idle_dispatch` (`sti; hlt`) loop instead of exiting.
A pre-existing regression was also fixed: `copy_from_current` bounded a byte
copy at `MAX_CAPS`=64 via a per-byte scratch array, and the `u64`-rights
`SpawnGrant` widening made dango's 5 grants (80 B) exceed it, so `sys_spawn`
returned `ERR_INVALID_ARG` and dango could not spawn.

**Evidence:** `devlog/2026-07-24-boot-check-hangs/` — every non-scripted
full-graph boot hung at `dango>` until an Escape keystroke was scripted.

**Exit condition (observed):** A non-scripted gen-1 boot parks `console`,
`dango`, and `spawn-service` as `idle-blocked` (consuming no CPU), the ready
queue drains to `on_idle`, and QEMU exits `Success` — no scripted Escape. Every
wake source re-readies its waiter: `just dango_check` (`dango native runtime
check: ok`), `just powerbox_check` (input + endpoint waiters), `just
generation_cmd_check` (multi-source generation-manager), `just
spawn_service_check`/`just storage_read_check` (`vertical slice healthy`), and
`just test` all pass, with `just fmt_check`/`just lint` (and `_components`)
clean.

### B1 — `generation_cmd_check` negative scenarios corrupted the wrong generation

**Resolved:** 2026-07-24.

**Problem:** `just generation_cmd_check` failed on its `bad-closure` and
`bad-release` scenarios. The original diagnosis (init's `spawn_and_wait`
aborting on a rejecting `Exit(1)`) was wrong: `generation-stage` already
classifies a `-4`/`-3` rejection internally and exits `0`, and init already
exits cleanly after the staged rejection. The real defect was in the fixture
builder `scripts/check/check-generation-commands.py`. `build_fixture` corrupted
`entries[1]` by fixed directory index, but the bootstore directory is
identity-sorted and staging targets the *candidate* generation (identity ≠
known-good). When component images changed the identity sort order, the
corruption landed on the untouched known-good generation, so staging *succeeded*
(`status=0`), `generation-stage` hit its non-`-4`/`-3` `fail()` path, and the
boot exited `Failed`.

**Evidence:** Instrumented `generation-stage` printed `unexpected status=0` on
`bad-closure`; probing the fixture confirmed the flipped byte fell inside the
known-good generation's blob, which staging never reads.

**Fix:** Select the candidate entry by `identity != known_good` (read from
BootState) instead of a fixed directory index, so the corruption always lands on
the generation staging actually validates.

**Exit condition (observed):** `just generation_cmd_check` passes for `success`
(`staged release=3`), `bad-closure` (`rejected status=-4`), and `bad-release`
(`rejected status=-3`), with rejected staging leaving both BootState slots
unchanged.
