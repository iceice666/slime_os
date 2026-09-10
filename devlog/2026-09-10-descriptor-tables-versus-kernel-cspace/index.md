# Descriptor tables sized against the linked kernel's root CSpace

| Field | Value |
|---|---|
| Date | 2026-09-10 |
| Kind | Defect |
| Status | Fixed |
| Scope | `slime-root/src/object_allocator.rs`'s descriptor-table sizing and its documentation, the asserted host-test count in `just/quality.just`, and the 52 regenerated system-image closures |
| Work items | none |
| Gates | `just test_sel4_root`, `just private_memory_check`, `just sel4_root_boot_check`, `just sel4_reclamation_check`, `just contracts_check`, `just system_image_builder_check` |
| Trigger | PR #25 review against `f26d79d6` reported one P2 and one P1 finding |
| Baseline | `52570122`'s and `f26d79d6`'s fixes; this round covers the two findings that commit did not close |

## Summary

Two findings against `f26d79d6`. The P2 is a real boot-capacity defect: the
enlarged descriptor tables were selected by a `#[cfg]` allowlist naming the
Milk-V Duo and one injection image, so both AArch64 physical boards —
`bcm2712-rpi5` and `ns02201-h1v1`, which keep seL4's 12-bit root CNode default
— compiled the ~4 MiB `AllocationRecord` array into `.bss`. In this root
`.bss` is capacity: the seL4 loader creates one root CSlot per page of the
root image before the root runs, so that array alone spends ~1026 of those
boards' 4096 total slots before `admit_total_slots` evaluates the product
graph. The P1 is documentation: the comment above the constant recorded a
reviewer finding, a named test's shortfall, and a future milestone's remedy,
all three of which the repository guide excludes from implementation comments.
Both are fixed. The table size is now derived from the kernel configuration
the root actually links against, which no platform allowlist can drift from,
and the comments state only invariants.

## Observable symptom

- Command: PR #25 review of `f26d79d6`; locally, compiling `slime-root` against each installed prefix.
- Expected: an image's allocator tables are affordable in the CSpace of the kernel that image boots on; a constant's comment states what the constant guarantees.
- Observed: `MAX_TASK_ALLOCATIONS` selected 262,657 records for every profile except `slime_cv1800b_duo` and `slime_private_small_tables`, so `aarch64-rpi5` and `aarch64-sel4-nt98690-h1v1` — neither excluded, neither re-pinned to a 19-bit CNode — carried tables their kernels cannot pay for; the comment above it named `four_holders_include_static_descriptors_at_default_pool_boundary`, the review that found it, and the ceiling-raising milestone that would fix it.
- Exit/fault/serial evidence: measured per prefix below; the boards were not booted, so the admission failure is derived from the pinned kernel configs and the measured `.bss`, not observed on hardware.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `sel4/config/qemu-arm-virt.cmake` and `qemu-riscv-virt.cmake` are the only two configs this PR set `KernelRootCNodeSizeBits 19` in; `bcm2712-rpi5.cmake`, `ns02201-h1v1.cmake`, and `cv1800b-duo.cmake` set it nowhere, and seL4's `config.cmake` defaults it to 12 | The exclusion list and the set of narrow-CSpace platforms disagreed by exactly the two AArch64 boards |
| 2 | The installed prefixes confirm it directly: `build/sel4-prefix` and `build/sel4-riscv64-prefix` report `ROOT_CNODE_SIZE_BITS` 19, `build/sel4-rpi5-prefix` and `build/sel4-cv1800b-duo-prefix` report 12 | The kernel configuration is an authoritative, already-installed source for the width; the `#[cfg]` allowlist was a hand-maintained restatement of it |
| 3 | `size_of::<[AllocationRecord; 262_657]>()` is 4,202,512 bytes — 1027 pages, so ~1026 root CSlots at one slot per image page — against 4096 total slots on a 12-bit CNode | Not a headroom question: the tables alone claim a quarter of those boards' entire CSpace before any product object is counted |
| 4 | `slime_cv1800b_duo` is set by `build.rs` from `SLIME_TARGET_PROFILE`, while `slime_private_small_tables` is set by `build-sel4.py` for one injection role; neither is derived from the kernel | Any future board, or any re-pin of an existing one, reintroduces the same divergence silently |
| 5 | `sel4::sel4_cfg_usize!(ROOT_CNODE_SIZE_BITS)` reads `$SEL4_PREFIX/libsel4/include/kernel/gen_config.json` at compile time, and `slime-root` already depends on `sel4` and uses `sel4_cfg` elsewhere | The correct selector was available without a new build-script variable, a new pin, or a new checker |

## Root cause

