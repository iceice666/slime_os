# Architecture and target profiles

## Portable contract versus platform mechanism

Portable semantics include capability kinds and rights, channels, generation
selection, rollback, component and protocol schemas, shared buffers, fabric
routes, wait-set behavior, lifecycle policy, I/O queue/epoch/lease rules, and
semantic block/link/network services.

Platform mechanism includes register frames, page tables, interrupt controllers,
timers, idle and QEMU-exit paths, firmware handoff, device-tree or ACPI parsing,
MMIO addresses, PCI functions and BARs, interrupt routes, IOMMU identifiers, and
early boot mappings. These facts stay in the platform profile and owning source;
they do not enter a portable service ABI.

One logical syscall operation has one semantic contract and error model, but each
architecture has an explicit calling convention. Architecture ports preserve
observable capability, fault, wait/wake, reclamation, generation, and rollback
semantics; they need not have byte-identical register or page-table evidence.

## Admitted profiles

`contracts/target-profile/v1/schema.zt` declares nine profiles. Three are
retired custom-kernel QEMU identities retained to classify old artifacts:
`x86_64-qemu-virtio`, `aarch64-qemu-virt`, and `riscv64-qemu-virt`.
`aarch64-rpi5` also retains its custom-kernel ABI declaration, although the
seL4 builder selects that name for the Raspberry Pi 5 platform. Do not read
that build route as a newly admitted seL4 ABI or observed board execution.
The five explicitly seL4-qualified identities are:

| Profile | Current role |
| --- | --- |
| `aarch64-sel4-qemu-virt` | primary automated product and complete plane corpus |
| `riscv64-sel4-qemu-virt` | RV64 QEMU architecture reference |
| `riscv64-sel4-milkv-duo` | qualified named Milk-V Duo physical architecture/product path |
| `x86_64-sel4-qemu-pc99` | x86-64 QEMU pc99 product reference |
| `x86_64-sel4-framework13-ai300` | exact named Framework removable-media CPU/product-boot profile |

The build platform table in `scripts/build/build-sel4.py` selects the three QEMU
platforms, the Raspberry Pi 5 build platform, the Milk-V Duo platform, and the
Framework profile over the pc99 kernel inputs. A profile identifies a complete
architecture/ABI/page-feature/platform contract, not just an ISA.

## Automated and physical evidence

`aarch64-sel4-qemu-virt` is the default development target. The cross-target
corpus also exercises RV64 QEMU and selected x86-64 pc99 paths. QEMU establishes
deterministic behavior for its exact image and profile only.

The named Milk-V Duo has retained physical evidence for its upstream-seL4
architecture and product path. The Raspberry Pi 5 kernel, loader, and media build
path exists, but no board boot was observed; its gate fails closed without serial
evidence.

The named Framework 13 cold-booted the exact P6 removable image twice on
2026-09-13 and rendered the resident readiness record through the firmware
framebuffer without internal-storage write authority. That observation qualifies
the CPU/product boot path only. The current observation is retained but its gate
is temporarily unbound from the rebuildable root image; a new operator boot is
required to rebind it. No device inventory, NVMe, USB, input, network, display,
audio, suspend, IOMMU, or daily-driver claim follows.

## Target-specific non-equivalence

- AArch64 and RISC-V mappings can express execute-never for component data.
  x86-64 seL4's mapping attributes expose no NX bit, so data-page W^X is
  currently unenforced there.
- AArch64 QEMU exposes EL0 physical counter/timer registers globally because the
  root uses that timer path; clock service authority does not make those raw
  registers inaccessible to hostile native code.
- QEMU DMA is trusted. Physical containment requires the exact platform's
  IOMMU/SMMU evidence.
- A CPU boot qualifies no device. Evidence from one physical target completes
  no other target's gate.

## Safety boundary

Physical development boots from removable media and must not write internal
NVMe. Internal storage promotion waits for target-specific identity, bounds,
DMA containment, timeout/reset, flush ordering, interruption, malformed metadata,
rollback, and recovery evidence. Destructive work belongs on an approved external
device.

Current unimplemented target qualifications live in
[`../plans/framework-hardware.md`](../plans/framework-hardware.md) and
[`../plans/rpi5-ros2-demo.md`](../plans/rpi5-ros2-demo.md).

## Verification

- `just sel4_pin_check`
- `just x86_64_sel4_image_check`
- `just x86_64_sel4_root_boot_check`
- `just x86_64_qemu_check`
- `just framework_media_check`
- `just framework_cpu_boot_check` — currently expected to refuse the stale image
  binding rather than manufacture a physical pass
- the Milk-V Duo and Raspberry Pi 5 targets in `just/hardware.just`
