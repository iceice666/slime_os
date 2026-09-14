# Framework hardware qualification

## Goal and boundary

Qualify the named Framework Laptop 13's actual firmware and devices after the
upstream-seL4 CPU/product boot. The existing removable-media observation proves
only that the resident product graph reached readiness without internal-storage
write authority. It proves no device support.

Work-item identity, dependencies, state, and per-slice exit conditions remain in
`.tasks/items/`. This plan owns the stable delivery and qualification boundary.

## Required sequence

1. **Inventory:** produce a bounded, versioned physical report for ACPI tables,
   MCFG functions, BARs, interrupt routes, framebuffer, IOMMU presence, NVMe
   identity, and input-controller stages; preserve the internal-NVMe comparison
   region and fail closed without a fresh exact-image observation.
2. **PCI binding:** project validated platform facts into exact IO1 device, MMIO,
   IRQ, and DMA capabilities without ambient enumeration or portable-ABI leakage.
3. **USB/input logic:** implement xHCI, USB core, HID, and a transport-neutral seat
   service under deterministic checks before physical DMA promotion.
4. **DMA containment:** establish one AMD-IOMMU domain per driver, map only live
   IO0 leases, report faults, and enable bus mastering only after containment.
5. **Device services:** qualify disposable USB storage and Ethernet before
   internal NVMe, then display/compositor, platform power, suspend/resume,
   touchpad, audio, Wi-Fi, Bluetooth, and Radeon paths through typed services.
6. **Integrated promotion:** observe the complete target under removable-media
   recovery and no unauthorized internal-storage modification.

## Invariants

- PCI identities, ACPI routes, BARs, APIC vectors, IOMMU aliases, and firmware
  methods remain exact Framework profile data.
- Drivers hold hardware authority; ordinary clients hold semantic service
  capabilities only.
- Before IOMMU qualification, DMA-capable physical work is restricted to
  approved disposable/read-only probes and makes no containment claim.
- Restart and rollback create fresh resource bindings and IO0 epochs; stale
  MMIO, IRQ, DMA, and completions are refused and reclaimed.
- Internal NVMe writes remain disabled until identity, bounds, reset/timeout,
  flush ordering, interrupted writes, malformed metadata, rollback, recovery,
  and containment are observed on disposable hardware first.
- QEMU and other boards validate portable mechanism only. Every Framework device
  promotion needs physical evidence for the exact image and generation.

## Completion boundary

A daily-driver claim requires usable input, display, audio, storage, network, and
power paths through explicit capabilities; IOMMU-confined DMA; repeated
suspend/resume with fresh mappings and epochs; recovery media; bounded resource
use; and no unauthorized internal-storage modification. Partial CPU boot,
framebuffer output, or one device driver cannot satisfy it.

## Current first step

The next qualification surface is the evidence-backed hardware inventory. The
historical CPU boot remains observed but its current gate refuses because the
recorded root image is no longer rebuildable byte-for-byte. A new operator boot
of the rebuilt image is required; documentation cannot re-stamp the record.

## Owning references

- [`../architecture/targets-and-portability.md`](../architecture/targets-and-portability.md)
- [`../architecture/io-substrate.md`](../architecture/io-substrate.md)
- `evidence/framework-cpu-boot/`
- `scripts/check/check-framework-cpu-boot.py`
- the Framework-tagged work items under `.tasks/items/`
