# Slime OS

Slime OS is an experimental atomic personal operating system built from a new kernel and userspace rather than on Linux. Its purpose is to explore capability-based isolation, component-oriented system services, explicit resource authority, and generation-based deployment while progressing toward a system usable on one real daily-driver laptop.

The project is currently an early Rust `no_std` kernel. It can build a UEFI image, boot under QEMU, print through the framebuffer and serial port, and run kernel tests. The architecture and daily-use goals below are committed design direction, not claims of current implementation.

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
    -> streamed output back to the console
```

Acceptance criteria:

1. A host-side Zutai configuration describes `init`, `console`, `dango`, and `sysinfo` components.
2. The build produces immutable component objects and one deterministic generation manifest.
3. The kernel starts an isolated bootstrap/init component rather than implementing userspace policy itself.
4. Init grants each component only the capabilities declared by the generation.
5. Dango resolves `sysinfo` to an executable capability; it does not assume a global executable path.
6. Spawn supplies no implicit environment, working directory, streams, or other authority.
7. `sysinfo` streams output over IPC and reports a structured termination reason.
8. Crashing `sysinfo` does not terminate Dango, the console service, init, or the kernel.
9. The same component and IPC contracts run under QEMU and from removable media on the Framework target.
10. The Framework run performs no write to the internal NVMe device.

This slice defines the minimum useful contracts: userspace entry, address-space isolation, capability IPC, executable identity, command resolution, spawning, streams, termination notification, manifest decoding, and fault containment.

## Milestone order

### 1. Kernel foundation

- exception and crash reporting;
- physical and virtual memory management;
- kernel allocation;
- APIC/timer support;
- architecture boundaries suitable for QEMU and Framework bring-up.

Exit condition: invalid mappings and faults are reported deterministically rather than silently hanging.

### 2. Isolation and IPC

- userspace mode and independent address spaces;
- preemptible tasks;
- kernel object and capability tables;
- channels, shared-memory transfer, timeouts, cancellation, and peer-death notification.

Exit condition: two userspace components communicate, and one may fault without corrupting the other or the kernel.

### 3. Bootstrap component graph

- boot object loading;
- versioned manifest decoding;
- init/service manager;
- explicit initial capability graph;
- console and `sysinfo` components.

Exit condition: the first vertical slice works under QEMU.

### 4. Framework safe bring-up

- UEFI/GOP console;
- ACPI discovery;
- keyboard input;
- timer and shutdown/reboot paths;
- removable-media boot with internal NVMe access disabled.

Exit condition: the same isolated userspace slice runs on the Framework without modifying internal storage.

### 5. Storage and generations

- virtio block, then Framework NVMe;
- GPT and an integrity-checked object store;
- immutable generations;
- pending/known-good boot state;
- rollback and garbage-collection roots;
- explicit persistent-state policy.

Exit condition: a failed pending generation automatically leaves or restores a bootable known-good generation.

### 6. Native interactive environment

- minimal Dango implementation and core runtime;
- command profile/resolver and spawn service;
- filesystem service and directory capabilities;
- generation inspection and update commands.

Exit condition: the system can inspect, build/stage, select, and roll back generations through native components.

### 7. Daily-driver hardware

Bring hardware up in risk order rather than feature visibility:

1. xHCI, USB HID, mass storage, and USB Ethernet;
2. native storage reliability and IOMMU-enforced DMA;
3. software-rendered display/compositor over GOP;
4. battery, charger, brightness, lid, thermal, and suspend/resume lifecycle;
5. touchpad and audio;
6. MT7925 Wi-Fi and Bluetooth;
7. Radeon display control and hardware acceleration.

GPU acceleration, Wi-Fi, and audio do not block the first native userspace milestone, but they are required before the Framework target can be called a daily-use system.

## Current repository layout

```text
kernel/       Rust no_std kernel and kernel tests
entry_point/  host-side UEFI image builder and QEMU runner
assets/       boot/runtime assets
Justfile      build, run, test, format, lint, and debug commands
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
- treating a framebuffer demo as completion of an OS architecture milestone.

Linux remains useful as the development host and may later run as an isolated guest for compatibility. It does not define Slime OS's kernel, native ABI, authority model, or deployment architecture.
