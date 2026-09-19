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
anchor CSlots are occupied resources, not free slots. The preservation registry
is bounded (4096 entries on large-descriptor images, 256 otherwise); exhaustion
refuses provisioning without skipping unowned bytes. Entries, including consumed
parents, remain owned for the root lifetime, so this is not unlimited allocator
metadata or adaptive memory support. Actual target image/CSlot fit still requires
its own admission and boot evidence.

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
  extent, outside its own page. Each access faults with its own task, access
  kind, and exact address, and the victim then faults on executing its own
  backed private memory.

These are QEMU envelopes on two architectures. They qualify no physical machine
and no larger target.

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

## Verification

- `just private_memory_check` exercises declared quotas and the published
  capacity envelope on both QEMU reference architectures.
- `just private_memory_isolation_check` exercises the fault and authority
  boundary between holders on both of them.
- `just private_memory_cycles_check` exercises repeated zeroing and reclamation
  across exit and fault.
- `just sel4_gate_control_check` mutation-checks the plane's marker contract.
- Contract changes additionally run `just contracts_check` and
  `just system_spec_check`.
