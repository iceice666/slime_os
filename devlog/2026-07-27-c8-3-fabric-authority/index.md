# C8.3 — Attenuated endpoint provisioning and control plane

| Field | Value |
|---|---|
| Date | 2026-07-27 |
| Kind | Change |
| Status | Verified |
| Scope | Zutai capability-transfer contract, `SYS_CAP_TRANSFER`, kernel descriptor validation, fabric service and three client components, generation manifest and bootstrap wiring, C8.3 checks |
| Roadmap | C8.3 |
| Gates | `just fabric_authority_check` |
| Trigger | C8.3 opened after C8.2 made the fabric graph authenticated generation data that nothing at runtime consumed |
| Baseline | C8.2 declared routes, directions, and QoS as generation data, but no component read the graph and the only capability-movement path (`SYS_SEND` attachment) moved a capability at its full held rights, so an attenuated route role could not be expressed |

## Summary

C8.3 makes the declared graph load-bearing. It adds one generic kernel
mechanism — `SYS_CAP_TRANSFER`, a bounded narrow-on-transfer move whose
destination rights are an exact subset of the source rights *and* of the
object's meaningful rights — and a userspace fabric service that composes route
authority out of it. The service mints both halves of a route, hands the
publisher `RIGHT_SEND` only and the subscriber `RIGHT_RECV` only, and omits
`RIGHT_TRANSFER` from both so neither can re-delegate. Clients are
authenticated by the generation-provisioned control endpoint their request
arrived on, never by the route name, direction, or type identity they supply —
`fabric-intruder` holds a real control endpoint and supplies byte-identical
route strings, and still receives nothing. The kernel gained no knowledge of
routes, schemas, or graph roles.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/capability-transfer/v1/` | Versioned Zutai contract for two 64-byte control messages: `FabricRequest` (what a client asks for) and `CapabilityTransfer` (the descriptor accompanying one move, reused with a nonzero `status` as a denial) | The wire form of an attenuated move is schema-owned; magics, widths, and offsets all derive from `schema.zt` |
| `kernel/src/syscall/mod.rs` | `SYS_CAP_TRANSFER` (30): requires `RIGHT_TRANSFER` at the source, rejects a mask outside the source rights or the object's `valid_rights()`, requires the descriptor's declared kind to be the moved capability's real kind, consumes the source, and restores it at full rights on a failed send | A route role can be handed over exactly attenuated, and the bytes the peer reads are the rights the kernel installed |
| `kernel/src/protocol/capability_transfer_proto.rs` | Kernel-side descriptor validation and the object-kind ↔ `KernelObject` mapping; `destination_rights` strips `RIGHT_TRANSFER` unless `FLAG_RETAIN_TRANSFER` is set | Non-delegability is the default, and retention is deliberate rather than incidental |
| `components/bins/src/bin/fabric-service.rs` | Userspace control plane: owns both route endpoints, answers by graph lookup keyed on the caller's control endpoint, sweeps every client through the non-blocking ABI and parks in `SYS_WAIT` across the whole set | Route policy lives in userspace; possession of names mints nothing |
| `components/bins/src/bin/fabric-{publisher,subscriber,intruder}.rs` | Three participants that assert their own denials: no opposite-direction authority, no re-delegation, no widening, and — for the ungranted component — no capability at all | Every required denial is observed on a live boot, not argued |
| `components/bins/build.rs` | Emits `FABRIC_PARTICIPANTS` from the same manifest the host builder encodes into the fabric-graph resource, cross-checked against an independent count of the same block | The service and the authenticated resource cannot disagree about which edges exist |
| `contracts/generation/v1/fixtures/valid.zti`, `kernel/src/runtime/bootstrap.rs`, `init.rs` | Four fabric components, their control-channel grants, capability slots 45–54 (transfer moved to 55/56), and the `SLIME_FABRIC_AUTHORITY_CHECK` scenario | The control-endpoint ↔ component binding is established by the generation at spawn, so "who is asking" is a capability fact |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A provisioned role gains the opposite direction | `just fabric_authority_check` → `[fabric-publisher] route receive denied`, `[fabric-subscriber] route publish denied` | The denial marker is absent or out of order in its chain |
| A route role becomes delegable, or widens itself | `just fabric_authority_check` → `re-delegation denied`, `widening denied`; `fabric_authority::transfer_authority_is_dropped_unless_explicitly_retained` | A `cap_transfer` on a provisioned role returns anything but `ERR_BAD_CAP` |
| A mask widens past the source or the object kind | `fabric_authority::masked_transfer_cannot_widen_rights` | `derive` or `insert` accepts a bit the source or object does not admit |
| A descriptor misdescribes what crossed | `fabric_authority::descriptor_kind_must_match_the_moved_object` | A declared kind matches a different object, or an untransferable object matches any kind |
| A moved capability is duplicated or lost on a failed send | `fabric_authority::a_move_consumes_the_source_and_never_duplicates` | The source survives the move, or a restore returns the narrowed rights |
| An ungranted component obtains an edge by supplying correct names | `just fabric_authority_check` → `[fabric] ungranted component denied: fabric-intruder`, plus the intruder's own status/mask/slot assertions | The intruder receives a nonzero mask or any capability |
| The fabric busy-waits | `check_no_busy_wait_shape` in `scripts/check/check-fabric-authority.py` | A fabric source contains `yield_now`, or never parks |
| The build-time participant table silently loses an entry | `assert_eq!(participants.len(), declared)` in `components/bins/build.rs` | Build failure naming the declared count |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just fabric_authority_check` | Passed: 7 kernel algebra tests plus the live boot; all six transcript chains observed in order | Direct |
| `just fabric_manifest_check` | Passed after updating the C8.2 fabric-host assertion from `init` to `fabric-service` (4 QEMU tests) | Direct |
| `just contracts_check` | Passed, including the new capability-transfer schema/renderer and its `--check` binding comparison | Direct |
| `just generation_check` | Passed: two byte-identical builds carrying the four new components | Direct |
| `just test`, `just sample_plane_live_check` | Passed; the C7 live plane is unaffected by the slot renumbering | Direct |
| `just fmt_check`, `just lint`, `just fmt_check_components`, `just lint_components`, `just framework_safety_check`, `just devlog_check` | Passed | Direct |
| Fault injection: 9 mutations across the fabric service, the syscall, and the descriptor validator | 8 caught by `just fabric_authority_check`, 1 (breaking the object-kind binding) caught by `fabric_authority::descriptor_kind_must_match_the_moved_object`; 2 further mutations were confirmed no-ops rather than gate gaps | Direct |
| Slot renumbering audit | Enumerated `launch_init`'s capability vector: the ten fabric capabilities occupy exactly 45–54 and transfer lands at 55/56, matching `init.rs` | Direct |

