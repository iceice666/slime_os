# PR review comment ownership, ABI pinning, and capacity integration

| Field | Value |
|---|---|
| Date | 2026-09-11 |
| Kind | Change |
| Status | Verified |
| Scope | QEMU ARM SMP configuration comments, private-memory endpoint ABI slots, allocator and rollback comment ownership, aligned live-untyped capacity simulation, MEM-ARENAS evidence correction, main integration, generated image closures and test-run identities |
| Work items | 01a07a2d-003d-77fc-9df8-dda85ed9a083 |
| Gates | `just system_spec_check`, `just test_sel4_root`, `just private_memory_check`, `just sel4_reclamation_check`, `just tasks_check`, `just fmt_check_all`, `just lint_all`, `just devlog_check` |
| Trigger | PR #25 reviews requested moving implementation history out of configuration and allocator comments, pinning the denied private-memory probe endpoint to its hard-coded ABI slot, refusing capacity reports whose aggregate ordinary bytes cannot realize the aligned extent sequence, correcting the contradicted MEM-ARENAS exit, and updating stale rollback commentary; the branch also conflicted with main |
| Baseline | Capacity branch `640a6cba`, review-fix merge `b9d4027f`, main `a1119ca6`, pre-alignment review head `12d73649`, and latest reviewed head `6f97fd3b` |

## Summary

Moved the SMP assurance discussion out of the QEMU ARM kernel configuration
into the existing capacity architecture document. Removed investigation and
future-milestone narratives from allocator and reclamation comments; existing
devlogs retain that evidence. Pinned both private-memory probe roles' shared
endpoint to ABI slot 0, matching the binary's `LOOPBACK_SLOT` contract. Capacity
qualification now replays static, private-table, and private-data extent
placement against copies of the live ordinary untyped region watermarks, so
aggregate free bytes cannot report a fit that aligned 2 MiB extents cannot
realize. The resulting `ordinary_layout=0`, `fit=0` evidence contradicts the
MEM-ARENAS four-holder exit, so the milestone is reopened and its recorded exit
is corrected. Private-growth and fault-probe comments now state only the
current retained-allocation rollback and reclamation requirements. Integrated
main without reverting the branch's closure-provenance cutover or 19-bit QEMU
root-CNode pins.

## Changes

