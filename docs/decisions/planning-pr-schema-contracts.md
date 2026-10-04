# Planning pull requests may fix the Zutai schema a grader binds to

**Status:** Proposed
**Related work item:** `01a1023d-42bd-7202-acdf-2b065c8b20b9`

## Context

A product or grader change implements a work item already on `main`, so the
item and its exam land in a planning pull request before any implementation.
That ordering is what makes the grader independent of the code it grades:
`devloop-approval.py` pins the recipe closure to `origin/main`, and an
implementation branch cannot rewrite the checker.

The grader has to bind to something observable. For a plane checker that is a
composition name and a marker vocabulary; for anything that crosses a
persistence, process, or boot boundary it is a versioned Zutai schema under
`contracts/`, which is the only schema language this repository admits. Until
this decision, `scripts/lib/pr_scope.py` classified every path under
`contracts/` as product, so a planning pull request could carry the checker but
not the schema the checker reads. The entropy plane grader landed that way
(`01a0ec3a-a91f-7349-a28a-26a60d8d7f4e`, planning commit `4552c87f`): its
checker, inputs, and recipe were on `main` while the closure, system spec,
composition, and contract it referred to did not exist anywhere, and nothing in
the planning pull request could have said what the service's records looked
like.

A second, smaller gap followed from the first: `check-contracts.py` compiled
only the schemas a generator or checker named. A schema that nothing refers to
yet — which is exactly what a planning pull request would land — was never
compiled.

The question was whether to let the planning pull request carry more of the
interface, and how much.

## Decision

1. **A new `interface` scope class.** An *added*
   `contracts/<name>/v<N>/schema.zt` (nested contract directories included) and
   a contract's `README.md` are `interface`, not `product`. A pull request that
   adds a work item may carry interface paths. Outside a planning pull request
   an interface change needs a landed item exactly as product and grader do.
2. **Only the schema.** The binding generator `gen_rust.zt`, generated bindings,
   `contracts/system-spec/v1/systems/*.zti`, derived manifests, boot-layout
   fixtures, a *modified* schema version, and a renamed or copied schema all
   remain product. A planning pull request that carries any of them is still
   refused and CI names the paths.
3. **Every schema compiles.** `check-contracts.py` runs `zutai-cli check` over
   every `contracts/**/schema.zt`, named or not, so a schema landed ahead of its
   implementation is checked by the gate that already owns contracts.
4. **The grader stays behavioural.** Code-level preconditions, postconditions,
   or invariants are not part of an exam. The dev-spec already carries the
   contract at the system boundary — a requirement statement is the
   postcondition, a counted predicate is its quantifier, and the negative
   controls prove the check is live. Assertions inside a component belong to
   the implementation pull request, and Zutai bounds belong to the schema.

## Alternatives and trade-offs

- **Leave `contracts/` as product and describe the interface in the spec's
  prose.** No checker change, but the interface is then protected only by the
  requirements digest and is not machine-checked; a grader can still land bound
  to nothing, which is the entropy failure again.
- **Let the planning pull request carry the system-spec plane as well.** The
  plane name is what a QEMU grader binds to, but a `systems/*.zti` change
  regenerates the composition manifest, the boot-layout fixture, and the
  closures; that is half an implementation, and it moves slot authority into a
  pull request that has no component to receive it. Refused.
- **Carry `gen_rust.zt` with the schema.** The generator template decides Rust
  type names and module layout; that is a code decision the implementation
  should make, and allowing it would invite committing generated bindings too.
  Refused.
- **Put design-by-contract assertions in the exam.** An exam must land before
  the code it would annotate exists, so it could only assert against an
  interface it also fixes — and that is the schema. Anything finer specifies
  how to implement rather than what to observe.

## Consequences

- A planning pull request for a new cross-boundary format now lands the item,
  the exam, and the versioned schema together; the implementation pull request
  adds the generator, regenerates bindings, and wires the system spec.
- Fixing the schema early is a commitment: changing it afterwards is a
  *modified* schema and therefore product, so it needs an item of its own or a
  new version directory. Choose the version deliberately.
- A schema that compiles but is never implemented is visible: `just
  contracts_check` compiles it, and the system-test-run inventory still refuses
  an unregistered planning-only run.

## Revisit conditions

- A grader that needs to bind to something a schema cannot express — a syscall
  operation name, a marker vocabulary — and that cannot wait for the
  implementation pull request. Extending the class would need the same
  argument this record makes for schemas: the path is an interface, not code.
- devloop or MyQue publishing an interface-pinning contract of their own, in
  which case this repository should defer to it as it does for requirements
  bodies.

## References

- `scripts/lib/pr_scope.py` — the classes; `INTERFACE`, `NEW_SCHEMA`,
  `CONTRACT_README`.
- `scripts/check/check-pr-scope.py` — the synthetic-diff controls.
- `scripts/check/check-contracts.py` — the schema sweep.
- `just pr_scope_check` — the recipe the item's acceptance binds.
- [`CONTRIBUTING.md`](../../CONTRIBUTING.md) "Pull request scope" — the class
  table contributors read.
