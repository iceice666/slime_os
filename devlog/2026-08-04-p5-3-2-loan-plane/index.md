# P5.3.2 — the loan plane and generation-declared quotas on seL4

| Field | Value |
|---|---|
| Date | 2026-08-04 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/{main,transit,graph,ipc,buffer_adapter,transfer_window,channel,shared_buffer}.rs`, `components/bins/src/bin/init.rs`, `contracts/generation/v1/fixtures/sel4-loan.{zti,md}`, `scripts/build/{build-generation,build-sel4}.py`, `scripts/check/check-sel4-loan-plane.py`, `Justfile` |
| Roadmap | P5.3.2, B13 |
| Gates | `just sel4_loan_check`, `just sel4_channel_check`, `just sel4_component_graph_check`, `just sel4_root_boot_check` |
| Trigger | P5.3.1 complete; `Spawn`'s sibling planes still unmediated and `SHARED_QUOTA` still a hardcoded constant |
| Baseline | P5.3.1's channel plane, `just sel4_channel_check` green at `sends=17 receives=17 parks=2 settled=3 parked=0 queues=0` |

## Summary

The four loan operations (`SharedBufferLoan`, `SharedBufferLoanMap`,
`SharedBufferReturn`, `SharedBufferRevoke`) had no dispatcher arm and fell to
the `unimplemented` catch-all, and every launched task received the same
hardcoded `SHARED_QUOTA` rather than the ceiling its generation declared.
`SharedBufferTable::reclaim_holder` existed but was called only from unit tests,
so a dead task's buffers, mappings, and loans were never settled.

This slice serves all four operations against quotas decoded from the
generation's `shared-buffer-budget` resource, adds capability transfer over
`send` — without which a loan cannot reach its receiver at all — and reclaims
every holder at death. The receiver in the gate is `sample-receiver`,
**unmodified**: the same binary the x86 oracle runs, which is the load-bearing
claim of the slice.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | Mid-implementation checkpoint after the quota decode alone: `just sel4_component_graph_check` passed, and a direct boot showed `spawn-service` budgeted `4/1/2/1` with the other four holders at `DENY`. | The decode was correct *and* the P5.2 graph was a real regression signal for it, exactly as planned. Every one of those four had silently held `2/2/2/0` before. |
| 2 | First loan boot: `[init] loan plane fail: writable map`, with `class=range`. | `serve_buffer_create` allocated **one** frame regardless of `pages`, so a two-page request created a one-page region and every range check — which reads the anchor count — then refused the caller's own two-page mapping. |
| 3 | Next boot: `[sample-receiver] fail: recv`. | `receive_atomic` ran the refusing adapter over the dequeued message's caps, which are now transit tokens, so every capability-carrying message was refused on delivery. Split into `CarryCapabilities` (receive, pass through) and `DepartingCaps` (send, move). |
| 4 | Next boot: seL4 itself complained — `ARMPageMap: Attempting to remap a frame that does not belong to the passed address space`. | An seL4 frame capability records exactly one mapping. A loan is two holders mapping the same frames, so the receiver's map cannot go through the capability the lender's is spent on. Fixed with a per-mapping `CNode_Copy` alias — the same technique `child_vspace::transfer_window_alias` already uses for the root's staging mapping, in its per-mapping form. |
| 5 | Next boot: `[init] loan plane fail: released buffer still nameable`. | A resolved-slot failure answered `ERR_INVALID_ARG`, but `sample-lender` and `sample-receiver` both test for exactly `ERR_BAD_CAP`. `IpcError` had no variant for it — every kernel that answers this ABI must, so `BadCapability` was added rather than the component's expectation relaxed. |
| 6 | Next boot: quota markers out of order. | The root prints per-holder quota lines in task-id order, and task ids follow staging order — which the milestone claims nothing about. Moved from the ordered sequence into an order-independent `check_declared_quotas` that checks each holder *by name* against the fixture. |
| 7 | Fault injection removing `serve_buffer_create`'s page ceiling **passed** the gate. | Not a gap: `SharedBufferTable::preflight_buffer_charge` re-checks the same ceiling. Recorded because a single-site injection would otherwise have read as uncovered. Removing both sites fails, as it must. |
| 8 | A three-lens review pass found nine issues the gate did not. Two were defects this slice introduced: the `undelegated` refusal answered a *distinguishable* status, giving a free oracle for sweeping every slot number to learn which hold channel ends (the check runs before `buffers.loan()`, so it costs no quota and leaves no state); and `serve_send`'s marker gained a field that silently broke two of P5.3.1's assertions. Three more were real but narrower: a loan could be sent to a task it was not minted for and stay charged, `receiver_slot` was resolved with no rights requirement, and a re-park with nowhere to go dropped a capability silently. All fixed; the `undelegated` one is the sharpest, because the fixture could never have caught it — every grant in `sel4-loan.zti` is transferable, so the arm stays green either way. | The gate proves the plane works; it does not prove the plane is not *leaky*. Error-code indistinguishability in particular is invisible to any transcript that only records the happy path and the arms a component checks. |
| 9 | The correctness lens found two more the gate could not: `serve_buffer_create` discarded the caller's `writable` flag — packed in bit 32 of the same word as the factory slot, which the sibling `SharedBufferMap` arm reads correctly — so every region was created writable whatever was asked for; and `alias_frame` recorded its registry entry *before* attempting the map, so a map that then failed left an entry nothing could ever take, since `take` runs from the `Unmap` arm alone and an unmap is only emitted for a mapping that committed. | Both fixed. The `writable` slip is a rights widening the root decided rather than the generation, in the one place this slice made sealed/read-only distinctions load-bearing. The alias one would have broken the gate's own `aliases=0` permanently rather than silently, but only on a path no fixture reaches. |
| 10 | Injecting "never record an alias" **passed** the gate, twice over. | A third coverage gap, and the quietest: the terminal `aliases=0` is satisfied by a boot that never aliased at all, and the receiver's unmap would then go through the anchor and tear down the *lender's* mapping while the receiver's silently survived its own teardown. Fixed by asserting the alias at the moment it is minted — `SLIME_GRAPH frame aliased` — which the injection now fails. Same shape as step 9's lesson: a zero at teardown evidences reclamation, never that the thing being reclaimed existed. |
| 11 | Fault injection removing `Transit::reclaim` **passed** the gate. | A real coverage gap. Every transfer in the fixture settled cleanly, so no boot had ever left a capability in flight and the arm looked covered while being untested. Fixed by having `init` strand one loan deliberately over a second channel to `console` that console never reads — deterministic rather than racing a peer that might consume it. Re-injecting now fails at `transit=1`. |

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Quota decoding | `shared_buffer_budget` / `declared_quota` in `main.rs` locate the budget resource by magic and resolve each component's ceiling through `holder_identity`, mirroring `kernel/src/runtime/generation.rs::shared_buffer_quota`. An absent or malformed budget, or an unnamed holder, yields `HolderQuota::DENY`. | A ceiling is generation-declared, not compiled into the root task. Authority is never ambient. |
| Quota enforcement | `serve_buffer_create` reads the holder's own `byte_pages` instead of `SHARED_QUOTA.byte_pages`. `SHARED_QUOTA` now serves only the P5.1 fixture phase, which has no generation to read. | Both admission sites read one source. |
| Loan plane | Four new dispatcher arms; `serve_buffer_loan` and `serve_loan_lifecycle` resolve every handle through the caller's own table and let `SharedBufferTable` decide. | A loan is minted, mapped, returned, and revoked through the same table that owns rights, quota, and frame accounting. |
| Capability transfer | `Transit` (new module) parks a capability between its send and the receive that collects it; `DepartingCaps` moves it out of the sender's table inside `send_atomic`; `land_caps` installs it into the receiver's at a slot that table chose. | A logical slot number means nothing outside its own table, so a move substitutes a token for a slot rather than copying a number across. |
| Transfer policy | `Resource::is_transferable` — kind, not rights — plus the generation's `RIGHT_TRANSFER` bit. Both must hold. | Only a loan moves. An endpoint end, executable, factory, or supervision handle is authority the generation placed. |
| Frame aliasing | `FrameAliases` in `buffer_adapter.rs`: a second mapping of one frame goes through a `CNode_Copy`, recorded by `(frame, vspace, vaddr)` — and only once the map has succeeded — so the unmap goes through the capability that holds it. | An seL4 frame capability records exactly one mapping; a loan means two holders map the same frames. |
| Delegation | `serve_buffer_loan` refuses to mint a loan over a channel the generation did not declare `transferable`. | The generation's delegation bit decides whether an edge may carry authority, rather than the resource kind deciding alone. |
| Error codes | Every capability failure — absent slot, wrong kind, insufficient rights, undelegated edge, unmovable kind — answers `ERR_BAD_CAP`, matching `sys_send` and `sys_shared_buffer_loan`. | A component cannot map the root's tables by watching which error a probe returns. |
| Reclamation | `reclaim_dead_task` calls `SharedBufferTable::reclaim_holder` and `Transit::reclaim`. | A dead task's charges are settled; a capability in flight belongs to no table and is reachable by nothing else. |
| Refusal markers | `buffer_error_class` names which ceiling or check refused; `buffer_error_status` maps to the retired kernel's codes. `IpcError::BadCapability` is new. | The wire status stays coarse (four ceilings → one `ERR_OUT_OF_MEMORY`) while the marker names the class a gate must assert. |
| Page allocation | `serve_buffer_create` allocates one frame **per requested page**, and releases them all if admission fails. | A region's anchor count matches what the caller asked for, so its own mapping is not later refused as out of range. |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A quota stops being read from the generation | `just sel4_loan_check` | `check_declared_quotas` reports the ceilings the root declared against the fixture's |
| A ceiling stops refusing at ceiling+1 | `just sel4_loan_check` | `[init] <class> quota did not bite`, or `check_quota_classes` counting fewer than four |
| A non-loan resource kind becomes transferable | `just sel4_channel_check` | `[init] capability transfer denied` missing — that gate's cap-send names an *endpoint* slot |
| A holder's charges outlive it | `just sel4_loan_check` | terminal `loans=`/`mappings=`/`regions=` non-zero |
| A capability is stranded in flight | `just sel4_loan_check` | terminal `transit=` non-zero |
| A frame alias outlives its mapping | `just sel4_loan_check` | terminal `aliases=` non-zero |
| P5.3.1's or P5.2's evidence drifts | `just sel4_channel_check`, `just sel4_component_graph_check` | any marker changing shape |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_loan_check` | Pass | Direct — [transcript](loan-plane-boot.log) |
| `just sel4_channel_check` | Pass. **Two assertions updated, both tightened** — see below | Direct |
| `just sel4_component_graph_check` | Pass, unchanged markers | Direct |
| `just sel4_root_boot_check` | Pass | Direct |
| Mid-implementation checkpoint: `just sel4_component_graph_check` after the quota decode alone | Pass; `spawn-service` alone budgeted, four holders `DENY` | Direct |
| Fault injection: `Resource::is_transferable` returns `true` | `just sel4_channel_check` **fails** | Direct |
| Fault injection: `serve_buffer_create`'s page ceiling removed | `just sel4_loan_check` **passes** — the table's own `preflight_buffer_charge` re-checks | Direct |
| Fault injection: both page/buffer ceiling sites removed | `just sel4_loan_check` **fails** | Direct |
| Fault injection: `SharedBufferTable::loan`'s ceiling removed | `just sel4_loan_check` **fails** | Direct |
| Fault injection: `Transit::reclaim` removed (before the strand existed) | `just sel4_loan_check` **passes** — a real coverage gap | Direct |
| Fault injection: `Transit::reclaim` removed (after the strand) | `just sel4_loan_check` **fails** at `transit=1` | Direct |
| Fault injection: `reclaim_holder` never called | `just sel4_loan_check` **fails** at `loans=1 regions=1` | Direct |
| Fault injection: `Unmap` ignores the alias registry | `just sel4_loan_check` **fails** at `aliases=2` | Direct |
| Fault injection: aliases never recorded (before the mint marker) | `just sel4_loan_check` **passes** — a third coverage gap | Direct |
| Fault injection: aliases never recorded (after the mint marker) | `just sel4_loan_check` **fails** | Direct |

