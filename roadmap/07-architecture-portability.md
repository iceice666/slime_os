# Architecture portability track

**Purpose:** Preserve one Slime capability/component/generation architecture across target profiles while making AArch64 and Raspberry Pi 5 the near-term product path.

**Status:** In progress — P0, P1, P2.1, and P2.2 complete.

**Decision:** AArch64/Raspberry Pi 5 is now the near-term physical target because the current product goal is the RPi5 ROS 2 two-node demo. The existing x86-64 QEMU path remains the regression oracle for completed work until each semantic corpus is replayed on AArch64, but x86-64/Framework is no longer the product-leading roadmap. RV64 is deferred.

Slime targets 64-bit little-endian systems with an MMU and user/supervisor isolation. MCU-class targets without that isolation boundary are external bounded companions, not reduced-security ports of this kernel.

## Initial target profiles

| Profile | Role | Initial machine | Required baseline |
| --- | --- | --- | --- |
| `x86_64-qemu-virtio` | Existing regression oracle | QEMU q35/UEFI | x86-64, 4 KiB pages, ring 0/ring 3, APIC, virtio |
| `aarch64-qemu-virt` | First non-x86 architecture gate | QEMU `virt`/UEFI or pinned firmware path | AArch64, 4 KiB translation granule, EL1/EL0, GICv3, generic timer, PL011, virtio |
| `aarch64-rpi5` | Near-term physical product target | Raspberry Pi 5, exact board/firmware/media profile selected by RP0/RP3 | AArch64, 4 KiB translation granule, EL1/EL0 or documented firmware entry state, GIC, generic timer, device tree, serial console, reproducible removable media |
| `riscv64-qemu-virt` | Deferred second architecture profile | Pinned QEMU `virt` machine and firmware | RV64 little-endian, S/U mode, Sv39, atomic operations, pinned interrupt/timer/UART devices, virtio |

A profile name identifies a complete executable and platform contract, not only an instruction set. A different page granule, privilege model, interrupt controller, firmware handoff, board revision, or incompatible device topology is a new profile until its own checks pass.

## Boundaries

- Capability semantics, object identities, rights, channels, generation selection, BootState, release authorization, rollback, Zutai protocols, C7 shared samples, C8 typed routes, and ROS local/wire profiles remain architecture-neutral.
- Trap frames, context switching, privilege transitions, page tables, TLB operations, interrupt controllers, timers, idle instructions, debug transports, QEMU exit paths, firmware handoff, device-tree parsing, and early boot mappings are architecture-specific mechanisms.
- The generation `target` remains the signed complete platform profile. Release metadata continues to bind the exact target.
- Kernel, component, and ROS node executables are built and authenticated per target. Architecture-neutral resource objects may be shared when their schemas and identities are byte-identical; executable objects are never assumed portable across targets.
- A logical syscall operation has one semantic contract, error model, bounds, and rights checks. Each architecture has an explicit calling convention and trap instruction; register layouts are not serialized as a cross-architecture ABI.
- The implementation uses small explicit architecture modules. It does not introduce a broad trait framework merely to hide one call site, and it does not move device or scheduling policy into the kernel.
- QEMU proves deterministic architecture behavior. It cannot establish a physical Raspberry Pi 5 board, firmware, storage, timing, or device-support claim.

## Sequencing

1. The backlog remains ahead of new roadmap gates.
2. P0 fixes target and executable-artifact contracts before another architecture emits executable generations.
3. P1 extracts and verifies the existing x86-64 implementation without changing observable behavior; this prevents x86 trap, APIC, CR3, GDT/IDT, PCI, and firmware assumptions from becoming universal contracts.
4. P2 establishes the AArch64 QEMU vertical slice and replays the architecture-neutral kernel/component corpus needed by RP2/RP4.
5. P4 first names and qualifies Raspberry Pi 5, not a generic “ARM board”. RP3/RP7 consume that physical evidence.
6. C8 and R0 may proceed on the existing reference path where useful, but the demo closes only after AArch64/RPi5 replay.
7. P3 RV64 is deferred until after the RPi5 ROS 2 demo stabilizes.

