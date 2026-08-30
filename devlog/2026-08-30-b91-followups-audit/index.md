# B91 follow-ups: two proposed gates measured, refuted, and replaced by one

| Field | Value |
|---|---|
| Date | 2026-08-30 |
| Kind | Audit |
| Status | Verified |
| Scope | `scripts/check/check-slot-pin-reasons.py`, `contracts/generation-manifest/v1/compositions/sel4-io-driver-authority.zti` (mutated and restored), `just contracts_check`, `just io_driver_authority_check` |
| Roadmap | B91 |
| Gates | `just contracts_check`, `just io_driver_authority_check` |
| Trigger | The six follow-ups recorded in `devlog/2026-08-30-b91-slot-pin-reasons/index.md`; three were classified as small self-contained gate work |
| Baseline | 611 pinned slots labelled and gated; the minimality clause reaches 4 of 260 `componentAbi` pins; five pins removed with no assertion recording that they were once pinned |

## Summary

Three of B91's follow-ups were meant to be cheap gate work: scope
`compiles_slot` to a code path, add a host gate against slot permutation, and
pin the five removed pins into their owning gates. Measuring each before
building it refuted the first two. Function-scoping the predicate flagged four
pins that are all genuinely load-bearing, because `io-driver-probe` resolves
those grants by name in its supervisor branch and forwards them *in array order*
to `spawn`, which is exactly how they become the worker's positional slots — the
two halves of the evidence are supposed to sit in different functions. The
permutation gate was unnecessary: the QEMU plane gate already catches a
permutation, which this entry observed directly rather than inferring, and the
one host invariant that looked like a substitute does not hold on the clean tree.
The third follow-up landed: five `MIGRATED_PINS` assertions, three
fail-closed controls, and the gate's own docstring corrected where it described
the first limit with a wrong example.

## Observable symptom

- Command: `python3 scripts/check/check-slot-pin-reasons.py` after scoping the
  minimality clause's two claims to a single top-level function.
- Expected: no change, or a flagged pin that is genuinely migratable. The
  predecessor entry recorded all four suppressed pins as "the worker's, so
  today's labels are right".
- Observed: four pins flagged —
  `io-driver-worker/probe-{device,mmio,irq,dma}` in
  `sel4-io-driver-authority.zti`.
- Exit/fault/serial evidence: gate exit 1. Joint removal of those four pins
  moves `probe-mmio` from slot 1 to 3 and `probe-dma` from 3 to 1, against
  `run_driver`'s `REGION_SLOT = 1` and `DMA_SLOT = 3`, so all four flags are
  false positives and the scoping was reverted.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | 19 executables run as more than one instance in a single manifest; only `io-driver-probe` holds `componentAbi` pins the clause currently suppresses | The role-awareness follow-up has exactly one subject, so it can be judged by inspection rather than by a heuristic |
