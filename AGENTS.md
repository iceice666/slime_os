# Slime OS Agent Guide

## Scope

These instructions apply to the entire repository.

## Project state

Slime OS is a QEMU-verified Rust `no_std` kernel with a minimal userspace component graph. Treat Framework laptop bring-up, storage, rollbackable generations, native Dango, and daily-driver hardware support as unfinished unless code and tests prove otherwise.

## Code map: start here, do not broad-search

Route work by ownership before searching for a symbol. Read the named module root first; use LSP symbols/references from there when available, and only then grep the exact symbol. Do not scan `deps/`, `target/`, `devlog/`, or `roadmap/` for implementation symbols unless the task specifically concerns them.

### Execution path

1. `stage0/src/main.rs::boot` selects and verifies boot state, generation, release, and kernel image, builds the handoff, and jumps to the kernel. Pure boot-selection helpers live in `stage0/src/lib.rs`.
2. `kernel/src/main.rs::kernel_main` initializes architecture, memory, devices, input, and time, then calls `kernel/src/runtime/bootstrap.rs::start`.
3. `kernel/src/runtime/bootstrap.rs` decodes the selected generation, creates the initial capability graph, launches components, records component task ids, and decides the healthy/idle exit condition.
4. Components enter through `components/bins/src/bin/*.rs`; their syscall surface is `components/runtime/src/syscall.rs`, whose authoritative kernel implementation is `kernel/src/syscall/mod.rs`.

### Task-to-file index

| Change | Canonical starting point | Follow-on files |
| --- | --- | --- |
| Capability kinds, rights, tables, derivation | `kernel/src/capability/mod.rs` | `kernel/src/syscall/mod.rs`, generation grants in `kernel/src/runtime/bootstrap.rs` |
| Channel IPC, message bounds, endpoint lifetime | `kernel/src/ipc/mod.rs` | `kernel/src/syscall/mod.rs::{sys_send,sys_recv}`, `components/runtime/src/syscall.rs` |
| Tasks, spawn, scheduling, wait, termination, reclamation | `kernel/src/task/mod.rs` | `kernel/src/syscall/mod.rs`, `kernel/src/memory/address_space.rs` |
| Syscall numbers, argument validation, rights gates | `kernel/src/syscall/mod.rs` | Mirror wrappers in `components/runtime/src/syscall.rs`; the kernel file is authoritative |
| Physical/virtual memory and heap | `kernel/src/memory/mod.rs` | `kernel/src/memory/{pmm,vmm,heap,address_space}.rs` |
| Shared-buffer allocation, mapping, loan, accounting | `kernel/src/memory/shared_buffer.rs` | handle kinds in `kernel/src/capability/mod.rs`, gates in `kernel/src/syscall/mod.rs` |
| Boot graph and component launch grants | `kernel/src/runtime/bootstrap.rs` | generation lookup in `kernel/src/runtime/generation.rs`, manifest fixture below |
| Generation decoding and identity | `boot-contracts/src/generation.rs` | kernel admission in `kernel/src/runtime/generation.rs` |
| Generation construction and manifest contents | `scripts/build/build-generation.py` | `contracts/generation/v1/fixtures/valid.zti`, `components/bins/build.rs` |
| Component image format/loading | `contracts/component/v1/schema.zt` | generated `components/proto/src/component.rs`, decoder `kernel/src/runtime/component.rs` |
| Userspace component behavior | `components/bins/src/bin/<component>.rs` | shared helpers in `components/bins/src/*.rs`; binary list in `components/bins/Cargo.toml` |
| Userspace syscall ABI | `components/runtime/src/syscall.rs` | exports in `components/runtime/src/lib.rs`, kernel implementation in `kernel/src/syscall/mod.rs` |
| IPC/service protocol semantics | `contracts/<protocol>/v1/schema.zt` | generated Rust in `components/proto/src/<protocol>.rs`; validators in `components/proto/src/lib.rs` |
| Boot/persistence contract decoder | `boot-contracts/src/<contract>.rs` | generated constants/layouts in `boot-contracts/src/generated/` |
| Fabric schemas, graph authority, stream framing | `contracts/interface-schema/v1/`, `contracts/fabric-graph/v1/`, `contracts/fabric-stream/v1/` | `boot-contracts/src/fabric_graph.rs`, `components/bins/src/bin/fabric-service.rs` |
| Block/storage transport and services | `kernel/src/storage/mod.rs` | `kernel/src/storage/{block_device,block_service,object_store,store_service}.rs`; hardware in `kernel/src/drivers/{virtio_blk,nvme,dma}.rs` |
| Generation management, rollback, recovery | `kernel/src/runtime/generation_manager.rs` | `kernel/src/runtime/generation_service.rs`, `kernel/src/storage/{transfer,recovery}.rs`, matching component binaries |
| Architecture, traps, interrupts, platform boot | `kernel/src/arch/x86_64/mod.rs` | `kernel/src/arch/x86_64/{trap,interrupts,boot,platform,pci}.rs` |
| Host build/check orchestration | `Justfile` target | implementation in `scripts/{build,check,generate,lib}/` |
| Kernel behavioral regression | `kernel/tests/<feature>.rs` | run the matching named `just *_check` or narrow Cargo/QEMU target |
| Protocol validation regression | `components/proto/tests/<protocol>.rs` | generated protocol module and schema |

