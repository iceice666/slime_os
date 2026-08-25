# Planning CP6–CP10: one source tree, one generated SDK, and tested release pairs

| Field | Value |
|---|---|
| Date | 2026-08-25 |
| Kind | Decision |
| Status | Proposed |
| Scope | `roadmap/10-component-platform.md`, `roadmap/README.md`, `devlog/README.md` |
| Roadmap | CP6, CP7, CP8, CP9, CP10 |
| Gates | `just devlog_check` |
| Trigger | CP5 proved out-of-tree development with a temporary pinned SDK bundle, leaving the permanent repository, platform inputs, version policy, and consumer update lifecycle unspecified |
| Baseline | External components can build and boot through CP4, but the proof constructs an ephemeral SDK and still obtains `SEL4_PREFIX` from the `slime_os` checkout |

## Summary

The component-platform track now continues through CP6–CP10 rather than treating
CP5's temporary repository as a durable release channel. The sequence keeps
`slime_os` authoritative, makes its SDK export deterministic and
self-describing, publishes that output to a generated repository, removes the
remaining checkout dependency by publishing verified platform prefixes, admits
only compatibility pairs backed by build/admission/boot evidence, and finally
proves a consumer upgrade and rollback across two immutable releases. This is a
documentation-only planning decision; no runtime behavior changed.

## Changes

| Area | Change | Established boundary |
|---|---|---|
| Track purpose and boundaries | Reopened the component-platform track after CP5 and stated that SDK sources continue to be owned in `slime_os` | The permanent SDK is a one-way generated mirror, not a second source tree or a bidirectional synchronization problem |
| CP6 | Added deterministic export and a Zutai `component-sdk-release/v1` record | Candidate and release builds consume one exporter; test-local copy-and-patch logic cannot define an alternate SDK |
| CP7 | Added permanent repository publication, immutable tags, protected credentials, idempotence, and reverse drift verification | A hosted commit is accepted only when it regenerates byte-for-byte from the source commit it records |
| CP8 | Added content-addressed QEMU and RPi seL4 prefix assets and a verified build entry point | An external build no longer borrows `slime_os/build/sel4-prefix*`; profile qualification remains exact and RPi build evidence is not mislabeled as physical boot evidence |
| CP9 | Separated Cargo source compatibility from syscall, protocol, component-image, target, toolchain, and prefix compatibility | A release pair is supported only after direct build, admission, and boot evidence; SemVer does not manufacture runtime compatibility |
| CP10 | Added a full-commit consumer pin, coupled update, generation rebuild, health confirmation, and rollback proof | Failed upgrades preserve the previous immutable SDK inputs, ELF, and bootable generation rather than leaving a half-updated checkout |
| Roadmap index | Added CP6–CP10 to the current-state row and dependency graph | The repository index names CP6 as the next component-platform gate and exposes the complete follow-on sequence |

## Decisions

- Decision: keep `boot-contracts`, the public component crates, public contracts,
  targets, and pin declarations authoritative in `slime_os`; generate the SDK
  repository in one direction from an allowlisted export.
- Rationale: root/runtime/protocol changes frequently need one clean cutover.
  Making the SDK repository canonical or synchronizing both ways would turn one
  coherent source change into a cross-repository ordering problem and make
  bisects depend on unpublished companion commits.
- Rejected alternative: move the public crates to the SDK repository and make
  `slime_os` consume them as git dependencies, or maintain the same crates in
  both repositories with subtree/cherry-pick synchronization.

- Decision: make the release record a versioned Zutai contract and include the
  identities that affect a component after Rust compilation, not only crate
  versions.
- Rationale: SDK metadata crosses a persistent repository and release boundary,
  and a component can compile while disagreeing with the root about syscall,
  protocol, image, target, or platform inputs. Those identities need one closed,
  generated format and explicit comparison.
- Rejected alternative: a hand-written JSON/TOML manifest plus SemVer ranges.
  That would introduce a second schema language and treat source compatibility
  as proof of runtime compatibility.

- Decision: separate deterministic export, repository publication, platform
  assets, compatibility, and consumer lifecycle into CP6–CP10.
- Rationale: each slice has a distinct failure boundary and direct exit
  condition. CP7 cannot safely publish what CP6 cannot reproduce; CP8 closes the
  remaining `SEL4_PREFIX` dependency; CP9 needs immutable releases and assets;
  CP10 needs two classified releases before an upgrade and rollback can be
  observed.
- Rejected alternative: one "publish the SDK" milestone. Its gate would mix
  source selection, credentials, artifact production, compatibility policy, and
  rollback, so a failure would not identify which contract was broken.

- Decision: initial compatibility support is exact-pair and evidence-backed.
- Rationale: the current repository has no general rule proving an ELF built
  against one SDK release interoperates with a different product release.
  Absence from the compatibility matrix therefore means unsupported until the
  declared build, admission, and QEMU boot path observes the pair.
- Rejected alternative: infer compatibility from equal package versions,
  unchanged Rust signatures, or a broad SemVer range.

## Open risks and follow-ups

- [ ] The canonical repository now exists at
      [`iceice666/slime_os-component_sdk`](https://github.com/iceice666/slime_os-component_sdk),
      but CP7 still must establish its release identity, generated default branch,
      branch protection, immutable-tag policy, and credential boundary.
- [ ] CP8's prefix archives are reproducible only to the degree the existing
      seL4 pin and observed-prefix checks cover host tools. The milestone must
      preserve those checks rather than claim a stronger toolchain closure.
- [ ] CP9 needs two real immutable releases. Synthetic mutations are valid
      negative controls but cannot by themselves satisfy the two-release exit
      condition.
- [ ] CP10's operator-owned component-spec update remains outside the external
      component source repository unless a later component registry or
      composition-authoring workflow gives it a different canonical home.
- [ ] The named CP6–CP10 `just` targets do not exist until their milestones land;
      this is deliberate roadmap naming, matching the repository's existing
      planned-gate convention.

## Artifacts and provenance

- Focused report: none; the complete decomposition and acceptance criteria are
  in [`roadmap/10-component-platform.md`](../../roadmap/10-component-platform.md).
- Raw transcript: not retained.
- Validation target: `just devlog_check`; documentation only, so no runtime tests
  apply.
- Related roadmap items: [CP6–CP10](../../roadmap/10-component-platform.md)
- Predecessor: [`devlog/2026-08-22-cp5-out-of-tree-component-sdk/`](../2026-08-22-cp5-out-of-tree-component-sdk/index.md)