### The one existing gate this slice changed

`serve_send`'s marker gained a `caps=` field, because a send can now move a
capability and a marker that did not say so would report the same line for a
transfer and a plain message. P5.3.1's gate matched the old shape in two places
and broke on both — the ordered `sent` marker and `check_queue_depth`'s regex,
which is the assertion P5.3.1 added specifically so a channel accepting one
message and then refusing could not pass as a correctly-bounded one.

Both were **updated and tightened**, not relaxed. Each now pins `caps=0`
explicitly, which additionally states something the old assertions could not:
that the channel plane's payload-carrying sends move no capability. That is the
complement of the arm two lines below it, where a capability-carrying send is
refused. The gate asserts strictly more than before the change.

Recorded here because the repository has a documented history of the opposite —
an assertion relaxed in the same change that altered its behaviour, which is
P5.3.1's own review finding #5.

Observed terminal transcript:

```
SLIME_GRAPH holder reclaimed task=1 charges=2 actions=2 stranded=1
SLIME_GRAPH served live=0 unsupported=0 unimplemented=0 buffers=15 windows=0 tables=0
SLIME_GRAPH channels served sends=4 receives=3 parks=3 settled=6 parked=0 queues=0 replies=8
SLIME_GRAPH loans served=4 loans=0 mappings=0 regions=0 transit=0 orphans=0 aliases=0
```

