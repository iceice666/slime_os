# C9.2: one block, one badge word, and the map a waiter cannot compute

| Field | Value |
|---|---|
| Date | 2026-08-25 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/wait-set/v1/`, `contracts/generation/v1/{schema.zt,fixtures/sel4-wait-set.zti}`, `contracts/syscall-abi/v1/schema.zt`, `boot-contracts/src/wait_set/`, `boot-contracts/src/generation.rs`, `slime-root/src/{wait_set,generation,ipc,main,notification}.rs`, `components/runtime/src/{wait_set.rs,syscall.rs,syscall/sel4_transport.rs}`, `components/bins/wait-set-probe/`, `components/bins/init/src/main.rs`, `scripts/build/{build-sel4,build-generation}.py`, `scripts/check/{check-sel4-wait-set-plane,check-sel4-boot-layout,check-sel4-gate-controls}.py`, `docs/syscall-abi.md`, `Justfile` |
| Roadmap | C9.2, C9, C9.1, C10.4, RP5, B23, B70, B76 |
| Gates | `just wait_set_check`, `just sel4_gate_control_check`, `just sel4_boot_layout_check`, `just contracts_check`, `just generation_check`, `just test_sel4_root`, `just test_host`, `just component_crate_split_check`, `just lint_all`, `just fmt_check_all` |
| Trigger | C9.2 became the next uncompleted milestone: the backlog is empty, C9.1 closed on 2026-08-24, and RP3/RP4 are deferred on a USB-UART adapter |
| Baseline | C9.1 delivered timer expiry as a signal on a generation-declared Notification with a declared badge. No component had a wait set: a component wanting several sources blocked on one badged notification and then swept its endpoints by hand (`fabric-service`, `call_broker`, `operation_broker`) |

## Summary

A component can now register timer, message, and peer-death sources against one
declared Notification, block once per ready set, recover every ready source from
the coalesced badge word, and dispatch them in a documented deterministic order.
The mechanism is entirely userspace — `slime-root` gains no wait set, no ready
queue, and no source registry — and the one thing the root does add is the half a
peer cannot supply: it signals a declared badge when a task the waiter supervises
terminates, because the peer whose death it reports is the thing that died.

The load-bearing design decision is that the badge-to-source map is *generation
data*. A waiter cannot compute it: `slime-root` derives a signaller's badge from
the **signaller's** declared slot (`1 << (slot % 63)`), which is a fact about the
peer, and C9.1's timer badge is contract data chosen independently of any slot.
Nothing in either table says whether a bit is a stream ingress, a call reply, or a
death. So the alternative to a declared table is compiling peers' slot numbers
into each component, which is exactly the coupling B70 removed.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/wait-set/v1/` | New Zutai contract: per-waiter source entries of `(waiter_identity, badge, notification_grant_identity, source_kind, drain_slot)`, a closed six-kind vocabulary, and the ascending-badge dispatch tie rule stated as a contract promise | Every cross-boundary format is a versioned Zutai schema; the dispatch order is declared rather than emergent |
| `contracts/generation/v1/schema.zt` | `waitSet? : List WaitSetSourceEntry` on the manifest; `WaitSetSourceEntry` names its waiter, notification, badge bit, kind, and optional drain slot | A composition declares what its components may be woken by |
| `boot-contracts/src/wait_set/mod.rs` | Decoder: one-bit badges, closed kinds, drain-slot presence agreeing with the kind, ascending `(waiter, badge)` order, and a per-waiter source ceiling counted across identity boundaries | A malformed source table is refused before a component waits on it |
| `boot-contracts/src/wait_set/dispatch.rs` | The bounded state machine: registration ordered by badge, badge demultiplexing, the ready queue, bounded dispatch. In `boot-contracts` because `slime-rt` has no host build, so tests there would be B23's blind spot | The half that implements the tie rule is reachable by a host gate |
| `scripts/build/build-generation.py` | `validated_wait_set`/`build_wait_set`: every entry must resolve to a badge the generation actually produces — a declared signaller's slot-derived bit, C9.1's timer badge, or the root's supervision badge — with the three producers mutually exclusive | The resource renames facts other tables already fix; it grants nothing |
| `slime-root/src/generation.rs` | `wait_set_admission` re-derives the same three-producer rule from the decoded generation, with `UnsatisfiableWaitSet` | Builder and root cannot disagree about what a badge means |
| `slime-root/src/wait_set.rs` | `WaitSetService`: one row per live task, one minted write-only badged cap per declared supervision source, signalled on termination only when the waiter's own declared slot still holds a `Supervision` capability naming the dead task | Death delivery follows authority the generation already granted; the badge adds a wake, not authority |
| `contracts/syscall-abi/v1/schema.zt`, `docs/syscall-abi.md` | `LIFECYCLE WAIT_SOURCES` (label 49): self-scoped, paged through the transfer window, gated on the one service every instance holds | A component reads its own source table and only its own |
| `components/runtime/src/wait_set.rs` | The syscall shell: read the declared sources, block on the Notification, and re-export the state machine's registration and dispatch | Blocking on your own notification is a property of being a task |
| `contracts/generation/v1/fixtures/sel4-wait-set.zti`, `components/bins/wait-set-probe/` | Generation 42 and its probe: a waiter with stream, timer, and supervision sources on one Notification, a signaller, and an instance the table does not name | The plane exercises all three producers and the deny-by-default answer |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A badge with two producers, or none, is admitted | `just generation_check`, `just contracts_check` | Builder `fail` naming the waiter, or root `UnsatisfiableWaitSet` |
| Dispatch order drifts from the contract's ascending-badge tie rule | `just test_host` (`out_of_order_registration_still_dispatches_ascending`, `re_registering_after_a_retirement_still_dispatches_ascending`) | The two tests fail on the exact sequences that broke it |
| The plane's coalescing evidence becomes vacuous | `just wait_set_check` | The widest single poll is compared against a width derived from the fixture, not from the transcript |
| A marker is deleted, reordered, or a failure marker appears | `just sel4_gate_control_check` | 15 pinned markers, mutation-tested with the other 36 gates |
| The new plane's capability layout drifts | `just sel4_boot_layout_check` | 28 frozen plane layouts |
| A supervision source's minted cap outlives its task | `just test_sel4_root` | Root host tests over the per-task row |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just wait_set_check` | Pass. Generation 42 boots; `SLIME_WAIT sources declared=3 resource=1`; the waiter registers three sources (`mask=0x20208` = bits 3, 9, 17) and refuses a duplicate badge, an undeclared badge, and an over-budget dispatch while staying usable; `SLIME_WAIT death task=4 woken=1`; one poll carries two badges (`wake ready=2 dispatched=2`) dispatched ascending; the timer arrives on a later block (`ready=1`); three sources over two dispatching passes; `wait-set-denied` reads `rows=0` and registers nothing; `SLIME_GRAPH HEALTHY generation=42 required=4 live=0 completed=4 failed=0` | Direct |
| `just sel4_gate_control_check` | Pass: 37 gates reject 1415 mutated transcripts and layouts | Direct |
| `just sel4_boot_layout_check` | Pass: 28 plane layouts match their fixtures | Direct |
| `just generation_check` | Pass: two isolated builds byte-identical; four resealed CPU-budget mutations refused | Direct |
| `just contracts_check` | Pass, including `docs/syscall-abi.md` documents all 36 declared operations | Direct |
| `just test_host` | Pass: 250 boot-contract tests, 22 of them wait-set | Direct |
| `just test_sel4_root` | Pass: 160/160 across 17 modules (was 158) | Direct |
| `just component_crate_split_check` | Pass: 56 component crates, each one package with one binary | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos` | Pass | Direct |
| Two review rounds over the diff | Round 1 found four issues, all applied; round 2 found one incomplete fix, applied. See Decisions | Direct |

## Decisions

- Decision: the badge-to-source map is generation-declared data in a new
  contract, not a component constant and not a root-computed answer.
- Rationale: a waiter cannot derive it. The signaller's badge is a fact about the
  signaller's declared slot, C9.1's timer badge is contract data, and the *kind*
  of a source exists in neither table — nothing distinguishes a stream ingress
  from a peer death when both are bits on one object. The only alternatives are a
  build-time table per component (B70's coupling) or a root-side registry (which
  the C9 architecture decision forbids).
- Rejected alternative: deriving the map in the runtime from the notification
  bindings. It cannot work: a waiter's own bindings say nothing about which slot
  each *peer* signals from.

- Decision: the state machine lives in `boot-contracts`, not in `components/runtime`.
- Rationale: `slime-rt` has no host build — `sel4-alloca`'s inline asm is
  ELF-only, which is why `just test_host` excludes it — so `#[cfg(test)]` there
  would be tests nothing compiles and nothing runs, which is precisely B23. The
  state machine is also the half that most needs testing, since it implements the
  dispatch order the contract defines, so it sits beside the format whose tie rule
  it honours. The runtime keeps only the two operations that touch the kernel.