| Area | Change | Preserved invariant |
|---|---|---|
| SMP comments | Replaced the 31-line discussion with three invariant/link lines; consolidated target-specific assurance and future concurrency audit requirements in `docs/directions/34-capacity-ceilings.md` | Configuration comments describe current constraints; architecture documentation owns alternatives and rationale |
| Private-memory endpoint ABI | Added the denied holder's `componentAbi` slot-0 pin and regenerated its manifest, closure, and system-test-run identities | Adding or reordering unrelated denied-holder bindings cannot move the endpoint away from the slot the probe binary uses |
| Allocator provenance comment | Removed the replaced-array narrative, observed boot refusal, and exact historical capacity comparison; retained only the current bounded-table invariant | Implementation comments state current invariants; `devlog/2026-08-28-io1-hardware-resource-authority/` owns the investigation evidence |
| Capacity placement | Replay one static extent followed by the planned private table and data extents for every holder against copied live untyped watermarks | `ordinary_layout=1` implies `provision_extent` can select a single aligned region for every planned extent; the existing restricted qualification now reports the honest `ordinary_layout=0` alongside `fit=0` |
| Remaining implementation comments | Reduced reclamation and backing-plan comments to current slot-count, runtime-ceiling, and extent-shape invariants | Investigation evidence and future work remain in devlogs or architecture documentation |
| MEM-ARENAS state | Reopened the milestone and replaced its contradicted observed exit with the new per-region placement result; appended a correction to the original devlog | A milestone remains done only while every exit condition has observed supporting evidence |
| Private rollback comment | Replaced extent-revocation wording with the live transaction boundary: unmap in-flight frames, retain typed reusable records and extents, and preserve committed mappings | Documentation matches the non-destructive rollback mechanism |
| Reclamation probe comment | Removed work-item, gate, and refusal-history narration; retained only the local requirement to allocate private backing before the deliberate fault | Implementation comments state the behavior required by the probe |
| Main integration | Preserved main's H1V1 PWM/UART board facts, tooling, work items, and evidence alongside the branch's QEMU pin checks | Neither side's independent changes are discarded |
| Conflict resolution | Regenerated 52 closures from merged sources, then re-blessed 48 test-run records; merged both devlog index additions chronologically | Generated identities describe the integrated source tree, not either pre-merge side |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Comment cleanup changes kernel options | Compare all non-comment CMake statements with branch HEAD; run the pin checker | Any changed setting or pin/config disagreement |
| Denied probe endpoint moves when another binding sorts first | System-spec derivation requires the explicit denied-holder `componentAbi` pin; the real private-memory plane exercises its receive/reply path | Generated manifest lacks denied slot 0, derivation drifts, or QEMU worker RPC stalls |
| Allocator comment regains investigation history | Repository comment policy plus review of the focused source diff | Replaced-array, failed-boot, or observed-refusal narrative appears beside the live bound |
| Fragmented ordinary RAM passes the aggregate byte comparison | Host regression supplies three partly consumed 2 MiB regions whose total exceeds the requirement but none can place a 2 MiB extent | `ordinary_layout_fits=false` and the capacity report refuses the plan |
| Capacity simulation diverges from provisioning order | The pure simulator uses the same first-fitting-region and `plan_allocation` alignment rule as `allocate_from_global`, replaying static, table, then data extents | Host regression fails or the QEMU qualification marker changes |
| Generated conflict resolution retains stale identities | Closure generator check mode and `just system_test_run_check` | Stale closure or mismatched image identity |
| SDK corpus loses the provenance cutover during integration | Existing SDK system-image checker | Export, build, declared QEMU boot, rollback identity, or selected-profile refusal fails |
| One side's work-item or devlog additions disappear | `just tasks_check`, `just devlog_check` | Store/index integrity failure |
| Invalid four-holder result remains closed | MyQue state plus `just tasks_check`; original devlog correction links the replacement evidence | MEM-ARENAS is `done`, retains the stale observed exit, or lacks a correction explaining the evidence reversal |
| Rollback documentation regresses to extent revoke | Focused source review plus private-memory rollback regressions | Module invariant claims a transaction revokes shared backing extents |
| Probe comment accumulates gate history | Focused source review | Work-item findings or checker behavior are narrated beside the faulting operation |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Compare non-comment `qemu-arm-virt.cmake` statements against `640a6cba` | Identical; `KernelMaxNumNodes` remains 1 | Direct |
| `python3 scripts/check/check-sel4-pins.py --skip-host-tools` | Source, toolchain, target, config, and host pin declarations verified; QEMU 19-bit and H1V1 checks coexist | Direct |
| `python3 scripts/generate/generate-system-image-closures.py --check` | All 52 closures current | Direct |
| `just system_test_run_check` | 48 records, 46 closure associations, and five refusal controls passed | Direct |
| `just system_image_closure_aggregate_check` | All 52 closures covered; six drift controls refused | Direct |
| `python3 scripts/check/check-component-sdk-system-image.py` | Two SDK exports built the declared closure; current and rollback QEMU boots passed; rollback reproduced every build-result artifact identity; missing/mismatched selected profiles refused | Direct |
| `just system_spec_check` | 43 systems compiled; all 43 derived manifests matched; the denied probe's generated binding is pinned to slot 0 | Direct |
| `just private_memory_check` | AArch64 observed 23 markers across seven chains and three images; RV64 observed 23 markers across seven chains, including the exactly-once worker RPC | Direct |
| `just system_test_run_check` | All 48 records matched their owning plane gates; 46 closure associations and five refusal controls passed | Direct |
| `just test_sel4_root` | 232/232 passed, including aggregate-byte-sufficient fragmented regions that cannot place the aligned full-window extents and a contiguous positive control | Direct |
| `just private_memory_check` after alignment fix | AArch64 observed 23 markers across seven chains and three image cases; its restricted four-holder qualification reported `ordinary_layout=0`, `fit=0`; RV64 observed 23 markers across seven chains | Direct |
| `just sel4_gate_control_check` after alignment fix | Passed, including refusal of a mutated `ordinary_layout=0`, `fit=1` report | Direct |
| `just fmt_check_all`, `just lint_all` | Passed with warnings denied | Direct |
| `just tasks_check` | 289 work items and 94 frozen backlog headings passed | Direct |
| `just ruff`, `just fmt_check_all` | Passed | Direct |
| `myque reopen 01a07a2d-003d-77fc-9df8-dda85ed9a083` and `just tasks_check` | MEM-ARENAS is open; 289 work items and 94 frozen backlog headings passed | Direct |
| `just sel4_reclamation_check` after probe comment cleanup | Segmented task extents and root CSlots were reclaimed and reused | Direct |
| `just private_memory_check` after rollback comment cleanup | AArch64 observed 23 markers across seven chains and three image cases; RV64 observed 23 markers across seven chains and one image case | Direct |
| `just contracts_check`, `just generation_check` | Contract corpus passed; two isolated generation builds were byte-identical and four CPU-budget mutations were refused | Direct |
| `just system_test_run_check`, `just system_image_closure_aggregate_check` | 48 run records matched their gates; all 52 regenerated closures remained covered | Direct |
| Index conflict resolution | Zero unresolved Git index entries after staging the regenerated corpus and combined index | Direct |

