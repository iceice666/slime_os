# C8.6 — Bounded native calls

| Field | Value |
|---|---|
| Date | 2026-07-28 |
| Kind | Change |
| Status | Verified |
| Scope | Zutai call schema and bindings, generation graph admission, userspace call broker/components, IPC send-capacity waits, shared-buffer and task reclamation, QEMU call gate |
| Roadmap | C8.6 |
| Gates | `just fabric_call_check`, `just contracts_check`, `just generation_check`, `just test`, `just lint`, `just lint_components`, `just fmt_check`, `just fmt_check_components` |
| Trigger | First unchecked core-runtime milestone after C8.5 |
| Baseline | C8.5 supplied authenticated graph roles and explicit time/events, but no native request/reply correlation or terminal call lifecycle existed |

## Summary

C8.6 adds generation-authorized `Call<Request, Reply>` routing with fixed per-route, per-client, and per-server state; exact generation/session/request correlation; inline and receiver-bound shared payloads; and one terminal outcome for success, rejection, timeout, cancellation, retry exhaustion, malformed reply, or peer death. Duplicate and stale identities do not re-execute non-idempotent work. The live QEMU gate exercises two clients, one server, terminal-queue backpressure, and peer-fault isolation, and the repository validation stack is clean.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Contracts and generation | Added the versioned Zutai fabric-call envelope, generated Rust binding, call graph entries, call component images/grants, and deterministic builder/check integration | Every call message and authority edge is versioned, authenticated, bounded, and generation-declared |
| Userspace fabric | Added a bounded call broker and isolated client, second-client, server, and explicit-time scenarios | Replies, cancellation, and terminal outcomes reach only the correlated client; duplicate/stale requests cannot re-execute non-idempotent work |
| Shared payloads | Relayed large requests and replies through sealed receiver-bound loans and settled all payload variants on every terminal path | Large payload authority remains bound to the declared receiver and every buffer/loan charge is reclaimed |
| Kernel IPC and teardown | Added send-capacity wait registration/wakes, queued-capability settlement on endpoint teardown, and stale waiter cleanup during task termination | A full receive queue can block without polling or lost wakeups; dropped queues cannot retain transferred capabilities or loans |
| Verification scenario | Added a private client/client-B coordination channel that deterministically fills the 16-message terminal queue before draining it | The backpressure arm proves actual queued terminal delivery rather than depending on scheduler timing |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Cross-client correlation or authority widening | `just fabric_call_check` | Missing ordered correlation/cancellation markers or a component/fabric failure marker |
| Duplicate non-idempotent execution | `just fabric_call_check` | The server execution marker appears other than exactly once |
| Lost terminal under a full IPC queue | `just fabric_call_check` | No `terminal delivery queued` marker or client-B cannot drain every terminal |
| Contract drift or malformed generation admission | `just contracts_check` and `just generation_check` | Generated binding mismatch, validation rejection, or nondeterministic generation bytes |
| Kernel IPC/task regression | `just test` | Kernel or QEMU integration test failure |
| Rust diagnostics or formatting drift | `just lint`, `just lint_components`, `just fmt_check`, `just fmt_check_components` | Any warning or formatting difference |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just fabric_call_check` | Passed; ordered live markers covered success, shared request/reply, rejection, malformed reply, duplicate, cancellation, stale session, actual terminal backpressure, timeout, retry exhaustion, peer death, reclamation, and a healthy vertical slice | Direct |
| `just contracts_check` | Passed; fabric-call bindings current and the full contract suite reported 47 passing tests | Direct |
| `just generation_check` | Passed; two generated stores were byte-identical and the selected generation validated | Direct |
| `just test` | Passed, including kernel unit and QEMU integration tests | Direct |
| `just lint` and `just lint_components` | Passed with warnings denied | Direct |
| `just fmt_check` and `just fmt_check_components` | Passed | Direct |

## Decisions

- Decision: keep call policy in the userspace fabric broker and add only generic send-capacity waiting and teardown settlement to the kernel.
- Rationale: call matching, retries, time, and terminal policy are component concerns; the kernel only needs bounded channel readiness and object-lifetime correctness.
- Rejected alternative: busy-yield retry loops for full terminal queues. They do not prove wake ordering and can starve or deadlock under bounded capacity.
- Decision: coordinate the two test clients through a private endpoint minted by init only for the mutually exclusive call profile.
- Rationale: it forces queue saturation deterministically without adding authority to the generation graph or weakening the broker's fixed bounds.
- Rejected alternative: rely on scheduler timing to fill the queue. The first implementation passed or failed depending on interleaving and did not establish the intended backpressure condition.

## Open risks and follow-ups

- [ ] C8.7 must compose this call lifecycle with operation feedback/result routes without widening cancellation or observation authority.
- [ ] The current call gate uses explicit deterministic time input; physical or wall-clock behavior remains outside C8.6.

## Artifacts and provenance

- Focused report: this entry
- Raw transcript: none retained; exact `just` targets and observed results are recorded above
- Serial/debugger/model output: live serial markers are asserted by `scripts/check/check-fabric-call.py`
- Related roadmap item: `roadmap/02-core-runtime.md` C8.6
