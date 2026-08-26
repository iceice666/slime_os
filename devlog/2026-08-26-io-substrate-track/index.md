# Planning IO0–IO4: one portable substrate, with Framework qualification left in H

| Field | Value |
|---|---|
| Date | 2026-08-26 |
| Kind | Decision |
| Status | Proposed |
| Scope | `roadmap/{README,01-foundations,02-core-runtime,03-ros2-compatibility,04-platform-hardware,05-foreign-workloads,06-authority-trust,07-architecture-portability,09-rpi5-ros2-demo,11-io-substrate}.md`, `devlog/README.md` |
| Roadmap | IO0, IO1, IO2, IO3, IO4, H2, H3, H4, H5, H6, H7, H8, H11, H12, H13, RP5, R0, R1, X1, X2, A3, A5, C7, C9.2, C9.4, P1, P4, P5.4.2, M5.7 |
| Gates | `just devlog_check` |
| Trigger | A handoff for userspace drivers specified queue epochs, leases, MMIO/DMA/IRQ authority, userspace virtio-blk, and later network/USB/audio/display consumers, while H2 and H6 already claimed overlapping Framework-scoped versions of the same mechanisms |
| Baseline | H2 owned both the portable driver authority ABI and Framework PCI binding, H6 owned both the portable network service and Framework USB Ethernet, and the next virtio-blk architecture step had no roadmap owner outside the deferred Framework track |

## Summary

The roadmap now has a separate architecture-neutral I/O track, IO0–IO4. It
owns bounded request/completion queues, request identities and driver epochs,
buffer-lease terminal states, explicit hardware-resource authority, userspace
virtio-blk and virtio-net reference drivers, `LinkDevice`, and exact-destination
network services. H remains the x86-64 Framework qualification track: H2 binds
IO1 resources to observed PCI/ACPI/APIC data, H4 proves AMD-IOMMU containment,
and later H slices implement and physically promote the named Framework
peripherals.

The split preserves the existing product boundary rather than introducing a new
one: seL4 supplies kernel mechanisms, `slime-root` constructs and reclaims only
explicit resources, supervised userspace drivers own device commands, and
ordinary clients receive typed semantic capabilities. This is documentation
only. No runtime behavior or status was claimed complete; the applicable gate is
`just devlog_check`.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `roadmap/11-io-substrate.md` | Added IO0 queue/epoch/lease rules, IO1 hardware-resource authority, IO2 userspace virtio-blk, IO3 userspace virtio-net/`LinkDevice`, and IO4 network/destination authority | Common mechanisms have one architecture-neutral owner and one independently observable proof sequence |
| `roadmap/04-platform-hardware.md` | Narrowed H2 to Framework PCI binding and H6 to Framework USB-Ethernet qualification; rewired H3/H4/H5/H7/H8/H11/H12/H13 to consume IO mechanisms while retaining device semantics and physical evidence | Framework identifiers keep their platform meaning without becoming a second portable ABI |
| `roadmap/README.md` | Added the IO track, dependency graph, release edges, and I/O invariants | The index names the owner read by every later track |
| C/R/RP/X/A consumers | Replaced H6 or implicit network ownership with IO4; made RP5 consume IO0/IO4; made X2/A3 consume IO buffer/DMA contracts while retaining H4 physical containment | Consumers depend on semantic services and portable mechanisms, not on an unrelated Framework release |
| `roadmap/07-architecture-portability.md` | Added the portable-I/O/platform-data boundary and a forward-ownership note after P5.4.2's preserved evidence | Completed P5 evidence remains historical truth while future generalization has a live owner |
| `roadmap/01-foundations.md` and `roadmap/02-core-runtime.md` | Added IO as the consumer of C7/C9/P1/P5 mechanisms | Completed foundations remain owners of their primitives; IO composes rather than redefines them |

## Decisions

- **Decision:** Create IO0–IO4 as a distinct cross-platform track rather than
  expanding H2.
- **Rationale:** H is explicitly one x86-64 Framework qualification track. Queue
  identity, epoch rejection, lease lifetime, hardware-resource capability
  classes, virtio reference drivers, `LinkDevice`, and exact-destination network
  services are needed by QEMU, Raspberry Pi 5, Framework, and future targets.
  Leaving them under H would either block portable work on Framework hardware or
  make PCI/APIC/AMD-IOMMU facts look universal.
- **Rejected alternative:** Rename H2 into the portable substrate and let H3–H14
  inherit it. That silently changes an existing Framework milestone's meaning
  and still leaves H6's network-service/platform-backend mixture unresolved.

- **Decision:** IO0/IO1 share mechanisms only; every device keeps a distinct
  typed protocol.
- **Rationale:** Fixed queue bounds, request/epoch identity, completion rules,
  Notification draining, leases, and resource accounting are reusable. NVMe
  operations, USB transfers, frames, TCP streams, PCM periods, surfaces, and GPU
  commands are not semantically interchangeable. The roadmap therefore forbids
  a universal `Device`, `IoOpcode`, or native `read`/`write`/`ioctl` surface.
