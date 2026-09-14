# Slime OS

Slime OS is an experimental atomic personal operating system: a Rust `no_std` userspace graph on upstream seL4, not a Linux distribution. Its purpose is to explore capability-based isolation, component-oriented system services, explicit resource authority, and generation-based deployment.

The current product is a QEMU-verified `aarch64-sel4-qemu-virt` image: upstream seL4 16.0.0 owns scheduling, address spaces, memory objects, capability enforcement, IPC, interrupts, and timers, while `slime-root` owns the dynamic mechanism above them — generation admission, task construction and reclamation, bounded object allocation, shared buffers, native Endpoint IPC, and fault supervision. The custom microkernel that preceded it was retired with P5; there is no Slime kernel and no Slime trap vector.

The Milk-V Duo (`riscv64-sel4-milkv-duo`) has observed upstream-seL4 product-boot evidence. The named Framework 13 has a recorded removable-media CPU/product boot, not device qualification. Raspberry Pi 5 boot, physical NVMe, and daily-driver hardware remain unclaimed. See [target profiles and evidence limits](docs/architecture/targets-and-portability.md).

## Current status

What the system does today, and what it explicitly does not. This is a
capability summary, not a state record: `just tasks_list` prints every item's
state and `just tasks_next` prints what is actionable, from the canonical store
in `.tasks/items/`.

- The automated target is `aarch64-sel4-qemu-virt` under `qemu-system-aarch64 -machine virt,virtualization=on`. `just run` boots it; `just test` runs the product behavioral aggregate.
- The QEMU corpus covers bounded shared samples and typed fabric, clock/timer authority, wait sets, scheduling classes, lifecycle policy, task-private memory, and userspace virtio-blk/virtio-net drivers. [Subsystem architecture](docs/architecture/README.md) names current owners and limits; [component/system/image boundaries](docs/architecture/component-system-image.md) explains the spec-derived build path.
- The Milk-V Duo has observed physical evidence: it boots upstream seL4, `slime-root`, and a target-qualified generation from removable media through unmodified vendor firmware.
- The named Framework cold-booted its exact removable image twice on 2026-09-13 without internal-storage write authority. The retained observation is currently unbound from the rebuildable root image; it is not evidence of a fresh build or of any device support.
- **Unclaimed:** physical Framework storage — no seL4 NVMe transport exists, so `just storage_nvme_read_check` fails closed; Raspberry Pi 5 boot — missing serial evidence keeps `just rpi5_boot_check` closed; network application streams — the service already attaches LinkDevice and exchanges ARP/ICMP, but TCP/UDP payload transport and DNS remain unfinished. The linked network gate expects `tcp=0`, not a working byte stream.
- Unimplemented plans include the physical RPi5 ROS 2 demo, broader ROS 2 wire compatibility, Framework daily-driver devices, foreign workloads, distributed authority, and native development. [Plans](docs/plans/README.md) and the [retained-detail classification](roadmap/README.md) own requirements; MyQue alone owns their priority and state.
- All DMA on QEMU is trusted; no containment claim is made. QEMU evidence completes no physical milestone, and one board's evidence completes no other board's gate.

## Vision

Slime OS is designed around five first-class concepts:

- **Component:** an isolated, versioned executable unit with an address space and explicit dependencies.
- **Capability:** unforgeable authority to use a kernel object or service endpoint.
- **Channel:** IPC that can carry messages and transfer capabilities.
- **State:** persistent data with an owner, schema, and upgrade/rollback policy.
- **Generation:** a complete bootable graph of components, capability grants, state bindings, and immutable objects.

A generation is built and verified before it becomes bootable. Activation must not overwrite the running system in place. The previous known-good generation remains available, and a pending generation becomes known-good only after userspace health confirmation.

Atomicity therefore covers more than package files: it includes the boot selection, component graph, service endpoints, and declared persistent-state transitions.

## Architectural direction

