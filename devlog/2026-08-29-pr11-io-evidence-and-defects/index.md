# PR #11 review: two slices printed their conclusions, and nine defects sat under them

| Field | Value |
|---|---|
| Date | 2026-08-29 |
| Kind | Defect |
| Status | Verified |
| Scope | `components/proto/src/io_queue_ring.rs`, `components/services/{network-service,virtio-blk-driver,virtio-net-driver}`, `components/lib/src/{block_io,virtio_mmio}.rs`, `components/testkit/io-*`, `slime-root/src/{io_resource,device,peer_endpoint}.rs`, `slime-root/src/graph_runtime/**`, `components/runtime/src/syscall/sel4_transport.rs`, `boot-contracts/src/{network_destination,block_authority,io_resource}.rs`, `contracts/{network-service,block-authority,io-resource,syscall-abi}/v1`, `contracts/generation-manifest/v1/compositions/sel4-io-{block,network}.zti`, `scripts/check/check-sel4-io-*-plane.py`, `scripts/check/check-sel4-gate-controls.py` |
| Roadmap | IO0, IO1, IO2, IO3, IO4, IO5, B85, B86, B88 |
| Gates | `just io_block_check`, `just io_network_check`, `just io_link_check`, `just io_queue_check`, `just io_driver_authority_check`, `just sel4_gate_control_check`, `just test_sel4_root`, `just contracts_check`, `just machete` |
| Trigger | External review of PR #11 (`feat/io-foundation`), 265 files, +29025/-4365, head `3c2a7bc` |
| Baseline | All five IO plane gates green at `3c2a7bc`, and green for the wrong reason in IO2 and IO4 |

## Summary

A review of the native I/O substrate found that in two of eight slices the
evidence layer printed its conclusions instead of reaching them: sixteen
gate-required markers were unconditional string literals asserting behaviour the
planes structurally could not perform, and both plane gates asserted those
literals as merge-blocking causal chains. Underneath them sat nine load-bearing
defects in the trusted root and the drivers, including a lost update on the IO0
ring header, a service-wide epoch bump that destroyed every live capability on
the first receive, a DNS authorisation path that checked no rights bit, and an
MMIO map whose epoch was never transmitted. All are fixed; every fabricated
marker is deleted or replaced by a computed one, and the five IO plane gates now
pass on evidence the components actually produce.

## Observable symptom

- Command: `just io_block_check`, `just io_network_check`
- Expected: markers derived from observed counters and byte comparisons
- Observed: both gates green while the components printed literal arrays
- Exit/fault/serial evidence: `io-block-probe` printed seven fault-injection
  markers plus `restarted old_epoch=1 fresh_epoch=2` from a literal array while
  the driver contained no fault, timeout, cancellation, or crash path and
  `sel4-io-block.zti` declared zero supervision grants; `io-network-probe`
  printed five reset/reclamation markers while `sel4-io-network.zti` declared
  `sharedBufferBudget = [ ]`, no notifications, and no supervision.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | The seven IO2 fault markers are elements of a literal array; the driver has no path that could produce any of them | The gate's `"all injected terminal causes reclaim exactly"` chain asserts fiction |
| 2 | IO2's only submitted opcode is `OP_READ`, and the gate asserts the marker sector is byte-*unchanged* | "write/flush/geometry parity" is affirmatively proved not to have happened |
| 3 | IO4's one real refusal traces to `*epoch += 1` on `OP_RECV`, not to a reset | The gate reads a capability-destroying bug as "link reset" evidence |
| 4 | `authorizes_resolve` matches holder and DNS name only, unlike `authorizes` beside it | The intruder's `send`-only, `dnsRecordLimit = 0` row authorises `OP_RESOLVE` |
| 5 | `put_header` rewrites all 128 bytes while the module doc and the schema's two-cache-line split both declare single-writer ownership | Client `submit` rolls back the driver's `submit_tail`: a re-executed request, not false sharing |
| 6 | `io_mmio_map`'s `epoch` never enters the message, and the root substitutes the table's own | Both sides are theatre; a stale nonzero epoch creates live MMIO authority |
| 7 | `take_region` moves the region out, `map_child` unmaps before mapping, and the `Err` arm drops it | A driver-chosen bad base permanently destroys its own device |
| 8 | After the two-device IO2 plane was built, `OP_WRITE` returned device IO_ERR | Root sorts transports into *reverse* physical order (`platform.rs:121-123`), so ordinal 0 is the first QEMU device, not the second |
| 9 | With ordinals corrected, the second ring's loan was refused `class=absent` | Slot 2 collides with the dynamic shared-buffer namespace; the repo's two-peer convention (`sel4-transfer.zti`) puts peer endpoints at slots 8/9 |

## Root cause

