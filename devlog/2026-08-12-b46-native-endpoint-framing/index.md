# B46 — native endpoint framing must fail closed

| Field | Value |
|---|---|
| Date | 2026-08-12 |
| Kind | Defect |
| Status | Verified |
| Scope | `components/runtime` native Endpoint transport and the seL4 sample plane |
| Roadmap | B46 |
| Gates | `just sel4_sample_check` |
| Trigger | Fresh reviewer pass over the first component-to-component native rendezvous |
| Baseline | The sample loopback exchanged the expected bytes, but the new public transport had not been reviewed against hostile message metadata or out-of-range slots |

## Summary

The first direct Endpoint rendezvous worked, but two transport checks were missing outside the friendly loopback: `native_recv` trusted the sender-controlled label without bounding it by the words the kernel transferred, and endpoint slot arithmetic could cross the 6-bit child CNode and alias a fixed capability. Both now fail with `ERR_INVALID_ARG`; the sample plane still observes a message crossing directly between two threads without the root.

## Observable symptom

- Command: fresh read-only review of the uncommitted B46 native rendezvous diff.
- Expected: malformed framing and impossible declared slots fail closed.
- Observed: a sender could claim more bytes in the message label than `MessageInfo.length()` transferred; `NATIVE_ENDPOINT_BASE + slot` had no runtime range check.
- Exit/fault/serial evidence: the valid loopback passed, so these defects required source-level review rather than the positive scenario alone.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `native_recv` copied the label-sized payload after checking only the destination buffer and the four-register maximum. | A peer could frame registers the kernel did not transfer as a successful payload. |
| 2 | seL4 resolves child CPtrs through a 6-bit CNode guard, while the runtime added the caller's slot without checking the resulting CPtr. | An out-of-range logical slot could alias slot 0 or a fixed service capability instead of producing a lookup failure. |
| 3 | Root installation already checked the same CNode bound in `peer_endpoint::native_slot`. | The component side needed the same fail-closed ABI constraint. |

## Root cause

The first native transport path treated its friendly loopback sender and admitted slot as implicit preconditions. That is not valid for a public peer IPC primitive: message label and requested slot are caller-controlled values, while only the kernel-transferred word count and configured CNode depth are authoritative.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Receive framing | Bound the claimed byte length by `MessageInfo.length() * size_of::<Word>()` before reading any message register | A receiver never exposes bytes the kernel did not transfer |
| Slot resolution | Use checked addition and reject any native CPtr outside the 6-bit child CNode | Invalid declared slots cannot wrap through the guard into fixed authority |
| Runtime buffer branch | Remove a duplicated identical `uses_ambient_buffer` condition in early debug output | Each thread selects exactly one ambient or explicit IPC-buffer path |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Valid native rendezvous no longer reaches both threads | `just sel4_sample_check` | Missing `[sample-worker] native endpoint carried a message` or terminal marker |
| Gate marker count drifts from the causal table | `just sel4_gate_control_check` | Sample-plane required marker count differs from the checker table |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_sample_check` | Passed after both transport checks | Direct |
| `just fmt_check_all` | Passed | Direct |
| `just lint_all` | Passed | Direct |
| Reviewer round 2 | Correct; no remaining introduced defects | Direct |

## Decisions

- Decision: Keep the runtime-side CNode depth literal alongside the root/build-time copies for now.
- Rationale: the component cannot import `slime-root`; disagreement fails closed by refusing a slot the root could install rather than aliasing authority.
- Rejected alternative: Rely on generation admission alone. Public runtime wrappers still accept a numeric slot and must not turn invalid input into a different CPtr.

## Open risks and follow-ups

- [ ] B46 still requires every logical channel caller to move to direct Endpoint/Notification/shared-ring paths before the compatibility mechanism can be deleted.
- [ ] A future wider child CSpace must update the builder, root, and runtime depth agreement together.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none.
- Serial/debugger/model output: `just sel4_sample_check` QEMU transcript in the invoking session.
- Related roadmap item: `roadmap/00-backlog.md` B46.
