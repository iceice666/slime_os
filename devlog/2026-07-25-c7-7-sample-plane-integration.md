# C7.7 sample-plane integration and isolation

| Field | Value |
|---|---|
| Date | 2026-07-25 |
| Status | Verified |
| Scope | Two-component sample-plane composition test over C7.2–C7.6, `just sample_plane_check`, hand-built retained-v2 known-good decode probe |
| Trigger | C7.7 milestone: close the C7 bounded resource and shared-sample plane gate |
| Baseline | C7.1–C7.6 complete; each shared-buffer primitive (factory allocation, per-holder quotas, mapping/sealing, loan/return lifecycle, sample descriptor) verified in isolation but never composed end to end |

## Summary

C7.7 composes the C7.2 factory allocation, C7.3 per-holder quotas, C7.4
mapping/sealing, C7.5 loan/return lifecycle, and the C7.6 sample descriptor into
the two-component exit condition. No new kernel mechanism was introduced: the
milestone is a QEMU integration test (`kernel/tests/sample_plane.rs`) plus a
`just sample_plane_check` recipe. Two isolated holders (distinct owner ids and
address spaces) exchange a payload larger than the kernel IPC message bound —
only the 64-byte descriptor crosses a real `ipc::channel`, while the receiver
reconstructs the full two-page payload from a quota-charged, sealed, loaned
buffer through exact read-only page-table translations. Malformed descriptors,
every per-holder quota class, and peer death all remain bounded and reclaim all
resources without disturbing an unrelated channel or the retained v2 known-good
decode path.

## Observable symptom

Not a defect; a milestone gate. The new verification target:

- Command: `just sample_plane_check`
- Expected: five QEMU cases pass; the C7 gate closes.
- Observed: `5 test(s)` all `[Passed]`.
- Exit/fault/serial evidence: QEMU `Success` exit under `-display none`.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | C7.6 test built a `Message` struct directly rather than moving the descriptor through a channel | C7.7 uses real `ipc::send`/`ipc::recv` so the "descriptor only, payload never in the queue" claim is exercised, not asserted |
| 2 | `SharedBufferTable::release` retains a buffer's page while a loan is outstanding (C7.5 semantics) | Cleanup of a still-loaned buffer must go through `reclaim_owner`, not `release`; first test run failed `total_pages()==0` until corrected |
| 3 | `Generation` header `bootstrap_component` is a component index, not an object index | First v2-artifact decode returned `BadIndex` with the field set to 1; the sole component is index 0 |

## Root cause

N/A — feature composition milestone, not a regression.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `kernel/tests/sample_plane.rs` | New five-case integration test composing C7.2–C7.6 over real channels, address spaces, and page tables | C7.7 exit condition observed end to end |
| `Justfile` | New `sample_plane_check` recipe (depends on `contracts_check`), mirroring the C7.6 `sample_descriptor_check` shape | Milestone owns an independently runnable QEMU gate |
| `roadmap/02-core-runtime.md`, `roadmap/README.md` | Marked C7.7 and the C7 track complete; recorded evidence | Roadmap reflects observed exit condition |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Payload leaks through the kernel message queue instead of the shared buffer | `just sample_plane_check` (`two_components_exchange_and_return_payload_over_message_bound`) | `recv` returns more than `DESCRIPTOR_LEN`, or reconstructed payload mismatch |
| A malformed descriptor maps or allocates receiver state | `malformed_descriptor_over_channel_maps_nothing` | `owner_mappings(RECEIVER) != 0` after a stale-identity descriptor |
| A quota class fails to bound, or exhaustion disturbs an unrelated owner | `every_quota_class_is_bounded_and_isolated` | wrong error kind, or unrelated buffer/mapping/channel perturbed |
| Peer death leaks charges or breaks an unrelated channel | `peer_death_reclaims_all_and_preserves_unrelated_channel` | non-zero retained charges, or unrelated channel stops delivering |
| Sample-plane work perturbs the retained v2 rollback window | `retained_v2_known_good_decode_is_unaffected` | v2 identity/rights change across a full exchange |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sample_plane_check` | 5/5 `[Passed]`, QEMU `Success` | Direct |
| `just test` | full kernel suite `[Passed]` (includes `sample_plane`) | Direct |
| `just fmt_check` | clean | Direct |
| `just lint` | clean (`-D warnings`) | Direct |
| `just contracts_check` | clean (run as a `sample_plane_check` dependency) | Direct |

## Decisions

- Decision: move the descriptor over a real `ipc::channel` (`send`/`recv`) rather than a hand-built `Message`.
- Rationale: the C7.7 exit condition is about isolated *components* exchanging a payload; using the real queue proves the >`MAX_MSG` payload never traverses it.
- Rejected alternative: reuse the C7.6 direct-`Message` construction — it would not exercise the channel boundary the milestone is about.

- Decision: build the retained-v2 known-good artifact in-test, mirroring the boot-contracts v2 builder layout.
- Rationale: no committed v2 binary fixture is reachable from a kernel test, and the boot generation is v3; an in-test v2 artifact is the minimal way to prove the rollback window decode is unperturbed.
- Rejected alternative: depend on a v3 boot generation only — it would not touch the retained v2 code path the required check names.

## Open risks and follow-ups

- [ ] None specific to C7.7. C8 (native typed data fabric) is the next open Core-runtime slice; its B2 wait/wake dependency is already resolved.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: reviewer verdict at `history://HostileGoldfish` (overall_correctness: correct, no findings).
- Serial/debugger/model output: `just sample_plane_check` QEMU serial (five `[Passed]` lines).
- Related roadmap item: `roadmap/02-core-runtime.md` C7.7.