## Decisions

- **Decision:** capability transfer over `send` lands in this slice rather than being deferred to P5.5's `Operation::CapTransfer`.
- **Rationale:** the loan cannot reach its receiver without it. `sample-lender` sends the loan capability alongside the descriptor; a slice that minted loans nobody could receive would satisfy no clause of the exit condition. This is the narrow form — one resource kind, moved over a channel the generation declared — and P5.5's `CapTransfer` is the *narrow-on-transfer* rights-reducing variant, a different operation.
- **Rejected alternative:** install the loan directly into the receiver's table at loan time. It would make the loan reachable without the lender ever having transferred it, which is not the model P5.3.4 composes and would need undoing.

- **Decision:** `serve_buffer_loan` names its receiver through a **channel peer**, not a supervision handle.
- **Rationale:** the retired kernel resolves a `RIGHT_SUPERVISE` handle minted when `init` spawned the receiver. There is no spawn here — that is P5.3.3 — so no such handle exists. Nor would deriving one from a `supervise` *grant* be sound: x86's `sample-plane-receiver-supervision` is `source = init, target = sample-lender`, meaning "init may hand sample-lender a handle", and names no subject at all. A rule reading a subject out of it would contradict the only witness for that right. The channel peer is a real bound — a component can only loan to a task the generation already connected it to — and it is still "named by capability", which is what the exit condition asks for.
- **Rejected alternative:** materialize `Resource::Supervision` from a `supervise` grant under a locally-invented subject rule. Rejected on the evidence above; P5.3.3 replaces the peer lookup with the real handle, and the operation's shape does not change.

