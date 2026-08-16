# Slime OS

Slime OS is an experimental atomic personal operating system: a Rust `no_std` userspace graph on upstream seL4, not a Linux distribution. Its purpose is to explore capability-based isolation, component-oriented system services, explicit resource authority, and generation-based deployment.

The current product is a QEMU-verified `aarch64-sel4-qemu-virt` image: upstream seL4 16.0.0 owns scheduling, address spaces, memory objects, capability enforcement, IPC, interrupts, and timers, while `slime-root` owns the dynamic mechanism above them — generation admission, task construction and reclamation, bounded object allocation, shared buffers, native Endpoint IPC, and fault supervision. The custom microkernel that preceded it was retired with P5; there is no Slime kernel and no Slime trap vector.

The near-term product goal is a concrete robotics demonstration: boot Slime OS on a Raspberry Pi 5 and run two local ROS 2 nodes exchanging bounded topic data through a minimal DDSI-RTPS/XCDR profile. Physical Raspberry Pi 5 qualification, Framework laptop bring-up, physical NVMe, and daily-driver hardware support are all open.

## Current status

- The automated target is `aarch64-sel4-qemu-virt` under `qemu-system-aarch64 -machine virt,virtualization=on`. `just run` boots it; `just test` runs the product behavioral aggregate.
- M1–M4 and M6 are complete, and M5 is complete except M5.7: no seL4 NVMe transport or physical Framework storage evidence exists, so `just storage_nvme_read_check` fails closed rather than reporting a false pass.
- Core runtime C7 and C8.1–C8.12 are complete under named QEMU gates. C8.13 (concurrent cross-plane traffic and resource ceilings) is the open milestone; C8.14–C8.15 and C9 follow.
- Architecture portability P0, P1, P2.1, P2.2, and P5 are complete; P2.3–P2.6 are superseded by P5. P4 physical Raspberry Pi 5 qualification is the next architecture evidence gate.
- The RPi5 ROS 2 demo track has RP0 and RP1 complete. RP2 onward is planned, and RP2's deliverables still need rewriting around the seL4 product boundary.
- ROS 2 compatibility, platform hardware H1–H14, foreign workloads, distributed authority, and native development D1–D7 are not started or deferred. ROS 2 is a bounded userspace compatibility profile over native Slime contracts, never a kernel ABI.
- The backlog (`roadmap/00-backlog.md`) is clear: B1–B55 are resolved with no open items.

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

`aarch64-sel4-qemu-virt` is the deterministic development and test platform, and the only profile the product image is built for. It exercises memory isolation, native Endpoint IPC, component lifecycle, generation boot, storage, and fault injection before equivalent paths are enabled on physical hardware.

seL4 16.0.0 is pinned in `sel4/pins.toml` and configured by `sel4/config/qemu-arm-virt.cmake`. The machine is `qemu-system-aarch64 -machine virt,virtualization=on -cpu cortex-a53 -smp 1 -m 2048M`, with virtio block devices attached by the gates that need them.

The five admitted target profiles are declared in `contracts/target-profile/v1/schema.zt`. Only `aarch64-sel4-qemu-virt` (id 5) builds an image today; `x86_64-qemu-virtio` (id 1) is the retained pre-P0 identity that the bounded rollback window must still decode, and `aarch64-qemu-virt`, `aarch64-rpi5`, and `riscv64-qemu-virt` are declared but unbuilt. Every generation names exactly one profile, and stage-0 rejects architecture, ABI, page-profile, and required-feature mismatches before mapping executable bytes.

### Tier 1: named physical targets

`aarch64-rpi5` is the near-term physical target and the demo's acceptance board; it is declared and contract-qualified but not yet booted. See [`roadmap/09-rpi5-ros2-demo.md`](roadmap/09-rpi5-ros2-demo.md).

`x86_64-framework13-amd-ai300` remains the eventual daily-driver target, deferred off the critical path. Its M4 removable-media vertical slice was observed on the retired custom kernel; no seL4 Framework image exists, so `just framework_inventory_check` fails closed.

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

### Dango: native interactive shell

[Dango](deps/dango) is the planned native shell and interactive command language. Its explicit environment, working-directory, stream, diagnostic, effect, and resource-lifetime semantics become the user-facing form of Slime's component launch model.

A command such as `$(sysinfo)` does not directly invoke a path-based syscall. The runtime:

1. asks the active command profile to resolve the name to an executable capability;
2. resolves the selected working directory under existing directory authority;
3. constructs explicit environment and stream endpoints;
4. asks the spawn service to create a component with only the listed grants;
5. maps structured component termination into Dango command results and effects.

Dango stdout is a data stream. Stderr is a separate diagnostic channel. A component fault, forced termination, peer loss, capability revocation, and a program-selected nonzero status remain distinguishable at the host boundary.

### Native application language

The native-development roadmap admits an additional application language whose compiler emits the exact target-qualified Slime component-image format directly. This is a programming-language frontend, not a replacement for Zutai configuration/schemas or Dango command interaction. The compiler and standard library are pinned content-addressed toolchain inputs; emitted images carry mapping information only, while capabilities, resource accounts, release authorization, and activation remain Slime generation/admission policy.

The language may consume Zutai-generated syscall and IPC bindings but may not define an independent cross-boundary schema source. Host and on-device compilation use the same normalized source/toolchain/target closure and must produce the same image identity.

## Component and generation boundary

