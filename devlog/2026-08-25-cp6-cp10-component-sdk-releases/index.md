# CP6–CP10: one exporter, one published mirror, and a consumer that can roll back

| Field | Value |
|---|---|
| Date | 2026-08-25 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/component-sdk-release/v1/`, `scripts/lib/{component_sdk,component_sdk_release_contract}.py`, `scripts/generate/generate-component-sdk-release-bindings.py`, `scripts/build/{build-component-sdk,publish-component-sdk}.py`, `scripts/check/{check-component-sdk-export,check-component-sdk-release,check-component-sdk-prefix,check-component-sdk-compatibility,check-component-sdk-upgrade,check-component-sdk-out-of-tree,check-contracts}.py`, `components/build-support/src/lib.rs`, `sdk/compatibility-matrix.*`, `Justfile`, `roadmap/{10-component-platform,README}.md` |
| Roadmap | CP6, CP7, CP8, CP9, CP10, CP5 |
| Gates | `just component_sdk_export_check`, `just component_sdk_release_check`, `just component_sdk_prefix_check`, `just component_sdk_compatibility_check`, `just component_sdk_upgrade_check`, `just contracts_check`, `just component_crate_split_check`, `just sel4_gate_control_check`, `just lint_all`, `just fmt_check_all`, `just machete`, `just test_host`, `just ruff` |
| Trigger | CP5 closed with a temporary pinned SDK bundle built inside its own gate, leaving the permanent repository, the platform build inputs, the version policy, and the consumer update lifecycle unspecified |
| Baseline | An external component could be built and booted through CP4, but the SDK it built against was constructed by test-local Python, described by nothing, and took `SEL4_PREFIX` from `slime_os/build/` |

## Summary

The component SDK is now a described artifact rather than a shape one gate
happened to build. `contracts/component-sdk-release/v1` declares what an export
is; `scripts/lib/component_sdk.py` is the only thing that produces one; the
publisher exports a detached checkout of the commit it records, so a published
mirror commit reproduces byte-for-byte from the source it names; each release
carries its own content-addressed seL4 prefix per target profile, so an external
build reads nothing below a `slime_os` checkout; two real releases are classified
against each other and every published compatibility row is backed by a build
plus the boot that observed it; and a template consumer moves between the two
releases, boots the content-bound generation, survives five injected failures
with its prior pin intact, and reproduces the previous ELF and generation
byte-for-byte on rollback.

Three things only appeared once the second target profile was real. The RPi
profile builds components against the `aarch64-unknown-none` triple with its own
link flags rather than the seL4 JSON target, and it links against a
repository-level linker script an out-of-tree crate cannot find; both are now
release-record data and an exported build input. And three negative controls
proved nothing as first written — two zeroed tar padding and changed no byte, one
watched an identity a pin change does not move — which is recorded below because
the gates were green while the properties were unproven.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/component-sdk-release/v1/schema.zt` | New Zutai contract: the originating commit, the exported-tree identity, every exported crate and public contract identity, the pinned toolchain and sources, the per-profile platform build inputs, the compatibility identities, and the tested-pair matrix | SDK release metadata crosses a persistence and repository boundary, so it is a versioned schema rather than a hand-written manifest |
| same | `treeIdentity` excludes exactly the three record files the contract names | A digest cannot cover bytes that include itself, and a producer-owned exclusion set is one a consumer cannot reproduce |
| same | Scalar breaking axes are separate from structural ones (`crates`, `profiles`), each keyed | Adding a target profile is a compatible feature and changing one is breaking; a single set digest could only say "different" and would force a major release for every added platform |
| `scripts/lib/component_sdk.py` | The whole exporter: the allowlist, the canonical digests, the generated workspace, the release record, the classification policy, and the two consumer entry points it emits | One exporter means a candidate export and a published release cannot be different artifacts that both pass |
| same | Exported crate manifests are copied byte-for-byte; the generated SDK workspace supplies the inherited lint and release-profile tables | CP5 deleted `publish = false` and `[lints] workspace = true` out of copied files, which made the export a rewrite rather than a copy |
| same | Every digest is domain-separated over an explicit length-prefixed encoding; archives are uncompressed `tar` with fixed member metadata | Hashing `tar` bytes whose metadata a host controls is not a content address, and a compressor is a second implementation in the reproducibility surface |
| same | The two libsel4 headers that record their `.bf` input by absolute path are canonicalized to the `/slime/sel4` prefix the kernel build already maps to; any other host path is a refusal | A prefix carrying this checkout's path is not checkout-independent, and a silent fixup is how one reaches a consumer |
| `scripts/build/build-component-sdk.py` | The exporter's command line | An export is an operation the repository owns, not a side effect of running a gate |
| `scripts/build/publish-component-sdk.py` | Publication exports a detached worktree of the recorded commit, writes at most one generated commit and one immutable signed `sdk-v<version>` tag, and refuses a dirty exported set, an absent commit, a reused version, a changed tree reusing a version, and a non-allowlisted file | A recorded commit that was not what got exported is a label; exporting the worktree would make CP7's drift check a tautology |
| same | `--sdk-repository` is separate from `--sdk-url` | The recorded canonical repository is release identity; the URL is only the transport a mirror or a local clone may differ on |
| `components/build-support/src/lib.rs` | `SLIME_COMPONENT_LINKER_DIR` overrides where the component linker scripts are found | The bare-metal triples link at the fixed component base their target profile declares, and an out-of-tree crate cannot resolve a repository-level script relative to its own manifest |
| `scripts/check/check-component-sdk-out-of-tree.py` | CP5 now consumes the exporter and its own bundle recipe is deleted | CP6's exit condition is precisely that no test-local alternate SDK survives |
| `sdk/compatibility-matrix.{zti,json,identity}` | The published matrix: two rows, each naming immutable commits and citing the build and boot that back it | The matrix is derived from tested pairs; absence is unsupported rather than implicit compatibility |
| `scripts/check/check-contracts.py` | The new contract, its two check entrypoints, its generator, and the published matrix are registered | A contract not in this registry is unguarded, whatever else validates it |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The export stops being deterministic | `just component_sdk_export_check` | Two exports of one source tree are diffed and both identities compared; any difference fails |
| The identity becomes a constant, or a digest of the whole repository | `just component_sdk_export_check` | Four probes: an exported source and a pin must move the release identity, two product-only files must not |
| The record drifts from the bytes it describes | `just component_sdk_export_check`, `just component_sdk_release_check` | Every crate, contract, archive, and target digest is recomputed; an identity file disagreeing with its record is refused |
| A published mirror is hand-edited | `just component_sdk_release_check` | The published tree is regenerated from its recorded commit and diffed; a README edit must be refused |
| Publication becomes non-idempotent, or an immutable tag moves | `just component_sdk_release_check` | Republishing an unchanged tree must write nothing and leave one commit; an existing tag is refused |
| An external build silently borrows `slime_os/build/sel4-prefix*` | `just component_sdk_prefix_check` | `SEL4_PREFIX` is poisoned and `SLIME_TARGET_PROFILE` set wrong before the build; the exported prefix must come from the record |
| A prefix archive is accepted corrupt, truncated, swapped, or mismatched | `just component_sdk_prefix_check` | Four mutations, each asserted to differ from the original first |
| The two profiles become interchangeable | `just component_sdk_prefix_check` | A QEMU-target ELF is refused by the RPi build, and the wrapper's declared profile id is read out of each generation |
| A release understates a changed identity | `just component_sdk_compatibility_check` | Five scalar and two structural axes are moved in isolation; each must force its classification, including equal crate versions across a changed syscall ABI |
| An untested pairing is presented as supported | `just component_sdk_compatibility_check` | Three untested pairings must report unsupported |
| A failed upgrade leaves a half-updated consumer | `just component_sdk_upgrade_check` | Five injected failures, after each of which the manifest, lockfile, recorded release, and built ELF must be unchanged |
| Rollback produces "a working older build" rather than the previous one | `just component_sdk_upgrade_check` | The rolled-back ELF and generation must be byte-identical to the retained ones and the generation identity must match |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just component_sdk_export_check` | Pass: two byte-identical exports, four sensitivity probes, a decoding record over 5 crate and 39 contract identities, five SDK crates resolved through a pinned commit, an SDK-built external component booted on the QEMU component graph, three malformed export requests refused | Direct |
| `just component_sdk_release_check` | Pass: SDK 1.0.0 published as one commit and one signed tag naming source `a8c73ff`, republication wrote nothing, four malformed publications refused, byte-identical regeneration from the recorded commit, a hand-edited mirror refused, a fresh clone's component booted, the in-tree graph still booted with every clone deleted | Direct |
| `just component_sdk_prefix_check` | Pass: both archives extracted to their recorded tree identities with no host path, the QEMU ELF booted, the RPi ELF was admitted only for `bcm2712` (profile id 3) while the QEMU-target ELF was refused by the RPi build, four malformed archives refused | Direct |
| `just component_sdk_compatibility_check` | Pass: 1.0.0 and 1.1.0 published and classified initial/compatible-feature, 5 scalar and 2 structural controls forced their classification, both matrix rows backed by build plus boot, three untested pairings unsupported | Direct |
| `just component_sdk_upgrade_check` | Pass: template consumer pinned by full commit, built and booted, five injected failures left the prior pin usable, upgrade rebuilt and booted a new generation, rollback reproduced the previous ELF and generation byte-for-byte, in-tree fallback booted | Direct |
| `just contracts_check` | Pass, including the new contract, both check entrypoints, its generator's `--check`, and the published matrix decoding as `#valid` | Direct |
| `just component_crate_split_check` | Pass: 58 component crates, allocator groups unchanged | Direct |
| `just sel4_gate_control_check` | Pass: 39 gates reject 1518 mutated transcripts and layouts | Direct |
| `just lint_all`, `just fmt_check_all`, `just machete`, `just test_host`, `just ruff`, `just typos` | Pass | Direct |
| RPi physical-board behavior | Not claimed. The RPi arm is host-side target qualification only | Not observed |
| The SDK repository's branch protection and credential boundary | Not gate-provable: a GitHub setting rather than a repository artifact | Not observed |

