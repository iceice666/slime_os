# CI: a gate that could not run, a job that could not start, and a path that only resolved on one machine

| Field | Value |
|---|---|
| Date | 2026-08-27 |
| Kind | Defect |
| Status | Verified |
| Scope | `.github/workflows/ci.yml`, `.github/actions/slime-env/action.yml`, `Justfile` (`lint_sel4_root`), `scripts/lib/release_trust.py` |
| Roadmap | B78 |
| Gates | `just lint_sel4_root`, `just test_sel4_root`, `just contracts_check`, `just component_spec_check`, `just bootstate_trace_check`, `just release_trust_check`, `just x86_portability_check`, `just framework_safety_check`, `just sel4_gate_control_check`, `just devlog_check` |
| Trigger | Every CI run since `84c75f5` red or never-starting; runs 32987525467 and 32990305002 observed |
| Baseline | `64c838a` added the gates when every one of them ran without a seL4 prefix |

## Summary

Three independent defects made CI structurally incapable of passing, and
fixing them exposed four more that only a clean machine can observe. The
`contracts` job ran `just contracts_check`, which since `84c75f5` builds all 36
seL4 manifests and reads the wire magic out of the bytes — a build that needs
`build/sel4-prefix` and `LIBCLANG_PATH`, neither of which an ordinary runner
has, so the job failed on every commit. The `sel4_builder` job requested
`[self-hosted, linux, sel4-builder]`, and the repository has zero registered
runners, so it sat `queued` forever and no run ever concluded. Separately,
`lint_sel4_root` looked for the root child ELF at a path that stopped being
written at `0dd7d0c`; it resolved only on a checkout still holding a
pre-`0dd7d0c` build, so the gate would have refused on any clean machine even
had a runner existed. The prefix-dependent gates now run on GitHub's free
`ubuntu-24.04-arm` runners inside this repository's own Nix dev shell, and run
33002668719 is green across all ten jobs.

## Observable symptom

- Command: `gh run view 32990305002`
- Expected: every job concludes.
- Observed: `Contracts and devlog` failed in 3m19s; `Pinned seL4 lint and root
  tests` never left `queued`.
- Exit/fault/serial evidence:
  - `generation v5 check: sel4: build failed: ['no installed seL4 prefix at build/sel4-prefix; run `just sel4_qemu_image_check` first']`, then
    `error: recipe 'contracts_check' failed on line 642 with exit code 1`.
  - `gh api repos/iceice666/slime_os/actions/runners` → `{"total_count":0,"runners":[]}`.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | Three most recent runs: two `queued` indefinitely, one `cancelled` by hand | The suite has never gone green; this is structural, not flaky |
| 2 | `--log-failed` on the contracts job names the missing prefix | `contracts_check` is not an ordinary-runner gate any more |
| 3 | `git log -S check-generation-v5 -- Justfile` → `84c75f5` | The gate changed cost when the seL4 cutover landed; the workflow was written against its earlier shape and never moved |
| 4 | Runners API returns `total_count: 0` | The `self-hosted` label resolves to nothing; the job waits for the 24h reaper |
| 5 | `Justfile:986` names `build/sel4-cargo/child/...`; `build-sel4.py:977` writes `CARGO_BUILD / platform.name / "child"` | The lint gate's precondition path is stale since `0dd7d0c` |
| 6 | `stat` shows the unqualified copy dated 2026-08-23, the qualified one 2026-08-24 | It passes here only because a stale artifact survives; a clean tree would fail |
| 7 | Moved the stale directory aside and re-ran the fixed recipe | Passed in 4.4s against the qualified path alone, so the fix is the path and not the leftover |

## Root cause

Two gates outgrew the runner they were assigned, and one path outlived the
layout it named.

`check-generation-v5.py` deliberately proves the wire version *by building*
rather than by reading `formatVersion`, because that field is the manifest
schema's version and says nothing about the bytes. That is the right design,
and it makes the check a compile-against-libsel4 operation:
`build_rust_components` routes seL4 profiles through
`sel4_component_environment`, which requires the installed prefix and bindgen's
`LIBCLANG_PATH`. `contracts_check` was therefore a prefix gate wearing an
ordinary-runner job's clothes.

