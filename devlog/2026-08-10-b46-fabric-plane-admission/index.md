# B46 — four defect classes between the fabric planes and their scenarios

| Field | Value |
|---|---|
| Date | 2026-08-10 |
| Kind | Defect |
| Status | Root-caused |
| Scope | `slime-root/src/channel.rs`, `contracts/generation/v1/fixtures/sel4-{channel,crossing,call,operation,visibility,qos}.zti`, `scripts/check/check-sel4-{channel,crossing,component-graph,visibility}-plane.py` |
| Roadmap | B46 |
| Gates | `just sel4_channel_check`, `just sel4_crossing_check`, `just sel4_visibility_check`, `just sel4_component_graph_check` |
| Trigger | All seven of B46's named gates were red at the start of the item. |
| Baseline | `sel4_component_graph_check` green; the seven fabric and channel gates red. |

## Summary

Three of B46's seven gates now pass and were red: `sel4_channel_check`,
`sel4_crossing_check`, `sel4_visibility_check`. The other four boot, spawn
their whole participant set, and reach their own scenario logic instead of
being refused at admission. None of the four defect classes fixed here was the
cutover B46 describes — they were sitting in front of it, and every one of them
hid the next.

## Observable symptom

- Command: `just sel4_channel_check` and six siblings.
- Expected: each plane runs its scenario.
- Observed: refused at spawn admission, or a marker that could never match, or
  — in the visibility plane's case — a Python `IndexError` in the gate itself.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | Every fabric plane's `mintedBindings` was `[]` while init hands each child minted capabilities at spawn | The preflight, which expects `parent_supplied + minted`, refused before a scenario ran |
| 2 | Declared control bindings sat at slots 0.. over the fabric's `FACTORY_SLOT = 0` | Shifted by two; admission passed |
| 3 | The visibility plane then handed the *intruder* a populated route page | Slots had been numbered alphabetically; `fabric-service` identifies callers by arrival slot against `FABRIC_CLIENTS`, an explicit tuple |
| 4 | `SLIME_GRAPH channel end` was asserted four times and emitted nowhere | Emitted from the install path; three further assertions on that gate then proved stale |
| 5 | `check-sel4-visibility-plane.py` raised `IndexError: no such group` | Its `TERMINAL_MARKER` has no capture groups while `boot` calls `.group(1)` on it — dead code until the plane got far enough to exit cleanly |
| 6 | Channel plane: init exits, console blocks forever | A declared side reads as permanently held, so `mark_dead` finds nothing abandoned and wakes nobody |

## Root cause

Four independent causes, in the order they were unmasked:

1. **Undeclared minted run tokens** in six fixtures. The same omission the
   probe planes and `sel4-generation` carried.
2. **Control slots numbered by the wrong key.** `fabric-service` reads a
   caller's identity from the slot its request arrived on. That mapping is
   `FABRIC_CLIENTS`, generated from `FABRIC_STREAM_CONTROL_GRANTS` — an
   explicit tuple, not a sort. Numbering the fixture's bindings alphabetically
   put `fabric-intruder` in `fabric-publisher`'s slot, and the visibility
   broker duly answered the unauthorized caller as the publisher.
3. **Two gates asserting markers with no emitter**, and one gate that would
   raise before it could pass.
4. **A declared channel side is never abandonable.** `has_declared_side` made
   `mark_dead` treat it as live forever, so a holder's exit woke nobody and
   marked no queue dead.

## Changes

- `slime-root/src/channel.rs`: `DeclaredEndpoint` gains `installed` and
  `holder_exited`; `mark_dead` retires the dying task's declarations before
  looking for abandoned sides; the install path emits `SLIME_GRAPH channel
  end`.
- Six fixtures declare their minted run tokens; four number their control
  slots by the canonical tuples.
- `check-sel4-component-graph.py` asserts the two services' exits and
  `live=0 completed=3` rather than parks and `live=2 completed=1`.
- `check-sel4-crossing-plane.py` drops the `kernel=` field.
- `check-sel4-visibility-plane.py`'s terminal marker captures.

