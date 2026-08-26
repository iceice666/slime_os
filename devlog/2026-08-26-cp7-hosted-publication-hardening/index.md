# CP7 hosted publication hardening: one atomic release and one credential

| Field | Value |
|---|---|
| Date | 2026-08-26 |
| Kind | Change |
| Status | Verified |
| Scope | `scripts/build/publish-component-sdk.py`, `scripts/check/check-component-sdk-release.py`, `iceice666/slime_os-component_sdk` repository rules, `iceice666@m3air` release credentials |
| Roadmap | CP7 |
| Gates | `just component_sdk_release_check` |
| Trigger | Preparing the first publication to the canonical hosted SDK repository exposed that the publisher pushed its branch and tag separately |
| Baseline | CP7's local bare-repository gate proved generated commits and signed tags, but the canonical repository was empty and carried no credential or ref protections |

## Summary

The SDK publisher now sends the generated branch commit and immutable release tag in one atomic Git push. Before this change, a successful branch push followed by a rejected tag push left a partial release that idempotent retry could not repair: the remote tree already matched, so the publisher returned before recreating the missing tag. A pre-receive-hook regression now rejects only the tag and proves that neither ref lands. The canonical repository now has active `generated` branch and `sdk-v*` tag rulesets whose only bypass actor is its write deploy key, held on `iceice666@m3air`; ordinary account credentials were observed failing both creation paths, while that deploy key's atomic dry run was accepted. No hosted SDK commit or tag was published by this change.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `scripts/build/publish-component-sdk.py` | Replaced two ordered pushes with one `git push --atomic origin HEAD:generated sdk-v<version>` | A release is either a branch commit plus its tag or no remote change |
| `scripts/check/check-component-sdk-release.py` | Added a bare remote whose hook rejects `sdk-v*`; the gate requires the remote to retain zero refs | A tag-side rejection cannot strand a generated branch commit |
| `iceice666@m3air` | Created separate Ed25519 repository-write and tag-signing keys, an SDK-only SSH host alias, and an allowed-signers file | Repository transport and release provenance use separate least-privilege credentials outside the source repository |
| `iceice666/slime_os-component_sdk` | Registered the write deploy key and active branch/tag rulesets restricting creation, update, and deletion to deploy-key bypass | Humans and ordinary account credentials cannot directly edit the generated mirror or its release tags |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Branch commit lands without its release tag | `just component_sdk_release_check` | The tag-rejecting remote contains any ref after the failed publication |
| Existing publication semantics regress | `just component_sdk_release_check` | Idempotence, immutable-tag, reverse-drift, external-build, or QEMU component-graph arms fail |
| Human credentials can create protected refs | Hosted rulesets plus observed negative push probes | GitHub accepts account-key creation of `generated` or `sdk-v*` |
| Release credential cannot publish both refs | Deploy-key atomic dry run from `m3air` | GitHub rejects either dry-run ref update |

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
| SDK signing-key local tag verification on `m3air` | Good SSH signature for `release-bot@slime-os.invalid` | Direct |
| Canonical SDK repository ref listing after all probes | Empty; no test or release ref exists | Direct |

## Decisions

- Decision: branch and tag are pushed atomically rather than repaired after a split failure.
- Rationale: repair would need to distinguish a legitimate existing tree from a partial publication and would add recovery state to an operation Git already supports atomically.
- Rejected alternative: keep two pushes and make the idempotent path create a missing tag. That path could bless a branch commit whose tag creation was rejected for policy or signing reasons.

- Decision: use a repository-specific deploy key for transport and a separate SSH signing key for tags.
- Rationale: compromise of either credential does not grant both repository mutation and release authorship; the transport key is accepted only by `slime_os-component_sdk`.
- Rejected alternative: store a personal access token or the repository's checked-in test signing key on the release machine.

## Open risks and follow-ups

- [ ] The canonical repository is still empty. The first hosted SDK release must name a `slime_os` commit already reachable from protected `origin/main`, run from the `m3air` release machine, and then pass a hosted clone/build/boot verification before CP7's deferred hosted-publication clause closes.
- [ ] The compatibility matrix still names local stand-in SDK commits. Replace them only after the corresponding hosted releases have been built and booted.
- [ ] `slime_os` source `main` still needs its own protection policy before a hosted release may record one of its commits.

## Artifacts and provenance

- Focused report: none; this entry is the curated evidence.
- Raw transcript: not retained; the full CP7 gate and GitHub refusal messages were observed in the change session.
- Serial/debugger/model output: QEMU evidence was emitted by `just component_sdk_release_check`; no separate capture was retained.
- Related roadmap item: [CP7](../../roadmap/10-component-platform.md#cp7--permanent-sdk-repository-and-one-way-publication)
- Predecessor: [`devlog/2026-08-25-cp6-cp10-component-sdk-releases/`](../2026-08-25-cp6-cp10-component-sdk-releases/index.md)
