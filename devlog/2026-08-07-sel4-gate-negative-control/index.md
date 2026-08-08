# The seL4 gates had no negative control

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Change |
| Status | Verified |
| Scope | `scripts/check/check-sel4-gate-controls.py`, `Justfile`, `AGENTS.md`, `.github/workflows/ci.yml` |
| Roadmap | P5.4.1, P5.4 |
| Gates | `just sel4_gate_control_check`, `just ruff`, `just typos` |
| Trigger | P5.4.1's own "Other risks" list recorded this as an open item; it was the only entry there closable without new hardware or a capability-model decision |
| Baseline | Ten marker-matching seL4 gates, nothing in-repo showing a missing marker makes one red |

## Summary

Every seL4 plane gate boots an image and asserts an ordered sequence of serial
markers. The oracle had `should_panic.rs` — proof that a failing assertion is
observable at all. The seL4 side had no equivalent, so the only thing standing
between a silently-vacuous gate and a real one was per-slice fault injection:
per-change discipline, not a standing guard.

`just sel4_gate_control_check` is that guard. Eight gates, 356 mutated
transcripts, all rejected.

## Changes

For each gate the control builds a synthetic transcript from the gate's **own**
`REQUIRED_MARKERS`, confirms the gate accepts it, then confirms the gate rejects:

* each required marker deleted, one at a time,
* the first two required markers transposed,
* each failure marker appended to an otherwise-complete transcript.

Building the transcript from the gate's own table is what keeps the control from
drifting out of step with what it guards. The markers are regexes, so
`literal_for` instantiates them — handling anchors, `\d+`, `[1-9]\d*`, hex
classes, alternations, lookarounds, and escaped literals — and **hard-errors on
any construct it does not understand** rather than skipping the marker. A control
that quietly stopped covering a pattern would be worse than none.

Each gate's required-marker count is **pinned** in `GATES`. That is the difference
between a control and a statistic: without the pin, deleting a marker from a gate
just makes this report a smaller number and still pass.

The gate's marker logic is driven directly rather than through `check_transcript`,
because that function also calls per-gate helpers like `check_queue_depth` which
read counters a synthetic transcript does not carry. Those are the gate asserting
things *about a real boot*, which is not what this control guards.

## Regression guards

Registered in the `Justfile`, in `AGENTS.md`'s canonical gate index, and in CI's
contracts job — it needs no build and no QEMU, so it runs anywhere the checkers
import.

## Verification

`just sel4_gate_control_check` — "8 gates reject 356 mutated transcripts".
`just ruff` and `just typos` pass.

Three fault injections, each reverted after being observed:

| Injection | Result |
|---|---|
| delete one `REQUIRED_MARKERS` entry from the channel gate | `declares 26 required markers, expected 27` |
| empty the channel gate's `FAILURE_MARKERS` | `no failure markers declared` |
| make the control's own ordering non-strict (search from 0) | `accepted a transcript missing 'console parked on an empty channel'` |

The first injection is the one that matters most, and it is why the pin exists: on
the first attempt — before the counts were pinned — deleting a marker left the
control **green**, merely reporting 355 instead of 356. The third injection is a
self-test: it weakens the control rather than a gate, and the control catches its
own weakening.

## Decisions

**Synthetic transcripts, deliberately.** A negative control must produce evidence
that is wrong in one specific way, which no real boot can be asked to do. The
tradeoff is stated plainly in the script's docstring: this asserts that the
gates' assertions have teeth, *not* that the markers are the right markers or
that a real boot emits them. The plane gates themselves cover that.

**Pin the counts.** See the first injection above. A derived count cannot detect
its own erosion.

**Two gates excluded, named rather than skipped.**
`check-sel4-stream-plane.py` composes its required set at runtime instead of
declaring one table, and `check-sel4-boot-layout.py` compares fixtures rather
than markers, so neither exposes the surface this control drives. Both are listed
in the script's `GATES` comment with the reason.

**Drive the marker logic directly.** Calling `check_transcript` would drag in
per-gate content assertions that a synthetic transcript cannot satisfy. The
four-line ordered-marker loop is copied instead, which also stops this control
from coupling to each gate's private helper set.

## Open risks and follow-ups

The two excluded gates have no negative control. The stream gate is the more
valuable of the two, and giving it one means having it declare its required set
as data rather than composing it inline — a refactor of that checker, not of this
control.

The control proves a gate rejects a transcript *missing* evidence. It does not
prove the gate's markers correspond to the behaviour their descriptions claim;
that remains per-slice review.

## Artifacts and provenance

`scripts/check/check-sel4-gate-controls.py`, run as
`just sel4_gate_control_check` on `aarch64-apple-darwin` at this entry's date.
Injections were made by editing `check-sel4-channel-plane.py` (or the control
itself), running the control, and restoring from a copy — no injection remains in
the tree.