Slime OS is not intended to become a small Unix clone with a different kernel implementation. Its native model is capability-based and component-oriented.

Privileged mechanism is seL4's, and it is not reimplemented:

- threads, scheduling, and address spaces;
- physical memory and memory objects;
- capability tables and object lifetime;
- Endpoint/Notification IPC and capability transfer;
- interrupts, timers, and minimal platform control.

`slime-root` owns the dynamic mechanism seL4 leaves to the initial task, and no policy: generation admission, task construction and reclamation, bounded kernel-object allocation, VSpace construction, shared buffers, and fault supervision.

Userspace services should own policy and most complex subsystems:

- component resolution and spawning;
- filesystems and persistent state;
- device management and most drivers;
- networking;
- display, input, and audio;
- generation construction, activation, health checking, and rollback.

New IPC protocols must be schema-first: message types are declared as versioned Zutai types under `contracts/`, and endpoint bindings are generated from or deterministically validated against those contracts. Root and component code must not introduce independent hand-written field offsets. This makes "tool call = channel" literal — an agent tool schema and a system IPC schema are the same artifact — and gives interposition tooling (auditing, recording, replay) typed messages instead of opaque bytes.

POSIX and Linux compatibility may exist later as userspace personalities or isolated virtual machines. They are compatibility facilities, not the native ABI or authority model.

## Reference targets

### Tier 0: automated product target

`aarch64-sel4-qemu-virt` is the default deterministic development and test platform. It exercises memory isolation, native Endpoint IPC, component lifecycle, generation boot, storage, and fault injection before equivalent paths are enabled on physical hardware. RV64 QEMU and x86-64 pc99 provide additional architecture-reference paths.

seL4 16.0.0 is pinned in `sel4/pins.toml` and configured by `sel4/config/qemu-arm-virt.cmake`. The machine is `qemu-system-aarch64 -machine virt,virtualization=on -cpu cortex-a53 -smp 1 -m 2048M`, with virtio block devices attached by the gates that need them.

The nine target identities are declared in `contracts/target-profile/v1/schema.zt`; `scripts/build/build-sel4.py` selects six build platforms. The [target-profile reference](docs/architecture/targets-and-portability.md) distinguishes current seL4 identities, retained artifact identities, the Raspberry Pi 5 build boundary, and physical evidence. Old executable encodings retain their original meaning; superseded generation formats are refused, not migrated.

### Tier 1: named physical targets

`riscv64-sel4-milkv-duo` is the RV64 physical lane, qualified by P3.D/P3.E/P3.F: the named Milk-V Duo boots upstream seL4, `slime-root`, and a target-qualified generation over a hands-off deployment loop, replays the architecture-neutral sample plane with byte-identical normalized traces, emits bounded fault evidence, recovers autonomously to vendor Linux, and serves Slisp as the resident shell. Gates: `just sel4_duo_image_check`, `just duo_payload_check`, and the board gates in `just/hardware.just`.


`aarch64-rpi5` remains the robotics demo's acceptance board. Its kernel, loader, and removable-media boot files build reproducibly (`just rpi5_media_check`), but no board boot was observed; `just rpi5_boot_check` fails closed on missing serial evidence. See the [RPi5 demo plan](docs/plans/rpi5-ros2-demo.md).

The Framework Laptop 13 has an exact `x86_64-sel4-framework13-ai300` profile and a recorded CPU/product boot. Device inventory and daily-driver qualification remain separate requirements in the [Framework hardware plan](docs/plans/framework-hardware.md). A CPU boot qualifies no device; the retained observation cannot be re-stamped to qualify a changed image.

Framework reference hardware:

