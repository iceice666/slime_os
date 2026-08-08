# B28 — the QoS plane needed more root iterations, not a bug fix

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Defect |
| Status | Verified |
| Scope | `slime-root/src/main.rs`, `slime-root/src/channel.rs`, `slime-root/src/ipc.rs`, `scripts/check/check-sel4-qos-plane.py`, `Justfile`, `AGENTS.md` |
| Roadmap | B28, P5.4.5 |
| Gates | `just sel4_qos_check`, `just sel4_gate_control_check`, `just test_sel4_root` |
| Trigger | P5.4.5 could not be gated: the `sel4-qos` plane never reached `[init] fabric stream complete` |
| Baseline | `MAX_GRAPH_ITERATIONS = 512`; the plane wedging with `live=5 parked=4` after 132 logged operations |

## Summary

`MAX_GRAPH_ITERATIONS` was 512. The QoS plane needs more than 512 and fewer than
768. **That is the whole defect.** No wake was lost, no capability was stale, no
scheduler was inconsistent, and every component was correct.

It took roughly thirty excluded readings to establish, and the reason it resisted is
worth more than the fix: the symptom was a *deadlock signature* — every task parked,
the CPU idle — produced by a *budget* limit.

## Observable symptom

```
SLIME_ROOT FATAL SLIME_GRAPH FAIL graph iterations exhausted live=5 parked=4
```

No component reported a failure. The plane stopped after
`[fabric-publisher-b] simulated time advanced` and never printed its terminal marker.

## Investigation log

Condensed; the full sequence of refuted readings is in the backlog entry.

1. Read as a lost wake on `fabric-publisher` (task 10). Every root layer was
   instrumented and found correct: the wake is generated, not dropped, answered on
   a reply CSlot matching the park, and `send_reply` precedes `release_slot`.
2. Read as a kernel scheduler fault. Hand-rolled TCB reads reported five threads
   `Running` on one core — impossible, and the tell that the offsets were wrong.
   Typed reads gave a healthy kernel. **A baseline on the passing stream plane then
   showed `ksCurThread == ksIdleThread` with empty ready queues is the *healthy*
   terminal state**, retiring three earlier "faults" at once.
3. Read as a B25 symptom. The passing plane shows the same `fail: role reply` lines,
   so no.
4. **The turn: fixed a diagnostic defect.** The wedge `fatal!` fired *before* the
   owed-reply accounting, and `fatal!` does not return, so the path that most needed
   a diagnosis printed only counts. Added `wedged waiter` output plus
   `ChannelTable::registered_waits` and `Channel::waits_for`.
5. That immediately showed **task 10 is not in the parked set**. `live=5 parked=4`,
   and task 10 is the one live, unparked, unreclaimed task — so its `SYS_WAIT` was
   answered. Every reading built on "the root owes task 10 a reply" was void.
6. The kernel said `BlockedOnSend`: a call into the root that was never received.
7. Logging the loop's final passes gave `task=9 op=Recv` eleven times out of twelve.
8. Instrumenting the broker's own loop past 400 passes produced **zero** lines, and
   `serve_wait`'s ready-probe for task 9 produced **zero** lines. The broker parks
   legitimately, sixteen times, and is woken sixteen times by real sends.
9. Raising the bound to 4096 made the plane complete. Bisecting: **768 completes,
   512 does not.**

## Root cause

The budget. The QoS plane drives a simulated clock through scheduled deadline,
lifespan, liveliness, and retry boundaries; each boundary is a park/wake cycle for
the broker plus a sweep of every participant. That is legitimately more root
round-trips than any previous plane, and the constant had been sized against the
stream plane's 136.

## Changes

- `MAX_GRAPH_ITERATIONS` 512 → **2048**, with the measurement recorded in its doc
  comment: the stream plane's 136, the QoS floor of 768, and why the wedge detector
  still bounds a real livelock to seconds.
- The wedge arm now names each waiter and every channel it waits on, via two
  diagnostic-only scans. This is kept: it is what solved the defect.
- **New gate `just sel4_qos_check`** — nine causal chains, fourteen markers.

## Regression guards

The gate asserts every C8.5 arm and treats `SLIME_GRAPH wedged waiter` as a
**failure marker**, so a reappearance of this exact defect is red rather than a
missing arm. Registered in the `Justfile`, `AGENTS.md`'s gate index, and
`sel4_gate_control_check` (now 11 gates, 464 mutations) with its marker count pinned
at 14.

## Verification

`just sel4_qos_check` — "14 markers observed across 9 causal chains". Arms observed:
`QoS matched` ×5, `reliable retry accounted` ×4, `QoS retry exhausted`,
`QoS deadline missed`, `QoS lifespan expired`, `QoS liveliness lost` ×3,
`QoS peer dead` ×2, `simulated time advanced`, `served live=0`.

`QoS peer dead` firing twice is new — the retire path was unreachable before,
which is why an earlier session recorded it as a suspected defect.

All other gates re-run green: seven plane checks, root-boot, comp-graph, layout 9/9,
`test_sel4_root` 109/109, contracts, generation, fmt, lint, typos.

### Fault injection

| Injection | Result |
|---|---|
| restore `MAX_GRAPH_ITERATIONS = 512` | **fails** with `failure marker: 'SLIME_GRAPH wedged waiter'` |
| silence the broker's liveliness arm | **fails** with `missing marker: QoS liveliness lost` |

The first is the defect itself, caught by its own signature.

## Decisions

**2048, not 768.** The floor is 768; 2048 leaves room for a denser graph without
another archaeology session. The wedge detector still fails in seconds.

**Keep the diagnostic.** It cost one commit and solved a defect that had survived
thirty readings. The `wedged waiter` line is the difference between reporting a
deadlock and explaining one.

**Failure marker on `wedged waiter`, not just the missing arms.** A budget wedge
produces *no* component failure, so the arms alone would report "missing marker"
and send the next reader hunting the arm rather than the budget.

**Split the three boundary arms into separate chains.** Their order is a function
of the declared QoS tuples, not a causal sequence; grouping them asserted an
ordering the contract does not promise, and it failed for exactly that reason.

**Scoped the `fail:` failure marker to `[init] stream plane fail:`.** Six `fail:`
lines are *expected* on this plane — every participant proves a denial arm — and an
unscoped marker made the gate red on a correct boot.

## Open risks and follow-ups

The bound is still a constant sized by measurement. A plane denser than QoS would
need it raised again; the doc comment now records how to tell.

`[fabric] QoS lease renewed` and tie-ordering remain unasserted — the scenario does
not emit them on this fixture, so they are absent rather than failing.

## Artifacts and provenance

All observations from `just sel4_qos_check` and direct QEMU boots of
`build/slime-sel4-qos.elf` on `aarch64-apple-darwin` at this entry's date.
Injections were made by editing the named file, rebuilding the plane, running the
gate, and restoring from a copy — none remains in the tree.
