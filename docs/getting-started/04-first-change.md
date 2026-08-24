# Your first change

The workflow, walked once. The exercise below touches nothing permanent —
it exists to make you collide with the two things that most shape work here:
gates that pin behavior, and the routing discipline that tells you where a
change belongs.

## Before anything: the standing order

1. **Backlog first.** `roadmap/00-backlog.md` tracks known defects. Open
   items are resolved (or explicitly deferred, with the reason recorded)
   before new milestone work starts.
2. **Route, don't search.** Find your change's row in `AGENTS.md`'s
   task-to-file index and read the named module root first. Broad
   symbol-grepping across the tree is how you end up editing a generated
   file or a check script instead of the source.
3. **Know your gate.** Every change area has a narrowest QEMU or host gate;
   identify it before editing so you know what "done" will look like.

## Exercise: watch a gate catch you

This demonstrates the marker contract from the
[boot walkthrough](03-boot-walkthrough.md) with a throwaway edit.

1. Run the product plane gate once, green:

   ```sh
   just sel4_component_graph_check
   ```

2. Open `components/bins/spawn-service/src/main.rs`, find the
   `[spawn-service] ready` debug line, and change the text — even one
   character.

3. Run the gate again. It fails: the transcript no longer contains the
   marker `spawn-service reached its service loop` pins, and the gate prints
   the transcript plus exactly which expectation broke
   (`scripts/check/check-sel4-component-graph.py` holds the table).

4. Revert your edit; run the gate green again.

What you just observed: component output is contract surface. A real change
to a marker updates the emitting code *and* the gate's marker table in the
same commit, and the gate diff is the evidence the change was intended —
never a reason to loosen the assertion.

## The real workflow, by change kind

**Behavior in the root or a component** — edit the owning module (per the
index), run the narrowest `sel4_*` plane gate that exercises the path, and
for root logic also `just test_sel4_root` (its test count is asserted; raise
it deliberately when you add tests).

**Anything with a wire format** — schema first, always:
edit `contracts/<name>/vN/schema.zt`, regenerate with the matching
`just *_gen`, commit schema and generated output together, and run
`just contracts_check`. Never edit a file whose first line says
`@generated`. See [contracts](../concepts/contracts.md).

**Authority: grants, slots, budgets** — these live in the generation
fixtures (`contracts/generation/v1/fixtures/`), not in component code.
Expect `just sel4_boot_layout_check` to show the layout diff; bless it only
when the diff is the change you meant. New rights or operations update
[`../capability-matrix.md`](../capability-matrix.md) /
[`../syscall-abi.md`](../syscall-abi.md) in the same change — the latter is
machine-enforced.

**Docs only** — state that no runtime tests were run; `just typos` and
`just devlog_check` (if you touched `devlog/`) still apply.

## Before you finish

For permanent Rust changes:

```sh
just fmt_check_all
just lint_all
```

Then the record-keeping, which is not optional ceremony here:

- **Devlog** (`devlog/`): non-trivial fixes, decisions, regressions, and
  milestone completions get an entry — a dated folder with a curated
  `index.md` from `devlog/TEMPLATE.md`. Run `just devlog_check`. The devlog
  is *how* a conclusion was reached; the roadmap keeps only the outcome plus
  a link.
- **Backlog hygiene**: a fixed defect's entry moves to the backlog's
  resolved log, collapsed to the five prescribed lines, heading text
  untouched — devlog links resolve against it.

`AGENTS.md` carries the full statement of these rules; this page is the
tour, that file is the law.
