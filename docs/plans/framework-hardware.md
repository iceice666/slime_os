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

The daily-driver release requires H1–H14 plus the common IO slices each H
milestone consumes; M5.7 separately gates the storage-aware boot. No P6, Duo,
QEMU, or CPU-only evidence satisfies a Framework device gate.

The Hardware track is complete only when all of the following are observed on
the named Framework and backed by deterministic IO and device checks. Evidence
from Milk-V Duo, Raspberry Pi 5, or QEMU validates portable mechanisms only:

- every DMA-capable physical driver consumes IO1 resources bound by H2 and runs
  in an H4 AMD-IOMMU domain mapping only live IO0 leases, with fault isolation
  and supervised restart;
- built-in or attached keyboard, touchpad, pointer, display, audio, USB storage,
  USB Ethernet, Wi-Fi, and Bluetooth are usable through typed services;
- applications receive input, display surfaces, audio streams, files, and IO4
  network destinations only through explicit capabilities; the manifest shows
  which component can reach each device or remote endpoint;
- internal NVMe writes pass the [M5.7](../../roadmap/01-foundations.md), IO2,
  and H7 bounds, reset, flush, interruption, malformed-metadata, device-identity,
  rollback, and recovery gates on disposable hardware before target promotion;
- compositor and Radeon survive client faults, stale completions, driver reset,
  and suspend/resume with a software-rendered fallback;
- battery, charger, brightness, lid, and thermal state are observable, controls
  explicitly authorized, and policy owned by userspace;
- repeated suspend/resume quiesces and restores storage, IOMMU, USB, input,
  display, network, wireless, and audio with fresh mappings/epochs and no
  BootState corruption;
- per-component resource and energy accounting is visible and bounded without
  silently introducing [C9 scheduling authority](../architecture/runtime-authority.md)
  or [A1 revocation](../../roadmap/06-authority-trust.md);
- a busy-looping background component is throttled past its generation-declared
  energy budget, with accounting readable per component;
- an interactive foreground component keeps its declared latency while a
  background container saturates every CPU; the container cannot claim ungranted
  foreground scheduling share, and each component's class is visible in the
  manifest. This requires physical qualification; QEMU ordering evidence cannot
  close it;
- integrated physical qualification has no unauthorized internal-storage
  modification or unbounded resource growth, and retains a bootable signed
  removable recovery path.

Every permanent hardware change requires its narrow deterministic device/service
scenario and applicable IO gate. Physical promotion additionally requires an
exact-image removable-media Framework run. Detailed H1–H14 acceptance, including
the integrated session and suspend-cycle thresholds, remains in the
[hardware track](../../roadmap/04-platform-hardware.md). Partial CPU boot,
framebuffer output, or one driver cannot satisfy completion.

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
