# Slime OS

Slime OS is an experimental atomic personal operating system built from a new kernel and userspace rather than on Linux. Its purpose is to explore capability-based isolation, component-oriented system services, explicit resource authority, and generation-based deployment while progressing toward a system usable on one real daily-driver laptop.

The project is currently a QEMU-verified Rust `no_std` kernel with a minimal userspace component graph, plus an observed removable-media boot on the Framework 13 AMD AI 300 target. It can build a UEFI image, print through GOP and serial, run kernel tests, decode a deterministic generation manifest, launch `init`, `console`, `dango`, `sysinfo`, `echo-agent`, and `storage-probe` components, pass IPC capabilities between them, read and SHA-256 verify a sector from a read-only virtio block device, and report a healthy vertical slice. Durable writes, rollbackable generations, native interactive Dango, and daily-driver hardware remain unfinished.

## Current status

- QEMU is the automated verification target; the same vertical slice has also reached `[generation] vertical slice healthy` from removable media on the Framework target.
- Kernel foundation work is in place: GDT/TSS, IDT exception reporting, physical and virtual memory management, heap allocation, and APIC timer support.
- Core isolation is exercised by tests: independent userspace components can communicate over IPC, and a faulting component does not corrupt or terminate its peer or the kernel.
- Generation format 1 has a pinned Zutai-side contract, fixtures, a deterministic host builder, and a kernel decoder.
- The first QEMU vertical slice is healthy: `init` launches `console`, `dango`, `sysinfo`, and `echo-agent`; Dango resolves executable authority through capabilities; `sysinfo` and `echo-agent` stream structured output; every component exits successfully.
- The storage capability foundation (M5.1) is complete: ACPI MCFG parsing, bounded PCI enumeration, rights-checked capabilities for PCI functions, DMA memory, interrupts, and shared memory, DMA pinning guarded against reclamation while requests are outstanding, and a bounded block request/reply IPC protocol, verified by the `storage_cap_check` QEMU target.
- Typed IPC schemas (M5.2a) are complete: the versioned Zutai block contract generates kernel Rust and component GNU assembler bindings, with stale-output, assembler, round-trip, bounds, and version checks.
- The read-only virtio block vertical slice (M5.2) is complete: a capability-gated userspace probe reads a fixed QEMU sector, verifies its SHA-256 digest, and confirms structured rejection of writes, short buffers, and out-of-range LBAs through `storage_read_check`.
- Framework safe bring-up is verified with storage authority absent; durable writes, rollbackable generations, native interactive Dango, and daily-driver hardware support are not complete.

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

The kernel should eventually own only the mechanisms that require privilege:

- threads, scheduling, and address spaces;
- physical memory and memory objects;
- capability tables and object lifetime;
- IPC channels and capability transfer;
- interrupts, timers, and minimal platform control.

Userspace services should own policy and most complex subsystems:

- component resolution and spawning;
- filesystems and persistent state;
- device management and most drivers;
- networking;
- display, input, and audio;
- generation construction, activation, health checking, and rollback.

New IPC protocols must be schema-first: channel message types are declared as versioned Zutai types under `contracts/`, and endpoint bindings are generated from or deterministically validated against those contracts. Kernel and component code must not introduce independent hand-written field offsets. This makes "tool call = channel" literal — an agent tool schema and a system IPC schema are the same artifact — and gives interposition tooling (auditing, recording, replay) typed messages instead of opaque bytes.

POSIX and Linux compatibility may exist later as userspace personalities or isolated virtual machines. They are compatibility facilities, not the native kernel ABI or authority model.

## Reference targets

### Tier 0: automated architecture target

`x86_64-qemu-virtio` is the deterministic development and test platform. It exists to exercise memory isolation, IPC, component lifecycle, generation boot, storage, networking, and fault injection before equivalent paths are enabled on physical hardware.

Expected virtual hardware includes UEFI, APIC, ACPI, GOP or virtio-gpu, and virtio block/network/input devices.

### Tier 1: physical daily-use target

`x86_64-framework13-amd-ai300` is the only initially supported physical machine.

Reference hardware:

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

Production system evaluation must not receive authority to modify the boot partition, switch generations, format storage, or grant kernel capabilities. Zutai describes intent; the Slime builder validates and executes it transactionally.

Zutai host capabilities are language-level declarations and are currently advisory. They are not Slime kernel capabilities. A Zutai runtime on Slime may hold opaque handles backed by real service capabilities, but only the kernel and trusted services enforce and transfer authority.

