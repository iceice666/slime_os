# Slime OS Agent Guide

## Scope

These instructions apply to the entire repository.

## Project state

Slime OS is a Rust `no_std` userspace graph on upstream seL4, with `slime-root`
owning dynamic mechanism and generated Zutai contracts owning every persisted
or cross-process format. It is QEMU-verified across every plane, and since
2026-09-13 one named physical machine boots it from removable media: a
Framework 13 (AMD Ryzen AI 300, x86-64, P6), without writing internal storage.
That is a CPU and product boot path only: treat physical NVMe qualification,
device support of every kind, rollbackable production generations, and
daily-driver hardware support as unfinished unless code and tests prove
otherwise. A CPU boot qualifies no device.

## Code map: start here, do not broad-search

Route work by ownership before searching for a symbol. Read the named module root first; use LSP symbols/references from there when available, and only then grep the exact symbol. Do not scan `deps/`, `target/`, `roadmap/`, or `.tasks/` for implementation symbols unless the task specifically concerns them.

For current subsystem rationale, start at [`docs/architecture/`](docs/architecture/README.md), then the owning code or contract below. Unimplemented design and qualification boundaries live in [`docs/plans/`](docs/plans/README.md). [`roadmap/README.md`](roadmap/README.md) classifies retained detail and historical source; do not reconstruct extracted architecture from milestone chronology.

### Execution path

1. `scripts/build/build-sel4.py` pins and builds seL4, the root task, its child fixture, and the loader image.
2. `slime-root/src/main.rs` admits the embedded generation, creates the initial capability graph, launches components, supervises faults, and owns bounded kernel-object allocation.
3. Components enter through `components/<lifecycle>/<component>/src/main.rs` — one crate per component (CP3), grouped under `system/`, `services/`, `applications/`, or `testkit/` — with shared helpers in `components/lib` and build-time support in `components/build-support`; their syscall surface is `components/runtime/src/syscall.rs`, with the native transport in `components/runtime/src/syscall/sel4_transport.rs` and authoritative operations in `slime-root/src/ipc.rs` plus the owning mechanism module.

### Task-to-file index

