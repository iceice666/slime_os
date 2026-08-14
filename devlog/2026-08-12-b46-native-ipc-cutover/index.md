# B46's native IPC cutover, and the slot namespaces it exposed

| Field | Value |
|---|---|
| Date | 2026-08-12 |
| Kind | Change |
| Status | Monitoring |
| Scope | `slime-root/src/{main,shared_buffer,buffer_adapter,object_allocator,graph,notification,peer_endpoint}.rs`, `components/runtime/src/syscall{,/sel4_transport}.rs`, `components/bins/src/bin/fabric-*.rs`, `components/bins/src/bin/init.rs`, `contracts/generation/v1/fixtures/sel4-stream.zti`, `contracts/generation/v1/fixtures/valid.zti`, `scripts/build/build-generation.py` |
| Roadmap | B46, B50 |
| Gates | `just sel4_channel_check`, `just sel4_crossing_check`, `just test_sel4_root`, `just lint_all`, `just fmt_check_all` |
| Trigger | `c8fc792`, deleting `channel.rs`, `transit.rs`, and `parked.rs` |
| Baseline | All seven B46 plane gates green on logical channels at 2026-08-10 |

## Summary

The logical channel mechanism is deleted and component-to-component messages
cross declared seL4 Endpoints directly, with buffered streams on a v2 shared
ring. Two of the seven named gates pass on the new path; five do not, so B46
stays open by its own exit condition. Running the cutover surfaced four defects
that made the delegated-ring design unrunnable — none of them in the ring — plus
two outside the fabric, and one open allocator-integrity defect. The dominant
defect class was not the IPC model at all: six consecutive failures were
hand-written slot numbers disagreeing with other hand-written slot numbers,
which is now scoped under B50 as four distinct namespaces.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `slime-root/src/main.rs` | `serve_capability_export` no longer requires a kernel endpoint; `ticket` is `Option<CPtrBits>` | A logical capability can be delegated at all |
| `components/runtime/src/syscall/sel4_transport.rs` | Delegation stops writing the export id over descriptor bytes 8..12 | `status` still distinguishes a grant from a denial |
| `components/runtime/src/syscall/sel4_transport.rs` | An empty `nb_recv` is identified by zero words and zero capabilities, not by a zero label | A poll that finds nothing is not read as a malformed payload |
| `slime-root/src/shared_buffer.rs` | `Loan`/`LoanHandle` record `writable`; `map_loan` maps at the loan's protection | A ring's two peers can write disjoint header fields; a sample loan stays read-only |
| `slime-root/src/main.rs` | `serve_buffer_loan` resolves a receiver through a declared endpoint as well as a supervision handle | Ring and sample loans can both exist without an impossible spawn order |
| `slime-root/src/buffer_adapter.rs` | Unmapping an aliased frame deletes the alias capability and releases its CSlot | Pool availability and CNode occupancy describe the same thing |
| `components/bins/src/bin/fabric-*.rs` | Ring depth read from the resolved profile; role replies told from QoS events by record magic | A component attaches at the depth the fabric formatted |
| `scripts/build/build-generation.py` | Notification slot constants derive from the manifest's bindings, not the resolved participant set | A component compiles under a profile that prunes it |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A retired universal label is silently reused | `just sel4_channel_check` | A component on the old ABI invokes whatever moved into its slot |
| Native endpoint authority lost across allocation | `just sel4_crossing_check` | `capability exported … kind=endpoint` missing |
| Loan writability widened by a receiver | `just test_sel4_root` | `map_loan` installs `ReadWrite` for a read-only loan |
| Ring reader trusts a peer-written header | `just test_sel4_root` | `valid_ring_header` accepts an inflated `slot_count` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_channel_check` | Pass | Direct |
| `just sel4_crossing_check` | Pass | Direct |
| `just test_sel4_root` | 118/118 across 13 modules | Direct |
| `just lint_all` | Pass | Direct |
| `just fmt_check_all` | Pass | Direct |
| `just sel4_stream_check` | **Fail** — ring loan refused `Destination not empty` | Direct |
| `just sel4_qos_check`, `sel4_call_check`, `sel4_operation_check` | Not run this session | — |
| `just sel4_visibility_check` | **Fail at build** — `fabric-intruder: bindings do not close over related grants` | Direct |

## Decisions

- Decision: a v2 ring crosses as a writable loan, not as a shared-buffer handle.
- Rationale: `authorize` requires `region.owner == holder`, so a peer handed a
  handle is refused when it maps. A loan is already the primitive for "another
  holder may map this range", and it keeps the fabric the accountable owner.
- Rejected alternative: reassigning region ownership on delegation, which would
  move the quota charge with it and leave the fabric unable to reclaim a ring
  whose peer died.

- Decision: a loan may name its receiver through a declared native endpoint.
- Rationale: the fabric loans a ring to each participant while
  `fabric-publisher-b` loans its large sample back to the fabric. Requiring
  supervision in both directions needs each spawned before the other. A declared
  endpoint is fixed by the generation before either task runs, so the receiver
  is still a capability rather than an ambient task id.
- Rejected alternative: a second spawn pass handing the fabric's handle over
  after the fact, which makes the participant's authority depend on spawn order.

- Decision: the export id stays off the wire; a receiver claims the oldest
  finalized export addressed to it.
- Rationale: the 64-byte descriptor is full, and the field the id was being
  written into is the one receivers read to detect a denial. Exports are
  recorded in finalize order and a sender finalizes before its message is sent,
  so the Nth delegation a receiver observes is the Nth export addressed to it.
- Rejected alternative: widening `CapabilityTransfer` past 64 bytes, which is
  also `MAX_MSG` — the descriptor would no longer fit one message.

## Open risks and follow-ups

- [ ] `fabric-subscriber`'s ring loan is refused `CNode Copy: Destination not
      empty` at slots the pool reports as freshly issued (1485, 1493, 1501,
      monotonic). Some path installs a root capability without going through
      `SlotPool`; `just sel4_stream_check` is the gate.
- [ ] `sel4_visibility_check` fails at build on `fabric-intruder` binding
      closure.
- [ ] `sel4_qos_check`, `sel4_call_check`, `sel4_operation_check` unrun since
      the cutover.
- [ ] Auto-allocate the declared slot namespace and resolve component slots by
      role (B50's `fixed-slot constants` clause, now scoped there).
- [ ] `SLIME_RT recv shape` is retained as a refusal diagnostic; remove it if
      the malformed-shape path proves uninteresting.

## Artifacts and provenance

- Focused report: this entry
- Raw transcript: not retained; the decisive lines are quoted in the roadmap
  entry for B46
- Serial/debugger/model output: `SLIME_RT recv shape label=573 words=0 caps=0`,
  `SLIME_GRAPH alias reserve frame=1494 slot=1501`, `CNode
  Copy/Mint/Move/Mutate: Destination not empty`
- Related roadmap item: `roadmap/00-backlog.md` B46, B50
