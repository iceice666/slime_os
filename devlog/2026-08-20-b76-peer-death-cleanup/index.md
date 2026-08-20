# B76: `IpcError::PeerDead` had no producer, the call clock's death was inferred from the wrong task, and removing the endpoint arm exposed a real parking deadlock

| Field | Value |
|---|---|
| Date | 2026-08-20 |
| Kind | Defect |
| Status | Fixed |
| Scope | `slime-root/src/ipc.rs`, `components/bins/src/{call_broker,operation_broker,matrix_broker,visibility_broker,fabric_call_scenario,fabric_operation_scenario}.rs`, `components/bins/src/bin/{fabric-call-worker,fabric-call-time,fabric-op-time,fabric-service,fabric-subscriber,echo-agent,powerbox-chooser,sel4-filesystem-service}.rs`, `components/bins/src/bin/init.rs`, `contracts/generation/v1/fixtures/{sel4-call,sel4-boot,sel4-traffic}.zti`, `scripts/check/check-sel4-call-plane.py` |
| Roadmap | B76 |
| Gates | `just sel4_call_check`, `just sel4_operation_check`, `just sel4_fabric_aggregate_check`, `just sel4_boot_check`, `just sel4_traffic_check`, `just sel4_boot_layout_check`, `just generation_check`, `just test_sel4_root` |
| Trigger | Found during B75's `pump_time` audit and recorded as B76; picked up as the next open backlog item after B75/B74 closed |
| Baseline | B75 fixed the call-broker wedge and the publisher-death race on `da4e207`; both fixes already used supervision as the sole death signal, but left `IpcError::PeerDead` and its ~43 consuming arms in place as unreachable redundancy |

## Summary

`IpcError::PeerDead` was declared in `slime-root/src/ipc.rs` and mapped to
`ERR_PEER_DEAD`, but no root path ever constructed it: a native seL4 Endpoint
has no closed-peer signal, so `slime_rt::recv` can never answer it. Forty-three
match arms across fourteen files branched on a status nothing could produce.
Two of them were load-bearing rather than merely redundant: `call_broker`
inferred its clock's death from the *server's* supervision handle, a
separately declared instance whose death says nothing about the clock's, and
`operation_broker` set a `time_closed` self-latch that nothing outside
`pump_time` ever read. The exit condition offered removing the variant and
every unreachable arm, or giving `PeerDead` a real producer; the latter is
architecturally impossible on this transport, so the fix removes the variant,
gives the call-plane clock its own supervision handle, and records a decision
that the operation-plane clock stays unsupervised. Removing the arms also
removed a park path the multi-lens review round found genuinely reachable: a
notification-based wait with no remaining live peer to signal it. Fixed by
refusing to park in that state instead. All listed gates pass; the fabric
aggregate determinism gate still produces byte-identical traces.

## Observable symptom

- Command: `grep -rn 'IpcError::PeerDead' slime-root/src/`
- Expected: at least one construction site, since a status the ABI declares
  and three brokers branch on should be reachable.
- Observed: exactly two occurrences, both in `ipc.rs` — the declaration at
  the old `:91` and the `slime_status` mapping arm at the old `:129`. No
  construction site anywhere in `slime-root`.