## P0: Architecture, target, and executable-artifact contracts

**Status:** Complete.

**Depends on:** Foundations and a cleared or explicitly deferred backlog.

### Deliverables

- define versioned Zutai component-image and kernel-image revisions carrying an explicit architecture identifier, architecture-qualified ABI identifier, required ISA/profile flags, and page-profile identifier;
- retain bounded decoding of existing x86 component and kernel images for the declared rollback window; old formats keep their existing meaning and are never reinterpreted as architecture-neutral;
- validate generation target, release target, kernel image, bootstrap image, ROS node executable closure, and every component executable as one compatible set before execution or activation;
- define the initial profile identifiers above and reject unknown architecture IDs, ABI IDs, required flags, page profiles, and target/image mismatches before mapping executable bytes;
- parameterize host builders, ELF validation, direct image emitters, linker inputs, Cargo targets, artifact paths, and QEMU/physical launch selection by the exact profile;
- preserve content identity for byte-identical architecture-neutral resources while producing separately identified target executables and complete target generations;
- define one semantic syscall table with per-architecture calling-convention documents for x86-64 `int 0x80`, AArch64 `svc`, and RV64 `ecall`;
- audit the stage-0 handoff and generation contracts so physical addresses, direct-map metadata, framebuffer/serial data, memory maps, device-tree references, and executable entry state remain versioned without serializing x86 page-table or register layouts.

### Required checks

- an image for one architecture or ABI cannot be staged, selected, or executed under another target even when its hashes and segment bounds are otherwise valid;
- retained x86 generations continue to decode and boot during the rollback window, while unsupported legacy or future formats fail closed;
- two builds of the same normalized target input are byte-identical, and changing only the target changes the authenticated generation and release identity;
- builders reject the wrong ELF machine, unsupported relocation, page profile, endianness, required ISA flag, or target-specific load layout before emitting a Slime executable image;
- resource objects that are declared architecture-neutral remain byte-identical across target builds, while executable object selection is exact and unambiguous;
- syscall numbers, errors, capability checks, message bounds, and transfer semantics are identical across calling-convention specifications.

### Planned verification target

```sh
just architecture_contract_check
```

### Exit condition

A generation and its release identify one exact target profile; stage-0 rejects every mismatched kernel, component, or node executable before mapping it, retained x86 rollback artifacts retain their old meaning, and deterministic builders emit only profile-valid authenticated artifacts.

## P1: x86-64 architecture boundary extraction

**Status:** Complete.

**Depends on:** P0.

### Deliverables

- place x86 trap frames, exception stubs, context switching, user entry, control-register access, page-table operations, TLB invalidation, interrupt masking, GDT/TSS/IDT, APIC/PIT time, port I/O, halt, serial, and QEMU-exit mechanisms behind an explicit `arch/x86_64` boundary;
- separate QEMU q35 and Framework platform assembly from ISA mechanisms so ACPI/PCI/UEFI policy does not become the interface required by AArch64/RPi5;
- give stage-0 profile-specific page-table construction, relocation validation, entry-state setup, and linker configuration while preserving the shared verified-generation and BootState selection flow;
- make userspace syscall wrappers select only the per-architecture trap/calling-convention implementation while retaining one semantic Rust API;
- add a source allowlist check that rejects x86 instructions, registers, ELF machine constants, and x86-only linker/QEMU assumptions outside admitted architecture/platform/build files;
- preserve all existing x86 behavior and evidence; this slice is a boundary extraction, not permission to weaken bounds or rewrite completed contracts.

### Required checks