| Change | Canonical starting point | Follow-on files |
| --- | --- | --- |
| Capability kinds, rights, derivation, transfer | `slime-root/src/generation.rs` | `slime-root/src/{graph,ipc}.rs`; grants are declared in the system spec below |
| Native endpoint IPC, message bounds, endpoint lifetime | `slime-root/src/peer_endpoint.rs` | `slime-root/src/{ipc,notification}.rs`, `components/runtime/src/syscall/sel4_transport.rs` |
| Tasks, spawn, supervision, termination, reclamation | `slime-root/src/task.rs` | `slime-root/src/{main,child_vspace,fault,supervision}.rs` |
| Syscall argument validation and rights gates | `slime-root/src/ipc.rs` | owner modules in `slime-root/src/`, wrappers in `components/runtime/src/syscall.rs` |
| seL4 object allocation and VSpace construction | `slime-root/src/object_allocator.rs` | `slime-root/src/{child_vspace,buffer_adapter}.rs` |
| Shared-buffer allocation, mapping, loan, accounting | `slime-root/src/shared_buffer.rs` | `slime-root/src/{buffer_adapter,transfer_window,ipc}.rs` |
| Boot graph and component launch grants | `slime-root/src/main.rs` | generation decoding in `slime-root/src/generation.rs`, system specs below |
| Generation decoding and identity | `boot-contracts/src/generation.rs` | admission in `slime-root/src/generation.rs` |
| Composition: which components run, grants, slots, budgets, health | `contracts/system-spec/v1/systems/<name>.zti` | derivation map `DERIVED_GENERATION_FIXTURES` in `scripts/lib/system_spec.py`; regenerate with `python3 scripts/generate/generate-generation-from-spec.py`; `just system_spec_check` refuses drift. `contracts/generation-manifest/v1/{compositions,fixtures}/*.zti` are derived outputs |
| Generation construction and manifest encoding | `scripts/build/build-generation.py` | `contracts/generation-manifest/v1/schema.zt`, `components/build-support/src/lib.rs` |
| System-image closure: the build key every `sel4_*` gate resolves before booting | `scripts/generate/generate-system-image-closures.py` | `contracts/system-image-closure/v2/{closures,negative}/*.zti` are derived outputs recording a tree digest of `slime-root`, `boot-contracts`, `components/{lib,proto,runtime,build-support}`, each closed-over component crate, `just`, `Cargo.{toml,lock}`, `contracts/{component-spec,interface-schema,system-spec}`, and `deps/rust-sel4`. After editing any of those, rerun the generator and commit the closures with the change, or every plane gate refuses with `closure does not resolve: … identity mismatch`. Resolver: `scripts/lib/system_image_closure.py`; drift gate: `just system_image_builder_check` |
| Component image format/loading | `contracts/component/v2/schema.zt` | generated `components/proto/src/component.rs`, decoder `boot-contracts/src/component_image.rs`, loader `slime-root/src/child_vspace.rs`; v1 is retained format history |
| Userspace component behavior | `components/<lifecycle>/<component>/src/main.rs` | shared helpers in `components/lib/src/*.rs`; the crate's own `components/<lifecycle>/<component>/Cargo.toml` |
| Userspace syscall ABI | `components/runtime/src/syscall.rs` | seL4 transport in `components/runtime/src/syscall/sel4_transport.rs`, root implementation in `slime-root/src/ipc.rs` |
| IPC/service protocol semantics | `contracts/<protocol>/v1/schema.zt` | generated Rust in `components/proto/src/<protocol>.rs`; validators in `components/proto/src/lib.rs` |
| Boot/persistence contract decoder | `boot-contracts/src/<contract>.rs` | generated constants/layouts in `boot-contracts/src/generated/` |
| Fabric schemas, graph authority, stream framing | `contracts/interface-schema/v1/`, `contracts/fabric-graph/v1/`, `contracts/fabric-stream/v1/` | `boot-contracts/src/fabric_graph.rs`, `components/services/fabric-service/src/main.rs` |
| Block/storage transport and services | `components/services/virtio-blk-driver/src/main.rs` | ring adapter `components/lib/src/block_io.rs`, per-ring rights `contracts/block-authority/v1/schema.zt` and `boot-contracts/src/block_authority.rs`, raw device authority `slime-root/src/{device,io_resource}.rs`; qualification probes in `components/testkit/`. The boot selector's pre-admission reader is `slime-root/src/boot_selector_block.rs`, compiled only into `slime_boot_selector` images |
| Generation management, rollback, recovery | `components/services/sel4-generation-manager/src/main.rs` | matching rollback/recovery/transfer probes in `components/testkit/`, all reaching storage over the userspace driver's IO0 rings |
| Architecture, traps, interrupts, platform boot | `sel4/config/qemu-arm-virt.cmake` | `slime-root/src/{fault,platform_timer}.rs`, `scripts/build/build-sel4.py` |
| Host build/check orchestration | `Justfile` target | implementation in `scripts/{build,check,generate,lib}/` |
| Work-item store policy, spec-driven bodies, terminal records | `scripts/check/check-work-items.py` | shared reader `scripts/lib/work_items.py`, toolchain adapter `scripts/lib/devloop.py`, gate adapter `scripts/check/devloop-gate.py`, approval `scripts/check/devloop-approval.py`, policy [`.devloop/policy.json`](.devloop/policy.json), pins in `flake.nix` and `nix/devloop.nix` |
| Root behavioral regression | `slime-root/src/<module>.rs` tests | run `just test_sel4_root` and the matching `just sel4_*_check` |
| Protocol validation regression | `components/proto/tests/<protocol>.rs` | generated protocol module and schema |
| Adding a component | new `components/{system,services,applications,testkit}/<name>/` crate | the matching `Cargo.toml` workspace member glob plus its `[profile.release.package]` stanza, a `contracts/component-spec/v1/components/` record, and a `contracts/system-spec/v1/systems/` entry with its regenerated manifest; `just component_crate_split_check` gates the shape. [`docs/getting-started/05-add-a-component.md`](docs/getting-started/05-add-a-component.md) walks it end to end |

### Generated-code rule

Files beginning with `@generated`, files under `boot-contracts/src/generated/`, every manifest named in `DERIVED_GENERATION_FIXTURES` (`scripts/lib/system_spec.py`), and every closure under `contracts/system-image-closure/v2/` are outputs, not sources. Change the matching `contracts/.../schema.zt`, `gen_rust.zt`, or `contracts/system-spec/v1/systems/*.zti`, then run the matching `scripts/generate/generate-*-bindings.py` / `just *_gen` / `generate-generation-from-spec.py` / `generate-system-image-closures.py`. `components/build-support` separately generates the build-time command tables from the derived `contracts/generation-manifest/v1/fixtures/valid.zti` into each consuming crate's `OUT_DIR`, and copies the per-plane fabric profile the host builder renders.

