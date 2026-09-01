# The SDK could not build a component for the board P3.F had just qualified

| Field | Value |
|---|---|
| Date | 2026-09-01 |
| Kind | Change |
| Status | Verified |
| Scope | `scripts/lib/component_sdk.py`, `scripts/check/check-component-sdk-{export,release}.py` |
| Roadmap | CP8, P3.E, P3.F |
| Gates | `just component_sdk_export_check`, `just component_sdk_preflight` |
| Trigger | Preparing SDK 3.0.0 after P3/P3.E/P3.F landed showed the repository declaring four target profiles while the exporter could produce assets for two |
| Baseline | CP8 shipped per-profile seL4 prefixes for `aarch64-sel4-qemu-virt` and `aarch64-rpi5`; `PROFILE_PLATFORMS` had no RISC-V entry and the exporter copied exactly one target specification |

## Summary

The Milk-V Duo lane qualified upstream seL4, `slime-root`, a target-qualified
generation, and a resident Slisp shell on a named board — and an out-of-tree
component could not be built for it. `contracts/target-profile/v1` declares
`riscv64-sel4-qemu-virt` and `riscv64-sel4-milkv-duo`, but
`scripts/lib/component_sdk.py` had no RISC-V entry at all, so the SDK's
exportable set was half the product's. The blocker was structural rather than a
missing table row: the exporter hard-coded one `TARGET_SPEC_SOURCE`, and both
RV64 profiles build against `riscv64imac-sel4-minimal.json`, so a single
exported specification could not describe them. Target specifications are now
per-profile and both RV64 profiles are declared, with every value read from the
product build rather than invented. Adding them is a `compatible-feature` by
CP9's own classifier, which is what keeps this separable from the pending
breaking 3.0.0 release.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `scripts/lib/component_sdk.py` | `TARGET_SPEC_SOURCE`/`TARGET_SPEC_SDK` replaced by `TARGET_SPECS` plus `target_spec_sdk_path()`; the export copies every declared specification | Two architectures ship two specifications, so a profile's `cargoTarget` names bytes that exist in the tree |
| same | `targetSpecHash` is digested from the specification each profile actually names | Binding both architectures to one hash would let a changed RV64 target leave every recorded digest untouched |
| same | `riscv64-sel4-qemu-virt` and `riscv64-sel4-milkv-duo` added, each with its own platform, prefix, and `sel4/pins.toml` section | A QEMU reference and a named board are distinct exact identities; the Duo's C906, firmware handoff, PLIC, timer, and memory window are not interchangeable with QEMU `virt` |
| same | New `target_spec_source_paths()` helper | The two gates that mirror the export's inputs derive them from the exporter instead of restating one path — the same drift that once made the linker scripts an unknown export input |
| `check-component-sdk-{export,release}.py` | Both mirror lists consume that helper | A gate's input mirror cannot silently omit a specification the exporter reads |

`DEFAULT_PROFILES` is deliberately unchanged. Which profiles exist and which a
defaulted export publishes are separate decisions, and changing both at once
would make the next release's profile set depend on this commit rather than on
the publisher's arguments.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A profile names a specification the export never copied | `just component_sdk_export_check` | The record's `cargoTarget` resolves to a missing file, or `tools/sdk-build.py` fails its recorded-hash comparison |
| Two architectures collapse onto one target digest | Per-profile `targetSpecHash` | An RV64 specification change leaves the recorded hash unmoved |
| A gate's input mirror drifts from the exporter | `target_spec_source_paths()` | A specification the exporter reads is absent from the mirrored tree |
| The RV64 profiles become interchangeable | Distinct prefixes and pins sections per profile | Both profiles report one prefix identity |
| A component built for one architecture is admitted by another | `component_image` target admission | The AArch64 profile accepts RISC-V ELF bytes |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Export with all four profiles | Pass: SDK 3.1.0 from `c42ae22`, tree identity `bb7db2e7…`; four prefix archives and two target specifications declared; 24/32 files, 4/4 profiles | Direct |
| Per-profile target digests | Both RV64 profiles report target `85e4e62f…` (one shared specification) with distinct prefixes `0a243642…` and `8690883a…`; `aarch64-rpi5` reports an empty hash as a bare triple | Direct |
| `tools/sdk-build.py --profile riscv64-sel4-qemu-virt` | Pass: built `external-component.elf` from the exported tree; `file` reports `UCB RISC-V, RVC, soft-float`, `e_machine = 243`, matching the profile's declared `elfMachine` | Direct |
| `--profile riscv64-sel4-milkv-duo` | Pass: byte-identical ELF to the QEMU-RV64 build. Expected — `SLIME_TARGET_PROFILE` reaches an image only through `option_env!` in `boot_contracts::target_profile`, and the template component does not read it; qualification is stamped into the component *image* header, not the bare ELF | Direct |
| RV64 component-image admission | Pass: the RISC-V ELF entered a `riscv64-sel4-qemu-virt` (id 6) component image, 38432 bytes | Direct |
| Cross-profile refusal | The same bytes against `aarch64-sel4-qemu-virt` were refused: "not a static executable for target aarch64-sel4-qemu-virt" | Direct |
| Classification of the added profiles | `changed_axes` reports no breaking axis and `profiles:added:riscv64-sel4-milkv-duo,riscv64-sel4-qemu-virt`; `classify` returns `compatible-feature`; `3.0.1` refused, `3.1.0` admitted | Direct |
| `just component_sdk_export_check` | Pass: two isolated exports byte-identical, four identity-sensitivity probes, 5 crate and 46 contract identities recomputed, five SDK crates resolved through a pinned commit with no path into this checkout, and an SDK-built external component booted the QEMU component graph | Direct |
| `just component_sdk_preflight` | Pass: still reports `breaking`/`3.0.0` against hosted 2.0.0, and names the two RV64 profiles as `NOT COMPARED` because the hosted release does not declare them | Direct |
| `just ruff`, `just typos` | Pass | Direct |
| Physical RV64 boot of an SDK-built component | Not attempted. The Duo profile's assets are qualified host-side only; no board boot of an out-of-tree RV64 component was observed | — |