| Area | Device |
| --- | --- |
| Machine | Framework Laptop 13, AMD Ryzen AI 300 Series, SKU `FRANVACP07` |
| CPU | AMD Ryzen AI 7 350, 8 cores / 16 threads |
| Memory | 32 GiB |
| GPU | AMD Radeon 860M, PCI `1002:1114` |
| Storage | WD_BLACK SN7100 1 TB NVMe, PCI `15b7:5045` |
| Wireless | MediaTek MT7925 / RZ717 Wi-Fi 7, PCI `14c3:0717` |
| Input | i8042 keyboard and PIXA3854 I2C touchpad |
| Audio | AMD HDA and ACP devices |
| Platform | x86-64 UEFI, ACPI, AMD IOMMU, xHCI, AMD-V |

No general PC compatibility is promised. Hardware that happens to share supported standards is best-effort until promoted explicitly.

### Physical-machine safety rule

Early Slime OS builds must boot from removable media and must not write to the internal NVMe device. Internal-disk writes remain disabled until the NVMe and storage stacks have deterministic tests for bounds, DMA isolation, timeout/reset, flush ordering, interrupted writes, and malformed metadata. Destructive storage development belongs on a dedicated external device.

## Language responsibilities

Slime OS does not need new configuration or shell languages. Two sibling projects already define those surfaces.

Both projects are pinned as Git submodules under `deps/`. Clone the complete source tree with:

```sh
git clone --recurse-submodules https://github.com/iceice666/slime_os.git
```

For an existing checkout, run `git submodule update --init --recursive`.

### Zutai: system configuration

[Zutai](deps/zutai) is the configuration evaluation language:

- `.zti` provides inert deterministic data;
- `.zt` provides pure, lazy, typed transformation and validation;
- records, unions, optionals, overlays, packages, and serialization provide the configuration vocabulary.

The configuration path is intentionally separated from activation:

```text
Zutai source and hardware data
    -> pure evaluation and normalization
    -> versioned Slime build request
    -> component/object resolution
    -> immutable generation manifest
    -> staged activation
```

Production system evaluation must not receive authority to modify the boot partition, switch generations, format storage, or grant capabilities. Zutai describes intent; the Slime builder validates and executes it transactionally.

Zutai host capabilities are language-level declarations and are currently advisory. They are not seL4 capabilities. A Zutai runtime on Slime may hold opaque handles backed by real service capabilities, but only seL4 and trusted services enforce and transfer authority.

The build pipeline runs Zutai on the development host. Porting the compiler/runtime into Slime userspace is deferred behind the demo path.

### Slisp: native interactive language

Slisp is the resident native shell and application language. Its syntax is only
S-expressions: integers, symbols, lists, lexical functions, `let`, `if`, `do`,
`quote`, and persistent top-level `define`. The freestanding C implementation
proves component development is not tied to Rust; it is built as an external
AArch64 ELF, hash-admitted through the ordinary component specification path,
and launched with only its generation-declared input and service endpoints.

Shell operations will be ordinary capability-bearing functions. Process spawn,
streams, filesystem access, diagnostics, and termination remain explicit Slime
service contracts rather than reader punctuation or ambient POSIX state.

### Native application compilation

The native-development roadmap extends Slisp from its bounded interpreter to a
compiler that emits the exact target-qualified Slime component-image format.
Zutai remains the only schema/configuration language; Slisp may consume generated
bindings but cannot define a second cross-boundary format or grant authority.

## Component and generation boundary

The stable cross-project artifact is the versioned, deterministic generation manifest. Its host-side source schema is `contracts/generation-manifest/v1/schema.zt`; the built wire format is v5, defined by `contracts/generation/v5/schema.zt` and decoded by `boot-contracts/src/generation.rs`. The decoder refuses v2, v3, and v4 as unsupported; rollback safety comes from refusing an undecodable candidate, not migrating it. Its logical content is:

```text
GenerationManifest
  format version
  target identity
  kernel and bootstrap objects
  immutable component objects
  initial component dependency graph
  initial capability grants
  persistent-state bindings and policies
  health-check policy
  parent/rollback metadata
  integrity hashes
```

