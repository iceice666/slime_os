# Private component memory

## Boundary

Private memory is one task's growable working region. It is not a shared buffer,
not a transferable object, and not an arbitrary mapping API. The root tracks
pages; the component runtime tracks allocations.

Authoritative owners:

- mechanism and accounting: `slime-root/src/private_memory.rs`;
- reservation and mapping: `slime-root/src/child_vspace.rs` and
  `slime-root/src/object_allocator.rs`;
- target-qualified budgets: `contracts/private-memory-budget/v1/`;
- component allocation policy: `components/runtime/src/private_heap.rs`;
- component declarations: `contracts/component-spec/v1/` and
  `contracts/system-spec/v1/`.

## Shape and authorization

A task with a nonzero generation quota receives one fixed-base virtual window at
spawn. The base never moves because native ELF code holds real pointers. Growth
maps only at the tail and fails if it would pass the reservation, the holder's
quota, the image-wide total, or available backing.

Address space is reserved before backing. The allocator may use aligned 2 MiB
frames or 4 KiB frames, with leaf tables created only where base pages require
them. A failed growth unwinds the attempt's mappings without discarding existing
committed pages or backing extents that remain reusable.

Shared-buffer mappings are refused at every address in the reserved private-memory
window, including addresses not yet backed by private frames. Outside the
reservation, ordinary shared-buffer mapping, sealing, unmapping, release, and
quota reuse are unchanged.

Quota is deny-by-default and is a generation budget, not a capability. The region
has no object identity and cannot be transferred, loaned, sealed, shared,
file-backed, or made executable. The root exposes a grow operation only; it does
not implement `malloc`, `free`, relocation, live shrink, or multiple regions.

On AArch64 and RISC-V private mappings are user read/write and execute-never. The
x86-64 seL4 mapping API exposes no NX frame attribute, so execute prevention is
currently unenforced on that profile; `slime-root/src/vm_attributes.rs` owns this
explicit non-equivalence.

## Ordinary backing and alignment

`slime-root/src/object_allocator/global_backing.rs` retains alignment prefixes as
independent ordinary untyped children before advancing a parent's watermark.
A prefix is partitioned into legal aligned powers of two, with descriptor and
CSlot preflight before kernel effects. Each successful preservation step commits
its own ownership, so a later failed retype does not lose the preceding pieces.

Subsequent allocations consume the best-fitting preserved leaf, splitting it
when necessary. A leaf is consumed whole; its retained parent is not advertised
as free afterward. BootInfo parents also keep a tracked child alive, because
seL4 resets an untyped's allocation position when its last descendant disappears.
Device untypeds never enter this path. Small preserved leaves can serve small
kernel objects, not only page-sized memory.

Ordinary tails, unconsumed preserved leaves, reusable task/shared extents and
initially live backing are separate, disjoint accounting categories. Retained
anchor CSlots are occupied resources, not free slots. Retained-prefix records
live in resource-backed storage that grows on demand from the root metadata
window, so exhaustion is a refusal for want of admitted memory rather than of
a compile-time table. Entries, including consumed parents, remain owned for
the root lifetime. Actual target image/CSlot fit still requires its own
admission and boot evidence.

## Expandable root CSpace and metadata storage

`slime-root/src/root_cspace.rs` installs a four-bit root CNode around the
kernel's initial CNode before any other root thread starts. Branch zero keeps
the initial authority at its original addresses under a guard; every later
leaf is an unguarded ten-bit CNode installed at an expanded prefix, and a
capability address is the full path, not an index into one flat node. Every
root-sharing thread configures the installed tree's guard, so a second root
thread resolves the same addresses.

`slime-root/src/object_allocator/segmented.rs` holds slot occupancy words,
allocation and extent descriptors, retained-prefix records and the metadata
ownership ledger in page-backed storage whose indices are stable and whose
lookup is constant time. Pages come from the bootstrap allocator in
`object_allocator/infrastructure.rs`, which owns one adopted ordinary source,
sixty-four emergency slots, and every transaction retained after a failed
mapping, installation or delete. Growth is charged, reported as
`infrastructure_owned`, and reusable: released records return to the pool
rather than to the platform, and the pool never exceeds the high-water demand
that funded it.

Capacity is therefore not a constant. The qualification report names the
resource that refuses a plan — allocation descriptors, extent descriptors,
root CSlots, ordinary bytes, metadata records or ordinary layout — and root
CSpace is grown to a generation's plan before that plan is admitted.

## Qualified simultaneous capacity