- **Decision:** the refusal *class* is a serial marker, not a wire status.
- **Rationale:** "each of the four quota classes fails at ceiling+1" is not observable from a status code that says only "quota" — and `slime_rt` has six codes, with every ceiling collapsing to `ERR_OUT_OF_MEMORY` exactly as the retired kernel's does. Widening the ABI so a gate could distinguish them would change what every component sees to make a test easier. The marker is root-internal and costs a component nothing.

- **Decision:** the transfer window's capability-slot array stays a calling
  convention rather than becoming a Zutai schema.
- **Rationale:** CLAUDE.md requires a versioned schema for every serialized
  format crossing a persistence, process, or boot boundary. This patch is the
  first to make that array's contents meaningful — before it,
  `RefuseCapabilityTransfer` guaranteed `cap_count == 0` on every delivery — so
  the question is properly raised here. It stays exempt on the same ground the
  rest of the window already does, stated in `transfer_window.rs`'s module doc:
  the layout is a register-and-frame calling convention between two halves of
  one operation, built for one target, never persisted and never crossing a boot
  or version boundary. The two ends are `components/runtime/src/syscall/wire.rs`
  and `slime-root/src/transfer_window.rs`, and both are rebuilt together by the
  same generation build.
- **Rejected alternative:** reuse `contracts/capability-transfer/v1/`. That
  schema is C8.3's `CapabilityTransfer` descriptor — a 64-byte record carrying
  `status`, `object_kind`, and an explicit `rights_mask` for a *narrow-on-transfer*
  move the fabric performs. This slice moves a capability at its held rights and
  narrows nothing, so the record's whole discriminating content would be
  unused. P5.5 is where that descriptor's operation arrives.

