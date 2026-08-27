# B80: the hosted RPi5 prefix build lacked its Rust target

| Field | Value |
|---|---|
| Date | 2026-08-27 |
| Kind | Defect |
| Status | Verified |
| Scope | `flake.nix`, `rust-toolchain.toml`, `just sel4_rpi5_image_check`, SDK publication workflow run 33063272940 |
| Roadmap | B80, P4 |
| Gates | `just sel4_rpi5_image_check`, `just sel4_pin_check`, `just fmt_check_all`, `just lint_all`, `just ruff`, `just devlog_check` |
| Trigger | The first two-profile SDK publication run reached the hosted RPi5 build and failed while compiling generation components for `aarch64-unknown-none` |
| Baseline | The RPi5 image gate passed on machines whose rustup state already carried `aarch64-unknown-none`, but a clean Nix shell installed only `x86_64-unknown-none` for the workspace toolchain |

## Summary

Clean Nix shells now install the Rust standard-library target that RPi5 generation components use. The RPi5 seL4 kernel and root use pinned seL4 JSON targets, but its userspace component graph is built for the built-in `aarch64-unknown-none` target. `flake.nix` and `rust-toolchain.toml` listed only `x86_64-unknown-none`, so a clean hosted runner failed with `E0463` after successfully building and verifying the RPi5 kernel prefix. Both target declarations now include `aarch64-unknown-none`, preserving the existing synchronization contract and making the two-profile SDK build self-contained.

## Observable symptom

- Command: `nix develop --command just sel4_rpi5_image_check` in GitHub Actions run 33063272940.
- Expected: build and install the RPi5 seL4 prefix, build the target-qualified component generation, and package the board image.
- Observed: the kernel prefix passed its pin check; Cargo then failed with `can't find crate for core` and `the aarch64-unknown-none target may not be installed`.
- Exit/fault/serial evidence: `Build seL4 prefixes` exited 1; the publish job was skipped and no deployment approval was created.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | The QEMU arm passed on the same clean hosted runner, including external Slisp construction | Nix, Clang, and the general hosted build path were working |
| 2 | RPi5 reached `build_rust_components` and invoked Cargo with `--target aarch64-unknown-none` | The failure was after kernel construction, in the board generation's userspace target |
| 3 | `rustTargets` and `rust-toolchain.toml` named only `x86_64-unknown-none` | Clean shells could not satisfy a target the permanent RPi5 gate requires |

## Root cause

The development shell's explicit target list overrode the target inventory Cargo would otherwise infer, but it omitted the built-in AArch64 bare-metal target used by RPi5 components. Existing machines masked the omission through persistent rustup state. Hosted runners start clean, so the missing declaration became an immediate `core` lookup failure.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `flake.nix` | Added `aarch64-unknown-none` to `rustTargets`, which the shell hook installs for the pinned workspace nightly | A clean `nix develop` contains every built-in Rust target permanent gates invoke |
| `rust-toolchain.toml` | Added the same target to the checked-in toolchain target list | The repository's two target inventories remain synchronized as their comment requires |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Clean RPi5 builds lose the target again | Hosted `nix develop --command just sel4_rpi5_image_check` | Cargo `E0463` for `core` or an absent `aarch64-unknown-none` target |
| Toolchain inventory drifts | `just sel4_pin_check` | The pin checker or future inventory assertion rejects the shell/toolchain mismatch |
| Repository checks regress | `just fmt_check_all`, `just lint_all`, `just ruff`, `just devlog_check` | Any named check fails |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| GitHub Actions run 33063272940 | Reproduced: QEMU prefix succeeded; RPi5 kernel prefix succeeded; component build failed with missing `aarch64-unknown-none` `core` | Direct |
| `nix develop --command rustup target list --installed` | Pass: listed `aarch64-unknown-none` under the pinned workspace nightly | Direct |
| `nix flake check --no-build` and `just sel4_pin_check` | Pass on Darwin; the flake evaluated the local shell and the pin contract remained valid | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just devlog_check` | Pass | Direct |
| GitHub Actions run 33063988008 | Pass for B80: the hosted RPi5 arm compiled the root child and generation components with `aarch64-unknown-none`, then exposed the independent B81 Slisp segment-layout defect | Direct |

## Decisions

- Decision: install the built-in target in both repository target inventories.
- Rationale: the target is a permanent input to `sel4_rpi5_image_check`; relying on mutable rustup state outside the checkout made clean builds non-reproducible.
- Rejected alternative: add `-Z build-std` to the ordinary component build. The components use a stable built-in target whose prebuilt `core` is the simpler, existing contract.

## Open risks and follow-ups

- [x] Run 33063988008 confirmed the clean hosted shell carried `aarch64-unknown-none`; the later B81 linker-layout failure is tracked separately.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [missing-target run 33063272940](https://github.com/iceice666/slime_os/actions/runs/33063272940), [post-fix run 33063988008](https://github.com/iceice666/slime_os/actions/runs/33063988008).
- Serial/debugger/model output: none; construction failed before packaging or boot.
- Related roadmap item: [P4](../../roadmap/07-architecture-portability.md#p4--raspberry-pi-5-board-bring-up).
- Predecessor: [`devlog/2026-08-27-b79-default-sel4-build/`](../2026-08-27-b79-default-sel4-build/index.md)
