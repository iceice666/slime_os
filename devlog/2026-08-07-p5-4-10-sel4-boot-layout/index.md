# P5.4.10 (part) — B10's boot layout, frozen on seL4

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/{main,graph}.rs`, `scripts/check/check-sel4-boot-layout.py`, `contracts/boot-layout/v1/fixtures/sel4-*.layout`, `Justfile`, `AGENTS.md` |
| Roadmap | P5.4.10, P5.4, P5.4.1, B10 |
| Gates | `just sel4_boot_layout_check` |
| Trigger | P5.4.1's inventory, which recorded B10 as covered only obliquely on seL4 |
| Baseline | Nine seL4 gates passing; nineteen x86 layout fixtures and none for seL4 |

## Summary

`just boot_layout_check` freezes init's resolved capability layout for nineteen
x86 profiles by booting the retired kernel and diffing its `[layout]` block
against a recorded fixture. No seL4 equivalent existed: P5.4.1 found B10 covered
only *obliquely* here, by three gates that assert specific slot numbers in
passing, which catches a layout that moved those slots and nothing else.
`slime-root` now emits the same block the oracle does, and `just
sel4_boot_layout_check` freezes all eight plane layouts. Fault-injected by
renaming a base-layout row: the gate reports the renumbering slot by slot.

## Changes

| Area | Change | Effect |
|---|---|---|
| `graph.rs` | `CapabilityTable::slots()` — every slot in numbering order | The dump can read the table without the layout's shape being inferred |
| `main.rs` | `[layout] path=… / [layout] N kind label rights / [layout] end`, emitted after materialization and before activation | The layout is observable in the oracle's own line shape |
| `main.rs` | `resource_label` | An executable carries its component name, as `dump_boot_layout` does |
| `check-sel4-boot-layout.py` | Boots all eight planes, checks block shape, diffs against fixtures | A layout change is a reviewable diff |
| `contracts/boot-layout/v1/fixtures/sel4-*.layout` | Eight fixtures | The frozen record |
| `Justfile`, `AGENTS.md` | `sel4_boot_layout_check` / `_bless`, registered | Discoverable |

The emit point is load-bearing: after `channel::materialize`, when init's table
is complete, and before `activate`, when nothing has run against it yet.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A base-layout row moves and renumbers a plane | `just sel4_boot_layout_check` | `layout differs from …` with a `was:`/`now:` pair per changed slot |
| A layout block becomes malformed | same | `malformed header` / `malformed entry` / count mismatch |
| Two grants resolve to one slot | same | `slot numbers are not strictly ascending` |
| The dump stops being emitted | same | `boot emitted no complete layout block` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_boot_layout_check` | Pass — 8 plane layouts match — [`layout-check.log`](layout-check.log) | Direct |
| Fault injection: base-layout row 49 renamed | Fails with a slot-by-slot diff on `sel4-stream` — [`fault-injection-renumbered.log`](fault-injection-renumbered.log) | Direct |
| The nine seL4 plane gates | All pass — the dump added lines none of them assert on | Direct |
| `just contracts_check` | Pass — the x86 resource checker ignores the new fixtures, as its own `FIXTURE_PROFILES` list is explicit | Direct |
| `just test_sel4_root`, `generation_check`, `devlog_check` | Pass | Direct |
| `just fmt_check_all`, `lint_all`, `ruff`, `typos` | Pass | Direct |

The recorded layouts are genuinely distinct and genuinely pruned — `sel4` has
one slot, `sel4-stream` eight, and `crossing-peer` sits at renumbered slot 15
against base-layout row 62, which is the pruning the roadmap predicted when that
row was added.

## Decisions

- Decision: **every plane**, not one.
- Rationale: each boots a different generation, and `layout_for` prunes the base
  table by the components that generation declares. The pruning is the
  interesting part because it renumbers, so freezing one plane would leave the
  other seven unguarded against the change most likely to break them — which is
  exactly the shape of the defect B10 exists to prevent.

- Decision: a **separate gate**, not an assertion added to each plane's gate.
- Rationale: `boot_layout_check` is separate on x86 for the same reason. A
  layout diff should be readable as a layout diff, not inferred from a component
  failing somewhere downstream, which is the whole failure mode B10 describes.

- Decision: the dump prints kind and rights, but not a channel's key.
- Rationale: a channel key is an allocation detail that can differ between two
  correct boots. Printing it would make the fixture record noise, and a fixture
  that changes for reasons no one intended stops being read.

## Open risks and follow-ups

- [ ] **Widening a layout row is legitimately invisible here**, and that is
      correct rather than a gap. `slime-root` installs rights from the *grant*
      and uses the layout as an upper bound (`channel.rs::bootstrap_slot`), so a
      layout granting more than the generation declares is absorbed. My first
      fault injection changed a rights bit and the gate passed; the second
      changed a slot number and it failed. Recorded because the first looks like
      a gate defect until the containment rule is read.
- [ ] **The P5.1 fixture variant has no layout**, deliberately: it embeds the
      retained x86 generation, launches no component graph, and so has no init.
      It is absent from `PLANES` rather than silently skipped.
- [ ] **Six P5.4.10 rows remain** — C8.1 collision rejection, C8.3 graph
      provenance, C8.4's structural arm, C7.1's retained-v2 arm, B11's
      product-vs-test pair, and `task_reclamation.rs`'s three properties.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [`layout-check.log`](layout-check.log).
- Serial/debugger/model output:
  [`fault-injection-renumbered.log`](fault-injection-renumbered.log).
- Related roadmap item:
  [P5.4.10](../../roadmap/07-architecture-portability.md) (one more row closed),
  [B10](../../roadmap/00-backlog.md) (the milestone this covers on seL4),
  [P5.4.1](../../roadmap/07-architecture-portability.md) (the inventory that
  recorded the gap).
