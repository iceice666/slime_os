# CP4: content-bound external artifacts enter an ordinary signed generation

| Field | Value |
|---|---|
| Date | 2026-08-21 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/component-spec/v1/`, `scripts/lib/component_spec.py`, `scripts/build/{build-generation.py,build-sel4.py}`, `scripts/check/{check-component-spec.py,check-generation.py,check-external-component-admission.py}`, `boot-contracts/src/component_image.rs`, `Justfile` |
| Roadmap | CP4 |
| Gates | `just external_component_admission_check`, `just test_host`, `just lint_all`, `just fmt_check_all`, `just ruff` |
| Trigger | CP4 required the generation builder to accept an explicitly declared, content-hash-bound ELF produced outside this workspace without adding a second trust root |
| Baseline | Every component in a seL4 generation was built through this repository's Cargo workspace; the component spec named a provider and binary but could not bind an external declaration to bytes |

## Summary

The component specification now distinguishes workspace and external implementations and binds an external implementation to the lowercase SHA-256 of its bare ELF bytes. The generation builder consumes an explicit implementation-name-to-path mapping, checks the digest and the root loader's bounded ELF invariants before signing, wraps the same admitted bytes as a normal component image, and reports the selected source. The focused gate independently builds `console` from a temporary crate outside this Cargo workspace, mixes it with workspace-built components, signs and host-admits the generation, embeds it in the seL4 component-graph image, and observes the existing graph boot gate pass. The root admission and loading modules remain unchanged; trust remains the existing whole-generation release signature.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Component-spec contract and corpus | `Implementation` gains `contentHash`; every workspace/undeclared record carries an empty value, while external declarations require exactly one lowercase SHA-256 and a unique implementation name | The spec selects the producer explicitly and an external declaration identifies immutable content rather than an operator-local path |
| `scripts/lib/component_spec.py` and its gate | Provider-specific validation rejects workspace hashes, missing/invalid external hashes, undeclared artifacts, duplicate implementation binaries, and cross-domain name collisions | One admitted component spec resolves to one unambiguous implementation artifact |
| `scripts/build/build-generation.py` | Adds `--component-spec-root` and repeatable `--external-component NAME=ELF`; builds only workspace implementations, requires every declared external mapping and no unused mapping, verifies the ELF digest, applies bounded host/root-equivalent ELF admission, and reports each component source | No external bytes reach generation signing unless their declaration, digest, target, load shape, footprint, and W^X properties agree |
| `scripts/build/build-sel4.py` | Forwards external/spec inputs and can embed one already built generation into the selected seL4 image while recording its identity and digest; incompatible selector combinations fail closed | The exact generation admitted by the host check is the generation exercised by the QEMU gate |
| `boot-contracts` and host generation checker | Both require the root loader's exact 56-byte ELF64 program-header entry and enforce the same bounded ELF shape | A host-admitted generation cannot later fail solely because the root parser applies a stricter program-header layout rule |
| `scripts/check/check-external-component-admission.py` | Builds an isolated external crate, proves mixed-source signing/admission/boot, checks source reporting, and mutates hash, header bounds, entry, W^X, mapped size, and program-header stride | CP4's positive and pre-signing refusal paths are executable evidence rather than an operator procedure |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| An external mapping is inferred, missing, duplicated, or unused | `just external_component_admission_check` plus component-spec mutation arms | Builder refuses with the named missing/duplicate/not-declared mapping error |
| External bytes disagree with the declared content | `just external_component_admission_check` | `does not match declared`; no `generation.bin` or `boot-store.bin` exists |
| A malformed or wrong-shape ELF is signed | `just external_component_admission_check` | The matching structural marker appears before any signed artifact exists |
| Host checks accept an ELF shape the root parser refuses | Rust regression plus the external `e_phentsize = 64` mutation | `invalid program header table` / `BadElfShape` |
| The QEMU check boots a generation other than the checked mixed generation | Prebuilt-generation identity recorded in `slime-sel4-graph.identity.json` and compared by the focused gate | Embedded identity or digest does not match the checked generation |
| Root code gains producer-specific behavior | CP4 diff and focused boot gate | Any implementation change under `slime-root/src/{generation,child_vspace}.rs` would violate the milestone boundary |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just external_component_admission_check` | pass; one independently built external `console` ELF was mixed with workspace components, signed, admitted, embedded, and passed `sel4_component_graph_check` | Direct |
| External content-hash mismatch | refused before signing; no generation or boot-store artifact remained | Direct |
| Five structural mutations: truncated program-header table, non-executable entry, writable executable page, oversized mapped segment, 64-byte program-header entry | all refused before signing with the expected admission marker | Direct |
| `just generation_check` (dependency of the focused gate) | pass; two isolated all-workspace builds produced byte-identical generation and boot-store bytes and passed admission | Direct |
| `just test_host` | pass; `boot-contracts` 209 tests, protocol integration tests, and 4 build-support tests | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff` | pass | Direct |
| Fresh reviewer pass | one P1 root-equivalence gap found, fixed, and re-reviewed; final verdict correct with no remaining findings | Direct |

## Decisions

- **Decision:** bind external implementations by bare-ELF SHA-256 in the component spec, but keep the path only as a command-line build input.
  **Rationale:** the persisted declaration must identify content independently of the operator's filesystem, while the path is local build configuration and must not become a new contract.
  **Rejected alternative:** infer external status from whether Cargo produced a file, which makes producer selection dependent on ambient workspace state.

- **Decision:** retain the whole-generation release signature as the only trust boundary.
  **Rationale:** after digest and structural admission, external bytes enter the same component-image and generation signing path as workspace bytes; a per-component signature would add a trust root and provenance protocol CP4 explicitly excludes.
  **Rejected alternative:** add component signatures or a provenance record to the generation format.

- **Decision:** validate the ELF before signing and require the root loader's exact program-header layout.
  **Rationale:** the host builder is the last point that can reject bad external bytes without producing an apparently valid release. Accepting a broader ELF subset than `object` 0.38.1 would let a signed generation fail only at component launch.
  **Rejected alternative:** rely on root launch-time rejection after the generation has already been signed and selected.

- **Decision:** the focused gate builds a temporary crate outside the workspace but does not claim CP5's out-of-tree development proof.
  **Rationale:** CP4 must prove artifact admission, while CP5 separately requires a distinct git checkout, the published/vendored SDK boundary, and the two RP4 data-path components.
  **Rejected alternative:** collapse CP4 and CP5 by treating one temporary copied crate as the complete external development workflow.

## Open risks and follow-ups

- [ ] CP5 must prove the authoring and SDK boundary from a genuinely separate git checkout and admit both RP4 data-path components through this path.
- [ ] Component-level provenance and signatures remain intentionally absent; introduce them only through a separately scoped trust-contract change.

## Artifacts and provenance

- Related roadmap item: [CP4](../../roadmap/10-component-platform.md)
- New gate: `scripts/check/check-external-component-admission.py`
- Serial evidence: the final `just external_component_admission_check` output and reviewer result were observed in this work session; no raw transcript retained
- Predecessor entry: [`devlog/2026-08-21-cp3-crate-per-component/`](../2026-08-21-cp3-crate-per-component/index.md)
