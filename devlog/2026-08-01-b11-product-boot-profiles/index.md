# B11 — Product boot profiles exclude verification scaffolding

| Field | Value |
|---|---|
| Date | 2026-08-01 |
| Kind | Defect |
| Status | Verified |
| Scope | `contracts/generation/v1/`, `scripts/build/build-generation.py`, `scripts/build/boot_layout.py`, `kernel/src/runtime/bootstrap.rs`, `components/bins/src/bin/init.rs`, `kernel/src/storage/block_device.rs`, `scripts/check/`, `Justfile` |
| Roadmap | B11 |
| Gates | `just product_boot_check`, `just boot_layout_check`, `just storage_write_check`, `just storage_fault_check`, `just storage_store_check` |
| Trigger | B11: the only generation manifest declared the sixteen named verification probes/scenario doubles plus `storage-writer` as peers of product services, with real capabilities and one probe in the health policy. |
| Baseline | `valid.zti` declared 42 components in one shape; every boot received the verification participants, and the default health policy required `storage-probe`. |

## Summary

The generation builder now resolves one named boot profile before encoding any
component, object, grant, state binding, shared-buffer budget, health policy, or
fabric graph. The `default` profile is the product profile and declares no
verification scaffolding; `test`, `visibility`, and `unified` explicitly declare
the probes and scenario doubles their gates exercise. The product generation
boots a healthy vertical slice in 45 capability slots while naming none of the
seventeen test-only components. Existing test profiles retain their prior
authenticated fabric graphs and all eighteen pre-B11 boot-layout fixtures.

## Observable symptom

- Command: build generation 1 without a gate-specific profile and inspect the authenticated manifest and boot layout.
- Expected: the product generation declares only product components and product health requirements.
- Observed: the only manifest shape declared seventeen probes/scenario doubles with real grants; `storage-probe` was a required health component.
- Exit/fault/serial evidence: before this change, the default generation included `storage-writer` plus the sixteen components B11 enumerated. After the change, `just product_boot_check` reports a healthy 45-slot product boot with none of the seventeen names.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | B10 moved capability slot identity into the generation-derived boot-layout resource and froze eighteen existing layouts. | B11 could change a profile's component set without reintroducing positional slot constants. |
| 2 | The existing `SLIME_FABRIC_PROFILE` selector governed only fabric interposition; all components, objects, grants, budgets, and health entries remained global. | Component selection had to extend that selector rather than add a second profile axis. |
| 3 | Resolving a component set requires a closed reduction over objects, grants, state owners, budget holders, fabric participants, interposition hops, health entries, and boot-layout labels. | The builder narrows the manifest once, before every emitted resource consumes it. |
| 4 | A product boot with the narrowed manifest reached `[generation] vertical slice healthy` in 45 slots and named none of the seventeen test-only components. | The product path satisfies B11 directly rather than by inference from a manifest diff. |
| 5 | Explicitly exercising the storage profiles exposed latent gate bugs: debug builds hung, candidate generations were not marked pending, init always asked for the read-probe label, generation 4 asked for the absent `storage-capability` role, and injected failures were collapsed into fatal transport errors. | The storage gates now use bounded release boots, build a matching pending BootState, select the executable and authority roles from generated layout constants, and preserve non-fatal injected-fault recovery semantics. |

## Root cause

The manifest had one global component graph and one global health policy. Its
named fabric profiles replaced only interposition chains, so selecting `default`,
`visibility`, or `unified` could not remove a component, its executable object,
its grants, its resource budget, or its health requirement. Verification
participants therefore crossed the same authenticated generation boundary as
product services even when no product path used them.