The `sel4_builder` job encoded an intent — "a labelled builder is provisioned
through this repository's pinned Nix shell" — that was never true of this
repository. A `runs-on` label is a claim about infrastructure, and nothing
validates it: GitHub queues the job rather than failing it, so an absent runner
is indistinguishable from a busy one until the timeout.

`0dd7d0c` gave `build-sel4.py` a second platform and correctly qualified every
per-platform output directory by `platform.name`. The `Justfile` precondition
check was not part of that rename, and because it is a *guard* rather than a
consumer — it only tests `-f` before exporting `CHILD_ELF` — the stale path
produced no error anywhere except on a machine without the old artifact.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `Justfile` | `lint_sel4_root`'s `child_elf` is platform-qualified to `build/sel4-cargo/qemu-arm-virt/child/...` | The gate's precondition names the path the builder writes, on any checkout |
| `.github/workflows/ci.yml` | `sel4_builder` deleted; `sel4_product`, `sel4_rollback`, `sel4_contracts` run on `ubuntu-24.04-arm` and build the prefix before consuming it | Every gate runs on a runner that exists and can satisfy it |
| `.github/workflows/ci.yml` | `contracts_check`/`component_spec_check` moved off `ubuntu-latest`; the tree-only gates split into `docs_gates` | A job's runner matches what its gates actually need |
| `.github/actions/slime-env/action.yml` | New composite action: Nix install, store + `~/.rustup` + `~/.cargo` cache, dev-shell realization, and the offline prefetch below | One definition of "the supported environment", shared by every heavy job |
| `.github/actions/slime-env/action.yml` | Prefetches this repository's three lockfiles *and* the toolchains' `rust-src` library workspace | `--locked --offline` builds resolve on a machine with no cargo cache |
| `scripts/lib/release_trust.py` | Narrows the fixture private keys to `0600` before invoking `ssh-keygen` | The release gates run from a clean clone, where git cannot carry the mode |
| `.github/workflows/ci.yml` | `timeout-minutes` on all ten jobs, sized from observed cold runs; `concurrency` cancels superseded runs on pushes as well as pull requests | A stalled build fails visibly, and a new push does not queue behind the previous run's slowest job |
| `.github/workflows/ci.yml` | Node-20 actions bumped (`checkout@v7`, `setup-just@v4`, `ruff-action@v4.1.0`) | The deprecation warnings on every job are gone |

Nix is installed with `nixbuild/nix-quick-install-action` and the store cached
with `nix-community/cache-nix-action`, both upstream CppNix tooling.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A prefix-dependent gate is added to an ordinary-runner job again | The three `sel4_*` jobs each run `just sel4_qemu_image_check` first, and the tree-only gates are isolated in `docs_gates` | The misplaced gate fails with `no installed seL4 prefix`, as `contracts_check` did |
| A per-platform output path goes stale again | `lint_sel4_root` refuses rather than linting a different configuration | `no root child ELF at <path>` |
| A job hangs instead of failing | `timeout-minutes` on every job | The job is cancelled at its bound |
| Toolchain drift reaches a downstream gate | `sel4_qemu_image_check` re-checks `[observed_prefix]` hashes at the top of each seL4 job | Pin-check failure in the first step |
| A fixture whose usability depends on state git cannot carry | The permission narrowing lives in `release_trust.py`, so any consumer of those keys inherits it | `ssh-keygen ... exit status 255` |
| An offline build gains a dependency the prefetch does not cover | The prefetch fails loudly when a `rust-src` workspace is absent rather than skipping it | `no matching package named '<crate>' found` under `--offline` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just lint_sel4_root`, with `build/sel4-cargo/child/` moved aside | Passed in 4.4s; resolved the qualified path alone | Direct |
| `just test_sel4_root` | `183/183 across 19 modules` | Direct |
| `just contracts_check` | Passed in 2m43s; `all 36 seL4 manifests encode SLIMEG5 version 5` | Direct |
| `just component_spec_check` | Passed in 2m51s; 42 records, 43 mutations refused | Direct |
| `just bootstate_trace_check` | Passed in 3m10s; 16 markers, 7 durable transitions, real QEMU boot | Direct |
| `just release_trust_check` | Passed in 3m3s; threshold, replay, and rotation refusals observed | Direct |
| `just x86_portability_check`, `just framework_safety_check`, `just sel4_gate_control_check` | All passed with no prefix present, confirming the `docs_gates` grouping | Direct |
| `just devlog_check` | `228 entries, 228 indexed` | Direct |
| `actionlint .github/workflows/ci.yml` | Clean | Direct |
| CI run 33002668719 (`45a482c`) | All ten jobs green, including the three arm64 seL4 jobs | Direct |

Local timings are from `aarch64-darwin` with a warm cache. The hosted arm64
numbers are cold and observed: 8m, 25m, and 28m.

Reaching that green run took four further defects, each fixed at its source:

| Defect | Root cause | Fix |
|---|---|---|
| All three arm64 jobs: `Can't find 'action.yml'` | A local composite action is resolved from the checked-out tree, so it cannot perform its own checkout | Checkout moved to each caller |
| `ruff-action@v4` unresolvable | The action publishes no floating `v4` tag, unlike its `v3.6` | Pinned `v4.1.0` |
| Child build: `no matching package named 'unwinding'` under `--offline` | `unwinding` is a dependency of the *sysroot* workspace `-Z build-std` compiles from `rust-src`, not of any manifest here; prefetching our three lockfiles could not satisfy it | Prefetch the toolchain's `lib/rustlib/src/rust/library/Cargo.toml` too |
| `ssh-keygen -y` exit 255 on the release keys | git tracks only the execute bit, so `0600` fixture keys materialize `0644` on a clean clone and ssh-keygen refuses them | `release_trust.py` narrows the mode before use |

