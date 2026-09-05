# Independent per-platform rust-sel4 loader branches

| Field | Value |
|---|---|
| Date | 2026-09-04 |
| Kind | Change |
| Status | Verified |
| Scope | `.gitmodules`, `sel4/pins.toml`, `scripts/build/build-sel4.py`, `scripts/generate/generate-system-image-closures.py`, `scripts/lib/system_image_closure.py`, `scripts/check/check-sel4-pins.py`, `.github/actions/slime-env/action.yml`, `deps/rust-sel4*` submodules, `contracts/system-image-closure/v1/closures/*.zti`, and the `iceice666/rust-sel4` fork's `slime-cv1800b-duo` / `slime-ns02201-h1v1` / `slime/bcm2712-loader-platform` branches |
| Roadmap | none |
| Gates | `just sel4_pin_check`, `just system_image_builder_check` |
| Trigger | A future physical platform's `sel4-kernel-loader` patch may need to live in a private fork for NDA reasons; the prior pin structure could not isolate one platform's patch from the others |
| Baseline | `sel4/pins.toml`'s single `[rust_sel4]` table pinned one commit (`20905bef`) on a cumulative branch (`slime-ns02201-h1v1`) that stacked the bcm2712, cv1800b-duo, and ns02201-h1v1 loader patches on top of each other; every platform, including the two QEMU targets that need no patch, built `sel4-kernel-loader` from that one checkout |

## Summary

