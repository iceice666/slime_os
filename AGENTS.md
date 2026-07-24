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

## Backlog before roadmap

`roadmap/00-backlog.md` tracks known defects, regressions, and latent bugs in implemented code. Resolve open backlog items before starting a new `roadmap/` track milestone. A green verification suite is a precondition for milestone work, not a milestone itself; if you cannot resolve an open backlog item, record why it is deferred rather than silently skipping it. When you fix a defect, move its entry to the backlog's resolved log with the observed exit condition rather than deleting it.

## Development log

`devlog/` is the curated, chronological record of investigations, regressions, design decisions, and verification results. Record an entry whenever you complete a roadmap milestone or land a non-trivial feature, make a design or architecture decision, fix a non-trivial regression, root-cause a defect, or run a verification campaign: a folder `devlog/YYYY-MM-DD-short-topic/` with a curated `index.md` written from `devlog/TEMPLATE.md`, keeping focused reports, raw transcripts, and other evidence as siblings in that folder. Register the entry in `devlog/README.md` and follow its evidence rules — prefer exact `just` targets and observed results, label inherited evidence and unobserved conclusions, and never rewrite a raw log. Roadmap completion stays authoritative in `roadmap/`; devlog entries explain how conclusions were reached. When a backlog item is resolved, link its devlog entry from the backlog's resolved log.

## Development rules

- **Zutai is the only schema language.** Every serialized format that crosses a persistence, process, or boot boundary — on-disk formats, IPC/protocol messages, manifests, handoff structures — must be defined as a versioned Zutai schema under `contracts/` (`schema.zt`), with Rust/Python bindings generated from it (`scripts/generate-*-bindings.py`, `just *_gen`). Do not introduce hand-written field offsets, ad-hoc `#[repr(C)]` wire structs, `struct.pack` layouts, or any other schema language (JSON Schema, protobuf, etc.) as the source of truth for a format. Purely in-memory types are exempt.
- Prefer small, direct changes over new abstractions.
- Keep the kernel policy-free; component policy belongs in userspace.
- Preserve the capability/component/generation model. Do not add ambient authority, global executable paths, or implicit environment assumptions.
- Do not treat framebuffer output alone as milestone completion.
- Do not claim physical-machine support without an observed removable-media Framework boot that does not write internal NVMe.
- Keep generation data deterministic, versioned, bounded, and explicitly validated.

## Verification

- For kernel or userspace behavior changes, run the narrowest QEMU path that exercises the changed behavior.
- For generation-format or builder changes, run `just contracts_check` and `just generation_check`.
- For permanent Rust changes, run `just fmt_check` and `just lint` before finishing (or the `_components` variants for changes under `components/`).
- For documentation-only changes, state that no runtime tests were run.
