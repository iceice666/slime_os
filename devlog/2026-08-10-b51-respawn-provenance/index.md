# B51 — a collected instance is not a new one

| Field | Value |
|---|---|
| Date | 2026-08-10 |
| Kind | Defect |
| Status | Verified |
| Scope | `slime-root/src/{channel,main}.rs`, `components/bins/src/bin/sample-receiver.rs`, `scripts/check/check-sel4-sample-plane.py` |
| Roadmap | B51 |
| Gates | `just sel4_sample_check`, `just sel4_spawn_check`, `just sel4_reclamation_check`, `just sel4_component_graph_check` |
| Trigger | `sel4_sample_check` failed at `[init] sample plane fail: budget did not recover after a child exited`. |
| Baseline | The gate was red before this backlog run; the stack fix in B46 unblocked it into this assertion. |

## Summary

The spawn preflight checked a request against the child instance's declared
grants, which assumes one spawn per declaration. A respawn — the same instance,
after the first died and was collected — is that declaration launched again,
and the root had no way to say so: `task_for_instance` answers liveness and
`release_by_task` clears it. The sample plane's third spawn deliberately brings
nothing, because the point is whether the *ceiling* admits it, so it was
refused on the count before the budget was consulted.

## Observable symptom

- Command: `just sel4_sample_check`
- Expected: the plane's third spawn is admitted, proving the budget released
  its dead.
- Observed: `SLIME_GRAPH spawn preflight instance=sample-receiver
  reason=declared-count requested=0 bindings=0 minted=1`, then
  `SLIME_GRAPH spawn refused task=0 slot=2 ungranted`.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `requested=0 bindings=0 minted=1` | The declaration is right; the request is deliberately empty |
| 2 | `release_by_task` takes the entry | Liveness and provenance were the same question |
| 3 | Granting the declared capability on the respawn | Child blocks on a `recv` for a lender that already exited; `required` health makes that fatal |
| 4 | Declaration matching is positional | A *partial* request would bind the caller's first capability to another declaration's slot |

## Root cause

`LaunchedInstances::entries` answered both "is this instance live" and, by
absence, "has it ever run". Collection clears the entry, which is correct for
liveness and destroys the second answer.

## Changes

- `LaunchedInstances` keeps a `launched_once` bitmap, sized `MAX_INSTANCES`,
  set on `record` and never cleared by `release_by_task`.
- The preflight admits a respawn with an **empty** grant set. Every other
  request, first launch or respawn, is checked exactly as before.
- `sample-receiver` exits 0 when its peer slot holds nothing.
- Three gate assertions corrected: init's shared-buffer factory pinned at slot
  14 where the fixture declares 4, `quota` markers naming a `component=` field
  that is now `instance=`/`executable=`, and B14's budget probe.

## Regression guards

- `collecting_a_task_does_not_unlaunch_its_instance` asserts the two questions
  stay separate, and checks neighbouring indices so a single global flag would
  not satisfy it. Verified by clearing the bit in `release_by_task` and
  observing the failure.
- `sel4_sample_check` itself refuses a partial respawn, verified by making the
  plane's third spawn carry one grant.

## Verification

| Check | Result |
|---|---|
| `just sel4_sample_check` | pass (was red) |
| `just sel4_spawn_check` | pass |
| `just sel4_reclamation_check` | pass |
| `just sel4_component_graph_check` | pass |
| Twenty-three further plane gates | pass |
| `cargo test -p slime-root --lib` | 145 passed |
| `just contracts_check`, `just test_host`, `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos` | clean |

## Decisions

**Empty, not fewer.** The first shape I wrote allowed a respawn to bring *at
most* the declared count. That is unsound: declaration matching is positional —
request N binds to the declaration with the Nth-lowest destination slot — so a
partial request installs the caller's first capability at some other
declaration's slot, under that declaration's rights ceiling, with no error. An
empty request has no such ambiguity and a full one is checked as a first launch
is, so those are the only two shapes admitted.

**The bitmap, not a generation field.** B51's own text suggested declaring a
restart policy on the instance, which is closer to what B47 wants. That is the
better long-term answer and a larger change; the bitmap answers the question
the preflight actually asks — has this instance run — without inventing a
contract field that B47 may shape differently.

**`sample-receiver` exits 0 on an empty slot.** It is `required`, so the
throwaway retry's deliberate emptiness was fatal. A component with no channel
has nothing to verify, and saying so is not a failure. The distinction is
`ERR_BAD_CAP` from an empty slot versus a real receive error, which the
component already had the imports to make.

## Open risks and follow-ups

- A respawn brings nothing, so an instance that needs its capabilities back
  cannot currently be restarted usefully. The sample plane does not need that;
  a supervisor restart policy would, and belongs with B47.
- Nothing checks that the positional declaration order a caller assumes matches
  the one the root computes. The rights ceiling catches a mismatch only when
  the two declarations differ in rights.

## Artifacts and provenance

- Commit: `210c84f`.
- The "was red" claim was verified by running the gate before the change, not
  inferred.
