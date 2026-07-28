# C10 — Bounded private component memory

| Field | Value |
|---|---|
| Date | 2026-07-28 |
| Kind | Decision |
| Status | Proposed |
| Scope | `roadmap/02-core-runtime.md`, `roadmap/00-backlog.md`, roadmap index invariants and track map; planned `contracts/` budget resource; `kernel/src/task`, `kernel/src/memory`, `components/runtime` |
| Roadmap | C10, C10.1, C10.2, C10.3, C10.4, B9, C7 |
| Gates | none |
| Trigger | Question of whether `SLIMECMP` manages stack and heap, and whether the WebAssembly linear-memory model could supply the missing half |
| Baseline | A component's working memory is fixed at build time: stack from the `SLIMECMP` header, `.data`/`.bss` from the linked image, no allocator and no page-yielding syscall |

## Summary

Native components cannot allocate. Stack size comes from the `SLIMECMP` header
and static data from the linked image; `components/runtime` installs no
`GlobalAlloc` and no syscall yields a page, so `Vec`, `Box`, and `String` are
unavailable and every buffer must be sized for its worst case in every
generation that carries the component. C10 adds one task-private, fixed-base,
generation-bounded region that grows on demand, with allocation policy entirely
in `slime-rt`. The split is WebAssembly's — a runtime that grows bounded
zero-filled pages under a host-enforced ceiling, a language runtime that
allocates inside them — with one forced divergence: WebAssembly addresses memory
by offset so a runtime may relocate the base on growth, while `SLIMECMP` code
holds real machine pointers, so the base is pinned and the reservation fixed.
Investigating the reclamation path uncovered backlog **B9**: `task::terminate`
never removes the terminated task, so `AddressSpace::drop` never runs and every
spawn permanently leaks its image and stack frames. B9 gates C10.1, which
extends that same teardown path. Status is Proposed: nothing is implemented.

## Changes

| Area | Change | Established boundary |
|---|---|---|
| C10 | New core-runtime milestone, decomposed C10.1–C10.4 | Working memory is a budgeted mechanism, distinct from the sample plane |
| C10.1 | Fixed-base reserved window, one growth syscall, all-or-nothing growth, W^X pages, termination reclamation | The kernel tracks a page count, never an allocation |
| C10.2 | Versioned Zutai private-memory budget resource, eagerly validated, aggregate-bounded | The declared quota is the live ceiling; an undeclared component allocates nothing |
| C10.3 | `GlobalAlloc` in `components/runtime`, batched growth, startup quota probe | Allocation policy is userspace; the syscall ABI stays in target pages |
| C10.4 | Convert one worst-case static buffer, measure spawn/exit frame drift, prove capability invisibility | Private memory is unreachable from every transfer path |
| B9 | New open backlog defect: terminated tasks are never reaped | Reclamation must be correct before a mechanism depends on it |
| Index | Status row, memory lane, `C7 --> C10` edge, invariant 14 | C10 consumes C7's quota pattern only, not C8 or C9 |

## Decisions

- Decision: authorize growth with a generation-declared page quota, not a capability.
- Rationale: the region is not nameable, transferable, loanable, sealable, or shareable, so no object exists for a capability to designate; the real question is how many pages a component may hold, which is a budget. This mirrors the stack, which is generation-sized and needs no capability, and leaves `docs/capability-matrix.md` unchanged because C10 adds no kernel object and no right.
- Rejected alternative: mint a `PrivateMemory` capability. It would force an answer to "can it be transferred?", and once the answer is no the capability is an empty shell that later invites someone to add transfer.

- Decision: pin the base address and reserve the window at spawn; a growth that would pass the reservation fails rather than relocating.
- Rationale: `SLIMECMP` images link at a fixed VA and hold real machine pointers, so relocating the base would invalidate every live pointer. Wasmtime may move a linear-memory base precisely because WebAssembly code addresses by offset; that freedom does not transfer to native code.
- Rejected alternative: a growable region that may relocate, as Wasmtime's dynamic memories do.

- Decision: keep private memory entirely separate from the C7 shared-buffer plane.
- Rationale: shared buffers exist to move samples *between* components — every region is a nameable, transferable, loanable kernel object drawn from the contiguous allocator under a 256-page kernel-wide ceiling. Working memory is private, never transferred, and needs no physical contiguity. Merging them would attach transfer and loan semantics to a heap and force fragmentation-prone contiguous runs on every allocation.
- Rejected alternative: back the heap with shared buffers. Workable as a prototype, wrong as a contract.

- Decision: add a new `private-memory-budget` resource rather than widening `shared-buffer-budget/v1`.
- Rationale: the two mechanisms have different holder sets and different semantics, and widening the existing entry would change its wire width and re-open the green C7.3 gate. The new contract reuses that resource's holder-identity, sorting, and aggregate-bound rules without disturbing it.
- Rejected alternative: a single combined component-resource budget. That integration belongs with C9's resource accounts, whose contract is not yet fixed.

- Decision: expose growth only — no `free`, no shrink, no arbitrary `mmap`, no second region.
- Rationale: `free` is a free-list operation in userspace; pages return to the kernel when the task dies. A shrink syscall can be added later without changing growth semantics.

- Decision: keep the syscall ABI in 4 KiB target pages and put batching in `slime-rt`.
- Rationale: WebAssembly's 64 KiB page is a good *allocator* batch size and a poor kernel ABI constant; keeping the batch in userspace means a later page profile changes no contract.

- Decision: file the terminated-task leak as B9 and gate C10.1 on it rather than folding the repair into C10.
- Rationale: it is a pre-existing defect on an already-claimed exit condition, which the backlog rules place ahead of new track work. It is separately observable and separately verifiable, and it also repairs the image and stack leak, which is not C10's to claim.

## Open risks and follow-ups

- [ ] B9's per-cycle frame cost is **[INFERENCE]** from source reading; no boot has measured the drift. C10.4's spawn/exit measurement is where it gets quantified.
- [ ] User-half page tables leak independently of the leaf frames. B9's fix must cover both, or C10's reserved window adds page-table frames to the same leak.
- [ ] The private-memory base and reservation size are x86-64 profile constants. They must move behind P1's `arch/` boundary with `ENTRY_VA` and `USER_STACK_TOP`, not become architecture-neutral contract.
- [ ] Shared-buffer mapping bases are hardcoded per component with no central registry, so a future component could collide with the reserved window. A shared userspace address-layout module in `slime-rt` would prevent a hard-to-diagnose class of bug.
- [ ] Kernel-wide and per-holder ceilings are not yet fixed. They must be chosen against the 256 MiB QEMU profile and the 24 MiB kernel heap, and be defensible on a smaller target.
- [ ] `just private_memory_check` does not exist yet; C10.1 introduces it with the mechanism.

## Artifacts and provenance

- Focused report: none; the design is recorded in `roadmap/02-core-runtime.md` under C10.
- Raw transcript: none retained.
- Serial/debugger/model output: none. No code was written or run for this entry.
- Related roadmap item: [C10](../../roadmap/02-core-runtime.md), [B9](../../roadmap/00-backlog.md), [C7](../../roadmap/02-core-runtime.md)
- External references consulted for the linear-memory split: the WebAssembly core specification's `memory.grow` semantics, Wasmtime's `Memory`/`ResourceLimiter` API documentation, and WAMR's `wasm_export.h` instantiation and module-heap APIs.