The QEMU budget admits four 65536-page (256 MiB) holders, 262144 pages in total,
on `aarch64-sel4-qemu-virt` and `riscv64-sel4-qemu-virt`. A budget that declares
one page beyond either bound is refused at admission rather than at first
growth, so an image that cannot be honoured in full never boots.

Two compositions qualify that envelope, both owned by
`scripts/check/check-sel4-private-memory-plane.py`:

- `sel4-private-memory-1g` keeps four holders resident while one of them dies
  and is readmitted twenty times. The surviving three re-verify their whole
  patterns after every replacement, and each replacement observes a zeroed
  region, so retention and non-disclosure are separate observations.
- `sel4-private-memory-isolation` gives one holder the full 256 MiB and two
  peers a single page. Every task's window starts at the same virtual address,
  so each peer first proves its own page works, is refused a second, and then
  reads or writes 128 MiB into that same address range — inside the victim's
  extent, outside its own page. Before either peer faults, both lend and receive
  a sealed shared buffer over their declared transferable endpoint. Each received
  loan maps and verifies outside the private window, is refused at its backed
  and reserved addresses, and is explicitly unmapped and returned before its
  source buffer is released. Owned buffers separately exercise those destination
  refusals, unowned-handle rejection, and sealed-write rejection. Each peer's
  private page remains intact and its buffer/loan accounting returns to zero.
  Each foreign access faults with its own task, access kind, and exact address,
  and the victim then faults on executing its own backed private memory.

These are QEMU envelopes on two architectures. They qualify no physical machine
and no larger target.

The capacity raise retains private-memory-budget/v1 and lifecycle-policy/v1:
record layouts, field meanings and identity domains are unchanged; only the
admitted target-specific quota and restart-attempt bounds increase. Existing
smaller declarations remain valid. This is not forward acceptance by older
readers: a root with the previous bounds must refuse a newly enlarged declaration.
Generation/image identities bind the selected contracts and implementation, so
the qualification does not authorize replaying a new budget against an old root.

## Userspace allocation

Components built with the private-heap feature install the first-fit, address-
ordered, coalescing allocator in `components/runtime/src/private_heap.rs`.
Growth is batched in userspace while the ABI and accounting remain page-based.
Freeing returns spans to the component's free list; physical frames return to the
root only when the task dies.

Fallible collection growth surfaces quota exhaustion through Rust's `try_*`
allocation APIs. Infallible allocation terminates the component visibly rather
than silently truncating or hanging.

## Reclamation and reusable evidence method

Task exit and task fault converge on the same subtree reclamation mechanism.
Useful qualification must therefore test both terminal paths and must distinguish
four properties:

1. every re-served word is zero before reuse;
2. a full write/read sweep detects aliases, not merely one sampled word per page;
3. allocator watermarks and live-object counts do not drift across incarnations;
4. reclaimed backing retains its expected large/base-frame shape and becomes
   immediately reusable.

The retained `MEM-64M` campaign used twenty alternating clean-exit/fault
incarnations on both QEMU architectures. Those measured numbers are historical
run evidence, not universal constants. The method above is current because it
checks the reclamation invariants directly and is implemented by the private-
memory plane checker.

## Current capacity

The contract publishes 65,536 pages (256 MiB) per holder and 262,144 pages
(1 GiB) total only for `aarch64-sel4-qemu-virt` and `riscv64-sel4-qemu-virt`.
Targets without an explicit row retain the conservative 512-page per-holder and
2,048-page total bounds. A QEMU capacity result proves no physical board's
memory map.

Further capacity work is specified in
[`../plans/memory-capacity.md`](../plans/memory-capacity.md). It does not reopen
the private-memory mechanism.

## Adaptive policy admission boundary

`contracts/private-memory-budget/v2/` defines explicit entitlements and instance
membership, separate guaranteed pages, fixed or pool-relative request maxima,
and an operational/restart reserve of bytes, CSlots, descriptors, extents and
tables. A subtree root restricts explicitly listed members; it never implicitly
authorizes every descendant. System and generation declarations share these
types. Component `privatePageQuota` remains a fixed-v1 default; a system opting
into `privateMemoryPolicy` must explicitly clear every effective fixed quota.
Fixed and adaptive declarations/resource objects cannot coexist.

The v2 resource retains the `SLIMEPM` magic family and changes the version, so
old v1 readers refuse it rather than treating authority as absent. The current
root scans the whole resource family, rejects duplicates, validates v2 structure
and instance ownership, and binds each subject to a real task. Existing fixed v1
bytes, target bounds and full-affordability semantics remain unchanged.