- Decision: the wait set's tables are fixed at the contract's per-waiter ceiling
  rather than allocated from the C10 private region, deviating from C9's plan.
- Rationale: nine `Source` records plus nine `Ready` records is 216 bytes,
  against the 29960 bytes of `.bss` C10.4 removed from `fabric-service`. A `Vec`
  here would add a `private-heap` feature to every component that waits —
  including the many linking no allocator at all — to save a quarter of a page.
  What C10.4 established is that a worst-case-sized table is worth removing *when
  it is large*.
- Rejected alternative: allocating from the private region as the C9 plan states.

- Decision: no `ReadyQueueFull` error; the ready queue's bound is proven, not
  enforced.
- Rationale: found by review. `register` refuses a duplicate badge, so at most
  `MAX_SOURCES` distinct badges are registered; every queued entry's badge is
  distinct from those already queued; and `MAX_READY == MAX_SOURCES`. So the
  overflow branch was unreachable by construction — a dead guard reading as
  working redundancy, which is the shape B76 removed. The derivation is written
  into `Registry::queue` and a `const` assertion keeps the equality load-bearing.
- Rejected alternative: sizing `MAX_READY` below `MAX_SOURCES` to make the
  ceiling real. That would refuse a wake the kernel is entitled to deliver.

- Decision: the signaller signals *before* its blocking send.
- Rationale: `send` is a rendezvous, so a message-then-signal order deadlocks a
  pair whose receiver is a wait set that will not drain a source it has not been
  told is ready — observed as a real QEMU hang during bring-up. Signalling first
  is safe because a badge is level-triggered readiness rather than a message
  count: the waiter drains until the endpoint would block, so an early signal
  costs one empty drain and never a lost message. This ordering is the protocol
  rule the wait set documents.

