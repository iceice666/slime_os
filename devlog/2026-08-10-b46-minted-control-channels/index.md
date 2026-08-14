# B46 — a declared grant and a minted one at the same slot

| Field | Value |
|---|---|
| Date | 2026-08-10 |
| Kind | Defect |
| Status | Verified |
| Scope | `contracts/generation/v1/fixtures/sel4-{call,operation}.zti`, `slime-root/src/shared_buffer.rs`, `slime-root/src/main.rs`, `scripts/check/check-sel4-{call,stream,qos}-plane.py` |
| Roadmap | B46 |
| Gates | `just sel4_call_check`, `just sel4_operation_check`, `just sel4_stream_check`, `just sel4_qos_check` |
| Trigger | The call and operation planes deadlocked with every participant parked; stream and QoS faulted inside the root. |
| Baseline | Three of B46's seven gates passing. |

## Summary

All seven of B46's named gates pass. Two unrelated causes stood between the
planes and their scenarios: a 144 KiB `ActionList` built in a stack frame, which
overflowed the root's stack during a loan teardown, and a fixture that declared
runtime-minted control channels as ordinary grants, so the root pre-created a
rival channel at every slot the minted one was meant to occupy.

## Observable symptom

- Command: `just sel4_call_check`, `just sel4_operation_check`.
- Expected: the plane runs its bounded-call scenario.
- Observed: every participant parked, init parked, no progress. The broker had
  consumed exactly one client request out of three sent.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | Stream faulted at `0x2a8500` with every stack slot `INVALID` | Symbolized to `build_actions`, not the aliasing path the prior entry guessed |
| 2 | `ActionList` measured 147,464 bytes | Built as a local and returned by value: two copies per teardown on a 1 MiB stack |
| 3 | Call plane: three sends, one receive | The broker was reading a different channel than init was writing |
| 4 | Fabric held declared ends at slots 2–5; init minted keys 4–9 | Two disjoint channel sets at the same slot numbers |
| 5 | `minted = true` exists on `Grant` for exactly this | Added by B39; the fixtures never used it |

## Root cause

**The stack overflow.** `MAX_TEARDOWN_ACTIONS` is `MAX_MAPPING_PAGES +
MAX_FRAME_ANCHORS * 2` = 4,608 slots of `Option<AdapterAction>`. The constant's
own comment said the bound was "deliberately independent of stack growth"; the
array was a local, and every by-value return stacked a second copy.

**The rival channels.** A send/recv grant normally materializes one pre-created
channel whose two ends the root installs at admission. The call and operation
planes do not want that: init calls `endpoint_create` and hands each side out
at spawn. Declaring those edges as ordinary grants meant the root built its own
channel per edge and installed the ends at the slots the broker reads. The
broker consumed a request off a declared channel, then blocked in
`consume_supervision` for a handle init had sent over a minted one.

## Changes

- `ActionList` is heap-allocated (`ActionList::boxed`), and `execute_teardown`
  takes it by value so the return moves a pointer. The root's heap is sized for
  two live lists.
- Nine control grants in `sel4-call.zti` and `sel4-operation.zti` carry
  `minted = true`; their eighteen bindings moved to `mintedBindings`.
- Four gate assertions that had never been reached: a stale `grants=13`, prose
  spliced into a marker pattern, `narrowed transfer role cannot widen` and
  `bounded time advanced`, neither of which any component emits.
- Two per-component failure budgets written for the P5.2 launch model, where
  the root launched every declared instance.

## Regression guards

- `a_boxed_list_is_empty_in_every_slot` walks all 4,608 slots. It caught my
  first `boxed`, which used `alloc_zeroed` on the assumption that `None` is the
  all-zero pattern for `Option<AdapterAction>` — it is not.
- The call and operation gates now assert the full scenario: 50 markers across
  10 causal chains, and 53 across 12, with every participant exiting cleanly.

## Verification

| Check | Result |
|---|---|
| `just sel4_call_check` | pass (was red) |
| `just sel4_operation_check` | pass (was red) |
| `just sel4_stream_check` | pass (was red) |
| `just sel4_qos_check` | pass (was red) |
| `just sel4_channel_check`, `sel4_crossing_check`, `sel4_visibility_check` | pass |
| Nineteen further plane gates | pass |
| `cargo test -p slime-root --lib` | 144 passed |
| `just contracts_check`, `just test_host`, `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos` | clean |

## Decisions

**`minted = true` rather than deleting the grants.** The edge, its rights, and
its two endpoints are real declarations that the root checks; only the object
is created at runtime. Removing the grants would have made the plane's control
topology undeclared, which is what B39 added the flag to avoid.

**The behavioural gates are green before the deletion, deliberately.** B46's
remaining half is replacing `channel.rs`, `transit.rs`, and `parked.rs` with
native Endpoint IPC and a `fabric-stream/v2` ring. These gates are what will
show that cutover preserved backpressure, bounded queues, timeouts, peer death,
and cap-transfer attenuation rather than merely compiling.

## Open risks and follow-ups

- The deletion itself: 1,912 lines, 41 call sites in `main.rs`, `WaitSet`
  threaded through four more modules, and six universal labels.
- `sel4_sample_check` was unblocked by the stack fix and now fails a real
  assertion, recorded as B51.
- Nothing checks that a fixture's control slots agree with the tuples in
  `build-generation.py` that the fabric resolves callers against. Only the
  visibility plane would catch a mismatch, and only because it has an intruder.

## Artifacts and provenance

- Commits: `f8922e2` (the stack fix), `4de4d92` (the minted controls).
- Every "was red" claim was verified by running the gate, not inferred.