- the current x86 QEMU boot, isolation, IPC, generation, rollback, recovery, storage, B2, C7, and C8 baseline checks retain their existing observable results as applicable when P1 lands;
- a user fault, syscall, timer preemption, address-space switch, and blocked-task wake traverse the extracted boundary without changing their structured result;
- no x86 assembly, CR register, GDT/IDT/APIC/PIT, port-I/O, ELF-machine, linker-format, or `qemu-system-x86_64` assumption remains in architecture-neutral kernel, component-runtime, contract, or generation code except an explicit profile dispatch;
- architecture-neutral code can be type-checked for AArch64 without importing x86-only modules, even before that target boots.

### Verification target

```sh
just x86_portability_check
```

The gate has two halves: a source allowlist over the architecture-neutral trees,
and a `cargo build` of the neutral kernel library and component runtime for
`aarch64-unknown-none`. The build, not a `cargo check`, is the binding half —
rustc validates inline-assembly mnemonics only during codegen, so a check-based
gate accepts x86 assembly on an AArch64 target. Both halves were confirmed to
fire on deliberately introduced leaks. It requires
`rustup target add aarch64-unknown-none`, declared in `rust-toolchain.toml`.

### Exit condition (observed)

Observed 2026-08-02; see [`devlog/2026-08-02-p1-x86-boundary-extraction/`](../devlog/2026-08-02-p1-x86-boundary-extraction/index.md).

The x86-64 reference vertical slice behaves as before through a named
architecture/platform boundary: `just test` passes the same 191 assertions and
`just product_boot_check` reaches the same healthy 45-slot product slice as the
pre-change baseline, with `just rollback_check`, `just architecture_contract_check`,
`just generation_check`, and `just contracts_check` clean. `just x86_portability_check`
enforces the boundary over 186 neutral Rust files and builds the neutral kernel
and component runtime for a second architecture. This proves the boundary holds;
it makes no claim that AArch64 boots, which is P2.

## P2: AArch64 QEMU vertical slice

**Status:** In progress — decomposed into P2.1–P2.6.

**Depends on:** P1 (complete), C7, and backlog item B2.

P2 as originally written is one milestone covering a complete second
architecture: firmware handoff, translation tables, exception vectors, syscall
entry, context switching, an interrupt controller, a timer, device transports,
target-specific component images, and a replay of the B2/C7/C8 acceptance
corpus. That is too broad for one reviewable slice, and its exit condition
cannot be partially observed — either every part boots or none of it is
evidence. It is decomposed below on the same principle that split C7 into
C7.1–C7.7 and C8.9 into C8.9–C8.15: each sub-slice introduces one primary
mechanism and owns an independently observable QEMU gate.

The parent's deliverables, required checks, and exit condition below remain
authoritative for the aggregate. P2 closes only when P2.1–P2.6 have each closed
under their own gate and the aggregate corpus runs; no sub-slice may claim the
parent's exit condition.

### Sub-slice sequence

| Slice | Primary mechanism | Gate |
| --- | --- | --- |
| P2.1 | Firmware handoff, EL1 entry, translation tables, PL011, QEMU exit | `just aarch64_boot_check` (complete) |
| P2.2 | Exception vectors, fault decoding, `svc` syscall entry, saved user context | `just aarch64_trap_check` (complete) |
| P2.3 | EL0 component execution, address-space switching, isolation, fault attribution | `just aarch64_isolation_check` |
| P2.4 | GICv3 delivery, generic timer, preemption, all B2 wake classes | `just aarch64_wake_check` |
| P2.5 | virtio-mmio transport, generation selection, rollback, wrong-target rejection | `just aarch64_generation_check` |
| P2.6 | C7/C8 bounded data path and the aggregate semantic corpus | `just aarch64_qemu_check` |

### Deliverables

