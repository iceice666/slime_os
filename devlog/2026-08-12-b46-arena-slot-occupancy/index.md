# B46 — an arena returns a CSlot the kernel still finds occupied

| Field | Value |
|---|---|
| Date | 2026-08-12 |
| Kind | Defect |
| Status | Verified |
| Scope | `slime-root/src/object_allocator.rs`, `components/bins/src/visibility_broker.rs`, the four fabric participants, `components/bins/src/bin/init.rs`, `scripts/build/build-generation.py`, `contracts/generation/v1/fixtures/sel4-visibility.zti`, `scripts/check/check-sel4-visibility-plane.py` |
| Roadmap | B46, B50 |
| Gates | `just sel4_stream_check`, `just sel4_visibility_check`, `just test_sel4_root`, `just lint_all`, `just fmt_check_all` |
| Trigger | `c8fc792` handoff item R1: `fabric-subscriber` fails to map its ring because `reserve_slot` returns an index the kernel refuses `DeleteFirst` |
| Baseline | Before the arena's minted-endpoint slots existed, every arena-charged CSlot held an object derived from the arena's own untyped, so the parent revoke emptied all of them |

## Summary

`ObjectAllocator::release_task_arena` revoked the arena's parent untyped and then
returned every CSlot the arena had charged straight to the bitmap. That is
correct only for slots holding objects *retyped from that untyped*. B46's native
IPC cutover added a second kind: `reserve_slot_in` charges a bare CSlot to an
arena while the capability that lands in it is minted from a **globally**
allocated Endpoint or Notification. No revoke of the arena parent can reach such
a capability, so the slot survived teardown still occupied while the pool was
told it was free. The pool tracks availability and never occupancy, so it could
not notice. The first later `reserve_slot` to reach that index was refused
`DeleteFirst` — surfacing 400 lines downstream as `fabric-subscriber`'s ring map
failing. Deleting each recorded slot before releasing it restores the invariant
that a released index is empty; `just sel4_stream_check` now runs the whole
scenario past the blocker.

## Observable symptom

- Command: `just sel4_stream_check`
- Expected: the stream plane provisions every ring and reaches its terminal marker.
- Observed: `fabric-subscriber` exits 1 at `subscriber ring map`; the graph then
  exhausts its iterations with five instances still live.
- Exit/fault/serial evidence:

```
<<seL4 … CNode Copy/Mint/Move/Mutate: Destination not empty.>>
SLIME_GRAPH loan mapped refused task=2 slot=1 class=other
[fabric-subscriber] fail: subscriber ring map
SLIME_ROOT FATAL SLIME_GRAPH FAIL graph iterations exhausted live=5
```

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | Printed pool geometry at boot: `base=955 len=3141 end=4096`, matching `bootinfo.empty().range()` exactly. The refused index is 1501. | 1501 is *inside* the pool's range. The handoff's leading candidate — a pool span overstating the genuinely empty BootInfo region — is eliminated. |
| 2 | Probed the reserved slot's real occupancy with `Move` onto itself before copying. The first five alias reserves answered `empty`; the sixth, at 1501, answered `occupied`. | The bitmap and the kernel disagree about exactly one index. This is an occupancy defect, not an arithmetic one. |
| 3 | Swept the whole 3,141-slot range comparing bitmap bits against kernel occupancy, before each reserve. Every sweep reported `ghosts=1 first_ghost=1501`, already present at the *first* alias reserve (`live=493`), long before the cursor advanced that far. | The capability was installed early and orphaned. Nothing reused it; the pool simply never knew it was full. Confirms the handoff's finding that the reuse theory is dead. |
| 4 | Probed each slot in `release_task_arena`'s release loop. Exactly one reported `still-occupied slot=1501`, immediately after `fabric-intruder` (task 6) exited. | The leak is created by arena teardown, not by any installer. |
| 5 | Logged the root-side index each minted endpoint occupies: `native endpoint task=6 native=33 root_minted=1501 object=1035`. Object 1035 came from `allocate_fixed` (global), while slot 1501 came from `reserve_slot_in` (arena-charged). | Root cause: the arena owns the *slot* but not the *object*, so its parent revoke cannot empty the slot. |

Raw sweep transcript: [`occupancy-audit.log`](occupancy-audit.log).

## Root cause