### Navigation traps

- `slime-root/src/lib.rs` exposes the mechanism modules host tests compile; the product binary in `slime-root/src/main.rs` links those same modules.
- A component's capability slot layout is established by grants in the matching `contracts/system-spec/v1/systems/<plane>.zti`, the manifest derived from it under `contracts/generation-manifest/v1/compositions/`, and the generated boot-layout fixture, not by the component binary alone. Inspect all three before changing slot numbers or authority; edit only the system spec.
- `scripts/check/` contains end-to-end QEMU assertions and expected serial markers; it is verification code, not the implementation of the behavior it checks.

## Commands

Use the Justfile targets from the repository root:

- `just run` — boot the current seL4 QEMU product image.
- `just test` — run the root/product behavioral aggregate.
- `just generation_check` — build and validate the deterministic seL4 generation.
- `just contracts_check` — validate generation manifest contracts.
- `just system_spec_check` — validate system specs and refuse a derived composition manifest that drifted from its spec.
- `just sel4_root_boot_check` — root admission, allocator, timer, fault isolation, cleanup, and ready path.
- `just sel4_boot_layout_check` — init's resolved capability layout on every seL4 plane, against frozen fixtures (B10). Bless with `just sel4_boot_layout_bless`.
- `just sel4_qos_check` — C8.5's declared QoS policy on the `sel4-qos` plane.
- `just sel4_fault_check` — C8.14's degradation and fault-isolation envelope on the `sel4-fault` plane, whose interposition hop is compiled to die.
- `just sel4_fabric_aggregate_check` — both aggregate schedules booted twice over one composition; compares per-participant semantic fields while exempting arrival ordinals and designated poll-sampled high-water counters, not byte-identical serial traces.
- `just sel4_gate_control_check` — prove every seL4 marker gate fails on missing, reordered, or explicit failure evidence.
- `just docs_check` — validate maintained local links/fragments, canonical UUID references, and current command references without network or an archive checkout.
- `just tasks_check` — real `myque check` plus backlog-first policy, terminal-record readability, devloop validation of every spec-driven body, and negative controls; offline, with no historical index dependency.
- `just tasks_list` / `just tasks_next` / `just tasks_graph` — the work-item store's generated views. Never authoritative; `.tasks/` is.
- `just devloop <args>` — the pinned devloop, run against this repository's `deps/zutai` build and [`.devloop/policy.json`](.devloop/policy.json): `admit`, `validate`, `start`, `gate`, `human`, `eligible`, `complete`, `render`, all by canonical UUID.
- `just x86_64_sel4_image_check` — P6.1's x86-64 admission: exact target profiles, pinned pc99 kernel/toolchain inputs, and a byte-identical rebuild, with no boot claim.
- `just x86_64_sel4_root_boot_check` — P6.3's root, component runtime, child loader, fault, thread-context, and timer markers on pc99.
- `just x86_64_qemu_check` — P6.4's corpus on pc99: root boot, wait-set, sample, product graph, boot layouts, and portability.
- `just framework_media_check` — P6.5's deterministic raw GPT/FAT32 image: byte-identical rebuild, malformed/drifted/unsafe-target refusals, and a boot of the exact raw bytes under pinned q35/OVMF.
- `just framework_cpu_boot_check` — P6.6's physical claim. Proves the observation validator refuses forged evidence, then reports the recorded boot from `evidence/framework-cpu-boot/`. It cannot manufacture an observation: `framework_cpu_boot_prepare` writes the image and hashes the protected internal region, the cold boots are an operator's power cycle, and `check-framework-cpu-boot.py verify` re-hashes and judges. QEMU cannot close it.
- `just fmt_check_all` — check Rust formatting for every surviving workspace crate.
- `just lint_all` — run clippy with warnings denied for components, boot-contracts, and seL4 product crates.
- `just deny` — dependency advisories, bans, licenses, and source pinning.
- `just machete` — unused-dependency scan of workspace crates.
- `just miri` — UB check of host-testable crates.
- `just test_host` — host-side unit tests for boot-contracts and slime-proto.
- `just test_sel4_root` — `slime-root`'s host unit tests, with the count asserted in `just/quality.just` (B23); raise it deliberately when adding tests. Requires the installed seL4 prefix.
- `just ruff` — Python lint for `scripts/`.
- `just typos` — spell-check sources and docs.

## Work items, backlog, and roadmap