- build an AArch64 stage-0/firmware handoff and kernel for the pinned QEMU `virt` profile using the same verified generation, release, BootState, handoff, and rollback semantics;
- enter the kernel at EL1, run isolated components at EL0, establish bounded 4 KiB translation tables and direct-map access, and reject malformed or unsupported mappings with structured failure;
- implement exception vectors, synchronous fault decoding, `svc` syscalls, saved user context, address-space switching, interrupt masking, and idle/wake behavior behind `arch/aarch64`;
- implement GICv3 interrupt delivery, the ARM generic timer, PL011 diagnostics, QEMU exit, and the deterministic virtio devices required by the exercised vertical slice;
- build target-specific component images from AArch64 ELF intermediates while keeping syscall semantics, capabilities, generation grants, and userspace service protocols identical to x86;
- run the same B2 wait/wake sources and C7/C8 data path required by the RPi5 demo rather than creating architecture-specific alternatives.

### Required checks

- verified stage-0 selects and transfers to an AArch64 kernel, launches at least two isolated EL0 components, and exchanges bounded IPC through the same capability semantics;
- invalid instruction, data abort, permission fault, malformed user range, and component crash terminate or report the responsible component without corrupting another component or the kernel;
- timer preemption, endpoint wake, scripted-input wake, and supervision wake drain and refill the ready queue without lost wakeups or busy polling;
- two components exchange and return a C7 payload larger than the control-message bound, with quota exhaustion and peer death reclaiming the same resources as on x86;
- a failing pending AArch64 generation returns to a verified AArch64 known-good generation; a signed x86 generation is rejected as the wrong target rather than attempted;
- fixed inputs produce deterministic normalized traces comparable at the semantic event level with x86; raw register frames and physical addresses are explicitly excluded from byte-equality claims.

### Planned verification target

```sh
just aarch64_qemu_check
```

### Exit condition

The AArch64 QEMU profile boots a verified rollbackable generation, runs isolated EL0 components, exercises IPC, faults, timer preemption, all B2 wake classes, and the bounded C7/C8 data path with the same architecture-neutral authority and lifecycle semantics as x86-64.

### P2.1 — Firmware handoff, EL1 entry, and translation tables

**Status:** Complete.

**Depends on:** P1.

The first slice that executes AArch64 instructions. Everything before it is
contract and boundary work; this is where the profile either boots or does not.

#### Deliverables

- pin the exact `qemu-system-aarch64 -machine virt` machine, CPU, memory, and UEFI firmware for the `aarch64-qemu-virt` profile, recorded where the launcher reads it rather than passed ad hoc;
- build the stage-0 loader for `aarch64-unknown-uefi`, sharing the architecture-neutral generation selection, BootState, release authorization, and rollback flow with x86 and supplying only AArch64 entry mechanism behind `stage0::arch`;
- construct bounded 4 KiB stage-1 translation tables covering the loaded kernel image, a direct map of physical memory, and a guarded boot stack, and validate the descriptor encodings against a running EL1 rather than inheriting them from the reference manual unchecked;
- enter the kernel at EL1 with `MMU`, caches, and the SIMD baseline configured, and reject an unsupported granule, address-size, or firmware entry state with a structured `BootError` rather than a hang;
- implement PL011 diagnostics and the profile's QEMU exit path so a boot produces observable serial output and a deterministic exit code;
- fill the AArch64 boot context from the same `KernelHandoffV1` bytes x86 consumes, with no second handoff format.

#### Required checks

- a verified generation built for `aarch64-qemu-virt` boots under the pinned QEMU machine and firmware, and emits its bring-up markers over PL011;
- the kernel runs at EL1 with the MMU enabled, reaches the heap through the direct map, and reports its translation configuration;
- a malformed or unsupported mapping, granule, or entry state fails with a structured error and a distinct marker instead of a silent reset or hang;
- an x86 generation, and a generation naming another AArch64 profile, are both rejected before executable bytes are mapped;
- the run terminates through the profile's debug-exit path with a deterministic exit code rather than a timeout;
- the x86 regression corpus is unchanged by the slice.

#### Verification target

```sh
just aarch64_boot_check
```

#### Exit condition (observed)

