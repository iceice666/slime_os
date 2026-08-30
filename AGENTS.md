# Slime OS Agent Guide

## Scope

These instructions apply to the entire repository.

## Project state

Slime OS is a QEMU-verified Rust `no_std` userspace graph on upstream seL4,
with `slime-root` owning dynamic mechanism and generated Zutai contracts owning
every persisted or cross-process format. Treat Framework laptop bring-up,
physical NVMe qualification, rollbackable production generations, and
daily-driver hardware support as unfinished unless code and tests prove otherwise.

## Code map: start here, do not broad-search

Route work by ownership before searching for a symbol. Read the named module root first; use LSP symbols/references from there when available, and only then grep the exact symbol. Do not scan `deps/`, `target/`, `devlog/`, or `roadmap/` for implementation symbols unless the task specifically concerns them.

### Execution path

1. `scripts/build/build-sel4.py` pins and builds seL4, the root task, its child fixture, and the loader image.
2. `slime-root/src/main.rs` admits the embedded generation, creates the initial capability graph, launches components, supervises faults, and owns bounded kernel-object allocation.
3. Components enter through `components/<lifecycle>/<component>/src/main.rs` — one crate per component (CP3), grouped under `system/`, `services/`, `applications/`, or `testkit/` — with shared helpers in `components/lib` and build-time support in `components/build-support`; their syscall surface is `components/runtime/src/syscall.rs`, with the native transport in `components/runtime/src/syscall/sel4_transport.rs` and authoritative operations in `slime-root/src/ipc.rs` plus the owning mechanism module.

### Task-to-file index

| Change | Canonical starting point | Follow-on files |
| --- | --- | --- |
| Capability kinds, rights, derivation, transfer | `slime-root/src/generation.rs` | `slime-root/src/{graph,ipc}.rs`, generation grants in `contracts/generation-manifest/v1/compositions/sel4-*.zti` |
| Native endpoint IPC, message bounds, endpoint lifetime | `slime-root/src/peer_endpoint.rs` | `slime-root/src/{ipc,notification}.rs`, `components/runtime/src/syscall/sel4_transport.rs` |
| Tasks, spawn, supervision, termination, reclamation | `slime-root/src/task.rs` | `slime-root/src/{main,child_vspace,fault,supervision}.rs` |
| Syscall argument validation and rights gates | `slime-root/src/ipc.rs` | owner modules in `slime-root/src/`, wrappers in `components/runtime/src/syscall.rs` |
| seL4 object allocation and VSpace construction | `slime-root/src/object_allocator.rs` | `slime-root/src/{child_vspace,buffer_adapter}.rs` |
| Shared-buffer allocation, mapping, loan, accounting | `slime-root/src/shared_buffer.rs` | `slime-root/src/{buffer_adapter,transfer_window,ipc}.rs` |
| Boot graph and component launch grants | `slime-root/src/main.rs` | generation decoding in `slime-root/src/generation.rs`, manifest fixtures below |
| Generation decoding and identity | `boot-contracts/src/generation.rs` | admission in `slime-root/src/generation.rs` |
| Generation construction and manifest contents | `scripts/build/build-generation.py` | `contracts/generation-manifest/v1/compositions/sel4-*.zti`, `components/build-support/src/lib.rs` |
| Component image format/loading | `contracts/component/v1/schema.zt` | generated `components/proto/src/component.rs`, decoder `boot-contracts/src/component_image.rs`, loader `slime-root/src/child_vspace.rs` |
| Userspace component behavior | `components/<lifecycle>/<component>/src/main.rs` | shared helpers in `components/lib/src/*.rs`; the crate's own `components/<lifecycle>/<component>/Cargo.toml` |
| Userspace syscall ABI | `components/runtime/src/syscall.rs` | seL4 transport in `components/runtime/src/syscall/sel4_transport.rs`, root implementation in `slime-root/src/ipc.rs` |
| IPC/service protocol semantics | `contracts/<protocol>/v1/schema.zt` | generated Rust in `components/proto/src/<protocol>.rs`; validators in `components/proto/src/lib.rs` |
| Boot/persistence contract decoder | `boot-contracts/src/<contract>.rs` | generated constants/layouts in `boot-contracts/src/generated/` |
| Fabric schemas, graph authority, stream framing | `contracts/interface-schema/v1/`, `contracts/fabric-graph/v1/`, `contracts/fabric-stream/v1/` | `boot-contracts/src/fabric_graph.rs`, `components/services/fabric-service/src/main.rs` |
| Block/storage transport and services | `components/services/virtio-blk-driver/src/main.rs` | ring adapter `components/lib/src/block_io.rs`, per-ring rights `contracts/block-authority/v1/schema.zt` and `boot-contracts/src/block_authority.rs`, raw device authority `slime-root/src/{device,io_resource}.rs`; qualification probes in `components/testkit/`. The boot selector's pre-admission reader is `slime-root/src/boot_selector_block.rs`, compiled only into `slime_boot_selector` images |
| Generation management, rollback, recovery | `components/services/sel4-generation-manager/src/main.rs` | matching rollback/recovery/transfer probes in `components/testkit/`, all reaching storage over the userspace driver's IO0 rings |
| Architecture, traps, interrupts, platform boot | `sel4/config/qemu-arm-virt.cmake` | `slime-root/src/{fault,platform_timer}.rs`, `scripts/build/build-sel4.py` |
| Host build/check orchestration | `Justfile` target | implementation in `scripts/{build,check,generate,lib}/` |
| Root behavioral regression | `slime-root/src/<module>.rs` tests | run `just test_sel4_root` and the matching `just sel4_*_check` |
| Protocol validation regression | `components/proto/tests/<protocol>.rs` | generated protocol module and schema |
| Adding a component | new `components/{system,services,applications,testkit}/<name>/` crate | the matching `Cargo.toml` workspace member glob plus its `[profile.release.package]` stanza, a `contracts/component-spec/v1/components/` record, and a generation-manifest entry; `just component_crate_split_check` gates the shape |

