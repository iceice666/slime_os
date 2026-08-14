# P5.4 — decomposed into an equivalence inventory before deletion

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Decision |
| Status | Proposed |
| Scope | `roadmap/07-architecture-portability.md`, `roadmap/README.md`, `roadmap/00-backlog.md` |
| Roadmap | P5.4, P5.4.1, C8.5, C8.6, C8.7, C8.8, C8.9, C8.10, B12, B16, B22, B23 |
| Gates | `just devlog_check` |
| Trigger | P5.4 selected as the next uncompleted milestone; found unimplementable as written |
| Baseline | P5.4 a single "Not started" milestone with an exit condition and no deliverables |

## Summary

P5.4 — *retire the custom kernel* — was the next uncompleted milestone and could
not be implemented as written, for two independent reasons: it was
underspecified (a status line, a dependency line, two sentences, and an exit
condition, where every completed sibling has Deliverables, Required checks, and
a Verification target), and its exit condition was not met (seL4 has equivalents
through C8.4; C8.5–C8.10 have none recorded). Deleting `kernel/` now would drop
that coverage silently, violating the milestone's own frozen-oracle invariant.
It is now a parent with four sub-slices, the first of which is the equivalence
inventory the exit condition has always implied but never produced. Nothing was
marked complete.

## Changes

| Area | Change | Rationale |
|---|---|---|
| `07-architecture-portability.md` | P5.4 becomes a parent with P5.4.1, P5.4.2…n, and P5.4.final; parent prose keeps the frozen-oracle statement and records the decomposition evidence | The invariant is the part worth preserving; the scope was the part that was wrong |
| `07-architecture-portability.md` | P5.4.1 specified with Required checks, a Verification target, and an Exit condition, matching P5.5.2's shape | It is the only sub-slice whose content is knowable today |
| `07-architecture-portability.md` | P5.4.2…n named but deliberately unspecified | P5.4.1's output determines their content; specifying them now is implement-by-inference |
| `README.md:15` | Backlog row rewritten: B1–B11 and B13–B21 resolved; B12, B22, B23 open | B17 was recorded open *and* resolved; B16 closed today; B22/B23 opened today |
| `README.md:18` | RPi5 row: RP0–RP1 complete, RP2 next, flagged as needing a rewrite | The row claimed RP1 was still to begin; `09-rpi5-ros2-demo.md:67` reads Complete |
| `README.md:19` | Portability row names P5.4.1 and states the C8.5–C8.10 gap | The row read as though P5.4 were ready to begin |
| `00-backlog.md` | Deleted the duplicate **open** B17 block | A rebase artifact: `d8ed010` removed it and added the resolved copy; `8fc61eb` re-added it verbatim. The resolved entry at the bottom carries strictly more, including the corrected premise |
| `00-backlog.md` | Open section reordered to descending id (B22, B23, B12) | Matches the resolved log's own ordering |

## Decisions

- Decision: decompose P5.4 rather than implement it.
- Rationale: its exit condition requires that "every acceptance check the custom
  kernel guards has an observed seL4 equivalent." That is a *finding*, and no
  one has produced it. The legacy surface is also wider than the roadmap text
  suggested — eight Justfile recipes run a named `kernel/tests/*` binary via
  `cargo test --test`, 31 shell out to `cd kernel` at all, and more reach the
  oracle only through `scripts/lib/harness.py`, which 34 of 43 checkers
  import. Eight
  `kernel/tests/*.rs` files have no named gate at all and are reachable only via
  `just test`; those are where coverage would vanish with no gate turning red.
- Rejected alternative: implement P5.4 by inference — pick a reasonable reading
  of "equivalent" and start deleting. The milestone's own invariant says the two
  systems "are never claimed to be one system", and inferring the equivalence is
  precisely that claim.
- Rejected alternative: mark P5.4 blocked and stop. The decomposition is the
  useful half of the work and needs no new evidence to be correct.

- Decision: the inventory is an **Audit**-kind devlog entry, not a new `roadmap/`
  file.
- Rationale: CLAUDE.md is explicit — the roadmap records completion, the devlog
  records how conclusions were reached, and "run a verification campaign" is a
  devlog trigger.

- Decision: P5.4.1's verification target is `just devlog_check`, with its limits
  stated in the milestone.
- Rationale: `devlog_check` validates that `Gates` resolve to real Justfile
  targets and `Roadmap` ids to real headings, which is most of what the
  inventory asserts *structurally*. It cannot check that a claimed equivalence
  is true. Saying so in the milestone is better than implying a check that does
  not exist; if a cross-referencing script is needed it is written as part of
  that slice.

- Decision: fold B22's fix into P5.4.1 rather than scheduling it standalone.
- Rationale: B16 and B22 are the same defect shape and were each found one at a
  time. P5.4.1 audits lifetime-vs-live bounds as a class, which is where a third
  would surface if one exists. (The B16 sweep established there is no third
  *per-task* table today.)

- Decision: flag RP2 as needing a rewrite; do not rewrite it.
- Rationale: its deliverables specify AArch64 exception vectors, `svc`,
  translation tables, GICv3, and the generic timer — the exact mechanisms P5
  delegates to seL4. Reconciling that is a real decision about the RPi5 track,
  not a documentation correction, and folding it into this change would bury it.

## Open risks and follow-ups

- [ ] **RP2 contradicts the P5 cutover** and is now the RPi5 track's next open
      gate. Flagged in `README.md:18`; needs an owner and its own decision entry.
- [ ] P5.4.2…n are named but unspecified by design. If P5.4.1 finds gaps outside
      C8.5–C8.10, the slice list changes rather than the gaps being forced into
      it.
- [ ] The C8.5–C8.10 gap is asserted from the roadmap's own status lines, not
      from an exhaustive check of what each seL4 gate happens to cover. P5.4.1 is
      what turns that into a finding — until then it is the *reason* for the
      inventory rather than its result.
- [ ] `just devlog_check` was the only gate run for this entry; no runtime
      behavior changed in the documentation half of this work.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none — this is a documentation and scheduling decision.
- Serial/debugger/model output: none.
- Related roadmap item:
  [P5.4 and its sub-slices](../../roadmap/07-architecture-portability.md);
  the C8.5–C8.10 gap in [`02-core-runtime.md`](../../roadmap/02-core-runtime.md);
  [B16 resolved, B22 and B23 opened](../../roadmap/00-backlog.md), recorded in
  [`2026-08-07-b16-supervision-records/`](../2026-08-07-b16-supervision-records/index.md).
