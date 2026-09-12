# The descriptor tables are `.data`, not `.bss`, and three RV64 arms had no record

| Field | Value |
|---|---|
| Date | 2026-09-12 |
| Kind | Defect |
| Status | Verified |
| Scope | `slime-root/src/object_allocator.rs` residency invariant, `slime-root/child/src/main.rs` fault-injection fixture, `scripts/generate/generate-system-test-runs.py` RV64 variants, `scripts/check/check-system-image-aggregate.py` image scan and exemptions |
| Work items | 01a07a2d-003d-77fc-9df8-dda85ed9a083 |
| Gates | `just sel4_root_boot_check`, `just private_memory_check`, `just system_test_run_check`, `just system_image_closure_aggregate_check`, `just system_image_closure_check`, `just test_sel4_root`, `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos`, `just devlog_check` |
| Trigger | PR #25 review of `d5e593cb` reported that `LARGE_DESCRIPTOR_TABLES`'s sizing rationale claims `.bss` for tables whose sentinels are non-zero, that the injected-failure fixture reports zero-fill without reading the page, and that `just riscv64_qemu_check`'s invocations have no frozen records |
| Baseline | Branch head `d5e593cb`, where the allocator comment states the tables are `.bss`, the `slime_private_fail_second_allocation` fixture sets `REPORT_MEM_ZEROED` from the returned page count alone, and `EXTRA_RUNS` registers one RV64 variant |

## Summary

The reviewer's `.bss` claim is correct and was verified by measurement, not
argument: `AllocationRecord::EMPTY`, `ArenaAllocation::EMPTY`, and the list-head
sentinels are `MAX`-valued, so `objdump -h` reports `.data` at 4,460,376 bytes
on a 19-bit kernel against 992 bytes on rpi5's small envelope. The suggested
remedy — restore zero-backed storage — was attempted and **rejected on
evidence**: `MaybeUninit::new(ObjectAllocator::empty())` materializes the whole
multi-megabyte value as a stack temporary, which overflows the root's 1 MiB
stack and cap-faults before the first marker on both QEMU planes. The invariant
is therefore corrected to state `.data`, with the failed alternative recorded
beside the static so it is not retried blind. Separately, the fault-injection
fixture claimed zero-fill without reading the page, and three RV64 checker arms
that `just riscv64_qemu_check` really executes had no frozen test-run record —
one of which the aggregate gate could not even see, because its image name is
composed from an f-string.

## Observable symptom

- Command: `objdump -h build/sel4-cargo/qemu-arm-virt/root/aarch64-sel4-roottask-minimal/release/slime-root.elf`
- Expected, per the comment: the descriptor tables in `.bss`, costing no image bytes
- Observed: `.data` 4,460,376 bytes, `.bss` 1,749,088 bytes; the same root built against the rpi5 12-bit prefix reports `.data` 992 bytes, which is the delta the tables account for
- Exit/fault/serial evidence: after switching the static to `MaybeUninit`, `.data` fell to 992 bytes and `.bss` rose to 6,209,632 — and both planes then failed with `Caught cap fault in send phase at address 0` before any marker

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `AllocationRecord::EMPTY` sets `owner: u16::MAX`, `extent: PRIVATE_EXTENT_NONE`, `next_state: PRIVATE_STATE_NONE`; `ArenaAllocation::EMPTY` is `u32::MAX` | A `const`-initialized allocator is not all-zero, so the linker must emit its bytes |
| 2 | `.data` measured at 4,460,376 bytes (19-bit) against 992 (rpi5) | The claim is wrong by ~4.4 MB, exactly the table size |
| 3 | `graph_runtime`'s `LAUNCH_TASKS` solves the identical problem with `MaybeUninit` and names `OBJECT_ALLOCATOR` as the pattern it follows | The repository already had the intended remedy, and its comment was stale |
| 4 | Applying that pattern moved the tables to `.bss` as predicted | The storage-class fix works at link time |
| 5 | Both planes then cap-faulted at address 0 before the first marker; reverting restored ordered markers | `MaybeUninit::new(value)` builds the value in a frame first — backlog B3's failure mode, at 4 MiB against a 1 MiB stack |
| 6 | `just riscv64_qemu_check` invokes five checkers with `--platform qemu-riscv-virt`; `check-sel4-wait-set-plane.py` rejects that choice outright and `check-sel4-sample-plane.py` builds an AArch64 closure image and times out | Two arms cannot run at all; both are identical on `main`, so they predate this PR |
| 7 | Root-boot, generation, and rollback arms all pass on RV64, and none had a test-run record | Three real invocations were unfrozen |
| 8 | Adding exemptions for the generation and rollback RV64 images was rejected by the aggregate gate as "naming an image no checker boots" | Those names are built as `f"slime-sel4-rollback{suffix}.elf"`, invisible to a literal scan |

## Root cause

