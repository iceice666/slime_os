# C7.2 — Shared-buffer authority and factory allocation

| Field | Value |
|---|---|
| Date | 2026-07-24 |
| Status | Verified |
| Scope | Capability object surface, kernel shared-buffer table, physical contiguous allocator, syscall ABI, host builder/checkers |
| Trigger | Roadmap C7 decomposition; C7.2 lands the shared-buffer factory the later C7 slices consume |
| Baseline | C7.1 generation format v3 + `u64` rights; `SharedBuffer(SharedRegion)` object defined but never instantiated (no userspace creation path); DMA owned a private contiguous frame allocator |

## Summary

C7.2 turns the dormant `SharedBuffer` object into a real, factory-authorized
resource. A distinct `SharedBufferFactory` kernel object gates the new
`SYS_SHARED_BUFFER_CREATE`/`SYS_SHARED_BUFFER_RELEASE` syscalls behind
`RIGHT_BUFFER_CREATE` (bit 24). Buffers carry a kernel-assigned, unforgeable
monotonic identity and only the narrow-only buffer-operation rights
(`RIGHT_BUFFER_WRITE`/`RIGHT_BUFFER_MAP`) plus `RIGHT_TRANSFER`; holding the
factory never widens into write or map authority. Allocation is bounded by three
fixed global ceilings — `MAX_SHARED_BUFFERS` (32 objects), `MAX_TOTAL_PAGES`
(256 pages / 1 MiB), `MAX_BUFFER_PAGES` (64 pages) — all checked before any frame
is pulled, so a rejected request disturbs neither physical memory nor an existing
holder, and each failure mode returns a structured `SharedBufferError`. DMA
authority and shared-sample authority remain distinct capability kinds even
though both now draw from one shared contiguous frame allocator hoisted into
`pmm`. Status: verified under `just shared_buffer_factory_check` and the full
gate stack.

## Observable symptom

Not a regression; planned foundation work. Exit condition from the roadmap
(C7.2): a factory-authorized holder creates and releases a kernel-identified
shared buffer within fixed global bounds; an unauthorized component is denied,
exhaustion is structured and isolated, and no derivation or transfer widens
authority.

- Command: `just shared_buffer_factory_check`
- Expected: 8 QEMU cases pass; unauthorized/oversized/exhausted paths fail closed
- Observed: all pass (see Verification)

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `SharedBuffer`/`SharedRegion` existed with a `RIGHT_BUFFER_WRITE`/`RIGHT_BUFFER_MAP` surface but no creation path; C7.1 left bits 24-63 free | Add `SharedBufferFactory` + `RIGHT_BUFFER_CREATE` (bit 24) and two syscalls, mirroring the M6.1 `EndpointFactory`/`SYS_ENDPOINT_CREATE` pattern |
| 2 | DMA owned a private `alloc_contiguous`/`free_contiguous`; the spec says shared-sample and DMA "reuse memory-accounting machinery" but stay distinct authority | Hoist one contiguous allocator into `pmm`; both `DmaTable` and `SharedBufferTable` call it while remaining separate object kinds |
| 3 | First QEMU run: `byte_exhaustion` test used `MAX_TOTAL_PAGES/2` = 128 pages > `MAX_BUFFER_PAGES` = 64 | Test bug — fill the byte budget with several sub-cap buffers instead of one oversized one |
| 4 | Second/third runs: filling with 64- then 16-page buffers hit `OutOfFrames` | Root cause below: the old contiguity check only recognized *ascending* pop order, but the PMM free-list stack hands out a block in *descending* address order, so multi-page runs were almost never detected |
| 5 | After rewriting the allocator to be pop-order-independent, all 8 cases and the DMA-exercising `storage_read_check` pass | Latent DMA fragility (shared by virtio queue allocation) is fixed as a side effect |

## Root cause

Two distinct violated constraints:

1. **Capacity/authority gap (the milestone):** the kernel had no factory object
   or right to mint a `SharedBuffer`, so the object was unreachable from
   userspace. C7.2 adds the object, right, table, and syscalls.
2. **Latent allocator defect (surfaced by testing):** the original
   `alloc_contiguous` (inherited from `dma.rs`) verified contiguity by requiring
   each successive `alloc()` to return `base + i*PAGE_SIZE` — i.e. strictly
   *ascending* addresses. The PMM free list is a LIFO stack seeded by ascending
   `push`, so within one free block frames pop in *descending* order. The check
   therefore rejected genuinely contiguous runs and only ever succeeded by luck
   of fragmentation, making any request >1 page unreliable. Rewritten to collect
   `pages` frames and accept them iff their address span is exactly
   `(pages-1)*PAGE_SIZE` regardless of pop order, with a bounded set-aside stash
   to shift the scan window across retries and unconditional return of every
   set-aside/partial frame on all exit paths.

## Changes

