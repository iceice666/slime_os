# B86/B87 — the virtio-net driver trusted the device's used ring

| Field | Value |
|---|---|
| Date | 2026-08-29 |
| Kind | Defect |
| Status | Verified |
| Scope | `components/services/virtio-net-driver/src/main.rs`, `components/lib/src/{virtio_mmio.rs,lib.rs,build.rs}`, `verification/virtio-proofs/`, `scripts/check/check-sel4-io-link-plane.py`, `just/quality.just`, `.github/workflows/ci.yml` |
| Roadmap | B86, B87, IO3, IO6, IO7 |
| Gates | `just io_link_check`, `just test_host`, `just kani_virtio_proofs`, `just sel4_gate_control_check` |
| Trigger | Asking what to verify next after IO0–IO6 closed: a survey of the substrate's unproved arithmetic asked which inputs no capability can constrain, and the used ring was the answer |
| Baseline | IO3 complete and `just io_link_check` green since 2026-08-28; IO6's eighteen Kani harnesses prove the *client*-facing wire arithmetic of `slime-proto` over all values |

## Summary

The IO3 driver validated everything the client sent and almost nothing the
device wrote. `drain_used` reduced the device-supplied used-ring descriptor id
with `id as usize / 2` and indexed three `[_; IO_SLOTS]` tables with the result,
and `take_used` consumed a ring cell whenever the device's published index
merely differed from the local cursor. Two consequences, of very different
character: an id of 16 or more panicked into `panic = "abort"` — noisy and
supervised — while an *odd, in-range* id, which this driver never publishes
because it submits two-descriptor chains at even heads, resolved to a valid
neighbouring slot and settled a **different client's** lease, request, and
completion with no marker and every gate green. Separately, the receive path
guarded length underflow but not overshoot, then reported the same figure two
incompatible ways: `as u16` into the LinkDevice reply and `u64::from` into the
IO0 completion. Both are fixed by three pure helpers in `components/lib`, with
the device-boundary rules now covered by seven host tests that run off-target,
because QEMU's virtio device is well behaved and therefore cannot exercise them.

## Observable symptom

No failing command existed, which is the finding. The defects are latent under
every available gate.

- Command: `just io_link_check`
- Expected: a gate that fails when the driver mishandles device input
- Observed: PASS, before and after the fix. QEMU's virtio-net device publishes
  only even ids it was given and never overstates a written length, so no plane
  gate distinguishes a validating driver from a trusting one.
- Exit/fault/serial evidence: the pre-fix reset marker read
  `rx drained=4 replenished=4 stalled=0` with no field naming device-rejected
  entries, because none could be rejected.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | IO6 proves `slime-proto` arithmetic over all values; IO5 models lifetimes over all interleavings | Both quantify over inputs reachable from the *client*. Neither covers the device. |
| 2 | `drain_used` computes `let slot = id as usize / 2` from `take_used`'s first tuple field | `id` is read from shared used-ring memory at `ControlQueue::take_used`; the device owns those bytes. |
| 3 | `settle` indexes `request_ids[slot]`, `dma[slot]`, `frame_lengths[slot]`, all `[_; 8]` | `id >= 16` indexes out of bounds. |
| 4 | `submit` publishes `head = slot * 2` only | Odd ids are unpublishable, so an odd id is device error — yet `3 / 2 == 1` is a *valid live slot*. This arm is silent, not fatal. |
| 5 | `take_used` compared `index == used` and consumed otherwise | No bound on how far ahead the device may claim to be; a corrupt index consumes unpublished cells until the cursor catches up. |
| 6 | RX path: `len.saturating_sub(NET_HEADER_BYTES)`, then `transferred as u16` at the reply and `u64::from(transferred)` at the completion | Underflow guarded, overshoot unguarded, and the two reports disagree above `u16::MAX`. |
| 7 | Checked virtio-blk for the same shape: it reads only the used *index* and ignores the id | IO3-specific, not a substrate-wide gap. Scope confined to one crate. |
| 8 | `cargo test -p slime-components` failed to build for the host | `virtio_mmio.rs` mixed `slime_rt` syscall wrappers with pure ring primitives, so none of it was host-testable. This is why the arithmetic had no tests. |