Neither seL4 nor `slime-root` parses Zutai source or owns system policy. `slime-root` admits the embedded generation, creates the initial capability graph, and launches the declared components; policy lives in those components.

On `aarch64-sel4-qemu-virt` a generation still declares exactly one `kernelObject` because the format requires it and the root re-checks that closure at admission, but nothing maps it: seL4 is the kernel, pinned and built separately under `sel4/pins.toml`.

Current executable payloads use `contracts/component/v2/`: a bounded exact-target qualification header followed by either segments or a complete native ELF (the seL4 product revision). Retained v1 images imply `x86_64-qemu-virtio`. Integrity comes from the generation object digest and authority from generation grants; the image carries neither independently. See [component and image boundaries](docs/architecture/component-system-image.md).

## Agentic direction

The five first-class concepts are also the natural primitives for running autonomous agents safely, and no new authority model is required for it.

- **Agent = Component.** An agent is an isolated component with an address space and explicit dependencies. Agent fault containment is component fault containment: a crashing agent does not terminate its peers, the services it uses, or the system.
- **Tool call = Channel.** A tool invocation is a typed IPC message to a service endpoint, not an arbitrary function call. The endpoint's schema defines the message; capability transfer along the channel is the only way authority crosses the boundary.
- **Agent authority = Capability grant.** Spawn supplies no implicit environment, working directory, streams, or other authority. An agent receives only the grants declared by the generation, and unforgeable capabilities mean authority cannot be ambient, guessed, or widened at runtime.
- **Agent memory = State binding.** Long-term agent state is a `StateBinding` with an owner, schema version, and policy. `snapshotBeforeUpgrade` and `discardOnRollback` give agent memory the same upgrade and rollback discipline as the rest of the system.
- **Agent update = Generation.** Changing an agent's model, prompt, or tool set produces a new generation. Health checking applies to agent behavior as well as to boot: a pending generation becomes known-good only after userspace confirmation, and a regressing agent rolls back with the same mechanism as a regressing system component.

Privileged mechanism does not treat agents or language models as special. A language model is a userspace service component that agents address over endpoints; the scheduler, context, and memory concerns of agent runtimes live in userspace services. This keeps mechanism policy-free for agents as it is for every other subsystem, and lets model choice, provider, and placement change as a generation without touching the component ABI.

External agent protocols such as MCP may be bridged by a dedicated component that exposes protocol servers as Slime capability endpoints. The bridge cannot grant authority the agent does not already hold, so prompt injection success at the model layer is still bounded by the generation's declared grants.

Because no component holds ambient authority, every capability can be transparently interposed by a user-chosen proxy component (a membrane). This enables agent dry-runs: an agent can be executed against virtualized capabilities to preview the effects it *would* have, before any real authority is granted. Capability transfers can also record provenance, so the system can answer "why is this component allowed to do X" as an explicit grant chain rooted in the generation manifest.

Atomicity and agentic operation reinforce each other: agent memory and authority are versioned, verified, and rollbackable by the same mechanisms as the boot graph, and the boot graph can include agent components without a separate agent deployment track.

## Differentiating directions

Exploratory directions are registered in [`docs/directions/`](docs/directions/README.md). They are not committed work: promotion requires a canonical work item with an observable exit condition and design context in the owning [`docs/plans/`](docs/plans/README.md) page.

## First vertical slice (complete)

The current product vertical slice connects Slime OS, Zutai, and Slisp:

```text
Zutai generation configuration
    -> target-qualified Rust service ELFs
    -> externally built freestanding C Slisp ELF
    -> hash-checked component admission
    -> isolated init, console, spawn service, and resident Slisp REPL
```

`just sel4_component_graph_check` boots six target-qualified ELF payloads,
observes the four required instances live, and stops after Slisp reaches its
prompt and reports its first blocked input read. `just slisp_core_check`
separately drives persistent definitions, lexical evaluation, typed refusal,
and clean termination through the same non-Rust implementation.

