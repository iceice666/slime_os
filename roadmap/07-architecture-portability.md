# Architecture portability track

**Purpose:** Preserve one Slime capability/component/generation architecture across target profiles while making AArch64 and Raspberry Pi 5 the near-term product path.

**Status:** In progress — P0, P1, P2.1, P2.2, and P5 complete. P2.3–P2.6 are superseded by P5.

**Decision:** AArch64/Raspberry Pi 5 is now the near-term physical target because the current product goal is the RPi5 ROS 2 two-node demo. The existing x86-64 QEMU path remains the regression oracle for completed work until each semantic corpus is replayed on AArch64, but x86-64/Framework is no longer the product-leading roadmap. RV64 is deferred. As of P5, the AArch64 kernel-side mechanism is being substituted with upstream seL4 rather than hand-written: see [P5](#p5-sel4-microkernel-substitution), which supersedes the custom-kernel half of P2.2-P2.6 if it completes.

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

**Status:** P2.1 complete; P2.2–P2.6 superseded by [P5](#p5-sel4-microkernel-substitution), which supplies the same mechanisms from upstream seL4 instead of a hand-written AArch64 kernel. Their deliverables below are retained as the record of what the custom-kernel route required, not as open work.

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

#### Exit condition (historical)

Observed 2026-08-03 on the retired custom-kernel path; see [`devlog/2026-08-03-p2-1-aarch64-boot/`](../devlog/2026-08-03-p2-1-aarch64-boot/index.md).

That historical gate booted `qemu-system-aarch64 -machine virt` to the retired
custom kernel's EL1 path. The current `aarch64_boot_check` target validates the
seL4 product image instead: it must not be cited as current proof of the former
PL011, custom stage-1 translation-table, direct-map, heap, or semihosting path.
Those observations remain historical evidence only; current architecture claims
must name the seL4 gate and the markers it actually asserts.

No component runs and no syscall is served; those are P2.3 and P2.2. This is the
first non-x86 execution in the project, and it is QEMU only — it establishes
nothing about Raspberry Pi 5 hardware.

### P2.2 — Exception vectors, fault decoding, and `svc` entry

**Status:** Complete on the retired custom-kernel path, then superseded by P5. seL4 owns exception entry, fault decoding, and the trap instruction; `slime-root/src/fault.rs` decodes seL4's fault messages into the architecture-neutral vocabulary supervision reports, and there is no Slime trap vector or register mapping left to implement. `just sel4_root_boot_check` covers fault isolation on the product path.

**Depends on:** P2.1.

#### Deliverables

- install the EL1 exception vector table and save/restore the `UserFrame` P1 defined, preserving `x0`–`x30`, `SP_EL0`, `ELR_EL1`, and `SPSR_EL1` across entry;
- decode synchronous exception classes from `ESR_EL1` into the existing architecture-neutral `UserFaultReason` vocabulary without adding an AArch64-specific fault taxonomy;
- implement `svc #0` syscall entry against a Slime-owned register mapping, dispatching into the shared syscall body — retired: `docs/syscall-abi.md` now documents label-dispatched `seL4_Call` operations, not a trap-and-register convention;
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

#### Exit condition (historical)

Observed 2026-08-03 on the retired custom-kernel path; see [`devlog/2026-08-03-p2-2-aarch64-traps/`](../devlog/2026-08-03-p2-2-aarch64-traps/index.md).

That historical gate installed the architected 16-slot EL1 vector table at
`VBAR_EL1`, decoded an EL1 `brk` and an EL0 undefined instruction through
`ESR_EL1.EC` into the shared `UserFaultReason` vocabulary, dispatched an
`svc #0` from EL0 into the retired `kernel/src/syscall/mod.rs` body, and
observed the 31-register frame plus `SP_EL0` surviving `eret`.

None of that mechanism survives. seL4 owns exception entry, the trap
instruction, and the register frame; `just aarch64_trap_check` now resolves to
`sel4_root_boot_check`, which asserts fault isolation on the product path
(`SLIME_ROOT child fault observed task=1 role=deliberate-fault
kind=VirtualMemory { access: Write }`, then a `Fault(...)` termination and
full slot reclamation). The PL011 vector-table, `svc`/`eret` frame, and
`DAIF`-window observations remain historical evidence only and must not be
cited as current proof; current architecture claims must name the seL4 gate
and the markers it actually asserts.

### P2.3 — EL0 execution, address spaces, and isolation

**Status:** Superseded by P5. EL0 execution, per-task VSpaces, and frame reclamation are seL4 objects the root constructs (`slime-root/src/{task,child_vspace,object_allocator}.rs`); `just sel4_root_boot_check` and `just sel4_reclamation_check` observe isolation and conservation.

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

**Status:** Superseded by P5. seL4 owns the GIC and the generic timer; the root drives the platform timer and declared Notifications (`slime-root/src/{platform_timer,notification}.rs`), and components wait on native Notifications rather than a root wait set. `just sel4_root_boot_check` observes timer interrupt delivery.

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

**Status:** Superseded by P5. The root drives virtio-mmio directly (`slime-root/src/{device,virtio_blk}.rs`) and generation selection, activation, and rollback run on that path under `just sel4_generation_check`, `just sel4_rollback_check`, and `just sel4_boot_selection_check`.

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

**Status:** Superseded by P5. The C7 sample plane and the C8 fabric planes run on seL4 under their own gates (`just sel4_sample_check`, `just sel4_stream_check`, `just sel4_qos_check`, `just sel4_call_check`, `just sel4_operation_check`, `just sel4_visibility_check`); P5.3 and P5.5 record the observations.

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

## P5: seL4 microkernel substitution

**Status:** Complete — P5.1–P5.5 are observed, the custom kernel and its
legacy-only gates are retired, and the seL4 product owns the surviving runtime
contract.

**Depends on:** P0 and P1. The custom-kernel half of P2.2–P2.6 is superseded;
P2.1's AArch64 stage-0 evidence remains historical and is not re-claimed here.

**Decision:** Slime's differentiator is the capability/component/generation
model in userspace, not a hand-written AArch64 microkernel. P2.2–P2.6 each
required re-deriving exception vectors, isolation, GICv3, timers, and virtio on
a second architecture — mechanism upstream seL4 already provides under formal
verification. P5 substitutes seL4 for the custom kernel and keeps Slime's
authority model as a root task, so architecture bring-up stops being Slime's
problem. The frozen custom-kernel oracle was retained until P5.4 established
equivalent or explicitly reclassified coverage; P5.4.final then removed it.

### P5.1 — Standalone seL4 root task with generation authority

**Status:** Complete.

#### Deliverables

- pin upstream seL4 and rust-sel4 as submodules with exact commit, release, toolchain, target-spec, kernel-config, and observed-artifact hashes, all enforced by a gate that fails closed on a dirty tree;
- build a deterministic `qemu-arm-virt` image from those pins, reproducible across source paths, with an identity manifest the boot gate re-verifies;
- decode and admit the existing verified generation inside a Rust root task, deriving every child's authority strictly from declared grants;
- construct child CSpace/VSpace/TCB from a native AArch64 ELF with no untyped, CNode, VSpace, ASID, or IRQ authority in any child CSpace;
- own IPC, fault supervision, timers, and shared buffers as bounded root-task mechanism behind the existing Slime operation vocabulary.

#### Required checks

- the pinned image boots and the root task admits the generation graph while explicitly activating no legacy component image;
- a native child runs, issues a grant-derived root-mediated request, and exits cleanly; a second child faults deliberately and is observed on the supervision path;
- a real hardware timer interrupt is claimed, delivered, serviced, and acknowledged, with the monotonic counter observed to advance;
- a shared frame carries bytes both ways between root and child, a read-only mapping refuses a write, a data page refuses execution, and teardown returns every frame, mapping, and quota to zero.

#### Verification target

```sh
just sel4_root_boot_check
```

#### Exit condition (observed)

Observed 2026-08-03; see [`devlog/2026-08-03-p5-1-sel4-cutover/`](../devlog/2026-08-03-p5-1-sel4-cutover/index.md).

`qemu-system-aarch64 -machine virt,virtualization=on` boots the pinned seL4
kernel and the `slime-root` root task, which admits the 25-component generation
graph and its authority manifest, states that all 25 legacy SLIMECM images are
not activated, claims PPI 30 and observes one real timer interrupt delivered
through its bound notification and acknowledged, maps two shared regions and
exchanges bytes both ways with a
native child, has seL4 refuse both a read-only write and an execute from a data
page, runs one clean-exit and one deliberate-fault child through root-mediated
IPC and fault supervision, and tears every resource back down to `live=0` — all
asserted as ordered serial markers.

No legacy component image runs on seL4, and no Slime service graph is active:
the proof is a native fixture. That is P5.2.

### P5.2 — Native component images on seL4

**Status:** Complete.

**Depends on:** P5.1.

The generation's component payloads were the retired kernel's custom SLIMECM
images, which the root task admits but cannot load. This slice rebuilds
`components/bins` as native AArch64 ELF images against the `sel4` transport
feature and boots a declared service graph from them.

#### Required checks

- a generation whose payloads are native ELF images launches its declared components with their declared grants;
- the root service answers the operation surface those components actually invoke, with the same errors and bounds as the legacy kernel;
- an unsupported operation returns its bounded Slime error rather than faulting the caller.

#### Verification target

```sh
just sel4_component_graph_check
```

A separate image from `sel4_root_boot_check`'s. The two differ only in which
generation the root task embeds, and each gate boots the artifact it asserts
about, so neither invalidates the other's evidence by being built last. The root
task chooses its startup path by what the generation carries — loadable payloads
or not — rather than by a flag it was built with.

#### Declared graph, and what is deferred

The seL4 generation is a sibling manifest,
[`contracts/generation/v1/fixtures/sel4.zti`](../contracts/generation/v1/fixtures/sel4.zti),
not a boot profile of `valid.zti`: `resolve_boot_profile` narrows by
subtraction, so naming a component in a new profile would drop it from
`default` and change the frozen 45-slot product generation that
`just product_boot_check` and the nineteen `just boot_layout_check` pairs
guard. See [`sel4.md`](../contracts/generation/v1/fixtures/sel4.md) beside it.

It declares the five components whose entire operation surface the root
mediates: `init`, `console`, `spawn-service`, `sysinfo`, `echo-agent`.

The remaining components are deferred, each on a plane
`slime-root/src/ipc.rs::Operation::mediation` answers `Unavailable` — those
planes have no seL4 mechanism owner in this cutover, and giving them one is not
scoped in P5:

| Deferred | Blocking plane |
| --- | --- |
| `dango`, `powerbox-chooser`, `powerbox-probe` | input, directory |
| `generation-manager` and the five `generation-*` commands | generation management |
| `filesystem-service`, `directory-probe` | directory, object store |
| `storage-probe`, `storage-writer`, `storage-fault-probe`, `storage-store-probe` | block, object store |
| `recovery` | recovery |
| `sample-lender`, `sample-receiver`, the whole `fabric-*` set | C7 sample plane and C8 fabric — P5.3 |

Two limits of this slice, both P5.3's work:

- `spawn` resolves its authority from the caller's declared grants but does not
  yet construct the child, so `spawn-service` cannot start `sysinfo` or
  `echo-agent`.
- `recv`, `send`, and `wait` are root-mediated but have no handler yet. They are
  answered with a bounded error and reported as `unimplemented`, held separate
  from the `unsupported` planes above so a gap in this slice is not recorded as
  a property of the cutover. Every declared component reaches its first `recv`
  and exits non-zero; the graph runs and is served, but does not yet do work
  over channels.

#### Exit condition (observed)

Observed 2026-08-04; see
[`devlog/2026-08-04-p5-2-native-component-images/`](../devlog/2026-08-04-p5-2-native-component-images/index.md).

A generation of five native ELF component images boots its declared graph on
seL4. Every payload is target-qualified and admitted before mapping
(`elf=5 slimecm=0 wrong_target=0 unrecognized=0`); each component is built from
its own generation object, receives the grants the generation declares for it —
`spawn-service` holds exactly the two executables it is granted, every other
component holds none — binds the transfer window the loader mapped for it, and
runs. `spawn-service` completes the full shared-buffer
create/map/write/seal/unmap/release cycle through real seL4 frames against its
declared quota. An unanswered operation returns its bounded Slime error with the
caller still running, and an ungranted executable slot is refused rather than
served.

The bounded-error property is observed on the operations these components
actually reach — which are the `unimplemented` ones — and asserted statically
over the nine `Unavailable` planes, since no declared component invokes one on
this boot path. Both halves are fault-injected in the devlog entry.

### P5.3 — C7 sample plane on seL4

**Status:** Complete — P5.3.1, P5.3.2, P5.3.3, and P5.3.4 all observed.

**Depends on:** P5.2 and C7.

Replay the bounded sample plane on the seL4 root task, so the RPi5 demo's data
path does not depend on the retired kernel.

**Retitled 2026-08-04.** This heading previously read "C7/C8 data path", but its
exit condition names only C7-shaped properties and the minimal typed-fabric
slice is four tasks plus an `Operation::CapTransfer` handler — see P5.5, which
now owns that work. It is also decomposed, in the same shape and for the same
reason C7 and C8.9 were: reaching the exit condition requires four independent
state surfaces — channels, the loan plane, child construction, and death
reclamation — and one slice landing all four is not reviewable.

#### Exit condition

Two components exchange and return a payload larger than the control-message
bound over seL4, with quota exhaustion and peer death reclaiming the same
resources the x86 corpus records.

### P5.3.1 — Channel plane on seL4

**Status:** Complete.

**Depends on:** P5.2.

`Send`, `Recv`, and `Wait` were root-mediated but had no handler: every declared
component reached its first `recv` and exited non-zero, so the P5.2 graph ran and
was served but did nothing over channels. This slice makes a channel a real
object — materialized from the generation's declared grants, owned by the root,
named by a logical slot the component was granted.

#### Required checks

- every channel the generation's send/recv grants declare is materialized before
  any component runs, with each end at the slot that end's component addresses
  and carrying only the rights that end holds;
- a component blocked in `recv` is parked in the kernel and woken by its peer's
  send, receiving a payload too large for the fast message registers through its
  transfer window;
- a bounded channel refuses a send past its depth, and a capability-carrying send
  is refused outright, both as ordinary Slime errors with the caller running;
- a `wait` on an already-ready source is answered rather than parked;
- a component still parked when its peer dies is woken with a bounded error, and
  every channel, held reply, and window is reclaimed.

#### Verification target

```sh
just sel4_channel_check
```

A third image, beside `sel4_root_boot_check`'s and
`sel4_component_graph_check`'s. All three differ only in which generation the
root task embeds, and each gate boots the artifact it asserts about. A separate
generation is mechanically required rather than preferred: `init.rs` selects its
scenario with `option_env!`, resolved at compile time, so one component build
cannot serve two gates.

#### Exit condition (observed)

Observed 2026-08-04; see
[`devlog/2026-08-04-p5-3-1-channel-plane/`](../devlog/2026-08-04-p5-3-1-channel-plane/index.md).

Two components exchange bounded messages over channels the generation declared.
`init` sends a 42-byte payload — over the 16 bytes the inline registers carry, so
it crosses the transfer window — to a `console` parked in `recv`, which is woken
and prints the exact bytes. A capability-carrying send is refused, a self-edge
accepts exactly `CHANNEL_CAPACITY` messages and refuses the next, a `wait` on a
ready source is answered rather than parked, and `console` — parked again when
`init` exits — is woken by its peer's death. The graph drains to
`live=0 … parked=0 queues=0`.

Three denial arms are fault-injected in the devlog entry. The peer-death arm was
not covered by the first fixture and the injection is what found it.

Not in this slice: the loan plane (P5.3.2), child construction from a resolved
spawn grant and supervision (P5.3.3), and the composed sample-plane exit
condition (P5.3.4). A channel grant naming the bootstrap component that the boot
layout does not label is reported unplaced rather than guessed at — those are the
halves `init` brokers through spawn, which arrives with P5.3.3.

### P5.3.2 — Loan plane and generation-declared quotas on seL4

**Status:** Complete.

**Depends on:** P5.3.1.

`SharedBufferTable` already implemented loan, loan-map, return, and revoke
against real seL4 frames, but no dispatcher arm reached them, and `SHARED_QUOTA`
was a hardcoded constant with `loan_count: 0` applied to every task rather than
the `shared-buffer-budget` resource the generation carries.
`SharedBufferTable::reclaim_holder` existed but was called only from unit tests,
so a dead task's buffers, mappings, and loans were never settled.

#### Required checks

- every launched component's four ceilings are decoded from the generation's
  `shared-buffer-budget` resource, and a component the budget does not name
  holds no quota at all;
- a component loans an exact sealed subrange to a receiver it named through a
  capability, and an unsealed source is refused;
- the loan capability moves to the receiver — a move, not a copy: the sender
  cannot name it afterwards — while every other resource kind stays unmovable;
- the receiver maps only the loaned bytes, read-only, and returns the loan
  exactly once;
- each of the four quota classes refuses at ceiling+1 while the other three are
  unspent, and a third holder that took no part is undisturbed;
- every loan, mapping, region, frame alias, and in-flight capability is
  reclaimed when its holder dies.

#### Verification target

```sh
just sel4_loan_check
```

A fourth image, beside the three P5.1–P5.3.1 gates boot. Each gate boots the
artifact it asserts about, so none invalidates another's evidence by being
built last.

#### Exit condition (observed)

Observed 2026-08-04; see
[`devlog/2026-08-04-p5-3-2-loan-plane/`](../devlog/2026-08-04-p5-3-2-loan-plane/index.md).

A component loans a sealed subrange to a receiver named by capability, the
receiver maps it read-only and returns it exactly once, and each of the four
quota classes fails at ceiling+1 against limits decoded from the generation
without disturbing an unrelated holder.

The receiver is `sample-receiver`, **unmodified** — the same binary the x86
oracle's `just sample_plane_live_check` runs, with no seL4 branch. It brings
four denial arms of its own, all observed: a descriptor naming another loan, a
map past the loaned range, a writable map of a read-only loan, and a second
return. The graph drains to `loans=0 mappings=0 regions=0 transit=0 orphans=0
aliases=0`.

Capability transfer over `send` landed here rather than with P5.5, because a
loan cannot reach its receiver without it. It is the narrow form — one resource
kind, over a channel the generation declared — and P5.5's `Operation::CapTransfer`
remains the separate narrow-on-transfer operation C8.3 needs.

Five denial arms are fault-injected in the devlog entry. Two are recorded
specially: the page ceiling survives a single-site injection because the table
re-checks it, and the in-flight reclamation arm was uncovered until the fixture
was made to strand a capability deliberately.

Not in this slice: resolving a `bufferCreate` capability before admitting an
allocation, so the quota is currently the only bound — recorded as **B13** in
[`00-backlog.md`](00-backlog.md), deferred because closing it renumbers every
component's capability slots, which is P5.3.3's distribution problem.

### P5.3.3 — Child construction and supervision on seL4

**Status:** Complete.

**Depends on:** P5.3.1.

`Spawn` resolved its authority from the caller's declared grants and then
refused, so no component could start another. `SupervisionStatus`, `CapDrop`,
and `EndpointCreate` had no handler at all, and `WaitSource::Supervision`
resolved to `Unmediated` — a wait naming only it was refused outright, because
no spawn existed to mint a handle for it to name.

#### Required checks

- an executable slot the caller was not granted, and one holding authority of
  another kind, are both refused with nothing constructed;
- a grant may not exceed the rights the parent holds at that slot, and the
  executable slot itself may not be handed to the child;
- a child is constructed from the grant-resolved executable and receives its
  declared capabilities at the slots its own numbering fixes, with the channel
  end *moving* — the parent cannot name it afterwards, while the half it kept
  still works;
- a live child's handle answers "no outcome" rather than blocking; a terminated
  child's outcome is collected exactly once and consumes the handle; a live
  handle can be dropped;
- a parent parked on a child's termination is woken by that death, and every
  registration is cleared at teardown.

#### Verification target

```sh
just sel4_spawn_check
```

A fifth image, beside the four P5.1–P5.3.2 gates boot, on the same rule: each
gate boots the artifact it asserts about.

#### Exit condition (observed)

Observed 2026-08-05; see
[`devlog/2026-08-05-p5-3-3-spawn-plane/`](../devlog/2026-08-05-p5-3-3-spawn-plane/index.md).

`init` constructs two children from grant-resolved executables, hands each the
capabilities its slots name, and collects `sysinfo`'s clean exit through a
supervision handle after being woken by that child's death. Both children are
**unmodified** — the same `console` and `sysinfo` binaries the x86 oracle
builds, with no seL4 branch in either, which the gate checks against the sources
rather than inferring from the transcript.

The channel a child receives is minted at runtime through the generation's
declared `endpointCreate` grant, not declared as a graph edge: that is the
broker shape the retired kernel's `init` uses, and it is what P5.3.1's
`channel.rs` recorded as impossible until spawn existed to distribute halves
through. An endpoint grant is a **move** rather than a copy, because a channel's
queues are resolved by which task holds each end; the parent keeps only the half
it did not grant.

Four denial arms are fault-injected in the devlog entry, and one of them found a
real gap: with the supervision wake removed the gate wedges rather than passing,
but with **B13's factory check removed every gate still passed**, because no
fixture had a component that held a budget and tried to allocate without a
grant. The loan gate now names one.

Also in this slice, because the milestone's own words required it: the bootstrap
component's executable and factory slots are placed from the boot layout rather
than from a running cursor. This is the first seL4 generation to grant `init` an
executable, and it is what made the coupling observable — a cursor puts
`sysinfo` at slot 2 while `init.rs` compiles against 4, which is exactly the
positional ambiguity B10 exists to remove.

Not in this slice: the composed sample-plane exit condition (P5.3.4).

### P5.3.4 — Sample-plane composition on seL4

**Status:** Complete.

**Depends on:** P5.3.2 and P5.3.3.

Composes the slices into P5.3's stated exit condition: the `sample-lender` and
`sample-receiver` components, unmodified, running the same ordered transcript
`just sample_plane_live_check` records on x86.

#### Required checks

- both components run **unmodified** — the same binaries the x86 oracle builds,
  with no seL4 branch, checked against the sources rather than inferred from
  serial output;
- the seventeen ordered markers the x86 gate requires are observed in its order,
  and the marker list is re-read from that gate at run time so the two cannot
  drift;
- a spawned child holds the shared-buffer ceiling the generation declares for
  its component, not the deny-by-default an unnamed holder gets;
- the generation's declared spawn budget refuses a child past its ceiling, as
  `ERR_OUT_OF_MEMORY` rather than a capability error;
- every loan, mapping, region, frame alias, in-flight capability, window, and
  table is reclaimed at teardown.

#### Verification target

```sh
just sel4_sample_check
```

A sixth image, beside the five P5.1–P5.3.3 gates boot, on the same rule.

#### Exit condition (observed)

Observed 2026-08-05; see
[`devlog/2026-08-05-p5-3-4-sample-plane/`](../devlog/2026-08-05-p5-3-4-sample-plane/index.md).

The unmodified `sample-lender` and `sample-receiver` exchange and return an
8192-byte payload over seL4 — 128× the 64-byte control-message bound — running
the transcript `just sample_plane_live_check` records on x86, and the graph
drains to `live=0 loans=0 mappings=0 regions=0 transit=0 orphans=0 aliases=0`.

Two changes in the root made that possible, and both are the milestone's own
words rather than additions to it. `serve_buffer_loan` now accepts a
`RIGHT_SUPERVISE` handle at `receiver_slot` alongside the channel end P5.3.2
admitted — the retired kernel names a loan's receiver that way, and
`sample-lender.rs::RECEIVER_SLOT` is exactly that handle, so accepting it is
what lets the component run unchanged. And a **spawned** child now takes the
ceiling the generation declares for its component; before this only
root-launched components were budgeted, so a spawned lender held `DENY` and
could not allocate at all.

The peer channel is minted at runtime through the declared `endpointCreate`
grant rather than declared as an edge, because a `source == target` grant is a
loopback and yields one slot where this composition needs two halves. That is
the same mechanism `spawn-service` uses on x86, and the components cannot tell:
each receives its half at its own slot 0 either way.

**B14 is closed here**, with the denial arm its deferral reason named:
`init`'s declared budget is exactly two, so the third spawn is refused by the
generation's own number rather than by a global table size.

Four denial arms are fault-injected in the devlog entry, including the two
changes above — with either removed the gate fails rather than passing.

P5.3 is complete: all four sub-slices are observed.

### P5.5 — C8 typed fabric on seL4

**Status:** Complete — P5.5.1 and P5.5.2 both observed.

**Depends on:** P5.3.3 and C8.

Split out of P5.3 on 2026-08-04, because P5.3's exit condition names only
C7-shaped properties while its title claimed C8 as well. The typed fabric is a
different slice by size and by mechanism: its smallest honest configuration is
four tasks — `init`, `fabric-service`, one publisher, one subscriber — because
the C8.3 authority claim is that a participant never holds a route endpoint
directly but is provisioned one by the fabric from the authenticated graph. That
provisioning is `Operation::CapTransfer`, which this cutover did not mediate
until P5.5.1, and it presupposes P5.3.3's spawn-time capability distribution.

Note the threshold difference: a C7 payload crosses to a shared buffer above the
64-byte control-message bound, a C8 stream sample above `maxInlineBytes = 32`.

**Decomposed 2026-08-05,** in the same shape and for the same reason P5.3 was.
The exit condition below is C8.3-shaped — it names where route authority comes
from, not how much data a route can move — and reaching it needs one route, one
publisher, and one subscriber. Reaching the *full* C8.4 stream plane with the
components unmodified needs a second publisher for the `>MAX_MSG` descriptor
path, a stalled subscriber for KEEP_LAST eviction, and a second route for the
many-to-many fan-in: twice the graph, and none of it required by the exit
condition. Landing both in one slice would make the reviewable claim depend on
the unreviewable one.

#### Exit condition

One declared typed route carries a sample from a publisher to a subscriber over
seL4, with the route endpoints provisioned by the fabric from the generation's
declared edges, a re-delegation refused, and an undeclared participant denied.

### P5.5.1 — Narrow-on-transfer provisioning on seL4

**Status:** Complete.

**Depends on:** P5.3.3.

`Operation::CapTransfer` was root-mediated but had no handler: it fell to the
dispatcher's catch-all and answered `unimplemented`, so no component could hand
a capability to a task that already existed. That is the one mechanism a
userspace fabric needs and neither `send` nor `spawn` provides — `send` moves
only a loan, whose handle names its own recipient, and a spawn grant's
destination is a task that does not exist yet. A route role is neither: it goes
to a running task, chosen by a broker, narrowed to one direction and made
non-delegable at the moment it crosses.

#### Required checks

- a capability moves to a channel's peer with its rights narrowed to exactly the
  mask its descriptor declares, and the descriptor's declared object kind must
  be the moved capability's real kind;
- `RIGHT_TRANSFER` is dropped at the destination unless the descriptor retains
  it, so a provisioned role is non-delegable by construction rather than by
  convention, and a participant's re-delegation is refused;
- a component the generation's graph declares no edge for is denied even when it
  holds a real control endpoint and supplies the exact route strings;
- a moved endpoint's channel *holder* moves with its capability, so the receiver
  resolves a live queue rather than a capability naming nothing;
- both halves of one route are separate objects, so neither participant can
  perform the other's operation with what it holds.

#### Verification target

```sh
just sel4_fabric_check   # retired by P5.5.2; see below
```

A seventh image, beside the six P5.1–P5.3.4 gates boot, on the same rule. It was
replaced by `just sel4_stream_check` when P5.5.2 landed, which asserts a
superset of it.

#### Exit condition (observed)

Observed 2026-08-05; see
[`devlog/2026-08-05-p5-5-1-typed-fabric/`](../devlog/2026-08-05-p5-5-1-typed-fabric/index.md).

One declared `telemetry` route carries a sample from `fabric-publisher` to
`fabric-subscriber` over seL4. Both route endpoints are provisioned by
`fabric-service` from the generation's declared edges through four
narrow-on-transfer moves — the publisher's data and credit halves, the
subscriber's data and ack halves — each landing with exactly one direction
(`rights=0x1` and `rights=0x2`) and no transfer bit. `fabric-publisher`'s
re-delegation and widening are both refused, `fabric-intruder` is denied with an
empty rights mask and no capability attached despite holding a real control
endpoint, and the graph drains to `transit=0`.

Three of the four participants — `fabric-service`, `fabric-publisher`,
`fabric-intruder` — run **unmodified**. `fabric-subscriber` carries exactly one
guarded branch, because it refuses to finish until both sample forms arrive and
the `>MAX_MSG` one comes from a publisher this graph does not declare. The gate
asserts that count exactly rather than asserting absence, so the difference from
P5.5.2 is a checked fact.

**Two defects were found by this slice and fixed in it**, both latent since
P5.3.1 and neither observable from a one-source graph:

- **`recv` parked the caller** where the retired kernel's is non-blocking. A
  component that sweeps several sources before parking — which every broker
  does — froze at the first empty one, holding samples a peer was parked waiting
  for. `recv` now answers `ERR_WOULDBLOCK` and `wait` remains the only operation
  that parks, which is the oracle's own split.
- **`resolve_channel` answered `-4`** where `sys_send` and `sys_recv` both
  answer `ERR_BAD_CAP`. Components compare against the literal, so a denial arm
  testing for it read as "the denial did not fire".

**B15 is closed here**, with a six-grant spawn observed under
`just sel4_spawn_check`.

Three denial arms are fault-injected in the devlog entry. A fourth — the
transfer's *subset* test — was recorded as **uncovered** rather than claimed:
deleting it left every marker intact, because no capability this graph could
produce held transfer authority while being narrower than its kind admits.

**Superseded by P5.5.2**, in two ways. The coverage gap is closed there, and
the reasoning above turned out to be wrong: a spawn grant produces exactly that
capability, so the property was reachable all along. This slice's gate,
generation, and image are also retired there, their assertions subsumed by a
larger graph.

### P5.5.2 — The full stream plane, unmodified, on seL4

**Status:** Complete.

**Depends on:** P5.5.1.

Runs the C8.4 stream plane as the x86 oracle builds it: two publishers, two
subscribers, two routes, the `>MAX_INLINE_BYTES` descriptor and loan path, and
KEEP_LAST eviction with a stalled subscriber told exactly what it lost. Every
component unmodified, on P5.3.4's standard rather than P5.5.1's counted-branch
one.

#### Required checks

- every stream participant runs with **no** seL4 branch, asserted at the source
  rather than inferred from the transcript;
- the transcript is the x86 gate's own, re-read at run time so the two cannot
  drift into transcripts that merely resemble each other;
- one `>MAX_INLINE_BYTES` sample incurs exactly one fabric copy and one
  quota-charged receiver-bound loan per matched subscriber;
- a stalled BEST_EFFORT subscriber loses a bounded number of samples, is told
  exactly what it lost, and does not disturb an unrelated route;
- the transfer contract's subset test refuses a widening that no earlier rule
  refuses first (B17).

#### Verification target

```sh
just sel4_stream_check
```

The seventh image. It **replaces** P5.5.1's, rather than joining it — see the
exit condition below.

#### Exit condition (observed)

Observed 2026-08-05; see
[`devlog/2026-08-05-p5-5-2-stream-plane/`](../devlog/2026-08-05-p5-5-2-stream-plane/index.md).

`fabric-service`, `fabric-publisher`, `fabric-publisher-b`,
`fabric-subscriber`, `fabric-subscriber-b`, and `fabric-intruder` all run on
seL4 with **no seL4 branch in any of them**, producing 48 markers across 10
causal chains — every participant marker one the x86 gate also requires, plus a
single declared seL4-only marker for B17's arm.

**B17 is closed, and its premise corrected.** The backlog held that only a
`cap_transfer` retaining its transfer bit could produce a capability holding
transfer authority while narrower than its kind admits, and therefore that no
declarable graph could reach the subset test. That was wrong: a plain **spawn
grant** produces one, because `preflight_spawn_grants` installs the requested
mask verbatim. Deleting `rights & !source.rights` now fails this gate, which is
what P5.5.1's graph could not do.

**P5.5.1's gate, generation, and image are retired here.** Every assertion that
slice made is a subset of this one's, over a strictly larger graph, so keeping
both would have meant maintaining two images to observe one property twice.
Recorded rather than silently dropped: P5.5.1's exit condition stays observed,
by this gate.

**A third ABI divergence was found and fixed**, after the two P5.5.1 found and
on the same pattern — latent since P5.3.2, unreachable until a component
exercised the path. `shared_buffer_unmap` refused a **loan** slot where
`sys_shared_buffer_unmap` accepts one, so a receiver that mapped through
`loan_map` had no slot it could unmap with: the region belongs to the lender.

`MAX_CHANNELS` also grew 16 → 32, and `MAX_GRAPH_TASKS` 16 → `MAX_TASKS`. Both
bounds were sized against the wrong quantity — task pairs rather than route
roles — and this is the first graph large enough to reach either.

**Two scheduling races the retired kernel hides were found and fixed here
(B18).** A publisher wrote to a route it had already retired — dead code that
turned fatal once the fabric exited — and `debug_write` emitted one syscall per
byte, so the root's own markers could land mid-string and destroy a component's.
The second wore three disguises, including an apparent provisioning race, since
a corrupted marker changes what the transcript appears to say. `DebugWrite` is
now served by the root's single-threaded graph loop, so line atomicity is
structural. The gate passes ten consecutive runs.

### P5.4 — Retire the custom kernel

**Status:** Complete. Every sub-slice is closed; `kernel/` and its legacy-only
orchestration are removed.

**Depends on:** P5.3 and P5.5.

The frozen custom kernel stayed unchanged until the equivalence inventory and
all follow-on slices established the surviving contract. P5.4.final completed
the coordinated cutover: portable contract checks moved to `boot-contracts`,
runtime behavior moved to seL4 planes, seL4-supplied mechanism was explicitly
reclassified, physical NVMe/Framework qualification remains an open hardware
milestone rather than false QEMU coverage, and the legacy build/check surface
was retired with the directory.

**P5.4.1's inventory has since replaced the estimates this decomposition was
written from.** Both halves of the original reading were wrong in the same
direction — the surface is wider, and the uncovered set is larger:

- "seL4 has equivalents through C8.4" **overstates C8.1–C8.4**. C8.2 has no
  equivalent at all: `slime-root` never decodes the fabric-graph resource, so
  aggregate admission before component launch is unmet rather than partial.
  C8.1's collision rejection, C8.3's graph provenance, and C8.4's structural
  arm are each uncovered.
- "C8.5–C8.10 have none recorded" is **confirmed**, and worse than stated: two
  C8.5 assertions live inside the C8.4-gated `kernel/tests/fabric_stream.rs`,
  so they vanish without `fabric_qos_check` turning red.
- **The M5.x, M6.x, B10, and B11 class was never named here at all.** Nineteen
  closed oracle milestones with named Justfile gates have zero or partial seL4
  coverage — ten M5 storage/rollback/recovery gaps (M5.2a, M5.6a, and M5.6b are
  host or model checks and survive deletion), five M6 service gaps, two M6
  partials, plus B10 and B11. This is the larger half of the remaining work.
- Of the three figures quoted below, one was wrong: **11** Justfile recipes run
  a named `kernel/tests/*` binary, not eight (the three `fabric_*` ones sit as
  second commands inside python-first recipes). The 31 `cd kernel` recipes and
  the 34 harness importers are correct — but only **24** of the 43 checkers
  then present (44 once this slice's own gate landed)
  actually depend on the oracle; importing `harness` for `ROOT` or `load_script`
  is not dependence.

Eight of the nineteen `kernel/tests/*.rs` files have no named gate at all and
are reachable only via `just test`:
`boot.rs`, `component_image.rs`, `generation_manager.rs`, `isolation.rs`,
`kernel_foundation.rs`, `object_store.rs`, `should_panic.rs`, and
`task_reclamation.rs`. Those hold 51 architecture-neutral assertions, 32 of them
`object_store.rs`'s M5.4 storage corpus, and they are where coverage would
disappear without any gate turning red.

#### Exit condition

Every sub-slice below is complete, which carries this milestone's original
condition: every acceptance check the custom kernel guards has an observed seL4
equivalent, and `kernel/` plus its legacy-only gates are removed in one
reviewable change.

### P5.4.1 — The oracle equivalence inventory

**Status:** Complete.

**Depends on:** P5.3 and P5.5.

Maps every acceptance check the frozen oracle guards to its observed seL4
equivalent, or records an explicit gap. This is the artifact P5.4's exit
condition asks for and no one has produced: without it, "every acceptance check
has an equivalent" is a claim rather than a finding.

Must cover all three legacy surfaces named above — the direct `kernel/tests/*`
targets, the harness-mediated gates, and the eight kernel tests with no named
gate — because the third is invisible to any audit that reads the Justfile
alone.

#### Required checks

- every named `kernel/*` Justfile target is listed with its seL4 equivalent or
  an explicit gap;
- every `kernel/tests/*.rs` file is accounted for, including the eight with no
  named gate;
- every harness-mediated checker is classified as legacy-only or portable;
- lifetime-vs-live resource bounds in `slime-root` are audited as a class,
  closing [B22](00-backlog.md) — B16 and B22 were found one at a time, and the
  inventory is where the remaining ones surface if any exist.

#### Verification target

```sh
just devlog_check
```

`devlog_check` validates that every `Gates` entry resolves to a real Justfile
target and every `Roadmap` id to a real heading, which is most of what the
inventory asserts structurally. It does **not** check that a claimed equivalence
is true; that is the reviewable content of the entry. If a cross-referencing
script turns out to be needed, it is written as part of this slice rather than
assumed here.

#### Exit condition (observed)

Observed 2026-08-07; see
[`devlog/2026-08-07-p5-4-1-oracle-inventory/`](../devlog/2026-08-07-p5-4-1-oracle-inventory/index.md).

All three legacy surfaces are mapped: 11 direct `kernel/tests/*` recipes with
their per-milestone verdicts, 24 legacy checkers of the 43 then present
separated from the 10 portable harness importers and the 9 seL4 gates — this
slice's own gate makes the current totals 44 and 10 — and all 19
`kernel/tests/*.rs`
files accounted for at 151 test entities — 130 architecture-neutral semantics
seL4 must uphold against 21 custom mechanisms that die with `kernel/`, of which
51 neutral assertions sit in the eight ungated files. Every gap is named and
assigned to a P5.4.2–P5.4.10 slice below. `just devlog_check` passes.

The bounds audit closed [B22](00-backlog.md) — `ChannelTable` now reclaims
through `channel::sweep`, gated by `just sel4_crossing_check` with three fault
injections confirmed failing — and found a third table of the same shape,
`SharedBufferTable::quotas`, opened as [B24](00-backlog.md) and closed
immediately after under `just sel4_supervision_check`. With those two the
lifetime-vs-live class is closed: every bounded table in `slime-root/src` is now
live-bounded with a named freeing function, deliberately monotonic with a typed
overflow refusal, or recorded as the weaker `orphans` case.

No architectural invariant in [`README.md`](README.md) changed: the slice adds
no kernel object and no right (invariant 4), introduces no protocol
(invariant 5), and preserves rather than alters the semantics invariant 10
names — the point of the inventory is to establish that the seL4 port *has*
preserved them before the oracle proving it is deleted.

A cross-referencing script was considered and not written. `devlog_check`
resolves the entry's `Gates` and `Roadmap` ids, but no script can check that a
claimed equivalence is *true* — that is the reviewable content of the entry,
and it is re-established per slice as each gap closes.

### P5.4.2 … P5.4.n — Close the recorded gaps

**Status:** In progress. The whole C-series is closed — P5.4.4 through P5.4.10,
covering C8.2 and C8.5 through C8.10 — and the M-series is not: P5.4.2 and
P5.4.3 both need a mechanism `slime-root` does not have.

**Depends on:** P5.4.1.

One slice per uncovered oracle milestone, in the order P5.4.1 recorded. The
M-series comes first because it is the larger half and P5.4's original text
never named it; the C-series follows in ascending order, since the later ones
compose the earlier.

| Slice | Uncovered | Shape |
| --- | --- | --- |
| P5.4.2 | M5.1–M5.9 | **In progress** — ten storage/rollback/recovery gaps. Structural: five of the nine `Mediation::Unavailable` planes are M5's surface. Carries `object_store.rs`'s 32 ungated assertions, of which the eight superblock-shaped ones are now portable and host-tested in `boot-contracts` (see [`devlog/2026-08-07-p5-4-2-store-superblock/`](../devlog/2026-08-07-p5-4-2-store-superblock/index.md)). The recovery index decoder had no tests either; thirteen added (see [`devlog/2026-08-07-p5-4-2-recovery-index/`](../devlog/2026-08-07-p5-4-2-recovery-index/index.md)). The rest need a block device `slime-root` does not have |
| P5.4.3 | M6.1–M6.7 | **In progress** — five gaps (directory, dango, generation commands, powerbox, transfer) and two partials (M6.1 v2 determinism, M6.2 protocol surface). M6.7's manifest decoder had no tests at all; thirteen are now host-tested and Miri-clean with three fault injections confirmed, which is unit evidence only. See below |
| P5.4.4 | C8.2 | **Complete** — aggregate fabric-graph admission before component launch; see below |
| P5.4.5 | C8.5 | **Complete** — `just sel4_qos_check` asserts fourteen markers across nine causal chains on the `sel4-qos` plane: RELIABLE retry accounting and exhaustion, missed deadline, expired lifespan, lost liveliness lease, peer-dead retirement, and a monotonic clock the generation grants. The blocker was B28, which was `MAX_GRAPH_ITERATIONS = 512` rather than a defect; see [`devlog/2026-08-07-b28-iteration-budget/`](../devlog/2026-08-07-b28-iteration-budget/index.md) |
| P5.4.6 | C8.6 | **Complete** — `just sel4_call_check` asserts 50 markers across ten causal chains on the `sel4-call` plane: parent-vouched post-spawn introduction over copied endpoint grants; correlated inline and loaned calls; rejection, malformed reply, duplicate, cancellation, stale session, terminal backpressure, timeout, retry exhaustion, peer-death propagation, and reclamation; plus unique clean exits for all five spawned tasks and init. B25 was closed by putting `Side` in endpoint authority and binding in-flight capabilities to the receiving side rather than a task chosen at send time. See [`devlog/2026-08-08-b25-endpoint-copy-call-plane/`](../devlog/2026-08-08-b25-endpoint-copy-call-plane/index.md) |
| P5.4.7 | C8.7 | **Complete** — `just sel4_operation_check` asserts 53 markers across twelve causal chains on the `sel4-operation` plane: correlation and ordered feedback, concurrent operations that never cross-correlate, terminal-state closure, duplicate goal and duplicate result suppression, retained retrieval claimable exactly once, all three unauthorized-access denials, deterministic participant restart, cancellation races settling once, explicit-time timeout and result expiry, and peer death settling both clients' operations while an unrelated route stays live; plus four parent-vouched introductions and one clean exit per spawned task. No new root mechanism was required. See below |
| P5.4.8 | C8.8 | **Complete** — `just sel4_visibility_check` asserts 25 markers across seven causal chains on the `sel4-visibility` plane: three callers receiving three different bounded views from their exact grants, an ungranted caller whose first record is the terminal one, a bypass refusal asserted before any relay, telemetry reaching its subscriber only through the declared proxy, and proxy death as a route event the subscriber both receives and then sees in its filtered view while the unrelated route still carries. It also re-derives the oracle's two structural claims — exactly twelve serialized view records and exactly two distinct interposition traces. Found and fixed a root defect: `DebugWrite` read through the 64-byte message reader, so every 128-character record was refused. See below |
| P5.4.9 | C8.9, C8.10 | **Complete** — C8.9 needed no port: its resolution and satisfiability path is the shared host builder, exercised by construction whenever a graph-bearing seL4 fixture is built, and this slice adds the widest such fixture (five routes, four schemas, every call and operation ceiling non-zero at once). C8.10 is `just sel4_boot_check`: 44 markers across sixteen causal chains on the `sel4-boot` plane — twenty components in one generation, nineteen composition tasks in disjoint slots with no profile-dependent rewrite, the fabric splitting into three bounded route workers, eleven checked roles plus four declared role-less idles, the unauthorized probe refused as its own task, and the whole graph at rest with nothing exited. Cost two root bounds: `MAX_CHANNELS` and `MAX_TASKS` both 32 → 48. See below |
| P5.4.10 | the partials | **Done** — nine rows: six closed by gates or tests, two reclassified, one partial for a structural reason; see below |

Their individual deliverables stay unwritten until each is opened: the
inventory fixes *what* each must prove, not how, and specifying the mechanism
in advance would be the same implement-by-inference this decomposition replaces.

#### Exit condition

Every gap P5.4.1 recorded has an observed seL4 gate, each with its own devlog
entry.

### P5.4.4 — C8.2 aggregate fabric-graph admission

**Status:** Complete.

**Depends on:** P5.4.1.

`slime-root` decoded only `BootLayout` and `SharedBufferBudget`. The declared
fabric-graph resource rode along in every generation the builder emits and
nothing read it, so a graph promising more than the mechanism could deliver
would have launched — C8.2's exit condition ("per-entry plus aggregate
admission before component launch") entirely unmet rather than partially.

#### Required checks

- a generation's declared fabric graph is validated against **this root's own**
  ceilings before any component launches, using the same `boot_contracts`
  predicate the oracle uses;
- a graph exceeding any ceiling this root owns is never admitted, checked one
  field at a time so a ceiling wired to the wrong constant is visible;
- a graph that contradicts itself within every ceiling is refused;
- a generation declaring no graph is unaffected.

#### Verification target

```sh
just sel4_stream_check
```

#### Exit condition (observed)

Observed 2026-08-07; see
[`devlog/2026-08-07-p5-4-4-fabric-graph-admission/`](../devlog/2026-08-07-p5-4-4-fabric-graph-admission/index.md).

The stream plane's real two-publisher/two-subscriber graph is admitted against
`slime-root`'s ceilings and the plane runs unchanged, asserted as
`SLIME_ROOT fabric graph=admitted` and fault-injected by removing the wiring.
The other eight planes report `absent`, so the marker distinguishes "checked"
from "nothing to check". `just test_sel4_root` covers the ceiling table itself.

Not closed by this: the oracle's `kernel/tests/fabric_manifest.rs` also asserts
route-authority tuples, interposition-chain termination, and per-pair QoS
compatibility over the booted graph. Those stay with P5.4.10's partials.

### P5.4.2 — M5 storage, rollback, and recovery

**Status:** In progress.

**Depends on:** P5.4.1.

Ten M5 gaps. The largest single block P5.4.1 recorded is
`kernel/tests/object_store.rs`'s thirty-two ungated assertions.

Split by what the property actually needs. Eight of those assertions are
**superblock-shaped** — a fixed 64-byte header in a 512-byte sector, decidable
from bytes alone — and are now portable, host-tested, and Miri-clean in
`boot-contracts::store_disk`; see
[`devlog/2026-08-07-p5-4-2-store-superblock/`](../devlog/2026-08-07-p5-4-2-store-superblock/index.md).

GPT partition validation is likewise portable and likewise had no tests:
`kernel/src/storage/gpt.rs` is 336 lines of pure byte parsing — protective MBR,
both header copies, entry-array CRCs, bounds, overlap, store selection — reachable
only from the frozen oracle. It is now `boot-contracts::gpt` behind a default-off
`gpt` feature, host-tested and Miri-clean with twelve tests covering M5.4's
redundancy and recovery-precedence properties; see
[`devlog/2026-08-07-p5-4-2-gpt-validation/`](../devlog/2026-08-07-p5-4-2-gpt-validation/index.md).

The recovery index — which generation to recover to, the LBA span holding its
state objects, and a content-addressed root over every state binding — likewise
had **no tests at all**, despite eight error variants, a strict ascending-order
rule, and a SHA-256 state root. Thirteen are now host-tested and Miri-clean with
three fault injections confirmed; see
[`devlog/2026-08-07-p5-4-2-recovery-index/`](../devlog/2026-08-07-p5-4-2-recovery-index/index.md).

The store itself moved too, and P5.4.1's "the rest need a block device" was half
wrong: `ObjectStore` reads and writes through a three-method `BlockIo` trait, not a
device handle, so an in-memory disk satisfies it — including one that fails at a
chosen write. Append/commit, crash consistency at every commit boundary, slot
alternation, monotonic sequence, and content-addressed integrity are now ten host
tests in `boot-contracts::object_store`; see
[`devlog/2026-08-07-p5-4-2-object-store/`](../devlog/2026-08-07-p5-4-2-object-store/index.md).
Flush *ordering* stays uncovered, because an in-memory disk makes every write
durable immediately.

**M5's portable surface is now exhausted**, which is worth stating so the next
author does not re-audit it. Every module in `boot-contracts` with logic carries
tests, and the storage modules that remain in `kernel/src/storage/` are device-bound
by their entry points rather than by convention: `recovery.rs::reconstruct` takes a
`PciFunctionInfo` and initialises a `BlockDevice` before doing anything, and
`transfer.rs` does the same. Neither has a byte-decidable core left to lift — the
recovery *index* decoder was already the portable half and is tested in
`boot-contracts::recovery`.

What is left needs a real block device. `slime-root` has none: its object allocator
skips every device untyped (`object_allocator.rs`, `descriptor.is_device()`),
so it holds no MMIO region and no DMA-capable frame, and its only interrupt
surface is `IRQControl` for the timer. Append/commit behaviour, GPT partition
validation, the recovery paths, and the five `Mediation::Unavailable` planes
all sit behind that, which is why this slice stays open rather than being
declared blocked: the device surface is buildable, just not small.

#### Decomposed 2026-08-08

The device surface is buildable and is not one slice. Scoped against
`slime-root` and the pinned `qemu-arm-virt` machine:

- **P5.4.2a — the device resource substrate. Complete.** `ObjectAllocator` retains only
  non-device untypeds and discards physical bases even for ordinary RAM
  (`slime-root/src/object_allocator.rs`), so the root can name no MMIO region
  and no DMA address. This slice adds a device-untyped table keyed by `paddr`, a
  targeted `retype_device_frame_at`, physical-address accounting for ordinary
  allocations, a second root scratch hole for a standing MMIO mapping, and a
  second IRQ binding beside `platform_timer`'s. Every primitive it needs already
  exists: `frame_map` against the root VSpace is what `transfer_window`'s
  `with_window_mapped` does, and `bootinfo` already exposes
  `device_untyped_range()` and each descriptor's `paddr`/`size_bits`.
  Exit condition (observed): `just sel4_device_check` boots with a virtio-blk
  device attached and requires the root to identify it by register read —
  `transport=0xa003e00 device=2 vendor=0x554d4551` — while
  `just sel4_root_boot_check` requires the same probe to report `found=0` with
  no drive attached. The pair is what makes it an observation rather than a
  constant. The device's own interrupt is acquired and bound too — `irq bound … irq=79`,
  the DTB's SPI for that transport — but never acknowledged: clearing a
  level-triggered virtio line before the driver writes `InterruptACK` is the
  ordering that storms, so servicing waits for P5.4.2b. See
  [`devlog/2026-08-08-p5-4-2a-device-substrate/`](../devlog/2026-08-08-p5-4-2a-device-substrate/index.md).
- **P5.4.2b — the virtio-blk transport. Complete.** A single-queue,
  single-outstanding driver over that substrate: legacy MMIO handshake,
  three-descriptor chain, polled completion, typed errors.
  `just sel4_device_check` observes `sectors=2048` read from config space, a DMA
  read reporting the fixture's own signature (`head=534c494d`), and a write,
  FLUSH, and byte-for-byte read-back — with the write confirmed durable in the
  host image afterwards. Discovery scans all thirty-two declared transports and
  identifies the attached one by register read rather than pinning a slot.
  Two things are deliberately not done: completion is polled rather than
  interrupt-driven, because the root is also the IPC dispatcher and a
  level-triggered virtio line must be cleared before its handler is
  acknowledged; and `BlockIo` is not yet implemented for the driver, because the
  trait's consumer is the store service P5.4.2c builds. See
  [`devlog/2026-08-08-p5-4-2b-virtio-blk/`](../devlog/2026-08-08-p5-4-2b-virtio-blk/index.md).
- **P5.4.2c — the M5 gates. In progress.** `BlockTransact` now answers
  `Mediation::RootService`: the root owns the device untyped and the DMA frames,
  so it owns the driver, and the operation authenticates a `Resource::Block`
  capability, checks `blockRead`/`blockWrite` against the requested op, and
  moves one sector through the caller's transfer window. `Resource::Block` is
  placed by the same loop that places the two factories, at the boot layout's
  existing `Role::StorageCapability`. The unmediated surface is eight
  operations, not nine, and `sel4_component_graph_check` pins that.

  The userspace half is open: `just sel4_storage_check` boots generation 23 and
  observes a *component* reading, writing, flushing, and verifying sectors
  through a capability its generation granted, plus three refusal arms — a slot
  holding no device, a malformed request, and a sector past capacity — with the
  flushed write confirmed durable in the host image after the boot. That closes
  M5.2 and M5.3's transport and durability core.

  It also found a real defect: `construct_child` installed the parent's grant
  list and never the child's own declared authority, so every spawned child was
  missing what its generation granted it. Invisible for eleven planes because
  every such grant had gone to a root-launched component; the storage plane is
  the first where the spawned instance is the subject.

  **M5.4 followed, and it needed no new root mechanism at all.**
  `just sel4_store_check` boots generation 24 and observes a component
  validating a GPT, opening the content-addressed object store, retrieving an
  object by hash with its payload re-verified, appending a durable commit that
  preserves the previous root, deduplicating identical content, scrubbing every
  payload, and falling back to the older superblock when the newest is damaged.
  The implementation is `boot_contracts::{gpt, object_store}` — the oracle's own
  code, which reads through a three-method `BlockIo` trait rather than a device
  handle, so satisfying it from userspace over `BlockTransact` is the whole
  port.

  `StoreTransact` stays `Mediation::Unavailable` **deliberately**, and this is
  the load-bearing difference from the oracle. That operation names policy:
  partition selection, root choice, allocation, commit ordering. The oracle puts
  all of it in `store_service` behind syscall 7; here the root mediates sectors
  and a component does the rest. The seL4 port therefore has no store syscall,
  and a component's capability says `blockRead` rather than "the store".

  A second root defect fell out: `bring_up_block` wrote a signature to sector 1
  at boot to prove the device-reads-a-buffer DMA direction. Sector 1 is the GPT
  primary header, so the root destroyed the partition table of any partitioned
  disk before userspace ran — invisible until now because GPT redundancy
  silently recovered from the backup copy. The write is deleted; the device gate
  now asserts the image is byte-identical after the boot.

  The gate runs five fixtures. Beyond the happy path and the damaged-newest
  fallback: an interrupted append (a valid-magic truncated record past the
  committed point, which the index must not carry), conflicting GPT copies, and
  dual damaged superblocks. The last two are correct *refusals* — the component
  reports the class and exits 0, the gate pins which class, and both disks are
  hashed to prove a rejected store is never written to. That covers M5.4's
  required checks on the device plane.

  **M5.6 followed the same way.** `just sel4_rollback_check` boots generation 25
  and observes a component walking the whole transition model on two durable
  BootState slots: stage a pending generation with two attempts, consume both
  durably (the oracle's `2 → 1 → 0`), find them exhausted, roll back to
  known-good, confirm rollback idempotent, refuse promotion with a wrong running
  identity or a stale release, and promote the running generation. Every commit
  is older-slot-first and the probe re-reads the other slot after each one, so
  the M5.6 invariant — no transition overwrites the only valid root — is checked
  rather than assumed.

  That slice also moved `select_bootstate` out of `stage0`, which depends on
  `uefi` and so was unreachable from a component, into `boot-contracts` beside
  the record it selects. The rule had **no tests**; it has six now.

  **M5.9 closed the same way, and its second half is the interesting one.**
  `just sel4_recovery_plane_check` boots generation 26 with *two* disks: the
  recovery target, and a guard disk no capability the component holds names. The
  component refuses two corrupt BootState slots, decodes a signed recovery
  index, retrieves and re-hashes every state object in its closure from the
  content-addressed store, reconstructs a bootable root into both slots
  idempotently — and its attempt to reach the guard disk is refused, with the
  guard image hashed before and after to prove nothing reached it. M5.9 requires
  reconstruction to modify no device it was not explicitly granted; the marker
  proves the component asked, and the hash proves the capability model held.

  The oracle gates this behind a syscall requiring `GenerationControl` plus a
  selected block capability. Here the capability *is* the gate.

  What remains: M5.2/M5.3's fault-injection arms (descriptor recovery, reset,
  stale completions, interrupted flush — the `contracts/block/v1` flags exist
  and nothing honours them); M5.6's interruption injections and M5.9's
  interrupted reconstruction, which both need the device write path to fail at a
  chosen point the way `object_store`'s host tests do with a mock disk; M5.6's
  state policies and GC; and Ed25519 signature verification on the recovery
  index, which currently trusts the index on the disk. See
  [`devlog/2026-08-08-p5-4-2c-storage-plane/`](../devlog/2026-08-08-p5-4-2c-storage-plane/index.md)
  and
  [`devlog/2026-08-08-p5-4-2c-object-store/`](../devlog/2026-08-08-p5-4-2c-object-store/index.md),
  and
  [`devlog/2026-08-08-p5-4-2c-rollback-plane/`](../devlog/2026-08-08-p5-4-2c-rollback-plane/index.md),
  and
  [`devlog/2026-08-08-p5-4-2c-recovery-plane/`](../devlog/2026-08-08-p5-4-2c-recovery-plane/index.md).

**A device-free partial was considered and rejected.** A RAM-backed `BlockIo`
would run the store algorithm on seL4 and prove the IPC wiring, but every
property that distinguishes M5 from its host tests — flush ordering, persistence
across a fresh boot, DMA and descriptor ownership, reset and stale-completion
recovery, durable pending-attempt decrement, repair-versus-guard isolation —
needs a real device. It would be a second fake storage substrate beside the
in-memory one `boot-contracts` already has.

**M5.7 is out of scope for all three.** NVMe is a different transport and its
exit condition additionally requires an observed physical Framework boot.

#### Exit condition

Every M5 gap has an observed seL4 gate, including the store's behaviour under
interruption at each append/commit boundary.

### P5.4.3 — M6 service, directory, and transfer

**Status:** Complete — M6.1 through M6.7 are all gated on seL4.

**Depends on:** P5.4.1.

Five gaps (M6.3 directory, M6.4 dango, M6.5 generation commands, M6.6 powerbox,
M6.7 transfer) and two partials (M6.1 generation-v2 determinism, M6.2 protocol
surface).

The transfer manifest decoder — the format carrying a generation, its object and
state tables, its release record, and its metadata across a persistence
boundary — had **no tests at all**, despite defining seven error variants and a
self-excluding SHA-256 over the whole manifest. Thirteen are now host-tested and
Miri-clean, with three fault injections confirmed; see
[`devlog/2026-08-07-p5-4-3-transfer-manifest/`](../devlog/2026-08-07-p5-4-3-transfer-manifest/index.md).

**M6.5 is now closed.** `just sel4_generation_check` boots generation 27 and
observes an *unprivileged* client driving list, inspect, stage, select, and
rollback through a management service that holds the plane's only block
capability — plus every refusal, each checked against the disk image rather than
only its status, because "fail before BootState changes" is a claim about bytes.
The client's own direct `BlockTransact` is refused, and not by a rights check:
it was spawned with one RPC endpoint, so no slot it holds names a device. It
knows the BootState format perfectly well and still cannot write it.

This was the first seL4 plane with a privileged *service* rather than a probe,
and that shape exposed two defects. An authority probe on a live endpoint
consumed the client's first request — a probing `recv` on a slot whose peer
sends traffic is not the same operation as one on a run token — and the
service's `ERR_PEER_DEAD` never arrived because init still held copies of both
queue ends. See
[`devlog/2026-08-08-p5-4-3-generation-plane/`](../devlog/2026-08-08-p5-4-3-generation-plane/index.md).

**M6.3's mechanism half is now closed**, and it is the first P5.4 slice whose
answer was "the root must own it". Every earlier one moved policy *out* of the
kernel; M6.3 splits. A directory's *contents* are a filesystem component's
business over the object store, but the capability is unforgeable shared state
with an atomic transition, so `slime-root` now carries `Resource::Directory`,
a `ScopeTable`, and the three operations. `just sel4_directory_check` observes a
component deriving narrower views that can neither escape their scope nor widen
their rights, being refused a stale commit and a scoped one, and seeing its
commits through every view of the shared namespace — with the root's own records
counted, so a refusal reported but not honoured fails the gate.

That left the unmediated surface at five operations, down from eight.

It also caused a regression worth remembering: inlining a 128-byte scope into
`Resource` grew the capability tables from ~96 KiB to ~432 KiB and cost the root
its stack, which surfaced as a cap fault in the *loan* plane. An enum copied
into a fixed-size table pays its largest variant everywhere; the scopes are
interned now. See
[`devlog/2026-08-08-p5-4-3-directory-plane/`](../devlog/2026-08-08-p5-4-3-directory-plane/index.md).

**M6.7 is blocked on a device-mapping limit, and the limit is now known
precisely.** The transfer plane needs two block devices at once — a source it
may only read and a receiver it may write — and `slime-root` brings up one.
`Resource::Block` now carries a device index and the root keeps a
`BlockDevices` table, so the *authority* model is ready; what is not is the
mapping. QEMU packs eight virtio-mmio transports into one 4 KiB granule, so two
attached disks land at `0xa003e00` and `0xa003c00` — the same page. seL4's
retype is monotonic and `frame_map` takes the frame once, so the second
transport cannot map it again, and `VirtioBlock` owns its `DeviceRegion`
outright.

The fix is a borrowed granule handle: one owner maps the page, and a second
driver reads its registers at its own offset through a handle carrying no frame
capability. That was prototyped and reverted here rather than half-landed — it
touches `DeviceRegion`, `VirtioBlock`, and the probe's region bookkeeping, and
it deserves its own slice with its own gate rather than riding along inside
M6.7's.

**M6.3 is now closed on both halves.** `just sel4_filesystem_check` boots
generation 29 and runs the **oracle's own `directory-probe`, unmodified** —
shared with `just directory_check` — against a seL4 filesystem service. It
resolves names, survives an interrupted root transition, commits a new one, and
derives a narrowed subdirectory, without knowing the service exists. M6.3's
userspace half is policy, and policy ports; what changed underneath is that
object bytes come from `boot_contracts::object_store` over a granted block
capability rather than from a kernel `store_transact`.

The client hands its *own* directory view to the service with every request, so
the service acts with the caller's authority rather than its own — which is what
forced `objectKindDirectory` into `contracts/capability-transfer`, the schema
change M6.6 also needed. Three defects fell out: `transferable = true` was
reaching the authority but being dropped by the placement mask; the send path
gated by kind and excluded directories; and a gate control had gone stale,
mutating a literal `slots=2` that the channel plane's layout had outgrown, so
the control silently stopped controlling. See
[`devlog/2026-08-08-p5-4-3-filesystem-plane/`](../devlog/2026-08-08-p5-4-3-filesystem-plane/index.md).

**`InputRead` is now mediated too**, which took two things: a `Resource::Input`
gated on `RIGHT_INPUT_READ` over a per-generation key script — the same
scripted source the oracle installs in `bootstrap`, because the pinned QEMU
profile has no keyboard and a gate needs a deterministic session — and a fix to
`resolve_wait_source`, where `WAIT_KIND_INPUT` mapped to `WaitTarget::Unmediated`
and was therefore *never ready*. A Dango session waiting on input would have
parked forever, and it would have looked like a hung component rather than an
unhandled wait kind. Input is always ready, because the root reads the script
synchronously.

That leaves **four** unmediated operations, down from nine at the start of
P5.4: `StoreTransact`, `RecoveryReconstruct`, `GenerationTransact`, and
`GenerationReceive` — and the first three are unmediated *by design*, because
each names policy that now runs in userspace.

What remains:

- **M6.4 (dango) is closed.** `just sel4_dango_check` boots generation 30 and
  observes a scripted console session resolving two commands through the
  generation's profile and launching both through the spawn service — the second
  carrying a derived working directory and a stdin endpoint — with an undeclared
  command denied at resolution and a malformed line a parse error. Every
  component is the oracle's, unmodified.

  B30 had three causes, all in the root and all about where a component's
  capabilities land: `construct_child` never placed a child's declared
  *executables*, so a spawned spawn service refused every request; declared
  authority was placed in a fixed *kind* order that no two multi-kind components
  could agree on; and `is_transferable` refused endpoints by kind, so a shell
  could not give a child its stdin. Both placement paths now walk the
  generation's own grant order, which is what the oracle does. See
  [`devlog/2026-08-08-p5-4-3-dango-plane/`](../devlog/2026-08-08-p5-4-3-dango-plane/index.md).
  Building it exposed four more root defects, all fixed and gated: `WAIT_KIND_INPUT`
  resolved to a wait target that is *never ready*, so a component waiting on
  input parked forever; saved reply CSlots were emptied but their indices never
  reused, exhausting the CSpace after 1220 calls; the boot and spawn placement
  paths disagreed on slot order, so one component found its device at different
  slots depending on how it started; and `RIGHT_TRANSFER` was being read as a
  resource kind, handing a namespace view to any component with a transferable
  grant of any kind. The last two were caught by the *filesystem* and *loan*
  gates, and `just sel4_input_check` now covers the input mechanism on its own
  so a defect there is distinguishable from one in the shell. See
  [`devlog/2026-08-08-p5-4-3-input-mediation/`](../devlog/2026-08-08-p5-4-3-input-mediation/index.md).
- **M6.6 (powerbox) is closed.** `just sel4_powerbox_check` boots generation 32
  and observes a chooser holding directory authority the requester lacks hand
  over exactly one narrowed view on a selection gesture, with a provenance
  record — and deny a request exceeding its own authority, refuse derivation
  past the granted scope, and mint nothing on cancellation. The gate counts
  capabilities crossing the channel rather than trusting the requester's
  refusal assertions: three requests are made and only one may carry anything.
  Both components are the oracle's, **unmodified**, and M6.6 needed no new
  mechanism — the directory capability, its transfer kind, and `InputRead` all
  landed in the two slices before it.

  It did surface the third placement-order defect in two slices:
  `powerbox-chooser.rs` reads a directory at slot 1 and input at 2, and the
  order had been set input-first to satisfy `dango.rs`. The order is now fixed
  in both paths with a comment saying it is an ABI. The underlying problem is
  that a non-bootstrap component's slot layout is *implicit* — the boot layout
  already solves this for the bootstrap component as declared, fixture-checked
  data, and extending it would turn a class of boot failures into build
  failures. See
  [`devlog/2026-08-08-p5-4-3-powerbox-plane/`](../devlog/2026-08-08-p5-4-3-powerbox-plane/index.md).
- **M6.7 (transfer) is closed.** `just sel4_transfer_check` boots generation 33
  with two devices — a read-only source carrying the manifest and a writable
  receiver — and observes a generation crossing: digest, object closure, and
  travel policy all verified before any write, staged pending without disturbing
  the known-good root, and promoted only on health confirmation. Both images are
  compared from the host afterwards, and the source is byte-identical.

  B29 is resolved. `device::MappedGranule` is a borrowed view carrying a base
  and no capability, so the two QEMU transports that share a 4 KiB page can both
  be driven — one owner maps it, the second reads its registers at its own
  offset. Two further defects surfaced: declared placement hardcoded device 0,
  so a component holding two devices reached one twice; and placement
  intersected the component's *union* of rights rather than the grant's own, so
  the read-only source came out writable and accepted a write. The plane's first
  run passed the milestone's refusal arm only by accident of ordering. See
  [`devlog/2026-08-08-p5-4-3-transfer-plane/`](../devlog/2026-08-08-p5-4-3-transfer-plane/index.md).

#### Exit condition

Every M6 gap has an observed seL4 gate, including a transfer that actually
crosses a boundary rather than a manifest that merely decodes.

### P5.4.5 — C8.5 reliable, retained, and timed QoS

**Status:** Complete — `just sel4_qos_check` asserts fourteen markers across nine
causal chains on the `sel4-qos` plane, and the plane reaches
`[init] fabric stream complete`. The blocker was B28, which turned out to be
`MAX_GRAPH_ITERATIONS = 512` rather than a defect: the QoS plane needs between 512
and 768 root round-trips. See
[`devlog/2026-08-07-b28-iteration-budget/`](../devlog/2026-08-07-b28-iteration-budget/index.md).

**Depends on:** P5.4.1.

P5.4.1 recorded C8.5 as uncovered on seL4. Half right: no gate asserted a QoS
property, but three were already running, because the QoS logic lives in
`fabric-service` and the stream plane boots it unmodified. Matching before
data, bounded loss under a stalled subscriber, and peer death as a distinct
event are now asserted by `just sel4_stream_check`; see
[`devlog/2026-08-07-p5-4-5-qos-arms/`](../devlog/2026-08-07-p5-4-5-qos-arms/index.md).

That plane now exists. An eleventh image, `sel4-qos`, is the stream graph at
generation 19 plus one runtime-minted monotonic-time channel, granted to
`fabric-service` at its `TIME_SLOT` 9 and to `fabric-publisher-b` at its
`TIME_SLOT` 3. With the clock advancing, three arms that had been unreachable
fire: **bounded RELIABLE retry accounting, deadline miss, and liveliness loss**.
B25 does not block it — every capability the plane needs, the clock included, is
a spawn grant, so the post-spawn introduction P5.4.6 is stuck on never arises.

That retained-head gap is now closed, and by a fixture declaration rather than
any reordering: `fabric-publisher-b`'s *diagnostics* participant is the one
sample sent inline and first, so declaring it `retained` with `retainedDepth = 2`
gives the fabric an inline retained head independently of publisher timing. Two
experiments had already ruled scheduling out — removing a yield loses the working
arms, and a bounded run of 8 or 64 closes the race but hangs `publish_large`.

With that, **five** arms are observed: RELIABLE retry accounting, retry
exhaustion, deadline miss, lifespan expiry, and liveliness loss. The clock driver
runs its full seven-step advance to `done`, and no configured component fails —
the transcript's `fail:` lines are the unconfigured root-launched instances the
stream gate budgets one of each for.

It still does not reach `[init] fabric stream complete`, and that is now backlog
**B28**, bisected to one fixture field: the same `retained` diagnostics
declaration that buys two of those arms is what stops `fabric-publisher` — a
different component on a different route — from ever taking its parked role
reply. Flipping that participant back to `volatile` wakes it and loses two arms;
reducing the clock to a single advance changes nothing, so it is not starvation
behind the clock. The committed fixture keeps `retained` because it observes
strictly more and neither setting reaches the marker. Lease and tie ordering sit
past that point.
No gate is registered while the plane cannot reach its final marker. See
[`devlog/2026-08-07-p5-4-5-qos-clock/`](../devlog/2026-08-07-p5-4-5-qos-clock/index.md).

One inversion is already recorded: P5.4.10 made an incompatible QoS pair a
*refusal at admission*, correct for a root with no QoS plane, but it means the
runtime event C8.5 requires is unreachable here until this slice lands. The
call site in `slime-root/src/generation.rs` says so.

#### Exit condition

Every item in C8.5's required-checks list has an observed seL4 gate.

### P5.4.6 — C8.6 bounded native calls

**Status:** Complete.

**Depends on:** P5.4.1.

The `sel4-call` image carries one `ParameterCall` route with two clients, one
server, and a capability-routed time source. `init` mints the four authenticated
control pairs plus two private phase pairs, spawns the broker and participants,
then transfers each participant's supervision handle to the broker over that
participant's control channel. The parent, not the participant, therefore
vouches for the identity the broker admits.

B25 closed the portability gap at the capability model. An endpoint capability
now carries `Resource::Endpoint { channel, side }`, so a spawn grant is the same
non-consuming narrowing copy as every other grant. `ChannelTable` stores queues,
not one task holder per end; queue resolution follows `Side`. Capability transit
binds attached authority to the receiving side and lets the task that dequeues
the bytes collect it, preserving one queue-delivery decision when an end has
co-holders. Operations that need a concrete task identity, such as channel-routed
loan creation, require a unique opposite-side holder and refuse ambiguity.

The scenario's clock is causal rather than schedule-dependent. Phase 1 and 2
advance only the timeout/retry boundaries; the client sends phase 3 only after it
has observed the server's peer-death terminal, and phase 3 is a completion barrier
that does not advance time. The time component probes slot authority directly so
the generation-launched unconfigured copy parks while the runtime-spawned copy
drives the scenario; if phase 1 is already queued, that probe consumes it as the
first receive instead of losing it.

`just sel4_call_check` is the standing gate. It requires 50 markers across ten
causal chains, counts exactly three parent-vouched supervision introductions,
requires the non-idempotent request to execute exactly once, derives all five
spawned task ids from the root's records, and requires one `status=0` exit for
each plus init. The same checker rejects root, graph, component, capability
transfer, fault, panic, and wedge markers. `just sel4_gate_control_check` covers
it in the global registry — 12 gates rejecting 535 mutated transcripts and
layouts — with the call gate's own 71 mutations among them.

B25's representation change rewrote marker text four sibling gates read, so
every seL4 plane gate was re-run rather than the call gate alone. That found one
lost assertion and one root defect: the spawn gate's per-slot distribution
marker had been deleted with the move semantics instead of replaced, and
`ChannelTable::live_queues` counted entries no capability table names, which the
retired per-end task cache had hidden. Both are fixed and gated; see the
devlog's `## Corrections`.

See
[`devlog/2026-08-08-b25-endpoint-copy-call-plane/`](../devlog/2026-08-08-b25-endpoint-copy-call-plane/index.md).

#### Exit condition

Every item in C8.6's required-checks list has an observed seL4 gate.

### P5.4.7 — C8.7 bounded native operations

**Status:** Complete.

**Depends on:** P5.4.1 and P5.4.6.

A twelfth image, `sel4-operation`, carries generation 20: the `navigation`
operation route with two clients, a supervised replacement for the second, a
server, and a capability-routed clock, plus client A's private `nav-backup`
route. `init` mints five authenticated control pairs, spawns the graph, and
transfers each participant's supervision handle to the broker over that
participant's own channel — P5.4.6's parent-vouched composition, which is why
this slice depends on it rather than only on the inventory.

**The broker and all five participants are the oracle's binaries, unmodified.**
The generation sets the oracle's own `SLIME_FABRIC_OPERATION_CHECK` alongside its
seL4 flag, so `fabric-service` and the five `fabric-op-*` components compile
identically for both planes; only `init`'s composition differs. That is the
property the gate exists to demonstrate, and it is why no `||` selector was added
to any participant.

**No new root mechanism was needed.** C8.7 is userspace composition over
primitives `slime-root` already answers — spawn, endpoint mint, channel IPC and
readiness, capability transfer and drop, supervision, transfer-window staging.
The nine `Mediation::Unavailable` operations are storage, directory, input, and
generation transfer; the operation plane calls none of them. The graph's
operation ceilings (`inFlightOperations`, `retainedSamples`, `eventDepth`) were
already decoded and admitted by `boot-contracts::fabric_graph`; the broker sizes
its fixed arrays from them.

Two composition facts are specific to this plane. The restart replacement is a
**declared identity** — its own component, route participant, and control grant —
so the broker admits it on a channel the dead participant never held while
keeping the authenticated client index, correlation high-water mark, and retained
results. And a private release barrier orders it: the replacement is spawned
early so the broker has a channel to park on, but blocks until `init` releases it
after the whole graph exists, so its role request cannot overtake the retained
result it is supposed to find.

`just sel4_operation_check` is the standing gate. It requires 53 markers across
twelve causal chains, counts the replacement's provisioning to require exactly
one, derives all six spawned task ids from the root's own records, requires one
`status=0` exit for each plus init, and requires exactly four parent-vouched
supervision introductions. `just sel4_gate_control_check` covers it in the global
registry at a pinned 53 markers — 13 gates rejecting 610 mutated transcripts and
layouts — and `just sel4_boot_layout_check` freezes its eight-row table.

What the gate does not claim: it does not re-derive C8.7's semantics. The markers
it matches are emitted by the oracle's own broker and participants. What it
establishes is that those semantics hold on `slime-root` under a composition the
seL4 capability model forces to differ.

See
[`devlog/2026-08-08-p5-4-7-operation-plane/`](../devlog/2026-08-08-p5-4-7-operation-plane/index.md).

#### Exit condition (observed)

Every item in C8.7's required-checks list has an observed seL4 gate, with one
recorded limit: the fourth check's "leaves unrelated **stream**, **call**, and
operation routes live" is proven here for an unrelated *operation* route only,
because this graph declares no stream or call route. A graph carrying all three
at once is C8.10's shape and belongs to P5.4.9.

### P5.4.8 — C8.8 filtered introspection and declared interposition

**Status:** Complete.

**Depends on:** P5.4.1.

A thirteenth image, `sel4-visibility`, carries generation 21: the stream graph
plus one declared interposition, with `fabric-intruder` on the telemetry
subscriber's chain. The broker and all five participants are the oracle's
binaries unmodified — the generation sets `SLIME_FABRIC_VISIBILITY_CHECK`
alongside its seL4 flag — and only `init`'s composition differs.

**Each participant is spawned with exactly one capability: its own control
endpoint.** That is stronger than the call and operation planes need and is what
makes this plane's authority claims mean something: the broker mints every route
half itself and hands out narrowed, non-delegable roles at provisioning time, so
"the proxy relays only its declared route" is a statement about what the broker
transferred rather than about what the parent withheld. No supervision handle is
delegated, because nothing in this graph names a task.

**The chain is profile-borne.** Every participant declares
`interposition = []`; the `sel4` profile carries
`telemetry / fabric-subscriber → [fabric-intruder]`, which
`resolve_fabric_graph` applies. That mirrors the oracle's own `visibility`
profile rather than inlining the chain, and the admission marker's
`interpositions=1` is asserted so a silently dropped chain fails the gate rather
than admitting a direct edge where the generation declared a hop.

`just sel4_visibility_check` requires 25 markers across seven causal chains and
re-derives the oracle's two structural claims: the composition emits exactly
twelve serialized view records and exactly two interposition traces that differ
from each other. It also requires zero component failures inside the composition
window, which is how the real participants are separated from the unconfigured
instances the root launches. `just sel4_gate_control_check` covers it at a
pinned 25 markers — 14 gates rejecting 653 mutations — and
`just sel4_boot_layout_check` freezes its eight-row table.

**This slice found a root defect.** `DebugWrite` read its staged payload through
the *message* reader, bounded by `MAX_MESSAGE_BYTES` at 64. The visibility
broker prints each 64-byte record as 128 hex characters, so every view and trace
record was refused as `InvalidLength` and only its prefix reached the transcript
— on a boot where the scenario itself was entirely correct. A diagnostic line is
not a message: it crosses no channel and is bounded by nothing the IPC contract
states. The arm now reads with `read_staged_array`, the same 1 KiB wide reader
the spawn-grant array already crosses this window through. No earlier plane
could have found it; every marker the other twelve gates assert is under 64
bytes.

See
[`devlog/2026-08-08-p5-4-8-visibility-plane/`](../devlog/2026-08-08-p5-4-8-visibility-plane/index.md).

#### Exit condition (observed)

Every item in C8.8's required-checks list has an observed seL4 gate, with one
recorded limit: the fourth check's byte-identical-across-runs half is inherited
rather than re-observed. The oracle boots its profile twice and compares the
records byte-for-byte; this gate boots once and asserts their count and the
distinctness of the two traces. A repeat-boot comparison would close it.

### P5.4.9 — C8.9 typed full-profile closure and C8.10 full-graph bootstrap

**Status:** Complete.

**Depends on:** P5.4.1, and P5.4.6 through P5.4.8 for the planes it composes.

#### C8.9 needed no port

C8.9's substance is host-side: one canonical resolved graph feeding both the
authenticated bytes and the userspace tables, with every declared limit checked
against the fabric holder's quota, the channel bound, and the capability layout.
`build_sel4_generation` calls the same `resolve_fabric_profile` and
`render_fabric_profile_rust` every x86 profile calls, so **every** graph-bearing
seL4 fixture already exercises it, and `just data_fabric_profile_check` boots
nothing at all.

What this slice adds is scale rather than a mechanism: generation 22 declares
five routes, four schemas, fifteen participants, and every call and operation
ceiling non-zero at once. A set of limits individually legal but mutually
unsatisfiable in that combination fails the build rather than the boot, which is
C8.9's third required check applied to the widest graph the repo declares.

#### C8.10 is `just sel4_boot_check`

A fourteenth image, `sel4-boot`, carries generation 22: the stream, call, and
operation planes, an unauthorized probe, a declared interposition proxy, and a
filtered-introspection client, all launched concurrently in disjoint slots with
no profile-dependent rewrite. `init` mints sixteen control pairs and spawns the
fabric plus all sixteen participants; `fabric-service` then spawns its two route
workers itself, because the declared wait peaks are stream 8, call 7, operation 9
against `MAX_WAIT_SOURCES = 9` and one combined task would have to poll.

The gate requires 44 markers across sixteen causal chains, exactly one init
layout report with every slot distinct and strictly under the ceiling, both
spawning parents with no component spawned twice, eleven checked roles, four
declared role-less idles by name, and the probe refused from both sides.

**Its lifecycle check is the inverse of every other seL4 gate's.** The exit
condition is *idle*, so the assertion is that **no** composition task exited: a
task that terminated would mean the graph finished rather than came to rest.

One structural difference from the oracle. Its layout numbers both halves of all
sixteen control channels, because its kernel materializes a declared channel into
the bootstrap component's layout slots; this root numbers a launched component's
declared ends from its own cursor, so a declared control arrives at a slot no
`FABRIC_FIRST_CONTROL_SLOT + index` describes — observed directly, with the
fabric receiving cursor-numbered ends and both worker spawns failing. `init`
mints them instead, as every seL4 plane since P5.4.6 does, and
`SEL4_BOOT_LAYOUT` is therefore 21 rows against the oracle's 53.

#### Two root bounds were raised

`channel::MAX_CHANNELS` and `task::MAX_TASKS`, both 32 → 48, both sized against
single-plane graphs and both B28's class — a table sized to a workload rather
than to an invariant. The peaks are 37 live channels (sixteen participant
controls, fourteen stream role channels, three call, four operation) and 37 live
tasks (twenty root-launched instances plus init's seventeen children). Raised to
48 rather than 37 on B28's rule: neither fails cleanly, so a bound raised to the
first passing number moves again with a worse symptom next time.

Two gates caught the consequences, which is what they are for.
`sel4_crossing_check` reads both `MAX_CHANNELS` and `CHANNEL_LOOP_PAIRS` from
source and refuses `pairs <= bound`, so raising the bound alone would have left
it passing while proving nothing; the loop moved 33 → 49.
`sel4_root_boot_check`'s pinned reclaimed CSlot ranges shifted by 7 because the
larger static tables move the allocator's cursor; the width-and-adjacency
property is unchanged and the pins were updated with that distinction recorded at
the assertion.

See
[`devlog/2026-08-08-p5-4-9-full-graph-boot/`](../devlog/2026-08-08-p5-4-9-full-graph-boot/index.md).

#### Exit condition (observed)

Every item in C8.9's and C8.10's required-checks lists has an observed seL4
gate, with one recorded limit: this plane provisions and rests rather than
carrying traffic. That is C8.10's own exit condition — "healthy blocked idle with
no traffic" — and per-plane traffic is covered by the stream, call, operation,
and visibility gates against the same unmodified brokers.

### P5.4.10 — The recorded partials

**Status:** Done. Nine rows in the table below: **six** closed by gates or
tests (the `component_image.rs` segment corpus, C8.1's tag collision, C8.2's
live-graph assertions, C8.3's graph provenance, C8.4's structural arm, and
B10's seL4 layout fixtures); **two** reclassified as needing no seL4 gate, with
the evidence and the conditions that would reopen them (C7.1's retained-v2 arm,
B11's product-vs-test pair); and **one** — `task_reclamation.rs` — carried as
far as a monotonic CSlot allocator permits, which is exact range accounting
rather than the per-cycle drift its three properties measure.

Each row's reasoning is in its own devlog entry rather than summarised here,
because two of the nine resolutions are "this cannot happen on this path" and
that claim is only worth what its evidence is.

**Depends on:** P5.4.1.

The gaps P5.4.1 recorded that are each too small for their own slice, collected
so none is lost. Unlike P5.4.2–P5.4.9 these do not map one-to-one onto oracle
milestones; they are the residue of milestones otherwise covered.

| Partial | State |
| --- | --- |
| `component_image.rs`'s malformed segment corpus | **Done** — `boot_contracts::component_image::validate_segments`, eleven host tests under `just test_host` and `just miri`; see [`devlog/2026-08-07-p5-4-10-segment-corpus/`](../devlog/2026-08-07-p5-4-10-segment-corpus/index.md) |
| C8.1 collision rejection | **Done** — `distinct_schemas_may_share_no_type_tag` pins the rule (both halves); it was implemented in `FabricGraph::decode` and tested by nothing. See [`devlog/2026-08-07-p5-4-10-collision-and-provenance/`](../devlog/2026-08-07-p5-4-10-collision-and-provenance/index.md) |
| C8.2 route-authority tuples, interposition termination, per-pair QoS | **Done** — the aggregate half closed as P5.4.4; membership and interposition are enforced by `FabricGraph::decode`, and per-pair QoS is now refused at admission. See [`devlog/2026-08-07-p5-4-10-qos-pair-admission/`](../devlog/2026-08-07-p5-4-10-qos-pair-admission/index.md) |
| C8.3 graph provenance | **Done** — admission refuses a graph naming a component the generation does not declare (`GenerationError::UndeclaredFabricParticipant`), across all three identity fields the resource carries: the fabric host, every participant, and every interposition hop. A participant the manifest dropped fails the boot closed instead of surfacing as a control endpoint that never arrives. Checked against the generation's component names, *not* against the fabric's `@generated` `FABRIC_CLIENTS` table, which lives in a crate the root does not link — both derive from the same fixture, so the check catches the drift that motivates C8.3, but it is not a direct comparison of the two artifacts. See [`devlog/2026-08-07-p5-4-10-collision-and-provenance/`](../devlog/2026-08-07-p5-4-10-collision-and-provenance/index.md) |
| C8.4's structural arm | **Done** — the admission marker carries the shape the graph declares (`schemas=/routes=/participants=/interpositions=`) and `sel4_stream_check` asserts it, covering the fan-out half; the bounds half was closed by P5.4.4's `validate_against` wiring. See [`devlog/2026-08-07-p5-4-10-graph-shape/`](../devlog/2026-08-07-p5-4-10-graph-shape/index.md) |
| C7.1's retained-v2 rollback arm | **Reclassified — needs no seL4 gate.** A v2 generation names its own kernel object, so a rollback boots the v2-era kernel rather than `slime-root`; and v2 predates the ELF component revision, so in practice every payload it carries is a SLIMECM image this root has no loader for — chronological rather than enforced, since no code couples the generation format version to the image revision, which is why the kernel-object argument above is the load-bearing half. The decode path stays host-tested in `boot-contracts` (`retained_v2_generation_passes_stage0_admission` and four siblings). Booting one here would assert that an unloadable graph is reported unloadable, which `sel4_root_boot_check`'s `slimecm=[1-9]` marker already does |
| B10's seL4 layout fixture | **Done** — `just sel4_boot_layout_check` freezes all eight plane layouts; see [`devlog/2026-08-07-p5-4-10-sel4-boot-layout/`](../devlog/2026-08-07-p5-4-10-sel4-boot-layout/index.md) |
| B11's product-vs-test profile pair | **Reclassified — structurally absent.** B11's defect is a *shared* manifest whose product graph declares probes and scenario doubles as peers of real services. The seL4 fixtures are per-scenario siblings rather than profiles of one manifest, so there is no shared graph to contaminate: `sel4.zti` declares five real components and no probe, and each scenario's doubles live only in its own fixture. Checked component-by-component across all eight. A product-vs-test pair would first require the shared manifest this design does not have |
| `task_reclamation.rs`'s per-cycle drift, cost scaling, rejected-spawn conservation | **Partial — as far as this allocator allows.** `sel4_root_boot_check` now pins each task's reclaimed CSlot range exactly (`832..882`, `882..932`, aggregate `100`) instead of matching `\d+`, so a short or overlapping reclaim fails. The three named properties are *frame-count differentials*, and root CSlots are never returned to the allocator (`task.rs::CleanupRecord::revoke`), so a free-count comparison here is flat by construction and would pass whether or not reclamation ran. Closing them needs an allocator that reuses — see [`devlog/2026-08-07-p5-4-10-slot-conservation/`](../devlog/2026-08-07-p5-4-10-slot-conservation/index.md) |

Six of the oracle's fifteen `component_image.rs` cases have no direct port —
`rejects_bad_magic`, `rejects_unsupported_version`,
`rejects_header_size_disagreeing_with_the_revision`,
`retained_v1_reserved_field_must_be_zero`, `rejects_bad_stack_sizes`, and
`rejects_abi_mismatch` — and the eleven ported `boot-contracts` tests map onto
the other nine. Five of the six are header-shape assertions already covered by
`boot-contracts`' own header tests; `rejects_bad_stack_sizes` reads a page
constant in a header context rather than a segment one, and belongs with a
header validator if one is written. `rejects_abi_mismatch` is covered by
`each_qualification_axis_is_reported_separately`.

This list is exact because P5.4.final's deletion of `kernel/` depends on it. An
earlier count here said "two of fifteen", which an independent review of this
milestone corrected.

#### Exit condition

Every row above is closed or explicitly reclassified, each with its own devlog
entry.

### P5.4.final — Delete `kernel/`

**Status:** Complete. `kernel/` and its legacy-only gates are removed after all
six deletion-audit findings were closed or deliberately reclassified.

**Depends on:** every P5.4.2+ slice.

#### Deletion-audit dispositions

1. **Task reclamation.** The seL4 root uses a monotonic object allocator, so the
   oracle's free-frame differential is not a meaningful invariant on this path.
   `sel4_root_boot_check` instead pins each task's exact reclaimed CSlot range,
   adjacency, aggregate width, and zero live-task result; shared-buffer teardown
   separately requires every mapping, frame anchor, and holder charge to return
   to zero.
2. **Component-image shape corpus.** Complete wrapper admission now lives in
   `boot_contracts::component_image`: ABI, reserved fields, stack bounds, ELF
   payload length, entry/segment shape, W^X, and mapped-footprint bounds are
   host-tested independently of any kernel loader.
3. **NVMe.** Deliberately not claimed. The retired QEMU/custom-kernel transport
   was not product evidence for M5.7's required physical Framework observation.
   `storage_nvme_read_check` now fails closed; M5.7 remains explicitly blocked
   until a seL4 userspace NVMe path and removable-media Framework evidence exist.
4. **Custom stage-0/EL1 boot.** Reclassified as historical P2.1 evidence, not a
   seL4 runtime acceptance property. Both UEFI stage-0 targets remain compiled
   and linted; the product boot is the pinned seL4 loader/root image.
5. **PMM/VMM/heap/APIC foundation tests.** Reclassified as tests of mechanism
   supplied by seL4. The product-side contract is observed at the boundary:
   independent frame allocation and exact accounting, isolated child VSpaces,
   timer delivery, mapped-page rights, and bounded fault attribution.
6. **Smoke, panic, and IPC fault isolation.** `sel4_root_boot_check` observes a
   clean child and a deliberately faulting child in one boot, exact task
   reclamation, mapping-protection faults, and a ready root. Its failure markers
   reject root/child panic, abort, escaped fault, kernel fault, and service-loop
   exhaustion. `sel4_gate_control_check` proves every seL4 marker gate turns red
   when required evidence is removed, reordered, or contradicted.

#### Coordinated cutover

The workspace member, component legacy transport, custom-kernel build scripts,
oracle checkers, harness artifact selector, CI targets, and generation-builder
dependency on a custom-kernel ELF were removed together. Historical Justfile
identifiers either resolve to their seL4/host successor or fail closed where the
required physical product path does not yet exist.

#### Exit condition (observed)

Every acceptance property retained by the product has an observed seL4 or host
contract gate, deliberate non-equivalences are recorded without claiming them,
and `kernel/` plus its legacy-only gates are removed in one reviewable change.

## MCU and embedded-companion boundary

Cortex-M, RV32 microcontrollers, and other systems without the admitted MMU and user/supervisor isolation baseline do not run a weakened form of this kernel. They are external devices reached through bounded userspace services.

A later companion profile may admit micro-ROS/XRCE-DDS or a smaller Zutai protocol over an exact serial, CAN, USB, or network capability. That profile must declare peer identity, types, directions, payload size, frequency, queue depth, timeout, reset behavior, and actuator authority. Disconnect, malformed traffic, reboot, and resource exhaustion become structured C8/C9 events; the companion never receives ambient graph, network, storage, or device authority.

## Verification policy

- P0 contract changes run `just contracts_check` and `just generation_check` in addition to their narrow target.
- Permanent Rust changes run the repository format and lint gates for every affected workspace.
- P1–P3 run the narrowest architecture QEMU target and then the shared semantic corpus named by the slice.
- A pass on one ISA cannot close another ISA's gate. A QEMU pass cannot close P4 or any RP physical demo milestone.
- Cross-architecture comparisons assert normalized semantic events and authenticated artifacts; they do not claim byte-identical register frames, page tables, physical addresses, or device traces.
