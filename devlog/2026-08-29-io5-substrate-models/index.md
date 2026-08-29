# IO5 — checked models of the IO0/IO1 lifetime and accounting rules

| Field | Value |
|---|---|
| Date | 2026-08-29 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/io-queue/model/io-queue.zt`, `contracts/io-resource/model/io-resource.zt`, `just/contracts.just`, `roadmap/11-io-substrate.md` |
| Roadmap | IO5, IO0, IO1, A0 |
| Gates | `just io_queue_model_check`, `just io_resource_model_check`, `just contracts_check` |
| Trigger | Question of whether the IO series should gain independent formal verification, and whether to continue with `zutai model-check` or adopt seL4-style proof |
| Baseline | IO0–IO4 each observed by one QEMU plane gate; no IO property checked over more than one schedule |

## Summary

The IO track's five plane gates observe one interleaving each, while IO0's and
IO1's contracts claim properties quantified over all of them — single-assignment
terminal states, exactly-once lease release, no ring overwrite, no request or
charge surviving an epoch boundary, and total charge return on driver death. A
serial transcript cannot carry a universally quantified claim, so adding QEMU
arms samples more schedules without closing the quantifier. Two bounded Zutai
transition models now close it: `io-queue.zt` (7 scenarios, 2322 states, 2.3 s)
and `io-resource.zt` (8 scenarios, 198 states, 0.3 s), together checking 14
safety properties, 12 reachability obligations, and 2 `leadsTo` liveness rules,
with 13 must-fail mutations. Tooling stayed with `zutai model-check`: the
seL4/Isabelle route does not reach `no_std` Rust userspace at all, and this
build is already outside the verified kernel set by its own config. The models
also produced two real findings rather than only confirming assumptions — IO1's
DMA accounting is per-region, not per-page, and unconditional charge-return
liveness is false without a fairness assumption and should not be claimed.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/io-queue/model/io-queue.zt` | New bounded model of the IO0 lifecycle: one queue, 3 request identities, 2 ring slots, 2 epochs, over submit/take/complete/cancel/drain, begin-reset, reset settlement, peer death, epoch advance | IO0's request/epoch/lease rules hold over every interleaving, not one schedule |
| `contracts/io-resource/model/io-resource.zt` | New bounded model of the IO1 charge lifetime against the `sel4-io-driver` plane's declared budget, over bind/map/charge/raise/ack/fault/reclaim/respawn | IO1's charge conservation and epoch confinement hold over every interleaving |
| `just/contracts.just` | Added `io_queue_model_check`, `io_resource_model_check`, and the `io_model_check` aggregate; registered both in `contracts_check` | A model that stops passing fails the contract gate rather than rotting |
| `roadmap/11-io-substrate.md` | Added the IO5 milestone with its tooling decision, findings, and boundary; added model gates to the track verification stack and one clause to the definition of done | The abstraction gap and the "a model cannot complete a QEMU slice" rule are stated, not implied |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A ring overwrites an unconsumed entry | `just io_queue_model_check` | `RingNeverOverwrites` counterexample |
| A request reaches two terminal states, or a lease releases twice | `just io_queue_model_check` | `SingleTerminalAssignment` / `LeaseReleasedAtMostOnce` counterexample |
| A lease is released while the device still owns the bytes | `just io_queue_model_check` | `LeaseHeldUntilTerminal` counterexample |
| A request survives the epoch that admitted it | `just io_queue_model_check` | `NoLiveRequestAcrossEpoch` counterexample |
| A live request never settles or never releases its lease | `just io_queue_model_check` | `EveryLiveRequestSettlesAndReleases` lasso |
| A driver exceeds its declared MMIO/DMA/IRQ/request budget | `just io_resource_model_check` | `WithinDeclaredBudget` counterexample |
| Driver death leaves a charge outstanding | `just io_resource_model_check` | `DeathReturnsEveryCharge` counterexample |
| Root reclamation runs twice and returns charges twice | `just io_resource_model_check` | `ReclaimRunsAtMostOnce` counterexample |
| A predecessor epoch's acknowledgement is admitted | `just io_resource_model_check` | `StaleAckRefused` counterexample |
| Authority is charged to an instance with no bound device | `just io_resource_model_check` | `NoAuthorityWithoutDevice` counterexample |
| A faulted driver gets stuck holding authority | `just io_resource_model_check` | `FaultedDriverAlwaysReachesZeroCharges` lasso |
| A mutation silently stops exhibiting its violation | both gates | `FAILED (expected violation of "…", none found)`, exit 1 |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just io_queue_model_check` (as `zutai-cli model-check contracts/io-queue/model/io-queue.zt`) | `model-check: all 7 scenarios passed`; main 2322 states, 6039 transitions, 3718 duplicates, max-depth 15, 198 terminals, 0 deadlocks; 2.3 s | Direct |
| `just io_resource_model_check` | `model-check: all 8 scenarios passed`; main 198 states, 667 transitions, 470 duplicates, max-depth 11, 1 terminal, 0 deadlocks; 0.3 s | Direct |
| Negative control 1 — `double-settle` scenario with its fault disabled | exit 1, `FAILED (expected violation of "SingleTerminalAssignment", none found)` over the full 2322-state graph | Direct |
| Negative control 2 — injected `requeue` cycle in `takeOne` | exit 1, `FAILED leadsTo "EveryLiveRequestSettlesAndReleases"` with a deterministic `take`/`requeue` lasso, largest-scc 4 | Direct |
| IO1 per-page DMA charge (first model draft) | exit 1, `FAILED reachability "fully-charged" never reached`, 150 states — the modelling error described below | Direct |
| Unconditional charge-return liveness (first model draft) | exit 1, `FAILED leadsTo` with a two-step `map-dma`/`release-dma` lasso — the property error described below | Direct |
| IO0–IO4 plane gates | Not re-run: this change adds host models and roadmap text and touches no Rust, generated binding, or generation data | Inherited — [`devlog/2026-08-29-b83-root-block-path-deleted/`](../2026-08-29-b83-root-block-path-deleted/index.md) and the IO0/IO1 entries |
| `zutai-cli format --check` on both models | Reports formatting required, as it also does for `contracts/bootstate/model/bootstate.zt` and `contracts/capability-rights/model/capability-rights.zt`; no gate checks model formatting, so the existing convention was left unchanged rather than restyled in this entry | Direct |

## Decisions

- **Decision:** Continue with `zutai model-check` over pure `.zt`; do not adopt
  seL4-style refinement proof for the IO substrate.
- **Rationale:** The seL4 route is unavailable rather than merely costly. Its
  proof chain is Isabelle/HOL from abstract specification to the *kernel's* C,
  scoped by `deps/sel4/CAVEATS.md` to listed platforms and configurations; on
  AArch64-with-hypervisor only integrity holds, with confidentiality and
  non-interference still in progress. This product's kernel build is already
  outside that set by its own admission — `sel4/config/qemu-arm-virt.cmake`
  sets `KernelVerificationBuild OFF` with `KernelDebugBuild`/`KernelPrinting
  ON`, and `qemu-arm-virt` is in no verified-platform list. Decisively, the code
  in question is `no_std` Rust in `slime-root` and `components/`, which no part
  of l4v covers, so "seL4's kind of verification" would mean building an
  Isabelle refinement proof for Rust userspace from scratch. Meanwhile
  `zutai model-check` is already this repository's checked-contract mechanism
  (M5.6a/M5.6b for BootState, A0 for the rights algebra) with the same
  must-fail-mutation discipline, and the measured cost is negligible: 2520
  states across both models in under 3 s, against BootState's 5416 states in
  about 80 s.
- **Rejected alternative 1:** Isabelle/HOL refinement for the Rust substrate.
  First deliverable would arrive long after the RPi5 demo, for properties the
  bounded checker settles in seconds. Rejected on sequencing, not on value.
- **Rejected alternative 2:** Kani bounded model checking of the Rust directly.
  It is already vendored transitively via
  `deps/rust-sel4/hacking/nix/scope/kani/` and would check the implementation
  rather than an abstraction, which is genuinely stronger where it applies. It
  is the wrong *first* step: the interesting IO0 properties are temporal and
  cross-party (`leadsTo` over a driver/client interleaving), which a
  per-function harness does not express, and a shared-memory ring harness would
  need an adversarial peer model — the transition system above under another
  name. Retained as a follow-up target now that a model says what to prove.
- **Rejected alternative 3:** More QEMU arms instead of a model. Each new arm
  samples one more schedule; no finite number of transcripts establishes "no
  reachable sequence", which is what IO0 and IO1 actually claim.
- **Decision:** Model lifetime and accounting only; leave wire layout and
  per-access address checks to the existing codec tests and plane arms.
- **Rationale:** `components/proto/tests/io_queue.rs` and the generated
  validators already decide magics, slot lengths, reserved bytes, absolute
  sequence encoding, and offset/length overflow. Restating them in a model
  would duplicate a checked boundary and obscure which artifact owns it — the
  same reasoning A0 recorded when declining to import all 33 rights names into
  a four-class model.

## Open risks and follow-ups

- [ ] These models are not a refinement proof. That `io_queue_ring.rs` and
  `slime-root/src/io_resource.rs` implement these transition systems is argued
  by construction and pinned by the plane gates, not machine-checked. The
  BootState precedent for closing part of this gap is M5.6c's finite-trace
  conformance check (`scripts/check/check-bootstate-trace.py`), which validates
  real durable traces against the model as an oracle; an equivalent for the IO0
  ring would need a bounded per-request trace the driver already emits, which
  it currently does not.
- [ ] Kani over `Outstanding`'s single-assignment settlement in
  `components/proto/src/io_queue_ring.rs` is now a well-specified target and is
  not attempted here.
- [ ] Bounds are 3 request identities against 2 slots, and 2 DMA pages under 1
  mapping. Both are the smallest values that make the corresponding refusal or
  partial-reclamation case observable, and neither is a proof for larger
  configurations. Unlike A0, no cost curve was measured across bound sizes, so
  the scaling claim is **[INFERENCE]** from the two observed graph sizes.
- [ ] `MIN_QUEUE_SLOTS..=MAX_QUEUE_SLOTS` is 2..=256 in the contract; the model
  fixes 2. The power-of-two and range rules stay owned by
  `admissible_slot_count` and its host tests.
- [ ] IO3/IO4 have no model. Their substrate rules are IO0's and IO1's, which
  these models cover; their own semantics (frame bounds, RX replenishment,
  destination authority) are unmodelled and remain plane-gate evidence only.

## Artifacts and provenance

- Focused report: none; the model sources are the artifact and carry their
  reasoning as header comments.
- Raw transcript: none retained.
- Serial/debugger/model output: model-check verdicts and graph statistics are
  quoted inline in *Verification* above; both gates reproduce them in about
  three seconds.
- Related roadmap item:
  [IO5](../../roadmap/11-io-substrate.md#io5--checked-models-of-the-substrates-lifetime-and-accounting-rules),
  with methodology precedent in
  [A0](../../roadmap/06-authority-trust.md#a0--checked-capability-rights-algebra)
  and [direction 24](../../docs/directions/24-rights-algebra-model.md).