The allocation-free model in `boot-contracts/src/private_memory_policy/ledger.rs`
charges guarantees once per entitlement, keeps incarnation tokens distinct from
entitlement identity, and serializes transactions in caller receipt order.
Committed bytes cannot be stolen. Failed cleanup retains quarantined charges;
successful revocation returns ownership, and a restarted instance cannot spend
the prior incarnation's token within that ledger. Callers must supply only ordinary
inventory, never device ranges; the pure partition check proves disjoint coverage,
not the provenance of its inputs. Boot exclusions appear only as absent ranges.
Exact placement plans check alignment and fragmentation separately from affordability.

The allocator supplying this model must certify simultaneous guarantee backing
and a conservative per-page envelope of table/metadata/slot costs, reserve real
resources before publication, and settle transactions only after kernel success.
Partial redemptions cannot draw another page's share. Successful rollback retains
only elastic-funded table resources; otherwise it remains pending or quarantined
until cleanup. Quarantined payload remains charged against authorization maxima.
Payload quotas do not count metadata or unused extent backing as mapped pages;
all such overhead remains charged to the resource pool, not hidden as free RAM.
The pure model does not prove kernel placement or discover RAM.

## Guarantee reservation and adaptive task windows

A guarantee is physical before it is promised. Admission runs before any task
exists: `slime-root/src/object_allocator/guarantee_vault.rs` takes concrete
aligned backing, extent records, allocation descriptors and CSlots for every
entitlement's guaranteed pages, all entitlements together or none, and removes
them from ordinary and elastic selection. Only the residual funds the ledger,
so the same bytes are never offered twice. A composition whose guarantees the
inventory cannot hold refuses admission and publishes nothing.

Guaranteed payload is mapped through a base-page lane, so a promise never
depends on an aligned 2 MiB placement surviving fragmentation; elastic payload
keeps the large-frame selection, and the fixed v1 path is unchanged. Planner,
acquisition and mapper consume one plan, and the placements the allocator
actually obtained are certified against their protected sources before any
retype. A guaranteed page is never served from the common pool: its funding,
including the descriptors and CSlots its frame and leaf table need, is lent from
its own entitlement and returned with it.

An adaptive subject's window is its declared address maximum rather than the
target's per-region capacity, so a policy may declare a window no power of two
fits; the base is still aligned independently and never moves. Backing arrives
only on demand, so construction reserves address space and nothing else.

Each incarnation is bound between construction and publication, on the boot path
and the dynamic spawn path alike: a staged task can neither be dispatched nor
grow before its binding succeeds, and a subject cannot hold two live
incarnations. Retirement returns capacity to the originating entitlement only
after the revoke succeeded; a failed revoke quarantines it instead, and a
restarted subject rebinds the same entitlement rather than a second one.

Device mappings meet the same window. The adaptive plane's IO holder binds a
real transport, proves an MMIO map and a device-queue map succeed outside its
reservation, and is then refused at its own backed page and at a reserved but
unbacked address inside the same window, on both reference architectures. The
positive controls use the same capabilities, rights and device epoch, so the
refusals are the destination's doing rather than absent authority.

What remains unproven is named rather than implied: the multi-inventory
qualification matrix is still separate work, and every result here is a QEMU
envelope on two architectures that qualifies no physical machine.

## Demand-backed acquisition

`slime-root/src/object_allocator/elastic.rs` and
`slime-root/src/private_memory/elastic.rs` implement the acquisition an elastic
holder's growth performs. An authorized maximum reserves nothing: a request
resolves into the mappings it will take, that shape is priced as a complete
resource tuple with checked arithmetic, the tuple is taken from the common
pool, and only then is a frame retyped or a page mapped.

Extents are sized to the request rather than to a quota. Large frames each take
their own aligned 2 MiB extent; base pages take an exact power-of-two
decomposition capped at 2 MiB; each new leaf table takes its own granule
extent. A growth's reserved bytes therefore equal its payload plus its tables,
which is what lets a fragmented machine serve page-granular growth from blocks
no aligned span would fit. Elastic arenas select backing best-fit, so a base
page cannot consume the aligned extent a large frame in the same transaction
was planned to occupy.

Acquisition precedes judgment because it is reversible. The ledger validates
the placements the allocator actually obtained — not a prediction — and a
policy refusal returns the whole acquisition before any mapping exists. A
failure after mapping unwinds the attempt, returns every unused extent,
descriptor and CSlot, and retains only leaf tables already bound to a span:
those name one fixed address, stay charged to their holder, and never enter a
kind-wide reusable pool. The ledger charges such a table's granule extent as a
table rather than as a separate extent record, so one record per retained table
stays allocated while the ledger counts none; the allocator's own record
capacity, not the ledger's, is what refuses a demand that cannot be stored.

