# IO0: the shared queue, identity, and buffer-lease substrate

| Field | Value |
|---|---|
| Date | 2026-08-28 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/io-queue/v1/`, `components/proto/src/{io_queue.rs,io_queue_ring.rs,lib.rs}`, `components/proto/tests/{io_queue.rs,io_queue_ring.rs}`, `components/testkit/io-queue-{client,driver}/`, `contracts/generation-manifest/v1/compositions/sel4-io-queue.zti`, `scripts/build/build-{generation,sel4}.py`, `scripts/check/check-sel4-io-queue-plane.py`, `scripts/check/check-sel4-gate-controls.py`, `scripts/check/check-contracts.py`, `scripts/generate/generate-io-queue-bindings.py`, `just/{generate,planes-mechanism}.just` |
| Roadmap | IO0 |
| Gates | `just io_queue_check`, `just sel4_gate_control_check` |
| Trigger | Opening the Native I/O substrate track: every later slice needed one asynchronous request/completion contract before any driver could be written against it. |
| Baseline | C7 shared buffers and loans, C9.2 Notification-backed WaitSets, C9.4 supervised restart, and the root-owned single-outstanding virtio-blk path. No asynchronous request identity, driver epoch, or buffer-slice descriptor existed anywhere. |

## Summary

The I/O track's load-bearing rule is *share I/O mechanisms, not device semantics*, and IO0 is where that rule becomes a format. This lands one versioned Zutai contract for the request/completion queue envelope, the buffer-slice descriptor, and the `RequestId`/`DriverEpoch` identity pair; one `no_std` cursor-and-lease library over it; and one QEMU plane in which two supervised components exchange protocol-specific work through fixed shared rings and prove the substrate's refusals. Nothing device-specific enters the envelope: the block LBA, the Ethernet frame, and the USB setup packet each stay in their own protocol schema and travel as opaque payload bytes.

The slice is complete and observed: `just io_queue_check` boots the plane and the transcript satisfies every ordered causal chain, including numeric lease-reclamation counts.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/io-queue/v1/schema.zt` | One queue header, one standalone `BufferSlice`, one 128-byte request slot, one 64-byte completion slot; lifecycle, status, direction, flag, and badge vocabularies pinned as constants | A driver and its client agree about capacity, epoch, and slot shape in one atomically-readable place, and every logical field has exactly one writer |
| `contracts/io-queue/v1/gen_rust.zt` | Reflects all four records, checks field order against layout, checks exact width sums, checks that the request's inlined slice agrees field-for-field with the standalone record, and checks the header's two single-writer halves land on separate cache lines | A standalone slice validator cannot become a lie, and false sharing on the substrate's hottest words cannot be introduced silently |
| `components/proto/src/io_queue_ring.rs` | `Queue` (attach/format/submit/take_request/complete/take_completion/begin_reset/advance_epoch/mark_driver_dead) plus `Outstanding<N>` (admit/find/start/settle/settle_all/adopt_epoch) | Cursor discipline is written once rather than in each driver, and terminal settlement is single-assignment because it *removes* the entry and returns the lease it retained |
| `components/proto/src/lib.rs` | `valid_queue_header`, `valid_buffer_slice`, `valid_request_slot`, `valid_completion_slot`, `valid_completion_status`, `queue_slot_index`, `valid_queue_badge`, `terminal_request_state`, `terminal_state_for_status` | Every field a peer wrote is refused before use; the bounds come from the reader's own provisioning record, never from the mapping |
| `components/testkit/io-queue-{client,driver}` | Two supervised components exchanging an echo token through one shared mapping over a WaitSet | IO0's exit condition is observed rather than asserted |
| Build/gate plumbing | `sel4-io-queue` composition and manifest entry, `io-queue` image variant and `--io-queue-plane` flag, plane checker, `GATES` registration at 15 markers, `io_queue_gen`/`io_queue_check` targets, `check-contracts.py` drift check | The contract cannot go stale and the gate cannot pass on missing, reordered, or failing evidence |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A schema edit relaxes a structural refusal | `cargo test -p slime-proto --test io_queue` (35 tests) | A named refusal test accepts the malformed value |
| A cursor or lease bug leaks a request or double-releases a lease | `cargo test -p slime-proto --test io_queue_ring` (19 tests) | `settle`/`settle_all` counts disagree, or a second settlement succeeds |
| Generated bindings drift from the contract | `just contracts_check` (runs the generator `--check`) | `generated io-queue bindings are stale; run just io_queue_gen` |
| The substrate stops working end to end | `just io_queue_check` | A missing or out-of-order causal marker |
| The gate could pass on absent evidence | `just sel4_gate_control_check` | Baseline accepted but a mutated transcript also accepted |
| A header edit reintroduces false sharing | `the_header_places_its_two_writers_on_separate_cache_lines` plus the generator's `headerHalvesSplit` predicate | A client-written field lands at offset >= 64 or the layout sum is not 2x64 |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just io_queue_check` | PASS — built `build/slime-sel4-io-queue.elf`, booted under QEMU, ended `seL4 I/O queue plane check: round trip, backpressure, late completion, reset epoch, and slice refusal proved` | Direct |
| `just sel4_gate_control_check` | PASS — accepted the registered `sel4_io_queue_plane` checker at exact marker count 15 | Direct |
| `cargo test -p slime-proto --test io_queue` | PASS — 35 passed, 0 failed | Direct |
| `cargo test -p slime-proto --test io_queue_ring` | PASS — 19 passed, 0 failed | Direct |
| `cargo test -p slime-proto` | PASS — every protocol suite, no regression in the existing fabric/spawn/store/trace tests | Direct |
| `python3 scripts/generate/generate-io-queue-bindings.py --check` | PASS — `I/O queue protocol bindings are current` | Direct |
| `just component_crate_split_check` | PASS — the two new testkit crates satisfy naming, single-binary, build-script, and release-profile rules | Direct |
| Runtime markers observed | `round trip drained=4 echoed=4`; `backpressure full refused overwrite=0`; `unknown completion refused`; `reset settled=2 leases=2`; `fresh epoch observed old epoch refused`; `malformed slice refused before submission`; `SLIME_GRAPH loans served=2 loans=0 mappings=0 regions=0 orphans=0 quota=0` | Direct |