## Decisions

- Merge main with two parents rather than rewrite the already-reviewed branch history. Main's existing commits remain attributable to their original authors.
- Regenerate conflicting closure records rather than choose either side's hashes. Preserve both additive devlog histories.
- Do not rebuild or re-pin kernels for a comment-only CMake edit: the pin checker ignores comments, and installed artifact identities do not hash configuration source comments.
- Do not broaden this review fix into the adjacent MCS discussion or unrelated documentation revisions.

## Open risks and follow-ups

- No new physical-board run was performed. Main's PWM/UART observations are inherited evidence, not measurements repeated during this integration.
- No new SDK release was published. The smoke checker creates local immutable export repositories for build and rollback verification.
- Follow-up review closed the previously deferred above-512-page capacity planning findings; the current planner and table bounds now include fallback extents, retained large frames, and static task descriptors.
- Capacity placement is simulated from a copy of the current region watermarks. Concurrent mutation is impossible while slime-root emits this single-threaded qualification report; a future concurrent allocator would require holding the allocator's serialization boundary across snapshot and decision.

## Artifacts and provenance

- Review: [PR #25 SMP comment finding](https://github.com/iceice666/slime_os/pull/25#discussion_r3980801681).
- Review: [PR #25 denied endpoint ABI-slot finding](https://github.com/iceice666/slime_os/pull/25#discussion_r3981263531).
- Review location: [`object_allocator.rs` provenance comment at reviewed commit](https://github.com/iceice666/slime_os/blob/b9d4027f9310105fdf372f129a6523d00e4d1ba5/slime-root/src/object_allocator.rs#L186-L190).
- Review: [PR #25 aligned ordinary-untyped capacity finding](https://github.com/iceice666/slime_os/pull/25#discussion_r3985951748).
- Review locations: [`task.rs` reclamation comment](https://github.com/iceice666/slime_os/blob/12d736491d9a88a422fd0a191ecd6768a7ba2ed9/slime-root/src/task.rs#L1129-L1133) and [`object_allocator.rs` backing-plan comment](https://github.com/iceice666/slime_os/blob/12d736491d9a88a422fd0a191ecd6768a7ba2ed9/slime-root/src/object_allocator.rs#L554-L560).
- Review: [PR #25 contradicted MEM-ARENAS exit](https://github.com/iceice666/slime_os/pull/25#discussion_r3986304556).
- Review: [PR #25 stale private rollback invariant](https://github.com/iceice666/slime_os/pull/25#discussion_r3986304561).
- Review: [PR #25 reclamation probe comment ownership](https://github.com/iceice666/slime_os/pull/25#discussion_r3986304566).
- Existing provenance investigation: [IO1 hardware resource authority](../2026-08-28-io1-hardware-resource-authority/index.md#decisions).
- Owning rationale: [capacity ceilings](../../docs/directions/34-capacity-ceilings.md#single-core-is-a-config-value-with-an-assurance-price).
- Raw transcripts: session command outputs; no physical evidence collected or rewritten.
