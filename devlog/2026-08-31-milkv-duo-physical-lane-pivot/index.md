# Milk-V Duo becomes the current physical bring-up lane

| Field | Value |
|---|---|
| Date | 2026-08-31 |
| Kind | Decision |
| Status | Verified |
| Scope | `roadmap/{README,01-foundations,04-platform-hardware,07-architecture-portability,09-rpi5-ros2-demo}.md`, `devlog/README.md` |
| Roadmap | P3, P3.D, P3.E, P4, RP3, M5.7, H1 |
| Gates | `just devlog_check` |
| Trigger | P3.D established the project's only observed, repeatable, hands-off physical deployment and serial evidence loop, while the available Raspberry Pi 5 USB-UART adapter produced no bytes |
| Baseline | Raspberry Pi 5 remained the active product-leading physical lane despite having no usable evidence path; Milk-V Duo was recorded as a deferred RV64 follow-up even after its board loop passed three consecutive runs |

## Summary

Milk-V Duo is now the current physical architecture bring-up and evidence lane.
The decision promotes the observed P3.D USB-NCM deployment, FIT handoff, serial
control, and recovery loop into the execution path for P3 RV64 QEMU and P3.E
seL4-on-Duo work. It does not rename Duo as the Raspberry Pi 5 product, replace
the Framework daily-driver target, or let evidence cross target-specific
boundaries. RP0–RP2 and the reproducible Raspberry Pi 5 build remain completed
history; RP3–RP8, M5.7, and H1–H14 retain their original physical exit
conditions and are deferred rather than erased.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `roadmap/README.md` | Replaced the blocked Raspberry Pi 5 execution sequence with P3 RV64 QEMU, P3.E risk qualification, physical seL4/root boot, verified generation, and component replay on Duo | The active lane has an observed evidence transport instead of depending on unavailable hardware output |
| `roadmap/07-architecture-portability.md` | Made P3 and P3.E current, added the `riscv64-sel4-milkv-duo` profile role, and placed memory fit, PLIC layout, and C906 MAEE state ahead of the platform port | A custom payload pass cannot imply a kernel port, and unknown SoC facts cannot be guessed into seL4 |
| `roadmap/09-rpi5-ros2-demo.md` | Deferred RP3–RP8 while retaining RP0–RP2 and every Raspberry Pi 5 acceptance condition | Reprioritization preserves completed evidence and does not substitute another board for the named product target |
| `roadmap/01-foundations.md` | Kept M5.7 open on a seL4 NVMe transport and physical Framework observation | Duo storage or component evidence cannot promote internal Framework NVMe writes |
| `roadmap/04-platform-hardware.md` | Kept H1–H14 Framework-specific and explicitly rejected cross-target completion | ACPI, PCI, AMD-IOMMU, NVMe, USB, input, display, wireless, and suspend claims remain owned by the hardware that must prove them |

## Decisions

- **Decision:** Use Milk-V Duo as the current physical architecture bring-up and
  evidence target.
- **Rationale:** P3.D observed three consecutive hands-off runs through one
  digest-verified USB-NCM deployment and serial evidence loop. The Raspberry Pi
  5 build exists, but the available USB-UART adapter produces no bytes and seL4
  supplies no display path that can replace early serial evidence.
- **Rejected alternative:** Leave Raspberry Pi 5 as the active lane and wait for
  a different USB-UART adapter. That is a cheaper board unblock, but it leaves
  all current physical execution dependent on unavailable evidence while an
  already observed loop remains deferred.

- **Decision:** Keep the pivot architecture-scoped until a measured component
  vertical slice exists.
- **Rationale:** P3.D proves firmware handoff and iteration only. P3.E still has
  three load-bearing risks: the 63.25 MiB DRAM window, an unconfirmed CV1800B
  PLIC S-mode context layout, and unaudited T-Head C906 MAEE state in Sv39 PTE
  bits. The roadmap therefore requires the RV64 QEMU corpus and those risk gates
  before the physical seL4/root claim.
- **Rejected alternative:** Rename the near-term ROS 2 product release to Duo
  immediately. No Slime generation, component graph, network backend, or ROS
  workload has run on the board, so that would convert an architecture decision
  into an unsupported product claim.

- **Decision:** Preserve Raspberry Pi 5, Framework, and M5.7 as independent
  deferred targets with unchanged evidence requirements.
- **Rationale:** Board identity is part of the signed target and of every
  physical claim. Duo cannot establish Raspberry Pi 5 firmware/timer/storage
  behavior or Framework ACPI/PCI/AMD-IOMMU/NVMe/input/display behavior.
- **Rejected alternative:** Treat a successful Duo component or storage path as
  portable physical evidence. Portable contracts may be reused, but physical
  containment, device identity, firmware behavior, and no-write claims remain
  target-specific.

## Open risks and follow-ups

- [x] P3 established the pinned `riscv64-sel4-qemu-virt` reference profile and replayed the selected architecture-neutral corpus before P3.E's physical claim.
- [x] P3.E measured the complete minimum image and initial-object placement inside the observed 63.25 MiB Duo DRAM window.
- [x] P3.E selected the CV1800B PLIC S-mode context from the SoC layout and observed IRQ delivery before and after graph activation.
- [x] The physical probe observed `sxstatus.MAEE=1`, and the loader/kernel apply the required C906 page-table encoding.
- [x] A verified generation and architecture-neutral component vertical slice produced repeatable physical evidence; selecting a Duo product workload remains a separate roadmap decision.
- [ ] Resume RP3 only with a working Raspberry Pi 5 serial evidence path and an explicit product reprioritization; resume M5.7/H1 only with their named Framework prerequisites.

## Artifacts and provenance

- Focused report: none; the decision and target boundaries are recorded directly in the changed roadmap files.
- Raw transcript: none added. This decision inherits the observed board transcript from P3.D rather than creating new hardware evidence.
- Serial/debugger/model output: [`P3.D Duo boot serial`](../2026-08-29-p3d-milkv-duo-bringup/duo-boot-serial.log).
- Related roadmap items: [P3/P3.D/P3.E/P4](../../roadmap/07-architecture-portability.md), [RP3](../../roadmap/09-rpi5-ros2-demo.md), [M5.7](../../roadmap/01-foundations.md), and [H1](../../roadmap/04-platform-hardware.md).
- Existing evidence: [`P3.D Milk-V Duo bring-up`](../2026-08-29-p3d-milkv-duo-bringup/index.md).