The stable cross-project artifact is the versioned, deterministic generation manifest. The built format is v5, defined by `contracts/generation/v1/schema.zt` and decoded by `boot-contracts/src/generation.rs`; retained v2 generations still decode for the bounded rollback window. Its logical content is:

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

Executable payloads use the component image format in `contracts/component/v1/`: a bounded qualification header naming one exact target profile, then either a segment table with per-segment R/W/X flags (revision V2) or a complete native ELF the loader maps (revision `Elf`, the seL4 product path). Retained V1 images carry the implicit `x86_64-qemu-virtio` qualification of their only producer. Integrity comes from the generation object digest and authority from generation grants, so an image itself carries neither.

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

Exploratory directions enabled by the capability and generation model — descriptions, dependencies, exit-condition sketches, and promotion status — are registered in [`docs/directions/`](docs/directions/README.md), one elaborated file per active entry. None of them is a committed milestone; each becomes real only when promoted into the owning file under [`roadmap/`](roadmap/) with an observable exit condition.

## First vertical slice (complete)

The first end-to-end system milestone connected Slime OS, Zutai, and Dango without requiring a full filesystem or desktop:

```text
Zutai configuration
    -> normalized static generation manifest
    -> boot under QEMU
    -> isolated init component
    -> console service
    -> minimal Dango runtime
    -> command resolver
    -> sysinfo component
    -> echo-agent stub component (tool-call round-trip with no language model)
    -> streamed output back to the console
```

Acceptance criteria:
1. A host-side Zutai configuration describes `init`, `console`, `dango`, `sysinfo`, and `echo-agent` components.
2. The build produces immutable component objects and one deterministic generation manifest.
3. The root task starts an isolated bootstrap/init component rather than implementing userspace policy itself.
4. Init grants each component only the capabilities declared by the generation.
5. Dango resolves `sysinfo` to an executable capability; it does not assume a global executable path.
6. Spawn supplies no implicit environment, working directory, streams, or other authority.
7. `sysinfo` streams output over IPC and reports a structured termination reason.
8. Crashing `sysinfo` does not terminate Dango, the console service, init, or the system.
9. The same component and IPC contracts run under QEMU and from removable media on the Framework target.
10. The Framework run performs no write to the internal NVMe device.
11. An `echo-agent` stub component receives a tool-call message over a channel and replies with a structured response, with no language model involved. This pins the agent abstraction to the same component, capability, channel, and structured-termination contracts as `sysinfo`.

Criteria 1–8 and 11 are observed on the seL4 product path; `just sel4_dango_check` runs the scripted console session and `just sel4_component_graph_check` the launch graph. Criteria 9 and 10 were observed on the retired custom kernel only (M4); no seL4 Framework image exists, so they are historical rather than current evidence.

This slice defines the minimum useful contracts: userspace entry, address-space isolation, capability IPC, executable identity, command resolution, spawning, streams, termination notification, manifest decoding, fault containment, and the agent abstraction as a non-special case of the above.

## Roadmap

The canonical plan, acceptance criteria, status, and dependency graph live in [`roadmap/`](roadmap/README.md). Completed M1–M6 evidence is preserved separately from independent future tracks:

- [Backlog: defects and unmasked debt](roadmap/00-backlog.md)
- [Foundations and implemented history](roadmap/01-foundations.md)
- [Core runtime C7–C9](roadmap/02-core-runtime.md)
- [ROS 2 compatibility R0–R3](roadmap/03-ros2-compatibility.md)
- [Platform hardware H1–H14](roadmap/04-platform-hardware.md)
- [Foreign workloads X1–X2](roadmap/05-foreign-workloads.md)
- [Authority and trust A1–A5](roadmap/06-authority-trust.md)
- [Architecture portability P0–P5](roadmap/07-architecture-portability.md)
- [Native development D1–D7](roadmap/08-native-development.md)
- [Raspberry Pi 5 ROS 2 two-node demo RP0–RP8](roadmap/09-rpi5-ros2-demo.md)

Work is selected demo-first: the [RPi5 ROS 2 demo track](roadmap/09-rpi5-ros2-demo.md) is the active lane, and the backlog sits ahead of every lane. Framework daily-driver work, RV64, foreign workloads, and distributed authority are deferred unless they de-risk the demo. Results compose only at the release gates defined by the roadmap index.

## Current repository layout

```text
Cargo.toml       Root Rust workspace and shared build profiles
sel4/            Upstream seL4 pins (`pins.toml`) and per-platform CMake configuration
slime-root/      The seL4 root task: generation admission, tasks, allocation, shared buffers, IPC, supervision
stage0/          UEFI stage-0 loader (x86-64 and AArch64 targets; unused by the seL4 product boot)
components/      Rust no_std userspace components (`bins`), the syscall runtime (`runtime`), and generated protocols (`proto`)
boot-contracts/  Shared Rust boot, generation, storage, recovery, and handoff contract decoders
contracts/       Versioned Zutai schemas for every persisted, IPC, and boot format, plus generation fixtures
scripts/         Host tooling grouped as build/, check/, generate/, and lib/
tools/           Developer-facing helpers such as LLDB attachment
roadmap/         Canonical status, backlog, dependency graph, milestones, checks, and release gates
devlog/          Curated investigations, regression evidence, decisions, and verification history
assets/          Boot/runtime assets
deps/            Pinned seL4, rust-sel4, Zutai, and Dango submodules
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