| 2 | Splitting the crate blob per top-level `fn`: `run_supervisor` resolves all four `probe-*` names and compiles no slot constant; `run_driver` compiles all four constants and resolves no name | The two claims are disjoint per function, so requiring their conjunction in one region flags all four pins |
| 3 | Joint removal of the four pins yields `{device: 0, mmio: 3, irq: 2, dma: 1}` against a declared `{0, 1, 2, 3}` | The pins are load-bearing; the flags are false positives, not discoveries |
| 4 | `run_supervisor` passes the four resolved slots positionally into a `grant` array handed to `spawn` | Root cause: spawn forwarding *legitimately* splits name-resolution and positional consumption across functions and across instances. The crate-wide predicate is the one that survives this shape |
| 5 | Permuting `probe-device` and `probe-mmio` in the composition: pin-reason gate exit 0, `check-boot-layout-resource.py` exit 0, resolved-slot table unchanged in shape | Confirms the documented gap is real and no existing host gate closes it |
| 6 | `just io_driver_authority_check` on the same permutation fails with `seL4 I/O driver authority plane check: failure marker in serial transcript: '[io-driver-probe] fail: '` | The predecessor entry's claim that QEMU catches it is now *observed*, not inferred. A new host gate would duplicate a working guard |
| 7 | Joint-removal equality — the obvious host substitute — fails on the **clean** tree too (`{0,1,2,3}` declared vs `{0,3,2,1}` recomputed) | Refutes the one candidate host invariant: the allocator refills freed pins lowest-first and reorders the group, so equality is not the property |
| 8 | Grant-name-to-constant-name token matching over all 260 `componentAbi` pins: 155 unique matches, 3 mismatches on the clean tree (`BACKUP_ROUTE_SLOT` vs `fabric-op-client-backup`, and two more) | Refutes name correspondence as a gating predicate: it has false positives on known-good source, so it cannot fail the build |
| 9 | `resolved_slot_table` keys are `(instance, namespace, grant)` triples, and `sel4-channel.zti` declares no `spawn-service` instance | Corrected three wrong entries in the first draft of `MIGRATED_PINS`; the recorded slots are now read from the production allocator rather than recalled |

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `scripts/check/check-slot-pin-reasons.py` | `MIGRATED_PINS`: the five pins deleted under B91, each with the slot the production allocator reproduces, asserted through `BUILDER.resolved_slot_table` | The five removals are guarded by a named assertion instead of by a session observation |
| `scripts/check/check-slot-pin-reasons.py` | Every `MIGRATED_PINS` entry must be reached, and a re-pinned migrated binding fails with a message naming why it was removed | A rename cannot silently retire the assertion, and a re-pin cannot pass unlabelled |
| `scripts/check/check-slot-pin-reasons.py` docstring | First limit rewritten: names the spawn-forwarding mechanism, records that function scoping was tried and reverted, and states what a real tightening needs (the grant's ordinal in the `spawn` array). Second limit records the observed QEMU failure and the refuted joint-removal substitute | The documented limits describe measured behavior, and two dead ends are recorded so they are not re-attempted |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `python3 scripts/check/check-slot-pin-reasons.py` | Passed — 611 pins, 152/12/187/260, 2 exempt, and `formerly pinned, now allocator-reproduced at their observed slots: 5` | Direct |
| Control: re-pin `network-service/network-service-link-device` at its own resolved slot 2 | Refused — `is pinned again to slot 2. It was removed because the holder resolves it by name and compiles no such position` | Direct |
| Control: re-pin `network-service/network-intruder-service` | Refused | Direct |
| Control: rename `network-intruder-service` throughout the composition | Refused — `MIGRATED_PINS names bindings that no longer exist` | Direct |
| Clean tree after every mutation restored | Gate green, `git diff` empty for the mutated composition | Direct |
| Permutation of `probe-device`/`probe-mmio`, host gates | Pin-reason gate and `check-boot-layout-resource.py` both exit 0 — the documented gap reproduced | Direct |
| Permutation of `probe-device`/`probe-mmio`, `just io_driver_authority_check` | Failed in QEMU with `[io-driver-probe] fail: ` | Direct |
| `just contracts_check` | Passed | Direct |
| `ruff check scripts/` | Passed | Direct |

## Open risks and follow-ups

- [ ] Tightening `compiles_slot` needs the grant's **ordinal in the `spawn`
      grant array**, not its enclosing function. That is a real analysis — it
      must follow a resolved slot into an array literal and match the index
      against the child instance's binding order — and it would judge one
      executable today. Recorded rather than attempted.
- [ ] The permutation gap stays closed by QEMU only. Every composition holding
      positionally-consumed pins has a plane gate today, but nothing enforces
      that pairing: a future composition could carry `componentAbi` pins with no
      plane gate and no host check of which number each pin holds. A gate
      asserting "every manifest with `componentAbi` pins is named by some plane
      check" would close that, and is not written.
- [ ] Eleven compositions have no `.layout` fixture, including
      `sel4-io-driver-authority`, `sel4-io-network`, and `sel4-io-block`. Their
      resolved slots are frozen only by the pin labels and their plane gates.
      Blessing layout fixtures for them would make slot drift visible on the
      host; it is a separate change with 11 new fixtures.
- [ ] The three broad B91 audits are untouched: the 254 unexamined
      `componentAbi` pins, the 187 `encodedLayout` pins per composition, and
      reason semantics for the 83 minted and 322 notification bindings.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none; every command result above was observed directly in the
  implementation session.
- Serial/debugger/model output: the permutation's QEMU failure marker is quoted
  in the Investigation log and Verification tables. No transcript file was
  retained, because the mutation was reverted and the plane is green.
- Related roadmap item: [B91 in `roadmap/00-backlog.md`](../../roadmap/00-backlog.md),
  resolved; predecessor
  [`devlog/2026-08-30-b91-slot-pin-reasons/`](../2026-08-30-b91-slot-pin-reasons/index.md).
