# CP5: two out-of-tree data-path components boot through a pinned SDK

| Field | Value |
|---|---|
| Date | 2026-08-22 |
| Kind | Change |
| Status | Verified |
| Scope | `scripts/check/check-component-sdk-out-of-tree.py`, `scripts/check/check-generation.py`, `Justfile` |
| Roadmap | CP5 |
| Gates | `just component_sdk_out_of_tree_check` |
| Trigger | CP5 required the RP4 producer and consumer to build in a separate git checkout against a pinned component SDK, enter the ordinary external-artifact path together, and boot without changing root-side admission |
| Baseline | CP4 admitted one independently built ELF, but no external repository consumed a versioned SDK bundle or supplied both sides of the bounded Arm data path |

## Summary

The CP5 gate materializes a git-consumable Slime component SDK containing `slime-rt`, `slime-proto`, `boot-contracts`, `slime-components`, `slime-build-support`, the pinned `rust-sel4` source and AArch64 target, a generated demo fabric profile, and a documented toolchain recipe. A second temporary git repository depends only on that exact SDK commit and builds the RP4 large-sample publisher and bounded subscriber as independent crates. Their content-bound ELFs enter one signed `sel4-demo` generation through CP4, and the exact generation boots on seL4/QEMU.

The baseline scenario observes the existing bounded sample exchange, route denial, subscriber-authority re-delegation refusal, quota refusal, and final reclamation. Three additional content-distinct external builds prove producer peer death, malformed descriptor rejection, and wrong-type rejection. Each rejected loan is returned before the producer sends a fresh valid terminal sample, so failure remains fail-closed without weakening the unchanged demo's bounded-data-path completion. The external checkout is then removed before the ordinary in-tree demo is rebuilt and booted, proving the SDK path is additive.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Pinned component SDK bundle | Copies the public component crates, build support, target specification, pinned `rust-sel4` source, generated demo profile, and operator recipe into a temporary git repository and records the exact commit | An external checkout can resolve every component dependency and toolchain input through one immutable SDK revision rather than ambient paths into this repository |
| Separate RP4 repository | Creates and commits independent producer and consumer crates, strips repository-private trace modules, adds focused authority/resource markers, and resolves dependencies through the SDK git URL | The two data-path executables are genuinely authored and built outside the Slime workspace with no `components/` path dependency |
| External generation proof | Rewrites only the temporary component specs to bind both external ELFs by SHA-256, builds and host-checks the signed generation, embeds that exact generation, and boots the existing demo gate | CP4 remains the only artifact-admission path and whole-generation signing remains the only trust boundary |
| Failure scenarios | Baseline checks publisher authority refusal and quota exhaustion; separate builds inject producer death, a malformed descriptor, and a wrong type tag, with rejected loans followed by a fresh valid terminal sample | External components retain the same fail-closed authority, validation, and reclamation behavior as the in-tree RP4 data path while the unchanged composition still completes |
| Host generation checker | Admits kernel-object kind 7, the notification kind already emitted by current seL4 generations and admitted by the root decoder | The independent host check covers current demo generations instead of rejecting their valid notification objects |
| Fallback | Deletes the external checkout, rebuilds the unmodified in-tree demo, and re-runs its transcript assertions | External development is an additive producer path, not a fork or persistent mutation of the demo composition |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The external repository reaches into this checkout's component sources | Metadata and source-boundary checks in `component_sdk_out_of_tree_check` | Any non-SDK dependency resolves under the Slime checkout or source text names its `components/` tree |
| SDK consumers float away from the exported revision | Cargo metadata must report every SDK package as `git+...#<exact revision>` | A package resolves from a path, registry, or different commit |
| One or both artifacts bypass CP4 | Component specs bind `cp5-fabric-publisher-b` and `cp5-fabric-subscriber` to their exact SHA-256 and the builder must report both as external | Missing external-source report, digest mismatch, generation admission failure, or signed-store identity mismatch |
| The boot image differs from the generation independently checked by the gate | The seL4 identity manifest is compared with the checked generation identity | Embedded generation identity differs |
| External authority, validation, or resource behavior weakens | Required serial markers plus the existing `sel4-demo` transcript checker across baseline, peer-death, malformed, and wrong-type generations | Publisher re-delegation succeeds, quota expands, malformed or wrong-type data is admitted, rejection strands a loan, peer death is absent, sample exchange fails, or the graph does not reclaim to healthy completion |
| External state contaminates normal builds | Checkout removal followed by an in-tree rebuild and boot | External marker survives or fallback transcript fails |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just component_sdk_out_of_tree_check` | pass; the pinned SDK built two components in a distinct git repository, both content-bound ELFs entered signed demo generations, baseline/peer-death/malformed/wrong-type QEMU boots passed, the checkout was removed, and the in-tree fallback boot passed | Direct |
| External repository boundary | pass; Cargo metadata resolved SDK packages through the exact temporary git commit and no source or dependency escaped to this repository's `components/` tree | Direct |
| Baseline external boot | pass; bounded large sample exchanged, publisher could not re-delegate subscriber authority, a second live buffer above its declared buffer-count quota was refused, and the graph reached healthy reclamation | Direct |
| Failure external boots | pass; peer death reached the external subscriber, malformed and wrong-type descriptors were rejected with their loans returned, each rejection generation then exchanged a fresh valid terminal sample, and all three graphs completed cleanly | Direct |
| `just external_component_admission_check`, `just generation_check`, and contract checks (dependencies of the CP5 gate) | pass | Direct |
| `just lint_all`, `just fmt_check_all`, `just machete`, `just test_host`, `just ruff` | pass; host tests included 209 `boot-contracts` tests, protocol tests, and 4 build-support tests | Direct |
| Fresh reviewer passes | confirmed the initial implementation under-proved CP5; fixes added a distinct git SDK revision, two external components, authority/quota assertions, peer-death/malformed/wrong-type boots, notification-aware host admission, and fallback cleanup | Direct |

## Decisions

- **Decision:** publish the SDK for this milestone as a temporary pinned git bundle assembled by the executable gate rather than committing copied crate sources into a second permanent tree.
  **Rationale:** CP5 requires a git-consumable versioned boundary and a separate checkout, not a second maintained copy of the repository's public crates. The gate proves the exact bundle from current sources and commit pin every time it runs.
  **Rejected alternative:** add a permanent vendored SDK directory that would drift beside the workspace crates.

- **Decision:** export `slime-components` and `slime-build-support` in addition to the minimum runtime/protocol crates named by the deliverable.
  **Rationale:** CP3's crate-per-component convention includes a public build script that generates the command and fabric profiles; reimplementing that parser in the external repository would create a second convention and violate the existing SDK boundary.
  **Rejected alternative:** copy generated Rust tables or duplicate the manifest parser in each external component.

- **Decision:** use the existing `sel4-demo` composition and replace only the two implementation artifacts through CP4.
  **Rationale:** this preserves RP4's declared routes, grants, quotas, and supervision topology while proving that artifact producer location is irrelevant to admission and runtime behavior.
  **Rejected alternative:** create a second demo manifest whose authority could silently diverge from the already verified RP4 slice.

- **Decision:** prove peer death, malformed descriptors, and wrong type tags in separate content-distinct external builds selected by a compile-time environment flag.
  **Rationale:** a clean baseline must still observe bounded sample exchange; peer death requires omitting the terminal flag, while rejection scenarios first transfer an invalid non-terminal loan and then use a fresh valid terminal loan so both fail-closed cleanup and unchanged graph completion are observed.
  **Rejected alternative:** accept failure behavior nondeterministically in one boot or cite only an in-tree gate for externally built code.

## Open risks and follow-ups

- [ ] The SDK bundle is executable evidence, not a supported public release channel; a future developer-experience milestone may choose a durable distribution location and compatibility policy.
- [ ] CP5 proves the Arm data path on `aarch64-sel4-qemu-virt`; RP4 still requires the corresponding observed Raspberry Pi 5 run after RP3/P4 physical qualification.

## Artifacts and provenance

- Related roadmap item: [CP5](../../roadmap/10-component-platform.md)
- New gate: `scripts/check/check-component-sdk-out-of-tree.py`
- Serial evidence: the final `just component_sdk_out_of_tree_check` run and the full build/test/lint/fmt stack were observed in this work session; no raw transcript retained
- Predecessor entry: [`devlog/2026-08-21-cp4-external-artifact-admission/`](../2026-08-21-cp4-external-artifact-admission/index.md)