- Exit/fault/serial evidence: none — this is a static-code defect, not a
  runtime fault. The fix's own review pass found a *second*, previously
  unobserved defect with real runtime evidence (see Investigation log).

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | Repo-wide grep for `PeerDead`/`ERR_PEER_DEAD` confirmed the backlog item's census: zero constructors in `slime-root`, 43 consuming arms across 14 files, all traceable to `slime_rt::recv`/`send`/`try_send` on a native Endpoint | Branch (a) (give `PeerDead` a producer) is not a design choice available on this transport; only branch (b) is reachable |
| 2 | `call_broker.rs`'s `observe_server_death` closed `time_closed` from `self.supervision[2]` (the server's slot) with a doc comment claiming "the server's task hosts this plane's clock"; a sibling doc comment on `retire_server` said the opposite ("they are separate declared instances") | The two comments could not both be right; the fixture settles it — `fabric-call-time` is its own instance with its own executable, so the contradiction was real, not just confusing prose |
| 3 | `operation_broker.rs`'s `time_closed` was written only by its own unreachable `ERR_PEER_DEAD` arm and read only by `pump_time`'s own early return, never by `finished()` | A pure write-then-self-read latch: removing it changes no observable behavior, confirming it was inert |
| 4 | Added a fourth supervision slot for the call-plane clock, updated both broker hosts (`fabric-service.rs`'s inline `[6,7,8,9]`, `fabric-call-worker.rs`'s new `TIME_SUPERVISION_SLOT`), all three fixtures, and reordered `init.rs` to spawn the clock before its supervisor; `just sel4_call_check` passed on the first run after fixing the frozen spawn-order tuple and the grant-count assertion | The mechanical wiring was correct; the marker updates were the only friction |
| 5 | Five parallel review lenses ran on the diff; four returned clean, one (concurrency) reported P1, confidence 0.97: after both clients and the server settle, the broker can call `notification_wait` on a notification only a *live* peer would ever signal, and the clock's own death signals nothing | Read as a possible false positive at first, since `just sel4_call_check` had already passed |
| 6 | Captured the raw serial transcript of a passing `sel4_call_check` run and grepped it for ordering: `SLIME_GRAPH component exit task=1` (client A) at transcript line 1064, `[fabric-call-time] bounded time completed` at line 1071, `[fabric] call state reclaimed` (broker flush) only at line 1139 | The broker did *not* hang in this run, but the finish happened well after every peer had already exited — consistent with either a lucky scheduling window or a structural guarantee, and the transcript alone could not distinguish them |
| 7 | Traced the exact signal path: `fabric-call-client.rs`'s last action is `signal_phase(3)` to the clock's *own* phase endpoint (not the broker's wake notification), then the task exits; `fabric-call-time.rs` receives phase 3, debug-writes, and exits without ever calling `send_time`/`signal_wake` again | Confirmed no peer signals the broker's wake notification after the clock's final `send_time` (its second time advance); the only thing that can still make the broker re-poll after that point is either a leftover unconsumed signal bit or the broker never having reached the park call yet — neither is a guarantee |
| 8 | Read `run()`'s park condition: it refuses to park while a terminal is owed or the server is mid-reply, for exactly the same reason (a peer blocked in `recv` signals nothing) — but had no equivalent guard for "no live peer remains to ever signal again" | The existing pattern already solves this class of problem elsewhere in the same function; the clock case was the one instance left unguarded |

## Root cause

Two independent mechanisms, one static and one dynamic.

**Static:** `IpcError::PeerDead` existed because early design assumed a
mechanism symmetrical to `ERR_WOULDBLOCK` — that the transport could report a
closed peer the way it reports backpressure. A native seL4 Endpoint cannot: a
`send`/`recv` against a peer that has exited without closing the endpoint
capability itself simply never completes and never errors. Every plane
learned this and switched to supervision-based detection (client/server
handles were already correct); the clock was the one case where a task
inherited its liveness signal from an unrelated sibling task's handle instead
of getting its own, and the dead `ERR_PEER_DEAD` arms were never removed
after the real detection mechanism landed, so they read as working redundancy.

**Dynamic:** `call_broker::run`'s park decision assumed that "no progress this
sweep" implies "some live peer will eventually signal the wake notification."
That holds for every state except one: both clients and the server settled,
clock still alive. In that state the clock is the *only* peer left who could
ever signal again, and its death — the one fact still outstanding — is
exactly the fact a native Endpoint cannot signal. The existing `owed` and
`server_call.is_some()` guards in the same function already encode this
principle for terminals and in-flight server replies; the clock was not
covered by either.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `slime-root/src/ipc.rs` | Removed the `PeerDead` variant and its `slime_status` mapping arm | A declared enum variant has a real producer, or does not exist |
| `contracts/generation/v1/fixtures/{sel4-call,sel4-boot,sel4-traffic}.zti` | Added a fourth `capabilityKind = "supervision"` `mintedBindings` entry (`fabric-call-time-supervision`, slot 9) in each fixture's call-plane broker/worker holder | The call-plane clock has its own death signal, not a borrowed one |
| `components/bins/src/bin/init.rs` | Spawn the call-plane clock before its broker/worker (was after) and grant its supervision handle alongside the three participants' | A supervision handle is granted only after the task it names exists |
| `components/bins/src/call_broker.rs` | New `CLOCK_SUPERVISION` index; `pump_time` reads it via latch-then-drain (mirrors B75's publisher-death ordering) instead of a dead `ERR_PEER_DEAD` arm; `Err(_)` from the handle now fails loudly instead of silently latching a termination; `observe_server_death` no longer closes the clock; `run()` refuses to park once both clients and the server are gone and the clock is not yet closed; removed 11 unreachable arms and the now-dead `drop_dead_client` helper | The clock's death is observed from its own handle, and the broker can always reach that observation |
| `components/bins/src/operation_broker.rs` | Removed the inert `time_closed` self-latch and its unreachable arm; the clock's lack of supervision coverage is recorded as a deliberate decision (see Decisions) rather than left silent | A field with no reader does not exist; an intentional gap is documented, not implicit |
| `components/bins/src/{matrix_broker,visibility_broker,fabric_call_scenario,fabric_operation_scenario}.rs`, eight `bin/` components | Removed the remaining ~30 unreachable `ERR_PEER_DEAD` arms, each replaced with a comment naming the real detection mechanism; `matrix_broker.rs`'s `Served::Gone` variant removed as it became unreachable once its only producer was gone | No branch in the tree can be reached only by a status the transport cannot produce |
| `scripts/check/check-sel4-call-plane.py` | Updated the frozen spawn-order tuple, the grant-count assertion (4→5), and the causal-chain marker sequence for the clock's new pre-broker spawn position and fourth grant | The gate's frozen evidence matches the composition it boots |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The call-plane broker parks forever once every peer that could signal it is gone | `just sel4_call_check` | `all five spawned tasks exited cleanly` line absent; the run hits its watchdog instead |
| A dead `ERR_PEER_DEAD` arm reappears and gets treated as reachable again | `just sel4_call_check`, `just sel4_operation_check`, `just sel4_fabric_aggregate_check` | Any of the three plane gates that exercise peer death via supervision (`[fabric] call peer death propagated`, `[fabric] operation peer death propagated`) would need to keep passing without ever exercising the removed arms; a future re-add with no producer is inert by construction, not caught mechanically |
| The clock's supervision slot drifts out of sync between the two broker hosts or the three fixtures | `just sel4_call_check`, `just generation_check` (`validate_supervision_binding_names`) | Boot-time refusal naming a malformed or duplicate supervision binding |
| The call-plane clock's Err(_) supervision path silently latches instead of failing | `just sel4_call_check` | No direct negative-control gate exercises a broken clock handle; guarded by code review discipline matching `observe_server_death`'s existing `fail()` precedent |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_call_check` | `seL4 call plane check: init minted authenticated control pairs, delivered four post-spawn supervision introductions, every C8.6 bounded-call arm ran with the unmodified participants, and all five spawned tasks exited cleanly` | Direct |
| `just sel4_operation_check` | `seL4 operation plane check: init minted authenticated control pairs, vouched for four participant identities, every C8.7 bounded-operation arm ran with the unmodified broker and participants, and all six spawned tasks exited cleanly` | Direct |
| `just sel4_fabric_aggregate_check` | `2 schedules over one declared composition each passed their own plane gate on two independent boots and produced 279 byte-identical semantic-trace records in total` | Direct |
| `python3 scripts/check/check-sel4-boot-plane.py` | `one generation launched every C8 role at once through a collision-free layout ... and the whole graph settled to idle without any participant exiting` | Direct |
| `python3 scripts/check/check-sel4-traffic-plane.py` | `the stream, call, and operation planes ran real traffic concurrently under one fixed schedule ... and every spawned task settled` | Direct |
| `python3 scripts/check/check-sel4-boot-layout.py` | `seL4 boot layout check: 25 plane layouts match their fixtures` (unchanged; the new supervision slot is not part of init's own frozen layout) | Direct |
| `python3 scripts/check/check-generation-determinism.py` | `two isolated builds forced the sel4 manifest and produced byte-identical generation.bin and boot-store.bin` | Direct |
| `python3 scripts/check/check-contracts.py`, `python3 scripts/generate/generate-syscall-abi-bindings.py --check` | Both pass unchanged; the syscall ABI schema itself was not touched (only `slime-root`'s consuming enum) | Direct |
| `just test_sel4_root` | `slime-root host tests: 131/131 across 15 modules` (unchanged from before this change) | Direct |
| `just lint_all` | Clean; `slime-components` and `slime-root` both clippy at `-D warnings` | Direct |
| `just machete`, `typos`, `cargo fmt --all --check` | Clean | Direct |
| Raw transcript trace of the pre-fix park-risk window | `SLIME_GRAPH component exit task=1` (client A) and `[fabric-call-time] bounded time completed` (clock exit) both precede `[fabric] call state reclaimed` (broker flush) by tens of transcript lines with no intervening peer activity, confirming the window the concurrency review flagged is genuinely reached in this scenario even though it did not hang before the fix | Direct |

## Decisions

- Decision: remove `IpcError::PeerDead` and every arm branching on it,
  rather than give it a producer.
- Rationale: a native seL4 Endpoint has no closed-peer signal; there is no
  code path in `slime-root/src/ipc.rs` or
  `components/runtime/src/syscall/sel4_transport.rs` that could ever
  construct one. Keeping the variant as documented-but-dead is what let it
  read as working redundancy in the first place, which the backlog item
  names as the mechanism behind B75's shipped defect.
- Rejected alternative: give `PeerDead` a real producer. Not available — the
  transport itself cannot detect it, only supervision can, and every plane
  already uses supervision for this.
- Decision: give the call-plane clock its own supervision handle, spawned
  and granted before its broker/worker.
- Rationale: `fabric-call-time` is a separately declared instance; inferring
  its death from the server's handle (the pre-fix behavior) meant a clock
  that outlived the server was unobserved and a clock that died while the
  server lived was misreported as the server dying too.
- Rejected alternative: leave the call-plane clock unsupervised, matching the
  operation plane. Rejected because call-plane closure gates the broker's
  *sole* trace-flush site through its exit predicate — an unsupervised clock
  there is a permanent hang risk, not a dropped marker.
- Decision: leave the operation-plane clock unsupervised, and document the
  gap instead of adding a fifth supervision handle.
- Rationale: `fabric-op-worker` already parks 9 of `MAX_WAIT_SOURCES = 9`
  wake sources with zero headroom (documented in its own header comment); a
  tenth would need either raising the kernel-declared bound or restructuring
  the worker's wait set, and `finished()` does not wait on time there — a
  dead operation clock silently drops expiry markers but does not hang the
  plane, and `just sel4_operation_check`'s timeout/expiry marker chain
  already fails visibly if that happens.
- Rejected alternative: add the fifth handle anyway. Rejected as
  disproportionate to a gate-visible-but-non-hanging gap, and blocked by the
  wait-source ceiling without a separate restructuring change.
- Decision: refuse to park in `call_broker::run` once both clients and the
  server are settled and the clock is not yet closed, rather than rely on
  any peer signaling the wake notification in that state.
- Rationale: found by the concurrency review lens, then confirmed by tracing
  the exact signal path — the clock's last `send_time` before this fix's own
  supervision-based closure was the last possible signal to the broker's
  wake notification, and its own exit (an orderly, scripted barrier
  completion in the test scenario, not a fault) produces none. The existing
  `owed`/`server_call.is_some()` guards already encode "do not park on a
  wake that cannot arrive" for two other cases; this is the third.
- Rejected alternative: leave it unfixed on the grounds that the observed
  transcript did not hang. Rejected because the transcript trace (step 6-7
  above) showed the risky window is genuinely reached, not structurally
  avoided, and correctness should not depend on exact scheduling luck on a
  single-core QEMU boot.

## Open risks and follow-ups

- [ ] The operation-plane clock remains unsupervised by design (see
      Decisions). If a future change makes `finished()` wait on time there,
      this gap becomes a hang risk symmetrical to the one this entry fixes
      for the call plane, and needs the same treatment plus a
      `MAX_WAIT_SOURCES` resolution.
- [ ] No negative-control gate exercises a broken/ungranted clock supervision
      slot on the call plane to prove the new `fail(b"call time supervision")`
      arm is reachable; it is exercised only by code-review discipline
      matching `observe_server_death`'s existing pattern.
- [ ] `docs/syscall-abi.md`'s `## Error model` section still documents
      `ERR_PEER_DEAD` as a live status constant (correct — the ABI-level
      constant survives; only `slime-root`'s consuming `IpcError` variant was
      removed) but the label-coverage gate `generate-syscall-abi-bindings.py
      --check` does not cover that section at all, so nothing mechanically
      re-verifies the error-model table stays in sync with `IpcError`'s
      variants if it drifts again in either direction.

## Artifacts and provenance

- Focused report: none; the investigation is captured in full in this entry.
- Raw transcript: none preserved as a sibling; the decisive excerpts are
  quoted inline above from a `sel4_call_check` boot of
  `build/slime-sel4-call.elf`.
- Serial/debugger/model output: none beyond the quoted gate output above.
- Related roadmap item: [B76](../../roadmap/00-backlog.md)