## Root cause

One violated invariant, stated in two places. The driver's per-slot tables are
indexed by *its own* submission slot, and the only authority on which slots are
live is the driver itself; the device's used ring is a report, not a source of
truth. `id as usize / 2` treated a device report as an index into driver state,
conflating "what the device said" with "what I published". The parity check is
the load-bearing half: bounds alone would convert the silent aliasing arm into
the loud panic arm, but not eliminate it, because odd ids land in range.

The secondary cause is structural and explains the absence of tests.
`components/lib/src/virtio_mmio.rs` placed `MediatedMmio` — whose every accessor
is an `slime_rt` syscall — beside pure functions over `&[u8]`. That made the
whole module bare-metal-only, so the arithmetic could only ever be exercised
through a booted QEMU plane driven by a *correct* device. The untestable seam
is why the trust boundary went unexamined, not merely why it went untested.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `components/lib/src/virtio_mmio.rs` | New `used_descriptor_slot(id, slots)`: admits only even ids below `slots * 2` | A used id resolves to a slot only if this driver could have published it |
| `components/lib/src/virtio_mmio.rs` | New `used_ring_progress(published, consumed, distance)`: `wrapping_sub` bounded by outstanding chains | The device may run ahead only as far as the entries actually in flight; wrapping stays progress, overshoot is refused |
| `components/lib/src/virtio_mmio.rs` | New `received_payload_len(reported, header, frame_len) -> Option<u16>` | A receive length includes the header and fits the published descriptor; the `u16` return makes reply and completion the same value by construction |
| `components/lib/src/virtio_mmio.rs` | `MediatedMmio`/`MediatedHandshake` gated behind `component-runtime` | The pure ring primitives compile — and are testable — on the host |
| `components/lib/src/lib.rs` | `fabric_self_view` gated behind `component-runtime` (it calls `slime_rt`) | The crate's default-feature-off build is honest |
| `virtio-net-driver` `take_used` | Returns `Result<Option<_>, ()>`; refuses an unpublishable used index | An empty ring and a lying device are no longer the same answer |
| `virtio-net-driver` `drain_used` | Validates id and RX length; counts refusals; new `outstanding_chains` bound | Device input is refused before it indexes driver state |
| `virtio-net-driver` `settle` | Takes `Option<u16>`; drops the `as u16` cast | Reply `frame_len` and completion `transferred` cannot disagree |
| `virtio-net-driver` | New `tx_stalled` counter | A transmit stall no longer increments `rx_stalled` — a pre-existing miscount found in passing |
| `scripts/check/check-sel4-io-link-plane.py` | Reset marker asserts `tx-stalled=0 device-refused=0` | The plane proves the happy path refuses nothing, so a refusal becomes visible |
| `just/quality.just` | `test_host` runs `slime-components --no-default-features` | The device-boundary rules are checked by CI |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A device id is trusted again | `just test_host` | `an_odd_used_id_is_refused_rather_than_aliased_onto_a_live_neighbour` fails on `Some(1)` |
| Out-of-range id indexes a table | `just test_host` | `an_out_of_range_used_id_is_refused_before_it_indexes_a_table` |
| Used index trusted beyond outstanding work | `just test_host` | `the_used_index_may_run_ahead_only_as_far_as_the_outstanding_entries` |
| Wrapping mistaken for device error | `just test_host` | `a_wrapping_used_index_is_progress_not_a_device_error` |
| RX length truncated to `u16` again | `just test_host` | `a_receive_length_past_the_offered_frame_is_refused_not_truncated` |
| The driver starts refusing valid traffic | `just io_link_check` | `device-refused=0` stops matching |
| The new marker becomes unassertable | `just sel4_gate_control_check` | A mutated transcript is accepted |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just io_link_check` | PASS — `duplex readiness, replenishment, reset, restart, and authority proved`, with the amended `tx-stalled=0 device-refused=0` marker | Direct |
| Control: marker mutated to `device-refused=7` | Gate exit 1 — `missing marker: … device-refused=7`, proving the new assertion is load-bearing | Direct |
| `just test_host` | PASS — 20 suites; the 7 new `virtio_mmio::tests` run and pass | Direct |
| `just io_block_check` | PASS — `oracle parity, async identity, faults, reclamation, and stale epoch proved`; confirms the feature gating did not disturb the shared virtio helpers | Direct |
| `just io_queue_check` | PASS — `round trip, backpressure, late completion, reset epoch, and slice refusal proved` | Direct |
| `just sel4_gate_control_check` | PASS — 45 gates reject 1768 mutated transcripts and layouts | Direct |
| `just fmt_check_all` | PASS after running `cargo fmt` on the two touched crates | Direct |
| `just lint_all` | PASS with warnings denied | Direct |
| `just machete` | PASS — no unused dependencies in `boot-contracts`, `components`, `slime-root` | Direct |

## Decisions

- **Decision:** Put the three rules in `components/lib/src/virtio_mmio.rs` as
  pure functions, not inline in the driver.
  **Rationale:** Inline `if` statements in a `no_std`/`no_main` binary are
  reachable only through a booted plane driven by a correct device — which is
  precisely how these defects survived. Pure functions over scalars are
  host-testable and, being total over their input types, are ready-made Kani
  targets.
  **Rejected alternative:** validating inside `settle`. It already holds
  `&mut LinkQueue` and would have mixed refusal with settlement, and it cannot
  refuse before the id has already indexed `request_ids[slot]`.
- **Decision:** Refuse an over-run used index rather than clamping it.
  **Rationale:** Clamping would consume *some* cell and settle *some* request,
  which is the aliasing failure with a different arithmetic. A device
  contradicting what this driver published is a device error and is reported.
  **Rejected alternative:** `fail()` on it. A malfunctioning NIC should not take
  down a driver that can decline the entry and keep serving; the counter makes
  the condition visible instead.
- **Decision:** Report refusals in the existing reset marker and assert zero on
  the happy path, rather than adding a new adversarial plane arm.
  **Rationale:** The plane's device is QEMU's; nothing in-tree can make it lie,
  so an adversarial arm would need a fake device. Asserting `device-refused=0`
  proves the validation refuses nothing legitimate, and the host tests own the
  adversarial cases. Claiming an observed adversarial device would be false.
  **Rejected alternative:** an `io-link-intruder` arm — that component is a
  *client*, and no client can forge a used-ring entry. The threat model is the
  device, which is exactly why no client-side gate reaches this code.
- **Decision:** Gate `MediatedMmio` and `fabric_self_view` behind
  `component-runtime` instead of adding a mock syscall layer.
  **Rationale:** The feature already exists and already means "links against
  `slime_rt`". Two modules were simply on the wrong side of it. A mock layer
  would be new scaffolding testing itself.

## Open risks and follow-ups

- [ ] These are host tests over fixed inputs, not proofs. The three helpers are
  total functions over `u16`/`u32`/`usize` and are the natural first IO7 Kani
  targets: `used_descriptor_slot` over all `u32`, `received_payload_len` over
  all `u32 × u16`, `used_ring_progress` over all `u16³`. Nothing here quantifies
  over every value yet.
- [ ] No adversarial *device* exists in this repository, so the refusal arms are
  observed only in host tests, never on a booted plane. A fake virtio backend
  that publishes odd ids and overstated lengths would close that gap and is
  unwritten; the plane asserts only that valid traffic is refused nothing.
- [ ] The device-boundary audit covered IO3's used ring. Not examined for the
  same shape: `slime-root/src/io_resource.rs`'s unchecked charge commits, whose
  preflight guards sit in different functions, and
  `boot-contracts/src/network_destination.rs`, a hand-written byte parser where
  the repository's Zutai rule requires generated code. Both are recorded in the
  survey, neither is addressed here.
- [ ] `pass_tx`/`pass_rx` coalescing counters remain `u32` and are reported but
  unbounded; not a defect, unexamined.

## Artifacts and provenance

- Focused report: none; the rules and their rationale are the doc comments on
  the three helpers in `components/lib/src/virtio_mmio.rs`.
- Raw transcript: none retained; `just io_link_check` reproduces the plane's
  serial evidence and the negative control is one `sed` on the marker.
- Serial/debugger/model output: the marker text under *Verification*, as emitted
  by `scripts/check/check-sel4-io-link-plane.py`.
- Related roadmap item: [IO3 — Userspace virtio-net and LinkDevice validation](../../roadmap/11-io-substrate.md#io3--userspace-virtio-net-and-linkdevice-validation), with the resolved entries at [B86 and B87](../../roadmap/00-backlog.md#resolved).

## Corrections

**2026-08-29 — the first follow-up is closed; IO7 landed the same day.** The
*Open risks and follow-ups* item above ("host tests over fixed inputs, not
proofs") named the three helpers as the natural first Kani targets. That is now
done and recorded as [IO7](../../roadmap/11-io-substrate.md): thirteen harnesses
over the shipped `components/lib/src/virtio_mmio.rs`, guarded by
`just kani_virtio_proofs`, wired into the existing `kani_proofs` CI job.

Nothing in the frozen body above is retracted — the host tests still exist and
still run under `just test_host`. The proofs are a stronger layer over the same
three functions, not a replacement, and the plane gate remains what proves the
driver actually calls them.

Observed:

| Command/scenario | Result | Evidence class |
|---|---|---|
| `nix develop .#kani --command just kani_virtio_proofs` | PASS — `13 successfully verified harnesses, 0 failures` | Direct |
| 8 source mutations, one per rule | Each produces a counterexample in the matching harness | Direct |
| Mutation: `wrapping_sub` to `saturating_sub` in `used_ring_progress` | Caught by `progress_is_exact_modular_distance_or_a_refusal`; every value stays in range, so only the exactness harness sees it | Direct |
| Mutation: drop the frame-fit check in `received_payload_len` | Caught by `an_accepted_receive_length_is_exact_never_truncated` — the assertion B87's `as u16` cast violated | Direct |
| Control: proof module disabled (`#[cfg(any())]`) | Gate exit 1 | Direct |
| Control: one `#[kani::proof]` attribute deleted | Gate exit 1 via count assertion, while Kani reported `VERIFICATION:- SUCCESSFUL` | Direct |
| `just lint_all`, `just fmt_check_all`, `just test_host`, `just io_link_check` | PASS after the change | Direct |