The first build pipeline may run Zutai on the development host. Porting the compiler/runtime into Slime userspace must not block kernel and component bring-up.

### Dango: native interactive shell

[Dango](deps/dango) is the planned native shell and interactive command language. Its explicit environment, working-directory, stream, diagnostic, effect, and resource-lifetime semantics become the user-facing form of Slime's component launch model.

A command such as `$(sysinfo)` does not directly invoke a path-based syscall. The runtime:

1. asks the active command profile to resolve the name to an executable capability;
2. resolves the selected working directory under existing directory authority;
3. constructs explicit environment and stream endpoints;
4. asks the spawn service to create a component with only the listed grants;
5. maps structured component termination into Dango command results and effects.

Dango stdout is a data stream. Stderr is a separate diagnostic channel. A component fault, forced termination, peer loss, capability revocation, and a program-selected nonzero status remain distinguishable at the host boundary.

## Component and generation boundary

The first stable cross-project artifact should be a versioned, deterministic generation manifest. Its logical content is:

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

The kernel must not parse Zutai source or own system policy. It starts a small bootstrap component with root bootstrap capabilities. That component verifies the selected generation and starts the userspace graph.

Manifest encoding is not yet frozen. It must be deterministic, bounded, versioned, safe to decode before the full userspace environment exists, and independent of JSON-specific tagged-union conventions.

## Agentic direction

The five first-class concepts are also the natural primitives for running autonomous agents safely, and no new authority model is required for it.

- **Agent = Component.** An agent is an isolated component with an address space and explicit dependencies. Agent fault containment is component fault containment: a crashing agent does not terminate its peers, services, or the kernel.
- **Tool call = Channel.** A tool invocation is a typed IPC message to a service endpoint, not an arbitrary function call. The endpoint's schema defines the message; capability transfer along the channel is the only way authority crosses the boundary.
- **Agent authority = Capability grant.** Spawn supplies no implicit environment, working directory, streams, or other authority. An agent receives only the grants declared by the generation, and unforgeable capabilities mean authority cannot be ambient, guessed, or widened at runtime.
- **Agent memory = State binding.** Long-term agent state is a `StateBinding` with an owner, schema version, and policy. `snapshotBeforeUpgrade` and `discardOnRollback` give agent memory the same upgrade and rollback discipline as the rest of the system.
- **Agent update = Generation.** Changing an agent's model, prompt, or tool set produces a new generation. Health checking applies to agent behavior as well as to boot: a pending generation becomes known-good only after userspace confirmation, and a regressing agent rolls back with the same mechanism as a regressing kernel.

The kernel does not treat agents or language models as special. A language model is a userspace service component that agents address over channels; the scheduler, context, and memory concerns of agent runtimes live in userspace services, not in the kernel. This keeps the kernel policy-free for agents as it is for every other subsystem, and lets model choice, provider, and placement change as a generation without touching the kernel ABI.

External agent protocols such as MCP may be bridged by a dedicated component that exposes protocol servers as Slime capability endpoints. The bridge cannot grant authority the agent does not already hold, so prompt injection success at the model layer is still bounded by the generation's declared grants.

Because no component holds ambient authority, every capability can be transparently interposed by a user-chosen proxy component (a membrane). This enables agent dry-runs: an agent can be executed against virtualized capabilities to preview the effects it *would* have, before any real authority is granted. Capability transfers can also record provenance, so the system can answer "why is this component allowed to do X" as an explicit grant chain rooted in the generation manifest.

Atomicity and agentic operation reinforce each other: agent memory and authority are versioned, verified, and rollbackable by the same mechanisms as the boot graph, and the boot graph can include agent components without a separate agent deployment track.

## Differentiating directions

These are exploratory directions enabled by the capability and generation model. None of them is a committed milestone; each becomes real only when promoted into ROADMAP.md with an observable exit condition.

