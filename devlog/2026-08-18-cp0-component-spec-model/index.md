# CP0 — component specification model

| Field | Value |
|---|---|
| Date | 2026-08-18 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/component-spec/v1/`, `scripts/lib/component_spec.py`, `scripts/check/check-component-spec.py`, `scripts/generate/generate-component-spec-bindings.py`, `scripts/check/check-contracts.py`, `Justfile` |
| Roadmap | CP0, B70 |
| Gates | `just component_spec_check`, `just contracts_check` |
| Trigger | `969fbac` opened the component platform track and B70; CP0 is its first milestone and has no unmet dependency |
| Baseline | No component-level specification existed: `contracts/generation/v1`'s `Executable`/`Instance` pair was the only description of a component anywhere in the repository |

## Summary

CP0 adds `contracts/component-spec/v1` as the first specification of what a Slime
OS component *is*, independent of how any one generation composes it, plus a
42-record corpus covering every component `contracts/generation/v1/fixtures/valid.zti`
declares and a `just component_spec_check` gate that validates it. The gate does
not merely decode records: it cross-checks each one field by field against the
reference generation — type, owner, health, dependencies, spawn budget, stack
bytes, extra threads, shared-buffer budget, target, and every fabric route role
with its exact QoS values — so a spec cannot disagree with the manifest that
composes it. 37 named malformations are refused, each paired with an admitted
baseline of the same shape.

Authoring the corpus surfaced one real defect in existing state: two components
`valid.zti` declares, `generation-list` and `storage-store-probe`, have no
`[[bin]]` target and no source file. Both were deleted as unreachable clients of
retired syscalls while their manifest, boot-layout, and fabric-profile entries
stayed. The schema records that as `implementation.provider = "undeclared"`
rather than inventing a source file for them, and the gate pins the set to
exactly those two from both directions.

No component source, root code, or generation byte changes. CP0 declares and
validates the model; CP1 derives manifests from it and CP2 moves slot resolution
to runtime.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/component-spec/v1/schema.zt` | New `ComponentSpec` record covering the twelve sections `spec/requirement-document-v0.6.md` §2.1 names, with every list and text field bounded and every vocabulary closed | A component has a formal description independent of one generation manifest (B70) |
| `contracts/component-spec/v1/schema.zt` | `QosPolicy` reuses `contracts/generation/v1/schema.zt`'s `FabricParticipant` QoS field names and value sets verbatim | One QoS vocabulary, so a spec's policy and a graph's policy are comparable field by field rather than by translation |
| `contracts/component-spec/v1/schema.zt` | Identity is SHA-256 over `identityDomain` plus normalized sorted-key whitespace-free JSON — `contracts/interface-schema/v1`'s convention, not a second normalizer | Component and interface identities are computed the same way |
| `contracts/component-spec/v1/schema.zt` | `Implementation` record with closed `provider` (`workspace`/`external`/`undeclared`) and a `binary` naming the `[[bin]]` target | The manifest-name-to-binary mapping is a declared fact, not a convention a reader rediscovers |
| `contracts/component-spec/v1/gen_python.zt` | Renders every bound and vocabulary into `scripts/lib/component_spec_contract.py` | The gate consumes the contract's vocabularies instead of holding a second copy (B57, B59, B60) |
| `contracts/component-spec/v1/components/*.zti` | 42 records, one per declared component | Every component in the reference generation has a spec |
| `scripts/lib/component_spec.py` | Compiler: decodes, validates semantics, computes identity | Semantic admission has one implementation both the gate and future CP1 derivation use |
| `scripts/check/check-component-spec.py` | Gate: corpus coverage, manifest agreement, fabric-graph projection, implementation facts, identity stability, 37 refusals | The model is guarded rather than trusted |
| `scripts/generate/generate-component-spec-bindings.py` | `--check` drift gate for the generated bindings | Stale bindings fail rather than silently diverge |
| `scripts/check/check-contracts.py`, `Justfile` | Wire the contract into `contracts_check` and add `component_spec_check` | The new contract is type-checked by the same gate every other contract is |
| `scripts/lib/component_spec.py` | Capability kinds and the device-kind subset are read from `build-generation.py`'s `CAPABILITY_KIND`/`SERVICE_BY_CAPABILITY_KIND`, and `MAX_SPAWN_BUDGET`/`COMPONENT_MAX_STACK_BYTES` are imported, not retyped | The validator holds no second copy of a vocabulary or ceiling (B57, B59, B60) |
| `scripts/check/check-component-spec.py` | `provides`/`requires` are derived from the manifest's `grants[]` and compared in both directions | A record cannot claim authority the generation never grants it, nor omit authority it does |
| `scripts/check/check-component-spec.py` | The fabric-graph projection is bidirectional: every declared interface entry and QoS policy must be authorized by a participant role, an interposition hop (including profile-declared chains), or route-worker ownership | A record cannot declare a route role or policy the graph does not give it |
| `scripts/lib/component_spec.py` | `passFailCriteria` is matched against string literals `ast`-parsed out of the named gate's check script, unescaping the gate's regex spelling rather than stripping backslashes from both sides | A criterion is text the gate matches on, not any fragment of its source |
| `scripts/lib/component_spec.py` | `compatibility.interface` must be a `contracts/<name>/v<N>` path with a `schema.zt`; `dependency`/`resource`/`runtime` modes are each derived from a declared fact; parameter names must be `runtime.resource` fields and defaults must equal them | Four fields that were free choice or decorative are now checked facts |
| `scripts/lib/component_spec.py` | The QoS value sets come from the builder's `FABRIC_RELIABILITY`/`FABRIC_DURABILITY`/`FABRIC_LIVELINESS` | The last inline vocabulary is gone; the spec and the graph are admitted against one table |
| `.github/workflows/ci.yml` | `just component_spec_check` runs in the contracts job | The gate is reachable from CI rather than only by hand |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A spec drifts from the generation that composes it | `just component_spec_check` | `console: spec health != manifest health` — observed by flipping one field |
| A spec's QoS diverges from the fabric graph | `just component_spec_check` | `fabric-publisher: QoS historyDepth for telemetry is 5, but the graph declares 4` — observed |
| A third component silently loses its implementation | `just component_spec_check` | `declared-without-implementation set is [...]`, and the reverse direction refuses a pinned name whose `[[bin]]` reappears — observed |
| A component is added to the generation without a spec | `just component_spec_check` | `no component spec for declared component(s): [...]` |
| A record claims a semantic, tag, or lifecycle its declarations do not back | `just component_spec_check` | the matching named refusal |
| The generated bindings go stale | `just contracts_check` | `scripts/lib/component_spec_contract.py` is stale |
| A record claims capability authority the manifest does not grant | `just component_spec_check` | `console: spec requires ['directory', 'input', 'sharedBufferFactory'] != the manifest's grant-derived []` — observed |
| A record declares a fabric role the graph does not give it | `just component_spec_check` | `console: declares a input entry for route telemetry (TelemetryStream) that the fabric graph does not give it` — observed |
| A record names an evidence marker no gate emits | `just component_spec_check` | `fabric-call-worker: pass/fail criterion '…' appears nowhere in sel4_boot_check's check script` — observed |
| A criterion names a gate literal that does not exist, or any source fragment | `just component_spec_check` | `pass/fail criterion 'import' matches no string literal in sel4_channel_check's check script` — observed for `def `, `import`, and `#` |
| A parameter default drifts from the resource field it configures | `just component_spec_check` | `configuration[spawnBudget]: default 3 disagrees with runtime.resource.spawnBudget = 18` — observed |
| A compatibility mode stops following the fact it describes | `just component_spec_check` | `compatibility.resource must be 'atMost': a resource requirement is a ceiling` — observed |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `python3 scripts/check/check-component-spec.py` | pass: `42 records validated against 4 declared interfaces and the reference generation; 37 named mutations refused, identities stable; declared-without-implementation: generation-list, storage-store-probe` | Direct |
| `python3 scripts/check/check-contracts.py` | pass, now naming `component-spec` in its summary; 182 boot-contract unit tests pass | Direct |
| `python3 scripts/generate/generate-component-spec-bindings.py --check` | `scripts/lib/component_spec_contract.py is current`, before and after a regeneration | Direct |
| `ruff check scripts/` | `All checks passed!` | Direct |
| `typos` over the new contract and scripts | no findings | Direct |
| Non-vacuity: neutralize the semantic-derivation rule | `a semantic no referenced interface backs was accepted`, then reverted | Direct |
| Non-vacuity: neutralize the durability/retained-depth agreement rule | `retained durability with no retained depth was accepted`, then reverted | Direct |
| Non-vacuity: neutralize dependency-cycle detection | `a dependency cycle was accepted`, then reverted | Direct |
| Non-vacuity: append a `[[bin]] generation-list` entry to `components/bins/Cargo.toml` | `the committed corpus was refused: implementation: declared undeclared, but [[bin]] 'generation-list' exists`, then reverted | Direct |
| Every record decodes under `contracts/component-spec/v1/check.zt` | 42/42 `#valid` | Direct |
| Reviewer probe: `console` claiming `directory`/`input`/`sharedBufferFactory` it holds no grant for | refused after the fix; accepted before it | Direct |
| Reviewer probe: `console` declaring a `telemetry` input role the graph never gives it | refused after the fix; accepted before it | Direct |
| Reviewer probe: a worker naming `SLIME_GRAPH spawned component=fabric-call-worker`, a marker no gate emits | refused after the fix; accepted before it | Direct |
| A fresh reviewer pass over the whole diff | verdict `incorrect` on the pre-fix diff, naming two grounding gaps and five secondary issues; all applied or answered below | Direct |
| Reviewer instrumentation of all 30 round-1 negative arms | each refused by the rule it names; none vacuous | Inherited (reviewer transcript) |
| Round-2 reviewer pass over the fixes | both P0s confirmed closed: 8 further projection attacks all refused, and the authorized-set minus declared-set across all 42 records is exactly 0, so the four authorization sources admit nothing the corpus does not already declare | Inherited (reviewer transcript) |
| Round 2 found two further defects, both fixed | the worker markers were off by one (each `[init]` line follows the spawn it confirms), and the criterion check was a raw-source substring test that accepted `def `, `import`, and `#` | Direct |