`sel4/pins.toml` and `build-sel4.py` pinned exactly one `deps/rust-sel4`
checkout for every platform. The upstream fork's `sel4-kernel-loader` patches
for bcm2712-rpi5, cv1800b-duo, and ns02201-h1v1 lived on a single stacked
branch, so a platform whose patch cannot be public (an NDA'd board) would have
had to fork the whole cumulative chain rather than just its own arm. This
entry splits each physical platform's loader patch onto its own branch,
diverging independently from the `v5.0.0` base rather than stacking, and wires
`build-sel4.py` / the closure generator / the closure checker to build each
patched platform's loader from its own submodule. The SDK-facing crates
(`sel4`, `sel4-panicking-env`, `sel4-runtime-common`, target specs) are
untouched by any of these patches and continue to build from one shared base
checkout, now pinned at the unmodified `v5.0.0` release point instead of the
old cumulative tip.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `iceice666/rust-sel4` fork | Rebuilt `slime-cv1800b-duo` (dropped two unrelated x86-64 commits it had accumulated) and `slime-ns02201-h1v1` as branches that cherry-pick only their own platform's commits directly onto `d441a880` (v5.0.0); `slime/bcm2712-loader-platform` was already independent and is adopted as-is | Each platform's loader patch is reachable from exactly one branch with no other platform's code, so an NDA'd board's patch can move to a private fork with no effect on the others |
| `.gitmodules`, `deps/rust-sel4-{bcm2712-rpi5,cv1800b-duo,ns02201-h1v1}` | Added one submodule per patched platform, each a full checkout of its own branch, alongside the unchanged `deps/rust-sel4` | A build reads one platform's loader patch from one checkout; no shared mutable working tree is checked out to different commits across platforms |
| `sel4/pins.toml` | `[rust_sel4]` repinned to the unmodified `v5.0.0` base commit (`d441a880`); added `[rust_sel4_bcm2712_rpi5]`, `[rust_sel4_cv1800b_duo]`, `[rust_sel4_ns02201_h1v1]`, each with its own `repository`/`branch`/`commit` | A patched platform's repository can diverge (including to a private host) without changing the `[rust_sel4]` table every other platform reads |
| `scripts/build/build-sel4.py` | `Platform` gained a `loader_source` field; `build_loader` and `boot_bundle_identity` read from `platform.loader_source` instead of the global `RUST_SEL4_SOURCE` constant | The loader and its boot-bundle identity are built from the platform's own pinned checkout |
| `scripts/generate/generate-system-image-closures.py` | `LOADER_IMPLEMENTATION` (one constant) replaced by `LOADER_IMPLEMENTATIONS` (platform-keyed); `closure_for` selects by `profile["platform"]` and fails loudly on an undeclared platform | A closure's loader artifact reference names the submodule that platform actually builds from |
| `scripts/lib/system_image_closure.py` | Loader-implementation-path check generalized from a hardcoded `deps/rust-sel4` comparison to a platform-keyed `_LOADER_PATHS` table, independently re-derived rather than trusted from the closure | The checker still refuses a closure naming the wrong loader tree, now per platform |
| `scripts/check/check-sel4-pins.py` | `check_submodules` iterates the three new `[rust_sel4_<platform>]` sections and their submodule paths in addition to `sel4`/`rust_sel4` | Each per-platform submodule's checked-out commit and origin are verified against its own pin, same as the base |
| `.github/actions/slime-env/action.yml` | `cargo fetch` loop extended to the three new submodule manifests | CI's offline build has every workspace's crates cached before `build-sel4.py` runs `--offline` |
| `contracts/system-image-closure/v1/closures/*.zti` (50 files) | Regenerated. The `deps/rust-sel4` tree identity changed because the checked-out commit moved from the old cumulative tip to the unpatched `v5.0.0` base (fewer files: no bcm2712/cv1800b/ns02201 loader modules in a tree only `qemu-arm-virt` closures currently reference); regeneration also caught pre-existing drift in `slime-root`/`boot-contracts`/`just` tree identities unrelated to this change, already present at HEAD before this session | `--check` reports all 50 closures current against live tree state |
## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A platform's submodule commit or origin drifts from `sel4/pins.toml` | `just sel4_pin_check` | `check_submodules` reports the submodule's commit/origin mismatched against its `[rust_sel4_<platform>]` (or base `[rust_sel4]`) pin |
| A closure names the wrong `deps/rust-sel4*` tree for its platform | `just system_image_builder_check` | `resolve_closure` fails with "kernel-loader implementation must be the declared ... tree" |
| Closures drift from the live submodule/workspace tree state | `just system_image_builder_check` | `generate-system-image-closures.py --check` reports a closure is not current |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `python3 scripts/check/check-sel4-pins.py` | `seL4 pin check: exact source, toolchain, target, config, and host pins verified` | Direct |
| `python3 scripts/generate/generate-system-image-closures.py` (regenerate) then `--check` | `50 system-image closures are current` | Direct |
| `python3 -c "import ast; ast.parse(...)"` on every edited `.py` | Parses cleanly | Direct |
| `just test` (`sel4_root_boot_check`, `sel4_component_graph_check`, `sel4_gate_control_check`) on `qemu-arm-virt`, rebuilding seL4 and the loader from the repinned `deps/rust-sel4` v5.0.0 base | Exit 0. `seL4 root boot check: ordered generation, timer, task, IPC, fault, and ready markers observed`; `seL4 component graph check: init launched console, spawn-service, and Slisp ...; all four required resident instances remained live`; `seL4 gate control check: image identity accepted 1 valid pair and rejected 7 invalid pairs`, `48 gates reject 1879 mutated transcripts and layouts; 8 identity cases and 4 runtime cases passed` | Direct |
| `just contracts_check` | Not completed: the recipe's Rust release build of `zutai-cli` exceeded this session's time budget and was killed by an external timeout, unrelated to the edits here | Not run |
| Full physical-platform QEMU/loader build (`build-sel4.py --platform cv1800b-duo` etc.) | Not run: physical-board cross-toolchains were not exercised in this session | Not run |

## Decisions

- Decision: independent per-platform branches (each diverging from the shared
  `v5.0.0` base), not the cumulative stacking `deps/sel4` still uses.
- Rationale: the immediate driver is that one board's patch may need to live
  in a private fork; a cumulative chain would force that board's branch to be
  an ancestor of every later platform's branch, leaking a private patch into
  any public branch built on top of it. Independent branches keep each
  platform's patch, and its potential private divergence, fully isolated.
- Rejected alternative: keep the cumulative model and single checkout
  (matches `deps/sel4`'s existing convention) — rejected because it cannot
  express "this platform's patch is not publishable" without either merging
  the private patch into the public chain or forking the whole chain
  privately.
- Rejected alternative: one shared `deps/rust-sel4` submodule with
  `build-sel4.py` doing a dynamic `git checkout <commit>` per platform build —
  rejected because it mutates one working tree across builds (races on
  concurrent/`--exhaustive` builds, and a stale checkout after an aborted
  build), where a dedicated submodule per platform makes each build's input
  state static and independently reproducible.

## Open risks and follow-ups

- [ ] `just contracts_check`, `just system_image_builder_check`, and the
      remaining QEMU-plane gates (`sel4_boot_layout_check`, `sel4_qos_check`,
      `sel4_fault_check`, …) beyond `just test` were not re-run end-to-end in
      this session; run them before relying on this change for a release.
- [ ] `ns02201-h1v1` has no `sdk-release.json` profile yet (per
      `roadmap/00-backlog.md` and P6.A/P6.B status), so no closure currently
      exercises its `LOADER_IMPLEMENTATIONS` entry; it will be exercised the
      first time that platform gets a profile and closures.
- [ ] If a platform's patch actually needs to move to a private repository,
      only `sel4/pins.toml`'s `repository`/`commit` for that platform's table
      and `.gitmodules`' matching `url` change; no other platform's pin,
      closure, or build path is affected by this design, but that claim is
      untested against an actual private-repo checkout in CI (credentialing
      for a private submodule fetch is unaddressed).

## Artifacts and provenance

- Related roadmap item: none (infrastructure change, not a roadmap milestone)
