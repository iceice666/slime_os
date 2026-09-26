# Component, system, and image boundaries

Slime OS separates five identities that chronological delivery documents often
collapsed. Each owns a different question.

## Component specification

`contracts/component-spec/v1/` describes a component independent of any one
composition: implementation provider, declared interfaces and QoS,
configuration, lifecycle vocabulary, resource requirements, health, compatibility,
and the gate that observes it. It is host-side normalized data, not a boot wire
format.

A spec cannot grant authority to another component because it cannot know which
other instances a system will compose.

## System specification

`contracts/system-spec/v1/` describes one composition: selected component specs,
instances, authority edges, notification bindings, slot pins, state bindings,
fabric graph, resource objects, boot profile, and target requirement. System
grants are the irreducible authority facts.

`scripts/generate/generate-generation-from-spec.py` derives the generation
manifest from this record and the referenced component specs. A slot pin is an
exception with a declared reason; the default is deterministic assignment.

## Generation and component image

The generated manifest under `contracts/generation-manifest/v1/` is encoded into
the versioned generation wire format. `boot-contracts/src/generation.rs` decodes
it and `slime-root/src/generation.rs` admits it before constructing the graph.
The generation binds executable object digests, target identity, authority,
resource budgets, state policy, and boot profile.

Executable payloads use the component image contract under
`contracts/component/v2/`, decoded by
`boot-contracts/src/component_image.rs` and mapped by
`slime-root/src/child_vspace.rs`. Integrity comes from the generation object
digest and authority from generation grants; an executable image carries
neither independently.

Retained old executable revisions remain classified so old artifacts are not
misread. Superseded generation wire formats are rollback-safe by refusal, not by
runtime migration.

### Delivery ELF metadata

The generation builder validates each native ELF before making a stripped delivery
copy with `llvm-strip` from the explicitly selected Rust toolchain. Workspace build
artifacts and externally supplied files remain unchanged. An external component's
declared content hash authenticates the original input; the generation's executable
object digest authenticates the wrapped delivery bytes.

The symbol table is not entirely optional: `ChildImage::worker()` resolves
`__slime_rt_worker_entrypoint` and `__slime_rt_worker_stack` from it. Delivery
stripping retains these two symbols and their values/sizes for two-thread images.

The delivery check preserves entry points, LOAD offsets, virtual addresses, sizes,
permissions, zero-fill extents and segment contents. Section-table header metadata
may change. Nonloaded metadata such as RISC-V attributes may move in the file, but
its attributes and bytes must remain identical. The stripped image is admitted
again before wrapping; stripping cannot make a previously refused input acceptable.

The root builder likewise embeds `slime-root-child.delivery.elf` beside the
symbol-bearing `slime-root-child.elf` in the child Cargo output directory. It keeps
the child fixture and all its boot/fault checks. Never apply this stripping step
blindly to the final loader ELF: the packaged image contains sectionless payloads.

The root-only `root-image` Cargo profile is separate from the component and host
release profiles. Its build disables target-default unwind tables because the
pinned root uses `panic=abort`; resource rollback and fault supervision remain
active and are not Rust stack unwinding. Original root symbols remain available
in the `root-image/slime-root.elf` Cargo output and copied root artifact.

File-size reductions are not automatically RAM or CSlot reductions. Loaded memory
includes zero-fill, page alignment and reserved stacks; moving data to BSS alone
does not eliminate that footprint.

## System-image closure

`contracts/system-image-closure/v2/` names every identity-bearing input to a
reproducible bootable-image build: system spec, selected implementations, target
and platform prefix, root and loader roles, release inputs, build parameters,
and expected outputs. Paths are locators; their recorded identities are what the
resolver trusts.
The release inputs also bind the generation builder, image builder and ELF
delivery helper, so a packaging change cannot reuse an old closure-cache image.

An executable-changing scenario is part of the selected implementation or root
role. A deliberately invalid build is a separate `NegativeBuildCase`, never a
valid image closure. The generic resolver verifies declared inputs before build
and emits a build-result record binding the generation, root, loader, packaged
image, and identity manifest to the closure identity.

