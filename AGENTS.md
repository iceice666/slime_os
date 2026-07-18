# Slime OS Agent Guide

## Scope

These instructions apply to the entire repository.

## Project state

Slime OS is a QEMU-verified Rust `no_std` kernel with a minimal userspace component graph. Treat Framework laptop bring-up, storage, rollbackable generations, native Dango, and daily-driver hardware support as unfinished unless code and tests prove otherwise.

## Commands

Use the Justfile targets from the repository root:

- `just run` — boot the current QEMU vertical slice.
- `just test` — run kernel and integration tests under QEMU.
- `just generation_check` — build and validate the deterministic generation binary.
- `just contracts_check` — validate generation manifest contracts.
- `just fmt_check` — check Rust formatting.
- `just lint` — run clippy with warnings denied.

## Development rules

- Prefer small, direct changes over new abstractions.
- Keep the kernel policy-free; component policy belongs in userspace.
- Preserve the capability/component/generation model. Do not add ambient authority, global executable paths, or implicit environment assumptions.
- Do not treat framebuffer output alone as milestone completion.
- Do not claim physical-machine support without an observed removable-media Framework boot that does not write internal NVMe.
- Keep generation data deterministic, versioned, bounded, and explicitly validated.

## Verification

- For kernel or userspace behavior changes, run the narrowest QEMU path that exercises the changed behavior.
- For generation-format or builder changes, run `just contracts_check` and `just generation_check`.
- For permanent Rust changes, run `just fmt_check` and `just lint` before finishing.
- For documentation-only changes, state that no runtime tests were run.