A cleanup that does not complete is quarantined rather than lost: the
acquisition record stays in the allocator, the holder keeps ownership, its next
request is refused, and exactly one retry returns the resources. Guarantees are
reserved in the ledger at admission rather than pre-provisioned physically, so
an exhausted elastic pool refuses elastic transactions while a guaranteed
holder's first growth still finds its bytes.

The qualification roles run their holders in windows of the root's own address
space before any component is published. That exercises the same allocator,
ledger, retypes and kernel mappings a component's growth would take, and
deliberately not the spawn, admission or window-placement path, which the next
stage owns.

## Verification

- `just private_memory_check` exercises declared quotas and the published
  capacity envelope on both QEMU reference architectures.
- `just private_memory_isolation_check` exercises the fault and authority
  boundary between holders on both of them.
- `just private_memory_cycles_check` exercises repeated zeroing and reclamation
  across exit and fault.
- `just private_memory_stress_check` runs mixed-growth rollback and userspace
  heap pressure on both QEMU architectures while three 256 MiB peers retain
  their patterns. Its injected images reserve real allocator CSlot/descriptor
  entries near their limits, without claiming a kernel CNode full of installed
  capabilities. Interleaved, reclaimed 4 MiB guard extents leave measured gaps
  between reused 2 MiB data extents; retained backing remains explicitly owned.
- `just private_memory_cspace_check` proves capabilities created, invoked,
  copied, retyped, deleted and revoked beyond the initial CNode namespace on
  both QEMU architectures, from the root thread and from a second root thread,
  with the initial namespace deliberately retired first.
- `just private_memory_metadata_check` grows, reuses and reconciles metadata
  storage, injects a retained construction and a failed delete, and proves each
  retry releases exactly once while nothing quarantined is reassigned.
- `just private_memory_bootstrap_check` measures the bootstrap reserve and
  exhausts RAM, root CSlots and the metadata window independently, each
  refusing before any task is published.
- `just private_memory_phase2_regression_check` aggregates the fixed-capacity
  surface those three must not regress.
- `just private_memory_elastic_check` proves an idle authorized maximum takes
  nothing from the pool, a peer consumes the spare capacity, a guaranteed
  holder is still served after the elastic pool is exhausted, and the refusal
  damages neither holder's pages.
- `just private_memory_fragmentation_check` prices one-page, mixed and bulk
  growth separately, serves page-granular growth from a machine with no aligned
  2 MiB block left, refuses the span request by the resource that ran out, and
  takes a returned holder's extents for the next request.
- `just private_memory_rollback_check` injects failure at extent acquisition,
  descriptor provisioning, retype, table mapping, frame mapping and a
  near-limit descriptor window, then proves a retry charges once and a failed
  cleanup is owned, refusing and retryable exactly once.
- `just private_memory_conservation_check` returns one holder's capacity to
  another with zeroed pages and an unchanged peer pattern, keeps ownership
  through a failed revoke, and reconciles unallocated bytes to their
  pre-workload baseline.
- `just private_memory_phase3_check` runs all four over the phase-2 surface, so
  one execution binds them to the same code closure and images.
- `just private_memory_adaptive_check` boots the adaptive composition twice on
  both QEMU architectures: guarantees are reserved before any task, every
  declared instance reports the entitlement actually installed on its record,
  holders are spawned dynamically, a fixed request schedule produces the same
  grants and refusals on both boots, a faulted subject's replacement rebinds one
  entitlement rather than two, an IO holder's device and queue mappings are
  refused inside its window and admitted outside it, and a separate
  over-guaranteed composition refuses admission before publication.
- `just private_memory_adaptive_lifecycle_check` boots the same composition
  from an image whose root carries two compiled-in failures. A construction
  that fails after its incarnation is bound holds the entitlement until the
  unwind's revoke succeeds, and the retried spawn binds the next incarnation
  rather than a second live one. The first adaptive holder to die is then
  quarantined, refunds nothing, and is returned by exactly one retry, while its
  peers finish their own schedule.
- `just private_memory_phase4_check` runs both over the whole phase-3 surface,
  because adaptive binding changes construction, growth and reclamation for
  fixed holders too.
- `just sel4_gate_control_check` mutation-checks the plane's marker contract.
- Contract changes additionally run `just contracts_check` and
  `just system_spec_check`.
