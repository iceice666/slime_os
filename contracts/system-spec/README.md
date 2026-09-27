# System specifications

`v1/schema.zt` owns the format-1 composition record. The compiler in
`scripts/lib/system_spec.py` validates authority, component references, slots and
resource policies, then derives generation manifests. Computed authoring does
not change that record shape or any boot format.

## Authoring ownership

`COMPUTED_SYSTEM_SOURCES` in `scripts/lib/system_spec.py` explicitly maps these
committed outputs under `v1/systems/` to `.zt` sources under `v1/sources/`:

- `sel4-call`
- `sel4-io-service-fault` (the lifetime graph with service-fault grants and restart causes)
- `sel4-private-memory-adaptive-rv64`
- `sel4-private-memory-matrix-rv64`
- `sel4-private-memory-stress-rv64`
- `sel4-private-memory-heap-stress-rv64`

Edit their `.zt` sources, not their `.zti` outputs. All other `systems/*.zti`
remain inert hand-authored inputs. An unmapped source, missing mapped source,
competing `.zti` in `sources/`, or `.zt` in `systems/` is refused rather than
silently choosing a precedence. Add a mapping when migrating another composition.
The map is host orchestration, not an additional serialized contract.

`v1/helpers.zt` provides typed architecture variants, executable spawn grants,
endpoint grants with their two slot pins, and minted supervision bindings.
Sources retain explicit authority choices and list ordering; helpers do not
infer extra grants or ambient permissions.

Factory sources encode authority provenance, not just the eventual holder.
The lifetime, service-fault, and driver-reset graphs declare the network
service's factory as a self-grant: root installs it without a capability from
the spawning supervisor. An `init`-sourced factory, as in the boot-launched TCP
and local graphs, instead counts as parent-supplied authority if dynamically
spawned. Preserve this distinction when deriving variants; the owning rule is
`grant_crosses_spawn` in `slime-root/src/generation.rs`.

## Generation and admission

Run from the repository root:

```sh
python3 scripts/generate/generate-generation-from-spec.py
just system_spec_check
```

The generator renders all computed sources, validates every result through the
existing schema and semantic compiler, and derives the manifests before writing
outputs. Its `--check` mode refuses either system-output or manifest drift.
Normal composition admission also refuses a stale computed output.

The small host adapter in `scripts/tools/system-spec-source/` uses the pinned
Zutai semantic analysis and evaluator from `deps/zutai`. Its standalone locked
Cargo graph builds under `build/system-spec-source-target`, not inside a
closure-bearing source tree. It evaluates the same analyzed import graph after
checking confinement and purity; `.zt` evaluation is not delegated to an
unrestricted CLI invocation. Relative imports may traverse contract directories
but may not escape the canonical `contracts/` root, including through symlinks
or package dependencies. Explicit standard-library imports, effectful programs,
and imports of derived system outputs are refused. The compiler's implicit
prelude remains available. Results must be first-order records, lists, text,
integers and booleans, and must satisfy `SystemSpec` plus host semantic checks.
Evaluation is bounded by a host timeout.

Rendering preserves record-field and list order, with the existing two-space
`.zti` layout. A semantics-preserving variant must preserve field values and
list order; inheriting a base record can also inherit its field order. Any
change to committed output bytes, including field order, must regenerate its
downstream manifests and image closures under their ordinary validation rules.

## Verification boundaries

The existing system-spec checker owns computed-source negative controls and
validates the remaining hand-authored corpus. System-image closures bind the
rendered `.zti` input; adding authoring sources must not silently change boot
inputs or rewrite an unrelated generated identity. A host generation check is
not evidence of a new runtime or physical-machine capability.