The last two are the interesting ones: both passed on every developer machine
for the same reason — a warm cargo cache, and file modes surviving from when
the keys were generated — so neither was observable without a clean checkout.

## Decisions

- Decision: run the prefix-dependent gates on GitHub-hosted `ubuntu-24.04-arm`
  rather than provisioning a self-hosted runner.
  Rationale: `aarch64-sel4-qemu-virt` is an AArch64 product; the arm64 runners
  are native, and free for public repositories, which this one is. A
  self-hosted runner is infrastructure that can silently disappear — which is
  exactly the failure being fixed.
  Rejected alternative: keeping `self-hosted` behind a repository variable so
  the job skips when no runner is registered. It makes CI green without making
  it cover anything, which is worse than the visible failure.
- Decision: split the seL4 work across three jobs rather than one.
  Rationale: they run concurrently, so the suite's critical path is the
  slowest job (28m) rather than their sum (61m), and a failure names the
  subsystem that broke. Disk was the original reason and turned out not to be
  one: the runner reports 119 GB free, against the ~14 GB the hosted-runner
  documentation quotes, so the reclamation step written for that budget is
  deleted.
  Rejected alternative: one job running every gate, which triples the critical
  path and makes every failure report the same job name.
- Decision: keep `check-generation-v5.py` building rather than reading
  `formatVersion`.
  Rationale: the check exists precisely because the declared field is not the
  wire format. Making it cheap would make it vacuous; the job moved instead.

## Open risks and follow-ups

- [x] Cold-build wall time, now observed on run 33002668719: `sel4_product`
  8m, `sel4_contracts` 25m, `sel4_rollback` 28m. The 90-minute placeholders
  are retuned to 30/60/60 against those numbers.
- [x] Disk headroom, now observed: `df -h /` on the arm64 runner reports 145G
  total with 119G available before any build, not the ~14G the hosted-runner
  documentation quotes. The SDK-reclamation step was written for the wrong
  number and is deleted.
- [ ] `just deny`, `just machete`, `just ruff`, and `just typos` still run
  through their upstream actions rather than the Justfile targets, so the
  versions CI uses are not the dev shell's.
- [ ] No job runs the QEMU boot aggregate (`just test`); the rollback plane is
  the only guest boot in CI.

## Artifacts and provenance

- Failing run: <https://github.com/iceice666/slime_os/actions/runs/32990305002>
- Prior queued-forever run: <https://github.com/iceice666/slime_os/actions/runs/32987525467>
- Related roadmap item: `roadmap/00-backlog.md` B78
