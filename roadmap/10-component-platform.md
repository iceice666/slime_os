# Component specification and out-of-tree development track

**Purpose:** Give "component" and "system" a formal, versioned specification independent of any one generation manifest, so `contracts/generation/v1` fixtures are generated from that specification instead of hand-authored in parallel with it, and prove that a component can be authored, built, and admitted into a Slime OS generation entirely from outside this repository. This track is the repository-side implementation of the Component Model described in `spec/requirement-document-v0.6.md` §2.1 and the Canonical IR / Component CLI phases of `spec/platform-development-plan-v0.6.md`, scoped to what this repository's existing seL4 product path needs rather than the full platform that document describes.

**Status:** CP0, CP1, CP3, and CP4 complete; CP2's mechanism landed with its migration partial; CP5 not started.

**Closes:** Backlog item **B70**, whose problem statement is the compile-time coupling this track removes (CP0–CP2).

**Motivating gap (confirmed 2026-08-17):** every one of `components/bins`'s 52 `[[bin]]` entries (`components/bins/Cargo.toml`, `autobins = false`) compiles against four files `components/bins/build.rs` privately generates from `contracts/generation/v1/fixtures/*.zti` by ad hoc string parsing, through three generator functions (`generate_boot_layout` at lines 57–66, `generate_command_profile` at lines 68–175 emitting both `command_profile.rs` and `dango_profile.rs`, `generate_fabric_profile` at lines 177–197) and `include!`s directly into component source at 19 sites across 17 files (confirmed at `components/bins/src/bin/spawn-service.rs:32`, `init.rs:37,118`, `fabric-publisher.rs:50`, and fourteen further files). `scripts/build/build-generation.py` builds every component with one command, `cargo build --release --target <profile> -p slime-components --bin <name> ...` (confirmed at lines 2413–2461), with no parameter accepting a component built anywhere else. The root-side admission path (`slime-root/src/generation.rs`, `child_vspace.rs`) needs no change to admit a new component; every blocker is in the host build pipeline and the single-crate `[[bin]]` design.

## Boundaries

