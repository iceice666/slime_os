# Architecture portability track

**Purpose:** Preserve one Slime capability/component/generation architecture across target profiles while making AArch64 and Raspberry Pi 5 the near-term product path.

**Status:** In progress — P0, P1, P2.1, P2.2, and P5 complete. P2.3–P2.6 are superseded by P5. P4's build path landed 2026-08-24 and its board boot is deferred on hardware: the gate and artifacts are ready, the USB-UART adapter on hand does not work, and the debug UART is the only console seL4 has.

**Decision:** AArch64/Raspberry Pi 5 is now the near-term physical target because the current product goal is the RPi5 ROS 2 two-node demo. The existing x86-64 QEMU path remains the regression oracle for completed work until each semantic corpus is replayed on AArch64, but x86-64/Framework is no longer the product-leading roadmap. RV64 is deferred. As of P5, the AArch64 kernel-side mechanism is being substituted with upstream seL4 rather than hand-written: see [P5](#p5-sel4-microkernel-substitution), which supersedes the custom-kernel half of P2.2-P2.6 if it completes.

Slime targets 64-bit little-endian systems with an MMU and user/supervisor isolation. MCU-class targets without that isolation boundary are external bounded companions, not reduced-security ports of this kernel.

## Initial target profiles

| Profile | Role | Initial machine | Required baseline |
| --- | --- | --- | --- |
| `x86_64-qemu-virtio` | Retired with P5; historical regression oracle only | QEMU q35/UEFI | x86-64, 4 KiB pages, ring 0/ring 3, APIC, virtio |
| `aarch64-sel4-qemu-virt` | Current product path | QEMU `virt` with upstream seL4 | AArch64, 4 KiB translation granule, EL1/EL0, GICv3, generic timer, PL011, virtio |
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
**Delivered:** Versioned Zutai component-image and kernel-image revisions carrying an explicit architecture identifier, ABI identifier, ISA/profile flags, and page-profile identifier; bounded decoding of existing x86 artifacts preserved for the rollback window; generation target, release target, and every executable validated as one compatible set before execution; one semantic syscall table with per-architecture calling-convention documents for x86-64 `int 0x80`, AArch64 `svc`, and RV64 `ecall`.
**Exit condition (observed):** A generation and its release identify one exact target profile; stage-0 rejects every mismatched kernel, component, or node executable before mapping it, retained x86 rollback artifacts keep their old meaning, and deterministic builders emit only profile-valid authenticated artifacts.
**Gates:** `just architecture_contract_check`
**Evidence:** [`devlog/2026-08-02-p0-architecture-contracts/`](../devlog/2026-08-02-p0-architecture-contracts/index.md)

**Depends on:** Foundations and a cleared or explicitly deferred backlog.

## P1: x86-64 architecture boundary extraction

**Status:** Complete.
**Delivered:** x86 trap frames, exception stubs, context switching, page-table operations, TLB invalidation, GDT/TSS/IDT, APIC/PIT time, port I/O, and QEMU-exit mechanisms placed behind an explicit `arch/x86_64` boundary; QEMU q35/Framework platform assembly separated from ISA mechanism; a source allowlist rejecting x86 instructions, registers, and ELF/linker/QEMU assumptions outside admitted architecture/platform/build files; all existing x86 behavior and evidence preserved.
**Exit condition (observed):** Observed 2026-08-02. `just test` passes the same 191 assertions and `just product_boot_check` reaches the same healthy 45-slot product slice as the pre-change baseline, with `just rollback_check`, `just architecture_contract_check`, `just generation_check`, and `just contracts_check` clean. `just x86_portability_check` enforces the boundary over 186 neutral Rust files and builds the neutral kernel and component runtime for `aarch64-unknown-none` — the build, not a `cargo check`, is the binding half, since rustc validates inline-assembly mnemonics only during codegen. This proves the boundary holds; it makes no claim that AArch64 boots, which is P2.
**Gates:** `just x86_portability_check`
**Evidence:** [`devlog/2026-08-02-p1-x86-boundary-extraction/`](../devlog/2026-08-02-p1-x86-boundary-extraction/index.md)

**Depends on:** P0.

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

The parent's deliverables, required checks, and exit condition below remain the
record of what a hand-written second architecture would have required. P2 did
not close this way: P2.3–P2.6 were superseded by P5 before opening their own
gates, so `just aarch64_qemu_check` above was never run and no sub-slice closed
under it. P5.1–P5.5 close the aggregate instead, over upstream seL4 rather than
this decomposition; see
[`devlog/2026-08-09-p5-4-final-kernel-retirement/`](../devlog/2026-08-09-p5-4-final-kernel-retirement/index.md)
for where the retired custom-kernel route's record lives, and P2.1–P2.6 below
for what each sub-slice's own historical evidence covers.

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
**Delivered:** The first slice to execute AArch64 instructions: the `qemu-system-aarch64 -machine virt` profile pinned; a stage-0 loader for `aarch64-unknown-uefi` sharing the architecture-neutral generation/BootState/rollback flow with x86; bounded 4 KiB stage-1 translation tables, a direct map, and a guarded boot stack; EL1 entry with MMU/caches/SIMD configured; PL011 diagnostics and QEMU exit. No component runs and no syscall is served — those are P2.3 and P2.2. QEMU only; establishes nothing about Raspberry Pi 5 hardware.
**Exit condition (historical):** Observed 2026-08-03 on the retired custom-kernel path. `aarch64_boot_check` now resolves to `just sel4_root_boot_check`, which validates the seL4 product image instead; the PL011, custom stage-1 translation-table, direct-map, heap, and semihosting observations remain historical evidence only and must not be cited as current proof.
**Gates:** `just aarch64_boot_check` (historical; now resolves to `just sel4_root_boot_check`)
**Evidence:** [`devlog/2026-08-03-p2-1-aarch64-boot/`](../devlog/2026-08-03-p2-1-aarch64-boot/index.md)

**Depends on:** P1.

### P2.2 — Exception vectors, fault decoding, and `svc` entry

**Status:** Complete on the retired custom-kernel path, then superseded by P5. seL4 owns exception entry, fault decoding, and the trap instruction; `slime-root/src/fault.rs` decodes seL4's fault messages into the architecture-neutral vocabulary supervision reports, and there is no Slime trap vector or register mapping left to implement.
**Delivered:** The EL1 exception vector table and `UserFrame` save/restore; synchronous exception decoding from `ESR_EL1` into the architecture-neutral `UserFaultReason` vocabulary; `svc #0` syscall entry against a Slime-owned register mapping; `DAIF` interrupt masking and the idle/park path.
**Exit condition (historical):** Observed 2026-08-03 on the retired custom-kernel path. That historical gate installed the architected 16-slot EL1 vector table at `VBAR_EL1`, decoded an EL1 `brk` and an EL0 undefined instruction through `ESR_EL1.EC`, dispatched an `svc #0` into the retired kernel's syscall body, and observed the 31-register frame plus `SP_EL0` surviving `eret`. None of that mechanism survives: `just aarch64_trap_check` now resolves to `sel4_root_boot_check`, which asserts fault isolation on the product path (`SLIME_ROOT child fault observed task=1 role=deliberate-fault kind=VirtualMemory { access: Write }`, then termination and full slot reclamation). The PL011 vector-table, `svc`/`eret` frame, and `DAIF`-window observations remain historical evidence only.
**Gates:** `just aarch64_trap_check` (historical; now resolves to `just sel4_root_boot_check`)
**Evidence:** [`devlog/2026-08-03-p2-2-aarch64-traps/`](../devlog/2026-08-03-p2-2-aarch64-traps/index.md)

**Depends on:** P2.1.

### P2.3 — EL0 execution, address spaces, and isolation

**Status:** Superseded by P5 before opening its own gate.
**Would have delivered:** Target-qualified AArch64 component images executing at EL0 with user/kernel translation separation, per-fault attribution, and frame reclamation on termination, replacing x86's single-root `KERNEL_HALF_START` split with an `arch::paging` boundary.
**Superseded before closure:** `just aarch64_isolation_check` was never built; the exit condition below was never observed on the custom-kernel path. EL0 execution, per-task VSpaces, and frame reclamation are seL4 objects the root constructs instead (`slime-root/src/{task,child_vspace,object_allocator}.rs`).
**Gates (current, via P5):** `just sel4_root_boot_check`, `just sel4_reclamation_check`
**Evidence:** No dedicated devlog entry exists for this slice; see [`devlog/2026-08-09-p5-4-final-kernel-retirement/`](../devlog/2026-08-09-p5-4-final-kernel-retirement/index.md) for where the retired custom-kernel route's record lives.

**Depends on:** P2.2.

### P2.4 — GICv3, generic timer, and the B2 wake classes

**Status:** Superseded by P5 before opening its own gate.
**Would have delivered:** GICv3 distributor/redistributor initialization and interrupt delivery, the ARM generic timer as the periodic tick, and the B2 wake classes (timer preemption, endpoint wake, scripted-input wake, supervision wake) through the shared scheduler.
**Superseded before closure:** `just aarch64_wake_check` was never built; the exit condition below was never observed on the custom-kernel path. seL4 owns the GIC and the generic timer instead; the root drives the platform timer and declared Notifications (`slime-root/src/{platform_timer,notification}.rs`), and components wait on native Notifications rather than a root wait set.
**Gates (current, via P5):** `just sel4_root_boot_check`
**Evidence:** No dedicated devlog entry exists for this slice; see [`devlog/2026-08-09-p5-4-final-kernel-retirement/`](../devlog/2026-08-09-p5-4-final-kernel-retirement/index.md) for where the retired custom-kernel route's record lives.

**Depends on:** P2.3 and backlog item B2.

### P2.5 — virtio-mmio, generation selection, and rollback

**Status:** Superseded by P5 before opening its own gate.
**Would have delivered:** A device-tree-discovered virtio-mmio block transport behind the neutral `device_discovery`/`BlockDevice` surfaces, and AArch64 generation staging, activation, and rollback through the same BootState/release/recovery flow as x86.
**Superseded before closure:** `just aarch64_generation_check` was never built; the exit condition below was never observed on the custom-kernel path. The root drives virtio-mmio directly instead (`slime-root/src/{device,virtio_blk}.rs`), and generation selection, activation, and rollback run on that path.
**Gates (current, via P5):** `just sel4_generation_check`, `just sel4_rollback_check`, `just sel4_boot_selection_check`
**Evidence:** No dedicated devlog entry exists for this slice; see [`devlog/2026-08-09-p5-4-final-kernel-retirement/`](../devlog/2026-08-09-p5-4-final-kernel-retirement/index.md) for where the retired custom-kernel route's record lives.

**Depends on:** P2.4.

### P2.6 — C7/C8 data path and the aggregate corpus

**Status:** Superseded by P5 before opening its own gate.
**Would have delivered:** The C7 shared-sample plane and the C8 bounded data path running unmodified on AArch64, producing deterministic normalized traces comparable with x86 at the semantic event level, closing the aggregate P2 corpus.
**Superseded before closure:** `just aarch64_qemu_check` was never built and the parent P2 exit condition above was never observed on the custom-kernel path. The C7 sample plane and the C8 fabric planes run on seL4 under their own gates instead; P5.3 and P5.5 record the observations.
**Gates (current, via P5):** `just sel4_sample_check`, `just sel4_stream_check`, `just sel4_qos_check`, `just sel4_call_check`, `just sel4_operation_check`, `just sel4_visibility_check`
**Evidence:** No dedicated devlog entry exists for this slice; see [`devlog/2026-08-09-p5-4-final-kernel-retirement/`](../devlog/2026-08-09-p5-4-final-kernel-retirement/index.md) for where the retired custom-kernel route's record lives.

**Depends on:** P2.5 and C7.

## P3: RV64 QEMU vertical slice

**Status:** Deferred until after the Raspberry Pi 5 ROS 2 demo stabilizes. Rescoped by P5: this is no longer a custom-kernel port. seL4 supports RV64 upstream, so the S-mode kernel, Sv39 translation tables, trap decoding, `ecall` entry, context switching, interrupt controller, and timer are all upstream mechanism to configure and pin, not Slime code to write. What remains Slime's is the target profile, the stage-0/loader route, and replaying the semantic corpus.

**Depends on:** P5, and P4 for the precedent of qualifying a second seL4 platform.

### Deliverables

- pin one QEMU `virt` machine version, firmware/loader route, RV64 ISA baseline, interrupt controller, timer, UART, and virtio device set, and add the matching upstream seL4 kernel configuration and pinned artifact hashes alongside the existing `sel4/config/qemu-arm-virt.cmake`;
- add the RV64 target profile to P0's architecture/ABI/page-profile contract and its `ecall` calling-convention document, so a generation names it and stage-0 rejects AArch64 and retained x86 artifacts before mapping executable bytes;
- build `slime-root` and the component images for the RV64 seL4 target, confirming the root's mechanism modules carry no AArch64 assumption outside the admitted boundary;
- replay the same isolation, B2 wait/wake, C7 sample-plane, generation, and rollback acceptance corpus the AArch64 seL4 gates run.

### Required checks

- the pinned RV64 seL4 QEMU profile boots the kernel and `slime-root`, admits a verified target generation, and rejects AArch64 and retained x86 artifacts before executable mapping;
- isolation, faults, root operations, timer preemption, blocked waits, shared samples, quota exhaustion, peer death, and rollback preserve the same structured semantics as the AArch64 planes;
- unsupported ISA extensions, page modes, firmware handoffs, interrupt profiles, ELF flags, and relocations fail explicitly rather than being guessed from the running machine;
- no AArch64 register, GIC, firmware, translation-table, or device assumption appears in shared or RV64-specific paths.

### Planned verification target

```sh
just riscv64_qemu_check
```

### Exit condition

The pinned RV64 QEMU profile passes the same architecture-neutral isolation, wait/wake, sample-plane, generation, and rollback corpus as AArch64 without importing x86 or ARM mechanism into its contracts.

## P4: Raspberry Pi 5 physical architecture qualification

**Status:** Deferred on hardware — the build path is complete and reproducible;
the board boot is **not** observed, so P4 is not closed. Blocked on a working
USB-UART adapter, not on code: the one on hand does not produce bytes, and the
debug header is the only console this kernel has (see *Why serial is the only
evidence path* below).

**Delivered so far (2026-08-24):** `bcm2712` is a second seL4 build platform
beside `qemu-arm-virt`, with its own prefix, cargo target directories,
generation, image, identity manifest, and pinned artifact hashes
(`[bcm2712_rpi5]`, `[observed_prefix_bcm2712_rpi5]`), reproducible byte-identical
across from-scratch rebuilds. Three blockers were closed rather than deferred:
`sel4-kernel-loader` had no `PLAT_BCM2712` arm at all (failing with `unresolved
import imp`), so `deps/rust-sel4` is now a fork adding a PL011 console on the
UART10 the board's own `overlay-rpi5.dts` designates; `objcopy -O binary` cannot
produce the boot image, because the loader payload lives in program headers
carrying no sections and is silently dropped (38312 bytes out of 797696), so
`scripts/build/build-rpi5-media.py` flattens PT_LOAD segments by physical
address into the pinned `kernel8.img`/`config.txt`; and upstream's *verified*
bcm2712 configuration forces `PRINTING` off, which would have made this
milestone's own serial exit condition unobservable. Board facts — memory window,
GIC-400/GICv2, 54 MHz generic timer, UART10 at `0x107d001000` — are read from
seL4's own platform description, never asserted by Slime.

**Not delivered:** the observed boot. `just rpi5_boot_check` builds, proves the
media is this build's, and then fails closed naming the missing serial device;
it never falls back to QEMU. Also open: the board is deliberately outside the
verified kernel configuration (recorded, not incidental), and it sees 1019 MiB
because upstream ships no RPi5 overlay above the VideoCore base.

**Gates:** `just sel4_rpi5_image_check`, `just rpi5_media_check`, `just rpi5_boot_check`
**Evidence:** [`devlog/2026-08-24-p4-rpi5-board-bringup/`](../devlog/2026-08-24-p4-rpi5-board-bringup/index.md)

**Depends on:** P5, which supplies the kernel this board runs. P2's custom-kernel AArch64 slice is superseded and is not a prerequisite.

### Why serial is the only evidence path

Recorded here because "just use HDMI" is the obvious question, and the answer is
structural rather than a missing feature.

seL4 ships exactly three driver families — `deps/sel4/src/drivers/{serial,timer,smmu}`.
There is no display, framebuffer, HDMI, storage, or network driver anywhere in
the kernel, and that is the design: everything except the mechanism needed to
*be* a microkernel lives in userspace. A grep for framebuffer or HDMI output in
`src/`/`include/` finds only x86 `boot_sys.c` and an unrelated `.dts`.

Even the serial driver is not really an exception. It exists only to implement
`printf`/`seL4_DebugPutChar` for debugging, is selected by device-tree
`compatible` string (`arm,pl011` → `pl011.c` for this board), and is compiled
out entirely in the verified configuration — `AARCH64_bcm2712_verified.cmake`
sets `KernelVerificationBuild ON`, which forces `KernelPrinting OFF`. So the
kernel's console is a debug facility that the proof configuration deliberately
removes, not a supported output device.

That leaves three ways a Pi 5 could ever show output, and only one is available
now:

1. **Debug UART** (current): `seL4_DebugPutChar` over UART10, which the board's
   own `overlay-rpi5.dts` designates through `seL4,elfloader-devices`. Needs a
   working USB-UART adapter. This is what P4 is blocked on.
2. **A userspace display driver**: real work, and much larger than it sounds on
   this SoC — the Pi 5's video output sits behind the RP1 southbridge across
   PCIe, so it needs PCIe enumeration, address translation, and HDMI/VC
   bring-up before a single pixel, all as generation-declared device authority.
   Roadmap invariant 2 puts it in userspace, and invariant 4 requires a real
   gate. It is also the wrong shape for boot evidence: a framebuffer cannot
   report a fault that happens before it is mapped.
3. **JTAG/SWD** over the same 3-pin header: needs a debug probe and gives
   register state rather than a transcript, so it diagnoses a wedge but does not
   produce the ordered marker evidence P4's exit condition asks for.

Roadmap invariant: framebuffer output alone is never milestone completion. That
rule already anticipated this — a display would not close P4 even if it existed.
The cheap unblock is a different USB-UART adapter.

### Deliverables

- select one exact Raspberry Pi 5 board revision or accepted revision set, firmware version, boot path, removable storage medium, interrupt topology, timer, serial path, and minimum device set;
- build the `bcm2712` upstream seL4 kernel and loader image from the existing pins and platform configuration, alongside the current `sel4/config/qemu-arm-virt.cmake`, and pin its artifact hashes the same way;
- source the board's memory map, UART, GIC, and timer facts from seL4's BootInfo and its platform configuration rather than from any Slime-side board table;
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

**Status:** Complete — P5.1–P5.5 are observed, the custom kernel and its legacy-only gates are retired, and the seL4 product owns the surviving runtime contract.
**Decision:** Slime's differentiator is the capability/component/generation model in userspace, not a hand-written AArch64 microkernel. P2.2–P2.6 each required re-deriving exception vectors, isolation, GICv3, timers, and virtio on a second architecture — mechanism upstream seL4 already provides under formal verification. P5 substitutes seL4 for the custom kernel and keeps Slime's authority model as a root task, so architecture bring-up stops being Slime's problem. The frozen custom-kernel oracle was retained until P5.4 established equivalent or explicitly reclassified coverage; P5.4.final then removed it.
**Exit condition (observed):** P5.1–P5.5 close the aggregate below over upstream seL4, per their own sections.
**Gates:** see P5.1–P5.5 below.
**Evidence:** see P5.1–P5.5 below.

**Depends on:** P0 and P1. The custom-kernel half of P2.2–P2.6 is superseded; P2.1's AArch64 stage-0 evidence remains historical and is not re-claimed here.

### P5.1 — Standalone seL4 root task with generation authority

**Status:** Complete.
**Delivered:** Upstream seL4 and rust-sel4 pinned as submodules with exact commit/release/toolchain/target-spec/kernel-config/observed-artifact hashes, enforced fail-closed on a dirty tree; a deterministic `qemu-arm-virt` image with a re-verified identity manifest; the existing verified generation decoded and admitted inside a Rust root task, deriving every child's authority strictly from declared grants; child CSpace/VSpace/TCB constructed from native AArch64 ELF with no untyped/CNode/VSpace/ASID/IRQ authority in any child CSpace; IPC, fault supervision, timers, and shared buffers owned as bounded root-task mechanism.
**Exit condition (observed):** `qemu-system-aarch64 -machine virt,virtualization=on` boots the pinned seL4 kernel and the `slime-root` root task, which admits the 25-component generation graph, states all 25 legacy SLIMECM images are not activated, claims PPI 30 and observes one real timer interrupt delivered and acknowledged, maps two shared regions and exchanges bytes both ways with a native child, has seL4 refuse both a read-only write and an execute from a data page, runs one clean-exit and one deliberate-fault child through root-mediated IPC and fault supervision, and tears every resource back down to `live=0` — all asserted as ordered serial markers. No legacy component image runs and no Slime service graph is active: the proof is a native fixture. That is P5.2.
**Gates:** `just sel4_root_boot_check`
**Evidence:** [`devlog/2026-08-03-p5-1-sel4-cutover/`](../devlog/2026-08-03-p5-1-sel4-cutover/index.md)

### P5.2 — Native component images on seL4

**Status:** Complete.
**Delivered:** `components/bins` rebuilt as native AArch64 ELF images against the `sel4` transport feature, booting a declared five-component service graph (`init`, `console`, `spawn-service`, `sysinfo`, `echo-agent`) from a sibling generation manifest ([`contracts/generation/v1/fixtures/sel4.zti`](../contracts/generation/v1/fixtures/sel4.zti), not a boot profile of `valid.zti`, so it does not narrow the frozen 45-slot product generation; see [`sel4.md`](../contracts/generation/v1/fixtures/sel4.md) beside it). The root service answers the operation surface those components actually invoke, with the same errors and bounds as the legacy kernel; an unsupported operation returns its bounded Slime error rather than faulting the caller. Deferred, each on an `Unavailable` mediation plane out of scope for this cutover: `dango`/`powerbox-*` (input, directory); `generation-manager` and its five commands (generation management); `filesystem-service`/`directory-probe` (directory, object store); the four `storage-*` probes (block, object store); `recovery` (recovery); `sample-lender`/`sample-receiver`/`fabric-*` (C7 sample plane and C8 fabric — P5.3). Two further limits, both P5.3's work: `spawn-service` resolves authority from grants but does not yet construct children, and `recv`/`send`/`wait` are root-mediated with no handler yet (reported `unimplemented`, distinct from the `Unavailable` planes above).
**Exit condition (observed):** A generation of five native ELF component images boots its declared graph on seL4; every payload is target-qualified and admitted before mapping (`elf=5 slimecm=0 wrong_target=0 unrecognized=0`); each component receives exactly the grants the generation declares for it and runs; `spawn-service` completes the full shared-buffer create/map/write/seal/unmap/release cycle through real seL4 frames against its declared quota; an unanswered operation returns its bounded error with the caller still running, and an ungranted executable slot is refused. The bounded-error property is observed on the operations these components actually reach (the `unimplemented` ones) and asserted statically, not observed, over the nine `Unavailable` planes, since no declared component invokes one on this boot path; both halves are fault-injected in the devlog entry.
**Gates:** `just sel4_component_graph_check` (a separate image from `sel4_root_boot_check`'s; neither invalidates the other's evidence by being built last)
**Evidence:** [`devlog/2026-08-04-p5-2-native-component-images/`](../devlog/2026-08-04-p5-2-native-component-images/index.md)

**Depends on:** P5.1.

### P5.3 — C7 sample plane on seL4

**Status:** Complete — P5.3.1, P5.3.2, P5.3.3, and P5.3.4 all observed.
**Delivered:** The bounded sample plane replayed on the seL4 root task, so the RPi5 demo's data path does not depend on the retired kernel. Retitled 2026-08-04 from "C7/C8 data path" — the exit condition names only C7-shaped properties; the minimal typed-fabric slice moved to P5.5. Decomposed into four independent state surfaces (channels, loan plane, child construction, death reclamation), for the same reviewability reason as C7 and C8.9.
**Exit condition (observed):** Two components exchange and return a payload larger than the control-message bound over seL4, with quota exhaustion and peer death reclaiming the same resources the x86 corpus records. All four sub-slices below are observed.
**Gates:** see P5.3.1–P5.3.4 below.
**Evidence:** see P5.3.1–P5.3.4 below.

**Depends on:** P5.2 and C7.

### P5.3.1 — Channel plane on seL4

**Status:** Complete.
**Delivered:** `Send`/`Recv`/`Wait` were root-mediated but had no handler (every declared component reached its first `recv` and exited non-zero). This slice makes a channel a real object — materialized from the generation's declared grants, owned by the root, named by a logical slot — with blocking parking/waking through the transfer window, bounded-depth and capability-send refusal, immediate answer for `wait` on a ready source, and peer-death reclamation.
**Exit condition (observed):** Two components exchange bounded messages over channels the generation declared: `init` sends a 42-byte payload (crossing the transfer window past the 16-byte inline registers) to a `console` parked in `recv`, which wakes and prints the exact bytes. A capability-carrying send is refused, a self-edge accepts exactly `CHANNEL_CAPACITY` messages and refuses the next, a `wait` on a ready source is answered rather than parked, and `console` — parked again when `init` exits — is woken by its peer's death; the graph drains to `live=0 … parked=0 queues=0`. Three denial arms are fault-injected; the peer-death arm was not covered by the first fixture and the injection is what found it. Not in this slice: the loan plane (P5.3.2), child construction and supervision (P5.3.3), and the composed sample-plane exit condition (P5.3.4).
**Gates:** `just sel4_channel_check`
**Evidence:** [`devlog/2026-08-04-p5-3-1-channel-plane/`](../devlog/2026-08-04-p5-3-1-channel-plane/index.md)

**Depends on:** P5.2.

### P5.3.2 — Loan plane and generation-declared quotas on seL4

**Status:** Complete.
**Delivered:** `SharedBufferTable` already implemented loan/loan-map/return/revoke against real seL4 frames, but no dispatcher arm reached them, `SHARED_QUOTA` was a hardcoded constant applying `loan_count: 0` to every task rather than the generation's `shared-buffer-budget` resource, and `reclaim_holder` was called only from unit tests. This slice wires generation-decoded per-component quota ceilings, capability-named sealed-subrange loans that move (not copy) to the receiver, read-only receiver mapping, exactly-once return, four independently-enforced quota classes, and full reclamation on holder death.
**Exit condition (observed):** A component loans a sealed subrange to a receiver named by capability, the receiver — `sample-receiver`, **unmodified**, the same binary the x86 oracle runs — maps it read-only and returns it exactly once, and each of the four quota classes fails at ceiling+1 against generation-decoded limits without disturbing an unrelated holder; the graph drains to `loans=0 mappings=0 regions=0 transit=0 orphans=0 aliases=0`. Capability transfer over `send` landed here (the narrow, one-resource-kind form) rather than with P5.5, because a loan cannot reach its receiver without it; P5.5's `Operation::CapTransfer` remains the separate narrow-on-transfer operation C8.3 needs. Five denial arms are fault-injected; the page ceiling and the in-flight reclamation arm needed the fixture built specifically to strand a capability. Not in this slice: resolving a `bufferCreate` capability before admitting an allocation — recorded as **B13** in [`00-backlog.md`](00-backlog.md), deferred because closing it renumbers every component's capability slots, which is P5.3.3's distribution problem.
**Gates:** `just sel4_loan_check`
**Evidence:** [`devlog/2026-08-04-p5-3-2-loan-plane/`](../devlog/2026-08-04-p5-3-2-loan-plane/index.md)

**Depends on:** P5.3.1.

### P5.3.3 — Child construction and supervision on seL4

**Status:** Complete.
**Delivered:** `Spawn` resolved authority from the caller's declared grants and then refused; `SupervisionStatus`, `CapDrop`, and `EndpointCreate` had no handler, and `WaitSource::Supervision` resolved to `Unmediated`. This slice constructs a child from the grant-resolved executable with rights-bounded capability handoff (moving, not copying, channel ends), a supervision handle that answers "no outcome" while live and collects the outcome exactly once on death, parent wake on child death, and teardown clearing every registration. The bootstrap component's executable and factory slots are now placed from the boot layout rather than a running cursor.
**Exit condition (observed):** `init` constructs two unmodified children (`console`, `sysinfo` — the same binaries the x86 oracle builds, checked against sources) from grant-resolved executables, hands each its declared capabilities, and collects `sysinfo`'s clean exit through a supervision handle after being woken by its death. A child's channel is minted at runtime through the generation's declared `endpointCreate` grant as a move, not a copy — the broker shape P5.3.1 recorded as impossible until spawn existed to distribute halves through. Four denial arms are fault-injected; one found a real gap — with **B13's factory check removed, every gate still passed**, because no fixture held a budget and tried to allocate without a grant; the loan gate now names one. Not in this slice: the composed sample-plane exit condition (P5.3.4).
**Gates:** `just sel4_spawn_check`
**Evidence:** [`devlog/2026-08-05-p5-3-3-spawn-plane/`](../devlog/2026-08-05-p5-3-3-spawn-plane/index.md)

**Depends on:** P5.3.1.

### P5.3.4 — Sample-plane composition on seL4

**Status:** Complete.
**Delivered:** Composes the prior three slices into P5.3's stated exit condition: `sample-lender` and `sample-receiver`, unmodified, running the same ordered transcript `just sample_plane_live_check` records on x86, with a spawned child now taking the shared-buffer ceiling the generation declares for its component (previously only root-launched components were budgeted, so a spawned lender held `DENY`), and `serve_buffer_loan` accepting a `RIGHT_SUPERVISE` handle at `receiver_slot` alongside the P5.3.2 channel end — the shape the retired kernel's `init` uses, letting the component run unchanged.
**Exit condition (observed):** The unmodified `sample-lender` and `sample-receiver` exchange and return an 8192-byte payload over seL4 — 128× the 64-byte control-message bound — running the transcript `just sample_plane_live_check` records on x86, with the graph draining to `live=0 loans=0 mappings=0 regions=0 transit=0 orphans=0 aliases=0`. **B14 is closed here**: `init`'s declared budget is exactly two, so a third spawn is refused by the generation's own number rather than a global table size. Four denial arms are fault-injected, including the two changes above — with either removed the gate fails rather than passing. P5.3 is complete: all four sub-slices are observed.
**Gates:** `just sel4_sample_check`
**Evidence:** [`devlog/2026-08-05-p5-3-4-sample-plane/`](../devlog/2026-08-05-p5-3-4-sample-plane/index.md)

**Depends on:** P5.3.2 and P5.3.3.

### P5.5 — C8 typed fabric on seL4

**Status:** Complete — P5.5.1 and P5.5.2 both observed.
**Delivered:** Split out of P5.3 on 2026-08-04 (P5.3's exit condition names only C7-shaped properties despite its old title claiming C8 too) and decomposed 2026-08-05 into narrow-on-transfer provisioning (P5.5.1, C8.3-shaped: one route, one publisher, one subscriber) then the full stream plane (P5.5.2, C8.4-shaped: two publishers, two subscribers, two routes, `>MAX_INLINE_BYTES` and KEEP_LAST) — landing both in one slice would have made the reviewable claim depend on the unreviewable one. Threshold differs from C7: a C8 stream sample crosses at `maxInlineBytes = 32`, not the 64-byte control-message bound.
**Exit condition (observed):** One declared typed route carries a sample from a publisher to a subscriber over seL4, with the route endpoints provisioned by the fabric from the generation's declared edges, a re-delegation refused, and an undeclared participant denied. Both sub-slices below are observed.
**Gates:** see P5.5.1–P5.5.2 below.
**Evidence:** see P5.5.1–P5.5.2 below.

**Depends on:** P5.3.3 and C8.

### P5.5.1 — Narrow-on-transfer provisioning on seL4

**Status:** Complete, superseded by P5.5.2.
**Delivered:** `Operation::CapTransfer` was root-mediated but had no handler (fell to the dispatcher's catch-all, answered `unimplemented`) — the one mechanism a userspace fabric needs that neither `send` nor `spawn` provides. This slice delivers capability moves narrowed to exactly the descriptor's declared mask and object kind, `RIGHT_TRANSFER` dropped at the destination unless retained (non-delegable by construction), denial of a participant the graph declares no edge for even holding a real control endpoint, and a moved channel holder that moves with its capability.
**Exit condition (observed):** One declared `telemetry` route carries a sample from `fabric-publisher` to `fabric-subscriber` over seL4, with both endpoints provisioned by `fabric-service` through four narrow-on-transfer moves each landing with exactly one direction and no transfer bit; re-delegation and widening are refused, an undeclared participant is denied despite holding a real control endpoint, and the graph drains to `transit=0`. Two defects were found and fixed here, both latent since P5.3.1: `recv` parked the caller where the retired kernel's is non-blocking (now answers `ERR_WOULDBLOCK`, `wait` is the only operation that parks), and `resolve_channel` answered `-4` where `sys_send`/`sys_recv` both answer `ERR_BAD_CAP`. **B15 is closed here**, with a six-grant spawn observed under `just sel4_spawn_check`. A fourth denial arm — the transfer's *subset* test — was recorded as **uncovered** rather than claimed: no capability this graph could produce held transfer authority while narrower than its kind admits. **Superseded by P5.5.2** in two ways: the coverage gap is closed there (a spawn grant does produce such a capability, so the reasoning above was wrong), and this slice's gate, generation, and image are retired there — subsumed by a larger graph, with this exit condition staying observed by that gate.
**Gates:** `just sel4_fabric_check` (retired by P5.5.2; superseded by `just sel4_stream_check`, which asserts a superset)
**Evidence:** [`devlog/2026-08-05-p5-5-1-typed-fabric/`](../devlog/2026-08-05-p5-5-1-typed-fabric/index.md)

**Depends on:** P5.3.3.

### P5.5.2 — The full stream plane, unmodified, on seL4

**Status:** Complete.
**Delivered:** The C8.4 stream plane as the x86 oracle builds it: two publishers, two subscribers, two routes, the `>MAX_INLINE_BYTES` descriptor and loan path, and KEEP_LAST eviction with a stalled subscriber told exactly what it lost — every component unmodified, on P5.3.4's no-seL4-branch standard rather than P5.5.1's counted-branch one.
**Exit condition (observed):** All six fabric components run on seL4 with no seL4 branch in any of them, producing 48 markers across 10 causal chains — every marker the x86 gate also requires, plus one declared seL4-only marker for B17's arm. **B17 is closed, and its premise corrected**: the backlog held that no declarable graph could produce a capability holding transfer authority while narrower than its kind admits, but a plain spawn grant does, since `preflight_spawn_grants` installs the requested mask verbatim — deleting `rights & !source.rights` now fails this gate. **P5.5.1's gate, generation, and image are retired here**, its assertions a subset of this one's over a strictly larger graph; its exit condition stays observed, by this gate. A third ABI divergence was found and fixed, on the same pattern as P5.5.1's two: `shared_buffer_unmap` refused a loan slot where `sys_shared_buffer_unmap` accepts one. `MAX_CHANNELS` grew 16→32 and `MAX_GRAPH_TASKS` 16→`MAX_TASKS`, both previously sized against the wrong quantity. **Two scheduling races the retired kernel hides were found and fixed here (B18)**: a publisher wrote to an already-retired route, and `debug_write` emitted one syscall per byte so markers could interleave mid-string; `DebugWrite` is now served by the root's single-threaded graph loop, and the gate passes ten consecutive runs.
**Gates:** `just sel4_stream_check` (replaces P5.5.1's image rather than joining it)
**Evidence:** [`devlog/2026-08-05-p5-5-2-stream-plane/`](../devlog/2026-08-05-p5-5-2-stream-plane/index.md)

**Depends on:** P5.5.1.

### P5.4 — Retire the custom kernel

**Status:** Complete. Every sub-slice is closed; `kernel/` and its legacy-only orchestration are removed.
**Delivered:** The frozen custom kernel stayed unchanged until the equivalence inventory (P5.4.1) and all follow-on slices established the surviving contract; P5.4.final completed the coordinated cutover — portable contract checks moved to `boot-contracts`, runtime behavior moved to seL4 planes, seL4-supplied mechanism was explicitly reclassified, physical NVMe/Framework qualification stayed an open hardware milestone rather than false QEMU coverage, and the legacy build/check surface was retired with the directory. P5.4.1's inventory corrected this decomposition's original estimate in both directions — wider surface, larger uncovered set: C8.2 had no equivalent at all (not partial); two C8.5 assertions were invisible inside a C8.4-gated file; the M5.x/M6.x/B10/B11 class (nineteen closed oracle milestones — ten M5 gaps, five M6 gaps, two M6 partials, plus B10/B11) was never named at all, and turned out to be the larger half of the remaining work. Eight of nineteen `kernel/tests/*.rs` files had no named gate and were reachable only via `just test` — `boot.rs`, `component_image.rs`, `generation_manager.rs`, `isolation.rs`, `kernel_foundation.rs`, `object_store.rs`, `should_panic.rs`, `task_reclamation.rs` — holding 51 architecture-neutral assertions that would have disappeared silently without this inventory.
**Exit condition (observed):** Every sub-slice below (P5.4.1 through P5.4.final) is complete: every acceptance check the custom kernel guarded has an observed seL4 equivalent, and `kernel/` plus its legacy-only gates were removed in one reviewable change.
**Gates:** see P5.4.1–P5.4.final below.
**Evidence:** see P5.4.1–P5.4.final below.

**Depends on:** P5.3 and P5.5.

### P5.4.1 — The oracle equivalence inventory

**Status:** Complete.
**Delivered:** A map of every acceptance check the frozen oracle guards to its observed seL4 equivalent or an explicit recorded gap — the artifact P5.4's exit condition needed and no one had produced, covering all three legacy surfaces: direct `kernel/tests/*` targets, harness-mediated gates, and the eight kernel tests with no named gate (invisible to a Justfile-only audit). Also audited lifetime-vs-live resource bounds in `slime-root` as a class, closing [B22](00-backlog.md).
**Exit condition (observed):** All three legacy surfaces are mapped: 11 direct `kernel/tests/*` recipes with per-milestone verdicts, 24 legacy checkers of the 43 then present separated from 10 portable harness importers and 9 seL4 gates (this slice's own gate makes the current totals 44 and 10), and all 19 `kernel/tests/*.rs` files accounted for at 151 test entities — 130 architecture-neutral semantics seL4 must uphold against 21 custom mechanisms that die with `kernel/`. Every gap is named and assigned to a P5.4.2–P5.4.10 slice. The bounds audit closed **B22** (`ChannelTable` now reclaims through `channel::sweep`, gated by `just sel4_crossing_check`) and found a third table of the same shape, `SharedBufferTable::quotas`, opened and closed immediately as **B24** under `just sel4_supervision_check`; with those two the lifetime-vs-live class is closed. No architectural invariant in [`README.md`](README.md) changed. A cross-referencing script was considered and not written: `devlog_check` resolves `Gates`/`Roadmap` ids structurally, but no script can check that a claimed equivalence is *true* — that is re-established per slice as each gap closes.
**Gates:** `just devlog_check`
**Evidence:** [`devlog/2026-08-07-p5-4-1-oracle-inventory/`](../devlog/2026-08-07-p5-4-1-oracle-inventory/index.md)

**Depends on:** P5.3 and P5.5.

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
**Delivered:** `slime-root` previously decoded only `BootLayout` and `SharedBufferBudget`; the declared fabric-graph resource rode along in every generation and nothing read it, so a graph promising more than the mechanism could deliver would have launched — C8.2's exit condition was entirely unmet, not partial. This slice validates a generation's declared fabric graph against this root's own ceilings, one field at a time, before any component launches, using the same `boot_contracts` predicate the oracle uses.
**Exit condition (observed):** The stream plane's real two-publisher/two-subscriber graph is admitted against `slime-root`'s ceilings and the plane runs unchanged, asserted as `SLIME_ROOT fabric graph=admitted` and fault-injected by removing the wiring; the other eight planes report `absent`, distinguishing "checked" from "nothing to check". Not closed by this: the oracle's `kernel/tests/fabric_manifest.rs` also asserts route-authority tuples, interposition-chain termination, and per-pair QoS compatibility over the booted graph — those stay with P5.4.10's partials.
**Gates:** `just sel4_stream_check`
**Evidence:** [`devlog/2026-08-07-p5-4-4-fabric-graph-admission/`](../devlog/2026-08-07-p5-4-4-fabric-graph-admission/index.md)

**Depends on:** P5.4.1.

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
**Delivered:** Closed five gaps (M6.3 directory, M6.4 dango, M6.5 generation commands, M6.6 powerbox, M6.7 transfer) and two partials (M6.1 generation-v2 determinism, M6.2 protocol surface). The transfer manifest decoder, previously untested despite seven error variants and a self-excluding SHA-256, is now host-tested and Miri-clean with three fault injections confirmed. M6.5's management service drives list/inspect/stage/select/rollback for an unprivileged client holding no block capability of its own; building it surfaced two root defects in probe-`recv` semantics and `ERR_PEER_DEAD` delivery. M6.3 split mechanism (directory as root-owned `Resource::Directory` with a `ScopeTable`, reducing the unmediated surface from eight operations to five, then four once input was mediated) from policy (a filesystem service ported over `boot_contracts::object_store`, running the oracle's own unmodified `directory-probe`); it also caused and fixed a capability-table stack regression (128-byte scope inlined into `Resource` grew the tables ~96 KiB→~432 KiB, since fixed by interning scopes) and surfaced three placement/gating defects (`transferable` dropped by the placement mask, directories excluded from the send path, a stale gate control). `InputRead` became mediated via `Resource::Input` and a `resolve_wait_source` fix, leaving four unmediated operations by design (`StoreTransact`, `RecoveryReconstruct`, `GenerationTransact`, `GenerationReceive`). M6.4 (dango) closed **B30** — three root capability-placement defects in `construct_child`/`is_transferable` — plus four more defects (unready input wait, CSpace-exhausting reply-slot leak after 1220 calls, inconsistent boot/spawn slot order, `RIGHT_TRANSFER` misread as a resource kind). M6.6 (powerbox) mediates directory-authority handoff with provenance using only mechanism landed in earlier slices, and surfaced a third placement-order defect. M6.7 (transfer) closed **B29** via `device::MappedGranule`, a borrowed view letting two QEMU virtio-mmio transports share one 4 KiB page without a frame capability conflict, plus fixes for hardcoded device-0 placement and a rights-union bug that made a read-only source writable.
**Exit condition (observed):** Every M6 gap has an observed seL4 gate, including a transfer that actually crosses a boundary (digest, object closure, and travel policy verified before any write, staged pending, promoted only on health confirmation, host-verified byte-identical) rather than a manifest that merely decodes.
**Gates:** `just sel4_generation_check`, `just sel4_directory_check`, `just sel4_filesystem_check`, `just sel4_dango_check`, `just sel4_input_check`, `just sel4_powerbox_check`, `just sel4_transfer_check`
**Evidence:** [`devlog/2026-08-07-p5-4-3-transfer-manifest/`](../devlog/2026-08-07-p5-4-3-transfer-manifest/index.md), [`devlog/2026-08-08-p5-4-3-generation-plane/`](../devlog/2026-08-08-p5-4-3-generation-plane/index.md), [`devlog/2026-08-08-p5-4-3-directory-plane/`](../devlog/2026-08-08-p5-4-3-directory-plane/index.md), [`devlog/2026-08-08-p5-4-3-filesystem-plane/`](../devlog/2026-08-08-p5-4-3-filesystem-plane/index.md), [`devlog/2026-08-08-p5-4-3-dango-plane/`](../devlog/2026-08-08-p5-4-3-dango-plane/index.md), [`devlog/2026-08-08-p5-4-3-input-mediation/`](../devlog/2026-08-08-p5-4-3-input-mediation/index.md), [`devlog/2026-08-08-p5-4-3-powerbox-plane/`](../devlog/2026-08-08-p5-4-3-powerbox-plane/index.md), [`devlog/2026-08-08-p5-4-3-transfer-plane/`](../devlog/2026-08-08-p5-4-3-transfer-plane/index.md)

**Depends on:** P5.4.1.

### P5.4.5 — C8.5 reliable, retained, and timed QoS

**Status:** Complete.
**Delivered:** Closed **B28** — the blocker was `MAX_GRAPH_ITERATIONS = 512`, not a defect; the QoS plane needs 512–768 root round-trips. Added an eleventh image, `sel4-qos` (the stream graph plus a runtime-minted monotonic-time channel granted to `fabric-service` and `fabric-publisher-b`), closing the retained-head gap by fixture declaration (`fabric-publisher-b`'s diagnostics participant declared `retained` with `retainedDepth = 2`, ruled out as a scheduling artifact by two prior experiments) rather than by reordering. Matching-before-data, bounded loss under a stalled subscriber, and peer death as a distinct event were already covered by `just sel4_stream_check`, since the QoS logic lives in `fabric-service` and the stream plane boots it unmodified.
**Exit condition (observed):** `just sel4_qos_check` asserts fourteen markers across nine causal chains on the `sel4-qos` plane, observing five arms — RELIABLE retry accounting, retry exhaustion, deadline miss, lifespan expiry, liveliness loss — and reaching `[init] fabric stream complete`. One inversion is recorded for later readers: P5.4.10 made an incompatible QoS pair a refusal at admission, correct for a root with no QoS plane, but it meant the runtime event C8.5 requires was unreachable until this slice landed.
**Gates:** `just sel4_qos_check`, `just sel4_stream_check`
**Evidence:** [`devlog/2026-08-07-b28-iteration-budget/`](../devlog/2026-08-07-b28-iteration-budget/index.md), [`devlog/2026-08-07-p5-4-5-qos-arms/`](../devlog/2026-08-07-p5-4-5-qos-arms/index.md), [`devlog/2026-08-07-p5-4-5-qos-clock/`](../devlog/2026-08-07-p5-4-5-qos-clock/index.md)

**Depends on:** P5.4.1.

### P5.4.6 — C8.6 bounded native calls

**Status:** Complete.
**Delivered:** The `sel4-call` image exercises one `ParameterCall` route (two clients, one server, a capability-routed time source) where the parent vouches for each participant's identity to the broker via transferred supervision handles. Closed **B25**'s portability gap at the capability model: an endpoint capability now carries `Resource::Endpoint { channel, side }`, so a spawn grant is a non-consuming narrowing copy like every other grant, `ChannelTable` stores queues rather than one task holder per end, and operations needing a concrete task identity require a unique opposite-side holder. B25's representation change rewrote marker text four sibling gates read, so every seL4 plane gate was re-run; that found one lost assertion (the spawn gate's per-slot distribution marker deleted rather than replaced) and one root defect (`ChannelTable::live_queues` counting entries no capability table named), both fixed and gated.
**Exit condition (observed):** `just sel4_call_check` requires 50 markers across ten causal chains, counts exactly three parent-vouched supervision introductions, requires the non-idempotent request to execute exactly once, derives all five spawned task ids from the root's records, and requires one `status=0` exit for each plus init, rejecting root/graph/component/capability-transfer/fault/panic/wedge markers. `just sel4_gate_control_check` covers it in the global registry (12 gates rejecting 535 mutated transcripts and layouts, 71 of them this gate's own).
**Gates:** `just sel4_call_check`, `just sel4_gate_control_check`
**Evidence:** [`devlog/2026-08-08-b25-endpoint-copy-call-plane/`](../devlog/2026-08-08-b25-endpoint-copy-call-plane/index.md)

**Depends on:** P5.4.1.

### P5.4.7 — C8.7 bounded native operations

**Status:** Complete.
**Delivered:** A twelfth image, `sel4-operation`, carries a `navigation` operation route (two clients, a supervised replacement for the second, a server, a capability-routed clock) using P5.4.6's parent-vouched composition. The broker and all five participants are the oracle's binaries, unmodified — the generation sets the oracle's own `SLIME_FABRIC_OPERATION_CHECK` alongside its seL4 flag, so only `init`'s composition differs. No new root mechanism was needed: C8.7 composes over primitives `slime-root` already answers (spawn, endpoint mint, channel IPC, capability transfer, supervision, transfer-window staging). Two composition facts specific to this plane: the restart replacement is a declared identity admitted on a channel the dead participant never held while retaining correlation state, and a private release barrier keeps its role request from overtaking the retained result it must find.
**Exit condition (observed):** `just sel4_operation_check` requires 53 markers across twelve causal chains, counts the replacement's provisioning to require exactly one, derives all six spawned task ids from the root's own records, requires one `status=0` exit for each plus init, and requires exactly four parent-vouched supervision introductions. `just sel4_gate_control_check` covers it in the global registry at a pinned 53 markers (13 gates rejecting 610 mutated transcripts and layouts) and `just sel4_boot_layout_check` freezes its eight-row table. One recorded limit, narrower than the full claim: the fourth check's "leaves unrelated stream, call, and operation routes live" is proven here for an unrelated *operation* route only, since this graph declares no stream or call route — a graph carrying all three belongs to C8.10 and P5.4.9.
**Gates:** `just sel4_operation_check`, `just sel4_gate_control_check`, `just sel4_boot_layout_check`
**Evidence:** [`devlog/2026-08-08-p5-4-7-operation-plane/`](../devlog/2026-08-08-p5-4-7-operation-plane/index.md)

**Depends on:** P5.4.1 and P5.4.6.

### P5.4.8 — C8.8 filtered introspection and declared interposition

**Status:** Complete.
**Delivered:** A thirteenth image, `sel4-visibility`, carries the stream graph plus one declared interposition (`fabric-intruder` on the telemetry subscriber's chain), with the oracle's binaries unmodified — only `init`'s composition differs. Each participant is spawned with exactly one capability, its own control endpoint, so "the proxy relays only its declared route" is a statement about what the broker transferred rather than what the parent withheld. The interposition chain is profile-borne: participants declare `interposition = []` and the `sel4` profile supplies the chain via `resolve_fabric_graph`, mirroring the oracle's own `visibility` profile. This slice found a root defect: `DebugWrite` read its staged payload through the message reader bounded at `MAX_MESSAGE_BYTES` (64), so every 64-byte record printed as 128 hex characters was refused as `InvalidLength`; fixed by switching to the 1 KiB `read_staged_array` reader — no earlier plane could have found it, since every marker the other twelve gates assert is under 64 bytes.
**Exit condition (observed):** `just sel4_visibility_check` requires 25 markers across seven causal chains, re-deriving the oracle's two structural claims (exactly twelve serialized view records, exactly two interposition traces that differ from each other) and requiring zero component failures in the composition window. `just sel4_gate_control_check` covers it at a pinned 25 markers (14 gates rejecting 653 mutations) and `just sel4_boot_layout_check` freezes its eight-row table. One recorded limit: the fourth check's byte-identical-across-runs half is inherited rather than re-observed — the oracle boots its profile twice and compares records byte-for-byte, while this gate boots once and asserts count and distinctness only.
**Gates:** `just sel4_visibility_check`, `just sel4_gate_control_check`, `just sel4_boot_layout_check`
**Evidence:** [`devlog/2026-08-08-p5-4-8-visibility-plane/`](../devlog/2026-08-08-p5-4-8-visibility-plane/index.md)

**Depends on:** P5.4.1.

### P5.4.9 — C8.9 typed full-profile closure and C8.10 full-graph bootstrap

**Status:** Complete.
**Delivered:** C8.9 needed no port: its substance is host-side, and `build_sel4_generation` calls the same `resolve_fabric_profile`/`render_fabric_profile_rust` every x86 profile calls, so every graph-bearing seL4 fixture already exercised it. This slice added scale rather than a mechanism — generation 22 declares five routes, four schemas, fifteen participants, and every call/operation ceiling non-zero at once, so a mutually unsatisfiable combination fails the build rather than the boot. C8.10 is `just sel4_boot_check`: a fourteenth image, `sel4-boot`, carries the stream, call, and operation planes plus an unauthorized probe, a declared interposition proxy, and a filtered-introspection client, all launched concurrently. Its layout differs structurally from the oracle's (oracle materializes declared control channels into the bootstrap layout; this root's `init` mints them per plane since P5.4.6), so `SEL4_BOOT_LAYOUT` is 21 rows against the oracle's 53. Raised two root bounds sized against single-plane graphs and prone to B28's class of failure: `channel::MAX_CHANNELS` and `task::MAX_TASKS`, both 32 → 48 (peaks were 37 channels / 37 tasks; raised past the first-passing number per B28's rule). `sel4_crossing_check`'s loop bound and `sel4_root_boot_check`'s pinned reclaimed CSlot ranges were updated to match, with the width-and-adjacency property confirmed unchanged.
**Exit condition (observed):** Every item in C8.9's and C8.10's required-checks lists has an observed seL4 gate: 44 markers across sixteen causal chains, exactly one init layout report with every slot distinct and under ceiling, both spawning parents with no component spawned twice, eleven checked roles, four declared role-less idles, the probe refused from both sides, and an inverted lifecycle check (idle is success — no composition task may exit). One recorded limit: this plane provisions and rests rather than carrying traffic, which is C8.10's own exit condition; per-plane traffic is covered by the stream, call, operation, and visibility gates against the same unmodified brokers.
**Gates:** `just sel4_boot_check`, `just data_fabric_profile_check`, `just sel4_crossing_check`, `just sel4_root_boot_check`
**Evidence:** [`devlog/2026-08-08-p5-4-9-full-graph-boot/`](../devlog/2026-08-08-p5-4-9-full-graph-boot/index.md)

**Depends on:** P5.4.1, and P5.4.6 through P5.4.8 for the planes it composes.

### P5.4.10 — The recorded partials

**Status:** Done.
**Delivered:** Nine gaps P5.4.1 recorded, each too small for its own slice and not mapping one-to-one onto oracle milestones, collected so none was lost. Six closed by gates or tests: `component_image.rs`'s malformed-segment corpus (`boot_contracts::component_image::validate_segments`); C8.1's tag-collision rejection (`distinct_schemas_may_share_no_type_tag`); C8.2's route-authority/interposition-termination/per-pair-QoS arms (aggregate half closed by P5.4.4, membership/interposition enforced by `FabricGraph::decode`, per-pair QoS refused at admission); C8.3's graph provenance (`GenerationError::UndeclaredFabricParticipant`, checked against the generation's declared component names rather than a direct comparison to the fabric's `@generated` table — the same fixture backs both, so the check catches the drift C8.3 cares about without being a literal artifact comparison); C8.4's structural arm (admission marker plus `sel4_stream_check` for fan-out, P5.4.4's `validate_against` for bounds); B10's seL4 layout fixtures (`sel4_boot_layout_check` freezing all eight plane layouts). Two reclassified as needing no seL4 gate, with evidence and reopening conditions recorded: C7.1's retained-v2 rollback arm (chronologically unreachable — v2 predates the ELF component revision, so every v2 payload is an unloadable SLIMECM image, already asserted unloadable by `sel4_root_boot_check`'s `slimecm=[1-9]` marker; decode path stays host-tested in `boot-contracts`); B11's product-vs-test profile pair (structurally absent — seL4 fixtures are per-scenario siblings rather than one shared manifest, so there is no shared graph to contaminate). One partial as far as the allocator allows: `task_reclamation.rs`'s per-cycle drift, cost scaling, and rejected-spawn conservation — `sel4_root_boot_check` now pins each task's reclaimed CSlot range exactly, but the three named properties are frame-count differentials and root CSlots are never returned to the allocator, so a free-count comparison is flat by construction. Also corrects an earlier "two of fifteen" `component_image.rs` count, caught by independent review: six of fifteen oracle cases have no direct port, five of those six already covered by `boot-contracts`' own header tests.
**Exit condition (observed):** Every row is closed or explicitly reclassified, each with its own devlog entry; each row's reasoning lives in that entry rather than summarized here, since two of the nine resolutions are "this cannot happen on this path" and that claim is only worth what its evidence is.
**Gates:** `just test_host`, `just miri`, `just sel4_stream_check`, `just sel4_boot_layout_check`, `just sel4_root_boot_check`
**Evidence:** [`devlog/2026-08-07-p5-4-10-segment-corpus/`](../devlog/2026-08-07-p5-4-10-segment-corpus/index.md), [`devlog/2026-08-07-p5-4-10-collision-and-provenance/`](../devlog/2026-08-07-p5-4-10-collision-and-provenance/index.md), [`devlog/2026-08-07-p5-4-10-qos-pair-admission/`](../devlog/2026-08-07-p5-4-10-qos-pair-admission/index.md), [`devlog/2026-08-07-p5-4-10-graph-shape/`](../devlog/2026-08-07-p5-4-10-graph-shape/index.md), [`devlog/2026-08-07-p5-4-10-sel4-boot-layout/`](../devlog/2026-08-07-p5-4-10-sel4-boot-layout/index.md), [`devlog/2026-08-07-p5-4-10-slot-conservation/`](../devlog/2026-08-07-p5-4-10-slot-conservation/index.md)

**Depends on:** P5.4.1.

### P5.4.final — Delete `kernel/`

**Status:** Complete.
**Delivered:** Closed or deliberately reclassified all six deletion-audit findings, then removed `kernel/` and its legacy-only gates together with the workspace member, component legacy transport, custom-kernel build scripts, oracle checkers, harness artifact selector, CI targets, and generation-builder dependency on a custom-kernel ELF. Historical Justfile identifiers either resolve to their seL4/host successor or fail closed where the required physical product path does not yet exist. Dispositions: (1) task reclamation — the oracle's free-frame differential is not meaningful under seL4's monotonic allocator, replaced by `sel4_root_boot_check`'s exact reclaimed-CSlot-range pins and shared-buffer teardown-to-zero checks; (2) component-image shape corpus — moved to host-tested `boot_contracts::component_image`, independent of any kernel loader; (3) NVMe — deliberately not claimed, since the retired QEMU/custom-kernel transport was never product evidence for M5.7's required physical Framework observation; `storage_nvme_read_check` fails closed and M5.7 stays explicitly blocked; (4) custom stage-0/EL1 boot — reclassified as historical P2.1 evidence, not a seL4 runtime acceptance property, with both UEFI stage-0 targets kept compiled and linted; (5) PMM/VMM/heap/APIC foundation tests — reclassified as tests of mechanism now supplied by seL4, observed instead at the product boundary; (6) smoke/panic/IPC fault isolation — covered by `sel4_root_boot_check`'s clean-and-faulting-child boot and `sel4_gate_control_check`'s proof that every seL4 marker gate turns red when required evidence is removed, reordered, or contradicted.
**Exit condition (observed):** Every acceptance property retained by the product has an observed seL4 or host contract gate, deliberate non-equivalences are recorded without claiming them, and `kernel/` plus its legacy-only gates were removed in one reviewable change.
**Gates:** `just sel4_root_boot_check`, `just sel4_gate_control_check`, `just test_host`, `just storage_nvme_read_check`
**Evidence:** [`devlog/2026-08-08-p5-4-final-deletion-audit/`](../devlog/2026-08-08-p5-4-final-deletion-audit/index.md), [`devlog/2026-08-09-p5-4-final-kernel-retirement/`](../devlog/2026-08-09-p5-4-final-kernel-retirement/index.md)

**Depends on:** every P5.4.2+ slice.

## MCU and embedded-companion boundary

Cortex-M, RV32 microcontrollers, and other systems without the admitted MMU and user/supervisor isolation baseline do not run a weakened form of this kernel. They are external devices reached through bounded userspace services.

A later companion profile may admit micro-ROS/XRCE-DDS, `zenoh-pico`, or a smaller Zutai protocol over an exact serial, CAN, USB, or network capability. `zenoh-pico` is a candidate *here* and not as a Slime component: its C toolchain, `z_malloc`/`z_realloc`/`z_free` requirement, and BSD-socket-shaped port API are properties of the microcontroller it runs on rather than obligations on Slime, and the demo's Zenoh transport is what it would peer with. That profile must declare peer identity, types, directions, payload size, frequency, queue depth, timeout, reset behavior, and actuator authority. Disconnect, malformed traffic, reboot, and resource exhaustion become structured C8/C9 events; the companion never receives ambient graph, network, storage, or device authority.

## Verification policy

- P0 contract changes run `just contracts_check` and `just generation_check` in addition to their narrow target.
- Permanent Rust changes run the repository format and lint gates for every affected workspace.
- P1–P3 run the narrowest architecture QEMU target and then the shared semantic corpus named by the slice.
- A pass on one ISA cannot close another ISA's gate. A QEMU pass cannot close P4 or any RP physical demo milestone.
- Cross-architecture comparisons assert normalized semantic events and authenticated artifacts; they do not claim byte-identical register frames, page tables, physical addresses, or device traces.
