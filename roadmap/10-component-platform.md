# Component specification and out-of-tree development track

**Purpose:** Give "component" and "system" a formal, versioned specification independent of any one generation manifest, derive generation inputs from that specification, support components authored, built, released, and upgraded outside this repository through a reproducible SDK whose source remains owned here, and close the remaining host-build gap with one canonical system-image closure that deterministically produces a bootable image. This track is the repository-side implementation of the Component Model described in `spec/requirement-document-v0.6.md` §2.1 and the Canonical IR / Component CLI phases of `spec/platform-development-plan-v0.6.md`, scoped to the existing seL4 product path rather than the full platform that document describes.

**Status:** CP0–CP11 complete. CP12 is substantially delivered — 40 of the 42 seL4 compositions are derived from system specs, and the remaining 2 are recorded with closed reasons by `contracts/composition-inventory/v1`. CP13 is delivered for the closure entry point: one generic command builds any of 44 generated closures with no plane flag, variant table, or composition named in builder source; its legacy-deletion clause belongs to CP15. CP14 is delivered in full: the ambient generation overrides, the executable-changing component scenarios, the root roles and their platform-qualified instrumentation, the B40 negative build cases, and the 45 frozen test-run records are all closed, typed, identity-bearing data. CP15 is partially delivered: 36 of 49 seL4 plane gates build by closure identity, `just system_image_closure_aggregate_check` closes both drift directions and is green, and the eight plane flags migration made unreachable are deleted; 13 gates, the rest of the legacy surface, and the SDK publication clause remain open. CP7's hosted-publication clause closed 2026-08-26: SDK 1.0.0 and 1.1.0 are immutable commits and signed tags on the canonical repository, both recording a source commit present on `origin/main`. `slime_os` stays authoritative; the SDK is a generated one-way mirror described by `contracts/component-sdk-release/v1`.

**Closes:** Backlog item **B70**, whose problem statement is the compile-time coupling this track removes (CP0–CP2).

**Motivating gap (confirmed 2026-08-17):** every one of `components/bins`'s 52 `[[bin]]` entries (`components/bins/Cargo.toml`, `autobins = false`) compiles against four files `components/bins/build.rs` privately generates from `contracts/generation-manifest/v1/fixtures/*.zti` by ad hoc string parsing, through three generator functions (`generate_boot_layout` at lines 57–66, `generate_command_profile` at lines 68–175 emitting both `command_profile.rs` and `dango_profile.rs`, `generate_fabric_profile` at lines 177–197) and `include!`s directly into component source at 19 sites across 17 files (confirmed at `components/bins/src/bin/spawn-service.rs:32`, `init.rs:37,118`, `fabric-publisher.rs:50`, and fourteen further files). `scripts/build/build-generation.py` builds every component with one command, `cargo build --release --target <profile> -p slime-components --bin <name> ...` (confirmed at lines 2413–2461), with no parameter accepting a component built anywhere else. The root-side admission path (`slime-root/src/generation.rs`, `child_vspace.rs`) needs no change to admit a new component; every blocker is in the host build pipeline and the single-crate `[[bin]]` design.

## Boundaries

- This track does not change seL4 capability enforcement, generation admission, or ELF structural validation; `slime-root/src/generation.rs` and `child_vspace.rs` are confirmed producer-agnostic already and CP0–CP5 add no code there beyond CP2's one new root-served query.
- CP4 introduces no per-component signature, provenance record, or new trust root. A generation containing an externally supplied component is trusted exactly as every generation is today, by the existing whole-generation release signature (`boot-contracts/src/release.rs::INITIAL_TRUST_ROOT`). Per-component provenance is separate future work, not part of this track.
- This track is independent of [D1–D7](08-native-development.md). D4's `ExecutableFactory`/`EXECUTABLE_ADMIT` authority is a *runtime*, in-booted-system, ephemeral admission mechanism for un-persisted dev-loop execution; CP4 is a *host-side, build-time* admission mechanism that produces an ordinary, persistent, normally signed generation. Neither depends on the other, and neither milestone should be implemented in terms of the other's mechanism.
- CP5's "separate checkout" means a genuinely distinct git repository, not a new directory inside this repository's Cargo workspace. CP3's in-repo crate split is a necessary mechanical precondition but is explicitly not sufficient by itself to claim components can be developed "out-of-tree" — that claim is CP5's alone, mirroring how this repository already refuses to conflate QEMU evidence with physical-board evidence elsewhere in this roadmap.
- CP5's completed exit condition covered a temporary pinned SDK bundle and the RP4 QEMU data path only. CP6–CP10 extended that evidence to a repository-owned deterministic exporter, a publication path first proven against a local clone of the canonical repository and since exercised against the hosted repository itself, downloadable per-profile platform build inputs, evidence-backed compatibility declarations, and a pinned consumer upgrade and rollback path. They did not retroactively widen CP5's claim.
- CP1 intentionally proved spec derivation on `valid.zti` and `sel4-channel.zti` without converting every remaining `sel4-*.zti` fixture. CP12 owns that complete migration. Until CP15 cuts over the final caller, the existing composition files and plane flags remain compatibility inputs rather than a second permanent convention.
- CP6–CP10 do not move authoritative runtime, protocol, contract, target, or toolchain sources out of this repository. The SDK repository is a generated release mirror with one-way synchronization from `slime_os`; fixes land here first and are exported, never patched independently in the mirror.
- SDK release metadata is a persisted cross-repository format and therefore remains a versioned Zutai contract under `contracts/`. The SDK adds no per-component signature, provenance trust root, or root-side admission path: ELF content hashes and the existing whole-generation release signature remain authoritative.
- Publishing a `bcm2712-rpi5` prefix proves that an external component can be built and target-qualified against that exact platform input. It does not claim physical Raspberry Pi 5 boot support; only the existing physical-board gates can make that claim.
- A system-image closure describes reproducible build inputs and their identities; it does not absorb the test oracle. QEMU arguments, disk/network fixtures, injected device behavior, timeouts, expected markers, and forbidden markers belong to a separate versioned test-run contract that references the image closure it exercises.
- Scenario or fault injection that changes executable bytes is a distinct component or root implementation identity in the closure. It is never an ambient environment switch omitted from the build key. Runtime test data that does not change image bytes stays in the test-run contract.
- CP11–CP15 are host-side build and verification work. They do not implement D1–D7's in-system compiler, executable admission, or live update path, and they do not widen any physical-board claim.

## Sequencing