This slice defines the minimum useful contracts: userspace entry, address-space isolation, capability IPC, executable identity, command resolution, spawning, streams, termination notification, manifest decoding, fault containment, and the agent abstraction as a non-special case of the above.

## Plan and work items

See [CONTRIBUTING.md](CONTRIBUTING.md) for GitHub intake, planning-before-implementation,
PR linking, and evidence-based completion.

Work-item identity, state, hierarchy, dependencies, and observed closure evidence live in `.tasks/items/`, one Markdown file per item under a canonical UUID, managed by [MyQue](https://github.com/mozufu/myque). Human keys such as `C9.4`, `IO4`, and `B92` are display aliases: optional, mutable, and never resolved through by a checker or dependency edge.

```sh
just tasks_list    # every item with its key, kind, and state
just tasks_next    # what is actionable right now
just tasks_graph   # the dependency graph
just tasks_check   # validate the store and the repository's ordering policy
```

Current ownership, invariants, and limitations live in
[`docs/architecture/`](docs/architecture/README.md). Unimplemented designs and
qualification requirements live in [`docs/plans/`](docs/plans/README.md);
long-lived choices in [`docs/decisions/`](docs/decisions/README.md) retain their
accepted or proposed status. [`docs/`](docs/README.md) is the entry point.

The [roadmap classification](roadmap/README.md) identifies each retained source,
its extracted owner, and detailed requirements not yet extracted. Historical
chronology and devlogs remain readable until verified archival. Open backlog
items precede milestone work under `just tasks_check`; this page does not carry
a second priority or completion table.

## Current repository layout

```text
Cargo.toml       Root Rust workspace and shared build profiles
sel4/            Upstream seL4 pins (`pins.toml`) and per-platform CMake configuration
slime-root/      The seL4 root task: generation admission, tasks, allocation, shared buffers, IPC, supervision
components/      Rust no_std userspace components, one crate per component under system/, services/, applications/, and testkit/, plus the syscall runtime (`runtime`), shared helpers (`lib`), and generated protocols (`proto`)
boot-contracts/  Shared Rust boot, generation, storage, recovery, and admission contract decoders
contracts/       Versioned Zutai schemas for every persisted, IPC, and boot format, plus generation fixtures
scripts/         Host tooling grouped as build/, check/, generate/, and lib/
tools/           Developer-facing helpers such as LLDB attachment
docs/            Current architecture, operation, references, decisions, plans, and exploration
roadmap/         Classified historical sources and explicitly unextracted requirements pending verified archival
devlog/          Retained historical investigations and evidence pending the verified archive cutover; not required for ordinary changes
assets/          Boot/runtime assets
deps/            Pinned seL4, rust-sel4, and Zutai submodules
.tasks/          Canonical work-item store (`items/`) and the pre-migration roadmap-id map
Justfile         Build, run, test, format, lint, generation, contract, and debug commands
```

Common development commands (see `just --list` for the full gate set):

```sh
just run                    # boot the seL4 product image on the pinned QEMU machine
just test                   # product behavioral aggregate
just test_sel4_root         # slime-root's host unit tests, count asserted
just contracts_check        # validate every Zutai contract and generated binding
just fmt_check_all
just lint_all
```

## Non-goals for the initial system

- supporting arbitrary PCs;
- reproducing Linux, FHS, systemd, UID/GID, `fork`, signals, or ambient path authority as native primitives;
- writing a desktop environment before isolation and service recovery work;
- running existing Linux binaries directly;
- inventing another configuration language or shell;
- embedding a language model, agent runtime, or agent scheduler in privileged mechanism;
- granting agents authority to switch generations, format storage, or grant capabilities;
- running agents outside the capability and generation model;
- treating a framebuffer demo as completion of an OS architecture milestone.

Linux remains useful as the development host and may later run as an isolated guest for compatibility. It does not define Slime OS's kernel, native ABI, authority model, or deployment architecture.