## Decisions

- Decision: the publisher exports a detached checkout of the commit it records,
  never the working tree.
- Rationale: CP7's reverse-drift check compares a published tree against a fresh
  export of its own recorded `sourceCommit`. If publication exported the
  worktree, the recorded commit would be a label and the comparison could pass
  for a tree that commit does not produce.
- Rejected alternative: export the worktree and record `HEAD`, refusing only a
  dirty tree. A clean tree is not the same as the commit — submodule state and
  untracked-but-read files both diverge.

- Decision: structural compatibility axes are compared as keyed sets, not as
  digests.
- Rationale: adding a target profile or an exported crate is a compatible
  feature, and changing or removing one is breaking. A digest over either set can
  only report "different", which would make every added platform a major release
  and so make the classification useless exactly where CP8 grows the platform
  set.
- Rejected alternative: one `profileSet` digest as a breaking axis, which is what
  the first revision of the contract declared.

- Decision: the release record carries each profile's Cargo target, whether it is
  a JSON specification, and its exact `RUSTFLAGS` and Cargo flags.
- Rationale: the two profiles genuinely differ. The seL4 JSON target inherits no
  config rustflags and needs `-Z build-std`; the `aarch64-unknown-none` triple
  has a prebuilt `core` and must link at the profile's fixed component base. One
  hard-coded flag set produced an ELF the generation builder refused with
  "invalid component load layout".
