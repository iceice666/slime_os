# P6 returns the active architecture lane to x86-64 seL4

| Field | Value |
|---|---|
| Date | 2026-09-02 |
| Kind | Decision |
| Status | Proposed |
| Scope | x86-64 target admission, seL4 pc99 QEMU boot, GRUB Multiboot2, UEFI removable media, Framework CPU boot, H1 and M5.7 sequencing |
| Work items | 01a09e12-117f-7568-b21a-2935b67a1865, 01a09e12-11a9-760e-acd7-be857217f6c3, 01a09e12-11d2-7b4f-8438-e0d867a5b2de, 01a09e12-11fa-7b9d-95ff-335ba058031a, 01a09e12-1223-7f64-b86a-5f807342bc2f, 01a09e12-124b-7ee5-b223-a024983557d8, 01a09e12-1274-7a7f-b757-f86b5854f667, 01a0724c-5400-7321-ac99-8f66ea7449f1, 01a0724c-5400-7e51-91a6-3529f25f3389 |
| Gates | none |
| Trigger | The completed Milk-V Duo architecture lane made x86-64 Framework the selected next physical bring-up target. |
| Baseline | The surviving product path booted upstream seL4 on AArch64 and RV64; x86-64 retained only a retired custom-kernel oracle and no current seL4 image or Framework evidence path. |

## Summary

The roadmap now makes P6 the active architecture lane: restore an x86-64 upstream-seL4 product under pinned QEMU q35/OVMF, package the same GRUB Multiboot2 file tree into one deterministic GPT/EFI image, and boot that exact image on the named Framework without internal-storage write authority. The decision separates CPU/product boot from hardware qualification: P6.6 unblocks H1 but cannot complete inventory, PCI, DMA, input, storage, network, display, or daily-driver claims.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Architecture portability | Added P6.1–P6.6 from target admission through physical removable-media CPU boot. | QEMU evidence precedes media construction, and the QEMU-proven media precedes a physical claim. |
| Boot contract | Selected seL4 pc99's native GRUB Multiboot2 route instead of extending the ARM/RISC-V rust-sel4 loader. | QEMU and Framework consume one boot layout rather than independently assembled artifacts. |
| Platform hardware | Made H1 depend on P6.6 and begin with inventory after the resident product already boots. | CPU boot cannot be mistaken for device discovery or qualification. |
| Storage foundations | Made M5.7 depend on the P6.6/H1/H2/H4 chain and kept NVMe absent from P6. | A removable-media boot cannot silently satisfy transport, DMA, persistence, or internal-write promotion. |
| Roadmap index | Replaced the completed Duo lane as the current goal and added the x86-64 CPU/product release boundary. | The roadmap index names the same next gate and evidence boundary as the owning tracks. |

## Decisions

- Decision: Add exact `x86_64-sel4-qemu-pc99` and Framework target profiles instead of reviving `x86_64-qemu-virtio`.
- Rationale: The retired profile identifies the deleted custom-kernel ABI; reusing it would make incompatible executables and boot contracts appear equivalent.
- Rejected alternative: Treat QEMU q35 and the Framework as one target profile. Firmware, ACPI/APIC topology, media handoff, and physical evidence make them distinct complete platform contracts.
- Decision: Use GRUB Multiboot2 as the shared x86 seL4 boot contract.
- Rationale: Upstream seL4 pc99 already consumes Multiboot modules, while the pinned rust-sel4 loader implements only Arm and RISC-V.
- Rejected alternative: First build a QEMU-only loader path and later replace it for USB. That would create two boot paths and leave the physical artifact unproven by the QEMU gate.
- Decision: Build a deterministic raw GPT/EFI image before an optional hybrid ISO.
- Rationale: The raw image is the exact artifact QEMU can boot and the removable-media writer can hash-verify; it also preserves a controlled path for later storage partitions without adding them to P6.
- Rejected alternative: Make ISO production the milestone exit. ISO boot alone does not prove the exact USB disk layout later used on the Framework.

## Open risks and follow-ups

- [ ] P6.1 must select and pin the exact Framework model and target-profile name before generated contracts land.
- [ ] P6.3 must select a userspace timer/IRQ source that works for the pc99 reference without assuming the same hardware on the Framework.
- [ ] P6.3 must define the x86-64 thread-pointer register shared by root task setup and the component runtime.
- [ ] P6.6 needs an observed evidence channel; QEMU COM1 must not be assumed to exist on the Framework, so GOP is the required baseline.
- [ ] Planned P6 gate names become real Justfile targets only with their implementation slices.

## Artifacts and provenance

- Related roadmap item: [`roadmap/07-architecture-portability.md` P6](../../roadmap/07-architecture-portability.md#p6-x86-64-sel4-qemu-and-framework-cpu-boot)
- Related hardware sequence: [`roadmap/04-platform-hardware.md` H1](../../roadmap/04-platform-hardware.md#h1-framework-evidence-harness-and-hardware-inventory)
- Related storage boundary: [`roadmap/01-foundations.md` M5.7](../../roadmap/01-foundations.md#m57--framework-nvme-transport-and-safety-promotion)