## Open risks and follow-ups

- [ ] Only the plane's own probe uses the wait set. The three hand-rolled sweeps
      the milestone's motivation names — `fabric-service`, `call_broker`,
      `operation_broker` — still sweep by hand. Converting them is ordinary work
      now that the mechanism is proven on a boot, but it is a behavioural change
      to three shipped brokers and does not belong in the slice that built the
      mechanism.
- [ ] `call` and `qosEvent` are declared source kinds with no plane coverage: the
      fixture exercises `stream`, `timer`, and `supervision`. The decoder and
      dispatcher treat all six alike and the two uncovered kinds differ only in
      which slot they name, so the gap is in the fixture rather than the
      mechanism — but it is a gap, and a broker conversion above would close it.
- [ ] `components/runtime/src/wait_set.rs` is the one part of this slice no host
      gate compiles, for the B23 reason recorded above. Its logic is a thin
      forward to the tested state machine plus two syscalls, and the plane
      exercises it on a real boot, but a `slime-rt` host build would still be
      worth having and is blocked on `sel4-alloca`.
- [ ] The root's supervision delivery iterates every declared row per
      termination. Bounded by `MAX_TASKS` and negligible at current graph sizes,
      but it is a linear scan on a path that already does several.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none retained; the gate's own assertions and the marker
  contract in `scripts/check/check-sel4-wait-set-plane.py` are the record.
- Serial/debugger/model output: quoted inline under Verification from a
  `just wait_set_check` run.
- Related roadmap item:
  [`roadmap/02-core-runtime.md`](../../roadmap/02-core-runtime.md) C9.2, whose
  dependency C9.1 is
  [`devlog/2026-08-24-c9-1-clock-authority/`](../2026-08-24-c9-1-clock-authority/index.md),
  whose plan is
  [`devlog/2026-08-24-c9-decomposition/`](../2026-08-24-c9-decomposition/index.md),
  and whose consumer is RP5 in
  [`roadmap/09-rpi5-ros2-demo.md`](../../roadmap/09-rpi5-ros2-demo.md).