Two independent classes.

The evidence class: a probe marker is a string, and nothing structurally
required it to be derived from an observation. `sel4_gate_control_check` mutates
declared markers to prove a gate fails on missing or reordered evidence, but a
mutation test over literals only proves the checker reads strings. The gates and
the probes agreed with each other while both disagreed with the machine.

The defect class: guard/use splits and single-writer violations in code whose
own comments stated the invariant being broken. `put_header` violated the
ownership rule documented ten lines above it; `begin_request` committed two
charges before two fallible lookups; `DMA_MAP` charged a lease before its last
gate; virtio-net indexed internal state by `request_id % IO_SLOTS` when the IO0
contract requires identities to be unique but not dense.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `io_queue_ring.rs` | Per-field header setters at generated `OFF_HEADER_*` offsets; `advance_epoch` is the sole whole-header writer | No field has two writers |
| `io_queue_ring.rs` | `take_request` returns `TakeRequestError { error, request_id, epoch }` | A refused request is answerable, so its lease releases |
| `network-service` | `OP_RECV` no longer bumps `epoch`; per-holder socket/listener/DNS budgets from the decoded table; identity from `resolve_binding`; listener data path answers `STATUS_UNSUPPORTED`; paged destination read; generated header offsets | A capability survives its own first receive; bounds come from the generation |
| `network_destination.rs` | `authorizes_resolve` requires holder, exact DNS name, `RIGHT_CONNECT`, and nonzero `dns_record_limit` | Resolution is authority, not name matching |
| `virtio-net-driver` | Explicit identity-to-slot association; `valid_link_request` ahead of every opcode; refusal arms settle exactly once; volatile used-entry reads; hardware reset precedes DMA release; unknown lease answers `STATUS_BAD_SLICE` | A client identity indexes nothing; no refusal leaves a live charge |
| `virtio-blk-driver` | Adopts `used_ring_progress`/`used_descriptor_slot`; validates used head and length; status byte mapped through the contract's set | The device cannot desynchronise the cursor or smuggle `0xff` as data |
| `block_io.rs` | Post-admission work moved into one fallible closure with a single unconditional settle | The lease releases exactly once on every exit |
| `slime-root` io_resource | Inventory restored on both map failures; `begin_request` mutations after its last fallible lookup; adapter bounds offset by granted length; per-instance `shared_granule`; `DMA_MAP` resolves the device before charging; `revoke_lease` wired to the production revoke path | A failure commits no charge and destroys no device |
| `MAP_MMIO` ABI | Packed capability slots free a word for the caller's epoch, validated before mapping | The epoch the caller passes is the epoch the root checks |
| `IRQ_WAIT_ACK` | Renamed `IRQ_ACK`; fabricated `interrupt_arrived` deleted | The ABI promises only the wait it performs |
| `block-authority`, `io-resource` | `gen_rust.zt` emits offset constants; decoders and test encoders consume them | B88's remedy reaches its sibling contracts |
| Generators | `generate-{link-device,network-service}-bindings.py` registered in `check-contracts.py` and `contracts_check` | No `@generated` artifact is ungated |
| IO2/IO4 probes and gates | Sixteen fabricated markers deleted; survivors computed; `CHAINS` and gate-control pins shrunk to match | A gate asserts only what a component observed |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A marker becomes a literal again | `just sel4_gate_control_check` | Pinned marker counts (io-block 10, io-network 16, io-driver 16) disagree with the gate |
| The ring header regains a second writer | `cargo test -p slime-proto --test io_queue_ring` | Cache-line ownership and stale-snapshot interleaving tests fail |
| A root charge outlives its refusal | `just test_sel4_root` (211) | `failed_mmio_map_leaves_device_usable`, `failed_queue_request_begin_leaves_mapping_destroyable` |
| Mediated MMIO widens to the granule | `just test_sel4_root` | `mediated_mmio_refuses_in_granule_offset_outside_grant` |
| A generated binding drifts | `just contracts_check` | Either newly registered generator reports stale bindings |
| A dead dependency is propped up | `just machete` | Unused dependency reported |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just io_block_check` | Pass: computed operations, byte readback, async identity, five refusal arms | Direct |
| `just io_network_check` | Pass: exact authority, per-destination budgets, structured denials, honest backend absence | Direct |
| `just io_link_check` | Pass | Direct |
| `just io_queue_check` | Pass | Direct |
| `just io_driver_authority_check` | Pass: mediated MMIO, bounded IRQ authority, ungranted denial | Direct |
| `just sel4_gate_control_check` | 45 gates reject 1748 mutated transcripts and layouts | Direct |
| `just test_sel4_root` | 211/211 across 19 modules | Direct |
| `just test_host` | Pass, including 7 `virtio_mmio` device-boundary tests | Direct |
| `just contracts_check` | Pass, both new generators reached and current | Direct |
| `just sel4_boot_layout_check` | 31 plane layouts match their fixtures | Direct |
| `just fmt_check_all`, `just lint_all`, `just machete`, `just ruff`, `just typos`, `just devlog_check` | Pass | Direct |
| Fresh-boot durability for IO2 | Not run; IO2 no longer claims it | Unsupported |
| `just kani_io_proofs` | 18 harnesses verified | Direct |
| `just kani_virtio_proofs` | 13 harnesses verified | Direct |

## Decisions

- Decision: gate DNS resolution on `RIGHT_CONNECT` plus a nonzero
  `dns_record_limit` rather than minting a resolve right bit.
  Rationale: the manifest already separates the probe (connect, limit 1) from
  the intruder (send-only, limit 0), so the fix is fail-closed with no wire
  change and no new vocabulary.
  Rejected alternative: adding a right bit to
  `contracts/network-destination/v1/schema.zt`, which changes a persisted
  authority format to express what two existing fields already distinguish.

- Decision: carry the `MAP_MMIO` epoch by packing the two u32 capability slots
  into `words[0]`.
  Rationale: AArch64 exposes exactly four fast message registers and `MAP_MMIO`
  used all four; packing two 32-bit slots is lossless and keeps
  `io_resource_request_len` at 4, so the dispatch-time length pin is unchanged.
  Rejected alternative: spilling to the transfer window, which would make the
  hottest driver setup path pay a window round trip for eight bytes.

- Decision: rename `IRQ_WAIT_ACK` to `IRQ_ACK` rather than implement the wait.
  Rationale: the device badge cannot reach the dispatch loop, which special-cases
  only the timer badge; routing it would change boot dispatch architecture. The
  achievable honest ABI is a non-waiting acknowledge.
  Rejected alternative: keeping the name and the fabricated `interrupt_arrived`
  call, which re-armed a level-triggered line unconditionally.

- Decision: reach IO2's missing-right arm with a second virtio-blk instance over
  a read-only disk.
  Rationale: `RING_INDEX` is compiled into the driver as 0, so a second
  authority row on the same device is unreachable without editing the driver,
  and one-driver-per-device is the established multi-disk pattern.
  Rejected alternative: relaxing the driver's ring constant to fit the test.

- Decision: `received_payload_len` takes the minimum frame size as a parameter
  rather than reading `slime_proto::link_device::MIN_FRAME_BYTES` directly.
  Rationale: the first cut imported the constant, which broke CI. Beyond the
  build, it was a layering error: `virtio_mmio.rs` is the virtio-mmio transport
  and a minimum frame size is a link-protocol fact. The import also violated a
  stated invariant — `verification/virtio-proofs` points `[lib] path` at this
  file so verified and shipped source cannot drift, which only holds while the
  file imports nothing but `core`. Making the bound a parameter fixed all
  three, and made the three Kani harnesses stronger: the minimum is now
  symbolic, so they hold for every minimum a link protocol could declare
  rather than only for 60.
  Rejected alternative: adding `slime-proto` to the proof crate's manifest,
  which would make the proof crate diverge from the shipped compilation and
  leave the layering inversion in place.

## Open risks and follow-ups

- [ ] Five of the eight per-destination bounds `boot-contracts` validates —
  `queue_depth`, `byte_budget`, `timer_budget`, `retry_limit`, `reconnect_limit`
  — are still decoded and unenforced. IO4 now claims only the three it enforces.
- [ ] IO4 implements no packet framing. Ethernet, ARP, IPv4, ICMP, UDP, TCP, and
  DNS transport are on the roadmap's explicit not-implemented list.
- [ ] `io-link-loopback` performs zero protocol operations; IO4's backend
  independence is not demonstrated and is no longer claimed.
- [ ] IO2 does not prove fresh-boot durability, and the plane has no supervision
  grant, so restart and stale-completion behaviour remain unproved there.
- [ ] `io-link-intruder` remains capability-less; the link plane's grant
  vocabulary expresses endpoint send/recv rights but no per-operation LinkDevice
  rights, so a rights-caused operation denial cannot be spelled today.

## Artifacts and provenance

- Focused report: `pr-11-review.md` at the repository root, the review this
  entry answers, finding by finding.
- Raw transcript: none retained; every claim above is reproducible from the
  gate commands in Verification.
- Serial/debugger/model output: the IO2 device-ordinal inversion was diagnosed
  from a QEMU serial transcript showing `status=9 dev_status=1` on `OP_WRITE`,
  and resolved against `slime-root/src/graph_runtime/platform.rs:119-123`.
- Related roadmap item: `roadmap/11-io-substrate.md`, IO0 through IO5.
