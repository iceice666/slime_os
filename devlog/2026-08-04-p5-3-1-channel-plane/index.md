# P5.3.1: the channel plane on seL4

| Field | Value |
|---|---|
| Date | 2026-08-04 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/{channel,parked,main,transfer_window,child_vspace,ipc}.rs`, `components/bins/src/bin/init.rs`, `components/bins/build.rs`, `contracts/generation/v1/fixtures/sel4-channel.{zti,md}`, `scripts/build/{build-sel4,build-generation}.py`, `scripts/check/check-sel4-channel-plane.py`, `scripts/check/check-sel4-component-graph.py`, `Justfile` |
| Roadmap | P5.3.1, P5.3, P5.5, B12 |
| Gates | `just sel4_channel_check`, `just sel4_component_graph_check`, `just sel4_root_boot_check` |
| Trigger | P5.3 opened as the next uncompleted milestone after P5.2 |
| Baseline | P5.2: the declared graph boots and is served, but `Send`, `Recv`, and `Wait` have no handler — every component reaches its first `recv` and exits non-zero |

## Summary

`Send`, `Recv`, and `Wait` were classified `Mediation::RootService` but had no
dispatcher arm, so P5.2's graph ran and was served while doing no work over
channels. This slice makes a channel a real root-owned object: materialized from
the generation's declared send/recv grants before any component runs, named by a
logical slot the component was granted, and served with parking — a component
blocked in `recv` is held in the kernel and answered when its peer sends or dies,
rather than being told to retry.

Two components now exchange bounded messages on seL4 under `just
sel4_channel_check`, including a payload too large for the fast message registers
(which required giving the root a way to read a child's transfer window at all),
a bounded queue-full refusal, a refused capability-carrying send, and a receiver
woken by its peer's death. P5.1's and P5.2's gates still pass; P5.2's needed one
assertion re-evidenced in lockstep, because a marker it pinned recorded the
absence of exactly what this slice adds.

A read-only reviewer pass over the finished diff found five further defects, none
of which any gate would have caught — including two that would have hung a
component silently. They are recorded in the investigation log rather than folded
away, because the gate being green is exactly why they are worth stating.

P5.3 was also retitled and decomposed, and its C8 half split into a new P5.5 —
see **Decisions**.

## Investigation log

Eight defects surfaced while building this slice. Three were found by running
P5.2's gate as a mid-implementation checkpoint, before the new image existed —
while the diff was small enough that each was one change to localize.

1. **Grant names are not layout labels.** The first materializer required a
   layout slot for every grant touching `init` and made a missing one fatal;
   `sel4.zti`'s `spawn-service-rpc` has no such label and the boot died at
   `UnlaidSlot`. Fixed by skipping and counting — see **Decisions**.
2. **A frame capability records exactly one mapping.** Staging mapped the
   window's own frame at the root's scratch address, which cannot succeed while
   the child's mapping is live; the root then read an unmapped page and took a VM
   fault. Fixed with a per-window alias capability. The repository already
   documents this constraint in the shared-buffer phase, which unmaps before it
   reads.
3. **Root task stack overflow.** With the channel table added, the deepest
   service-loop frame — `SharedBufferTable::unmap`, whose `TeardownPlan` and
   `ActionList` locals are sized for the whole table — ran off a 256 KiB stack.
   It was visible only because it landed on `FREE_PAGE`, the scratch page
   `ScratchPage::claim` deliberately leaves unmapped; anywhere else it would have
   silently corrupted `.bss`. This is backlog B3's failure mode a second time,
   and it is why the channel table itself is a static rather than a local.

One was found by fault injection rather than by the gate: the first fixture
never left a receiver parked when its peer died, so dropping the peer-death wake
entirely still passed. The fixture now has `console` block again before `init`
exits, and the gate asserts `woken=1`.

The remaining five were found by review rather than by any run, and none of them
would have failed a gate — which is the point worth recording, since the gate
was green when the review started.

4. **`ParkedReplies::commit` destroyed the reply it was refusing.** On failure it
   consumed the `SavedReply` and deleted its CSlot, so a refused park would have
   left that component blocked forever with the one capability that could answer
   it gone — a hang with no marker, since the root serves everyone else. `commit`
   now hands the save back and both call sites answer with the bounded error.
5. **`deliver_wake` never woke a `wait`-parked task.** The return expression
   short-circuited on a flag that is false for `ParkReason::Wait`, so
   `parked.wake` was never reached for it and the task would be held forever.
   Unconditional, and invisible: neither shipped graph parks on `wait`.
6. **The fault path did not tear down what the exit path did.** It reclaimed
   channels but not the capability table or window, so `windows=0 tables=0` would
   have misreported after any fault; and an *undecodable* fault returned without
   suspending or decrementing `live`, wedging the loop. Both paths now do the
   same teardown.
7. **A self-edge allocated a queue nothing could name.** For a loopback both
   accessors resolve to `forward` — correctly, since a task sending to itself
   must receive what it sent — but `push` still built `reverse`, and the boot
   marker reported two queues where the graph has one. Now allocated and reported
   as one. The two-party bidirectional case is implemented but unexercised by
   this graph; noted as a follow-up rather than claimed.
8. **The layout rights check validated bits that were never installed.**
   `bootstrap_slot` was given the grant's declared rights while `materialize`
   installed the per-end derived ones, so the containment check could not see the
   case it exists for. Both now use `held_rights`. Tightening it immediately
   caught a real fixture error: `init`'s send end was placed under
   `console-output`, whose layout entry declares the *receive* half. The fixture
   now uses `dango-output`, the label that declares the send half.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `slime-root/src/channel.rs` (new) | `ChannelTable` materialized from declared grants; per-end rights; layout-driven slots for the bootstrap component and a cursor for everyone else | A component finds exactly the channels its generation declared, at the slots it compiles against, with only the rights its end holds |
| `slime-root/src/parked.rs` (new) | Reply authority saved out of the implicit slot with `seL4_CNode_SaveCaller`, held per task, answered on wake | A blocked caller is answered rather than told to spin; every saved CSlot is handed back |
| `slime-root/src/main.rs` | `Send`/`Recv`/`Wait` arms; loop routed through `ipc::recv_request`; peer-death settlement on both the exit and fault paths; channel accounting in a second terminal marker | The whole channel plane resolves to a bounded answer; a dying task's peers are told |
| `slime-root/src/transfer_window.rs` | Root-side staged read/write through a per-window frame alias | The root can read a payload a component staged, without unmapping the live child's own view of it |
| `slime-root/src/child_vspace.rs` | A second capability to each window frame, from the same allocator cursor | The alias is covered by the task's existing cleanup record |
| `slime-root/src/main.rs` | Root task stack 256 KiB → 1 MiB | The deepest service-loop frame (a shared-buffer teardown) fits |
| `contracts/generation/v1/fixtures/sel4-channel.zti` | A third seL4 generation: two components, two channel grants | The scenario is declared data, not a flag |
| `components/bins/src/bin/init.rs` | One guarded `SLIME_SEL4_CHANNEL_CHECK` branch | — |
| `scripts/build/*.py` | Manifest selector, third image variant, per-manifest component target directory | Each gate boots the artifact it asserts about |

`components/runtime/`, `console.rs`, `kernel/`, and `sel4.zti` are unchanged.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A channel materialized backwards — both ends waiting to receive | `just sel4_channel_check` | The `producer=1 consumer=0` marker, and a boot that reaches no further |
| An end granted rights it does not hold | `just sel4_channel_check` | The per-end `rights=0x1` / `rights=0x2` markers |
| A send accepted past the channel's depth | `just sel4_channel_check` | `check_queue_depth` asserts the exact accepted count, not just that a refusal happened |
| A payload shrinking below the inline bound, silently skipping the window path | `just sel4_channel_check` | `check_payload_crosses_the_window` fails before the boot |
| A parked caller left blocked | `just sel4_channel_check` | `parked=0` in the terminal marker; `SLIME_GRAPH park refused` is a failure marker |
| A peer's death not reaching a parked receiver | `just sel4_channel_check` | `woken=1` in the peer-death marker |
| A loopback allocating a queue nothing can name | `just sel4_channel_check` | `queues=1` on the self-edge, and `queues=2` in the totals |
| P5.2's evidence silently changing | `just sel4_component_graph_check` | Every marker but the `unimplemented` assertions is unchanged; `unimplemented=0` is now pinned exactly, so an operation losing its handler fails there |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_channel_check` | Pass | Direct — [`channel-plane-boot.log`](channel-plane-boot.log) |
| `just sel4_component_graph_check` | Pass | Direct |
| `just sel4_root_boot_check` | Pass | Direct |
| `just contracts_check`, `just generation_check` | Pass | Direct |
| `just fmt_check_all`, `just lint_all` | Pass | Direct |
| `just test` (x86 oracle) | Pass | Direct |
| `just test_host`, `just ruff`, `just typos`, `just machete` | Pass | Direct |
| Fault injection: cap-transfer refusal removed | Gate fails — `[init] channel plane fail: capability transfer was permitted` | Direct |
| Fault injection: queue-full refusal removed | Gate fails — `[init] channel plane fail: a full queue accepted more than its depth` | Direct |
| Fault injection: peer-death wake dropped | Gate fails — boot times out with `woken=0` | Direct |

The queue-full injection needed **both** the preflight and commit bounds removed
before the gate failed; removing only the preflight check passed. That is
defence in depth working as designed rather than a gap — `Channel::commit_send`
re-checks the bound — but it is recorded because a single-site injection would
have wrongly read as the arm being uncovered.

`slime-root`'s `#[cfg(test)]` unit tests, including the ones added here, are
**not run by any gate**: `cargo test -p slime-root` fails to build the
`unwinding` crate for the host target. That is pre-existing — every `slime-root`
module has had unreachable test modules since P5.1 — and it means the QEMU
markers are the whole of this slice's evidence. Recorded as a follow-up rather
than fixed here.

## Decisions

- **Decision:** Decompose P5.3 into P5.3.1–P5.3.4, and land only the channel
  plane.
  **Rationale:** P5.3's exit condition needs four independent state surfaces —
  channels, the loan plane, child construction, death reclamation — and none of
  the other three existed. The repository already did this to C7 and to C8.9 for
  the same reason.
  **Rejected alternative:** One slice reaching the stated exit condition; the
  diff would span `slime-root/`, both build scripts, a new fixture, and a new
  gate at once, and a partial landing would be indistinguishable from a complete
  one.

- **Decision:** Split C8 into a new P5.5 and retitle P5.3 to the C7 sample plane.
  **Rationale:** The heading claimed C8 while its exit condition named only
  C7-shaped properties. The minimal typed-fabric slice is four tasks plus
  `Operation::CapTransfer`, because the C8.3 claim is that a participant is
  *provisioned* a route endpoint rather than holding one.
  **Rejected alternative:** Leaving the heading and recording the gap in this
  entry; a roadmap heading that outruns its own exit condition is how a milestone
  gets closed on partial evidence.

- **Decision:** Park a blocked caller by saving its reply capability, rather than
  answering `ERR_WOULDBLOCK` and letting it re-poll.
  **Rationale:** `MAX_GRAPH_ITERATIONS = 512` bounds a wedged graph; two children
  spinning through `recv`/`wait` would burn it, and raising it weakens the
  detector it exists to be. `ipc.rs` was already built for parking —
  `register_receive_waiter`, `WakeDecision`, `WakeBatch` all existed and were
  called only from that module's own tests.
  **Rejected alternative:** Poll-and-retry; functionally sufficient, since both
  components already loop on `ERR_WOULDBLOCK`, but it trades the wedge detector
  for the easier implementation.

- **Decision:** Refuse capability-carrying sends rather than implement transfer.
  **Rationale:** This slice mediates no transferable logical resource — loans
  arrive in P5.3.2 — so a transfer implementation would be code no caller
  exercises, justified only by a future one. The refusal is observed by the gate
  and fault-injected.
  **Rejected alternative:** Implementing the move now against `CapabilityTransfer`;
  it would ship untested.

- **Decision:** A channel grant naming the bootstrap component that the boot
  layout does not label is skipped and counted, not placed at a derived slot.
  **Rationale:** Grant names and layout channel labels are different namespaces —
  the layout labels the two *halves* (`dango-spawn`, `service-spawn`) while the
  generation names the *grant* (`spawn-service-rpc`), and the retired kernel
  hardcodes the correspondence in `bootstrap.rs`. Those are the halves `init`
  brokers through spawn, which is P5.3.3.
  **Rejected alternative:** Deriving a slot; a component would find a channel at
  a number it never compiled against.

- **Decision:** The boot layout's rights **bound** a grant's rather than equalling
  them.
  **Rationale:** A layout entry states what the slot carries, including
  `RIGHT_TRANSFER` for every half `init` brokers onward; a grant states what the
  channel confers on its endpoints. Requiring equality demanded that every
  generation restate a delegation bit about a different thing, and the first
  version of this check rejected a well-formed graph for exactly that. Containment
  still fails a grant that exceeds the slot.

- **Decision:** Keep `component_graph` in the image identity manifest and add
  `variant` beside it.
  **Rationale:** P5.1's and P5.2's gates assert on that field. A third image is no
  reason to edit verification code those slices' evidence rests on.

## Open risks and follow-ups

- [ ] `slime-root`'s unit tests are unreachable in **both** directions:
      `cargo test -p slime-root` fails building `unwinding` for the host, and
      `cargo clippy --all-targets` for the seL4 target fails with `can't find
      crate for \`test\``. Pre-existing since P5.1 — every module's
      `#[cfg(test)]` block is dead, and `lint_sel4_root` does not pass
      `--all-targets`, which is why stale test code compiled nowhere and went
      unnoticed. Until it is fixed, `slime-root` behaviour is only ever evidenced
      by QEMU markers, and the ~160 lines of tests added here are documentation.
- [ ] Two-party bidirectional channels are implemented but unexercised: the only
      bidirectional grant in this fixture is a loopback. Covering it needs a
      second component with a channel to reply on, which is P5.3.3's spawn-time
      distribution.
- [ ] `MAX_WAIT_SOURCES` (9) × `WAIT_RECORD_BYTES` (8) is 72, over
      `MAX_STAGED_BYTES` (64), so a maximal wait set is refused rather than
      staged. A bounded refusal, not an overrun, but the two bounds should agree.
- [ ] A failed `frame_unmap` in `with_window_mapped` would leave the scratch
      address occupied and wedge every subsequent windowed operation. It returns
      `TransferFailed`, so the caller is answered, but there is no recovery.
- [ ] `MAX_CHANNELS = 16` with queues stored inline makes `ChannelTable` tens of
      kilobytes, and it lands in `.data` rather than `.bss` because
      `Option<Message>::None` is not all-zero. Costs image size only, but a larger
      graph will want a representation that zeroes.
- [ ] The scenario orders itself against its peer with bounded `yield_now` loops,
      because a component holds no capability naming another task. The counts are
      bounds, not timing assumptions — the gate asserts the root's own `parked`
      and `woken` markers, so an insufficient count fails rather than silently
      skipping the arm — but a real ordering primitive would be better.
- [ ] B12 remains open and deferred; the reason was re-reviewed before this gate
      opened and recorded in `roadmap/00-backlog.md`.

## Artifacts and provenance

- Raw transcript: [`channel-plane-boot.log`](channel-plane-boot.log)
- Fixture rationale: [`contracts/generation/v1/fixtures/sel4-channel.md`](../../contracts/generation/v1/fixtures/sel4-channel.md)
- Related roadmap item: [P5.3.1](../../roadmap/07-architecture-portability.md)