### Generated-code rule

Files beginning with `@generated` and files under `boot-contracts/src/generated/` are outputs, not sources. Change the matching `contracts/.../schema.zt` or `gen_rust.zt`, then run the matching `scripts/generate/generate-*-bindings.py` / `just *_gen`. `components/bins/build.rs` separately generates build-time command and fabric profiles from `contracts/generation/v1/fixtures/valid.zti` into `OUT_DIR`.

### Navigation traps

- `kernel/src/lib.rs` re-exports architecture, driver, protocol, runtime, storage, and support modules, so `crate::<name>` callsites often refer to implementations in those subdirectories rather than the crate root.
- Kernel protocol modules in `kernel/src/protocol/` mostly adapt or re-export generated `slime-proto`; schema and generated bindings remain the source path for wire-layout changes.
- A component's capability slot layout is established by grants in `kernel/src/runtime/bootstrap.rs` and `contracts/generation/v1/fixtures/valid.zti`, not by the component binary alone. Inspect all three before changing slot numbers or authority.
- `scripts/check/` contains end-to-end QEMU assertions and expected serial markers; it is verification code, not the implementation of the behavior it checks.

## Commands

Use the Justfile targets from the repository root:

- `just run` — boot the current QEMU vertical slice.
- `just test` — run kernel and integration tests under QEMU.
- `just generation_check` — build and validate the deterministic generation binary.
- `just contracts_check` — validate generation manifest contracts.
- `just release_trust_check` — signed release staging, replay refusal, trust-root rotation continuity, rollback, and promotion.
- `just devlog_check` — validate devlog structure, front matter, and links.
- `just sel4_boot_layout_check` — init's resolved capability layout on every seL4 plane, against frozen fixtures (B10). Bless with `just sel4_boot_layout_bless`.
- `just sel4_qos_check` — C8.5's declared QoS policy on the `sel4-qos` plane: RELIABLE retry accounting and exhaustion, missed deadline, expired lifespan, lost liveliness lease, peer-dead retirement (P5.4.5).
- `just sel4_gate_control_check` — proves the seL4 plane gates fail when a required marker is missing, out of order, or a failure marker appears. The seL4 analogue of the oracle's `should_panic.rs`; needs no build or QEMU.
- `just fmt_check_all` — check Rust formatting for every workspace crate.
- `just lint_all` — run clippy with warnings denied for every workspace crate (kernel, components, stage0, boot-contracts).
- `just lint_pedantic` — advisory-only clippy pass (missing SAFETY comments, lossy casts); has known existing hits and is not a gate.
- `just deny` — dependency advisories, bans, licenses, and source pinning (deny.toml).
- `just machete` — unused-dependency scan of workspace crates.
- `just miri` — UB check of host-testable crates (boot-contracts, slime-proto).
- `just test_host` — host-side unit tests for boot-contracts and slime-proto.
- `just test_sel4_root` — `slime-root`'s 109 host unit tests, with the count asserted (B23). A gate of its own rather than a `test_host` arm: it compiles against the installed seL4 prefix, so it needs `just sel4_qemu_image_check` first and does not run in CI.
- `just ruff` — Python lint for `scripts/` (ruff.toml).
- `just typos` — spell-check sources and docs (_typos.toml).

