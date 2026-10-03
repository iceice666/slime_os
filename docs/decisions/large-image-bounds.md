# Large component image bounds within existing wire revisions

Status: accepted

Work item: `01a0fa78-cfc0-7dc0-976e-3ff092b908a4`.

## Context

Statically linked applications exceed the former component and generation size
bounds even without large embedded maps. Increasing only the root's per-image
constant would multiply descriptor provisioning by the maximum task population,
while leaving independent builder and decoder refusals unchanged.

## Decision

Raise the bounds in [component v2](../../contracts/component/v2/schema.zt) and
[generation v5](../../contracts/generation/v5/schema.zt) in place. Component ELF
bytes and generation object payloads each have a 128 MiB ceiling; an embedded
generation has a 256 MiB ceiling. The component wrapper counts toward the object
payload bound, so these are independent constraints, not a promise that an ELF
at the exact byte ceiling can also fit its wrapper into one object.

Only the exact `aarch64-sel4-qemu-virt` profile admits an image footprint of
32,768 base pages, including the main thread's IPC and transfer-window pages.
Its aggregate declared image ceiling is 65,536 pages. Other profiles retain
512 pages per image. Additional threads' runtime pages are accounted separately.
Root descriptor provisioning covers the aggregate image demand plus per-task
runtime objects and translation tables, not 48 copies of the per-image maximum.

The root reads authenticated generation payloads directly with an unaligned ELF
reader. This removes the whole-ELF staging buffer; mapping still copies each
segment through a root-owned scratch page into child-owned frames.

## Alternatives and trade-offs

A new generation v6 would provide a version distinction but no changed layout,
field interpretation, identity algorithm, or authority semantics. Keeping v5
avoids a layout migration solely for an admission-bound increase. Older readers
still refuse images beyond their compiled bounds; the version alone does not
promise that every implementation can load every well-formed image.

Keeping all targets on the larger envelope would spend physical-board CSlots
and RAM without qualification. Target-specific limits preserve their conservative
admission behavior. Shared or demand-mapped executable pages could reduce copies
but require different lifetime and authority rules and are not part of this work.

## Consequences and revisit conditions

Builders, generated bindings, decoders, resource quotas, and the root loader
must agree on the bounds. Whole-graph aggregate admission happens before any
instance is constructed. The existing reclamation plane owns large-image
qualification, including exact-limit refusals, repeated teardown, and a faulting
incarnation; this decision is not itself evidence that those checks passed.

The boot selector retains its separate 4 MiB boot-store loading limit. An embedded
generation above it must not be presented as a boot-store artifact. Large-image
qualification uses QEMU's default 2 GiB inventory and establishes neither the
1 GiB inventory case nor physical-machine support, Nav2 compatibility, or a
language runtime/JIT implementation.

Revisit the wire revision when field interpretation or identity changes, the
target envelope when a new target is actually qualified, and the loading model
when sharing or demand paging becomes an explicit requirement.

Implementation owners: `slime-root/src/{child_vspace,object_allocator,generation}.rs`,
`slime-root/src/graph_runtime/`, `scripts/build/build-generation.py`, and
`scripts/check/check-generation.py`.