`.tasks/` is the canonical record of work-item identity, state, hierarchy, and dependencies, managed by [MyQue](https://github.com/mozufu/myque) at the `work-item/v2` envelope. Active work is `.tasks/items/<UUID>.md`; a done or cancelled item may be *retired* to a minimal `.tasks/terminal/<UUID>.json` record after its exact final bytes are retained in Git, and its identity, relationships, and readiness still resolve offline from that record. Canonical identity is the UUID; a cancelled, unknown, or corrupt identity is never done. Human keys such as `C9.4`, `IO4`, and `B92` are display aliases: they are optional, they may change, and no checker, devlog reference, dependency edge, or generated view may resolve through them. Do not allocate an id by scanning for the next number, and do not create a persistent relationship using a human key — use `myque` to create and mutate items, which allocates a UUIDv7 locally and writes relationships as UUIDs.

An item's frontmatter is an open record: MyQue owns an enumerated key set and preserves every other top-level namespace verbatim. `devloop` is this repository's only consumer namespace. An item that carries it is *spec-driven*: its body after the title is exactly one fenced `zti` [`dev-spec/v1`](https://github.com/mozufu/devloop) payload under devloop's `fenced-zti/v1` profile, and its record carries the profile, pinned schema and helper identities, requirements digest, admission identity, and evidence. `just tasks_check` validates those bodies through devloop — which owns that format — while `myque check` still owns store validity. Admit, start, gate, and complete such work with `just devloop`; gate identities and their declared typed observations live in [`.devloop/policy.json`](.devloop/policy.json), never in requirement text. Ordinary prose items, including every legacy `work-item/v1` item, remain valid and are not subject to any of this. Each devloop validation compiles and links a native binary, so `just tasks_check` costs seconds once an item is spec-driven; a direct `myque close` without `--expected` bypasses devloop's eligibility boundary by design. [`docs/getting-started/06-work-item-lifecycle.md`](docs/getting-started/06-work-item-lifecycle.md) walks both lifecycles command by command, and [`docs/decisions/spec-driven-work-item-bodies.md`](docs/decisions/spec-driven-work-item-bodies.md) records what this repository owns and what it defers.

Backlog defects are the items tagged `backlog`. Resolve, defer, or block every open one before starting a new track milestone; a green verification suite is a precondition for milestone work, not a milestone itself. `deferred` means postponed by decision and `blocked` means waiting on something outside this repository — both satisfy the rule, `open` and `active` do not. `just tasks_check` enforces that ordering and validates the store, and `just tasks_next` lists what is actionable. Create an item with `myque new "<title>" --kind bug --tag backlog`, which allocates the UUID; close it with `myque close`, which records the closure date, and record the exit condition that was *observed* in the item's body — a milestone closes on observed evidence, never on implementation status alone.

Non-trivial work implements a canonical item that is already on `main`; land a work-item-only planning PR first, then the implementation PR. Never use GitHub auto-close keywords (`Closes`, `Fixes`, `Resolves`) against projected Issues, and never hand-edit `myque:*` markers or the `myque-gh` PR trailer. Only a genuine typo, broken link, format-only change, or mechanical maintenance with no behavioral or project-state consequence skips the landed-item requirement; judge semantics, not line count. [`CONTRIBUTING.md`](CONTRIBUTING.md) owns the full PR workflow.

The old frozen backlog index is archived, not a live obligation. New and reopened defects belong only to the canonical store. Historical evidence uses the repository identity, full commit, and original path described in [`docs/history.md`](docs/history.md); archived task snapshots never participate in current validation or projection.

`roadmap/` retains explicitly identified unextracted requirements and their surrounding historical context. Its classification index routes extracted subjects to `docs/architecture/` and `docs/plans/`; update those owners rather than maintaining parallel milestone prose. State, hierarchy, and dependencies belong only to the store.

## Change records and historical investigations

Ordinary features, bug fixes, refactors, and milestone completions do not require a separate devlog entry. The commit and PR record the change, claim, risk, review surface, exact verification, and known limits; the canonical work item records scope, state, and the exit conditions actually observed. Distinguish direct observations from inherited evidence and unobserved inference, and bind hardware or image claims to the target, revision, and binary identity that was tested.

Update the owning current documentation when a contract, operating procedure, or limitation changes. Put long-lived cross-module choices in [`docs/decisions/`](docs/decisions/README.md) when they need a durable record; put unimplemented designs and qualification requirements in [`docs/plans/`](docs/plans/README.md); keep exploratory directions in [`docs/directions/`](docs/directions/README.md); keep local invariants beside the implementation. A decision record states context, the decision, alternatives and trade-offs, consequences, revisit conditions, status (`proposed`, `accepted`, or `superseded`), and relevant work-item UUIDs and code references. Moving an old proposal never makes it accepted. The accepted ownership split is recorded in [`docs/decisions/development-record-ownership.md`](docs/decisions/development-record-ownership.md).

Historical investigations and raw evidence live in the private [`slime_os-history` archive](docs/history.md). Preserve observed results and bytes; new exceptional investigations belong there, not in the product tree. Ordinary changes require neither an investigation nor a complete transcript. Product checks validate current local knowledge, not archived entries against today's recipes or tasks.

## Documentation ownership

A comment should state what must remain true, not argue that the author was right.

Keep implementation comments for correctness or safety invariants not evident
from types or code, ownership and ordering requirements, ABI constraints,
capability or security boundaries, platform constraints, and why an apparently
simpler local implementation would violate a current invariant. Do not put
investigation history, failed approaches, reviewer findings, mutation campaigns,
historical alternatives, speculative designs, verification results, or narration
of the code in implementation comments.

Place local invariants beside the implementation and stable subsystem rationale
in the owning `docs/` or contract documentation. Prefer one to three precise
sentences over defensive paragraphs. Historical investigation records explain
how an older conclusion was reached and are referenced rather than duplicated.

## Development rules

- **Zutai is the only schema language.** Every serialized format that crosses a persistence, process, or boot boundary — on-disk formats, IPC/protocol messages, manifests, and boot records — must be defined as a versioned Zutai schema under `contracts/` (`schema.zt`), with Rust/Python bindings generated from it (`scripts/generate/generate-*-bindings.py`, `just *_gen`). Do not introduce hand-written field offsets, ad-hoc `#[repr(C)]` wire structs, `struct.pack` layouts, or any other schema language (JSON Schema, protobuf, etc.) as the source of truth for a format. Purely in-memory types are exempt, as are readers and writers of externally specified formats (ELF, GPT, Ethernet/IP, MAVLink, SSH signatures) that Slime OS does not own; those may use `struct` with the standard's offsets.
- Prefer small, direct changes over new abstractions.
- Keep mechanism in `slime-root`; component policy belongs in userspace components.
- Preserve the capability/component/generation model. Do not add ambient authority, global executable paths, or implicit environment assumptions.
- Do not treat framebuffer output alone as milestone completion.
- Do not claim physical-machine support without an observed removable-media Framework boot that does not write internal NVMe.
- Keep generation data deterministic, versioned, bounded, and explicitly validated.

## Verification code discipline

Do not create a new `scripts/check/check-*.py` merely because a change adds an
invariant or roadmap gate. Identify the verification mechanism that owns the
invariant, extend its checker when one exists, and add a case or module when
execution and evidence collection are shared. Add a top-level checker only for a
genuinely new mechanism or independently reusable boundary, such as a different
execution environment, distinct input/output protocol, stable subsystem boundary
with multiple checks, independent reuse, or materially different semantics.

Roadmap items, backlog IDs, compositions, individual regressions, and individual
QEMU planes do not by themselves justify top-level checker executables. Public
`just` targets remain narrow and descriptive; several targets may invoke one
checker with different cases.

For seL4 planes, the shared mechanism lives in `scripts/lib/sel4_plane.py`
(QEMU invocation, transcript collection, common failures, control and mutation
machinery) and `scripts/lib/sel4_gate_markers.py` (marker chains and ordering).
Each `scripts/check/check-sel4-*-plane.py` owns only its concrete expectations
for boot, lifecycle, fabric, IO, storage, and similar domains. Move repeated
QEMU or transcript handling into those libraries rather than copying it into a
new plane script. Extract only observed repetition; do not build a generic
class-heavy test framework, and keep existing public gates.

## Verification

- For root or userspace behavior changes, regenerate the system-image closures (`python3 scripts/generate/generate-system-image-closures.py`), then run the narrowest seL4 QEMU path that exercises the changed behavior. The generator refuses a tree holding gitignored files; a closure digest is a raw filesystem walk, so ignored build output would record bytes no clone reproduces.
- For generation-format or builder changes, run `just contracts_check` and `just generation_check`.
- For permanent Rust changes, run `just fmt_check_all` and `just lint_all` before finishing.
- For documentation-only changes, state that no runtime tests were run.