- **Decision:** `FrameAliases` is a `static`, like `main.rs`'s `CHANNELS`.
- **Rationale:** it must outlive every `BufferAdapter` — one adapter installs a mapping and a later one tears it down, and both must agree which capability records it — while every adapter is a short-lived local built per operation. Threading it through the eight construction sites would add a parameter to each to reach the two that read it.

## Open risks and follow-ups

- [ ] **B12** remains open and is again deferred, not skipped. Its own analysis
  establishes that the seL4 target is unaffected — `components/.cargo/config.toml`
  keys its rustflags by triple, the seL4 component build matches none of them,
  and `build-generation.py` passes `--remap-path-prefix={ROOT}=.` explicitly on
  that path. This slice adds a fourth seL4 generation built through that same
  path, so it neither touches the defect nor extends its reach. Fixing it still
  means rebuilding every frozen x86 component image and re-authenticating every
  generation identity the x86 gates assert against.
- [ ] **B13**: `serve_buffer_create` still admits an allocation without resolving a `bufferCreate` capability, so the quota is the only bound. Recorded in `roadmap/00-backlog.md` with its deferral reason: closing it renumbers every component's capability slots, which is the same distribution problem P5.3.3 solves for spawn grants.
- [ ] `slime-root`'s unit tests — now including `transit.rs`'s six — remain unrunnable in both directions (host `unwinding` fails; the seL4 target has no `test` crate). Pre-existing since P5.1; the QEMU markers are this slice's entire executable evidence.
- [ ] Three findings from the review are recorded rather than gated, because
  each needs a fixture arm the current graph cannot express: a loan sent to a
  task it was not minted for (now refused, but no boot attempts it), a
  receive-only end named as a loan receiver (now refused, likewise), and a
  re-park into a full transit table (now reported, unreachable at
  `MAX_TRANSIT = 16`).
- [ ] The transfer is exercised with exactly one capability per message. `MAX_MESSAGE_CAPS` is four, and `DepartingCaps`'s all-or-none rollback across a partial move is covered only by reasoning, not by a boot.
- [ ] `Transit::recall` and `unland_caps` are likewise unexercised: no boot has yet failed a send after its capabilities were parked, or failed a landing partway.
- [ ] A `frame_unmap` that fails still has no recovery path; the orphan table records it and the terminal `orphans=` count surfaces it. Carried from P5.3.1.

## Artifacts and provenance

- Raw transcript: [`loan-plane-boot.log`](loan-plane-boot.log)
- Fixture rationale: [`contracts/generation/v1/fixtures/sel4-loan.md`](../../contracts/generation/v1/fixtures/sel4-loan.md)
- Related roadmap item: [P5.3.2](../../roadmap/07-architecture-portability.md)
- Preceding slice: [`devlog/2026-08-04-p5-3-1-channel-plane/`](../2026-08-04-p5-3-1-channel-plane/index.md)