- **IPC flight recorder and deterministic replay.** All component input crosses channel boundaries, so recording at that boundary yields deterministic re-execution of a single component. A bug report becomes a generation hash plus an IPC trace.
- **Generation bisect.** Generations form a content-addressed parent chain, so "which update regressed this" is automatable as safe boot-and-health-check bisection.
- **Shadow boot.** A pending generation can be health-checked in a constrained sub-graph or guest VM before real activation consumes a boot attempt.
- **Cross-machine generation sync.** A generation is a manifest plus content-addressed objects; moving a system to a new machine is object transfer plus activation, including capability grants and state policy — not dotfile reconstruction.
- **Zutai-defined state migrations.** State schema upgrades expressed as pure Zutai transformations are deterministic, dry-runnable before activation, and covered by the same rollback contract as the boot graph.
- **Powerbox UI.** Applications never hold an ambient "open file" right; the file dialog is a system component, and the user's selection gesture itself mints a single-object capability. Authorization and intent are the same gesture.
- **Per-component energy accounting.** Scheduler-attributed energy per component and per channel activity, with policy such as background power budgets carried as grants.
- **Per-destination network authority.** Network access is a capability to explicit endpoints declared by the generation, making exfiltration surface auditable in the manifest — particularly relevant for agent components.
- **MPK/PKU lightweight compartments.** A third isolation tier between full components and same-address-space code for latency-sensitive boundaries, using user-space protection keys available on the target CPU.

## First vertical slice

The first end-to-end system milestone connects Slime OS, Zutai, and Dango without requiring a full filesystem or desktop:

```text
Zutai configuration
    -> normalized static generation manifest
    -> UEFI boot under QEMU
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
3. The kernel starts an isolated bootstrap/init component rather than implementing userspace policy itself.
4. Init grants each component only the capabilities declared by the generation.
5. Dango resolves `sysinfo` to an executable capability; it does not assume a global executable path.
6. Spawn supplies no implicit environment, working directory, streams, or other authority.
7. `sysinfo` streams output over IPC and reports a structured termination reason.
8. Crashing `sysinfo` does not terminate Dango, the console service, init, or the kernel.
9. The same component and IPC contracts run under QEMU and from removable media on the Framework target.
10. The Framework run performs no write to the internal NVMe device.
11. An `echo-agent` stub component receives a tool-call message over a channel and replies with a structured response, with no language model involved. This pins the agent abstraction to the same component, capability, channel, and structured-termination contracts as `sysinfo`.

This slice defines the minimum useful contracts: userspace entry, address-space isolation, capability IPC, executable identity, command resolution, spawning, streams, termination notification, manifest decoding, fault containment, and the agent abstraction as a non-special case of the above.

## Roadmap

The canonical milestone order, acceptance criteria, and detailed implementation plan live in [`ROADMAP.md`](ROADMAP.md).

Current sequence:

1. Kernel foundation — QEMU tests passing.
2. Isolation and IPC — core QEMU exit passing.
3. Bootstrap component graph — QEMU vertical slice passing.
4. Framework safe bring-up — verified.
5. Storage and generations — in progress; M5.1, M5.2a, and M5.2 complete.
6. Native interactive environment — minimal stub only.
7. Daily-driver hardware — not yet implemented.

The current milestone is storage and rollbackable generations. Its exit condition is that a failed pending generation automatically leaves or restores a bootable known-good generation. The capability foundation (M5.1), typed IPC schemas (M5.2a), and read-only virtio block I/O (M5.2) are complete; durable virtio writes and fault handling (M5.3) are next. `ROADMAP.md` decomposes the remaining work into durable writes, GPT and the object store, boot-state records, rollback and GC, and Framework NVMe safety promotion.

## Current repository layout

```text
kernel/       Rust no_std kernel, boot path, generation decoder, scheduler, IPC, and tests
components/   Minimal assembly userspace components for the QEMU vertical slice
contracts/    Generation manifest v1 contract, Zutai fixtures, and validation entrypoints
scripts/      Host-side generation build/check and contract validation helpers
assets/       Boot/runtime assets
deps/         Pinned Zutai and Dango submodules
Justfile      Build, run, test, format, lint, generation, contract, and debug commands
```

Common development commands:

```sh
just run
just test
just fmt_check
just lint
```

## Non-goals for the initial system

- supporting arbitrary PCs;
- reproducing Linux, FHS, systemd, UID/GID, `fork`, signals, or ambient path authority as native primitives;
- writing a desktop environment before isolation and service recovery work;
- requiring the Slime kernel to run existing Linux binaries directly;
- inventing another configuration language or shell;
- embedding a language model, agent runtime, or agent scheduler in the kernel;
- granting agents authority to switch generations, format storage, or grant kernel capabilities;
- running agents outside the capability and generation model;
- treating a framebuffer demo as completion of an OS architecture milestone.

Linux remains useful as the development host and may later run as an isolated guest for compatibility. It does not define Slime OS's kernel, native ABI, authority model, or deployment architecture.