## Backlog before roadmap

`roadmap/00-backlog.md` tracks known defects, regressions, and latent bugs in implemented code. Resolve open backlog items before starting a new `roadmap/` track milestone. A green verification suite is a precondition for milestone work, not a milestone itself; if you cannot resolve an open backlog item, record why it is deferred rather than silently skipping it. When you fix a defect, move its entry to the backlog's resolved log with the observed exit condition rather than deleting it.

## Development log

`devlog/` is the curated, chronological record of investigations, regressions, design decisions, and verification results. Record an entry whenever you complete a roadmap milestone or land a non-trivial feature, make a design or architecture decision, fix a non-trivial regression, root-cause a defect, or run a verification campaign.

Every entry is a folder `devlog/YYYY-MM-DD-short-topic/` holding a curated `index.md` written from `devlog/TEMPLATE.md`, with focused reports, raw transcripts, and other evidence as siblings in that folder — a folder even when there is no evidence yet, so later evidence never moves the entry. Front matter declares `Date`, `Kind` (`Defect`/`Change`/`Audit`/`Decision`, which selects the required sections), `Status`, `Scope`, `Roadmap`, `Gates`, `Trigger`, and `Baseline` in that order; `Roadmap` ids must resolve to real roadmap headings and `Gates` to real Justfile targets. Register the entry in `devlog/README.md` and follow its evidence rules — prefer exact `just` targets and observed results, label inherited evidence and unobserved conclusions, and never rewrite a raw log; corrections are appended under `## Corrections`, never edited into the frozen body. Run `just devlog_check` after touching `devlog/`. Roadmap completion stays authoritative in `roadmap/`; devlog entries explain how conclusions were reached. When a backlog item is resolved, link its devlog entry from the backlog's resolved log.

## Development rules

- **Zutai is the only schema language.** Every serialized format that crosses a persistence, process, or boot boundary — on-disk formats, IPC/protocol messages, manifests, handoff structures — must be defined as a versioned Zutai schema under `contracts/` (`schema.zt`), with Rust/Python bindings generated from it (`scripts/generate/generate-*-bindings.py`, `just *_gen`). Do not introduce hand-written field offsets, ad-hoc `#[repr(C)]` wire structs, `struct.pack` layouts, or any other schema language (JSON Schema, protobuf, etc.) as the source of truth for a format. Purely in-memory types are exempt.
- Prefer small, direct changes over new abstractions.
- Keep the kernel policy-free; component policy belongs in userspace.
- Preserve the capability/component/generation model. Do not add ambient authority, global executable paths, or implicit environment assumptions.
- Do not treat framebuffer output alone as milestone completion.
- Do not claim physical-machine support without an observed removable-media Framework boot that does not write internal NVMe.
- Keep generation data deterministic, versioned, bounded, and explicitly validated.

## Verification

- For kernel or userspace behavior changes, run the narrowest QEMU path that exercises the changed behavior.
- For generation-format or builder changes, run `just contracts_check` and `just generation_check`.
- For permanent Rust changes, run `just fmt_check_all` and `just lint_all` before finishing (or the narrower per-crate variants for scoped changes).
- Stage-0 denies `clippy::unwrap_used`, `clippy::expect_used`, `clippy::panic`, and `clippy::indexing_slicing` at the crate level; every fallible step there must return a `BootError`.
- For documentation-only changes, state that no runtime tests were run.