### Generated-code rule

Files beginning with `@generated` and files under `boot-contracts/src/generated/` are outputs, not sources. Change the matching `contracts/.../schema.zt` or `gen_rust.zt`, then run the matching `scripts/generate/generate-*-bindings.py` / `just *_gen`. `components/build-support` separately generates the build-time command tables from `contracts/generation-manifest/v1/fixtures/valid.zti` into each consuming crate's `OUT_DIR`, and copies the per-plane fabric profile the host builder renders.

### Navigation traps

- `slime-root/src/lib.rs` exposes the mechanism modules host tests compile; the product binary in `slime-root/src/main.rs` links those same modules.
- A component's capability slot layout is established by grants in the matching `contracts/generation-manifest/v1/compositions/sel4-*.zti` and generated boot-layout fixture, not by the component binary alone. Inspect all three before changing slot numbers or authority.
- `scripts/check/` contains end-to-end QEMU assertions and expected serial markers; it is verification code, not the implementation of the behavior it checks.

## Commands

Use the Justfile targets from the repository root:

- `just run` — boot the current seL4 QEMU product image.
- `just test` — run the root/product behavioral aggregate.
- `just generation_check` — build and validate the deterministic seL4 generation.
- `just contracts_check` — validate generation manifest contracts.
- `just sel4_root_boot_check` — root admission, allocator, timer, fault isolation, cleanup, and ready path.
- `just sel4_boot_layout_check` — init's resolved capability layout on every seL4 plane, against frozen fixtures (B10). Bless with `just sel4_boot_layout_bless`.
- `just sel4_qos_check` — C8.5's declared QoS policy on the `sel4-qos` plane.
- `just sel4_fault_check` — C8.14's degradation and fault-isolation envelope on the `sel4-fault` plane, whose interposition hop is compiled to die.
- `just sel4_fabric_aggregate_check` — C8.15's parent close: both aggregate schedules booted twice over one composition, with byte-identical semantic traces.
- `just sel4_gate_control_check` — prove every seL4 marker gate fails on missing, reordered, or explicit failure evidence.
- `just devlog_check` — validate devlog structure, front matter, gates, and links.
- `just fmt_check_all` — check Rust formatting for every surviving workspace crate.
- `just lint_all` — run clippy with warnings denied for components, boot-contracts, and seL4 product crates.
- `just deny` — dependency advisories, bans, licenses, and source pinning.
- `just machete` — unused-dependency scan of workspace crates.
- `just miri` — UB check of host-testable crates.
- `just test_host` — host-side unit tests for boot-contracts and slime-proto.
- `just test_sel4_root` — `slime-root`'s 211 host unit tests across 19 modules, with the count asserted (B23); requires the installed seL4 prefix.
- `just ruff` — Python lint for `scripts/`.
- `just typos` — spell-check sources and docs.

