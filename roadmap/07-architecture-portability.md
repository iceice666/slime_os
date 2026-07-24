# Architecture portability track

**Purpose:** Preserve one Slime capability/component/generation architecture across x86-64, AArch64, and RV64 without pretending that one kernel binary, executable image, interrupt model, or physical qualification applies to every target.

**Status:** Not started.

**Decision:** x86-64 remains the deterministic reference architecture. AArch64 is the first non-x86 implementation and physical-product priority. RV64 is the second architecture profile. Slime targets 64-bit little-endian systems with an MMU and user/supervisor isolation; MCU-class targets without that isolation boundary are external bounded companions, not reduced-security ports of this kernel.

## Initial target profiles

| Profile | Role | Initial machine | Required baseline |
| --- | --- | --- | --- |
| `x86_64-qemu-virtio` | Reference and regression oracle | QEMU q35/UEFI | x86-64, 4 KiB pages, ring 0/ring 3, APIC, virtio |
| `aarch64-qemu-virt` | First non-x86 architecture | QEMU `virt`/UEFI | AArch64, 4 KiB translation granule, EL1/EL0, GICv3, generic timer, PL011, virtio |
| `riscv64-qemu-virt` | Second architecture profile | Pinned QEMU `virt` machine and firmware | RV64 little-endian, S/U mode, Sv39, atomic operations, pinned interrupt/timer/UART devices, virtio |

A profile name identifies a complete executable and platform contract, not only an instruction set. A different page granule, privilege model, interrupt controller, firmware handoff, or incompatible device topology is a new profile until its own checks pass.

## Boundaries

- Capability semantics, object identities, rights, channels, generation selection, BootState, release authorization, rollback, Zutai protocols, C7 shared samples, C8 typed routes, and ROS wire profiles remain architecture-neutral.
- Trap frames, context switching, privilege transitions, page tables, TLB operations, interrupt controllers, timers, idle instructions, debug transports, QEMU exit paths, and early boot mappings are architecture-specific mechanisms.
- The generation `target` remains the signed complete platform profile. Release metadata continues to bind the exact target.
- Kernel and component executables are built and authenticated per target. Architecture-neutral resource objects may be shared when their schemas and identities are byte-identical; executable objects are never assumed portable across targets.
- A logical syscall operation has one semantic contract, error model, bounds, and rights checks. Each architecture has an explicit calling convention and trap instruction; register layouts are not serialized as a cross-architecture ABI.
- The implementation uses small explicit architecture modules. It does not introduce a broad trait framework merely to hide one call site, and it does not move device or scheduling policy into the kernel.
- QEMU proves deterministic architecture behavior. It cannot establish a physical board, firmware, DMA, storage, timing, or device-support claim.

## Sequencing

1. The backlog remains ahead of new roadmap gates. P0 opens only after every active backlog item is resolved or explicitly deferred under the backlog rules.
2. B2 and C7 continue on x86-64; neither waits for an AArch64 or RV64 boot. New low-level work must not add uncontained x86 assumptions outside the architecture/platform boundary P1 will own.
3. P0 fixes artifact and target contracts before another architecture emits executable generations.
4. P1 extracts and verifies the existing x86-64 implementation without changing observable behavior.
5. H2's Framework driver ABI and C9's timer/scheduling work consume P1 so they do not establish APIC, PCI, CR3, or x86 register layouts as universal kernel contracts.
6. C8 may proceed after its existing C7/B2 gates while P2 is implemented. A later architecture-qualified native release replays the C8 corpus on the admitted non-x86 target.
7. P2 establishes AArch64 before P3 establishes RV64. P3 may reuse proven boundaries, but it must not silently inherit AArch64 platform assumptions.
8. P4 names and qualifies exact physical boards only after their corresponding QEMU profiles pass.

## P0: Architecture, target, and executable-artifact contracts

**Status:** Not started.

**Depends on:** Foundations and a cleared or explicitly deferred backlog.

### Deliverables

