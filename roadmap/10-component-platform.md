# Component specification and out-of-tree development track

**Purpose:** Give "component" and "system" a formal, versioned specification independent of any one generation manifest, derive generation inputs from that specification, and support components authored, built, released, and upgraded outside this repository through a reproducible SDK whose source remains owned here. This track is the repository-side implementation of the Component Model described in `spec/requirement-document-v0.6.md` §2.1 and the Canonical IR / Component CLI phases of `spec/platform-development-plan-v0.6.md`, scoped to the existing seL4 product path rather than the full platform that document describes.

**Status:** CP0–CP10 complete, with CP7's hosted-publication clause deferred and named in its entry. `slime_os` stays authoritative; the SDK is a generated one-way mirror described by `contracts/component-sdk-release/v1`.

**Closes:** Backlog item **B70**, whose problem statement is the compile-time coupling this track removes (CP0–CP2).

**Motivating gap (confirmed 2026-08-17):** every one of `components/bins`'s 52 `[[bin]]` entries (`components/bins/Cargo.toml`, `autobins = false`) compiles against four files `components/bins/build.rs` privately generates from `contracts/generation-manifest/v1/fixtures/*.zti` by ad hoc string parsing, through three generator functions (`generate_boot_layout` at lines 57–66, `generate_command_profile` at lines 68–175 emitting both `command_profile.rs` and `dango_profile.rs`, `generate_fabric_profile` at lines 177–197) and `include!`s directly into component source at 19 sites across 17 files (confirmed at `components/bins/src/bin/spawn-service.rs:32`, `init.rs:37,118`, `fabric-publisher.rs:50`, and fourteen further files). `scripts/build/build-generation.py` builds every component with one command, `cargo build --release --target <profile> -p slime-components --bin <name> ...` (confirmed at lines 2413–2461), with no parameter accepting a component built anywhere else. The root-side admission path (`slime-root/src/generation.rs`, `child_vspace.rs`) needs no change to admit a new component; every blocker is in the host build pipeline and the single-crate `[[bin]]` design.

## Boundaries

- This track does not change seL4 capability enforcement, generation admission, or ELF structural validation; `slime-root/src/generation.rs` and `child_vspace.rs` are confirmed producer-agnostic already and CP0–CP5 add no code there beyond CP2's one new root-served query.
- CP4 introduces no per-component signature, provenance record, or new trust root. A generation containing an externally supplied component is trusted exactly as every generation is today, by the existing whole-generation release signature (`boot-contracts/src/release.rs::INITIAL_TRUST_ROOT`). Per-component provenance is separate future work, not part of this track.
- This track is independent of [D1–D7](08-native-development.md). D4's `ExecutableFactory`/`EXECUTABLE_ADMIT` authority is a *runtime*, in-booted-system, ephemeral admission mechanism for un-persisted dev-loop execution; CP4 is a *host-side, build-time* admission mechanism that produces an ordinary, persistent, normally signed generation. Neither depends on the other, and neither milestone should be implemented in terms of the other's mechanism.
- CP5's "separate checkout" means a genuinely distinct git repository, not a new directory inside this repository's Cargo workspace. CP3's in-repo crate split is a necessary mechanical precondition but is explicitly not sufficient by itself to claim components can be developed "out-of-tree" — that claim is CP5's alone, mirroring how this repository already refuses to conflate QEMU evidence with physical-board evidence elsewhere in this roadmap.
- CP5's completed exit condition covered a temporary pinned SDK bundle and the RP4 QEMU data path only. CP6–CP10 extended that evidence to a repository-owned deterministic exporter, a publication path proven against a local clone of the canonical repository, downloadable per-profile platform build inputs, evidence-backed compatibility declarations, and a pinned consumer upgrade and rollback path. They did not retroactively widen CP5's claim, and CP7's own entry records that no commit is hosted in the canonical repository yet.
- Converting every remaining `sel4-*.zti` fixture to spec-derived form is deferred follow-on work once CP1's generator is proven on `valid.zti` and one `sel4-*.zti`; CP1's exit condition does not require converting all of them.
- CP6–CP10 do not move authoritative runtime, protocol, contract, target, or toolchain sources out of this repository. The SDK repository is a generated release mirror with one-way synchronization from `slime_os`; fixes land here first and are exported, never patched independently in the mirror.
- SDK release metadata is a persisted cross-repository format and therefore remains a versioned Zutai contract under `contracts/`. The SDK adds no per-component signature, provenance trust root, or root-side admission path: ELF content hashes and the existing whole-generation release signature remain authoritative.
- Publishing a `bcm2712-rpi5` prefix proves that an external component can be built and target-qualified against that exact platform input. It does not claim physical Raspberry Pi 5 boot support; only the existing physical-board gates can make that claim.

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

- publish, or provide as a pinned, git-consumable vendored bundle, `slime-rt`, `slime-proto`, `boot-contracts`, the seL4 JSON target spec, and the exact `sel4-sys`/`SEL4_PREFIX`/`LIBCLANG_PATH` toolchain recipe as a versioned "component SDK" a checkout outside this repository can consume by pinned commit — no crates.io/registry publish, matching this repository's existing pinned-submodule convention (`deps/rust-sel4`, `deps/sel4`, `deps/zutai`, `deps/dango`);
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

**Status:** Complete, with one clause deferred and named below.

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

**Deferred clause:** the required check naming "the permanently hosted commit, not a temporary stand-in" is *not* met and is not claimed. Every arm above ran against a local bare clone of [`iceice666/slime_os-component_sdk`](https://github.com/iceice666/slime_os-component_sdk)'s `generated` branch, driven through the real publisher, so what is proven is the publisher's behavior rather than GitHub's. Publishing for real needs two things this milestone does not have: the recorded `slime_os` source commit present on `origin`, since a hosted SDK commit whose source commit is unpublished is an artifact nobody can regenerate, and a release identity whose signing key is not `contracts/release/v1/test-keys`. The branch protection, force-push denial, and credential boundary are GitHub settings rather than repository artifacts and are configured out of band, on the same terms this track already refuses to call QEMU evidence board evidence.

**Gates:** `just component_sdk_release_check`, `just component_sdk_export_check`, `just lint_all`, `just fmt_check_all`, `just ruff`.

**Evidence:** [`devlog/2026-08-25-cp6-cp10-component-sdk-releases/`](../devlog/2026-08-25-cp6-cp10-component-sdk-releases/index.md)

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