- Rejected alternative: hard-code the seL4 flags in the build entry point and
  treat the triple as an unsupported profile, which would have made CP8's
  two-profile deliverable unmeetable.

- Decision: the exporter canonicalizes exactly two generated libsel4 headers and
  refuses every other host path.
- Rationale: those two are emitted by seL4's own Python generators and record
  their `.bf` input by absolute path, which the kernel build's
  `-ffile-prefix-map` does not reach. Rewriting them to the same `/slime/sel4`
  logical prefix uses the convention already in place. Any other host path is a
  real leak, and rewriting it silently is how a checkout path reaches a consumer.
- Rejected alternative: a generic host-path rewrite over the whole export, or
  hashing the prefix as-is and documenting the leak.

- Decision: the release identity, not `treeIdentity`, is what a sensitivity
  control watches.
- Rationale: a pin change moves what the record declares — the toolchain a
  consumer must use, the axis CP9 classifies on — without moving one exported
  byte. A control watching only `treeIdentity` passed a release whose declared
  toolchain had silently changed.
- Rejected alternative: adding the pins to the exported tree so `treeIdentity`
  would move. That would export `sel4/pins.toml`, which is product data an SDK
  consumer has no use for.

## Open risks and follow-ups

- [ ] The canonical repository `iceice666/slime_os-component_sdk` has no
      published commit yet. Every gate publishes to a local bare clone of that
      repository's `generated` branch through the real publisher, so what is
      proven is the publisher's behavior; the first hosted commit, the branch
      protection, and the credential boundary remain to be configured.
- [ ] Release tags are signed with `contracts/release/v1/test-keys/key1`, the
      repository's existing test trust root. A real publication needs a release
      identity whose key is not in the repository, and verification needs an
      allowed-signers file, which is a deployment fact rather than a repository
      artifact.
- [ ] `sdk/compatibility-matrix.*` records two SDK releases against one product
      commit. A genuine cross-release row — an older SDK re-exercised against a
      later product commit — needs two product commits and is the first real test
      of the matrix's own reason for existing.
- [ ] The exported prefixes are reproducible to the degree `sel4/pins.toml`
      covers host tools, which its own comments say excludes `cmake`, `ninja`,
      and the kernel's Python generators. CP8 preserves that boundary rather than
      claiming a stronger closure.
- [ ] CP10's operator-owned component-spec update still happens outside the
      consumer repository: `tools/sdk-update.py` reports the new content hash and
      an operator writes it into a `contracts/component-spec/v1` record here.

## Artifacts and provenance

- Focused report: none; the acceptance criteria are in
  [`roadmap/10-component-platform.md`](../../roadmap/10-component-platform.md).
- Raw transcript: not retained; every gate prints its own evidence lines.
- Published artifact: [`sdk/compatibility-matrix.zti`](../../sdk/compatibility-matrix.zti)
- Related roadmap items: [CP6–CP10](../../roadmap/10-component-platform.md)
- Predecessor: [`devlog/2026-08-25-cp6-cp10-sdk-release-plan/`](../2026-08-25-cp6-cp10-sdk-release-plan/index.md)
- CP5's own entry: [`devlog/2026-08-22-cp5-out-of-tree-component-sdk/`](../2026-08-22-cp5-out-of-tree-component-sdk/index.md)
