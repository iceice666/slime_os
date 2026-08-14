# B25 (part) — a second supervision handle for one task

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/{ipc.rs,main.rs}`, `components/runtime/src/syscall{.rs,/legacy.rs,/sel4_transport.rs}`, `components/runtime/src/lib.rs`, `components/bins/src/bin/init.rs`, `scripts/check/check-sel4-supervision-plane.py`, `scripts/check/check-sel4-gate-controls.py` |
| Roadmap | B25, P5.4.6, P5.4 |
| Gates | `just sel4_supervision_check`, `just sel4_gate_control_check`, `just test_sel4_root`, `just lint_all`, `just fmt_check_all` |
| Trigger | B25's own proposed-fix section records a third option "cheaper than either and worth weighing first"; it was the only part of B25 implementable without deciding the move/copy question |
| Baseline | Each spawn returns exactly one supervision handle; no operation produces a second |

## Summary

`supervision_derive` (operation 32) lets a component holding `RIGHT_SUPERVISE` on
a supervision handle obtain a **second** capability naming the same task, at the
same rights, in a fresh slot, keeping the source.

This removes one of B25's two blocking reasons. It does **not** close B25.

## Changes

Root: `Operation::SupervisionDerive = 32`, wired through `from_label`,
`mediation`, and `MAX_OPERATION_LABEL`; handler `serve_supervision_derive`
resolves the source with `RIGHT_SUPERVISE`, installs a copy from slot 1 upward,
and returns the new slot in the reply's auxiliary word.

Components: the ABI is mirrored in **both** transports — `sel4_transport.rs` and
`legacy.rs` — plus the `slime_rt::supervision_derive` wrapper and its re-export.
Missing the legacy arm is what `just lint_all` caught; the x86 oracle shares this
runtime.

## Regression guards

Two markers in `check-sel4-supervision-plane.py` — the root's
`SLIME_GRAPH supervision derived` line and init's
`[init] second supervision handle derived` — so the operation cannot regress
silently. The negative control's pinned required-marker count for that gate moved
9 → 11, which is what makes a *later* deletion of either marker a red gate rather
than a smaller number.

## Verification

Asserted on the supervision plane, the one plane where init holds a supervision
handle it has not yet given away:

```
SLIME_GRAPH supervision derived task=0 child=3 slot=5
[init] second supervision handle derived
[init] supervision plane complete
```

Init derives while still holding the source, queries the **derived** handle for the
already-terminated child's outcome, and then performs the pre-existing transit
transfer with the source — so the sequence proves the copy carries real authority
*and* leaves the original intact. Both markers are gated.

| Check | Result |
|---|---|
| `sel4_supervision_check` | passes with both new markers |
| 8 other plane gates, root-boot, comp-graph, layout | unaffected |
| `sel4_gate_control_check` | 441 mutations; pinned count 9 → 11 |
| `test_sel4_root` | 109/109 |
| `test_host` | 168 |
| `lint_all`, `fmt_check_all`, `ruff`, `typos`, `machete` | pass |

### Fault injection

| Injection | Result |
|---|---|
| return the source slot instead of a new one | `fail: derive returned the source slot` |
| install the derived handle with rights `0` | `fail: derived handle reported no outcome` |
| drop the `RIGHT_SUPERVISE` gate on the source | **not covered** — see below |

The third injection leaves the gate green, and that is recorded rather than
papered over: every caller on this plane holds `RIGHT_SUPERVISE`, so no fixture
exercises the refusal. Covering it needs a component holding a handle narrowed
past that right, which no current generation declares.

**The negative control earned its keep immediately.** Adding two markers made
`sel4_gate_control_check` fail on its pinned required-marker count, one turn after
that pin was introduced for exactly this reason.

**Return the slot in the auxiliary word.** Matches `endpoint_create`, which
already returns minted slots that way, rather than inventing a convention.

**Install from slot 1, not 0.** Slot 0 is the component's control endpoint and is
never handed out by the root; `serve_spawn`'s own handle install uses
`free_slot_from(1)` for the same reason.

**Assert on the supervision plane, not a new one.** That plane already exercises
handles in transit, retained across a crossing, and past the record bound. A new
image would have added a generation for one operation.

**Record the uncovered injection.** Two of three is the honest count.

## Decisions

**An operation rather than changing move/copy semantics.** The two semantic fixes B25 weighs both have large blast radii: making a
spawn-granted endpoint *copy* needs a holder model admitting two tasks per end,
because `ChannelTable` resolves queues by holder; making it *move* breaks the
oracle's own call plane as written. This third option touches neither.

**It widens nothing, by construction:**

* the result names the **same** task, so no new subject becomes reachable;
* its rights are the source's own, so no new verb becomes permitted;
* `RIGHT_SUPERVISE` on the source is required to ask — the same gate
  `serve_supervision_status` puts in front of a query.

So a caller can only mint a handle it could already have transferred.

**Reclamation needed no change**, which is worth stating because it is the
non-obvious part: `graph::holds_supervision` already scans every live table for
*any* holder rather than tracking one owner, because a handle has always been
movable. A second holder is the same shape as the first.

## Open risks and follow-ups

**B25 stays open.** This closes the *second* of its two reasons — init needing two
`RIGHT_SUPERVISE` handles naming the fabric, one per lender. The first stands: a
spawn-granted endpoint still moves on seL4 and copies on x86, and
`init.rs::launch_fabric_calls` still has to be restructured to the inverted spawn
order (participants first with the participant halves, fabric last with the
service halves) before the derive helps the call plane. That restructuring is now
a composition change rather than a model decision, which is the actual progress
here.

The `RIGHT_SUPERVISE` refusal path is ungated. A generation declaring a
non-supervising supervision grant would close it.

## Artifacts and provenance

Markers and injections observed by booting `build/slime-sel4-supervision.elf` under
the pinned QEMU line and by `just sel4_supervision_check` on
`aarch64-apple-darwin` at this entry's date. Injections were made by editing
`slime-root/src/main.rs`, rebuilding the plane, running the gate, and restoring
from a copy — none remains in the tree.
