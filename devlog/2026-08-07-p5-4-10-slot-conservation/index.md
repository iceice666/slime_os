# P5.4.10 (part) — B9's conservation, in the shape seL4 can hold it

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Change |
| Status | Verified |
| Scope | `scripts/check/check-sel4-root-boot.py` |
| Roadmap | P5.4.10, P5.4, P5.4.1, B9 |
| Gates | `just sel4_root_boot_check` |
| Trigger | P5.4.10's remaining rows, worked in order |
| Baseline | Reclamation counts matched as `slots=\d+`; three rows open |

## Summary

`kernel/tests/task_reclamation.rs` carries B9's three properties: a task that
goes away returns every frame it consumed, measured as *drift* across
spawn/release cycles rather than absolute equality. The seL4 root already
reclaimed and reported per-task CSlot ranges, but every count in
`sel4_root_boot_check` was a `\d+` wildcard — a task reclaiming half its slots
produced a passing transcript. The ranges are now pinned exactly, which is the
strongest form of the property this allocator can hold.

## Changes

| Area | Change | Effect |
|---|---|---|
| `check-sel4-root-boot.py` | Each task's reclaimed range pinned: `slots=832..882` and `slots=882..932` | Contiguous, equal-width, adjoining — a short or overlapping reclaim fails |
| `check-sel4-root-boot.py` | `cleanup … slots=100` and `READY … reclaimed_slots=100` pinned | The aggregate must agree with the per-task ranges |
| `check-sel4-root-boot.py` | Reclaim markers interleaved with the settle markers | The list is order-sensitive and each task is reclaimed as it settles |

No root change: the accounting already existed and was simply unasserted.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A task reclaims fewer slots than it took | `just sel4_root_boot_check` | `reclaimed_slots=98` against a pinned `100`; gate exits 1 |
| Two tasks' ranges overlap, so one revokes the other's slots | same | The second range no longer starts where the first ended |
| A task is not reclaimed at all | same | `cleanup tasks=2 slots=100` fails |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_root_boot_check` | Pass — [`slot-accounting.log`](slot-accounting.log) | Direct |
| Fault injection: `CleanupRecord::revoke` returns `slot_count() - 1` | `reclaimed_slots=98`, gate exits 1 — [`slot-accounting.log`](slot-accounting.log) | Direct |
| The other nine seL4 gates | Pass, unchanged | Direct |

## Decisions

- Decision: assert **accounting**, not **conservation across cycles**.
- Rationale: B9's drift measurement is not available here, and pretending
  otherwise would be the more dangerous outcome. `CleanupRecord::revoke` says so
  directly — "Root CSlots are not returned to the allocator: they are recorded
  as reclaimed so accounting stays monotonic and auditable." A free-count
  comparison would therefore be flat by construction and would pass whether or
  not reclamation ran. The same monotonic allocator was recorded in B24's
  amended exit condition, for the same reason. What *is* checkable is that every
  slot a task took is revoked, deleted, and counted, which the pinned adjoining
  ranges establish.

- Decision: exact ranges rather than a width check.
- Rationale: `slots=832..882` and `slots=882..932` encode three properties in
  two literals — each task took 50, they do not overlap, and there is no gap. A
  computed width check would express less and read as more.

## Open risks and follow-ups

- [ ] **The absolute slot numbers are now frozen in a gate.** Any change to the
      root's allocation order shifts 832 and moves both ranges; the gate will
      fail loudly and the new numbers must be read from a boot rather than
      guessed. That is the intended cost of pinning, but it is a real
      maintenance edge and the failure will look alarming when it is benign.
- [ ] **Two P5.4.10 rows remain**: C8.1 collision rejection and C8.3 graph
      provenance.
- [ ] The live counterpart the oracle names — `just spawn_service_check`, where
      real components spawn and exit through the reaper — has its seL4 analogue
      in `sel4_spawn_check`, which asserts reclamation happened but not its
      width. Left as-is: the root-boot plane is where the exact numbers are
      stable, since the spawn plane's counts depend on how many children the
      scenario happens to create.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [`slot-accounting.log`](slot-accounting.log).
- Related roadmap item:
  [P5.4.10](../../roadmap/07-architecture-portability.md),
  [B9](../../roadmap/00-backlog.md),
  [B24](../../roadmap/00-backlog.md) (which recorded the same monotonic-CSlot
  property from the other direction).
