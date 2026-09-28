# BootState partition layouts: one product layout, one fixture layout

**Status:** Accepted
**Related work items:** `01a0e395-3d12-7a17-b6ed-9aac687f635f` (the defect
this records), `01a0e54d-8a04-767d-95c5-0ded2ddc9a2d` (deferred unification)

## Context

Two on-disk arrangements carry BootState today, and both sit inside a GPT
partition that `boot_contracts::gpt::validate_store_partition` admits by the
same type GUID (`SLIMEOSSTOREGPT!`):

- **Boot-store v1**, declared in
  [`contracts/bootstate/v1/schema.zt`](../../contracts/bootstate/v1/schema.zt):
  the two BootState slots at partition-relative LBA 0 and 1, the boot-store
  header and directory at byte 4096, detached releases from 8192, generations
  from 16384, and a fixed 32 MiB capacity. `slime-root/src/boot_selector.rs`
  reads and commits exactly this, and `sel4_boot_selection_check` builds its
  disk in this layout (`build-store-fixture.py --variant boot-selection`).
- **The userspace fixture layout**: an object-store partition
  ([`contracts/store/disk/v1`](../../contracts/store/disk/v1/schema.zt),
  superblock slots at LBA 0 and 1, records from LBA 2)
  with BootState placed at LBA 1024 and 1025 of its record area, next to a
  recovery index at 1026 and a transfer manifest at 1030.
  `components/services/sel4-generation-manager` and the rollback, recovery,
  and transfer probes under `components/testkit/` use it, and
  `build-store-fixture.py` builds it as immutable test input. The object store
  does not reserve those sectors; the fixture builder only asserts that its
  seeded records end before them.

The two cannot be reconciled by changing a constant. Both claim LBA 0 and 1,
for different records, so a partition is one or the other. The manager also
recognizes two constant identities rather than reading a directory of signed
generations, so pointing it at a boot store would not make it stage anything
the selector could boot.

## Decision

Boot-store v1 is the only product layout for BootState. It is the one a root
selects from, the one a future writable generation store must produce, and
the one any qualification of rollbackable production generations is judged
against.

The userspace layout is test scaffolding for the BootState transition model,
the management protocol, recovery reconstruction, and transfer. It is not a
product format and gains no contract: nothing may create it on a device
outside a QEMU fixture, and no product composition may depend on it. Today
only the `sel4-generation` plane and the rollback, recovery, and transfer
planes include a writer of it. Its
constants stay local to the fixture builder, the probes, and the manager
until the unification below replaces them.

The two stay distinct until the manager can stage a real signed generation
into a boot store. Unifying them is deferred to
`01a0e54d-8a04-767d-95c5-0ded2ddc9a2d`, whose exit condition is observed: one
plane stages through the manager and selects through the selector on the
same disk.

## Alternatives considered

- **Move the userspace slots to LBA 0 and 1 now.** It collides with the
  object-store superblocks those planes also read, and it would still leave a
  manager that writes identities no directory contains. The layouts would look
  shared without being qualified as one.
- **Unify now.** It requires the manager to read the boot-store directory,
  write generations and releases into it, and a plane that reboots one disk
  from the manager into the selector. That is milestone-sized work behind the
  production-generation boundary, not a backlog repair, so it is tracked
  rather than done here.
- **Declare the fixture layout as a second contract.** It would give test
  scaffolding the standing of a persisted format and invite a product
  consumer, the opposite of what the split protects.

## Consequences

- The userspace planes qualify the BootState state machine, the command
  protocol, and recovery and transfer logic, not a storage layout the
  selector boots from. Documentation must not describe them as qualifying
  disk-backed selection.
- A disk the manager stages on is not one the selector selects from, so
  userspace management and pre-admission selection remain two separate
  qualifications, as
  [generation management](../architecture/generation-management.md) states.
- Any new consumer of BootState outside a test fixture reads boot-store v1.

## Revisit when

- the manager, or any other component, must write a generation a root will
  select, which is the point where `01a0e54d-8a04-767d-95c5-0ded2ddc9a2d`
  stops being deferred;
- a writable object store needs sectors 1024 onward of a partition that also
  carries the fixture regions;
- the boot-store v1 layout itself changes version.
