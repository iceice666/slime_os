# P5.4.10 (part) — C8.4's structural arm: the shape the generation fixed

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/{main,generation}.rs`, `scripts/check/check-sel4-stream-plane.py` |
| Roadmap | P5.4.10, P5.4, P5.4.1, C8.4 |
| Gates | `just sel4_stream_check` |
| Trigger | P5.4.10's remaining rows, worked in order |
| Baseline | `sel4_stream_check` asserting C8.4's live arm only; four rows open |

## Summary

`kernel/tests/fabric_stream.rs` describes itself as covering "the two things a
transcript cannot show", the first being that *the graph the boot admitted
really declares the fan-out*. Counting transcript markers proves samples moved;
only reading the authenticated resource proves they moved along edges the
generation fixed. `slime-root` already decoded the graph for C8.2's admission
but reported only whether one was present, so `sel4_stream_check` inherited the
live arm and none of the structural one. The admission marker now carries the
shape the graph declares, and the gate asserts it.

## Changes

| Area | Change | Effect |
|---|---|---|
| `generation.rs` | `fabric_graph_admission` returns `Option<FabricShape>` instead of `bool` | The counts exist where the graph is already decoded, with no second decode |
| `generation.rs` | `Admission` gains `fabric_{schemas,routes,participants,interpositions}` | The shape reaches the marker |
| `main.rs` | Marker extended to `fabric graph=admitted schemas=N routes=N participants=N interpositions=N` | The declared shape is observable |
| `check-sel4-stream-plane.py` | Asserts `schemas=2 routes=2 participants=6 interpositions=0` | A silently dropped participant fails the gate |

C8.4's *second* point — that declared bounds are ones this kernel can honour —
needed nothing: P5.4.4 wired `validate_against` into the same admission path,
against this root's real ceilings rather than a copy.

Also repaired the C8.2 marker's comment, which had lost two fragments
(`` `absent` for the generations that... ``) and read as a broken sentence.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A participant is dropped from the graph while its peers still transcribe correctly | `just sel4_stream_check` | `missing marker: SLIME_ROOT fabric graph=admitted … participants=6 …` |
| A route or schema is added or removed | same | same, on the changed count |
| An interposition hop appears where none was declared | same | `interpositions=` differs |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_stream_check` | Pass — [`shape-marker.log`](shape-marker.log) | Direct |
| Fault injection: one participant removed from route 1 of `sel4-stream.zti` | Marker reports `participants=5`; the gate rejects it — [`shape-marker.log`](shape-marker.log) | Direct |
| The other nine seL4 gates | All pass — they assert `fabric graph=absent`, unchanged by the added fields | Direct |
| `just test_sel4_root` | Pass, 107/107 | Direct |
| `just fmt_check_all`, `lint_all` | Pass | Direct |

The counts were **read from a boot, not derived from the fixture**. My first
draft asserted `interpositions=1` by inspection and the gate rejected it; the
real value is `0`, because every route in `sel4-stream.zti` declares
`interposition = []` and `fabric-intruder` is a denial probe rather than a hop.
Recorded because a fixture-derived number would have been wrong in a way that
still looked plausible.

## Decisions

- Decision: report the shape from the root; do not re-derive it in the checker.
- Rationale: the checker could parse the `.zti` and compute the same numbers,
  and it would then be asserting that the fixture agrees with itself. The
  question C8.4 asks is what the *booted, authenticated* resource declares,
  which only the root can answer.

- Decision: counts in the marker, not a per-participant dump.
- Rationale: `stream_authority_does_not_cross_routes` — the oracle's other
  structural test — is about grant identities, which the transcript already
  covers through the intruder's denial. Counts catch the case that arm cannot:
  a participant that vanishes without anyone denying anything.

## Open risks and follow-ups

- [ ] **Counts are weaker than the oracle's per-route census.** A graph that
      dropped one publisher and gained one subscriber would keep
      `participants=6`. The transcript's per-component markers make that
      specific swap visible, so the pair is sound, but neither half is
      sufficient alone.
- [ ] **Three P5.4.10 rows remain**: C8.1 collision rejection, C8.3 graph
      provenance, and `task_reclamation.rs`'s three properties.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [`shape-marker.log`](shape-marker.log).
- Related roadmap item:
  [P5.4.10](../../roadmap/07-architecture-portability.md),
  [C8.4](../../roadmap/02-core-runtime.md),
  [P5.4.4](../../roadmap/07-architecture-portability.md) (which wired the
  admission path this extends).

## Corrections

- **2026-08-07.** The Verification table's row "The other nine seL4 gates | All
  pass — they assert `fabric graph=absent`" states a false reason. **No gate
  asserted that marker at all**; grepping `SLIME_ROOT fabric` across
  `scripts/check/` matched only `check-sel4-stream-plane.py`. The other planes
  were unaffected because they assert nothing about the marker, not because
  they pin `absent`, and the row's "Direct" evidence label was therefore
  applied to a property that had not been observed. Found by an independent
  documentation review of this milestone.

  The gap the false reason concealed is real: a generation that silently
  started or stopped declaring a fabric graph would have changed no gate's
  verdict. It is now closed — `check-sel4-root-boot.py` pins the retained
  generation's own shape, `fabric graph=admitted schemas=3 routes=4
  participants=7 interpositions=1`, read from a boot. Note this also corrects
  an assumption in the original investigation: the P5.1 plane does **not**
  report `absent`; it carries the retained x86 generation's graph, which is a
  different and larger one than any `sel4-*` fixture declares, and the only one
  with an interposition hop. Fault-injected by incrementing `route_count()`:
  the gate reports the missing marker.