Three independent weakenings of `component_spec.py` each made exactly the arm
that names the weakened rule falsely pass, and no other arm — so the arms test
the rules they name rather than tripping an earlier guard. This is the discipline
[B67](../../roadmap/00-backlog.md) established after two negative controls were
found structurally incapable of failing.

No QEMU gate was run: CP0 changes no component source, no root code, and no
generation byte, so no booted behavior is affected. `just contracts_check` is the
gate that guards the surface this change actually touches.

## Decisions

- **Decision:** `implementation.provider` includes `undeclared`, and two records use it.
  **Rationale:** `generation-list` and `storage-store-probe` are declared in `valid.zti`, in every `contracts/boot-layout/v1/fixtures/*.layout`, and (for `generation-list`) in `components/bins/src/default_fabric_profile.rs`, but neither has a `[[bin]]` entry or source file — independently confirmed against `components/bins/Cargo.toml` and corroborated by [`devlog/2026-08-10-b44-policy-labels-deleted/`](../2026-08-10-b44-policy-labels-deleted/index.md) and [`devlog/2026-08-10-b43-block-service-endpoint/`](../2026-08-10-b43-block-service-endpoint/index.md).
  **Rejected alternative:** mapping `storage-store-probe` onto `sel4-store-probe.rs`, which is a role analog rather than a name resolution — `build-generation.py::component_executable` resolves an ELF strictly by the manifest's literal executable name with no rename table, so the mapping would have been fabricated. Also rejected: deleting the manifest entries, which is a generation change and therefore CP1's or a backlog item's call, not CP0's.