## Regression guards

- `sel4_channel_check` asserts each declared end's install slot, so a fixture
  and a component disagreeing about where an end lives now fails a gate
  instead of going quiet until the first send goes nowhere.
- `sel4_component_graph_check` asserts both services exit on peer death, which
  is what makes peer-death propagation observable at all.
- `sel4_visibility_check` is the only gate with an intruder, so it is the only
  one that catches control slots numbered against the wrong key.

## Verification

| Check | Result |
|---|---|
| `just sel4_channel_check` | pass (was red) |
| `just sel4_crossing_check` | pass (was red) |
| `just sel4_visibility_check` | pass (was red) — 25 markers, 7 causal chains |
| `just sel4_component_graph_check` | pass |
| Nineteen further plane gates | pass |
| `cargo test -p slime-root --lib` | 143 passed |
| `just contracts_check`, `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos` | clean |

## Decisions

**A declaration is retired on its holder's death, not at install.** I tried
install-time first and it broke `sel4_component_graph_check`, which led me to
record the two planes as an irreconcilable trade. That was wrong, and the error
was in the reasoning rather than the measurement: retiring at install also
destroys the exemption's real purpose, which is covering the window before a
declared end is placed. Keyed on the holder's own exit, both planes pass.

**The graph gate's expectations were the stale half.** Init holds the consumer
end of both declared channels and exits when its launch sequence is done, so
both services see `PeerDead` and exit 0 — which is exactly what `console.rs`
and `spawn-service.rs` are written to do, each having an `ERR_PEER_DEAD =>
exit(0)` arm that was previously unreachable. Asserting the exits makes
peer-death propagation observable; asserting parks asserted the bug.

**The descriptor itself is untouched.** A repeatable instance template still
installs from it. What changed is whether it is still *owed*.

## Open risks and follow-ups

- Stream and QoS fault inside the root during their scenarios — the same
  aliasing class `sel4_sample_check` shows. Call and operation stall with
  participants parked. All four are `channel.rs`/`transit.rs`/`parked.rs`
  behaviour, which B46 deletes rather than repairs.
- Four planes' control slots are now numbered against tuples in
  `build-generation.py` with nothing checking the two agree. A fixture that
  renumbers them silently reintroduces the visibility leak; only that plane's
  gate would catch it, and only because it has an intruder.

## Artifacts and provenance

- Commits: `bdd748e`, `9d6f930`, `6e2458a`, and the two fixture commits before
  them.
- Every "was red" claim was verified by stashing the work and re-running, not
  inferred.

## Corrections

**2026-08-10 — the stream and QoS faults were a stack overflow, not aliasing.**
This entry's open-risks section said stream and QoS "fault inside the root
during their scenarios — the same aliasing class `sel4_sample_check` shows".
That was a guess from the `frame aliased` lines immediately preceding the
fault, and it was wrong.

`ActionList` is 147,464 bytes: `MAX_MAPPING_PAGES + MAX_FRAME_ANCHORS * 2`
slots of `Option<AdapterAction>`, built as a local in `build_actions` and
returned by value, so each teardown put two copies on the root's 1 MiB stack
from an already-deep dispatch frame. Symbolizing the faulting PC against the
stream plane's own root ELF named `build_actions` directly, and every stack
slot in the dump read `INVALID`. The constant's own comment claimed the bound
was "deliberately independent of stack growth"; the array was a local.

The list is heap-allocated now and `execute_teardown` takes it by value so the
return moves a pointer rather than copying 144 KiB back through the caller.
`sel4_stream_check` and `sel4_qos_check` are green.

`sel4_sample_check` was also unblocked by the same change — it no longer
faults — but fails a real assertion behind it, recorded as B51: the spawn
preflight cannot distinguish a respawn from a first launch.

**The "four defect classes" count was low.** Two more of the same kind were
behind the fault: a stale `grants=13` admission count, prose spliced into a
marker pattern, a marker no component emits, and a failure budget written for
the P5.2 launch model. All four were on gates that had never run far enough to
reach them.