| Area | Change | Restored/established invariant |
|---|---|---|
| `kernel/src/capability/mod.rs` | `RIGHT_BUFFER_CREATE` (bit 24); `SharedBufferFactory` object + `valid_rights`/`RIGHT_ALL` wiring; `SharedRegion` gains kernel-assigned `id` and `ptr_eq` | Object-specific creation right, distinct factory object, unforgeable identity |
| `kernel/src/memory/shared_buffer.rs` (new) | `SharedBufferTable` with fixed byte/object/per-buffer ceilings; `create`/`release` returning structured `SharedBufferError`; global `SHARED_BUFFER_TABLE` | Bounded, kernel-created, isolated allocation; charges checked before frames pulled |
| `kernel/src/memory/pmm.rs` | Hoisted + rewrote a pop-order-independent `alloc_contiguous`/`free_contiguous` (`CONTIG_MAX_FRAMES`) | One shared contiguous allocator; multi-page runs reliably found; no frame leak |
| `kernel/src/drivers/dma.rs` | Delegates to `pmm::alloc_contiguous`/`free_contiguous`; drops its private copies | Single source of contiguous allocation; DMA path unchanged in behavior |
| `kernel/src/syscall/mod.rs` | `SYS_SHARED_BUFFER_CREATE` (21) / `SYS_SHARED_BUFFER_RELEASE` (22) with `RIGHT_BUFFER_CREATE` gate, release-on-insert-failure, holder-cap invalidation | Capability-gated mint/reclaim; no leak on table-full; releasing holder loses the cap |
| `scripts/build-generation.py` | `"bufferCreate": 1 << 24` in the rights map | Manifest can grant the factory right 1:1 to the bit name |
| `scripts/check-no-storage-authority.py` | Allowlist adds `SharedBufferFactory`, `RIGHT_BUFFER_CREATE`, two syscalls; fixed pre-existing missing `SYS_WAIT` and stale kernel subpaths (`runtime/`, `drivers/`, `storage/`) | Framework safety allowlist matches the real surface again |
| `Justfile` | `shared_buffer_factory_check` target | Independently reviewable C7.2 gate |
| `docs/capability-matrix.md` | `BUFFER_CREATE` row, bounds rows, buffer-right creation-authority update, horizon entry resolved | Matrix tracks the object/rights/bounds surface in the same change |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Unauthorized allocation / rights widening | `just shared_buffer_factory_check` | `factory_rights_are_object_specific` / `buffer_rights_narrow_only` fail |
| Bound bypass or cross-holder disturbance | `just shared_buffer_factory_check` | `object_exhaustion`/`byte_exhaustion_is_structured_and_isolated` fail |
| DMA allocator regression from the hoist | `just storage_read_check`, `just test` | virtio-blk queue alloc fails; storage-probe cannot verify sector 0 |
| Capability/rights/syscall surface drift | `just framework_safety_check` | allowlist mismatch |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just shared_buffer_factory_check` | pass (8/8 QEMU cases) | Direct |
| `just storage_read_check` | pass (DMA path healthy; sector 0 verified; slice healthy) | Direct |
| `just test` (QEMU) | pass (spawn_authority, storage_capability, all suites) | Direct |
| `just contracts_check` | pass (incl. 12 boot-contracts lib tests) | Direct |
| `just generation_check` | pass (byte-identical two builds) | Direct |
| `just fmt_check` / `just lint` | pass (`-D warnings`) | Direct |
| `just framework_safety_check` | pass | Direct |
| Reviewer verdict | `correct`, confidence 0.83, no findings | Direct (`history://EndlessWeasel`) |

## Decisions

- Decision: Enforce two independent ceilings (object count and total pages) plus
  a per-buffer page cap, all checked before pulling frames.
- Rationale: The roadmap requires that a holder cannot exhaust bytes or objects
  independently and that a rejected request disturbs no existing holder; ordering
  the checks before allocation makes rejection side-effect-free.
- Rejected alternative: Charge pages first then roll back on object-table-full —
  rejected because it briefly perturbs the global page total another holder could
  observe.
- Decision: Hoist and fix one contiguous allocator in `pmm` rather than keep a
  DMA-private copy.
- Rationale: C7.2 needs contiguous shared buffers; duplicating the (defective)
  scanner would double the bug surface. One allocator fixes the DMA path too.
- Decision: The creator receives `RIGHT_BUFFER_MAP` always and `RIGHT_BUFFER_WRITE`
  only when it requested a writable buffer.
- Rationale: Matches `SharedRegion.writable`; write authority should not exceed
  the requested mutability. Both remain narrowable on transfer and are gated by
  their own syscalls in C7.4.

## Open risks and follow-ups

- [ ] `RIGHT_BUFFER_WRITE`/`RIGHT_BUFFER_MAP` remain ungated until C7.4 lands the
  map/seal operations; the buffer is allocatable and transferable but not yet
  mappable. Tracked in `roadmap/02-core-runtime.md` (C7.4).
- [ ] C7.3 adds generation-declared per-holder quotas and supervision-subtree
  accounting on top of these global ceilings, and reclamation on peer
  death/restart/revocation.
- [ ] `SharedBufferFactory` is not yet wired into a generation manifest fixture;
  the gate exercises the mechanism directly. Manifest wiring lands with the C7.7
  two-component integration.

## Artifacts and provenance

- Related roadmap item: `roadmap/02-core-runtime.md` (C7.2)
- Reviewer verdict: `history://EndlessWeasel` (overall_correctness: correct, no findings)
- Kernel gate: `kernel/tests/shared_buffer_authority.rs`; `just shared_buffer_factory_check`
- Capability surface: `docs/capability-matrix.md`