`release_task_arena` conflated two things an arena can own. `allocate_in`
retypes an object *from the arena's untyped* into a charged slot, so revoking
the parent destroys the object and empties the slot. `reserve_slot_in`, added by
the native-IPC cutover, charges only the slot; `peer_endpoint::install_instance`
and `notification::install_instance` then mint into it from an Endpoint or
Notification allocated globally by `allocate_fixed`. Revoking the arena's parent
untyped is not an ancestor of that capability and therefore does not touch it.

The violated invariant is `release_slot`'s own documented precondition — *the
caller must have emptied the slot first* — which `release_task_arena` asserted
by construction rather than establishing. Because `SlotPool` records
availability and not occupancy, the divergence was silent until the allocation
cursor wrapped around to the orphaned index thousands of allocations later. The
`DeleteFirst` at `buffer_adapter::alias_frame` is the innocent crash site: it is
merely the first caller unlucky enough to be handed slot 1501.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `object_allocator.rs::release_task_arena` | Delete each recorded CSlot before releasing it to the bitmap; propagate failure as `ArenaCleanup` | A released index is empty, so the pool's availability bitmap and kernel occupancy cannot diverge |
| `contracts/generation/v1/fixtures/sel4-visibility.zti` | Declare the 28 endpoint edges as ordinary `bindings` rather than `mintedBindings`; keep only what init genuinely creates at runtime (publisher-b's buffer factory, four supervision handles) minted | The builder's closure rule holds, and the root materializes each declared edge exactly as the working stream fixture does |
| `visibility_broker.rs` | Derive the seven route-endpoint slots from `FIRST_CONTROL_SLOT + FABRIC_CLIENTS.len() + FABRIC_SUPERVISION.len()` instead of hardcoding 7..13 | A route edge cannot land on a supervision handle when the participant set changes |
| `fabric-publisher.rs`, `fabric-subscriber.rs`, `fabric-publisher-b.rs`, `fabric-subscriber-b.rs` | Take the visibility role from the *declared* endpoint the descriptor names instead of calling `capability_import()` on a reply that carries nothing; subscriber answers the proxy with a real `WireStreamAck` | The plane's roles are generation facts, as the post-cutover broker already assumed |
| `build-generation.py` | Grant a supervision handle to every declared interposition proxy, not only ring holders | The fabric can observe a proxy's death, which no native Endpoint reports |
| `init.rs` | Spawn the declared proxy before the fabric and pass its handle among the supervision grants | A handle cannot name a task that does not yet exist |
| `visibility_broker.rs`, `fabric-subscriber.rs` | Replace the three `ERR_PEER_DEAD` waits with `await_exit` on the supervision handle; restore the subscriber's post-loss view paging | Termination is observed through a capability rather than an absent signal |
| `check-sel4-visibility-plane.py` | Match the cutover's `endpoints=`/`notifications=` spawn marker instead of the retired `channels=`, correct `grants=13`→`22`, and record the inverted spawn order | The gate resolves init's task id again — matching `channels=` silently never matched, so it could not see init's clean exit |
| `syscall.rs`, `sel4_transport.rs` | Add `try_send`, a best-effort send over `seL4_NBSend` | An *unsolicited* message to a peer that is not receiving no longer blocks the sender; `send` on a native Endpoint always blocks, so `ERR_WOULDBLOCK` was unreachable and every non-fatal `WOULDBLOCK` arm in the fabric was dead code |
| `fabric-service.rs` | Send QoS events and terminal `STREAM_END` with `try_send`, re-offering END each pass until the peer takes it or exits; claim delegated loans with `capability_import` rather than reading `received[0]`; outlive every ring holder before exiting | The broker cannot wedge on a participant that has moved on to its ring, and a loan mapping is not reclaimed under a task still running against it |
| `fabric-subscriber.rs`, `fabric-subscriber-b.rs` | Claim delegated loans with `capability_import`; **block** on the control endpoint once the ring is drained | Only a native Endpoint travels inline in a message — every other kind is a root-recorded export. And `seL4_NBSend` delivers *only* to a receiver already blocked on the endpoint: a reader that merely polls is permanently invisible to it, so two non-blocking peers live-lock while the sender faithfully re-offers |
| `fabric-subscriber-b.rs` | Read the shared control endpoint in one place and file each record under the route named by its own `type_identity`, replacing two `static mut` flags | Both routes share one endpoint and a receive is destructive; every record already carries the route it belongs to, so the reader that owns the endpoint dispatches rather than each loop guessing |
| `fabric-subscriber.rs` | Restore the subscribe role's two authority assertions, checking the descriptor's declared direction and that its rights carry no send | The cutover dropped them entirely. Probing by *asking* for the publish side proves nothing: the fabric reads a request's `direction` only to discard it and answers each client exactly once |
| `fabric-service.rs` | Derive C8.5's `EVENT_PEER_DEAD` from the publisher's supervision handle | A publisher that exits without `FLAG_LAST` leaves no trace on a native Endpoint. `EVENT_PEER_DEAD` existed in the contract with nothing emitting it — the same absent-signal defect as the visibility plane's, and the same supervision-handle answer |
| `check-sel4-stream-plane.py` | Correct `grants=9` to the cutover's `grants=5 endpoints=7 notifications=12`; stop ordering participant markers against `provisioned`/refusal lines that race them; match the root's actual terminal-accounting order; fix `roles`→`rings` | Each stale assertion pinned a scheduling accident or a retired marker shape rather than a causal fact |

Deleting is unconditional and idempotent: a slot the revoke already emptied
deletes successfully as a no-op, so the arena-owned majority is unaffected.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| An arena returns a still-occupied slot | `just sel4_stream_check` | A later `reserve_slot` is refused `DeleteFirst`; a loan map fails and the graph exhausts its iterations |
| A recorded slot cannot be emptied at teardown | `just test_sel4_root` | `release_task_arena` answers `AllocError::ArenaCleanup` rather than silently reusing the index |
| The visibility fixture stops closing over its grants | `just sel4_visibility_check` | `build-generation.py` fails `bindings do not close over related grants` at build, before boot |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_stream_check` | **PASS** — `57 markers observed across 14 causal chains`; `init spawned the six declared participants and none of them reported a failure`; `57 frozen markers plus 4 declared seL4-only marker(s)` | Direct |
| `just test_sel4_root` | 118/118 across 13 modules | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just devlog_check`, `just typos` | Clean | Direct |
| `just sel4_channel_check`, `just sel4_crossing_check` | Both still pass — no regression from the teardown change | Direct |
| `just sel4_root_boot_check` | Pass | Direct |
| `just sel4_visibility_check` | **PASS** — `25 markers observed across 7 causal chains; 12 view records and 2 distinct traces; six spawned tasks exited cleanly` | Direct |
| `just contracts_check`, `just sel4_boot_layout_check`, `just sel4_supervision_check` | Fail, but **identically with the changes stashed** — pre-existing, unrelated to this entry | Direct (baselined) |
| `just sel4_qos_check`, `just sel4_call_check`, `just sel4_operation_check` | Fail at `spawn refused … ungranted`, **identically with the changes stashed** — the same fixture-vs-`FABRIC_CLIENTS` drift, untouched by this entry | Direct (baselined) |

## Decisions

- Decision: empty every arena-recorded CSlot at teardown rather than tracking
  which slots hold arena-derived objects and which hold globally minted ones.
- Rationale: the delete is idempotent and cheap, and it makes the postcondition
  unconditional — a released index is empty, full stop. A two-class slot record
  would put the burden on every future caller of `reserve_slot_in` to classify
  correctly, which is exactly the reasoning that failed here.
- Rejected alternative: skipping occupied slots inside `reserve_slot`. The
  handoff rules this out and is right to: it hides an allocator-integrity defect
  and would let two owners believe they hold the same slot.

- Decision: fix the visibility fixture by moving endpoint edges *out* of
  `mintedBindings` into ordinary `bindings`, rather than marking their grants
  `minted = true`.
- Rationale: post-cutover, init "holds no route capability and mints nothing" —
  control endpoints are generation-declared and root-materialized. The working
  `sel4-stream.zti` shows the intended shape: `mintedBindings` carries only what
  init creates at runtime. Marking the grants minted was tried first and made
  the plane worse — the root then pre-created nothing and init had nothing to
  hand over, so spawn was refused `ungranted`.
- Rejected alternative: weakening the builder's `expected_bindings` closure rule,
  which the handoff explicitly forbids and which would have masked the fixture's
  real disagreement with the cutover.

- Decision: cut the five visibility participants over to *declared endpoints*
  rather than teaching the broker to export a capability per role.
- Rationale: the pre-cutover broker moved a real capability with `cap_transfer`;
  the post-cutover one sends the descriptor alone because the edges are now
  generation facts installed before any task runs. The components were the half
  left unconverted — they still called `capability_import()` on a reply that
  carries nothing. Making the broker export again would re-introduce the runtime
  minting this milestone exists to remove.
- Rejected alternative: hardcoding the broker's route slots. They are derived
  from `FABRIC_CLIENTS` and `FABRIC_SUPERVISION` instead, so adding a
  participant renumbers them with the manifest — R2's rule, applied where the
  collision actually bit.

## Open risks and follow-ups

- [x] **Shared control endpoint — resolved, and the earlier framing was wrong.**
      `fabric-subscriber-b` multiplexes two routes over one control endpoint
      with two sequential readers, and a receive is destructive, so whichever
      loop ran consumed the other route's terminal event. This entry first
      recorded that as needing either separate endpoints per route (a fixture
      change) or a dispatching reader, and deferred it as a design decision.
      Checking the wire settled it instead: `WireStreamEvent`, `WireQosEvent`,
      and `WireSampleDescriptor` all carry `type_identity`, and the broker
      already stamps every record with its route's tag. The information needed
      to demultiplex was on the endpoint the whole time, so the two `static mut`
      flags were reconstructing by hand what the record already said. One reader
      owns the endpoint and files each record under the route it names; a route
      with a pending record does not touch the endpoint at all. No contract, no
      fixture, and no slot numbering changed.
- [x] **Two non-blocking peers never rendezvous — resolved.** Demultiplexing
      alone did not close the plane: the fabric announces QoS and terminal
      events with `seL4_NBSend`, which delivers *only* to a receiver already
      blocked on the endpoint and discards otherwise, while both subscribers
      polled with a non-blocking `recv` and slept on a ring notification. The
      sender faithfully re-offered forever and the reader was never once
      visible. Each loop now blocks on the control endpoint after draining its
      ring, which is exactly when it has nothing else to wait on. This is the
      hazard `try_send` carries by construction, and it is why the primitive is
      only correct for traffic a peer is genuinely waiting on.
- [x] **A same-route ordering case the mailbox cannot hold.** The telemetry loss
      is reported *during* the stall that causes it, to the loop that caused it,
      while the reader that must account for it has not started — same route, so
      one slot cannot hold it for that later reader. Returned as a value
      (`EarlyLoss`) rather than hidden in another flag.
- [x] **Native peer death — resolved.** The broker detected the proxy's exit by
      waiting for `ERR_PEER_DEAD` on its control endpoint, and no such signal
      exists on a native Endpoint: `ERR_PEER_DEAD` is a logical-channel concept
      and `sel4_transport::receive_native` cannot produce it, so a dead peer is
      indistinguishable from a silent one. Resolved the way the model already
      answers this question — a **supervision handle**, which is what
      `spawn-service` and `init::wait_clean` already use. The builder now grants
      one for every declared interposition proxy, not only ring holders, on the
      same reasoning the B46 comment gives for publishers; init spawns the proxy
      before the fabric so the handle can exist; and the broker's three waits go
      through one `await_exit` helper.
- [ ] `just contracts_check` fails on `sel4-dango`
      (`minted binding dango-echo-stdin-send: holder is not owned by its
      minter`). Confirmed pre-existing by stashing all changes and re-running.
- [ ] `just sel4_boot_layout_check` and `just sel4_supervision_check` fail at
      baseline too; both belong to the cutover's unfinished half.
- [ ] R2 (auto-allocating declared slot numbers) remains untouched and is still
      B50's exit condition. This entry's fixture work is a manual instance of
      exactly the drift R2 exists to remove.

## Artifacts and provenance

- Focused report: this entry
- Raw transcript: [`occupancy-audit.log`](occupancy-audit.log) — the pool sweep,
  the `still-occupied` release, and the minted-slot origin line
- Serial/debugger/model output: [`visibility-boot.log`](visibility-boot.log) —
  the visibility plane booting its whole graph after the fixture change
- Stream-plane transcript: [`stream-progress.log`](stream-progress.log) — the
  whole plane running to `[fabric] stream plane complete`, with `QoS peer dead`
  observed and terminal accounting clean, against a baseline that stalled at 12
  component markers and no clean exits
- Related roadmap item: `roadmap/00-backlog.md` B46 (open), B50