- **Decision:** QoS reuses `FabricParticipant`'s field names and value sets rather than defining a component-side vocabulary.
  **Rationale:** the two must be comparable field by field; a second vocabulary could only be compared through a translation table, which is where the two would diverge. The same two agreement rules the generation builder enforces (`retained` needs a retained depth, `manual` liveliness needs a lease) are enforced here for the same reason.
  **Rejected alternative:** a reference into `contracts/fabric-qos/v1`, which declares runtime QoS *events* (`QosEvent`, `matched`/`incompatible`/`expired`), not declared policy, so it is the wrong contract to reference.

- **Decision:** each conditional lifecycle state is tied to a declared fact — `Configure` to having parameters, `Ready` to being an init or service component, `Degraded` to a QoS policy that can expire, `Stop` to holding supervision authority.
  **Rationale:** the requirement document notes lifecycle applicability depends on component type, so a subset must be admissible; but an unconstrained subset would be taste. Tying each state to a fact already in the record makes it derivable and therefore checkable in both directions.
  **Rejected alternative:** requiring all eight states of every component, which would make `Configure` meaningless for the 34 components with no parameters.

- **Decision:** `test.requiredTestEnvironment` must name a real Justfile target and `passFailCriteria` carries the exact assertion or serial marker.
  **Rationale:** same rule `just devlog_check` already enforces for a devlog's `Gates` front matter — a spec naming a gate that does not exist claims verification the repository cannot honour.
  **Rejected alternative:** free-text test prose, which is unenforceable and would rot silently.