- This track does not change seL4 capability enforcement, generation admission, or ELF structural validation; `slime-root/src/generation.rs` and `child_vspace.rs` are confirmed producer-agnostic already and CP0–CP5 add no code there beyond CP2's one new root-served query.
- CP4 introduces no per-component signature, provenance record, or new trust root. A generation containing an externally supplied component is trusted exactly as every generation is today, by the existing whole-generation release signature (`boot-contracts/src/release.rs::INITIAL_TRUST_ROOT`). Per-component provenance is separate future work, not part of this track.
- This track is independent of [D1–D7](08-native-development.md). D4's `ExecutableFactory`/`EXECUTABLE_ADMIT` authority is a *runtime*, in-booted-system, ephemeral admission mechanism for un-persisted dev-loop execution; CP4 is a *host-side, build-time* admission mechanism that produces an ordinary, persistent, normally signed generation. Neither depends on the other, and neither milestone should be implemented in terms of the other's mechanism.
- CP5's "separate checkout" means a genuinely distinct git repository, not a new directory inside this repository's Cargo workspace. CP3's in-repo crate split is a necessary mechanical precondition but is explicitly not sufficient by itself to claim components can be developed "out-of-tree" — that claim is CP5's alone, mirroring how this repository already refuses to conflate QEMU evidence with physical-board evidence elsewhere in this roadmap.
- Out-of-tree development is proven for [RP4](09-rpi5-ros2-demo.md#rp4--arm-component-data-path-on-qemu-and-raspberry-pi-5)'s two Arm data-path components only. Extending it to RP6's ROS 2 node components, publishing a public registry, or a hosted SDK release, is not part of this track's exit condition.
- Converting every remaining `sel4-*.zti` fixture to spec-derived form is deferred follow-on work once CP1's generator is proven on `valid.zti` and one `sel4-*.zti`; CP1's exit condition does not require converting all of them.

## Sequencing

1. CP0 and CP2 have no dependency on each other and may proceed in parallel once the backlog is clear: CP0 is a host-side contract/schema change, CP2 is a root/component runtime change.
2. CP1 depends on CP0 and re-derives the generation fixtures from the component/system specs it defines.
3. CP3 depends on CP2's runtime binding resolution: splitting components into independent crates is only useful once no crate-shared `build.rs` output is required at compile time.
4. CP4 depends on CP0 (a place to declare a component's implementation as workspace-built or externally supplied), CP2 (so an externally built component needs no `OUT_DIR`-generated file), and CP3 (the crate convention an external build reproduces).
5. CP5 depends on CP3 and CP4, and is what [RP4](09-rpi5-ros2-demo.md#rp4--arm-component-data-path-on-qemu-and-raspberry-pi-5) now requires before its own exit condition is satisfied.

## CP0 — Component specification model

**Status:** Complete.

**Delivered:** `contracts/component-spec/v1` declares a bounded, closed-vocabulary `ComponentSpec` covering the twelve sections `spec/requirement-document-v0.6.md` §2.1 names, with 42 records — one per component `contracts/generation/v1/fixtures/valid.zti` declares — each cross-checked field by field against that manifest and its fabric graph, and each identified by `contracts/interface-schema/v1`'s own normalization convention rather than a second one.

**Exit condition (observed):** `just component_spec_check` validates all 42 records against the 4 declared interfaces and the reference generation, refusing 37 named malformations with stable identities, and reports the two components the repository declares but ships no implementation for (`generation-list`, `storage-store-probe`); `just contracts_check` type-checks the new contract and its generated bindings.

**Gates:** `just component_spec_check`, `just contracts_check`.

**Evidence:** [`devlog/2026-08-18-cp0-component-spec-model/`](../devlog/2026-08-18-cp0-component-spec-model/index.md)

## CP1 — System specification model and generation derivation

**Status:** Complete.

**Delivered:** `contracts/system-spec/v1` declares a composition — its components, authority edges, notifications, fabric graph, and boot profiles — and `scripts/generate/generate-generation-from-spec.py` derives a `contracts/generation/v1` manifest's `executables`, `instances`, `objects`, `sharedBufferBudget`, and `health.requiredInstances` from it plus the CP0 component corpus. Reaching byte equivalence required sorting `declared_spawn_grant_counts`, whose raw-manifest-order output reached `init`'s compiled `FABRIC_MINTED_GRANTS` and so made instance order change a component ELF.

**Exit condition (observed):** `contracts/generation/v1/fixtures/valid.zti` and `sel4-channel.zti` are generator output, regenerate byte-identically under `--check`, and reproduce the frozen pre-CP1 baselines; building `sel4-channel` from the derived fixture yields byte-identical `generation.bin` and `boot-store.bin`, and `sel4_channel_check`, `sel4_component_graph_check`, `sel4_boot_check`, `sel4_generation_check`, `sel4_dango_check`, and `sel4_boot_layout_check` (25 plane layouts) all pass on QEMU.

**Gates:** `just system_spec_check`, `just contracts_check`, `just generation_check`, `just sel4_boot_check`, `just sel4_generation_check`.

**Evidence:** [`devlog/2026-08-18-cp1-generation-derivation/`](../devlog/2026-08-18-cp1-generation-derivation/index.md)

## CP2 — Runtime-resolved component binding

**Status:** Mechanism complete and gated, answering grant bindings, namespaced boot-layout roles, unambiguous capability roles, the fabric-graph read, and the generation's boot action; the site-by-site migration is partial — 9 `include!` sites remain, all `fabric_profile` readers blocked on declared bounds that size fixed arrays rather than on any missing query.

**Progress (2026-08-21, boot action):** `CAPABILITY BOOT ACTION` (label 40) answers `BootAction`'s frozen id, so a component reads which composition it was booted into instead of `include!`ing a `build.rs`-private per-plane string. Five sites migrated and six `include!`s deleted, taking the all-profile count from 15 to 9. Gated on the *lifecycle* service rather than the capability table its label namespace names: the query must be answerable to every launched instance, and 30 of the 182 instances the seL4 fixtures declare hold no capability-transfer service where 0 lack lifecycle. **Evidence:** [`devlog/2026-08-21-b70-boot-action-query/`](../devlog/2026-08-21-b70-boot-action-query/index.md)

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
`contracts/generation/v1` format change. `init`'s remaining ~134 boot-layout
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
