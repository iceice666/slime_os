# m3air becomes the credential-isolated SDK publication runner

| Field | Value |
|---|---|
| Date | 2026-08-27 |
| Kind | Change |
| Status | Verified |
| Scope | `.github/workflows/publish-sdk.yml`, GitHub `release` environment, repository self-hosted runner `m3air`, `iceice666@m3air` LaunchAgent |
| Roadmap | CP7 |
| Gates | `just component_sdk_release_check`, `just component_sdk_prefix_check`, `just devlog_check` |
| Trigger | CP7's canonical SDK releases still required an operator to SSH into m3air and run the publisher by hand because the signing and push credentials intentionally exist only on that machine |
| Baseline | `iceice666/slime_os` had no registered self-hosted runner and no workflow could reach the release credentials; prefix construction and publication were one manual operation with no GitHub environment approval boundary |

## Summary

`m3air` is now a persistent repository-scoped GitHub Actions runner, while expensive reproducible prefix construction stays on GitHub's hosted arm64 fleet. The manual-only `Publish component SDK` workflow builds the selected QEMU and RPi5 seL4 prefixes without credentials, uploads them as a one-day artifact, then requires approval for the protected `release` environment before scheduling `Publish and verify` on `[self-hosted, m3air]`. Only that second job can read the existing SDK push key, tag-signing key, and allowed-signers file. The workflow publishes one atomic signed release and verifies the resulting hosted tag from a clean clone.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `iceice666@m3air` | Installed GitHub Actions runner 2.337.0 under `~/actions-runner`, registered it as `m3air`, and installed its per-user LaunchAgent | The credential-owning Mac accepts repository jobs without an interactive SSH publication session |
| GitHub `iceice666/slime_os` | Created the protected `release` environment with `iceice666` as required reviewer and a custom deployment policy allowing only `main` | A dispatched job cannot touch release credentials before explicit approval, and non-main refs cannot deploy |
| `.github/workflows/publish-sdk.yml` | Added a `workflow_dispatch`-only two-job release workflow: hosted arm64 prefix build, then protected self-hosted publication and tag verification | Build inputs are reproducible and credential-free; credential use is isolated to the smallest job and never runs on push or pull request |
| Runner filesystem | Restricted `.runner` and `.credentials` to mode 0600 | Repository runner identity material is readable only by the runner account |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Ordinary source events trigger release credentials | Workflow event set contains only `workflow_dispatch` | A `push` or `pull_request` run of `Publish component SDK` exists |
| Prefix construction gains access to release keys | Separate hosted `build-prefixes` job with read-only contents permission | A credential path or self-hosted label appears in the build job |
| An unapproved or non-main deployment reaches m3air | Protected `release` environment and `main` deployment branch policy | GitHub schedules `Publish and verify` without review or for another ref |
| Runner disappears silently | Repository runners API and LaunchAgent `KeepAlive` state | `m3air` is offline or the service is not running |
| Publication creates an unsigned or partial release | Publisher's atomic push plus workflow `verify-tag` step | Branch/tag mismatch, rejected push, or failed SSH signature verification |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Runner bundle SHA-256 | Pass: runner 2.337.0 archive matched `5a2cd92908a93d7276a194e1de6008099f3e7946f3f8e14aa7a1a7b4a31fdec2` | Direct |
| `svc.sh status` and GitHub runners API | Pass: LaunchAgent running; repository reported `m3air` online, idle, with `self-hosted`, `macOS`, `ARM64`, and `m3air` labels; rechecked after publication | Direct |
| Runner credential modes | Pass: `.runner` and `.credentials` were both mode 0600 | Direct |
| GitHub environment inspection | Pass: required reviewer `iceice666`; custom branch policy enabled; sole deployment policy `main` of type `branch`; rechecked after publication | Direct |
| Workflow source review | Pass: manual-only trigger, read-only default contents permission, hosted arm64 prefix job, protected `[self-hosted, m3air]` publish job, atomic publisher invocation, and hosted signature verification | Direct |
| SDK 2.0.0 two-profile workflow run 33072132822 | Pass: hosted arm64 built and packaged QEMU plus RPi5 prefixes; deployment 6122752148 crossed the protected `release` environment; `m3air` published generated commit `3633ea9d8cf3f4d22f5b6f05aab9f2e51e340bc0` and verified `sdk-v2.0.0` from a clean clone | Direct |
| Hosted SDK release record | Pass: version `2.0.0` binds source commit `e6e1a91172b7adb8e2cfba8c51726e9ecf9aa76b`, both `aarch64-sel4-qemu-virt` and `aarch64-rpi5` prefix archives, tree identity `5131d19ade9e13af6775dd74c7ff88738c5d52952cd007d7717b8703a5bd09b4`, and record identity `5f5449cb1c7120d0b7d9f212ae7241a80082c028a106351b81f0dd4f00532969` | Direct |
| Independent `m3air` clean-clone audit | Pass: generated branch and dereferenced `sdk-v2.0.0` both resolved to `3633ea9d8cf3f4d22f5b6f05aab9f2e51e340bc0`; SSH signature verified for `release-bot@slime-os.invalid` with ED25519 key `SHA256:+YG3ChntQX3MZxE9VtUUO0aCAIIzpTV8mHfpmDxFcOI` | Direct |
| `nix develop --command just component_sdk_upgrade_check` after direct product-manifest fixture repair | Pass: the complete CP4–CP10 chain completed, including prefix qualification, compatibility boots, upgrade failure injection, and rollback | Direct |
| `nix develop --command just ruff`, `just fmt_check_all`, `just lint_all`, `just devlog_check`, and `just typos` after evidence finalization | Pass | Direct |

## Decisions

- Decision: use m3air only for the credential-bearing publication job, not for prefix construction.
- Rationale: the Mac already owns the least-privilege keys, while GitHub-hosted arm64 runners provide the Nix and cross-build capacity without expanding that machine's toolchain or attack surface.
- Rejected alternative: install Nix and the complete seL4 build environment on m3air. That would duplicate the reproducible hosted builder and increase maintenance on the credential host.

- Decision: require a protected environment review even though `workflow_dispatch` already requires repository write access.
- Rationale: dispatch authorizes intent; environment review authorizes the moment credentials are exposed and leaves a separate deployment audit event.
- Rejected alternative: put the keys in GitHub secrets and publish on a hosted runner. That would move long-lived release authority away from the machine and repository rules CP7 already hardened.

## Open risks and follow-ups

- [ ] The runner is a per-user LaunchAgent; it resumes with that user's GUI session and is not a machine-wide daemon before login.
- [ ] Runner updates remain an operator action; the installed bundle is pinned and checksum-verified but not automatically upgraded.
- [x] SDK 2.0.0 publication evidence is captured above from workflow run 33072132822 and an independent clean-clone signature audit.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [successful SDK 2.0.0 workflow run 33072132822](https://github.com/iceice666/slime_os/actions/runs/33072132822), including hosted prefix job 98516828688 and `m3air` publication job 98518319007; prior failed build-only runs are recorded by B79, B80, and B81.
- Serial/debugger/model output: none; this is repository automation and hosted publication infrastructure.
- Related roadmap item: [CP7](../../roadmap/10-component-platform.md#cp7--permanent-sdk-repository-and-one-way-publication).
- Predecessor: [`devlog/2026-08-26-cp7-hosted-publication-hardening/`](../2026-08-26-cp7-hosted-publication-hardening/index.md)