Two independent causes. The residency claim was written for the storage class
the tables *should* have had, and no gate asserts a section, so it drifted from
what the linker actually emits. The unfrozen RV64 arms follow from
`check_records_and_planes_correspond` deriving its expected set from the same
`EXTRA_RUNS` table it validates: a variant absent from the table is absent from
both sides of the comparison, so the check passes vacuously.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Residency invariant | States `.data`, names the `MAX`-valued sentinels that put it there, and keeps the CSlot-per-image-page consequence | The comment describes the bytes the linker emits |
| Static storage | `OBJECT_ALLOCATOR` stays `const`-initialized, with the rejected `MaybeUninit` alternative and its observed cap-fault recorded beside it | A future reader does not re-derive a change that does not boot |
| Fixture zero-fill | The injected-failure path reads three offsets of the grown page before writing `MEM_PATTERN`, mirroring the ordinary path | `REPORT_MEM_ZEROED` reports an observation, not an inference from a page count |
| RV64 variants | `EXTRA_RUNS` registers the generation, rollback, and private-memory RV64 arms, keyed on the recipe that invokes them | Every executable invocation of a plane checker has a frozen record |
| Aggregate scan | `booted_images` expands `f"slime-…{suffix}.elf"` against the checker's own platform vocabulary | An image a recipe boots is visible to the gate whether or not its name is spelled in one piece |
| Exemptions | The three RV64 images are declared with their blocker: their closures name `qemu-arm-virt` | An image with no closure carries a stated reason |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The allocator's tables outgrow the linked kernel's CSpace | `allocator_state_fits_the_linked_kernels_root_cspace` in `just test_sel4_root` | `allocator state spends N of M root CSlots` |
| A growth returns a recycled non-zero frame | `just private_memory_check` | `SLIME_CHILD mem nonzero addr=… value=…`, and the missing `REPORT_MEM_ZEROED` flag |
| An executable RV64 arm loses its record | `just system_test_run_check` | Record/plane correspondence names the missing variant |
| A composed image name escapes the exemption table | `just system_image_closure_aggregate_check` | `booted by […] but no closure named … exists and no reason is declared` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `objdump -h` on the AArch64 and rpi5 roots | `.data` 4,460,376 vs 992 bytes | Direct |
| `MaybeUninit` variant | `.data` 992, `.bss` 6,209,632 — then `Caught cap fault in send phase at address 0` on both `check-sel4-root-boot.py` arms; reverting restored ordered markers | Direct |
| `check-sel4-root-boot.py` at branch head with changes stashed | Passed on RV64, isolating the fault to the attempted change | Direct |
| `just sel4_root_boot_check` | Ordered generation, timer, task, IPC, fault, and ready markers on the pinned profile | Direct |
| `just private_memory_check` | AArch64 and RV64: 23 markers across 7 causal chains each, including the fault-injection image whose fixture now reads the page | Direct |
| `check-sel4-generation-plane.py --platform qemu-riscv-virt` | Passed | Direct |
| `check-sel4-rollback-plane.py --platform qemu-riscv-virt` | Passed | Direct |
| `just system_test_run_check` | 50 records, 46 resolving a closure, 4 declared closure-exempt | Direct |
| `just system_image_closure_aggregate_check` | 16 booted images — 8 closure-reachable, 8 exempt with a declared reason | Direct |
| `just system_image_closure_check` | Contracts, resolution, identity, isolation, and bytes verified; closure `5be26d059fc5` built | Direct |
| `just test_sel4_root` | 235/235 across 19 modules | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos` | Passed | Direct |

## Decisions

- Decision: correct the invariant to `.data` rather than moving the tables to `.bss`.
- Rationale: the storage-class change works at link time and does not boot. `MaybeUninit::new(ObjectAllocator::empty())` constructs the value before storing it, so a 4 MiB temporary lands in a 1 MiB stack; both planes cap-faulted at address 0 before the first marker, and reverting restored them. Shrinking this cost is a representation change — zero-valued sentinels — not a storage-class change, and that is a separate piece of work with its own regression surface.
- Rejected alternative: keeping the `MaybeUninit` form and raising the stack. The temporary is the whole table; the stack would have to exceed it, which trades ~1026 root CSlots of image for a larger stack reservation and still copies 4 MiB at every boot.

- Decision: register only the RV64 arms that actually execute.
- Rationale: a frozen record asserts execution inputs for a real invocation. `check-sel4-wait-set-plane.py --platform qemu-riscv-virt` is rejected by its own argument parser, and `check-sel4-sample-plane.py` builds an AArch64 closure image and times out on RV64; a record for either would freeze inputs for something that never runs. Both are byte-identical on `main`, so they are pre-existing recipe defects rather than anything this PR introduced.
- Rejected alternative: recording all five and marking two as expected failures. That makes the corpus assert a boot that does not happen.

- Decision: expand composed image names in the aggregate scan instead of hand-listing them.
- Rationale: the literal scan rejected the very exemptions the correspondence check requires, and the reason was purely lexical. Expanding `f"…{suffix}.elf"` against the checker's own declared platforms keeps one authority; a parallel list would be the same fact maintained twice.

## Open risks and follow-ups

- [ ] `just riscv64_qemu_check` still invokes two checkers that cannot run on RV64: `check-sel4-wait-set-plane.py` has no `qemu-riscv-virt` platform entry, and `check-sel4-sample-plane.py` builds only its AArch64 closure image. Both are identical on `main` and out of this PR's scope; fixing them means either giving those planes a real RV64 build path or removing the lines from the recipe.
- [ ] The descriptor tables still cost ~1026 root CSlots of image on 19-bit kernels. Eliminating that needs zero-valued sentinels in `AllocationRecord` and `ArenaAllocation`, which changes the encoding rather than the storage class.

## Artifacts and provenance

- Focused report: none; each change is local to its named file.
- Raw transcript: none retained; every gate above is reproducible from the listed command.
- Serial/debugger/model output: `just sel4_root_boot_check` and `just private_memory_check` (AArch64 and RV64) serial transcripts, plus the RV64 generation and rollback plane transcripts.
- Related work item: [MEM-ARENAS](../../.tasks/items/01a07a2d-003d-77fc-9df8-dda85ed9a083.md)
- Preceding investigation: [A single-profile SDK export could not publish, and five more comment-ownership fixes](../2026-09-12-corpus-profile-and-comment-ownership/index.md)
