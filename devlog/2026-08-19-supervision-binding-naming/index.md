# Supervision binding naming convention

| Field | Value |
|---|---|
| Date | 2026-08-19 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/generation/v1/fixtures/sel4-{boot,traffic,call,operation,generation,sample}.zti`, `scripts/build/build-generation.py` |
| Roadmap | B70, CP2 |
| Gates | `just contracts_check`, `just generation_check`, `just system_spec_check`, `just sel4_boot_layout_check`, `just sel4_call_check`, `just sel4_operation_check`, `just sel4_sample_check`, `just sel4_generation_check`, `just sel4_fabric_aggregate_check`, `just ruff`, `just typos` |
| Trigger | B70's `minted:` axis landed (ad34017) but could not be migrated onto: `FABRIC_SUPERVISION`'s 13 uses resolve names spelled three incompatible ways across 8 manifests |
| Baseline | 55 minted supervision bindings across 10 seL4 manifests, named by three conventions; `minted:<name>` resolvable in principle, unusable from a component in practice |

## Summary

The `minted:` resolve axis answers which of a component's slots holds a named
runtime-created binding, but a component can only ask by name if the name means
the same thing in every generation that declares it. Three conventions were in
the fixtures at once — `fabric-publisher-supervision`,
`fabric-service-supervision-publisher`, and
`fabric-service-call-client-supervision` — so asking by name required a
manifest-specific alias table, which is the compile-time coupling B70 exists to
remove. All 55 bindings now follow one rule: a supervision handle is named for
the **task it supervises**, `<supervised-instance>-supervision`. 35 were renamed
and 20 already conformed. The convention is asserted in the builder rather than
merely applied, so a fourth convention is a build failure.

The 13 `FABRIC_SUPERVISION` uses this unblocks turn out not to be 13 instances of
one question. The **2** that are genuine name→slot lookups are migrated here and
verified on real boots; the other 5 need a *count* or a *membership list* of a
component's bindings, which one-name-one-slot cannot express, and the remaining 4
mentions are comments. So the naming blocker is closed and real, and what remains
is a query-shape question rather than a naming one.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| 6 fixtures | 35 minted supervision bindings renamed to `<supervised-instance>-supervision` | One binding name denotes one graph fact across every generation declaring it |
| `build-generation.py` | New `validate_supervision_binding_names`, called before minted encoding | A supervision binding names a declared instance, owned by the same minter, that is not its own holder |
| `fabric-service.rs` | `supervision_slot_for` resolves `minted:<component>-supervision` from the root instead of reading `FABRIC_SUPERVISION` | A component's supervision slot is runtime-resolved, not compiled in |
| `matrix_broker.rs` | `settled` calls that same resolver instead of its own table scan | One resolution path, so the refusal behaviour cannot diverge between the two |

Why the supervised task rather than the holder: it is the only choice that is a
property of the graph rather than of which manifest declares it. `fabric-service`
and `fabric-call-worker` both supervise `fabric-call-client` on different planes;
under this rule both name that handle `fabric-call-client-supervision`, which is
exactly what lets one component source resolve it under either composition. A
holder-first name (`fabric-service-call-client-supervision`) encodes the
composition into the name and so re-creates the coupling.

The owner clause was probed against all 10 manifests before being written, not
assumed: every supervised instance is in fact owned by its handle's minter, and
no holder supervises itself.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A future fixture reintroduces a fourth convention | `just contracts_check` (builder invariant) | `minted binding <name>: a supervision binding is named <supervised-instance>-supervision` |
| A name points at a task its minter cannot supervise | `just contracts_check` | `supervises an instance owned by X, but is minted by Y` |
| A rename silently moved authority | `just sel4_fabric_aggregate_check` and the four plane gates | Plane timeout or trace-record mismatch |

The invariant was proven non-vacuous by re-injecting each retired convention into
`sel4-call.zti` and observing the refusal, including the two real historical
spellings:

```
refused (old holder-first convention): minted binding fabric-service-call-client-supervision:
  names no declared instance ('fabric-service-call-client' is not an instance in this generation)
refused (old worker convention): minted binding fabric-call-worker-supervision-server:
  a supervision binding is named <supervised-instance>-supervision
refused (suffix dropped entirely): minted binding client-b-handle:
  a supervision binding is named <supervised-instance>-supervision
refused (names a non-instance): minted binding fabric-call-ghost-supervision:
  names no declared instance ('fabric-call-ghost' is not an instance in this generation)