The storage-gate defects were masked by that global shape. The old debug kernel
failed to finish on this host, so the scenario executables were not observed.
Once bounded release boots made the gates executable, the generated layout
showed that generations 2–4 name `storage-writer`, `storage-fault-probe`, and
`storage-store-probe`, not `storage-probe`; generation 4's authority is the first
`object-store` role, not a `storage-capability` role. The block layer also mapped
`VirtioBlkError::InjectedFailure` to the same generic variant as a fatal device
error, causing a synthetic request failure to discard a healthy device before
replay.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Generation contract | Added a versioned Zutai `BootProfile` carrying the component extension, required health set, and linked fabric profile. | One authenticated profile selector owns both component shape and fabric interposition. |
| Generation builder | Resolves the selected profile to a closed manifest before component images, grants, health, fabric graph, and boot layout are encoded. | A dropped component leaves no executable object, authority, budget, health edge, or fabric participant behind. |
| Product/test profiles | `default` declares no scaffolding; `test`, `visibility`, and `unified` declare only the scenario participants they need. | Product authority excludes test participants; gates opt in explicitly. |
| Boot layout/kernel bootstrap | Product-only layouts compact surviving slots; placement tolerates profile-absent scaffolding while still rejecting declared-but-unfilled slots and rights mismatches. | The product table has no holes or undeclared capabilities, while existing profile tables remain byte-identical. |
| Init | Uses generated layout constants for executable, endpoint, storage, and service roles; storage scenarios select their present executable and generation 4's first object-store role. | Component-side slot use follows the same resolved layout the kernel places. |
| Gate selection | Probe-dependent checks set `SLIME_FABRIC_PROFILE=test` (or their explicit visibility/unified profile); `product_boot_check` clears gate flags and selects `default`. | A gate cannot pass by accidentally booting a different participant set. |
| Storage checks | Use bounded release kernels and pending-generation BootState fixtures for generations 2–4. | Each storage probe reaches its observable scenario instead of hanging or failing health confirmation first. |
| Block recovery | Distinguishes injected device/timeout statuses from fatal transport errors while preserving their wire status. | Deterministic failure/replay does not discard a healthy virtio device; real fatal errors still reinitialize it. |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Product generation regains a probe or scenario double | `just product_boot_check` | A scaffolding name appears in the transcript/layout, or the product slice fails to become healthy. |
| Existing gate layouts drift while product slots are removed | `just boot_layout_check` | Any of eighteen frozen fixtures differs, or the new product fixture differs from the resolved table. |
| A storage profile selects the wrong executable or authority role | `just storage_write_check`, `just storage_store_check` | Missing writer/store markers or `spawn rejected BadCapability`. |
| Injected failures poison the cached device before replay | `just storage_fault_check` | Replay/init failure or a missing `recovery and replay verified` marker. |
| Manifest filtering leaves dangling authority or nondeterministic output | `just contracts_check`, `just generation_check` | Contract validation or byte-identical generation comparison fails. |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just product_boot_check` | healthy vertical slice in 45 capability slots; none of the seventeen test-only components declared | Direct |
| `just boot_layout_check` | all nineteen profile/layout pairs pass; the eighteen existing fixtures remain unchanged | Direct |
| `just contracts_check`, `just generation_check` | pass; repeated generation builds are byte-identical | Direct |
| `just test` | 189 kernel assertions pass with the explicit `test` boot profile | Direct |
| `just dango_check`, `just directory_check`, `just powerbox_check`, `just sample_plane_live_check` | pass with their explicitly selected scaffolding | Direct |
| `just fabric_manifest_check`, `just fabric_authority_check`, `just fabric_stream_check`, `just fabric_qos_check`, `just fabric_call_check`, `just fabric_operation_check`, `just fabric_visibility_check`, `just data_fabric_boot_check` | pass; pre-existing test/visibility/unified graph behavior remains observable | Direct |
| `just storage_read_check`, `just storage_nvme_read_check`, `just storage_write_check`, `just storage_fault_check`, `just storage_store_check` | pass; read, durable write, replay/recovery, and object-store scenarios launch their declared probe | Direct |
| `just generation_cmd_check`, `just spawn_service_check`, `just rollback_check`, `just bootstate_trace_check`, `just transfer_check` | pass with explicit profile selection where scaffolding is required | Direct |
| `just framework_safety_check` | pass; storage scenario selection remains generation/layout-driven | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos`, `just devlog_check` | pass | Direct |

## Decisions

- Decision: extend the existing `SLIME_FABRIC_PROFILE` mechanism into a boot profile rather than add a second component-profile selector.
- Rationale: one name now selects the component set and the fabric interposition override together, so the authenticated graph cannot combine independently drifting shapes.
- Rejected alternative: a second test-generation manifest. It would duplicate routes, QoS, grants, budgets, and health policy and permit product/test definitions to drift.

- Decision: retain a superset source fixture and resolve it before encoding rather than split every declaration into profile-local blocks.
- Rationale: references remain authored once, while the builder proves the selected closure and emits only reachable declarations.
- Rejected alternative: duplicate complete component/grant/object lists per profile. That increases review surface and makes equality of shared product authority an authoring convention.

- Decision: compact slots only when a profile drops components; preserve every existing layout byte-for-byte otherwise.
- Rationale: the product generation should not carry dead holes for undeclared test authority, while B10's frozen fixtures remain the regression oracle for all existing gates.
- Rejected alternative: renumber all layouts. It would rewrite the evidence B10 preserved and obscure whether a gate still receives the same authority.

- Decision: synthetic block failures carry the same wire status as real failures but a separate recovery class.
- Rationale: the fault gate tests request recording and recovery without physically damaging the virtio transport. Conflating status with transport liveness made replay impossible.
- Rejected alternative: suppress reinitialization in the gate. Recovery semantics belong in the common block layer, not in a test-only caller branch.

## Open risks and follow-ups

- [ ] `SLIME_FABRIC_PROFILE` retains its historical name although it now selects the full boot profile. Renaming it is a cross-repository interface change and is out of B11 scope.
- [ ] Boot profiles are authored in one superset fixture. Contract and determinism gates prove the selected output, but a future author can still add a new scaffolding component without assigning it to a non-product profile; `product_boot_check` covers every component currently assigned to a non-product profile, while schema-side classification of future names is a separate design question.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none.
- Serial/debugger/model output: `contracts/boot-layout/v1/fixtures/product.layout` and the `[layout]`/health markers consumed by `scripts/check/check-product-boot.py`.
- Related roadmap item: `roadmap/00-backlog.md` B11; prerequisite change in `devlog/2026-08-01-boot-layout-resolution/`; diagnosis in `devlog/2026-07-31-boot-layout-positional-coupling/`.