The descriptor pool was sized by naming the platforms believed to be narrow
rather than by measuring the CSpace the image is compiled against. That
inverts the dependency: the kernel configuration decides how many root CSlots
exist, so a constant enumerating platforms is a copy of that fact which
nothing keeps current. The two AArch64 boards were absent from the copy, and
because no gate compiles the root against their prefixes and asserts anything
about image size, nothing reported the divergence.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Table sizing | `KERNEL_ROOT_CNODE_SLOTS` reads `ROOT_CNODE_SIZE_BITS` from the linked kernel's configuration; `LARGE_DESCRIPTOR_TABLES` requires that width to reach `MAX_ROOT_CSLOTS`, and `MAX_TASK_ALLOCATIONS`/`MAX_TASK_EXTENTS` select from it | An image's allocator tables are affordable in the CSpace of the kernel it boots on, for every platform, including ones not yet added |
| Platform allowlist | The `slime_cv1800b_duo` arm is gone from both constants; the Duo now selects the small envelope because its kernel is 12-bit, which is the actual reason | A board's table size follows its kernel, not its name |
| Documentation | The comments above `MAX_ROOT_CSLOTS`, `MAX_TASK_ALLOCATIONS`, and `MAX_TASK_EXTENTS` state the bound, why `.bss` is capacity in this root, and that `TaskBackingCapacity::fits` — not the bound alone — decides admissibility | Implementation comments carry invariants, not review history, named test shortfalls, or milestone plans |
| Regression | `allocator_state_fits_the_linked_kernels_root_cspace` measures `size_of::<ObjectAllocator>()` in image pages against a quarter of the linked kernel's CSlots, and asserts both table constants track `LARGE_DESCRIPTOR_TABLES` | The sizing rule is checked against the kernel, so a future hand-set flag cannot reintroduce the divergence |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A platform is added, or an existing one re-pinned, whose kernel cannot afford the tables its image compiles | `just test_sel4_root` compiled against that platform's prefix | `allocator_state_fits_the_linked_kernels_root_cspace` reports the slots spent against the slots available |
| Table size is made to follow a hand-set flag again instead of the kernel width | `just test_sel4_root` | the `LARGE_DESCRIPTOR_TABLES` equality assertions fail |
| The production QEMU envelope shrinks by accident | `just private_memory_check`, `just test_sel4_root` | capacity traces and the four-holder boundary tests move |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Compile `slime-root` against `build/sel4-prefix` (19-bit) | `cnode_slots=524288 large=true allocs=262657 extents=1076` — production envelope unchanged | Direct |
| Compile `slime-root` against `build/sel4-rpi5-prefix` (12-bit) | `cnode_slots=4096 large=false allocs=4096 extents=144` — the board now selects the small envelope | Direct |
| Mutation: `LARGE_DESCRIPTOR_TABLES` reverted to the pre-fix flag-only form, compiled against the rpi5 prefix | `allocator_state_fits_the_linked_kernels_root_cspace` fails (SIGABRT on the assertion); restored source passes | Direct |
| `just test_sel4_root` | 229/229 passed across 19 modules | Direct |
| `just private_memory_check` | AArch64: 23 markers across 7 causal chains and 3 image cases. RV64: 23 markers across 7 causal chains and 1 image case | Direct |
| `just sel4_root_boot_check` | ordered generation, timer, task, IPC, fault, and ready markers observed | Direct |
| `just sel4_reclamation_check` | segmented task extents and root CSlots reclaimed and reused | Direct |
| `just contracts_check` | passed, exit 0 | Direct |
| `just system_image_builder_check` | 52 closures current, resolving with distinct identities; 1 closure built twice byte-identically | Direct |
| `just lint_sel4_root`, `cargo fmt -p slime-root --check` | passed | Direct |
| Physical boot of `aarch64-rpi5` or `aarch64-sel4-nt98690-h1v1` on the fixed image | not run — no board boot was performed for this change | none |

## Decisions

- Decision: derive the table size from `sel4_cfg_usize!(ROOT_CNODE_SIZE_BITS)` rather than from a corrected `#[cfg]` allowlist.
- Rationale: the allowlist was the defect's mechanism, not its instance. Adding the two missing boards would have left the next board, and any re-pin of an existing one, failing the same way; reading the kernel's own configuration cannot drift from the kernel the image links against.
- Rejected alternative: enlarge and re-pin the physical platforms' CNodes to 19 bits. That spends 16 MiB of kernel memory per board to serve a 256 MiB-holder case no runtime can reach — `MAX_REGION_PAGES` clamps every real holder to 512 pages — and it would require re-taking each board's pinned kernel and prefix hashes, including the two boards whose evidence is physical.
- Decision: measure `size_of::<ObjectAllocator>()` against a quarter of the CSpace rather than asserting an exact record count per platform.
- Rationale: the constraint is that the image's own `.bss` must leave room for the product graph's objects; a per-platform record count would need re-blessing whenever an unrelated allocator field changes, while the measured bound tracks the real cost.

## Open risks and follow-ups

- [ ] Neither AArch64 board was booted for this change, so the fix is verified by measurement against their pinned kernel configurations and installed prefixes, not by board evidence. `just rpi5_boot_check` still fails closed on the missing serial adapter; the NT98690 path is manual.
- [ ] `plan_task_backing`'s `>512`-page branch still omits the per-span fallback extent and large-frame descriptors, unchanged from the previous round; the ceiling-raising milestone owns it together with re-taking the frozen capacity markers.

## Artifacts and provenance

- Focused report: none.
- Raw transcript: none.
- Serial/debugger/model output: command output from this session, quoted in Verification.
- Related work item: none.
