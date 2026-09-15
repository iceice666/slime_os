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

The contract publishes 16,384 pages (64 MiB) per holder and 32,768 pages total
only for `aarch64-sel4-qemu-virt` and `riscv64-sel4-qemu-virt`. Targets without an
explicit row retain the conservative 512-page per-holder and 2,048-page total
bounds. A QEMU capacity result proves no physical board's memory map.

Further capacity work is specified in
[`../plans/memory-capacity.md`](../plans/memory-capacity.md). It does not reopen
the private-memory mechanism.

## Verification

- `just private_memory_check` exercises declared quotas and isolation on both
  QEMU reference architectures.
- `just private_memory_cycles_check` exercises repeated zeroing and reclamation
  across exit and fault.
- `just sel4_gate_control_check` mutation-checks the plane's marker contract.
- Contract changes additionally run `just contracts_check` and
  `just system_spec_check`.
