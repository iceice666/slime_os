# Component specification and out-of-tree development track

**Purpose:** Give "component" and "system" a formal, versioned specification independent of any one generation manifest, so `contracts/generation/v1` fixtures are generated from that specification instead of hand-authored in parallel with it, and prove that a component can be authored, built, and admitted into a Slime OS generation entirely from outside this repository. This track is the repository-side implementation of the Component Model described in `spec/requirement-document-v0.6.md` §2.1 and the Canonical IR / Component CLI phases of `spec/platform-development-plan-v0.6.md`, scoped to what this repository's existing seL4 product path needs rather than the full platform that document describes.

**Status:** Not started.

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

**Status:** Not started.

**Depends on:** Cleared or explicitly deferred backlog.

Defines the component-level data `spec/requirement-document-v0.6.md` §2.1 describes (Identity, Purpose, Capability, Interface, Dependency, Communication, Configuration, Lifecycle, Runtime Requirement, Status/Health, Compatibility, Test Specification) as a real Zutai contract, independent of any one generation manifest.

### Deliverables

- add `contracts/component-spec/v1/schema.zt` defining a `ComponentSpec` record with: Identity (`name`, `type`, `version`, `owner`); `purpose` (Text); Capability (`provides : List Text`, `requires : List Text`); Interface references (named entries resolving to `contracts/interface-schema/v1/interfaces/*.zti` identities, tagged input/output/command/event); `dependencies : List Text` naming other `ComponentSpec` identities; Communication (`semantic`, and a QoS reference reusing `contracts/generation/v1/schema.zt`'s existing `FabricParticipant` QoS fields rather than inventing a second QoS vocabulary); Configuration (`parameters` with name/default/valid-range); Lifecycle (`states : List Text` drawn from the closed set `{Initialize, Configure, Start, Ready, Running, Degraded, Stop, Error}`); Runtime Requirement (`executionEnvironment`, `resourceRequirement`, `deviceRequirement`); Status/Health (a named health-check reference); Compatibility (`platform`, `interface`, `dependency`, `resource`, `runtime`, `qos` constraint fields); Test Specification (`testCondition`, `expectedResult`, `passFailCriteria`, `requiredTestEnvironment`);
- compute each record's identity the same way `contracts/interface-schema/v1/schema.zt` does: SHA-256 over a domain-separation prefix plus normalized, sorted-key, whitespace-free UTF-8 JSON bytes — reuse that exact normalization convention rather than defining a second one;
- add a `just component_spec_check` gate (new `scripts/check/check-component-spec.py`, following the shape of `scripts/check/check-contracts.py`) that structurally and semantically validates every `contracts/component-spec/v1` record: required identity fields present, `provides`/`requires` well-formed, every interface reference resolving to a real `contracts/interface-schema/v1/interfaces/*.zti` entry, every lifecycle state drawn from the closed set;
- author one `component-spec` record for every `Executable`/`Instance` pair currently declared in `contracts/generation/v1/fixtures/valid.zti`, as the first real corpus proving the schema can describe every existing component before anything is asked to derive from it.

### Required checks

- a `component-spec` record with an unresolvable interface reference, a lifecycle state outside the closed set, or a missing identity field is rejected with a location, mirroring the Zutai `zutai-cli check` error shape;
- two independently produced normalized encodings of the same `component-spec` content compute the same identity hash;
- every component named in `valid.zti`'s `executables`/`instances` has a corresponding, schema-valid `component-spec` record, verified by `just component_spec_check`.

### Planned verification target

```sh
just component_spec_check
```

### Exit condition

`contracts/component-spec/v1` exists, is validated by a real gate, and every component in the reference `valid.zti` generation has a corresponding `component-spec` record with a stable computed identity.

## CP1 — System specification model and generation derivation

**Status:** Not started.

**Depends on:** CP0.

### Deliverables

- add `contracts/system-spec/v1/schema.zt` defining a `SystemSpec` record per `spec/requirement-document-v0.6.md` §4.2: `components : List Text` (referencing `component-spec` identities), component relationships (reusing `contracts/generation/v1/schema.zt`'s existing `FabricRoute`/`FabricParticipant` shape rather than a second graph representation), `interfaces`, `dependencies`, `configuration`, a `targetRequirement` reference into `contracts/target-profile/v1`, `runtimeRequirement`, `deploymentConstraint`, and `acceptanceCriteria`;
- write a host-side generator (`scripts/generate/generate-generation-from-spec.py`, alongside the existing `scripts/generate/` family) that produces a `contracts/generation/v1` `GenerationManifest`'s `executables`, `instances`, `objects`, `grants`, and `fabricGraph` sections from a `system-spec` record plus its referenced `component-spec` records, so those sections are generated output rather than independently hand-authored `.zti` text;
- re-derive `contracts/generation/v1/fixtures/valid.zti` and the smallest `sel4-*.zti` fixture from a hand-written `system-spec`/`component-spec` source, diffing the generator's output against the previously committed fixture and reconciling any divergence by fixing the schema or generator, never by special-casing the fixture;
- extend `just contracts_check` with `component-spec`/`system-spec` validation alongside the existing `--check` drift gates, and add a derivation-drift check analogous to the existing `scripts/generate/*.py --check` pattern: regenerating a `.zti` from its `system-spec` source must byte-match the committed fixture.

### Required checks

- a `system-spec` referencing an undeclared `component-spec` identity, an interface neither component provides nor requires, or a target profile `contracts/target-profile/v1` does not name is rejected before generation;
- the derived `valid.zti` and the chosen `sel4-*.zti` are byte-identical to the previously hand-authored fixtures, or the divergence is a deliberate, recorded fixture update in the same change;
- `just generation_check` and `just contracts_check` pass unchanged in observed behavior after the derivation is wired in.

### Planned verification target

```sh
just system_spec_check
```

(plus existing `just contracts_check`, `just generation_check`)

### Exit condition

`contracts/generation/v1/fixtures/valid.zti` and the smallest `sel4-*.zti` are generated artifacts derived from `component-spec`/`system-spec` sources rather than independently hand-authored text. Converting the remaining `sel4-*.zti` fixtures is deferred follow-on work, not required by this exit condition.

## CP2 — Runtime-resolved component binding

**Status:** Not started.

**Depends on:** Cleared or explicitly deferred backlog. Independent of CP0/CP1.

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

**Status:** Not started.

**Depends on:** CP2.

### Deliverables

- replace `components/bins`'s single-crate, 52-entry `[[bin]]` population with one independent crate per component, each depending on `slime-rt`, `slime-proto`, and `boot-contracts` directly rather than sharing one crate's `Cargo.toml`/`build.rs`; this closes the [B65 deferred follow-up](00-backlog.md) ("the 52-binary fixture population uncollapsed");
- extract `components/bins/build.rs`'s manifest-parsing helpers (already reduced in scope by CP2, since the `OUT_DIR` generators are gone) into a shared, documented build-support crate or module every component crate — in-tree or out-of-tree — can depend on, replacing the current private, unexported, ad hoc string-splitting parser;
- scope the `store` feature's allocator requirement to only the crates that themselves depend on `boot-contracts/gpt` or `slime-rt/heap`, removing the crate-wide `#[global_allocator]` contagion `components/bins/Cargo.toml`'s existing comment documents (lines ~23–28);
- update `scripts/build/build-generation.py`'s `build_rust_components()` to invoke one `cargo build` per component crate (or a single command naming each crate's package) instead of one batched `-p slime-components --bin A --bin B ...` invocation, preserving byte-identical output for every existing in-tree component;
- state the syscall ABI's compatibility policy in `docs/syscall-abi.md` (for example: operation labels are additive-only within a format version; a breaking change is a new `contracts/syscall-abi/v1` major version) so a component crate outside this workspace has a documented contract to build against.

### Required checks

- every existing `just sel4_*_check` gate passes unchanged after the crate split;
- a new component can be added by creating one new crate directory and one generation-manifest entry, without editing any other component's crate or its `Cargo.toml`;
- the `store` feature no longer forces `#[global_allocator]` on a component crate that does not itself depend on `boot-contracts/gpt` or `slime-rt/heap`.

### Planned verification target

```sh
just component_crate_split_check
```

(plus the existing full `just sel4_*_check` gate suite and `just sel4_gate_control_check`)

### Exit condition

`components/bins`'s hand-listed `[[bin]]` entries are replaced by independent crates sharing `slime-rt`, `slime-proto`, `boot-contracts`, and a documented build-support library; every existing seL4 gate observes unchanged behavior; the B65 deferred follow-up is closed; `docs/syscall-abi.md` states a compatibility policy.

## CP4 — External-artifact admission path

**Status:** Not started.

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

## CP5 — Out-of-tree component development proof

**Status:** Not started.

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