Two things the work itself exposed, both recorded rather than smoothed over:

- The `expected=` count assertion first read `14`, because I miscounted the
  harnesses I had written. The gate failed with `expected 14 harnesses, ran 13`
  before any proof was trusted — the assertion earning its place on its first
  run, in the direction that matters.
- `slime-components` had no `build.rs`, so `#[cfg(kani)]` broke `just lint_all`
  under `-D warnings` (`unexpected_cfgs`) even though no product build sets the
  cfg. The proof crate's own `build.rs` does not cover the package under proof.
  Fixed by `components/lib/build.rs`, mirroring `components/proto/build.rs`.

Still open from the original list: no adversarial device exists in-tree, so the
refusal arms remain proved over all values but never observed on a booted
plane; and `slime-root/src/io_resource.rs`'s charge accounting is unaddressed.

**2026-08-29, same day —** the third item,
`boot-contracts/src/network_destination.rs`, is closed as
[B88](../../roadmap/00-backlog.md#resolved): its Zutai-declared layout is now
generated as `OFF_*` constants rather than restated as byte literals, and the
module went from 3 positive-path tests to 9. **Evidence:**
[`devlog/2026-08-29-b88-network-destination-generated-offsets/`](../2026-08-29-b88-network-destination-generated-offsets/index.md).