Observed 2026-08-03; see [`devlog/2026-08-03-p2-1-aarch64-boot/`](../devlog/2026-08-03-p2-1-aarch64-boot/index.md).

`qemu-system-aarch64 -machine virt` boots a verified `aarch64-qemu-virt`
generation to EL1 under AArch64 UEFI firmware with the MMU and both caches
enabled, brings up physical and virtual memory over the direct map, allocates
from a working heap, reports the generation identity and BootState stage-0
selected, and ends through the profile's semihosting exit rather than a timeout
— all asserted as ordered PL011 markers by `just aarch64_boot_check`. The x86
corpus is unchanged at 191 assertions.

No component runs and no syscall is served; those are P2.3 and P2.2. This is the
first non-x86 execution in the project, and it is QEMU only — it establishes
nothing about Raspberry Pi 5 hardware.

### P2.2 — Exception vectors, fault decoding, and `svc` entry

**Status:** Complete.

**Depends on:** P2.1.

#### Deliverables

- install the EL1 exception vector table and save/restore the `UserFrame` P1 defined, preserving `x0`–`x30`, `SP_EL0`, `ELR_EL1`, and `SPSR_EL1` across entry;
- decode synchronous exception classes from `ESR_EL1` into the existing architecture-neutral `UserFaultReason` vocabulary without adding an AArch64-specific fault taxonomy;
- implement `svc #0` syscall entry against the register mapping already documented in `docs/syscall-abi.md`, dispatching into the shared syscall body;
- implement `DAIF` interrupt masking and the idle/park path behind the `arch::cpu` signatures P1 fixed.

#### Required checks

- a synchronous fault taken at EL1 is decoded, attributed, and reported rather than escalating silently;
- every documented syscall argument and return register carries its value across `svc` and `eret` unchanged;
- the frame saved on entry is the frame restored on return, including for a handler that mutates it;
- syscall numbers, errors, bounds, and rights checks match x86 for the same call, exercised through the shared syscall body rather than an architecture-specific stub.

#### Verification target

```sh
just aarch64_trap_check
```

#### Exit condition (observed)

Observed 2026-08-03; see [`devlog/2026-08-03-p2-2-aarch64-traps/`](../devlog/2026-08-03-p2-2-aarch64-traps/index.md).

The architected 16-slot EL1 vector table is installed at `VBAR_EL1`, an EL1
`brk` and an EL0 undefined instruction are both decoded through `ESR_EL1.EC`
into the shared `UserFaultReason` vocabulary and reported, an `svc #0` issued
from EL0 carries all five documented argument registers into the
architecture-neutral `kernel/src/syscall/mod.rs` body and returns its result in
`x0`, the complete 31-register frame plus `SP_EL0` survives `eret` including a
deliberate handler mutation, and `DAIF` masking is observed enabled, masked,
and restored — all asserted as ordered PL011 markers by
`just aarch64_trap_check`. The x86 corpus is unchanged.

No component is scheduled and no address space is switched per task; the EL0
evidence comes from a bounded bring-up probe that builds its own TTBR0 root and
releases it. Component execution and isolation are P2.3.

### P2.3 — EL0 execution, address spaces, and isolation

**Status:** Not started.

**Depends on:** P2.2.

#### Deliverables

- move the user/kernel top-level table split behind `arch::paging` before the first EL0 task exists: `KERNEL_HALF_START` in `memory/vmm.rs` describes x86's single-root layout, and AArch64's two-root split makes `free_user_half` leak the upper half of every user root;
- build target-qualified AArch64 component images from AArch64 ELF intermediates through the existing profile-parameterized builders;
- execute components at EL0 with user/kernel translation separation, switch address spaces on schedule, and reclaim every frame on termination as x86 does;
- attribute an invalid instruction, data abort, permission fault, and malformed user range to the responsible component and terminate it without disturbing another component or the kernel.

#### Required checks

