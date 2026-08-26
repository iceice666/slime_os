# CP7's hosted publication: one atomic release, one credential, and the first hosted commits

| Field | Value |
|---|---|
| Date | 2026-08-26 |
| Kind | Change |
| Status | Verified |
| Scope | `scripts/build/publish-component-sdk.py`, `scripts/check/check-component-sdk-release.py`, `sdk/compatibility-matrix.{zti,json,identity}`, `roadmap/10-component-platform.md`, `iceice666/slime_os` and `iceice666/slime_os-component_sdk` repository rules, `iceice666@m3air` release credentials |
| Roadmap | CP7 |
| Gates | `just component_sdk_release_check`, `just component_sdk_prefix_check`, `just contracts_check` |
| Trigger | Preparing the first publication to the canonical hosted SDK repository exposed that the publisher pushed its branch and tag separately |
| Baseline | CP7's local bare-repository gate proved generated commits and signed tags, but the canonical repository was empty, carried no credential or ref protections, and the compatibility matrix named local stand-in commits |

## Summary

The SDK publisher now sends the generated branch commit and immutable release tag in one atomic Git push, and SDK 1.0.0 and 1.1.0 are published to the canonical repository — the clause CP7 explicitly deferred. Before the atomicity fix, a successful branch push followed by a rejected tag push left a partial release that idempotent retry could not repair: the remote tree already matched, so the publisher returned before recreating the missing tag. Publication ran from `m3air` under a repository-scoped deploy key and a signing key held outside this repository, against source commit `726ebb0` published to protected `origin/main` first, since a hosted SDK commit whose source commit is unpublished is an artifact nobody can regenerate. Both hosted releases were then re-derived from that recorded commit byte-identically, and a component built from the hosted commit entered a signed generation and booted the QEMU component graph.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `scripts/build/publish-component-sdk.py` | Replaced two ordered pushes with one `git push --atomic origin HEAD:generated sdk-v<version>` | A release is either a branch commit plus its tag or no remote change |
| `scripts/check/check-component-sdk-release.py` | Added a bare remote whose hook rejects `sdk-v*`; the gate requires the remote to retain zero refs | A tag-side rejection cannot strand a generated branch commit |
| `iceice666@m3air` | Created separate Ed25519 repository-write and tag-signing keys, an SDK-only SSH host alias, and an allowed-signers file | Repository transport and release provenance use separate least-privilege credentials outside the source repository |
| `iceice666/slime_os-component_sdk` | Registered the write deploy key and active branch/tag rulesets restricting creation, update, and deletion to deploy-key bypass | Humans and ordinary account credentials cannot directly edit the generated mirror or its release tags |
| `iceice666/slime_os` | Active `main` ruleset denying deletion and force-push, with no bypass actor | A hosted release's recorded source commit cannot be rewritten out from under it |
| `sdk/compatibility-matrix.{zti,json,identity}` | Both rows now name the hosted SDK commits `5fee7b1` and `31742d1`, each citing the component ELF and generation identity observed from that hosted commit | A published matrix row names an immutable artifact a reader can fetch |
| `roadmap/10-component-platform.md` | CP7's deferred clause recorded as closed, with the track status and CP5/CP6 boundary note corrected | Roadmap status matches observed hosted evidence |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Branch commit lands without its release tag | `just component_sdk_release_check` | The tag-rejecting remote contains any ref after the failed publication |
| Existing publication semantics regress | `just component_sdk_release_check` | Idempotence, immutable-tag, reverse-drift, external-build, or QEMU component-graph arms fail |
| Human credentials can create protected refs | Hosted rulesets plus observed negative push probes | GitHub accepts account-key creation of `generated` or `sdk-v*` |
| A hosted release's source commit is rewritten | `iceice666/slime_os` `main` ruleset | GitHub accepts a force-push or deletion of `main` |
| A matrix row names an artifact nobody can fetch | `just contracts_check` | The matrix fails to decode, or a row's identity disagrees with its normalized bytes |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Focused `prove_atomic_publication` execution | Pass: a remote rejecting only `sdk-v*` retained no branch or tag | Direct |
| `just component_sdk_release_check` | Pass: full CP7 gate, including the atomic-refusal arm, deterministic publication, reverse drift, external component build, and QEMU component graph boot | Direct |
| `just ruff` | Pass | Direct |
| GitHub ruleset inspection | Active `generated` branch ruleset and active `sdk-v*` tag ruleset; each has only `DeployKey` as an always-bypass actor | Direct |
| Ordinary account-key push to `generated` | Refused with `GH013` because creation is restricted | Direct |
| Ordinary account-key push to `sdk-v0.0.0-test` | Refused with `GH013` because creation is restricted | Direct |
| Release deploy-key `--dry-run --atomic` from `m3air` | Accepted both `HEAD:generated` and `sdk-v0.0.0-dryrun`; no refs were written | Direct |
| Ordinary account-key force-push to `slime_os` `main` | Refused: "Cannot force-push to this branch" | Direct |
| `just component_sdk_prefix_check` at source commit `726ebb0` | Pass: both profile archives extracted to their recorded identities, the QEMU ELF booted, the RPi ELF was admitted only for `bcm2712`, and five malformed archives were refused | Direct |
| Publication of SDK 1.0.0 from `m3air` | Hosted commit `5fee7b1a48f6`, tag `sdk-v1.0.0`, tree identity `d97c677d6475…` — equal to the identity the local gate computed independently | Direct |
| Publication of SDK 1.1.0 from `m3air` | Hosted commit `31742d1e6779`, tag `sdk-v1.1.0`, tree identity `c960d2a78e29…`, adding the `aarch64-rpi5` profile | Direct |
| Hosted clone: tags, signatures, and history | `sdk-v1.0.0` and `sdk-v1.1.0` both verify as good SSH signatures for `release-bot@slime-os.invalid`; each tagged commit is an ancestor of `generated`, which holds exactly two commits | Direct |
| Hosted reverse drift | The hosted `generated` tree verified against its own record and regenerated byte-identically from recorded source commit `726ebb0` | Direct |
| Hosted build and boot | A consumer resolving all five SDK crates through hosted commit `31742d1e6779`, with no path into this checkout, built a component that entered a signed generation and booted the QEMU component graph | Direct |
| Both hosted releases rebuilt for matrix evidence | 1.0.0 and 1.1.0 each built and booted from their own hosted commit; classified `initial` and `compatible-feature` | Direct |
| `just contracts_check` | Pass, including the republished matrix decoding at identity `0dc64a14b3a7…` | Direct |
| Physical Raspberry Pi 5 behavior | Not claimed. The `aarch64-rpi5` profile is host-side target qualification only | Not observed |

