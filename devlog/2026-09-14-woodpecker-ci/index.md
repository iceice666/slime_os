# General CI moves to Woodpecker

| Field | Value |
|---|---|
| Date | 2026-09-14 |
| Kind | Change |
| Status | Monitoring |
| Scope | `.woodpecker/`, `scripts/ci/prepare-sel4.sh`, GitHub workflows, contributor CI guidance, local Git origin |
| Work items | none |
| Gates | `just typos`, `just devlog_check` |
| Trigger | Maintainer request to change origin to the self-hosted SSH forge and migrate CI to Woodpecker |
| Baseline | General CI ran in `.github/workflows/ci.yml`; SDK publication and issue projection had separate GitHub workflows |

## Summary

General CI now uses four Woodpecker workflow files. Push filters for `main`
and `develop`, unrestricted pull-request target branches, both Miri packages,
all four generation shards, and the existing verification commands are
preserved. Local configuration and helper execution were checked; no hosted
Woodpecker run or Linux container build was observed.

## Changes

| Area | Change | Preserved invariant |
|---|---|---|
| Host and Kani workflows | Nine host matrix jobs and one separate Kani job on Linux AMD64 | Existing host gates and both proof suites remain required |
| seL4 workflows | Three product matrix jobs and four generation shards on Linux ARM64 | Preparation precedes the pinned image and dependent gates |
| Environment | Versioned Nix image, pinned repository shells, per-job command timeouts | Product builds still use the declared toolchains and locked offline dependencies |
| Preparation | Fetch five manifest closures under both toolchains and both rust-src workspaces | Offline build-std does not depend on an accidentally warm cache |
| GitHub integration | Remove general CI and its projector completion subscription; keep release and projection workflows | CI jobs receive no release signing or issue-writing credentials |
| Local origin | Fetch and push URL changed to `ssh://git@git.justaslime.dev:2222/iceice666/slime_os.git` | No push, remote connection test, or submodule URL change |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Invalid workflow syntax | `woodpecker-cli lint --strict .woodpecker/` | Official validator rejects the configuration |
| Unsafe helper shell constructs | `shellcheck scripts/ci/prepare-sel4.sh` | ShellCheck diagnostic |
| Documentation drift | `just typos`, `just devlog_check` | Invalid spelling, entry structure, or links |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Woodpecker CLI 3.16.0 strict lint | All four workflow files valid | Direct |
| In-memory matrix expansion and original-workflow comparison | 17 jobs; all 21 original Just recipes and direct checks retained; four shard arguments and trigger parity checked; expanded shell commands parse | Direct |
| `bash scripts/ci/prepare-sel4.sh` | Exit 0 in the existing development environment; real locked dependency fetches completed | Direct |
| `bash -n scripts/ci/prepare-sel4.sh` and ShellCheck 0.11.0 | Exit 0 | Direct |
| `python3 scripts/check/check-sel4-pins.py --skip-host-tools` | Exact source, toolchain, target, and configuration pins verified | Direct |
| `just typos` | Exit 0, including the completed migration documentation | Direct |
| `just devlog_check` | Exit 0; 327 entries, 327 indexed | Direct |
| Docker availability | Local daemon socket absent; container execution unavailable | Direct |

## Decisions

- Use Woodpecker's overall pipeline result rather than synthetic success-only
  aggregate jobs. Branch protection must use the status actually emitted by
  the new forge integration.
- Keep every job's setup and gates inside one container. Separate containers
  do not share their Nix store or home directory merely by sharing a workspace.
- Do not introduce privileged host mounts or shared writable caches for PR
  jobs. Cold runs now fetch dependencies again; no GitHub cache is transferable.
- Use the existing pinned default shell for host jobs too. This increases
  cold setup cost relative to the old lightweight GitHub jobs, but avoids
  introducing a second compiler/tool installation convention.
- SDK publication and GitHub issue projection are not ordinary CI. Their
  platform-specific workflows remain; this does not migrate either service
  to the new forge.

## Open risks and follow-ups

- No observed Woodpecker server run, clone authentication, agent provisioning,
  Linux container build, or end-to-end QEMU result from the new pipeline.
- Configure Docker agents for Linux AMD64 and ARM64, pull-request approval,
  previous-pipeline cancellation, an overall timeout above 60 minutes, and
  branch protection. These settings belong to the server, not workflow YAML.
- Cold Nix realization now falls within command timeouts; capacity and cold
  timing need observation on the actual agents.
- Existing GitHub issue reconciliation operates only on the GitHub repository;
  it does not mirror issues or new-forge checks. The SDK still publishes to its
  existing repository using its existing release runner.

## Artifacts and provenance

- Workflows: [host](../../.woodpecker/host.yml),
  [Kani](../../.woodpecker/kani.yml), [seL4](../../.woodpecker/sel4.yml),
  [generation shards](../../.woodpecker/generations.yml).
- Operator guidance: [Continuous integration](../../CONTRIBUTING.md#continuous-integration).
- Syntax and operational boundaries: [Woodpecker workflow syntax](https://woodpecker-ci.org/docs/usage/workflow-syntax)
  and [project settings](https://woodpecker-ci.org/docs/usage/project-settings).
- Verification output was observed in the implementation session; no raw
  transcript is committed, and no remote CI evidence is claimed.