- stage-0 launches at least two isolated EL0 components that exchange bounded IPC under the same capability semantics as x86;
- each fault class terminates the responsible component with the same structured result x86 produces;
- a component cannot read or write another's mapped pages, and address-space teardown conserves frames.

#### Verification target

```sh
just aarch64_isolation_check
```

#### Exit condition

Two isolated EL0 components run under one AArch64 kernel, exchange bounded IPC
through unchanged capability semantics, and every fault class is attributed and
reclaimed as on x86.

### P2.4 — GICv3, generic timer, and the B2 wake classes

**Status:** Not started.

**Depends on:** P2.3 and backlog item B2.

#### Deliverables

- implement GICv3 distributor/redistributor initialization and interrupt delivery for the pinned machine;
- implement the ARM generic timer as the periodic tick behind the boundary's timer slot, replacing the retained `apic` name with an architecture-neutral one on both architectures;
- drive timer preemption, endpoint wake, scripted-input wake, and supervision wake through the shared scheduler rather than an AArch64 alternative.

#### Required checks

- timer preemption advances the monotonic clock and rotates the ready queue;
- every B2 wake class drains and refills the ready queue with no lost wakeup and no busy polling;
- the idle path parks without a lost-wake window, as the x86 `sti; hlt` pairing does.

#### Verification target

```sh
just aarch64_wake_check
```

#### Exit condition

The generic timer preempts and all four B2 wake classes drain and refill the
ready queue on AArch64 under the shared scheduler.

### P2.5 — virtio-mmio, generation selection, and rollback

**Status:** Not started.

**Depends on:** P2.4.

#### Deliverables

- implement the device-tree-discovered virtio-mmio block transport behind the neutral `device_discovery` and `BlockDevice` surfaces P1 established, replacing the empty non-x86 device list;
- run AArch64 generation staging, activation, and rollback through the same BootState, release, and recovery flow as x86.

#### Required checks

- a failing pending AArch64 generation returns to a verified AArch64 known-good generation and durably drains its attempt window;
- a signed x86 generation is rejected as the wrong target rather than attempted;
- block reads and writes reach a deterministic virtio-mmio device with the same capability gates and error model as the PCI transport.

#### Verification target

```sh
just aarch64_generation_check
```

#### Exit condition

An AArch64 generation is selected, activated, and rolled back through the shared
flow over a device-tree-discovered virtio-mmio transport, with wrong-target
artifacts refused before execution.

### P2.6 — C7/C8 data path and the aggregate corpus

**Status:** Not started.

**Depends on:** P2.5 and C7.

#### Deliverables

- run the C7 shared-sample plane and the C8 bounded data path required by the RPi5 demo on AArch64, unmodified;
- produce deterministic normalized traces from fixed inputs, comparable with x86 at the semantic event level.

#### Required checks

- two components exchange and return a payload larger than the control-message bound, with quota exhaustion and peer death reclaiming the same resources as on x86;
- fixed inputs produce normalized traces that match x86 at the semantic event level, with raw register frames and physical addresses explicitly excluded from the comparison;
- the aggregate P2 corpus passes and the x86 corpus is unchanged.

#### Verification target

```sh
just aarch64_qemu_check
```

#### Exit condition

The parent P2 exit condition above, observed: the AArch64 QEMU profile boots a
verified rollbackable generation, runs isolated EL0 components, and exercises
IPC, faults, preemption, every wake class, and the bounded C7/C8 data path with
the same architecture-neutral authority and lifecycle semantics as x86-64.

## P3: RV64 QEMU vertical slice

**Status:** Deferred until after the Raspberry Pi 5 ROS 2 demo stabilizes.

**Depends on:** P2.

### Deliverables

- pin one QEMU `virt` machine version, firmware/stage-0 route, RV64 ISA baseline, interrupt controller, timer, UART, and virtio device set;
- implement S-mode kernel and U-mode component execution with Sv39, 4 KiB pages, bounded page-table construction, TLB invalidation, and explicit unsupported-feature rejection;
- implement trap decoding, `ecall` syscalls, saved user context, address-space switching, interrupt masking, idle/wake behavior, timer preemption, diagnostics, and QEMU exit behind `arch/riscv64`;
- replay the same isolation, B2, C7, generation, and rollback acceptance corpus used by P2.

