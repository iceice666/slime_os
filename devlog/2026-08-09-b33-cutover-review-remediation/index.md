# B33 — seL4 kernel cutover review remediation

| Field | Value |
|---|---|
| Date | 2026-08-09 |
| Kind | Audit |
| Status | Verified |
| Scope | seL4 root capability/lifecycle/memory/storage paths; component runtime and services; build, gate, CI, profile, roadmap, and dependency policy |
| Roadmap | P5.4.final, B33 |
| Gates | `just test_sel4_root`, `just test_host`, `just sel4_qos_check`, `just sel4_root_boot_check`, `just sel4_gate_control_check` |
| Trigger | Static cutover review recorded CUT-001 through CUT-077 as merge-blocking or same-branch findings |
| Baseline | P5.4.final had retired the custom kernel, but the review found mechanism defects and gates that could not establish their named claims |

## Summary

The cutover review recorded 77 findings across capability isolation, task construction and reclamation, shared-memory aliases, virtio and durable storage, runtime services, gate correctness, CI/profile policy, and project records. Every CUT-001 through CUT-077 finding was re-grounded and repaired. Verification then exposed two additional integration defects: the QoS subset proof sent its transferred capability into the fabric control channel, and init retained copied endpoint ends that prevented peer-death retirement. The proof now uses a private carrier pair and init drops retained authority after spawning; the QoS plane drains to its terminal marker.

## Observable symptom

- Command: `just sel4_qos_check`
- Expected: the current QoS image builds, all six configured participants exit cleanly, and init reports `[init] fabric stream complete`.
- Observed before the final integration repair: first, `fabric-publisher` failed while collecting the narrowed transfer; after routing that transfer privately, the plane timed out with init parked because retained endpoint copies kept the fabric queues live.
- Exit/fault/serial evidence: the repaired run reports `narrowing succeeded and widening was refused`, all configured participants terminate cleanly, peer-dead retirement completes, and the gate exits successfully.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | The handoff contained exactly CUT-001 through CUT-077, grouped by mechanism, runtime, gate, CI, and record ownership. | Each finding was tracked independently; no category-level completion substituted for an item. |
| 2 | Root/task and shared-buffer tests exposed ownership transitions that were previously comments or counters rather than enforced cleanup. | Construction, spawn unwind, reply-slot, alias, orphan, and loan-right paths gained fail-closed state transitions and focused tests. |
| 3 | Repaired gates rejected stale artifacts, truncated transcripts, false target aliases, and incomplete marker matching. | Later green runs became current-tree evidence rather than inherited results. |
| 4 | The repaired QoS gate initially failed in the new capability-subset arm. | The proof transport was separated from the fabric protocol using a private carrier endpoint pair. |
| 5 | The next QoS run reached participant completion but not fabric termination. | Init's retained copies were identified as live holders and explicitly dropped after child spawn. |
| 6 | Final lint and dependency runs found a Clippy simplification and cargo-deny treating the pinned upstream rust-sel4 workspace as publishable crates. | Code was simplified; ISC was admitted; wildcard checks now skip only the pinned rust-sel4 dependency trees while advisories, licenses, and sources remain enforced. |

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Capability and lifecycle | Removed child receive authority; established construction cleanup ownership; recycled parked replies; unwound spawn quotas; made unhealthy termination nonzero; tightened ELF admission and mapping bounds. | A child cannot intercept root requests, and every failed or terminated task has one bounded reclamation path. |
| Shared memory | Sized aliases by pages, made map/record rollback transactional, preserved ownership across failed unmap, continued orphan-full cleanup, and derived loan rights from the mapped region. | No successful or failed mapping leaves untracked access or targets another holder on retry. |
| Storage and contracts | Added volatile virtqueue ordering and timeout poisoning; made flush ambiguity explicit; strengthened GPT, component ELF, page-size, recovery-index, timer, and fixture validation. | Durable and device state cannot be reused after ambiguous ownership, and persisted/component inputs fail closed. |
| Runtime and services | Removed destructive time probing; bounded staged capabilities; corrected diagnostic staging, response lengths, rights constants, recovery authority/idempotency, command slots, phase markers, and transfer source validation. | Userspace observes the same bounded syscall and service contracts the root enforces. |
| Gate correctness | Rebuilt the correct QoS variant; collected terminal graphs; matched causal chains; crossed supervision bounds; restored refusal, failure, lifecycle, device, layout, environment, and marker assertions. | A green gate names the current artifact and contains the evidence its target claims. |
| CI and profiles | Restored dependency checkout, product lint/test coverage, release scenarios, fail-closed architecture/framework targets, build-environment tracking, target requirements, and dependency-policy semantics. | CI cannot silently skip the seL4 product or pass a retired target name as equivalent evidence. |
| Records | Corrected linker, portability, platform, and typed-fabric statements after implementation verification. | Roadmap and devlog claims describe the surviving seL4 product rather than retired commands or unobserved hardware support. |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just test_sel4_root` | 124 tests passed across 13 modules, with the asserted count updated to the observed suite. | Direct |
| `just test_host` | Host contract suites passed. | Direct |
| `just sel4_supervision_check` | The graph crossed `MAX_RECORDS` over its lifetime and retained live supervision handles correctly. | Direct |
| `just sel4_qos_check` | Current QoS artifact built and booted; QoS policy, subset refusal, participant lifecycle, and terminal drain passed. | Direct |
| `just sel4_root_boot_check` | Ordered generation, timer, task, IPC, fault, dynamic adjoining reclaim ranges, and ready markers passed. | Direct |
| `just sel4_gate_control_check` | 27 gates rejected 1,094 mutated transcripts and layouts. | Direct |
| `python3 scripts/check/check-boot-layout-resource.py` | 19 fixtures and 16 seL4 fixtures agreed with the resolver; 18 generation/profile pairs round-tripped. | Direct |
| `just fmt_check_all && just lint_all && just ruff` | Rust formatting, product Clippy, and Python lint passed. | Direct |
| `just deny` | Advisories, bans, licenses, and sources passed; unmatched allowances remained warnings. | Direct |

## Open risks and follow-ups

- Physical Framework/NVMe qualification remains outside this review and retains its existing roadmap status; no physical-machine support claim is made here.
- The seL4 kernel linker still emits its upstream RWX LOAD-segment warning during builds; this review did not classify that upstream image-layout warning as a Slime component mapping.

## Artifacts and provenance

- Focused report: review findings and final dispositions are summarized in this entry.
- Raw transcript: none retained; verification results were observed in the remediation session.
- Serial/debugger/model output: QEMU serial output was consumed by the named seL4 gate scripts.
- Related roadmap item: [`roadmap/07-architecture-portability.md#p54final--delete-kernel`](../../roadmap/07-architecture-portability.md#p54final--delete-kernel)