## Backlog before roadmap

`roadmap/00-backlog.md` tracks known defects, regressions, and latent bugs in implemented code. Resolve open backlog items before starting a new `roadmap/` track milestone. A green verification suite is a precondition for milestone work, not a milestone itself; if you cannot resolve an open backlog item, record why it is deferred rather than silently skipping it. When you fix a defect, move its entry to the backlog's resolved log rather than deleting it, and collapse it there to five lines: the `### B<N> — <title>` heading unchanged, a `**Status:**` line carrying the resolution date and class, one `**Was:**` sentence naming the observable wrong behavior, one `**Exit condition (observed):**` sentence in the past tense, and an `**Evidence:**` line linking the devlog entry. The resolved log records that a defect closed and where to read about it; the investigation, the evidence, and the fix narrative live in the devlog entry and are not duplicated here. Never renumber or reword an existing `B<N>` heading — devlog `Roadmap` fields and anchored links resolve against that exact text.

## Development log

`devlog/` is the curated, chronological record of investigations, regressions, design decisions, and verification results. Record an entry whenever you complete a roadmap milestone or land a non-trivial feature, make a design or architecture decision, fix a non-trivial regression, root-cause a defect, or run a verification campaign.

Every entry is a folder `devlog/YYYY-MM-DD-short-topic/` holding a curated `index.md` written from `devlog/TEMPLATE.md`, with focused reports, raw transcripts, and other evidence as siblings in that folder — a folder even when there is no evidence yet, so later evidence never moves the entry. Front matter declares `Date`, `Kind` (`Defect`/`Change`/`Audit`/`Decision`, which selects the required sections), `Status`, `Scope`, `Roadmap`, `Gates`, `Trigger`, and `Baseline` in that order; `Roadmap` ids must resolve to real roadmap headings and `Gates` to real Justfile targets. Register the entry in `devlog/README.md` and follow its evidence rules — prefer exact `just` targets and observed results, label inherited evidence and unobserved conclusions, and never rewrite a raw log; corrections are appended under `## Corrections`, never edited into the frozen body. Run `just devlog_check` after touching `devlog/`. Roadmap completion stays authoritative in `roadmap/`; devlog entries explain how conclusions were reached. When a backlog item or milestone closes, its devlog entry is the record of how it closed, and the `roadmap/` side keeps only the outcome plus a link to that entry. A resolved backlog item or completed milestone with no devlog link is incomplete: either write the entry or leave the full text in place and say why no entry exists.

## Development rules

- **Zutai is the only schema language.** Every serialized format that crosses a persistence, process, or boot boundary — on-disk formats, IPC/protocol messages, manifests, and boot records — must be defined as a versioned Zutai schema under `contracts/` (`schema.zt`), with Rust/Python bindings generated from it (`scripts/generate/generate-*-bindings.py`, `just *_gen`). Do not introduce hand-written field offsets, ad-hoc `#[repr(C)]` wire structs, `struct.pack` layouts, or any other schema language (JSON Schema, protobuf, etc.) as the source of truth for a format. Purely in-memory types are exempt.
- Prefer small, direct changes over new abstractions.
- Keep mechanism in `slime-root`; component policy belongs in userspace components.
- Preserve the capability/component/generation model. Do not add ambient authority, global executable paths, or implicit environment assumptions.
- Do not treat framebuffer output alone as milestone completion.
- Do not claim physical-machine support without an observed removable-media Framework boot that does not write internal NVMe.
- Keep generation data deterministic, versioned, bounded, and explicitly validated.

## Verification

- For root or userspace behavior changes, run the narrowest seL4 QEMU path that exercises the changed behavior.
- For generation-format or builder changes, run `just contracts_check` and `just generation_check`.
- For permanent Rust changes, run `just fmt_check_all` and `just lint_all` before finishing.
- For documentation-only changes, state that no runtime tests were run.