1. CP0 and CP2 have no dependency on each other and may proceed in parallel once the backlog is clear: CP0 is a host-side contract/schema change, CP2 is a root/component runtime change.
2. CP1 depends on CP0 and re-derives the generation fixtures from the component/system specs it defines.
3. CP3 depends on CP2's runtime binding resolution: splitting components into independent crates is only useful once no crate-shared `build.rs` output is required at compile time.
4. CP4 depends on CP0 (a place to declare a component's implementation as workspace-built or externally supplied), CP2 (so an externally built component needs no `OUT_DIR`-generated file), and CP3 (the crate convention an external build reproduces).
5. CP5 depends on CP3 and CP4, and is what [RP4](09-rpi5-ros2-demo.md#rp4--arm-component-data-path-on-qemu-and-raspberry-pi-5) now requires before its own exit condition is satisfied.
6. CP6 depends on CP5 and turns its temporary bundle construction into one deterministic, contract-described exporter inside this repository.
7. CP7 depends on CP6 and publishes the exporter output to one permanent, bot-owned SDK repository; publication never runs from untrusted pull-request code.
8. CP8 depends on CP6 and CP7 and publishes the platform-specific seL4 prefixes that an external build otherwise has to borrow from a `slime_os` checkout.
9. CP9 depends on CP7 and CP8 and defines release versioning and the tested compatibility matrix; an untested cross-release pairing remains unsupported rather than inferred from SemVer.
10. CP10 depends on CP9 and proves the consumer-side pin, update, generation rebuild, boot, and rollback workflow across two immutable SDK releases.
11. CP11 depends on CP1 and CP8: the logical system model and the target-specific platform inputs already exist, so this slice defines the closure that binds them without duplicating either contract.
12. CP12 depends on CP11 and converts every current seL4 test composition to a spec-derived closure while preserving each resolved generation byte-for-byte.
13. CP13 depends on CP12 and replaces the builder's plane flags, variant tables, and source-owned output paths with the generic closure entry point.
14. CP14 depends on CP13 and makes every remaining generation delta, compile-time scenario, selector role, physical instrumentation profile, and negative mutation an explicit identity-bearing closure or test-run input.
15. CP15 depends on CP14 and cuts every QEMU checker and SDK consumer over to closure identities, deletes the legacy build surface, and proves the complete corpus reproducible from clean roots.

## CP0 — Component specification model

**Status:** Complete.

**Delivered:** `contracts/component-spec/v1` declares a bounded, closed-vocabulary `ComponentSpec` covering the twelve sections `spec/requirement-document-v0.6.md` §2.1 names, with 42 records — one per component `contracts/generation-manifest/v1/fixtures/valid.zti` declares — each cross-checked field by field against that manifest and its fabric graph, and each identified by `contracts/interface-schema/v1`'s own normalization convention rather than a second one.

**Exit condition (observed):** `just component_spec_check` validates all 42 records against the 4 declared interfaces and the reference generation, refusing 37 named malformations with stable identities, and reports the two components the repository declares but ships no implementation for (`generation-list`, `storage-store-probe`); `just contracts_check` type-checks the new contract and its generated bindings.

**Gates:** `just component_spec_check`, `just contracts_check`.

**Evidence:** [`devlog/2026-08-18-cp0-component-spec-model/`](../devlog/2026-08-18-cp0-component-spec-model/index.md)

## CP1 — System specification model and generation derivation

**Status:** Complete.

**Delivered:** `contracts/system-spec/v1` declares a composition — its components, authority edges, notifications, fabric graph, and boot profiles — and `scripts/generate/generate-generation-from-spec.py` derives a `contracts/generation-manifest/v1` manifest's `executables`, `instances`, `objects`, `sharedBufferBudget`, and `health.requiredInstances` from it plus the CP0 component corpus. Reaching byte equivalence required sorting `declared_spawn_grant_counts`, whose raw-manifest-order output reached `init`'s compiled `FABRIC_MINTED_GRANTS` and so made instance order change a component ELF.

**Exit condition (observed):** `contracts/generation-manifest/v1/fixtures/valid.zti` and `sel4-channel.zti` are generator output, regenerate byte-identically under `--check`, and reproduce the frozen pre-CP1 baselines; building `sel4-channel` from the derived fixture yields byte-identical `generation.bin` and `boot-store.bin`, and `sel4_channel_check`, `sel4_component_graph_check`, `sel4_boot_check`, `sel4_generation_check`, `sel4_dango_check`, and `sel4_boot_layout_check` (25 plane layouts) all pass on QEMU.

**Gates:** `just system_spec_check`, `just contracts_check`, `just generation_check`, `just sel4_boot_check`, `just sel4_generation_check`.

**Evidence:** [`devlog/2026-08-18-cp1-generation-derivation/`](../devlog/2026-08-18-cp1-generation-derivation/index.md)

## CP2 — Runtime-resolved component binding

**Status:** Complete. The query surface answers grant bindings, namespaced boot-layout roles, unambiguous capability roles, the fabric-graph read, and the generation's boot action, and the site-by-site migration finished on 2026-08-22: no component source `include!`s a `build.rs`-private, manifest-derived constant table. The nine `fabric_profile` sites that no query could retire — their symbols size fixed arrays at compile time — split into authenticated `fabric-graph` header fields (`trace_depth`, `trace_overflow`, query ids 24/25), existing `RuntimeLimits` ceilings, and published `contracts/fabric-graph/v1` storage constants. `render_fabric_profile_rust`, the `SLIME_DATA_FABRIC_PROFILE` handoff, both command-table generators, and `components/build-support`'s manifest parser are deleted.

**Progress (2026-08-21, boot action):** `CAPABILITY BOOT ACTION` (label 40) answers `BootAction`'s frozen id, so a component reads which composition it was booted into instead of `include!`ing a `build.rs`-private per-plane string. Five sites migrated and six `include!`s deleted, taking the all-profile count from 15 to 9. Gated on the *lifecycle* service rather than the capability table its label namespace names: the query must be answerable to every launched instance, and 30 of the 182 instances the seL4 fixtures declare hold no capability-transfer service where 0 lack lifecycle. **Evidence:** [`devlog/2026-08-21-b70-boot-action-query/`](../devlog/2026-08-21-b70-boot-action-query/index.md)

**Closure (2026-08-22):** [`devlog/2026-08-22-b70-profile-include-closure/`](../devlog/2026-08-22-b70-profile-include-closure/index.md)

**Depends on:** Cleared or explicitly deferred backlog. Independent of CP0/CP1.

**Delivered (2026-08-18):** `contracts/syscall-abi/v1` declares
`CAPABILITY RESOLVE BINDING` (label 37). The root answers it from the calling
instance's *own* `InstanceBinding` list, scoped by the badge it authenticated,
with the name bounded and UTF-8-validated before any table walk. A prefixed name
— `executable:` or `channel:` — additionally reaches
`contracts/boot-layout/v1`'s resource object for the bootstrap instance, so the
61 slots that are layout roles rather than manifest grants are runtime-resolvable
too. `slime-rt` exposes `resolve_binding`; `docs/syscall-abi.md` and
`docs/capability-matrix.md` document it per invariant 4;
`just runtime_binding_resolution_check` guards it.

**Correction (same day):** an earlier revision of this entry recorded the layout
half as *blocked* on a `contracts/boot-layout/v1` format change. That was wrong,
and the error is worth keeping because two unsound attempts preceded the fix. An
unprefixed fallback to the layout was written twice — once for any caller, once
restricted to the bootstrap instance — and both failed on real boots, because the
layout and the grant list use overlapping names for different things: `console` is
a layout executable at slot 1 while `init-console` is a grant at the same slot,
and `console-output` is a grant under one generation and absent from the layout
under another. A flat lookup therefore answered a channel question with an
executable slot and `init` sent into an endpoint nobody was waiting on. The fix
needed no format change: the caller states which table it means, using the two
identity domains the contract already declares to keep a component and a channel
sharing a name apart. An unprefixed name is a grant and can never reach the
layout, so no layout entry can shadow a grant.

**Progress (2026-08-18, role axis):** a third query axis,
`kind:<capabilityKind>` or `kind:<capabilityKind>+<right>,<right>`, resolves a
caller's own binding by what the capability *is* rather than by its grant name.
This exists because grant names are not stable across generations and so cannot
be written into a component: `spawn-service`'s shared-buffer factory grant is
`spawn-service-shared-buffer-factory` under `valid.zti` and its RPC endpoint is
`spawn-service-rpc` there but `dango-e-spawn-service-rpc` under
`sel4-dango.zti`. Kind and rights are properties of the capability instead, and
`components/bins/build.rs` already asked exactly this question of the manifest
(`binding_with_right_slot`, `related_binding_slot`) before this axis let the root
answer it at runtime; those two now-unused derivations were removed rather than
left to drift.

`spawn-service` resolves its shared-buffer factory slot this way. Its RPC
endpoint is deliberately *not* migrated: `sel4-dango.zti` grants it three
`send`+`recv` endpoints (the RPC channel plus one context endpoint per command),
so the role is ambiguous there and the query refuses it rather than guessing —
observed directly: resolving it hung the dango plane at `dango> $(sysinfo)`
until the query was left refusing and `RPC_SLOT` restored to the generated
table. Ambiguity refusal, not a lowest-slot tiebreak, is the same discipline the
boot-layout namespace fix above encodes: a plausible wrong answer is worse than
no answer.

**Remaining:** `spawn-service`'s RPC endpoint and its two command executables
(`COMMAND_PROFILE`) stay generated, because "the endpoint that carries requests"
and "the executable this command name spawns" are graph-shape facts a
kind/rights query cannot distinguish, not properties of one capability;
resolving them needs a binding to carry a stable logical role, which is a
`contracts/generation-manifest/v1` format change. `init`'s remaining ~134 boot-layout
constants (of 136; 2 migrated) are the same fabric-profile-shaped question as
`fabric_profile`'s 46 non-slot constants — the `fabric_profile` sites are a
distinct case: 46 of its 64 constants are graph facts (route tables, QoS depths,
trace depth) rather than slots, so a slot-resolution query is the wrong
instrument and those components should read the authenticated `fabric-graph`
resource instead.

### Deliverables

- add a root-served query operation that, given a binding name declared for the calling instance in the active generation manifest, returns the CSpace/notification slot number the root already assigned that instance during activation — authenticated by the caller's badge, the same way `contracts/capability-transfer/v1`'s `FabricRequest` ignores caller-supplied identity and authenticates by which endpoint the request arrived on, not by a self-reported name;
- update `docs/capability-matrix.md` and `docs/syscall-abi.md` in the same change per this roadmap's invariant 4, and extend the `just contracts_check` label-coverage gate this requires;
- migrate the four files `components/bins/build.rs` currently generates into `OUT_DIR` (`command_profile.rs`, `dango_profile.rs`, `fabric_profile.rs`, `boot_layout.rs`) so their consumers call the new runtime query at startup instead of reading compile-time `include!`d constants; retain the underlying manifest data only as a root-side/builder implementation detail;
- migrate the 19 confirmed `include!` call sites across 17 files one-for-one to the runtime query, preserving each component's observed behavior: `components/bins/src/{call_broker.rs, fabric_boot.rs, fabric_call_scenario.rs, fabric_matrix.rs, operation_broker.rs}` and `components/bins/src/bin/{console.rs, dango.rs, fabric-intruder.rs, fabric-publisher.rs, fabric-publisher-b.rs, fabric-subscriber.rs, fabric-subscriber-b.rs, fabric-service.rs, init.rs, sample-lender.rs, sample-receiver.rs, spawn-service.rs}`.

### Required checks

- an instance querying a binding name it was not granted receives a structured denial, never another instance's slot;
- every migrated component passes its existing `just sel4_*_check` gate with unchanged observed behavior;
- a component crate built with no `OUT_DIR`-generated file at all still resolves its bindings correctly at runtime, proving the compile-time coupling is gone.

### Planned verification target

```sh
just runtime_binding_resolution_check
```

(plus the existing full `just sel4_*_check` gate suite)

### Exit condition

No component source file `include!`s a `build.rs`-private, manifest-derived constant table; every component resolves its own bindings at startup through a documented, gated, root-served query instead.

## CP3 — Crate-per-component SDK boundary

**Status:** Complete.

**Delivered:** `components/bins` is 52 independent workspace packages, one per component, each with its own `Cargo.toml`, `build.rs`, and `src/main.rs`; the shared helpers are the `slime-components` library at `components/lib`, and the generation-manifest parser that was private to the old crate's build script is the documented `slime-build-support` crate any component crate depends on from `[build-dependencies]`. `build_rust_components()` builds `-p slime-component-<name>` in two invocations grouped by feature set, which is what actually scopes the allocator: Cargo unifies features across every package in one invocation, so a plain component built beside a store component gained the heap too — measured as 6 heap symbols in the linked `slime-rt` rlib against 0 when grouped. `docs/syscall-abi.md` states the v1 compatibility policy (frozen labels and statuses, additive-only growth, retired numbers reserved, an incompatible change is a new major contract version).

**Exit condition (observed):** all 33 `just sel4_*_check` plane gates and `just sel4_gate_control_check` pass with unchanged behavior; a new component was added as one directory, built to an ELF, and removed with no edit to any other crate; a plain component's shipped ELF carries 0 allocator symbols against a store component's 4; `just generation_check` is byte-identical across two isolated builds; `just component_crate_split_check` pins the split's six properties, each proven to fail under a one-line perturbation. The [B65 deferred follow-up](00-backlog.md) ("the 52-binary fixture population uncollapsed") is closed by this milestone.

**Amended deliverable:** the "preserving byte-identical output for every existing in-tree component" clause was unachievable and is recorded as amended rather than met. Cargo's `-C metadata` hash derives from the package name and appears in CGU symbol names inside the shipped `.symtab`, so renaming a package necessarily moves the ELF — measured as 15 differing bytes, all inside that string, with source, bin name, target, and target directory held fixed. No repository artifact pins those bytes. Determinism plus unchanged gate behavior is the property that replaced it, and both are observed above.

**Gates:** `just component_crate_split_check`, `just lint_all`, `just fmt_check_all`, `just machete`, `just test_host`, `just generation_check`, the 33 `just sel4_*_check` planes, `just sel4_gate_control_check`.

**Evidence:** [`devlog/2026-08-21-cp3-crate-per-component/`](../devlog/2026-08-21-cp3-crate-per-component/index.md)

## CP4 — External-artifact admission path

**Status:** Complete.

**Depends on:** CP0, CP2, CP3.

### Deliverables

- add an `implementation` field to `contracts/component-spec/v1`'s `ComponentSpec` naming a component's build source as `workspace` (built by this repository's `cargo build`) or `external` (bytes supplied out-of-band, identified by content hash), so the generation builder decides which path to use without inferring it;
- extend `scripts/build/build-generation.py`'s component-build step to accept a `name -> external ELF path` mapping for every `component-spec` marked `external`, and call the existing `elf_component_image()` on the supplied bytes exactly as it already does for workspace-built ELFs — that function is already producer-agnostic once handed bytes;
- reuse the existing whole-generation release signing (`boot-contracts/src/release.rs::INITIAL_TRUST_ROOT`) as the sole trust boundary for a generation containing an externally sourced component; add no new per-component signature or trust root;
- record, in generation build output, which components in a given build came from the external path, for operator visibility during this milestone's own verification (not a new persistent contract).

### Required checks

- an externally supplied component image that fails `boot_contracts::component_image::admit`'s existing structural/target-qualification checks is rejected at build time, before signing, exactly as a workspace-built one would be;
- a generation mixing workspace-built and externally supplied components boots and passes the same gates as an all-workspace generation;
- `slime-root/src/generation.rs` and `child_vspace.rs` require zero code changes, confirming the root-side admission path is producer-agnostic as observed in this track's motivating investigation.

### Planned verification target

```sh
just external_component_admission_check
```

### Exit condition

`scripts/build/build-generation.py` packages a generation containing at least one component whose ELF bytes were not produced by `cargo build` in this workspace, and that generation boots and passes its declared gates identically to an all-workspace build.

**Delivered:** component specs now select `workspace`, `external`, or `undeclared` implementations and bind an external implementation to the SHA-256 of its bare ELF. `build-generation.py` accepts an explicit implementation-name-to-ELF mapping, builds only workspace implementations, verifies each external digest, applies the same bounded ELF/target/W^X checks the root loader requires before release signing, and reports every selected source. `build-sel4.py` can embed the exact checked generation in the component-graph image without rebuilding it. No per-component signature, provenance record, or new trust root was added.

**Exit condition (observed):** `just external_component_admission_check` independently built `console` from a temporary crate outside this Cargo workspace, verified its ELF differed from the workspace artifact, mixed it with workspace components, signed and host-admitted the generation, embedded that exact generation in the seL4 component-graph image, and passed the existing QEMU graph gate. Hash-mismatched and five structurally invalid external ELFs were refused before either signed artifact existed. `slime-root/src/generation.rs` and `slime-root/src/child_vspace.rs` are unchanged.

**Gates:** `just external_component_admission_check`, `just test_host`, `just lint_all`, `just fmt_check_all`, `just ruff`.

**Evidence:** [`devlog/2026-08-21-cp4-external-artifact-admission/`](../devlog/2026-08-21-cp4-external-artifact-admission/index.md)

## CP5 — Out-of-tree component development proof

**Status:** Complete.

**Depends on:** CP3, CP4.

### Deliverables

- publish, or provide as a pinned, git-consumable vendored bundle, `slime-rt`, `slime-proto`, `boot-contracts`, the seL4 JSON target spec, and the exact `sel4-sys`/`SEL4_PREFIX`/`LIBCLANG_PATH` toolchain recipe as a versioned component SDK a checkout outside this repository can consume by pinned commit;
- in a separate git repository (may be an unpublished scratch checkout used only for this milestone's verification), author [RP4](09-rpi5-ros2-demo.md#rp4--arm-component-data-path-on-qemu-and-raspberry-pi-5)'s two Arm data-path components (the bounded-C7/C8-sample producer and consumer) using only the SDK bundle above and CP3's crate-per-component convention, with zero commits or edits to this repository's `components/` tree;
- build both components from that separate checkout into target-qualified component images for `aarch64-sel4-qemu-virt`;
- feed both images through CP4's external-artifact admission path into a demo-scoped generation satisfying RP4's declared route/grant/capability requirements, sign it with the existing release trust root, and boot it.

### Required checks

- the separate checkout's build succeeds using only the published/vendored SDK commit and documented toolchain steps, with no path reference into this repository's `components/` directory;
- the resulting generation is admitted by the unmodified root-side path (`slime-root/src/generation.rs`, `child_vspace.rs`) with no root-side code change;
- the two out-of-tree components exchange the bounded C7/C8 sample under `aarch64-qemu-virt` with the same route-authority and resource-reclamation properties RP4's own required checks demand: the publisher cannot receive or re-delegate subscriber authority and the subscriber cannot publish unless explicitly granted; malformed descriptors, wrong type tags, quota exhaustion, peer death, and route denial fail closed and reclaim resources;
- removing the out-of-tree checkout and rebuilding the demo generation from in-tree fallback components still passes RP4's existing checks, proving the out-of-tree path is additive rather than a fork of the demo.

### Planned verification target

```sh
just component_sdk_out_of_tree_check
```

### Exit condition

Two RP4 data-path components, authored and built entirely in a separate git checkout against a published or vendored Slime component SDK, are admitted into a demo-scoped generation through CP4's external-artifact path and observed exchanging the bounded C7/C8 sample under `aarch64-qemu-virt`, with zero edits to this repository's `components/` tree.

**Delivered:** `just component_sdk_out_of_tree_check` materializes a versioned component SDK as a pinned git repository containing the public runtime/protocol/build crates, the pinned `rust-sel4` source, the AArch64 target specification, a generated `sel4-demo` fabric profile, and the exact toolchain recipe. A second git repository depends on that SDK commit, builds independent `fabric-publisher-b` and `fabric-subscriber` crates with no reference to this repository's `components/` tree, binds both ELFs by content hash through CP4, signs the mixed demo generation, embeds that exact generation, and boots it. The gate then deletes the external checkout and proves the ordinary in-tree demo still boots.

**Exit condition (observed):** the external baseline boot exchanged RP4's bounded large sample and retained the existing route-denial and reclamation assertions while additionally observing publisher re-delegation refusal and quota exhaustion. Three content-distinct external generations then proved producer peer death, malformed descriptor rejection, and wrong-type rejection; each failure arm reclaimed the rejected loan, and the rejection arms continued with a fresh valid terminal sample so the unchanged demo graph reached healthy completion. Cargo metadata proved every exported SDK crate resolved through the exact git revision, and neither `slime-root/src/generation.rs` nor `slime-root/src/child_vspace.rs` changed.

**Gates:** `just component_sdk_out_of_tree_check`, `just lint_all`, `just fmt_check_all`, `just machete`, `just test_host`, `just ruff`.

**Evidence:** [`devlog/2026-08-22-cp5-out-of-tree-component-sdk/`](../devlog/2026-08-22-cp5-out-of-tree-component-sdk/index.md)

## CP6 — Deterministic component SDK export

**Status:** Complete.

**Depends on:** CP5.

### Deliverables

- add `contracts/component-sdk-release/v1/schema.zt` as the source of truth for SDK release metadata, including the originating `slime_os` commit, exported-tree identity, Rust toolchain, seL4 and `rust-sel4` pins, target-spec identities, public contract-set identities, and the supported target profiles;
- replace CP5's test-local copy-and-patch logic with one repository-owned exporter that emits `boot-contracts`, `slime-rt`, `slime-proto`, `slime-components`, `slime-build-support`, the pinned `rust-sel4` source, target specifications, generated public bindings, a root workspace manifest, and the decoded release record from an explicit allowlist;
- preserve exported crate manifests byte-for-byte. The generated SDK workspace supplies the inherited lint and release-profile context instead of deleting `publish = false` or `[lints] workspace = true` after copying;
- exclude component implementations, generation fixtures, product compositions, signing keys, build outputs, and every manifest-derived per-plane table from the export;
- make CP5 consume this exporter so the candidate path and the eventual release path cannot construct different SDKs.

### Required checks

- two isolated exports from the same source tree are byte-identical and report the same exported-tree identity;
- changing any allowlisted public source or pin changes the identity, while an unrelated product-only file does not;
- the emitted release record decodes through the generated Zutai binding and every recorded digest matches the emitted bytes;
- Cargo metadata for a minimal external repository resolves every SDK crate inside the exported tree and no dependency escapes to the source `slime_os` checkout;
- a component built from a local git commit of the export enters the existing CP4 path and boots on the QEMU component graph.

### Planned verification target

```sh
just component_sdk_export_check
```

### Exit condition

One checked-in exporter, invoked twice from the same `slime_os` commit, produces byte-identical self-describing SDK trees; CP5 uses that exact output, and no test-local alternate bundle recipe survives.

**Delivered:** `contracts/component-sdk-release/v1` declares what an export is — the originating commit, the exported-tree identity, every exported crate and public contract identity, the pinned toolchain and sources, the per-profile platform build inputs, and the compatibility identities — and `scripts/lib/component_sdk.py` is the only thing that produces one. Digests are domain-separated over an explicit length-prefixed encoding rather than over `tar` bytes a host controls, and `treeIdentity` excludes exactly the three record files the contract names. Exported crate manifests are copied byte-for-byte: the generated SDK workspace supplies the `[workspace.lints]` and release-profile tables `[lints] workspace = true` resolves against, read from this repository's own root manifest. The public contract set is *derived* from the `@generated by` headers in the exported bytes rather than listed, so a binding that moves crates cannot silently leave a format out of the set compatibility is decided on. `just component_sdk_out_of_tree_check` consumes this exporter and its own bundle recipe is deleted.

**Exit condition (observed):** `just component_sdk_export_check` exported one source tree twice byte-identically with equal exported-tree and release identities; four sensitivity probes required an exported source and a pin to move the release identity and two product-only files to leave it unchanged; the emitted record decoded as `#valid` through the generated binding with all 5 crate and 39 contract digests recomputed against the emitted bytes; a minimal external repository resolved all five SDK crates through a pinned git commit of the export with nothing escaping to this checkout; and a component built from that commit entered CP4's path and booted the QEMU component graph. Three malformed export requests were refused.

**Gates:** `just component_sdk_export_check`, `just contracts_check`, `just component_sdk_out_of_tree_check`, `just lint_all`, `just fmt_check_all`, `just ruff`.

**Evidence:** [`devlog/2026-08-25-cp6-cp10-component-sdk-releases/`](../devlog/2026-08-25-cp6-cp10-component-sdk-releases/index.md)

## CP7 — Permanent SDK repository and one-way publication

**Status:** Complete. The hosted-publication clause deferred at first landing closed 2026-08-26 and is recorded below.

**Depends on:** CP6.

**Repository:** [`iceice666/slime_os-component_sdk`](https://github.com/iceice666/slime_os-component_sdk). Created empty on 2026-08-25; CP6 must define the reproducible contents before CP7 creates its first generated commit.

### Deliverables

- use [`iceice666/slime_os-component_sdk`](https://github.com/iceice666/slime_os-component_sdk) as the canonical generated SDK repository; its generated branch and release tags are writable only by the release identity, with force-push and direct human edits disabled;
- publish only from an exact protected `slime_os` commit after CP6 and the existing external-component gates pass; pull-request jobs may build a candidate export but hold no credential capable of writing the SDK repository;
- write one generated commit carrying the complete SDK tree and its originating `slime_os` commit, then create an immutable signed `sdk-v<version>` tag; external components continue to pin the full commit rather than a branch or movable tag;
- make publication idempotent: an unchanged exported-tree identity creates no new SDK commit or tag, and a changed tree cannot reuse an existing version;
- add a reverse drift check that clones the permanent SDK commit, regenerates it from the recorded `slime_os` source commit, and refuses any byte difference. Corrections are made in `slime_os` and republished, never committed directly to the generated mirror.

### Required checks

- a fresh clone of the permanent SDK repository builds the external fixture with every SDK dependency resolved through the pinned SDK commit and no path into a local `slime_os` checkout;
- publication refuses a dirty source tree, an unverified source commit, a mismatched release record, a reused version, and an SDK tree containing a non-allowlisted file;
- the permanent commit regenerates byte-identically from the source commit named by its release record;
- the permanently hosted commit, not a temporary stand-in, supplies an external ELF that enters a signed generation and passes the QEMU component graph gate;
- deletion of the SDK clone does not affect the ordinary in-tree component build or boot.

### Planned verification target

```sh
just component_sdk_release_check
```

### Exit condition

An immutable SDK commit and signed tag exist in the canonical repository, regenerate exactly from their recorded `slime_os` commit, and build a QEMU-booted external component without any source or dependency path into this checkout.

**Delivered:** `scripts/build/publish-component-sdk.py` is the release path. It exports a *detached worktree of the commit it records* rather than the working tree, which is what makes a recorded `sourceCommit` a reproducible claim instead of a label, and it writes at most one generated commit plus one immutable signed `sdk-v<version>` tag whose message names both the source commit and the exported-tree identity. Idempotence is decided by the exported-tree identity rather than by a diff: CP6 makes two exports of one commit byte-identical, so an unchanged identity means there is nothing to publish. `--sdk-repository` is separate from `--sdk-url`, so the recorded canonical repository is release identity while the URL is only transport. Tags are signed through the repository's existing Ed25519 SSH release trust root rather than a second key format.

**Exit condition (observed):** `just component_sdk_release_check` published SDK 1.0.0 as one commit and one signed tag naming source `a8c73ff`; republishing the unchanged tree wrote no commit and left the branch at one; a dirty exported set under a defaulted source commit, an unverified source commit, a reused version with a changed tree, and an SDK tree carrying a non-allowlisted file were each refused with a distinguishing message; the published commit regenerated byte-identically from the source commit its own record names, while a one-line hand edit to the mirror's README was refused; a fresh clone of the published commit resolved all five SDK crates through it, built an external component that entered a signed generation, and booted the QEMU component graph; and with every SDK clone deleted the ordinary in-tree component graph still built and booted.

**Deferred clause (closed 2026-08-26):** the required check naming "the permanently hosted commit, not a temporary stand-in" is now met. SDK 1.0.0 (`5fee7b1`) and 1.1.0 (`31742d1`) exist as immutable commits and signed tags on [`iceice666/slime_os-component_sdk`](https://github.com/iceice666/slime_os-component_sdk)'s `generated` branch, published by `scripts/build/publish-component-sdk.py` from the release machine `m3air`, and both record source commit `726ebb0`, which is present on `origin/main` — so anyone holding that commit can regenerate them, which was the whole reason the clause existed. Both tags verify against a release signing key held outside this repository rather than `contracts/release/v1/test-keys`, and the hosted `generated` branch regenerated byte-identically from its own recorded source commit. What was configured out of band is now itself observed rather than asserted: the hosted `generated` branch and `sdk-v*` tags carry active rulesets whose only bypass actor is the release deploy key, an ordinary account credential was refused on both ref paths, and that deploy key is accepted by the SDK repository alone. The publisher also sends the branch commit and its release tag in one atomic push, so a refused tag cannot strand a published tree; `just component_sdk_release_check` proves that against a remote that rejects only release tags.

**Gates:** `just component_sdk_release_check`, `just component_sdk_export_check`, `just lint_all`, `just fmt_check_all`, `just ruff`.

**Evidence:** [`devlog/2026-08-25-cp6-cp10-component-sdk-releases/`](../devlog/2026-08-25-cp6-cp10-component-sdk-releases/index.md), and for the hosted closure [`devlog/2026-08-26-cp7-hosted-publication-hardening/`](../devlog/2026-08-26-cp7-hosted-publication-hardening/index.md)

## CP8 — Platform build-input releases

**Status:** Complete.

**Depends on:** CP6, CP7.

### Deliverables

- publish a content-addressed seL4 prefix archive for each SDK-supported profile, initially `aarch64-sel4-qemu-virt` and `bcm2712-rpi5`, rather than requiring `SEL4_PREFIX` to point into `slime_os/build/`;
- bind each archive in the CP6 Zutai release record to its profile, seL4 source/config identity, kernel and libsel4 configuration hashes, platform-info hash, archive hash, target-spec hash, Rust nightly, and `rust-sel4` commit;
- publish the exact source rebuild recipe beside the prebuilt archive, retaining `sel4/pins.toml` and the existing prefix checks as the authority for its contents;
- provide one non-interactive SDK build entry point that verifies the release record and archive before exporting `SEL4_PREFIX`, `SLIME_TARGET_PROFILE`, target specification, and required Cargo flags;
- keep profile assets separate: a QEMU-qualified component cannot silently consume the RPi prefix or target identity, and vice versa.

### Required checks

- archives extracted in an empty directory reproduce the recorded prefix identities and contain no absolute build-host or source-checkout path;
- an external component builds with only the permanent SDK clone, the selected downloaded prefix, `libclang`, and the pinned Rust toolchain; no file below the `slime_os` checkout is opened;
- the QEMU asset produces an ELF admitted into and booted by the QEMU product image;
- the RPi asset produces an ELF admitted for `bcm2712-rpi5` and refused as wrong-target by the QEMU profile; this host-side qualification is not recorded as physical-board evidence;
- corrupt, truncated, swapped-profile, and metadata-mismatched archives are refused before Cargo or bindgen runs.

### Planned verification target

```sh
just component_sdk_prefix_check
```

### Exit condition

A clean external checkout builds target-qualified QEMU and RPi component ELFs using only one immutable SDK release and its verified platform asset; the QEMU ELF boots, and neither build references `slime_os/build/sel4-prefix*`.

**Delivered:** each release carries one content-addressed `tar` per supported profile — `aarch64-sel4-qemu-virt` and `aarch64-rpi5` — bound in the release record to its profile, its platform, the five `sel4/pins.toml` artifact hashes for that platform, an archive hash, an extracted-tree hash, and the exact `build-sel4.py` plus `check-sel4-pins.py` rebuild recipe. The archive and the extraction are hashed separately because reproducible bytes and a reproducible extraction are two claims and only the second is what Cargo and bindgen read. `tools/sdk-build.py` verifies the record, the archive, and the extracted tree before exporting `SEL4_PREFIX`, `SLIME_TARGET_PROFILE`, the target, and the flags.

Two things the QEMU profile alone hid. The RPi profile builds components against the `aarch64-unknown-none` triple with its own link flags rather than the seL4 JSON target, so the record now carries each profile's target, whether it is a JSON specification, and its exact `RUSTFLAGS` and Cargo flags — one hard-coded flag set produced an ELF the generation builder refused with "invalid component load layout". And that triple links against a repository-level linker script an out-of-tree crate cannot find relative to its own manifest, so `components/build-support` honours `SLIME_COMPONENT_LINKER_DIR` and the export ships the scripts as build inputs. The exporter also canonicalizes the two libsel4 headers seL4's own Python generators stamp with an absolute `.bf` path, to the same `/slime/sel4` logical prefix the kernel build maps its debug paths to; every *other* host path in an exported byte is a refusal rather than a rewrite.

**Exit condition (observed):** `just component_sdk_prefix_check` extracted both archives into empty directories, reproduced their recorded tree identities, found no build-host path, and matched each kernel against its pin. An external checkout then built target-qualified ELFs for both profiles with `SEL4_PREFIX` pointed at a nonexistent path and `SLIME_TARGET_PROFILE` set wrong beforehand, so a build that still succeeded can only have taken both from the release record; neither build referenced `build/sel4-prefix*`. The QEMU ELF entered a signed generation and booted the QEMU component graph. The QEMU-target ELF was refused by the RPi build with "invalid component load layout", and the RPi ELF was admitted only under profile id 3 while a QEMU-profile generation wrapping it declared profile id 5 for the root to refuse before mapping — host-side qualification only, and explicitly not physical-board evidence. Corrupt, truncated, swapped-profile, and metadata-mismatched archives were each refused before Cargo ran.

**Amended deliverable:** the profile is named `aarch64-rpi5` rather than `bcm2712-rpi5`. Those are two vocabularies: `bcm2712-rpi5` is the `build-sel4.py` *platform* that produces the prefix, and `aarch64-rpi5` is the `contracts/target-profile/v1` *profile* a component is qualified against. The record carries both, because a consumer that conflated them would export the wrong `SLIME_TARGET_PROFILE` beside a correct prefix.

**Gates:** `just component_sdk_prefix_check`, `just component_sdk_release_check`, `just test_host`, `just lint_all`, `just fmt_check_all`, `just ruff`.

**Evidence:** [`devlog/2026-08-25-cp6-cp10-component-sdk-releases/`](../devlog/2026-08-25-cp6-cp10-component-sdk-releases/index.md)

## CP9 — SDK versioning and compatibility matrix

**Status:** Complete.

**Depends on:** CP7, CP8.

### Deliverables

- define the SDK version policy separately from Cargo source compatibility: release metadata records syscall-ABI, component-image, public Zutai contract-set, target-spec, toolchain, `rust-sel4`, and prefix identities that can affect a built component;
- classify release changes as patch, compatible feature, or breaking, with an automated comparison refusing a release whose version change understates any changed identity or removed public API;
- begin with exact-pair support: an SDK release is supported against its originating product release. Cross-release compatibility appears in the matrix only after the same external component source or artifact is exercised by the declared build, admission, and boot gates;
- publish a machine-readable compatibility matrix derived from tested release pairs, not a hand-maintained promise; absence from the matrix means unsupported, not implicitly compatible;
- retain the existing trust model: the matrix is release guidance and CI evidence, not a new root-side provenance field or an admission bypass.

### Required checks

- negative controls change each compatibility identity in isolation and force the expected release classification;
- a release cannot claim compatibility from equal crate versions while its syscall ABI, target specification, component-image format, protocol identity, or platform prefix changed incompatibly;
- every matrix row names immutable SDK and `slime_os` commits and is backed by a build plus the narrowest applicable QEMU boot gate;
- an untested old/new pairing is absent and reported unsupported rather than accepted by version-range inference;
- patch and compatible-feature examples retain the prior row only when the prior external fixture still builds, enters a generation, and boots unchanged.

### Planned verification target

```sh
just component_sdk_compatibility_check
```

### Exit condition

Two consecutive immutable SDK releases are correctly classified, every published compatibility row has direct build/admission/boot evidence, and no untested cross-release pairing is presented as supported.

**Delivered:** the version policy is stated over identities that can make a component disagree with the system it is loaded into *after* it compiles, never over crate versions. Five are scalar and breaking on any change — syscall ABI, component-image format, the public contract set, the toolchain, and the `rust-sel4` pin. Two are structural and compared as keyed sets: the exported crates and the target profiles. That split is the milestone's real content. Adding a profile or a crate is a compatible feature and changing or removing one is breaking, and a digest over either set could only report "different" — which would make every platform CP8 adds a major release and so make the classification useless exactly where it matters. `admit_version_change` is one-directional: overstating a change is allowed, understating one is refused with the changed axis named. `sdk/compatibility-matrix.{zti,json,identity}` is the published matrix, decoded by `just contracts_check` through the contract rather than trusted because this repository wrote it.

**Exit condition (observed):** `just component_sdk_compatibility_check` published SDK 1.0.0 and 1.1.0 as two immutable commits and classified them `initial` and `compatible-feature`, the second because it genuinely adds the RPi profile with every other identity unchanged. Five scalar and two structural negative controls each moved one identity in isolation and forced the expected classification, including the case the milestone exists for: a release claiming a patch while its syscall ABI changed, with all crate versions equal, was refused naming `syscallAbi`. A non-advancing version was refused outright. Both published rows name immutable SDK and product commits and cite the component ELF, the generation identity, and `just sel4_component_graph_check` — each observed in that run, with the prior release's row retained only because its own fixture still built, entered a generation, and booted. Three untested pairings — the RPi profile nobody booted, the first SDK against another product commit, and a synthetic future commit — each reported unsupported.

**Gates:** `just component_sdk_compatibility_check`, `just contracts_check`, `just component_sdk_prefix_check`, `just lint_all`, `just fmt_check_all`, `just ruff`.

**Evidence:** [`devlog/2026-08-25-cp6-cp10-component-sdk-releases/`](../devlog/2026-08-25-cp6-cp10-component-sdk-releases/index.md)

## CP10 — Consumer pin, upgrade, and rollback workflow

**Status:** Complete.

**Depends on:** CP9.

### Deliverables

- provide a canonical external component workspace template that pins the SDK by full commit, commits `Cargo.lock`, selects one release-record profile, and contains no branch, tag-only, registry, or `slime_os` path dependency;
- provide a non-interactive update command that changes the SDK commit, lockfile, verified platform asset, and recorded release identity together, then rebuilds the component and reports its new bare-ELF SHA-256 for the operator-owned component spec;
- exercise the real composition update: update the external component spec's `implementation.contentHash`, build and sign the generation through CP4, embed that exact generation, and boot it before the new pin is considered usable;
- retain the previous SDK pin, prefix asset, component ELF, and generation as immutable rollback inputs until the updated generation passes its declared health gate;
- make a failed dependency resolution, build, admission, signing, or boot leave the prior consumer checkout and prior bootable generation selected and reproducible.

### Required checks

- the template builds from a fresh clone with `cargo build --locked` and rejects every floating SDK reference;
- upgrading from the first CP9 release to the second changes all coupled pins in one reviewable diff, rebuilds the external ELF, updates its content hash, and boots the resulting generation;
- fault injection at dependency fetch, prefix verification, compile, digest admission, and QEMU health confirmation leaves the previous pin and generation usable;
- rolling back the consumer repository reproduces the previous ELF and generation identities byte-for-byte from retained immutable inputs;
- the in-tree fallback generation still builds and boots after both the upgrade and rollback scenarios.

### Planned verification target

```sh
just component_sdk_upgrade_check
```

### Exit condition

A separate consumer repository moves between two immutable SDK releases, boots the newly content-bound generation, and then reproduces and boots the prior generation after rollback, with no floating reference or dependency on a `slime_os` checkout.

**Delivered:** every release ships `template/`, a workspace pinning all five SDK crates by full commit, plus `tools/sdk-update.py`, which moves a consumer to a new release by changing the SDK revision, the lockfile, the verified platform asset, and the recorded release identity together. The update works in a staging copy and promotes it only after the rebuild succeeds, so a failure at any step leaves the previous checkout exactly as it was, and it reports the rebuilt component's bare-ELF SHA-256 for the operator-owned component spec.

**Exit condition (observed):** `just component_sdk_upgrade_check` created a consumer from the shipped template, verified its pin is a full commit with no branch, tag, registry, or `slime_os` path reference, and built it as shipped with `--locked`. It then booted a content-bound generation, upgraded to the second release — changing exactly `Cargo.toml`, `Cargo.lock`, and both recorded-release files, asserted as a set — rebuilt the ELF, and booted the new generation. Five failures were injected at the five named points: an SDK commit the repository lacks, a corrupt prefix archive, consumer source that does not compile, a component spec whose declared content hash disagrees with the bytes, and the retained generation's continued admissibility standing for health confirmation. After each, the manifest, lockfile, recorded release, and built ELF were unchanged. Rollback then restored the retained snapshot and reproduced the previous ELF and generation byte-for-byte, with matching generation identity, and booted. The in-tree fallback generation built and booted afterwards.

**Amended deliverable:** the template's own component is built as shipped to prove the template compiles, and then given the `console` component's behavior for the boot arms. The substitution is necessary rather than convenient: the QEMU component graph drives `console` through a scripted scenario and waits for its markers, so a component that merely started and exited would leave the graph waiting until the boot timed out. What the upgrade and rollback arms observe is the pin, the rebuild, and the generation identity, so the component beneath them must be one the graph actually composes.

**Gates:** `just component_sdk_upgrade_check`, `just component_sdk_compatibility_check`, `just lint_all`, `just fmt_check_all`, `just ruff`.

**Evidence:** [`devlog/2026-08-25-cp6-cp10-component-sdk-releases/`](../devlog/2026-08-25-cp6-cp10-component-sdk-releases/index.md)

## CP11 — Canonical system-image and test-run closure contracts

**Status:** Complete.

**Depends on:** CP1, CP8.

### Deliverables

- define `contracts/system-image-closure/v1/schema.zt` as the canonical, bounded description of one reproducible bootable-image build, referencing rather than restating the selected `system-spec/v1`, component implementations, target profile, SDK/toolchain release, seL4 platform prefix, root/loader role, release inputs, normalized build parameters, and expected output classes;
- define `contracts/system-test-run/v1/schema.zt` for execution-only inputs: one image-closure identity, emulator or board profile, disk/network/device fixtures, bounded fault controls, timeout, expected marker-contract identity, and forbidden outcomes;
- add generated Python bindings and one resolver that verifies every referenced digest, target/profile pairing, implementation selection, platform asset, and build parameter before invoking Cargo, the generation builder, or the image packager;
- provide one non-interactive host entry point accepting a resolved image closure and an output directory, with no composition name, plane flag, implicit output path, ambient component mapping, or undeclared environment override;
- emit a versioned build-result record naming the input closure identity and the identities of the generation, root, loader, final image, and identity manifest.

### Required checks

- two isolated resolutions of one closure are byte-identical and reject a missing, changed, wrong-target, wrong-profile, or unrecorded input before compilation;
- changing any executable-affecting source, toolchain, platform, parameter, external ELF, generation input, or root role changes the closure identity, while changing only a test-run marker oracle does not;
- the resolver proves that all paths used by a build are reachable from the closure or the selected immutable release, with no fallback to the checkout's ambient `build/`, environment, current directory, or component registry;
- representative channel, component-graph, and external-component closures reproduce their existing `generation.bin` and packaged image bytes exactly;
- malformed and excessive closure/test-run records fail through their generated Zutai decoders and declared bounds.

### Planned verification target

```sh
just system_image_closure_check
```

### Exit condition

One canonical, versioned image closure resolves in two clean build roots to the same generation and bootable seL4 image, and one separate test-run record boots that image without contributing any executable build input.

**Delivered:** `system-image-closure/v1` now binds the selected system spec, component implementations, SDK/toolchain, target-qualified seL4 prefix, root and loader roles, release inputs, build parameters, and output classes into one normalized identity. `system-test-run/v1` keeps emulator/board fixtures, runtime faults, timeout, marker oracle, and forbidden outcomes outside executable identity. The generic `build-system-image.py CLOSURE OUTPUT_DIR` resolver verifies every declared path and digest before compilation, uses isolated Cargo/generation/image roots, rejects ambient `SLIME_*` build controls, and emits versioned image and build-result identity records.

**Exit condition (observed):** `just system_image_closure_check` resolved the canonical channel closure, refused missing, changed, wrong-target, wrong-profile, unrecorded, malformed, and excessive inputs, built it in two isolated roots plus an adversarial-environment root, and observed byte-identical generation, root, loader, image, image identity, normalized build result, and build-result identity. `just sel4_fault_check` and `just sel4_boot_selection_check` separately confirmed the legacy scenario and selector paths still compile their declared controls after closure builds received a distinct hermetic build profile.

**Gates:** `just system_image_closure_check`, `just sel4_fault_check`, `just sel4_boot_selection_check`, `just lint_all`, `just fmt_check_all`, `just ruff`.

**Evidence:** [`devlog/2026-09-02-cp11-system-image-closure/`](../devlog/2026-09-02-cp11-system-image-closure/index.md)

## CP12 — Complete spec derivation for every test composition

**Status:** Substantially delivered; the exit condition is not met. 40 of the 42 compositions are derived; the remaining 2 are recorded, per composition and with a closed reason, by `contracts/composition-inventory/v1` and remain open work.

**Depends on:** CP11.

### Deliverables

- add a reviewed `system-spec/v1` source for every one of the 42 current `contracts/generation-manifest/v1/compositions/*.zti` compositions, preserving grants, placements, slot-pin reasons, notification bindings, state, budgets, fabric graphs, boot profiles, and target requirements;
- extend `generate-generation-from-spec.py` only where an existing manifest field is genuinely not derivable from the current component/system vocabulary, adding the narrow owning contract field rather than a fixture-name branch;
- make every composition manifest a generated output with an explicit source-closure identity and keep the current paths stable until the final cutover so existing admission and layout checks compare the same artifacts;
- preserve the distinction between one logical system and multiple declared closures: traffic/saturation/fault and matrix/matrix-unsatisfiable may share generated source functions, but each emitted closure is complete canonical data with its own identity;
- add a closed inventory mapping all 42 legacy composition names to their system spec, image closure, expected generation identity, and owning verification gate.

### Required checks

- regenerating the complete composition corpus under `--check` produces no diff, and deleting or hand-editing any generated manifest fails the gate;
- each migrated manifest and `generation.bin` is byte-identical to its pre-migration baseline unless the milestone records and reviews an unavoidable identity change before implementation;
- every current boot-layout fixture resolves against the spec-derived manifest with the same slots and all 611 surviving pin reasons remain builder-verified;
- the generator has no condition keyed on a composition filename, test name, plane name, or generation number;
- all contract, generation determinism, host admission, and representative QEMU plane gates remain green throughout the migration.

### Planned verification target

```sh
just system_composition_closure_check
```

### Exit condition

All 42 seL4 test compositions are generated from component/system specifications and complete image closures, with byte-identical resolved manifests and generations and no hand-authored composition remaining authoritative.

**Delivered:** `contracts/system-spec/v1` now expresses what the corpus actually declares. Eight generation-manifest sections it could not represent at all — `clockAuthority`, `ioResourceBudget`, `networkDestinations`, `blockRingAuthority`, `waitSet`, `schedulingClass`, `lifecyclePolicy`, `recording` — are declared with a companion `*Object` presence boolean each, on `sharedBufferBudgetObject`'s own terms: `sel4-io-network` carries a `wait-set` resource object with no declared source, so object presence and list non-emptiness are independent facts and deriving one from the other would change which payload that generation encodes. Two binding facts the grant table cannot imply are declared rather than inferred — `ExtraBinding` for a spawn broker holding an `executable` grant it neither issued nor received, and `SystemMintedBinding` for a capability minted after activation with no object to name. Nine `Placement` overrides carry what varies per composition rather than per component: `health` (`supervision-child` is `required` under `sel4-supervision` and `optional` under `sel4-reclamation`), `role` (`slisp` is an application in the reference generation and its own bootstrap in `sel4-slisp`), `dependencies`, `stackBytes`, the four shared-buffer ceilings, and `privatePageQuota`. `derive_bindings` admits a source's retained binding only where a slot pin or extra binding declares it, which is what the corpus measures: 23 of 24 `sharedBufferFactory`, 12 of 12 `directory`, and 13 of 14 `device`-kind source-side bindings are pinned, and the sole unpinned case grants a component authority over itself. Notification validation now matches the builder's real rule — one waiter, one or more signallers including the declared source. 11 new component specs cover every executable the converted compositions declare, and `contracts/composition-inventory/v1` is the closed record of which compositions are derived and why the rest are not.

**Exit condition (observed, partial):** `just system_composition_closure_check` compiles 41 system specs, derives 41 manifests semantically identical to their committed fixtures, refuses 21 named derivation mutations, inventories all 42 compositions (40 derived, 2 hand-authored) with every row backed by a real owning gate, and refuses 7 named inventory mutations. Every converted composition's `generation.bin` was rebuilt from its frozen pre-migration text and from its derived text under one toolchain and compared: all 40 are byte-identical (39 measured directly; `sel4-channel` is a file this milestone does not modify and whose pre-B91 baseline predates `slotReason`). `just sel4_boot_layout_check` resolved 31 plane layouts against their frozen fixtures, `just generation_check` produced byte-identical generations across two isolated builds, `just sel4_gate_control_check` proved 45 gates reject 1748 mutated transcripts, and 36 QEMU plane gates passed — including the 23-instance `sel4-stress` graph, the 5-instance `sel4-clock-authority` plane, and the 4-instance `sel4-lifecycle-restart` plane that the one-instance-per-component model could not express. The generator carries no condition keyed on a composition, test, plane, or generation number.

**Not delivered:** 2 compositions. `sel4-matrix` gives three fabric components route roles their specs do not declare, and `just component_spec_check` requires a spec's interface list to match `valid.zti`'s graph exactly, so one spec cannot describe two compositions' route sets; resolving it needs per-composition interface entries or a system-level route-role override. `sel4-c-runtime`'s implementation is a freestanding C source built by a helper script at gate time with no committed content identity to pin. Both reasons are in the inventory contract's closed vocabulary and tracked in [`roadmap/00-backlog.md`](00-backlog.md).

**Gates:** `just system_composition_closure_check`, `just system_spec_check`, `just contracts_check`, `just generation_check`, `just sel4_boot_layout_check`, `just sel4_gate_control_check`, `just devlog_check`.

**Evidence:** [`devlog/2026-09-02-cp12-composition-derivation/`](../devlog/2026-09-02-cp12-composition-derivation/index.md)

## CP13 — Data-driven seL4 image builder cutover

**Status:** Partially delivered. One generic command builds any of 38 generated closures with no plane flag, variant table, or composition named in builder source, and `just system_image_builder_check` gates that. The legacy `build-sel4.py` flag family is not yet deleted, because CP15 owns migrating its last checker.

**Depends on:** CP12.

### Deliverables

- make `scripts/build/build-sel4.py` accept one closure plus an explicit output directory and derive the selected generation, target directories, root inputs, loader inputs, image name, and identity path entirely from resolved closure data;
- remove the ordinary-plane `--component-graph` and `--*-plane` argument family, `VARIANT_MANIFESTS`, `VARIANT_TARGET_DIRS`, `VARIANT_IMAGES`, and every equivalent source table that maps a scenario name to build behavior;
- replace the JSON identity manifest's `variant` authority with the canonical closure identity, system identity, target profile, root role, and output identities while retaining compatibility fields only until their last checker migrates in CP15;
- make prebuilt and external component inputs ordinary closure implementation records rather than separate `--prebuilt-generation`, `--component-spec-root`, or `--external-component NAME=ELF` orchestration paths;
- keep platform configuration in the existing platform/prefix release contracts: selecting QEMU, Raspberry Pi 5, or Milk-V Duo changes closure data, not Python control-flow naming a composition.

### Required checks

- every ordinary plane image builds through the same command shape and no new plane requires a builder source edit;
- selecting a closure with another target, root role, component implementation, or output directory cannot reuse a stale Cargo target, generation, root ELF, image, or identity record;
- the generic builder reproduces every CP12 baseline and refuses output collisions between distinct closure identities;
- an external SDK consumer builds a closure using only its immutable SDK release and verified prefix assets, with no path into this checkout's `build/` tree;
- source checks prove the removed flags and variant mapping tables cannot return.

### Planned verification target

```sh
just system_image_builder_check
```

### Exit condition

One data-driven command builds every ordinary product and test image from its closure, and adding another composition requires only contract data and its behavioral checker, never a new builder flag, constant, or output-path branch.

**Delivered:** `scripts/generate/generate-system-image-closures.py` emits one closure per derived composition, plus the scenario and root-role closures CP14 declares — 43 in total — from repository state, and the CP11 resolver independently re-reads every path and refuses any digest that disagrees, so generation and resolution stay separate authorities rather than one circular claim. Each closure names its system spec's identity, every component spec's identity, every implementation crate tree, the SDK release, the target-qualified prefix, the root and loader roles, and the fifteen shared workspace build inputs. `scripts/check/check-system-image-builder.py` proves the corpus property CP11 could only prove for one record: every derived composition has a closure or a declared reason, no closure is orphaned, all 43 resolve, no two share an identity, and each resolved closure's manifest is exactly the one its system spec derives — so a closure cannot become a second authority on what a plane admits. The builder's own argument surface is asserted from its AST: exactly `closure` and `output_dir`, with no `--*-plane` option, no `VARIANT_*` table, and no composition name anywhere in its source.

**Exit condition (observed, partial):** `just system_image_builder_check` resolved all 43 closures with distinct identities and spec-matching manifests, built `sel4-channel` twice into separate output directories with byte-identical generation, root, loader, image, identity-manifest, and build-result identities, and observed the builder refuse a non-empty output directory — the collision guard that keeps two closure identities from sharing one tree. `--exhaustive` builds every closure the same way. `just system_image_closure_check` still passes against the generated `sel4-channel` closure.

**Not delivered:** `scripts/build/build-sel4.py`'s `--*-plane` family, `VARIANT_*` tables, and the identity manifest's `variant` authority are still in place. They are what the 36 existing QEMU plane checkers call, and CP15 owns migrating those checkers; deleting the flags before their last caller moves would break every plane gate. The new builder path is additive and independently gated until then.

**Gates:** `just system_image_builder_check`, `just system_image_closure_check`, `just system_composition_closure_check`, `just ruff`.

**Evidence:** [`devlog/2026-09-02-cp12-composition-derivation/`](../devlog/2026-09-02-cp12-composition-derivation/index.md)

## CP14 — Explicit scenario, selector, and negative-test identities

**Status:** Delivered.

**Depends on:** CP13.

### Deliverables

- replace `SLIME_GENERATION_NUMBER`, `SLIME_FABRIC_LIMIT_OVERRIDE`, and `SLIME_FABRIC_QOS_OVERRIDE` with canonical closure data for saturation, fault, and unsatisfiable-matrix generations;
- model executable-changing component scenarios such as proxy early exit, stream early exit, generation-command cases, boot-selection failure, and recovery images as named implementation/build profiles whose normalized parameters and resulting ELF identities are part of the closure;
- model the embedded-generation root, disk boot selector, root fixture, reclamation unwind probe, and board instrumentation as closed root roles or platform-qualified instrumentation profiles rather than variant branches;
- represent B40 child-CSpace mutations and similar deliberately invalid builds as negative build cases referencing a valid base closure and one closed mutation, never as valid product closures;
- move QEMU disks, device topology, corruption schedules, runtime fault controls, timeouts, and marker contracts into `system-test-run/v1` whenever they do not alter executable bytes.

### Required checks

- no executable or generation byte can change through an environment variable absent from the closure identity;
- each scenario implementation builds byte-identically twice, differs from its base implementation, and is admitted only by closures that explicitly select it;
- product closures cannot select test-only scenarios, root mutations, or board instrumentation, and wrong-platform instrumentation fails before compilation;
- boot-selector tests prove the selector image contains no embedded generation while embedded-generation closures prove the opposite from their build-result records;
- every negative build case fails for its declared reason and cannot emit a signed generation or a bootable image presented as valid.

### Planned verification target

```sh
just system_image_scenario_check
```

### Exit condition

Every remaining build-time distinction is explicit, identity-bearing closure data or a separately typed negative/test-run input; no ambient override or variant branch can silently change executable, generation, root, or image bytes.

**Delivered:** all five deliverables. `SLIME_GENERATION_NUMBER`, `SLIME_FABRIC_LIMIT_OVERRIDE`, and `SLIME_FABRIC_QOS_OVERRIDE` are `contracts/system-image-closure/v1` `BuildParameter` entries drawn from a closed three-name vocabulary (`generationNumber`, `fabricLimitOverride`, `fabricQosOverride`), so a fourth ambient knob cannot appear without a contract change and the resolver refuses any name outside the set. `build-system-image.py` applies them from the resolved closure, validating each against what the manifest already declares — an override naming an undeclared limit, route, participant, or field is a refusal rather than a silently created graph field. Saturation and fault are now scenario closures over the `sel4-traffic` composition, carrying the exact numbers `build-sel4.py`'s `VARIANT_GENERATION_DELTAS` declared, so their generation identities are unchanged by becoming closure data.

The second deliverable is the executable-changing scenarios. `ImplementationSelection.buildProfile` — a field CP11 declared and never used — is now a closed seven-name vocabulary (`default`, `proxyEarlyExit`, `streamEarlyExit`, `generationCmdBadClosure`, `generationCmdBadRelease`, `bootSelectionFail`, `recoveryImage`), each mapping to exactly one `option_env!` knob the components already read. A profile is per implementation rather than per closure because exactly one component's bytes change; naming it on the closure would make every other component's ELF look scenario-dependent. `build-system-image.py` translates the resolved profiles into those knobs from closure data alone, and `build-generation.py`'s `closure` build profile no longer strips them — it cannot, because the closure builder clears every `SLIME_*` before setting exactly the ones its identity declares. Distinct knobs coexist (`sel4-fault` needs both the proxy and the stream death); two profiles setting one knob to different values are refused rather than silently resolved.

The third deliverable is the root roles. `BuildRole.role` is now a closed four-name vocabulary — `embedded-generation`, `boot-selector`, `root-fixture`, `reclamation-unwind` — and `BuildRole.parameters`, which CP11 declared and admitted only empty, carries the two platform-qualified instrumentation knobs `qemuKeyboard` and `duoTestTerminator`. The resolver refuses an unadmitted role, an unadmitted parameter, and a parameter on the wrong platform before Cargo runs, so a root that would read a device its platform does not have fails at resolution rather than at boot. `build_application` accepts the role and parameters from closure data and takes no `variant` on that path, so the builder's variant table cannot select a root build under a closure. A boot-selector root is required to embed *no* generation and every other role to embed exactly the one its closure resolved — opposite claims, both checked, which is what keeps a selector image from silently shipping a generation.

The fourth deliverable is the negative build cases. `NegativeBuildCase` is a separate record type — deliberately *not* a `SystemImageClosure`, because a closure keys a build meant to boot and these key builds that must be refused; one type for both would let a deliberately invalid root be presented as a product image. Each names a base closure by identity, one of the six closed B40 child-CSpace mutations, and the refusal its owning audit must emit, so a case that failed for some other reason is not counted as evidence. `build-system-image.py --negative` resolves the base by identity, compiles the mutation into the root, and writes no `image.identity.json` and no `build-result.json` — the two artifacts every consumer reads as "this is a verified image" — emitting `negative-image.elf` instead. The ambient `SLIME_B40_MUTATION` no longer reaches a closure build at all.

**Exit condition (observed, partial):** `just system_image_scenario_check` confirmed the contract admits exactly three build parameters and refuses a fourth; both scenario closures resolve with identities distinct from their base (`sel4-traffic` `a47ca3c6…`, `sel4-saturation` `a31466 11…`, `sel4-fault` `05997de1…`) and from each other; each scenario's declared parameters change exactly the manifest fields they name, measured field by field against the base — saturation moves only `.generation` and `.fabricGraph.limits.inFlightOperations`; eight malformed parameters were refused with their declared reasons; and all 41 closures' parameters apply to their own manifests. For the build profiles it confirmed the seven-name vocabulary is closed, an unadmitted profile and a same-knob conflict are both refused, every non-default profile maps to a real knob, and — the arm that makes the rest more than bookkeeping — `sel4-stream-death`'s profile moved `fabric-publisher.elf` from `cd5cedc17aea` to `7c3f028a5226` reproducibly across two builds, while `init.elf` and every other unnamed component stayed byte-identical and the two images differed. For the root roles it confirmed the four-name vocabulary is closed with an unadmitted role, an unadmitted parameter, and a wrong-platform parameter each refused; both root-role closures resolve with identities distinct from their bases; and `sel4-channel-fixture` moved `root.elf` from `96322f2a8025` to `edf09fe2ba56` reproducibly while its generation stayed byte-identical to its base's — a root role changes the root, never the graph. For the negative cases it confirmed all six mutations are covered exactly once over real base closures, malformed cases are refused, and `sel4-b40-missing` produced a distinct root while writing neither an image-identity nor a build-result record.

The fifth deliverable is the test-run records. All 45 plane gates that boot a seL4 QEMU image now have a frozen `contracts/system-test-run/v1` record declaring their disks, device topology, injected fault kinds, timeout, marker contract, and forbidden outcomes, extracted from each gate and held fixed the way `sel4_boot_layout_check` holds its per-plane layouts. `just system_test_run_check` requires the plane set and the record set to correspond exactly in both directions, decodes each record through the contract so the schema's bounds and closed vocabularies are enforced rather than assumed, and requires each record either to resolve a closure belonging to its own plane or to name an image the aggregate gate independently agrees is closure-exempt — so an empty identity is a checked statement rather than a blank field.

**Exit condition (observed):** 45 records, 40 resolving their own plane's closure and 5 declaring a closure-exempt image, with 13 fault controls declared, every record matching the gate it was frozen from, and 5 named controls refused. Extraction itself found three defects in its own reading: forbidden outcomes are written as regexes with a trailing matcher, so a literal-only pattern reported "forbids nothing" for most planes and silently dropped the hand-authored `sel4-channel` record's two markers; the fault-kind heuristics matched any `inject` and so declared faults planes do not exercise; and a plane's image is not its name — the component-graph gate boots `slime-sel4-graph.elf` — so keying by name attributed a plane to the wrong image.

**Note:** the records are declarations, not yet the execution path. The gates still run from their own constants, and consuming the records is CP15's remaining work; the freeze is what that migration is verified against. The `generationCmd*`, `bootSelectionFail`, and `recoveryImage` profiles and the `boot-selector` root role are declared, resolvable, and gated but not yet carried by a closure, because their host compositions build through the legacy path CP15 is migrating.

**Gates:** `just system_image_scenario_check`, `just system_test_run_check`, `just system_image_builder_check`, `just system_image_closure_check`, `just ruff`.

**Evidence:** [`devlog/2026-09-02-cp12-composition-derivation/`](../devlog/2026-09-02-cp12-composition-derivation/index.md)

## CP15 — Whole-corpus closure cutover and legacy deletion

**Status:** Partially delivered. 36 of 49 seL4 plane gates build by closure identity, the aggregate inventory gate exists and is green, and the eight plane flags migration made unreachable are deleted. The remaining 13 gates, the rest of the legacy surface, and the SDK publication clause are open.

**Delivered:** the aggregate inventory gate and the first, third, and fourth deliverables in part.

`scripts/lib/closure_image.py` is the migration seam: a gate names a closure and receives a built image plus its build-result record, with the identity independently re-resolved from repository state before every build rather than trusted from the record. One shared mechanism rather than 49 near-identical subprocess calls, per this repository's verification-code discipline. QEMU invocation, marker ordering, disks, and fault injection deliberately stay out of it — those belong to the owning gate and its test-run record, and folding them in would rebuild the separation CP11 drew.

`just system_image_closure_aggregate_check` is the fourth deliverable and closes both drift directions at once: a closure nobody exercises is an untested build key, and an image no closure describes is a build nobody can reproduce. It also requires every surviving plane flag to be reachable from a `just` target, which is what made the eight orphaned flags a mandatory deletion rather than an optional cleanup, and it refuses a gate that builds through `closure_image` *and* invokes the legacy builder — holding both is how a gate silently keeps booting the legacy artifact while appearing migrated.

**Exit condition (observed, partial):** 36 gates migrated and passing from closure-built images, including every IO and storage plane, the fault and stream planes, the boot plane, and the B40 capability-layout audit now driven by CP14's typed negative cases rather than an ambient `SLIME_B40_MUTATION`. `just sel4_gate_control_check` (45 gates, 1748 mutations), `just sel4_boot_layout_check` (31 layouts), `just sel4_fabric_aggregate_check`, `just sel4_component_graph_check`, and `just sel4_device_check` all pass. The aggregate gate reports 44 closures each exercised by an owning gate, 36 flags each reachable from a target, no gate holding both build paths, and 6 named drift controls refused.

**Two real defects the migration surfaced**, both in the CP13/CP14 builder and both invisible while only the closure gates — which compare a closure against itself — exercised it. The component target directory was named `generation-<n>` under the closure profile where the legacy path named it `sel4-call-18`, and CP3 established that this name reaches the shipped ELF's symbols. Worse, components were built under the caller's output directory, which sits inside the repository root, so the `--remap-path-prefix` rule rewrote its leading portion and left the caller's path in `core::panic::Location` strings — the same closure produced different bytes for different output directories, which is exactly the non-reproducibility a closure identity is supposed to preclude. Both fixed; `sel4-call`'s generation is now byte-identical between the closure build and the legacy build of the same composition. A third gap was in CP14's own modelling: the qos plane's publisher-death profile was never made closure data, so its peer-dead marker vanished the moment it built from its closure.

**Not delivered:** 13 gates remain on plane flags — the boot-layout composer, the fabric aggregate, the component-graph and device gates, the demo, matrix, c-runtime, stress, generation, rollback, recovery, and boot-selection planes, and the root-boot aggregate. `VARIANT_MANIFESTS`, `VARIANT_TARGET_DIRS`, `VARIANT_IMAGES`, `VARIANT_GENERATION_DELTAS`, and the identity manifest's `variant` authority survive because those gates call them; the aggregate gate's flag-reachability rule forces each to be deleted as its last caller migrates. The 18 legacy-only `SLIME_*` knobs are enumerated by that gate and may only shrink. The two-clean-corpus-build check, the source guard against hard-coded checker image paths, and the SDK publication clause — an external consumer turning one declared closure into a verified bootable image without a `slime_os` checkout — are not started.

**Depends on:** CP14.

### Deliverables

- migrate every seL4 QEMU plane checker, boot-layout checker, gate-control mutation, SDK admission/upgrade check, and architecture replay to build or consume an image by closure identity and build-result record;
- preserve each checker's behavioral ownership: marker chains, disk/network fixtures, corruption schedules, negative controls, and physical evidence rules remain in the owning checker or referenced test-run contract rather than moving into the image builder;
- delete the legacy plane flags, variant compatibility fields, ambient generation/component mappings, duplicate output-path constants, and any composition loader that bypasses the closure resolver;
- add one aggregate inventory gate proving every shipped/tested image is reachable from exactly one canonical closure and every closure is exercised by at least one owning build or boot gate;
- publish the closure builder and contracts through the component SDK so an external consumer can compose target-qualified components, build a signed generation, package a bootable image, and invoke its declared QEMU test run without a `slime_os` checkout.

### Required checks

- the full existing seL4 gate set and `sel4_gate_control_check` pass through closure-derived images with their original behavioral assertions intact;
- two clean complete-corpus builds produce identical closure, generation, root, loader, image, and build-result identities for every non-negative case;
- a source guard rejects reintroduction of `--*-plane`, `VARIANT_*`, undeclared `SLIME_*` build knobs, or checker-owned hard-coded image paths that bypass build results;
- the external SDK fixture builds and boots one complete system closure using only immutable published inputs, and rollback to the previous SDK release reproduces its previous image identity;
- `just contracts_check`, `just generation_check`, `just test_host`, `just lint_all`, `just fmt_check_all`, `just ruff`, and the complete QEMU aggregate pass after legacy deletion.

### Planned verification target

```sh
just system_image_closure_aggregate_check
```

### Exit condition

Every current test composition and supported product image is built, identified, and exercised through a canonical closure; the legacy variant builder surface is deleted; and a clean external consumer can turn one declared closure into the same verified bootable image without repository-private orchestration.