- define versioned Zutai component-image and kernel-image revisions carrying an explicit architecture identifier, architecture-qualified ABI identifier, required ISA/profile flags, and page-profile identifier;
- retain bounded decoding of existing x86 component and kernel images for the declared rollback window; old formats keep their existing meaning and are never reinterpreted as architecture-neutral;
- validate generation target, release target, kernel image, bootstrap image, and every component executable as one compatible set before execution or activation;
- define the three initial profile identifiers above and reject unknown architecture IDs, ABI IDs, required flags, page profiles, and target/image mismatches before mapping executable bytes;
- parameterize host builders, ELF validation, linker inputs, Cargo targets, artifact paths, and QEMU launch selection by the exact profile; each builder accepts only the ELF machine and relocation subset declared by that profile;
- preserve content identity for byte-identical architecture-neutral resources while producing separately identified target executables and complete target generations;
- define one semantic syscall table with per-architecture calling-convention documents for x86-64 `int 0x80`, AArch64 `svc`, and RV64 `ecall`;
- audit the stage-0 handoff and generation contracts so physical addresses, direct-map metadata, framebuffer data, memory maps, and executable entry state remain versioned and sufficient without serializing x86 page-table or register layouts.

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

A generation and its release identify one exact target profile; stage-0 rejects every mismatched kernel or component executable before mapping it, retained x86 rollback artifacts retain their old meaning, and deterministic builders emit only profile-valid authenticated artifacts.

## P1: x86-64 architecture boundary extraction

**Status:** Not started.

**Depends on:** P0.

### Deliverables

- place x86 trap frames, exception stubs, context switching, user entry, control-register access, page-table operations, TLB invalidation, interrupt masking, GDT/TSS/IDT, APIC/PIT time, port I/O, halt, serial, and QEMU-exit mechanisms behind an explicit `arch/x86_64` boundary;
- separate QEMU q35 and Framework platform assembly from ISA mechanisms so ACPI/PCI/UEFI policy does not become the interface required by AArch64 or RV64;
- give stage-0 profile-specific page-table construction, relocation validation, entry-state setup, and linker configuration while preserving the shared verified-generation and BootState selection flow;
- make userspace syscall wrappers select only the per-architecture trap/calling-convention implementation while retaining one semantic Rust API;
- add a source allowlist check that rejects x86 instructions, registers, ELF machine constants, and x86-only linker/QEMU assumptions outside the admitted architecture/platform/build files;
- preserve all existing x86 behavior and evidence; this slice is a boundary extraction, not permission to weaken bounds or rewrite completed contracts.

### Required checks

- the current x86 QEMU boot, isolation, IPC, generation, rollback, recovery, storage, B2, and C7 checks retain their existing observable results as applicable when P1 lands;
- a user fault, syscall, timer preemption, address-space switch, and blocked-task wake traverse the extracted boundary without changing their structured result;
- no x86 assembly, CR register, GDT/IDT/APIC/PIT, port-I/O, ELF-machine, linker-format, or `qemu-system-x86_64` assumption remains in architecture-neutral kernel, component-runtime, contract, or generation code except an explicit profile dispatch;
- architecture-neutral code can be type-checked for another 64-bit target without importing x86-only modules, even before that target boots.

### Planned verification target

```sh
just x86_portability_check
```

### Exit condition

The full x86-64 reference vertical slice behaves as before through a named architecture/platform boundary, and an enforced allowlist prevents new x86 mechanism from leaking back into shared contracts or runtime code.

## P2: AArch64 QEMU vertical slice

**Status:** Not started.

**Depends on:** P1, C7, and backlog item B2.

### Deliverables

- build an AArch64 UEFI stage-0 and kernel for the pinned QEMU `virt` profile using the same verified generation, release, BootState, handoff, and rollback semantics;
- enter the kernel at EL1, run isolated components at EL0, establish bounded 4 KiB translation tables and direct-map access, and reject malformed or unsupported mappings with structured failure;
- implement exception vectors, synchronous fault decoding, `svc` syscalls, saved user context, address-space switching, interrupt masking, and idle/wake behavior behind `arch/aarch64`;
- implement GICv3 interrupt delivery, the ARM generic timer, PL011 diagnostics, QEMU exit, and the deterministic virtio devices required by the exercised vertical slice;
- build target-specific component images from AArch64 ELF intermediates while keeping syscall semantics, capabilities, generation grants, and userspace service protocols identical to x86;
- run the same B2 wait/wake sources and C7 shared-sample lifecycle rather than creating architecture-specific alternatives.

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