## Decisions

- **Decision:** make target specifications per-profile rather than adding a second hard-coded constant.
  **Rationale:** the profile table already keys everything else per profile, and both RV64 profiles share one specification while `aarch64-rpi5` needs none. A second constant would have encoded "there are exactly two architectures" in a third place.
  **Rejected alternative:** export only `riscv64imac-sel4-minimal.json` beside the existing one and keep a single `target_spec_hash`, which would bind both architectures to one digest.

- **Decision:** declare both RV64 profiles now, and leave `DEFAULT_PROFILES` alone.
  **Rationale:** the repository qualified both a QEMU RV64 reference and a named board, and omitting either would leave the SDK unable to describe work the product has already done. Which profiles a defaulted export publishes is the publisher's argument, not a property of this table.
  **Rejected alternative:** add only the Duo profile, which would make the RV64 QEMU reference — the profile P3.E consumes before any board claim — unbuildable out of tree.

- **Decision:** record the identical RV64 ELFs as expected rather than treating them as a defect.
  **Rationale:** verified at the source. `slime-build-support` exports `SLIME_TARGET_PROFILE` as a `rustc-env`, and only `boot_contracts::target_profile::current()` reads it through `option_env!`; a component that never calls it compiles identically for two profiles sharing one Cargo target. The qualification that separates them is the component image header, which admission checks — demonstrated by the AArch64 refusal above.
  **Rejected alternative:** force a per-profile difference into the ELF, which would add a build input nothing needs and break reproducibility for no invariant.

## Open risks and follow-ups

- [ ] No out-of-tree RV64 component has been booted, on QEMU `virt` or on the Duo. This change proves host-side build and admission only; a boot arm belongs with whichever gate takes ownership of RV64 external components.
- [ ] `sdk/compatibility-matrix.*` gains no row here. The matrix still holds only the 1.0.0/1.1.0 rows against product `726ebb0`, so hosted 2.0.0 and any RV64 pairing remain unsupported by CP9's rule until a release publishes and is classified.
- [ ] `maxProfiles` is 4 and the exporter now declares exactly 4. A fifth profile needs a `contracts/component-sdk-release/v1` bound change, which is a format change rather than a table addition.
- [ ] The Duo profile ships a prefix built from `sel4/pins.toml`'s `[observed_prefix_cv1800b_duo]`, whose values were read from one named board. Publishing it asserts a reproducible build input, not that any other Duo behaves identically.

## Artifacts and provenance

- Related roadmap items: [CP8](../../roadmap/10-component-platform.md), [P3.E](../../roadmap/07-architecture-portability.md), [P3.F](../../roadmap/07-architecture-portability.md)
- Predecessor entry: [`devlog/2026-09-01-sdk-publication-guards/`](../2026-09-01-sdk-publication-guards/index.md)
- Evidence: the export, per-profile digests, RV64 build, admission, cross-profile refusal, classification, and gate results above were observed in this session against `origin/main` at `c42ae22`; no raw transcript retained