The component SDK is a generated, one-way release mirror. Authoritative source,
contracts, target definitions, and fixes remain in this product repository.

## System test run

`contracts/system-test-run/v1/` is deliberately separate. It declares the
execution profile, disks, networks, devices, runtime fault controls, timeout,
marker contract, and forbidden outcomes for one image closure. Changing an
oracle or timeout must not change executable identity.

The committed records are frozen checker declarations, not independent execution
requests. `compile_test_run_declaration` accepts paired empty fixture path/identity
or fault target/value fields as unbound declarations. Empty image identity is
allowed only where the owning gate confirms an explicit closure exemption.
`compile_test_run` remains the strict boundary used by SDK consumers: it refuses
all such placeholders. A populated digest is a declaration of expected content,
not proof that a fixture exists or matches it; the executing checker owns that
verification. No declaration alone proves that a test ran.

After an intended image-closure change, update only its test-run references with
`python3 scripts/generate/generate-system-test-runs.py --refresh-identities`.
This checks the entire batch before writing and refuses any execution-field
drift. Review genuine checker-input changes separately before using
`just system_test_run_bless`, then run `just system_test_run_check` and
`just system_image_closure_check`. The fixed private-memory records do not inherit
the adaptive-only transport from the same checker module.

A build closure proves reproducibility of bytes. An observed test execution
proves behavior within its tested scope. Neither substitutes for the other, and
a QEMU run cannot establish a physical-board observation.

## Qualification limits

- **Target qualification:** A published `bcm2712-rpi5` platform prefix qualifies
  external component build and admission against that platform input only; it
  does not claim Raspberry Pi 5 boot support, which only a physical-board gate
  qualifies.
- **Host-side scope:** System-image closure, scenario, builder, and SDK-consumer
  machinery is host-side; it does not implement in-system compilation, executable
  admission, or live update, and does not widen physical-board qualification.
- **Hosted publication:** The controlled-remote publication gate proves publisher
  behavior—publication, refusal, regeneration, build/boot of the published SDK,
  and atomic cleanup leaving no branch commit when the remote refuses the tag—not
  the hosted repository's current branch/tag protection, credential scope, or
  deployed state. Actual release qualification must check the hosted repository.
- **Upgrade coverage:** Retained-generation admissibility and console fixtures in
  the upgrade/rollback check are not an observed QEMU health-confirmation failure;
  that failure path is unexercised.
- **Reviewed non-closure exceptions:** The exceptions retain `--component-graph`
  for mixed-source SDK admission/upgrade generations; `--demo-plane`,
  `--generation-plane`, and `--rollback-plane` for non-closure arms;
  `--sample-plane` for Milk-V Duo; `--boot-selection`; and `--skip-pin-check`:
  `check-sel4-boot-selection.py` needs per-arm `boot_bundle_identity` test-run data;
  `check-sel4-root-boot.py` has no plane-specific closure;
  `check-sel4-demo-plane.py` has a closure-less boot-selection arm and a
  wrong-target arm requiring scrubbed input; and `check-sel4-generation-plane.py`
  and `check-sel4-rollback-plane.py` have RV64 arms outside their QEMU-AArch64
  (`qemu-arm-virt`) closures. These are reviewed exceptions, not unfinished
  ordinary-plane migration: legacy-only `SLIME_*` controls remain confined to the
  aggregate reachability inventory, whose reachability rule is the source guard,
  with no independent hard-coded-path or flag guard claimed; remove entries when
  their paths migrate.

## Build and admission flow

```text
component specs + system spec
    -> derived generation manifest
    -> versioned generation bytes
    -> system-image closure resolves exact build inputs
    -> root/components/loader/image plus build-result identities
    -> system-test-run supplies execution-only inputs and oracle
    -> root admits the embedded generation and launches the declared graph
```

## Verification

- `just component_spec_check`
- `just system_spec_check`
- `just generation_check`
- `just system_image_closure_check`
- `just system_image_builder_check`
- `just system_image_closure_aggregate_check`
- `just system_test_run_check`
- `just sel4_boot_layout_check` for frozen capability layouts