### Required checks

- the pinned RV64 QEMU profile boots a verified target generation and rejects x86/AArch64 artifacts before executable mapping;
- S/U isolation, faults, syscalls, timer preemption, blocked waits, shared samples, quota exhaustion, peer death, and rollback preserve the same structured semantics as the other architectures;
- unsupported ISA extensions, page modes, firmware handoffs, interrupt profiles, ELF flags, and relocations fail explicitly rather than being guessed from the running machine;
- no AArch64 register, GIC, firmware, translation-table, or device assumption appears in shared or RV64-specific paths.

### Planned verification target

```sh
just riscv64_qemu_check
```

### Exit condition

The pinned RV64 QEMU profile passes the same architecture-neutral isolation, wait/wake, sample-plane, generation, and rollback corpus as AArch64 without importing x86 or ARM mechanism into its contracts.

## P4: Raspberry Pi 5 physical architecture qualification

**Status:** Not started.

**Depends on:** P2 for the first AArch64 board.

### Deliverables

- select one exact Raspberry Pi 5 board revision or accepted revision set, firmware version, boot path, removable storage medium, interrupt topology, timer, serial path, and minimum device set;
- record reproducible removable-media images, generation/release identities, firmware and board identities, normalized device tree/topology, serial evidence, storage-integrity boundaries, and every granted device capability;
- qualify DMA, storage writes, networking, sensors, and actuators only through their owning demo or hardware milestones; a CPU boot does not promote an untested peripheral;
- replay the AArch64 QEMU semantic corpus on the board where physically meaningful, labeling hardware-only differences instead of hiding them;
- provide the board evidence consumed by RP3, RP4, RP7, and RP8.

### Required checks

- the named Raspberry Pi 5 runs the isolated native vertical slice from reproducible media and preserves every declared no-write or exact-device storage boundary;
- firmware changes, wrong board revisions, unsupported page/interrupt profiles, and missing required devices fail with bounded diagnostics rather than silently selecting a nearby profile;
- physical timer, interrupt, reset, serial, and storage behavior is recorded separately from inherited QEMU evidence;
- the board can run at least two isolated components and report a bounded data-path transcript before any ROS layer is claimed.

### Planned verification target

```sh
just rpi5_boot_check
```

### Exit condition

One named Raspberry Pi 5 profile runs the verified isolated Slime vertical slice with reproducible firmware/media evidence and no unqualified device or storage claim; this physical evidence is available to the RPi5 ROS 2 demo track.

## MCU and embedded-companion boundary

Cortex-M, RV32 microcontrollers, and other systems without the admitted MMU and user/supervisor isolation baseline do not run a weakened form of this kernel. They are external devices reached through bounded userspace services.

A later companion profile may admit micro-ROS/XRCE-DDS or a smaller Zutai protocol over an exact serial, CAN, USB, or network capability. That profile must declare peer identity, types, directions, payload size, frequency, queue depth, timeout, reset behavior, and actuator authority. Disconnect, malformed traffic, reboot, and resource exhaustion become structured C8/C9 events; the companion never receives ambient graph, network, storage, or device authority.

## Verification policy

- P0 contract changes run `just contracts_check` and `just generation_check` in addition to their narrow target.
- Permanent Rust changes run the repository format and lint gates for every affected workspace.
- P1–P3 run the narrowest architecture QEMU target and then the shared semantic corpus named by the slice.
- A pass on one ISA cannot close another ISA's gate. A QEMU pass cannot close P4 or any RP physical demo milestone.
- Cross-architecture comparisons assert normalized semantic events and authenticated artifacts; they do not claim byte-identical register frames, page tables, physical addresses, or device traces.