- **Decision:** resource bounds cite the constants the builder and root already enforce rather than new numbers.
  **Rationale:** `COMPONENT_MAX_STACK_BYTES` is imported from `scripts/lib/boot_contracts.py` directly; `MAX_SPAWN_BUDGET`, `MAX_CHILD_THREADS`, and `MAX_TOTAL_PAGES` are named to their sources in comments. A spec declaring more than the platform can grant is refused here rather than at the boot that would have failed.

- **Decision:** two invented resource rules were removed during implementation after the manifest disproved one.
  **Rationale:** an initial `mappingCount` implies `bufferCount` rule refused the real corpus: `fabric-subscriber` and `fabric-subscriber-b` are granted 4 mappings and 0 buffers in `valid.zti`, because a subscriber maps loaned pages it never allocated. The rule was replaced with the pages/buffers relation, which the budget actually holds. Recorded because it is the exact failure mode this milestone exists to prevent — a checker asserting the author's model instead of the system's.

- **Decision:** the reviewer's two P0 findings were fixed by adding cross-checks, not by weakening the corpus.
  **Rationale:** a fresh read-only reviewer pass returned `incorrect`, having compiled a `console` record that claimed `directory`/`input`/`sharedBufferFactory` authority it holds no grant for, and observed that the fabric projection only ran graph→spec, so a record could declare route roles and QoS policies no participant entry gives it. Both are the fabricated-fact class CP0 exists to prevent, and both were derivable from data the gate already loaded: `provides`/`requires` are now derived from `grants[]` and compared both ways, and the projection now authorizes each declared entry from a participant role, an interposition hop, or route-worker ownership. The committed corpus needed no edit — it already agreed with both derivations — which is itself the evidence that the corpus was grounded and only the *gate* was permissive.
  **Rejected alternative:** narrowing the claims in the records so the missing checks could not catch anything, which would have preserved the hole for the next author.

- **Decision:** `passFailCriteria` is verified against the named gate's own script rather than trusted.
  **Rationale:** the reviewer found two worker records naming `SLIME_GRAPH spawned component=fabric-op-worker`, a marker no gate emits — the real line carries `task=N child=N` between. A valid Justfile target paired with a marker nothing looks for is the same unverifiable claim wearing a valid name. The gate now resolves each recipe to its check script and requires the literal to appear there, which corrected both records and closes the class.

## Open risks and follow-ups

- [ ] `generation-list` and `storage-store-probe` remain declared with no implementation. CP0 records the gap; resolving it (delete the manifest, layout, and fabric-profile entries, or build the components) needs a generation change and belongs to CP1 or a new backlog item.
- [ ] The capability-kind vocabulary and the device-kind subset are read from `scripts/build/build-generation.py`'s `CAPABILITY_KIND` and `SERVICE_BY_CAPABILITY_KIND`, which is the table that admits a manifest's `capabilityKind` — but that table is itself hand-written Python, not generated from a contract. Collapsing it into `contracts/generation/v5` alongside the rights vocabulary B57 moved there is follow-on work in the same class as B59. CP0 consumes the existing single source rather than adding a second one; it does not make that source generated.
- [ ] Every record's `runtime.executionEnvironment` is `x86_64-qemu-virtio`, because that is what `valid.zti` declares. The seL4 product path uses `aarch64-sel4-qemu-virt` fixtures, which CP1 covers when it derives the `sel4-*.zti` manifests.
- [ ] `provider = "external"` is declared and validated but unused until CP4 admits an externally supplied artifact.

## Artifacts and provenance

- Contract: [`contracts/component-spec/v1/README.md`](../../contracts/component-spec/v1/README.md)
- Related roadmap item: [CP0 in the component platform track](../../roadmap/10-component-platform.md)
- Backlog item this bears on: [B70](../../roadmap/00-backlog.md)