## Decisions

- Decision: branch and tag are pushed atomically rather than repaired after a split failure.
- Rationale: repair would need to distinguish a legitimate existing tree from a partial publication and would add recovery state to an operation Git already supports atomically.
- Rejected alternative: keep two pushes and make the idempotent path create a missing tag. That path could bless a branch commit whose tag creation was rejected for policy or signing reasons.

- Decision: use a repository-specific deploy key for transport and a separate SSH signing key for tags.
- Rationale: compromise of either credential does not grant both repository mutation and release authorship; the transport key is accepted only by `slime_os-component_sdk`.
- Rejected alternative: store a personal access token or the repository's checked-in test signing key on the release machine.

## Open risks and follow-ups

- [ ] `sdk/compatibility-matrix.*` still records both SDK releases against one product commit. A genuine cross-release row — an older SDK re-exercised against a later product commit — needs two product commits and is the first real test of the matrix's own reason for existing.
- [ ] `slime_os` `main` denies deletion and force-push but does not yet require pull requests or passing checks, so publication discipline still rests on the operator running the gates before publishing rather than on the branch refusing an ungated commit.
- [ ] The release signing key is a single Ed25519 key on one machine with no rotation or revocation path. An allowed-signers file is a deployment fact rather than a repository artifact, so a consumer verifying `sdk-v*` must obtain the public key out of band.
- [ ] The hosted assertions in this entry were observed once, by hand. No repository gate re-checks the hosted repository, so hosted drift would be caught only at the next publication or a manual re-run.

## Artifacts and provenance

- Focused report: none; this entry is the curated evidence.
- Raw transcript: not retained; the full CP7 gate and GitHub refusal messages were observed in the change session.
- Serial/debugger/model output: QEMU evidence was emitted by `just component_sdk_release_check`; no separate capture was retained.
- Related roadmap item: [CP7](../../roadmap/10-component-platform.md#cp7--permanent-sdk-repository-and-one-way-publication)
- Predecessor: [`devlog/2026-08-25-cp6-cp10-component-sdk-releases/`](../2026-08-25-cp6-cp10-component-sdk-releases/index.md)