## Decisions

- Decision: Add one generic `SYS_CAP_TRANSFER` rather than a fabric-aware provisioning syscall.
- Rationale: The roadmap fixes the kernel's only new C8 mechanism as "a generic bounded narrow-on-transfer operation so a userspace service can move a capability with an exact non-widening rights mask". The kernel therefore validates rights, object kind, and consumption, and knows nothing of routes — `route_identity` and `direction` ride in the descriptor as opaque bytes it never interprets.
- Rejected alternative: A `SYS_ROUTE_GRANT` that understood the fabric graph. That would move route policy into the kernel and make every later C8 slice a kernel change.
- Decision: Drop `RIGHT_TRANSFER` at the destination unless a distinct flag retains it, rather than treating its presence in the mask as consent.
- Rationale: The deliverable requires transfer authority to be omitted "unless it was both held and explicitly retained". Two independent conditions mean a provisioning service that simply forwards its own rights produces a non-delegable capability by default; forgetting to strip a bit is the common mistake, and this makes the safe outcome the default one.
- Rejected alternative: Reading the mask alone, where one stray bit silently makes every issued role re-delegable.
- Decision: Make the descriptor the same bytes the kernel enforces, and require its declared object kind to match the moved capability.
- Rationale: Otherwise a service could hand over a `RIGHT_SEND` endpoint while describing a read-only buffer, and the receiver's own validation would be checking a fiction. Binding them means the peer's parse and the kernel's installation are one decision.
- Decision: Authenticate clients by control endpoint, and have the request carry the route name, direction, and type identity anyway.
- Rationale: The required check is that an ungranted component fails "even when it supplies the exact route and schema strings". Carrying those fields and demonstrably ignoring them turns that into an executed test rather than an absence of code; `fabric-intruder` sends byte-identical strings to the publisher's.
- Decision: Derive the service's participant table from the manifest at build time instead of reading the graph resource at runtime.
- Rationale: The kernel exposes no syscall to read the graph, and adding one would give the kernel schema awareness. Generating from the same manifest the host builder encodes keeps one source of truth. Because the parser is indentation-keyed, a second independent count of the same block guards against a silently short table — a dropped participant would otherwise deny a declared component with no diagnostic.
- Decision: Let the service exit after one provisioning round rather than looping forever.
- Rationale: Provisioning is the entire control plane at C8.3, a role is minted once, and a bounded run is what lets the gate assert both declared directions were claimed before teardown. C8.4 gives the service ongoing work and the loop becomes unbounded then; the parking behaviour under test is identical either way. Recorded here because the milestone text says "long-lived", so a later reader should not read the exit as a regression.

## Open risks and follow-ups

- [ ] A capability moved to a task that dies between the send and its `recv` is destroyed with the queued message rather than returned: `Endpoint::drop` never drains `channel.queue`. `SYS_SEND`'s cap attachment has the same shape, so this is pre-existing rather than introduced, but C8.3's one-shot route capabilities make it reachable — a client dying in that window silently retires a declared route endpoint. Closing it needs queue-draining on endpoint teardown, which is a kernel-wide change.
- [ ] The service consumes an endpoint factory and the generation graph, but not the shared-buffer budget/factory or an explicit time input, and it mints one stream route pair rather than data/ack/call/operation endpoints. Those are consumed by C8.4 (streams and shared-sample brokering) and C8.5 (timed QoS); C8.3 provisions the route roles they will use.
- [ ] `check_no_busy_wait_shape` is a necessary condition, not a proof: a loop waiting on an already-dead peer would spin while passing both greps. What actually excludes it is the service's loop shape, which retires a client on `ERR_PEER_DEAD` before parking again. That reasoning is reviewed, not tested.
- [ ] Visibility and interposition are declared in the graph and still unread; C8.8 owns filtered introspection and the declared proxy chain.

## Artifacts and provenance

- Focused report: none.
- Raw transcript: none.
- Serial/debugger/model output: none captured; the live arm's transcript is printed by `just fabric_authority_check`.
- Related roadmap item: [`C8.3`](../../roadmap/02-core-runtime.md#c83--attenuated-endpoint-provisioning-and-control-plane).
