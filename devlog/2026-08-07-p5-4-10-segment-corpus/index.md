# P5.4.10 (part) — the component-image segment corpus, made portable

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Change |
| Status | Verified |
| Scope | `boot-contracts/src/component_image.rs` |
| Roadmap | P5.4.10, P5.4, P5.4.1, P0 |
| Gates | `just test_host`, `just miri` |
| Trigger | P5.4.1's inventory, which recorded `component_image.rs`'s 11 neutral assertions as coverage that vanishes silently with `kernel/` |
| Baseline | Nine seL4 gates passing; the segment rules living only in the frozen oracle |

## Summary

The component-image *segment* rules — W^X, page alignment, sorted
non-overlapping ranges, file ranges within the payload, entry inside an
executable segment, and the footprint ceiling — existed only in
`kernel/src/runtime/component.rs` and were exercised only by
`kernel/tests/component_image.rs`, one of the eight files no Justfile target
names. P5.4.1 recorded them as architecture-neutral coverage with no seL4
equivalent: P5.2 observes the positive path and target mismatch, and nothing
exercises the malformed corpus. Moved to `boot-contracts` as
`validate_segments`, host-tested by eleven cases, so the rules survive the
oracle's deletion.

## Changes

| Area | Change | Effect |
|---|---|---|
| `component_image.rs` | `SegmentError` and `validate_segments(records, data, count, entry_offset, page_size)` | The rules are a property of the format, where every producer and consumer can reach them |
| `component_image.rs` | Re-exports `MAX_IMAGE_BYTES`, `MAX_STACK_BYTES`, `SEGMENT_FLAG_*`, `WireSegmentRecord` | Callers need not reach into `wire::` |
| `component_image.rs` tests | Eleven cases, one per malformed class plus two positive | The corpus is run by `just test_host` and `just miri` |

`page_size` is a parameter rather than a constant, which is the one substantive
difference from the oracle's copy: that one reads `crate::memory::PAGE_SIZE`, an
x86 constant. A format rule cannot depend on the host architecture's page
granule, so the caller supplies its profile's.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A rule is dropped | `just test_host` | The matching case fails; fault-injected for W^X |
| A rule is subtly wrong on a boundary | `just test_host` | `a_footprint_past_the_image_ceiling_is_refused` caught exactly this while being written — the first fixture used `MAX_IMAGE_BYTES` itself, which is admissible |
| UB in the new arithmetic | `just miri` | 108 tests clean under Miri |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `cd boot-contracts && cargo test --all-features` | Pass — 108, of which 11 are new — [`segment-tests.log`](segment-tests.log) | Direct |
| Fault injection: the W^X check removed | Fails — [`fault-injection-no-wx.log`](fault-injection-no-wx.log) | Direct |
| `just miri` (boot-contracts arm) | Pass — 108 clean in 104s | Direct |
| `just contracts_check`, `just generation_check`, `just devlog_check` | Pass | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos` | Pass | Direct |
| The nine seL4 gates | All pass — this slice adds a function nothing yet calls at runtime, so no boot behavior changes | Direct |
| `just test_host` as a whole | **Fails on this host for an unrelated pre-existing reason** — its `slime-proto` arm pins `x86_64-unknown-linux-gnu`. The `boot-contracts` arm carrying these tests passes; run directly above | Direct, partial |

## Decisions

- Decision: the rules move to **`boot-contracts`**, not to `slime-root`.
- Rationale: `slime-root` has no SLIMECM loader and will not grow one — P5.2
  replaced those payloads with native ELF. The rules are a property of the
  *format*, which is what `boot-contracts` is for, and P0's required check says
  the corpus must reject a bad load layout "regardless of producer". Putting
  them in a consumer would have re-created the coupling P5.4.1 exists to remove.

- Decision: the frozen oracle keeps its own copy.
- Rationale: rewriting `kernel/src/runtime/component.rs` to call this would edit
  the oracle, which the frozen-oracle rule forbids while it is still the
  regression reference. The duplication is deliberate and temporary; removing it
  is P5.4.final's business, when the file is deleted rather than changed.

- Decision: eleven cases rather than porting all fifteen.
- Rationale: four of the oracle's fifteen are SLIMECM *wire-tag* assertions —
  magic, revision version, header size, the retained-v1 reserved field. Those
  are already covered by `boot-contracts`' existing fourteen header tests, and
  they die with the format rather than with the kernel. The eleven ported are
  exactly the neutral set P5.4.1 identified.

## Open risks and follow-ups

- [ ] **Nothing calls `validate_segments` yet.** It is the rules made portable
      and testable, not a new admission path: `slime-root` has no SLIMECM
      loader, and the oracle keeps its own copy until deletion. If a future
      loader for a segment-carrying format appears, this is what it should call.
      Recorded plainly so the function is not mistaken for a live guard.
- [ ] **P5.4.10 is not closed by this.** It covers `component_image.rs`'s
      malformed corpus only. C8.1's collision rejection, C8.3's graph
      provenance, C8.4's structural arm, C7.1's retained-v2 arm, B10's missing
      seL4 layout fixture, B11's product-vs-test pair, and
      `task_reclamation.rs`'s three uncovered properties all remain.
- [ ] The `stack_bytes` rules (zero, unaligned, over `MAX_STACK_BYTES`) were
      **not** ported: they read the oracle's `PAGE_SIZE` in a header context
      rather than a segment one, and belong with a header validator if one is
      ever written. Two of the oracle's fifteen cases; recorded rather than
      silently dropped.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [`segment-tests.log`](segment-tests.log) — the eleven cases
  and the suite total.
- Serial/debugger/model output:
  [`fault-injection-no-wx.log`](fault-injection-no-wx.log).
- Related roadmap item:
  [P5.4.10](../../roadmap/07-architecture-portability.md) (partially advanced),
  [P5.4.1](../../roadmap/07-architecture-portability.md) (the inventory that
  recorded the gap), [P0](../../roadmap/07-architecture-portability.md) (whose
  required check says the corpus must reject regardless of producer).
