# Your first change

The workflow, walked once. The exercise below touches nothing permanent —
it exists to make you collide with the two things that most shape work here:
gates that pin behavior, and the routing discipline that tells you where a
change belongs.

## Before anything: the standing order

1. **Backlog first.** The `backlog`-tagged items in `.tasks/items/` are the
   known defects, and each one owns its own problem statement.
   Open items are resolved (or explicitly deferred, with the reason
   recorded) before new milestone work starts, which `just tasks_check`
   enforces. `just tasks_next` lists what is actionable.
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

2. Open `components/services/spawn-service/src/main.rs`, find the
   `[spawn-service] ready` debug line, and change the text — even one
   character.

3. Run the gate again. It refuses before booting:
   `closure does not resolve: implementations[spawn-service].artifact:
   identity mismatch for components/services/spawn-service`. Every seL4 gate
   first resolves a system-image closure — a recorded digest of each source
   tree the image is built from (`contracts/system-image-closure/v2/closures/`)
   — and your edit changed one of those trees. Regenerate the closures:

   ```sh
   python3 scripts/generate/generate-system-image-closures.py
   ```

4. Run the gate a third time. Now it boots and fails on the marker:
   `missing unordered marker: \[spawn-service\] ready`, with the transcript
   tail and exactly which expectation broke
   (`scripts/check/check-sel4-component-graph.py` holds the table; the
   component's own spec in
   `contracts/component-spec/v1/components/spawn-service.zti` names that
   literal as its pass/fail criterion).

5. Revert your edit, regenerate the closures again, and run the gate green.

What you just observed, twice: component output is contract surface, and so
is the identity of the source that produced it. A real change to a marker
updates the emitting code, the gate's marker table, *and* the regenerated
closures in the same commit; the gate diff is the evidence the change was
intended — never a reason to loosen the assertion.

## The real workflow, by change kind

**Behavior in the root or a component** — edit the owning module (per the
index), regenerate the closures, run the narrowest `sel4_*` plane gate that
exercises the path, and for root logic also `just test_sel4_root` (its test
count is asserted; raise it deliberately when you add tests). Commit the
closures with the change.

**Anything with a wire format** — schema first, always:
edit `contracts/<name>/vN/schema.zt`, regenerate with the matching
`just *_gen`, commit schema and generated output together, and run
`just contracts_check`. Never edit a file whose first line says
`@generated`. See [contracts](../concepts/contracts.md).

**Authority: grants, slots, budgets** — these live in the system spec
(`contracts/system-spec/v1/systems/<name>.zti`), not in component code and not
in the manifests under `contracts/generation-manifest/v1/`, which are derived
outputs. Edit the spec, run
`python3 scripts/generate/generate-generation-from-spec.py`, commit both, and
run `just system_spec_check`. Expect `just sel4_boot_layout_check` to show the
layout diff; bless it only when the diff is the change you meant. New rights
or operations update [`../capability-matrix.md`](../capability-matrix.md) /
[`../syscall-abi.md`](../syscall-abi.md) in the same change — the latter is
machine-enforced.

**Docs only** — state that no runtime tests were run; `just typos` applies.
Run `just docs_check` when editing Markdown documentation, work-item bodies,
PR templates, or documentation links. This checks current local documentation
without requiring a history checkout or archive-network access.

## Before you finish

For permanent Rust changes:

```sh
just fmt_check_all
just lint_all
```

Then leave the records that make the claim reviewable:

- **Commit and PR**: state the change, claim, risks/invariants, review surface,
  exact commands and observed results, evidence class, target or image identity
  when relevant, and known limits. Ordinary changes do not need a devlog.
- **Owning knowledge**: update the owning docs or contract when current behavior,
  operation, or limitations change. Long-lived cross-module choices belong in
  `docs/decisions/`; unfinished designs in `docs/plans/`; exploration stays in
  `docs/directions/`. Expensive reusable investigations or unusual verification
  campaigns belong with their owning knowledge and immutable evidence; consult
  [the history guide](../history.md) for archived records.
- **Work-item hygiene**: record the exit condition that was *observed*, its
  evidence class, scope, target or image identity, and limits in the item. Only
  then close it with `myque close`, which records the closure date. Never
  allocate an id by scanning for the next number — `myque` assigns a UUID. Run
  `just tasks_check`.

`AGENTS.md` carries the full statement of these rules; this page is the
tour, that file is the law.

## Next

This page changes existing behavior. To add a new component end to end — crate,
declaration, composition, authority, launch policy, and observed QEMU behavior —
continue with [Add a component](05-add-a-component.md).
