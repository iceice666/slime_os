# The SDK publish workflow trusted its operator for three separate things

| Field | Value |
|---|---|
| Date | 2026-09-01 |
| Kind | Change |
| Status | Verified |
| Scope | `.github/workflows/publish-sdk.yml`, `scripts/check/check-component-sdk-preflight.py`, `just/component-sdk.just` |
| Roadmap | CP7, CP9 |
| Gates | `just component_sdk_preflight`, `just component_sdk_export_check`, `just component_sdk_release_check` |
| Trigger | Reviewing whether SDK publication was automated found the workflow already existed, and that three of its preconditions were operator discipline rather than mechanism |
| Baseline | `publish-sdk.yml` published atomically from `m3air` behind a required reviewer and verified the hosted tag, but accepted any dispatch ref, ran no SDK gate, and took `version` as unchecked free text |

## Summary

Publication was already a workflow; what it lacked were the three checks its
own milestone text claims. CP7's deliverable says "publish only from an exact
**protected** `slime_os` commit **after CP6 and the existing external-component
gates pass**", and neither clause was implemented: `workflow_dispatch` accepted
any branch while recording `github.sha` as the release's `sourceCommit`, and no
`component_sdk_*` gate ran anywhere in the publish path or in `ci.yml`.
Separately, CP9 makes the change classification computable, but nothing computed
it before publication, so an understated `version` was caught by
`admit_version_change` only after the prefix build, inside the one job holding
the release credentials. This adds a `guard` job that requires the dispatch ref
*and* commit containment in `origin/main`, runs the export and release gates
before any credential is reachable, and adds a credential-free preflight that
reads the hosted release and reports the version publication will admit.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `.github/workflows/publish-sdk.yml` | New `guard` job: refuses a dispatch ref other than `refs/heads/main`, then refuses a `github.sha` that `git merge-base --is-ancestor` does not place on `origin/main`; `fetch-depth: 0` so ancestry is decidable | A recorded `sourceCommit` is reachable from `main`, so the release regenerates from the commit it names |
| same | `publish` names `needs: [guard, build-prefixes]` and re-asserts `if: github.ref == 'refs/heads/main'` on the job holding the keys | A later edit that reorders or drops a dependency cannot silently reopen publication from an arbitrary branch |
| same | `component_sdk_export_check` and `component_sdk_release_check` run in `build-prefixes`, before the credentialed job; timeout 30 → 90 | CP7's "after the gates pass" clause is mechanism rather than operator memory |
| same | The requested `version` is checked against the computed class on the ordinary runner | A wrong version costs seconds instead of a prefix build, and is refused before a credential is reached |
| `scripts/check/check-component-sdk-preflight.py` | New read-only checker: clones the hosted release, exports the current source commit, prints the classification, the moved axes, and the lowest admissible version; `--require-version` makes it a gate | The version a publication must claim is derived from the release record rather than chosen |
| same | The profile axis is compared over the intersection of hosted and exported profiles, and uncompared profiles are printed as `NOT COMPARED` | Exporting a subset of prefixes cannot masquerade as a release removing a platform, and a partial comparison says so |
| `just/component-sdk.just` | `component_sdk_preflight *ARGS`, depending on no gate | The preflight needs only an installed prefix, so it runs before the release path rather than behind it |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Publication from a feature branch records an unreproducible `sourceCommit` | `guard` job in `just`-less workflow YAML | The job refuses, naming either the ref or the uncontained commit |
| A ref-name check is mistaken for containment | Same job checks both, independently | A `main` ref carrying an off-`main` commit is refused naming the commit |
| An understated version reaches the release machine | `just component_sdk_preflight --require-version` | Non-zero exit naming the required version and the changed axes |
| Export determinism or the release lifecycle regresses unnoticed | `just component_sdk_export_check`, `just component_sdk_release_check` in the publish path | Either gate fails before the credentialed job starts |
| Hosted drift goes unobserved between publications | `just component_sdk_preflight` reads the hosted record and self-checks its three files | The hosted record is refused as inadmissible |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just component_sdk_preflight --profile aarch64-sel4-qemu-virt` from `origin/main` | Pass: hosted `2.0.0` (source `e6e1a91`) against source `ce256d2` classified `breaking`, required version `3.0.0`, naming `syscallAbi`, `contractSet`, `crates:changed:boot-contracts,slime-components,slime-proto,slime-rt`, and `profiles:changed:aarch64-sel4-qemu-virt` | Direct |
| `--require-version 3.0.0` / `2.1.0` / `2.0.1` | Exit 0 / 1 / 1; both refusals name the required `3.0.0` and the `breaking` class | Direct |
| Same through `just component_sdk_preflight` | Pass and exit 1 respectively, so the refusal propagates through the target | Direct |
| `guard` shell logic extracted and run against real refs | Three arms: `main` ref + `main` commit passes; a feature ref is refused naming the ref; a `main` ref carrying `feat/duo-bringup`'s `7f962ea` is refused naming the commit | Direct |
| Workflow YAML parsed; job graph read back | `guard` → `build-prefixes` → `publish`, with `needs: ['guard', 'build-prefixes']` and the `if` present on `publish` | Direct |
| `just ruff`, `just typos` | Pass | Direct |
| `just --list` | Parses; 222 targets; the preflight renders its intended one-line description | Direct |
| `component_sdk_export_check`, `component_sdk_release_check` | Not run locally in this session: both boot QEMU and publish to a stand-in repository. Wired into the workflow but unobserved here | Inherited |

## Decisions

- **Decision:** add a top-level checker rather than extend an existing `component_sdk_*` one.
  **Rationale:** `AGENTS.md` restricts new `scripts/check/check-*.py` to a genuinely new mechanism or boundary. Reading the *hosted* repository is exactly that: `devlog/2026-08-26-cp7-hosted-publication-hardening/index.md` records that no repository gate reads it, so the existing checkers all operate on locally exported or stand-in trees and none has a hosted-comparison mechanism to extend.
  **Rejected alternative:** fold the comparison into `check-component-sdk-release.py`, which would make a credential-free preflight depend on a gate that boots QEMU.

- **Decision:** check both the dispatch ref and commit containment, not either alone.
  **Rationale:** they fail differently. A ref check gives the operator a clear message; containment is the property CP7's reverse-drift check actually needs, and a `main` ref can carry a commit that is not on `main`. Observed directly: the third guard arm refuses exactly that case.
  **Rejected alternative:** rely on `main`'s branch protection, which denies force-push and deletion but does not make an arbitrary dispatch ref unpublishable.

- **Decision:** refuse a version that overstates the change class, even though `admit_version_change` admits one.
  **Rationale:** the workflow input is a typed string; a mismatch against the computed value is more likely a typo than a deliberate conservative bump. An operator who wants an overstated version can still call the publisher directly.
  **Rejected alternative:** warn and continue, which would make the preflight advisory and leave the late refusal in place.

- **Decision:** compare the profile axis over the intersection and print what was not compared.
  **Rationale:** which prefixes exist locally is an invocation property, so treating an absent profile as `profiles:removed` would manufacture a breaking change out of how the gate was called. Silently ignoring it would instead claim a completeness the run does not have.
  **Rejected alternative:** require every hosted profile to be exported, which makes the preflight unusable on a machine that has built one prefix.

## Open risks and follow-ups

- [ ] `sdk/compatibility-matrix.*` still holds only the `1.0.0` and `1.1.0` rows against product `726ebb0`. Hosted `sdk-v2.0.0` exists with no row, so by CP9's own rule that pairing is unsupported rather than implicitly compatible. The publish path still does not emit a row; closing this is the remaining half of gap 5 and subsumes CP7's follow-up that every row names one product commit.
- [ ] No `component_sdk_*` gate runs in `ci.yml`, so export determinism and the release lifecycle are still only exercised when a publication is attempted. These gates are slow, so wiring them needs either a path filter or a chosen subset rather than an unconditional addition.
- [ ] `build-prefixes` hands the prefixes to `m3air` through `actions/upload-artifact@v4`, and `tree_digest` includes each file's executable bit. Whether the artifact round-trip preserves that bit across runners is untested here; if it does not, `prefix.treeHash` would be runner-dependent. Unproven either way — `2.0.0` published successfully, and the local/hosted archive-hash difference is fully explained by the `sel4/pins.toml` changes on `feat/duo-bringup`, so this session could not isolate the bit.
- [ ] The preflight compares the hosted release against a *local* export, so it detects hosted drift only for the profiles it exports and only when invoked. Nothing re-checks the hosted repository on a schedule.

## Artifacts and provenance

- Related roadmap items: [CP7](../../roadmap/10-component-platform.md), [CP9](../../roadmap/10-component-platform.md)
- New checker: `scripts/check/check-component-sdk-preflight.py`
- New gate: `just component_sdk_preflight`
- Predecessor entry: [`devlog/2026-08-26-cp7-hosted-publication-hardening/`](../2026-08-26-cp7-hosted-publication-hardening/index.md)
- Evidence: the preflight, guard-arm, YAML, `ruff`, and `typos` results above were observed in this session against `origin/main` at `ce256d2`; no raw transcript retained