```

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| 28-manifest byte snapshot, before vs after **the rename alone** | Exactly the 6 edited manifests changed `generation.bin`/`boot-store.bin`; the other 22 byte-identical | Direct |
| Minted-record multiset, name field excluded, **rename alone** | Identical before and after for all 28: no `(owner, holder, slot, rights, kind)` tuple changed | Direct |
| `just test_sel4_root` | 129/129 across 15 modules | Direct |
| `just fmt_check_all`, `just lint_all` | Pass | Direct |
| `just sel4_matrix_check`, `just sel4_stream_check`, `just sel4_visibility_check`, `just sel4_qos_check` | Pass — the four planes exercising the two migrated call sites | Direct |
| `just sel4_fabric_aggregate_check`, re-run after the component migration | Pass — 280 byte-identical records again | Direct |

Both byte comparisons were taken across the fixture rename **only**, before the
two component call sites were migrated. Component code is compiled into the
generation, so the later `fabric-service`/`matrix_broker` edits necessarily move
`generation.bin`'s bytes on every plane carrying those binaries; that is the
change being made, not drift, and the plane gates above are what verify it. The
isolated comparison is what establishes the *rename* moved no authority.
| `just contracts_check` | Pass | Direct |
| `just generation_check` | Pass — two isolated builds byte-identical | Direct |
| `just system_spec_check` | Pass — CP1 baselines unaffected (they declare no supervision minted binding) | Direct |
| `just sel4_boot_layout_check` | 25 plane layouts match their frozen fixtures, no bless needed | Direct |
| `just sel4_call_check` | Pass — 47 markers, three post-spawn supervision introductions | Direct |
| `just sel4_operation_check` | Pass — 53 markers across 15 causal chains, four participant identities vouched | Direct |
| `just sel4_sample_check` | Pass — peer death reclaimed every resource | Direct |
| `just sel4_generation_check` | Pass | Direct |
| `just sel4_fabric_aggregate_check` | Pass — 280 byte-identical semantic-trace records across four boots of `sel4-boot`/`sel4-traffic` | Direct |
| `just ruff`, `just typos` | Pass | Direct |

The aggregate gate is the decisive one: `sel4-boot` and `sel4-traffic` carry 13
renamed bindings each, the largest change in the set, and both booted twice with
byte-identical traces.

## Decisions

- **Decision:** name a supervision handle for the supervised task, and assert it
  in the builder.
- **Rationale:** the supervised task is a graph fact; the holder is a
  composition fact. Only the former is stable across the generations a single
  component source must run under. Asserting it means the next fixture cannot
  quietly reintroduce the problem this change exists to remove.
- **Rejected alternative:** an alias table mapping each manifest's spelling to a
  logical role. That is the manifest-specific indirection B70 is removing, moved
  from `build.rs` into a component.

## Open risks and follow-ups

- [x] The **2** name→slot sites are migrated: `supervision_slot_for` resolves
      `minted:<component>-supervision` from the root, and `settled` calls it
      rather than keeping a second table scan. This is what proves the convention
      is usable, not merely consistent.
- [ ] **5** uses remain, and they are not the same question. **3** are `.len()`
      arithmetic deriving `TIME_SLOT`/`FIRST_ROUTE_SLOT` (`fabric-service.rs:119`,
      `matrix_broker.rs:832`, `visibility_broker.rs:61`), needing a *count* of the
      holder set; **2** are `.iter()` teardown walks (`fabric-service.rs:381,525`),
      needing its *membership*. Neither is expressible as one-name-one-slot, so
      both need a query returning a component's binding *set* — a query-shape
      question for CP2, not a naming one. The remaining 4 mentions are comments
      and one import, which fall out with the code they describe.
- [ ] The `.iter()` walks additionally ask which components *exist*, which remains
      a graph fact belonging to the `fabric-graph` resource read that has no
      syscall yet.

## Artifacts and provenance

- Rename map derivation: authoritative, from the builder's own resolved
  `FABRIC_SUPERVISION` table and init's spawn order in `launch_fabric_calls` /
  `launch_fabric_operations` / `drive_generation_plane` / `drive_sample_plane` —
  never from the old name's spelling, since B71 recorded three names whose
  spelling disagreed with the slot they resolved.
- Related roadmap item: [B70](../../roadmap/00-backlog.md), [CP2](../../roadmap/10-component-platform.md)
- Predecessor: [`devlog/2026-08-18-cp2-runtime-binding-query/`](../2026-08-18-cp2-runtime-binding-query/index.md)