## Decisions

- **Decision:** One generic envelope carrying an opaque protocol payload, rather than one union opcode covering block, net, and USB.
  **Rationale:** The roadmap's boundary is explicit that `BlockRequest`, `PacketTx`, and `UsbTransfer` must never become variants of one `IoOpcode`. Generic identity, epoch, lease, and lifecycle are genuinely shared; operation meaning is not.
  **Rejected alternative:** A discriminated `IoOpcode` union, which would have put every device's vocabulary in one schema and made adding a device class a change to every consumer.

- **Decision:** The queue header is 128 bytes in two single-writer halves, with explicit padding fields, and the generator asserts the split.
  **Rationale:** A 64-byte header puts `submit_head` and `complete_head` on one cache line, so each side's position update invalidates the other's copy on every single request. False sharing is a performance fault, so no functional test would ever catch its reintroduction — which is precisely why it is checked in the generator and in a unit test rather than merely documented.
  **Rejected alternative:** One 64-byte line with a comment asking future editors to keep the fields apart.

- **Decision:** `Outstanding::settle` removes the entry and returns the released lease, rather than marking a terminal state in place.
  **Rationale:** It makes "every terminal transition is single-assignment and every lease releases exactly once" structural. A second settlement of the same identity finds nothing and is refused — which is exactly the answer a late completion after cancellation, reset, or peer death must receive.
  **Rejected alternative:** An in-place `state` field, where a double-settle is a bug each driver must remember not to write.

- **Decision:** `terminal_state_for_status` is total over defined statuses, and an unknown status yields `None`.
  **Rationale:** "Every submitted request reaches one terminal state" is only checkable if no defined status is unclassified. A partial table would silently treat a new status as harmless.
  **Rejected alternative:** A default arm mapping unknown statuses to `STATE_COMPLETE`.

- **Decision:** A device-level failure is substrate status `STATUS_DEVICE_ERROR` with its detail in the owning protocol's completion payload.
  **Rationale:** The substrate has no vocabulary for what went wrong inside a device it does not parse, and inventing one would pull device semantics into `slime-root`'s contract surface.
  **Rejected alternative:** Substrate statuses per failure class (bad sector, link down, endpoint stalled), which is the universal-device-interface mistake in a different costume.

- **Decision:** `mapped_len` is supplied by the validating side, never read from the wire, and `offset + length` is checked with `checked_add`.
  **Rationale:** A slice is a claim about which bytes an operation touches; the only check that matters is against the extent the *validator* knows the lease covers. Overflow is where a hostile descriptor aims, since a wrapped sum passes a naive bound test.
  **Rejected alternative:** Trusting a length carried alongside the slice.

## Open risks and follow-ups

- [ ] The queue library is exercised by host tests and one plane; the first real driver (IO2 virtio-blk) is what will show whether the 56-byte request payload is the right size in practice. If it is not, that is a `contracts/io-queue/v2`, not an in-place widening.
- [ ] `FLAG_FENCED` is defined and validated but no consumer orders on it yet; the first driver with genuinely dependent requests owns proving it.
- [ ] `SLOT_CLAIMED` is refused everywhere but never written, because every slot body is a single `encode` copy. It stays in the contract for a future writer that streams a payload in pieces, and `valid_*_slot` must keep refusing it either way.
- [ ] `mark_driver_dead` is the seam the root writes during reclamation. IO1's plane work owns wiring it to actual supervision-driven task death.

## Artifacts and provenance

- Focused report: none; the contract comments in `contracts/io-queue/v1/schema.zt` and the module docs in `components/proto/src/io_queue_ring.rs` carry the design rationale.
- Raw transcript: none retained; the plane's serial evidence is reproduced by `just io_queue_check`.
- Serial/debugger/model output: the marker list under *Verification*, as emitted by `scripts/check/check-sel4-io-queue-plane.py`.
- Related roadmap item: [IO0 — Queue, identity, and buffer-lease contract](../../roadmap/11-io-substrate.md#io0--queue-identity-and-buffer-lease-contract)
