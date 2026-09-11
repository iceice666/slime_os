# PR review comment ownership, ABI pinning, and capacity integration

| Field | Value |
|---|---|
| Date | 2026-09-11 |
| Kind | Change |
| Status | Verified |
| Scope | QEMU ARM SMP configuration comments, private-memory endpoint ABI slots, allocator provenance comments, capacity architecture documentation, main integration, generated image closures and test-run identities |
| Work items | none |
| Gates | `just system_spec_check`, `just private_memory_check`, `just system_test_run_check`, `just fmt_check_all`, `just lint_all`, `just devlog_check` |
| Trigger | PR #25 reviews of `640a6cba` and `b9d4027f` requested moving implementation history out of configuration and allocator comments and pinning the denied private-memory probe endpoint to its hard-coded ABI slot; the branch also conflicted with main |
| Baseline | Capacity branch `640a6cba`, review-fix merge `b9d4027f`, and main `a1119ca6` |

## Summary

Moved the SMP assurance discussion out of the QEMU ARM kernel configuration
into the existing capacity architecture document. Removed the failed-boot and
replaced-array narrative from the allocator's provenance-table comment; its
existing devlog retains that evidence. Pinned both private-memory probe roles'
shared endpoint to ABI slot 0, matching the binary's `LOOPBACK_SLOT` contract.
Integrated main without reverting the branch's closure-provenance cutover or
19-bit QEMU root-CNode pins. The merged SDK corpus builds, boots, and rolls back
with identical artifact identities.

## Changes

| Area | Change | Preserved invariant |
|---|---|---|
| SMP comments | Replaced the 31-line discussion with three invariant/link lines; consolidated target-specific assurance and future concurrency audit requirements in `docs/directions/34-capacity-ceilings.md` | Configuration comments describe current constraints; architecture documentation owns alternatives and rationale |
| Private-memory endpoint ABI | Added the denied holder's `componentAbi` slot-0 pin and regenerated its manifest, closure, and system-test-run identities | Adding or reordering unrelated denied-holder bindings cannot move the endpoint away from the slot the probe binary uses |
| Allocator provenance comment | Removed the replaced-array narrative, observed boot refusal, and exact historical capacity comparison; retained only the current bounded-table invariant | Implementation comments state current invariants; `devlog/2026-08-28-io1-hardware-resource-authority/` owns the investigation evidence |
| Main integration | Preserved main's H1V1 PWM/UART board facts, tooling, work items, and evidence alongside the branch's QEMU pin checks | Neither side's independent changes are discarded |
| Conflict resolution | Regenerated 52 closures from merged sources, then re-blessed 48 test-run records; merged both devlog index additions chronologically | Generated identities describe the integrated source tree, not either pre-merge side |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Comment cleanup changes kernel options | Compare all non-comment CMake statements with branch HEAD; run the pin checker | Any changed setting or pin/config disagreement |
| Denied probe endpoint moves when another binding sorts first | System-spec derivation requires the explicit denied-holder `componentAbi` pin; the real private-memory plane exercises its receive/reply path | Generated manifest lacks denied slot 0, derivation drifts, or QEMU worker RPC stalls |
| Allocator comment regains investigation history | Repository comment policy plus review of the focused source diff | Replaced-array, failed-boot, or observed-refusal narrative appears beside the live bound |
| Generated conflict resolution retains stale identities | Closure generator check mode and `just system_test_run_check` | Stale closure or mismatched image identity |
| SDK corpus loses the provenance cutover during integration | Existing SDK system-image checker | Export, build, declared QEMU boot, rollback identity, or selected-profile refusal fails |
| One side's work-item or devlog additions disappear | `just tasks_check`, `just devlog_check` | Store/index integrity failure |

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
| `just fmt_check_all`, `just lint_all` | Passed with warnings denied | Direct |
| `just tasks_check` | 289 work items and 94 frozen backlog headings passed | Direct |
| `just ruff`, `just fmt_check_all` | Passed | Direct |
| Index conflict resolution | Zero unresolved Git index entries after staging the regenerated corpus and combined index | Direct |

## Decisions

- Merge main with two parents rather than rewrite the already-reviewed branch history. Main's existing commits remain attributable to their original authors.
- Regenerate conflicting closure records rather than choose either side's hashes. Preserve both additive devlog histories.
- Do not rebuild or re-pin kernels for a comment-only CMake edit: the pin checker ignores comments, and installed artifact identities do not hash configuration source comments.
- Do not broaden this review fix into the adjacent MCS discussion or unrelated documentation revisions.

## Open risks and follow-ups

- No new physical-board run was performed. Main's PWM/UART observations are inherited evidence, not measurements repeated during this integration.
- No new SDK release was published. The smoke checker creates local immutable export repositories for build and rollback verification.
- The previously deferred above-512-page capacity planning findings remain outside this comment and integration fix.

## Artifacts and provenance

- Review: [PR #25 SMP comment finding](https://github.com/iceice666/slime_os/pull/25#discussion_r3980801681).
- Review: [PR #25 denied endpoint ABI-slot finding](https://github.com/iceice666/slime_os/pull/25#discussion_r3981263531).
- Review location: [`object_allocator.rs` provenance comment at reviewed commit](https://github.com/iceice666/slime_os/blob/b9d4027f9310105fdf372f129a6523d00e4d1ba5/slime-root/src/object_allocator.rs#L186-L190).
- Existing provenance investigation: [IO1 hardware resource authority](../2026-08-28-io1-hardware-resource-authority/index.md#decisions).
- Owning rationale: [capacity ceilings](../../docs/directions/34-capacity-ceilings.md#single-core-is-a-config-value-with-an-assurance-price).
- Raw transcripts: session command outputs; no physical evidence collected or rewritten.