- **Rejected alternative:** One generic opcode queue with device-specific
  variants. It moves semantic coupling into the common ABI and eventually into
  root validation, exactly where the userspace-service boundary says it cannot
  live.

- **Decision:** Separate `SharedBuffer` authority from DMA authority.
- **Rationale:** C7 proves CPU-visible sharing, identities, loans, quotas, and
  reclamation. A device mapping adds different authority and lifetime: access
  direction, IOVA/physical mapping, interrupt-driven completion, reset, and
  target containment. IO1 therefore grants DMA mappings only for live IO0
  leases; ordinary clients never receive physical addresses or IOVAs.
- **Rejected alternative:** Treat any mapped SharedBuffer as DMA-capable. That
  would let a service-facing capability silently acquire hardware reachability
  and make later AMD-IOMMU/SMMU enforcement an ABI change.

- **Decision:** Use userspace virtio-blk first and virtio-net second.
- **Rationale:** P5.4.2 and M5 already provide a deterministic root-owned
  read/write/flush oracle and capability-selected clients. IO2 can therefore
  prove migration, multi-request transport, DMA lifetime, reset, crash, and stale
  completion against observed behavior. IO3 then tests the same substrate under
  duplex TX/RX readiness and replenishment; passing both without substrate hacks
  is stronger evidence than designing directly for unimplemented NVMe or xHCI.
- **Rejected alternative:** Begin with NVMe, TCP, or xHCI. Each adds a new device
  state machine and, for physical hardware, containment/evidence dependencies,
  making substrate defects harder to distinguish from driver defects.

- **Decision:** Retain the root-served block path only as a temporary IO2 oracle,
  then remove its device-specific implementation after parity.
- **Rationale:** A clean cutover is the destination: `slime-root` constructs and
  reclaims resources, while the supervised component parses virtio descriptors
  and owns reset policy. Keeping both indefinitely would create two production
  storage architectures and two failure semantics.
- **Rejected alternative:** Permanently proxy every userspace driver operation
  through the existing root driver. That preserves device opcode knowledge in
  the root and does not prove the hardware-resource model IO1 exists to supply.

- **Decision:** Keep platform containment and physical promotion in the owning
  platform track.
- **Rationale:** IO1 can define a target-neutral DMA account/domain contract and
  an explicit trusted-DMA QEMU profile. Only H4 can prove AMD-IOMMU aliases,
  invalidation, bus-master ordering, and physical Framework fault containment;
  an Arm target needs a separately qualified SMMU or trusted-device decision.
- **Rejected alternative:** Let an IO QEMU pass imply physical DMA safety. The
  roadmap already prohibits QEMU evidence from completing a physical milestone.

## Open risks and follow-ups

- [ ] IO0–IO4 gates named in the roadmap do not exist yet. Each implementation
  slice must add the narrowest real gate, refusal arms, and a Change devlog before
  its status can move from Not started.
- [ ] The exact Zutai schema split for generic queue envelopes versus
  protocol-specific payloads is intentionally not frozen by this planning entry.
  IO0 must prove generated bindings can compose without duplicating field
  layouts or introducing a universal opcode.
- [ ] IO1's exact capability/object names and syscall operations remain open
  until the current generation resource model and seL4 device-untyped/IRQ paths
  are changed together with `docs/capability-matrix.md`, `docs/syscall-abi.md`,
  and their contract generators.
- [ ] The root-owned virtio-blk path has known missing fault arms recorded under
  P5.4.2. IO2 must distinguish inherited oracle coverage from newly observed
  cancellation, reset, interrupt, crash, and stale-epoch coverage.
- [ ] RP5 now depends on IO0 and IO4. This is a deliberate sequencing change and
  may move portable I/O work onto the near-term demo path; the roadmap still
  requires backlog-first selection and the narrowest slice that actually blocks
  the demo.
- [ ] H3 remains titled “under QEMU” although it now describes Framework xHCI
  device logic over the portable substrate. Its deterministic backend and exact
  emulated controller profile must be chosen when H3 opens; no current QEMU
  xHCI claim was made. **[INFERENCE]**

## Artifacts and provenance

- Focused report: none; the ownership split and milestone contracts are recorded
  directly in `roadmap/11-io-substrate.md` and the changed consumer tracks.
- Raw transcript: not retained. The source inventory was read-only and its
  conclusions are represented in the Changes and Decisions above.
- Serial/debugger/model output: none. This was a documentation-only architecture
  decision; no runtime or physical evidence applies.
- Related roadmap items: [IO0–IO4](../../roadmap/11-io-substrate.md),
  [H1–H14](../../roadmap/04-platform-hardware.md),
  [P5.4.2](../../roadmap/07-architecture-portability.md), and
  [RP5](../../roadmap/09-rpi5-ros2-demo.md).
- Existing evidence retained as the migration baseline:
  [`P5.4.2a device substrate`](../2026-08-08-p5-4-2a-device-substrate/),
  [`P5.4.2b virtio-blk`](../2026-08-08-p5-4-2b-virtio-blk/), and
  [`P5.4.2c storage plane`](../2026-08-08-p5-4-2c-storage-plane/).