The AArch64 QEMU profile boots a verified rollbackable generation, runs isolated EL0 components, exercises IPC, faults, timer preemption, all B2 wake classes, and the bounded C7 sample plane with the same architecture-neutral authority and lifecycle semantics as x86-64.

## P3: RV64 QEMU vertical slice

**Status:** Not started.

**Depends on:** P2.

### Deliverables

- pin one QEMU `virt` machine version, firmware/stage-0 route, RV64 ISA baseline, interrupt controller, timer, UART, and virtio device set; supporting multiple RISC-V firmware or interrupt profiles is outside this slice;
- implement S-mode kernel and U-mode component execution with Sv39, 4 KiB pages, bounded page-table construction, TLB invalidation, and explicit unsupported-feature rejection;
- implement trap decoding, `ecall` syscalls, saved user context, address-space switching, interrupt masking, idle/wake behavior, timer preemption, diagnostics, and QEMU exit behind `arch/riscv64`;
- validate the admitted RISC-V ELF machine, ISA flags, relocation subset, entry state, and component executable layout through the P0 artifact contract;
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

## P4: Named physical architecture qualification

**Status:** Not started.

**Depends on:** P2 for the first AArch64 board and P3 before any RV64 physical-support claim.

### Deliverables

- select one exact AArch64 board revision, firmware version, boot path, storage medium, interrupt topology, timer, serial path, and minimum device set; “ARM support” is not a valid target name or exit condition;
- select an RV64 board only after P3 exposes the actual firmware, interrupt, timer, DMA, and device assumptions that require physical evidence;
- record reproducible removable-media images, generation/release identities, firmware and board identities, normalized topology, serial evidence, storage-integrity boundaries, and every granted device capability;
- qualify DMA, storage writes, networking, sensors, and actuators only through their owning hardware milestones; a CPU boot does not promote an untested peripheral;
- after R1/R2 and AArch64 networking exist, replay the pinned ROS conformance probes between an AArch64 Slime target and the same content-addressed external peer environment to provide heterogeneous evidence without changing the wire profile.

### Required checks

- each named board runs the isolated native vertical slice from reproducible media and preserves every declared no-write or exact-device storage boundary;
- firmware changes, wrong board revisions, unsupported page/interrupt profiles, and missing required devices fail with bounded diagnostics rather than silently selecting a nearby profile;
- physical timer, interrupt, reset, link, and storage behavior is recorded separately from inherited QEMU evidence;
- the ROS heterogeneous run uses the same admitted types, QoS, packet capture, allowed/denied routes, Fast DDS selection, and Cyclone DDS selection as the deterministic R1/R2 corpus.

### Planned verification

Each admitted board receives a board-specific `just` target and evidence record when selected. There is intentionally no generic `just arm_check` or `just riscv_check`.

### Exit condition

One named AArch64 board, and later one separately named RV64 board, runs the verified isolated Slime vertical slice with reproducible firmware/media evidence and no unqualified device or storage claim; admitted ROS routes interoperate heterogeneously only after their deterministic QEMU corpus passes.

## MCU and embedded-companion boundary

Cortex-M, RV32 microcontrollers, and other systems without the admitted MMU and user/supervisor isolation baseline do not run a weakened form of this kernel. They are external devices reached through bounded userspace services.

A later companion profile may admit micro-ROS/XRCE-DDS or a smaller Zutai protocol over an exact serial, CAN, USB, or network capability. That profile must declare peer identity, types, directions, payload size, frequency, queue depth, timeout, reset behavior, and actuator authority. Disconnect, malformed traffic, reboot, and resource exhaustion become structured C8/C9 events; the companion never receives ambient graph, network, storage, or device authority.

## Verification policy

- P0 contract changes run `just contracts_check` and `just generation_check` in addition to their narrow target.
- Permanent Rust changes run the repository format and lint gates for every affected workspace.
- P1–P3 run the narrowest architecture QEMU target and then the shared semantic corpus named by the slice.
- A pass on one ISA cannot close another ISA's gate. A QEMU pass cannot close P4.
- Cross-architecture comparisons assert normalized semantic events and authenticated artifacts; they do not claim byte-identical register frames, page tables, physical addresses, or device traces.
