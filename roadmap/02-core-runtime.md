# Core runtime track

**Status:** C7 and all of C8 (C8.1–C8.15) are complete under their named QEMU gates. The C8 track closed on 2026-08-17: C8.13's concurrent cross-plane traffic and resource ceilings (with C8.13.1–C8.13.3), C8.14's degradation and fault-isolation envelope, and C8.15's aggregate determinism gate, which boots both aggregate schedules twice over one declared composition and compares 279 semantically identical trace records field by field. The backlog is clear: B55's full-graph boot defects and B56's unpassable C8.9 profile check are both resolved. B46 replaced the logical channel mechanism these planes were gated on with native seL4 Endpoints, and all seven of its named plane gates — channel, crossing, stream, QoS, call, operation, visibility — pass on that path; B50 then deleted the logical capability and universal-syscall residue behind it. C10 (bounded private component memory) closed 2026-08-24 across C10.1–C10.4. C9 (robot runtime authority) is the track's one remaining open milestone, now decomposed into C9.1–C9.6; two of its original deliverables were rescoped against the pinned platform rather than carried as plans — EL0 counter access is a global kernel grant the root itself depends on, and conserved CPU accounts have no mechanism while `KernelIsMCS OFF`.

This track turns the existing bounded channels, capabilities, components, and generations into a native typed communication runtime. It is local-first: C7 and C8 require no network or physical driver, and they do not wait for unrelated display, audio, wireless, or GPU work.

ROS 2 compatibility in [`03-ros2-compatibility.md`](03-ros2-compatibility.md) is a userspace profile over this runtime. Neither seL4 nor `slime-root` learns nodes, topics, services, actions, graph discovery, message types, or transport QoS policy.

## Boundaries

- Root-mediated IPC remains a small control plane. The current 64-byte message bound (`slime-root/src/ipc.rs::MAX_MESSAGE_BYTES`) is not enlarged for sensor or image data.
- Bulk samples live in bounded shared buffers referenced by typed control messages.
- Component working memory is task-private and non-transferable. Shared buffers carry samples *between* components; they are not a general allocator, and neither mechanism may be reinterpreted as the other.
- Topic names and types are userspace metadata. Authority is carried by SEND/RECV endpoint capabilities minted or distributed by the declared fabric service.
- The generation declares which component may publish, subscribe, call, serve, inspect, or administer each graph edge.
- `TransportQoS` controls message delivery. `SchedulingClass` controls CPU ordering. They are separate contracts and namespaces.
- Slime capability transfer is native-only. A protocol gateway may retain and proxy a capability but may never serialize a kernel capability as application data.
- Capability, IPC, shared-sample, schema, QoS, lifecycle, and scheduling-policy semantics are architecture-neutral. Trap registers, syscall entry, context switching, page tables, interrupt controllers, and timer mechanisms belong to [`07-architecture-portability.md`](07-architecture-portability.md).
- C7 and B2 are observed on the `aarch64-sel4-qemu-virt` product path; the x86-64 reference path they were first built on was retired with P5. New low-level work must not add uncontained architecture assumptions outside the boundary P1 enforces.

## Sequencing

1. C7 consumes the M6 endpoint factory, spawn accounting, supervision, and generation machinery.
2. C8 consumes C7's bounded sample plane.
3. C9 consumes C8 plus P5's timer, interrupt, context-switch, and idle mechanisms. It is decomposed into C9.1–C9.6, sequenced so each slice's evidence is the next one's precondition: clock authority, then wait sets over it, then class, then restart, then replay, then the composed workload.
4. C10 consumed C7's per-holder quota and accounting pattern only; it did not consume C8 or C9, and closed 2026-08-24.
5. H2 consumes C7's generation-v3/shared-buffer foundation and P1's extracted architecture/platform boundary for userspace drivers.
6. ROS R1 consumes C8 and H6 networking; it does not block C9, and its initial wire-conformance gate does not require a physical-board boot.

## C7: Bounded resource and shared-sample plane

**Status:** Complete.
**Delivered:** Decomposed into C7.1–C7.7 (v3 generation format, factory-gated
shared buffers, generation-declared per-holder quotas, map/unmap/seal,
loan/return with fault reclamation, a versioned sample descriptor, and
two-component integration), mirroring the M5/M6 sub-slice convention. The
2026-07-26 audit reopened this gate on three findings, all resolved: C7.5's
boot wedge (backlog B3), the dormant live-path shared-buffer plane (backlog
B4), and the absence of syscall-level or real-component evidence (backlog
B5). A built generation now carries a digest-authenticated
[`shared-buffer-budget/v1`](../contracts/shared-buffer-budget/v1/) resource; `bootstrap` mints a `SharedBufferFactory`
and validates its generation grants; `dango` and `spawn-service` boot with
distinct non-`DENY` quotas; `sample-lender`/`sample-receiver` move a
`>MAX_MSG` payload through the real `SYS_SHARED_BUFFER_*` syscalls.
**Exit condition (observed):** Every C7 gate passes, including the
full-graph boot checks. Residual debt is narrow and recorded rather than
open: `SYS_SHARED_BUFFER_REVOKE` has no live caller, and the two
insert-failure rollback paths are uncovered.
**Gates:** `just sample_plane_check`, `just sample_plane_live_check`, `just generation_check`, `just contracts_check`
**Evidence:** [`devlog/2026-07-26-c7-audit/`](../devlog/2026-07-26-c7-audit/index.md), [`devlog/2026-07-26-b3-shared-buffer-table-stack-overflow/`](../devlog/2026-07-26-b3-shared-buffer-table-stack-overflow/index.md), [`devlog/2026-07-26-b4-live-shared-buffer-budget/`](../devlog/2026-07-26-b4-live-shared-buffer-budget/index.md), [`devlog/2026-07-26-b5-live-sample-plane/`](../devlog/2026-07-26-b5-live-sample-plane/index.md)

**Depends on:** the M6 endpoint factory, spawn accounting, supervision, and generation machinery.

**Sequencing:** C7.1 lands the v3 generation format and `u64` rights that every later slice consumes. C7.2 introduces the shared-buffer capability objects and factory-authorized allocation under fixed kernel bounds. C7.3 adds generation-declared quotas and supervision-subtree accounting. C7.4 adds map, unmap, and irreversible read-only sealing. C7.5 adds loan/return ownership and fault reclamation. C7.6 defines and validates the sample-descriptor contract over that lifecycle. C7.7 composes the slices into the two-component exit condition and owns `just sample_plane_check`.

### C7.1 — Generation format v3 and u64 rights

**Status:** Complete.
**Delivered:** Generation format v3 with `u64` rights, built byte-identical
across two builds; retained v2 generations still decode, keep their signed
release authorized, and pass the stage-0 admission chain. The grandfathered
`RIGHT_MAP` name was replaced with the object-specific `bufferMap` manifest
key (backlog B7, resolved).
**Exit condition (observed):** A v3 generation built from normalized input
is byte-identical across two builds, boots the existing vertical slice with
`u64` rights, and a retained v2 known-good artifact still decodes, keeps its
signed release authorized, and passes the stage-0 admission chain; an
unsupported version and an unknown rights bit both fail closed. The v2 arm
is proven to admission, not to a completed boot: no v2 artifact exists to
boot, because the builder has only ever emitted v3 and each generation
embeds the kernel it runs, so a v2 rollback would execute its own v2-era
kernel rather than this tree's (backlog B6, resolved with that scope
recorded).
**Gates:** `just generation_check`, `just contracts_check`, `just test`, `just transfer_check`
**Evidence:** [`devlog/2026-07-24-c7-1-generation-v3-u64-rights/`](../devlog/2026-07-24-c7-1-generation-v3-u64-rights/index.md), [`devlog/2026-07-26-b6-retained-v2-rollback-scope/`](../devlog/2026-07-26-b6-retained-v2-rollback-scope/index.md), [`devlog/2026-07-26-b7-b8-budget-hygiene/`](../devlog/2026-07-26-b7-b8-budget-hygiene/index.md)

**Depends on:** M6.1 generation format v2 and the capability/rights foundation.

### C7.2 — Shared-buffer authority and factory allocation

**Status:** Complete.
**Delivered:** A distinct `SharedBufferFactory` kernel object gates
`SYS_SHARED_BUFFER_CREATE`/`SYS_SHARED_BUFFER_RELEASE` behind
`RIGHT_BUFFER_CREATE`; buffers carry a kernel-assigned unforgeable identity
and only narrow-only `RIGHT_BUFFER_WRITE`/`RIGHT_BUFFER_MAP`/`RIGHT_TRANSFER`.
Allocation is bounded by fixed global ceilings (`MAX_SHARED_BUFFERS`=32,
`MAX_TOTAL_PAGES`=256, `MAX_BUFFER_PAGES`=64), returning structured
`SharedBufferError`. The factory is minted on the live boot path and granted
to `dango` and `spawn-service`, both of which allocate and release through
the real syscalls at startup (backlog B4).
**Exit condition (observed):** A factory-authorized holder creates and
releases a kernel-identified shared buffer within fixed global bounds; an
unauthorized component is denied, exhaustion is structured and isolated, and
no derivation or transfer widens authority. The create-insert-failure
rollback path remains uncovered.
**Gates:** `just shared_buffer_factory_check`, `just sample_plane_live_check`
**Evidence:** [`devlog/2026-07-24-c7-2-shared-buffer-factory/`](../devlog/2026-07-24-c7-2-shared-buffer-factory/index.md), [`devlog/2026-07-26-b4-live-shared-buffer-budget/`](../devlog/2026-07-26-b4-live-shared-buffer-budget/index.md), [`devlog/2026-07-26-b5-live-sample-plane/`](../devlog/2026-07-26-b5-live-sample-plane/index.md)

**Depends on:** C7.1 v3 rights and the M6.1 factory-capability pattern.

### C7.3 — Generation quotas and supervision accounting

**Status:** Complete.
**Delivered:** A versioned Zutai shared-buffer budget contract
([`contracts/shared-buffer-budget/v1/`](../contracts/shared-buffer-budget/v1/)) is stored as a generation
`KIND_RESOURCE` object declaring per-holder `byte_pages`, `buffer_count`,
`mapping_count`, and `loan_count` quotas, validated deterministically at
generation decode (bounding each holder and, since backlog B8, summing
holders so a validating budget can be honoured with every holder at its
ceiling at once). `SharedBufferTable::create` charges each allocation to the
creating supervision-subtree owner; `reclaim_owner` returns every unloaned
page and charge on release, peer death, supervised restart, and revocation.
The live boot path declares a real budget (backlog B4): `dango` and
`spawn-service` boot with distinct non-`DENY` quotas.
**Exit condition (observed):** Two holders receive distinct
generation-declared budgets; one reaches byte or buffer-count exhaustion
without affecting the other, and termination of its supervision subtree
returns every unloaned page and charge.
**Gates:** `just shared_buffer_accounting_check`, `just contracts_check`, `just generation_check`
**Evidence:** [`devlog/2026-07-24-c7-3-shared-buffer-accounting/`](../devlog/2026-07-24-c7-3-shared-buffer-accounting/index.md), [`devlog/2026-07-26-b4-live-shared-buffer-budget/`](../devlog/2026-07-26-b4-live-shared-buffer-budget/index.md)

**Depends on:** C7.2 factory allocation; M6.1 supervision and per-spawner accounting.

### C7.4 — Mapping and read-only sealing

**Status:** Complete (mechanism), with a coverage caveat from the
2026-07-26 audit.
**Delivered:** Bounded `SYS_SHARED_BUFFER_MAP`/`SYS_SHARED_BUFFER_UNMAP`/
`SYS_SHARED_BUFFER_SEAL`. Mapping installs only page-aligned, non-executable,
exact-frame user PTEs, gated by `RIGHT_BUFFER_MAP` and charged against the
holder's `mapping_count` quota (`MAX_MAPPINGS`=64); offset/length/base are
range- and overflow-checked, and a partial map is fully rolled back. Sealing
is an irreversible Arc-shared read-only transition that downgrades every
live writable PTE before publishing the seal.
**Exit condition (observed):** A holder maps only an in-bounds region
charged to its manifest quota, seals the buffer read-only, and cannot
recover write access; malformed ranges and lifecycle misuse fail before
page-table changes. All three syscalls are driven at the syscall boundary
by real components under `just sample_plane_live_check` (B5), which asserts
a writable mapping cannot be obtained after sealing.
**Gates:** `just shared_buffer_mapping_check`, `just sample_plane_live_check`
**Evidence:** [`devlog/2026-07-24-c7-4-shared-buffer-mapping/`](../devlog/2026-07-24-c7-4-shared-buffer-mapping/index.md), [`devlog/2026-07-26-b5-live-sample-plane/`](../devlog/2026-07-26-b5-live-sample-plane/index.md)

**Depends on:** C7.2 shared-buffer objects and C7.3 accounting.

### C7.5 — Loan/return lifecycle and fault reclamation

**Status:** Complete (mechanism); its boot regression is fixed.
**Delivered:** Bounded loan/return over an exact sealed subrange as
`SYS_SHARED_BUFFER_LOAN`/`SYS_SHARED_BUFFER_LOAN_MAP`/
`SYS_SHARED_BUFFER_RETURN`/`SYS_SHARED_BUFFER_REVOKE` behind
`RIGHT_BUFFER_LOAN` and a receiver-bound `SharedBufferLoan` kernel object. A
loan requires an irreversibly sealed source region, names its receiver
through a `RIGHT_SUPERVISE` capability, charges against the lender's
`loan_count` quota, and carries an unforgeable single-return identity.
`reclaim_owner` settles every loan naming a dying task as lender or
receiver. C7.5 originally wedged every full-graph boot: a 10520-byte
`SharedBufferTable` published through a `LazyLock` was first constructed on
a 32 KiB unguarded task kernel stack inside `task::terminate`, overflowing
it silently. Fixed by const-initializing the table into `.bss` (backlog B3,
resolved).
**Exit condition (observed):** A lender loans one sealed region and cannot
reclaim its pages until the receiver returns it; duplicate, stale, and
wrong-buffer returns fail closed, while peer death deterministically settles
the loan and restores every charge. All four syscalls are driven by real
components under `just sample_plane_live_check` (B5).
**Gates:** `just shared_buffer_loan_check`, `just transfer_check`, `just spawn_service_check`, `just dango_check`
**Evidence:** [`devlog/2026-07-25-c7-5-shared-buffer-loan/`](../devlog/2026-07-25-c7-5-shared-buffer-loan/index.md), [`devlog/2026-07-26-b3-shared-buffer-table-stack-overflow/`](../devlog/2026-07-26-b3-shared-buffer-table-stack-overflow/index.md)

**Depends on:** C7.3 accounting and C7.4 sealed mappings.

### C7.6 — Versioned sample descriptor

**Status:** Complete.
**Delivered:** A versioned Zutai sample-descriptor contract
([`contracts/sample-descriptor/v1/`](../contracts/sample-descriptor/v1/)) renders byte-identical `slime-proto`
bindings (`WireSampleDescriptor`) whose fixed control message is exactly the
channel bound (`DESCRIPTOR_LEN == MAX_MSG == 64`), referencing a transferred
`SharedBufferLoan` by capability kind, unforgeable loan identity,
page-aligned offset/length, type identity, sequence, and known flags.
`valid_sample_descriptor` rejects malformed descriptors before any mapping
or allocation; the kernel `map_loan` independently re-validates loan
identity, receiver binding, bounds, and read-only mapping.
**Exit condition (observed):** A receiver validates a bounded versioned
descriptor, maps only the exact loaned bytes, and observes a payload larger
than the control-message bound (`MAX_MSG` = 8192 bytes) without widening
`MAX_MSG` or copying payload bytes through the kernel queue; every malformed
descriptor fails before mapping or allocation.
**Gates:** `just sample_descriptor_check`, `just contracts_check`
**Evidence:** [`devlog/2026-07-25-c7-6-sample-descriptor/`](../devlog/2026-07-25-c7-6-sample-descriptor/index.md)

**Depends on:** C7.4 sealed mappings and C7.5 loan/return lifecycle.

### C7.7 — Sample-plane integration and isolation

**Status:** Complete.
**Delivered:** The composition was originally `kernel/tests/sample_plane.rs`,
which is deleted; P5.3.4 re-observed it on the product path as
`just sel4_sample_check`, where two separately spawned components hold
generation-granted capabilities rather than being composed in-harness. It
joins the C7.2 factory allocation, C7.3 per-holder quotas, C7.4
mapping/sealing, C7.5 loan/return lifecycle, and the C7.6 sample descriptor
into two holders that exchange a `>MAX_MESSAGE_BYTES` payload: only the
64-byte descriptor crosses a real endpoint while the receiver reconstructs
the full two-page payload from the quota-charged sealed loaned buffer
through exact read-only translations.
**Exit condition (observed):** Two isolated components exchange and return
a payload larger than the kernel IPC message bound through a quota-charged
shared buffer; malformed descriptors, every quota class (byte-pages,
buffer-count, mapping-count, loan-count), and peer death remain bounded,
reclaim all resources, and do not disturb an unrelated channel or the
retained v2 known-good boot path.
**Gates:** `just sample_plane_check`
**Evidence:** [`devlog/2026-07-25-c7-7-sample-plane-integration/`](../devlog/2026-07-25-c7-7-sample-plane-integration/index.md)

**Depends on:** C7.1–C7.6.

## C8: Native typed data fabric

**Status:** Complete.
**Delivered:** C8.1–C8.15, decomposed from a former single C8.9 integration
slice into C8.9–C8.15 so profile authority, topology, deterministic tracing,
denial, resource ceilings, fault isolation, and the parent close each own one
reviewable gate.
**Exit condition (observed):** All fifteen sub-milestones pass their named
QEMU gates. Three of the closing slices found the honest scope narrower or
differently shaped than their text assumed, and each is recorded that way
rather than claimed: C8.13's `resourceEvent` and the call worker's
`resourceLoan` are structural walls, C8.13.3's `capabilitySlots` turned out
to bound a different slot space than the census first counted, and C8.14
turned out to be an assertion milestone over machinery C8.4–C8.9 had already
built and driven. The C8.15 audit also reopened and closed C8.9: backlog B56
records that `just data_fabric_profile_check` had been red since B55 on a
check that could not pass.
**Gates:** see each C8.x sub-milestone below; the closing aggregate is `just data_fabric_check`
**Evidence:** [`devlog/2026-08-17-c8-15-fabric-aggregate/`](../devlog/2026-08-17-c8-15-fabric-aggregate/index.md), [`devlog/2026-08-17-structural-audit/`](../devlog/2026-08-17-structural-audit/index.md)

**Depends on:** C7's bounded sample plane and backlog item **B2** (scheduler
`Blocked` state and its wait mechanism). Both are complete. C8 is local-first
and was closed on the `aarch64-sel4-qemu-virt` product path; B46 replaced the
root-owned channel and wait-set mechanism it was first built on with native
seL4 Endpoints and Notifications.

### Architecture decisions

- the authoritative `InterfaceSchema` identity is a domain-separated SHA-256
  digest of versioned normalized Zutai schema bytes; generated bindings embed
  the full identity;
- C7's existing 64-bit sample-descriptor `type_identity` remains wire-stable
  and becomes a generation-local type tag derived from the full identity;
  generation admission rejects tag collisions between distinct admitted
  schemas, and route matching never treats the tag alone as authority;
- seL4 and `slime-root` remain unaware of schemas, graph names, route kinds,
  QoS, and correlation policy. The root's only new C8 mechanism is a generic
  bounded narrow-on-transfer operation so a userspace service can move a
  capability with an exact non-widening rights mask;
- the fabric brokers large samples through C7's receiver-bound loans. It maps a
  publisher loan read-only, makes one bounded copy into a fabric-owned sealed
  buffer, and creates one receiver-bound downstream loan per subscriber. C8
  does not add multi-receiver loans or transferable ambient supervision;
- timed QoS consumes an explicit capability-routed monotonic-time input. The
  C8 corpus drives it with deterministic simulated time; C9 later supplies the
  standard component-facing monotonic and simulated-time services without
  changing C8 QoS state-machine meanings;
- the initial fabric graph admits at most `MAX_INGRESS_SOURCES` live ingress
  sources per fabric instance, declared in `contracts/fabric-graph/v1` and
  currently 9. Admission rejects a graph whose endpoint and control topology
  cannot block without polling; expanding the multi-source wait or introducing
  bounded route workers requires a later observed profile need;
- `Operation<Goal, Feedback, Result>` owns bounded transport, correlation,
  feedback, result, cancellation, and peer-loss semantics. Application goal
  policy and the ROS action state machine remain outside the fabric.

### Sequence

1. C8.1 defines the normalized schema and generated native contracts.
2. C8.2 makes the graph, QoS, visibility, interposition, and every resource
   ceiling deterministic generation data.
3. C8.3 supplies attenuated capability handoff and the live fabric control
   plane. Complete.
4. C8.4 establishes bounded streams; C8.5 adds reliable, retained, and timed
   QoS. Complete.
5. C8.6 establishes calls; C8.7 composes calls and streams into operations.
   Complete.
6. C8.8 adds filtered introspection and declared interposition. Complete.
7. C8.9 closes the typed full-profile and resource-bound contract.
8. C8.10 establishes a collision-free full-graph bootstrap and bounded route
   workers.
9. C8.11 unifies simulated time and versioned semantic traces.
10. C8.12 executes the complete matching, visibility, and denial matrix.
11. C8.13 proves concurrent cross-plane traffic and every resource ceiling.
12. C8.14 proves degradation and fault isolation.
13. C8.15 owns the repeated-boot determinism corpus and closes C8.

### C8.1 — Deterministic interface schemas and native bindings

**Status:** Complete.
**Delivered:** A bounded versioned Zutai normal form for native interface
schemas, with the authoritative `InterfaceSchema` identity derived from the
exact normalized bytes as a domain-separated SHA-256 digest, generated Rust
bindings and embedded identities for `Stream<T>`, `Call<Request, Reply>`,
and `Operation<Goal, Feedback, Result>`, and a generation-local 64-bit
type-tag derivation consumed by the retained C7 sample descriptor.
**Exit condition (observed):** `just interface_schema_check` and the live
sample-plane gate pass with one deterministic normal form, full identity,
generated local tag, and native binding set; malformed, unsupported,
over-bound, duplicate, and forced-collision inputs fail before output.
**Gates:** `just interface_schema_check`
**Evidence:** [`devlog/2026-07-27-c8-1-interface-schemas/`](../devlog/2026-07-27-c8-1-interface-schemas/index.md)

### C8.2 — Generation graph, QoS, and aggregate admission

**Status:** Complete.
**Delivered:** A versioned Zutai fabric-graph contract
([`contracts/fabric-graph/v1/`](../contracts/fabric-graph/v1/)) stored as a generation `KIND_RESOURCE` object
fixing the admitted schema set, route table, participant table with exact
`TransportQoS`/visibility/interposition chains, and every per-graph resource
ceiling. Route authority is the fold of (route name, full interface
identity, contract kind); participant authority additionally folds in
component identity and direction. The agreement between declared limits and
the root's live constants is re-checked at admission by
`slime_root::generation::fabric_graph_is_satisfiable`, so a drifting graph
is refused at boot rather than only at build.
**Exit condition (observed):** One authenticated generation resource
deterministically fixes every native interface, graph edge, direction, QoS
policy, visibility grant, interposition hop, and resource ceiling;
malformed, unauthorized, or globally impossible graphs fail before component
launch. `just fabric_manifest_check` passes: a deterministic 896-byte
resource with 2 schemas, 2 routes, 4 participants, and one interposition
hop, a 35-case negative corpus each rejected by its intended check, 18
`boot-contracts` decoder tests, and 4 QEMU tests against the booted
generation.
**Gates:** `just fabric_manifest_check`
**Evidence:** [`devlog/2026-07-27-c8-2-fabric-graph-admission/`](../devlog/2026-07-27-c8-2-fabric-graph-admission/index.md)

**Depends on:** C8.1.

### C8.3 — Attenuated endpoint provisioning and control plane

**Status:** Complete.
**Delivered:** A versioned Zutai capability-transfer contract
([`contracts/capability-transfer/v1/`](../contracts/capability-transfer/v1/)) and the kernel's only new C8
mechanism, `SYS_CAP_TRANSFER` (30), which requires `RIGHT_TRANSFER` at the
source, rejects any mask outside the source or object-meaningful rights,
consumes the source, and restores it at full rights on a failed send;
`RIGHT_TRANSFER` is dropped at the destination unless
`FLAG_RETAIN_TRANSFER` is set. A userspace `fabric-service` owns both halves
of the declared route, authenticating each client by the
generation-provisioned control endpoint its request arrived on rather than
route name, direction, or type identity.
**Exit condition (observed):** The live fabric derives exact non-widening,
non-transferable route endpoints from the authenticated generation graph;
on a real boot the publisher holds `RIGHT_SEND` only and the subscriber
`RIGHT_RECV` only, and `fabric-intruder` — holding a real
generation-provisioned control endpoint and supplying byte-identical route
strings — receives a denial carrying no capability. The "consumes no CPU
through a poll/yield loop" arm is proven by a source lint (the gate rejects
any fabric component containing `yield_now` or lacking a `SYS_WAIT` park), a
necessary condition rather than a direct measurement.
**Gates:** `just fabric_authority_check`
**Evidence:** [`devlog/2026-07-27-c8-3-fabric-authority/`](../devlog/2026-07-27-c8-3-fabric-authority/index.md)

**Depends on:** C8.2.

### C8.4 — Bounded many-to-many streams

**Status:** Complete.
**Delivered:** A versioned Zutai fabric-stream contract
([`contracts/fabric-stream/v1/`](../contracts/fabric-stream/v1/)) defining the three fixed 64-byte records a
bounded stream moves (`StreamSample`, `StreamAck`, `StreamEvent`). The
userspace `fabric-service` brokers each route by ingress-endpoint identity;
a payload larger than `MAX_MSG` arrives as a C7.6 descriptor over a
receiver-bound loan, copied once into a fabric-owned sealed buffer and
re-loaned per matched subscriber. Delivery is bounded by each subscriber's
declared KEEP_LAST depth, evicting the exact oldest sequence past depth and
reporting the loss on resume.
**Exit condition (observed):** A generation-declared many-to-many stream
moves bounded typed inline and shared samples under exact route authority;
KEEP_LAST and BEST_EFFORT behavior is deterministic, and a stalled or
faulting participant cannot grow or disturb unrelated state. On a real boot
two publishers and two subscribers exchange both sample forms over
`telemetry` while `diagnostics` carries an unrelated stream through the same
service; one `>MAX_MSG` sample is counted at exactly one fabric copy and one
quota-charged receiver-bound loan per subscriber. The eviction rule is
pinned by host unit tests because a transcript can show samples arrived but
not which one was dropped; a participant fault beyond a deliberate stall is
C8.9's composition.
**Gates:** `just fabric_stream_check`
**Evidence:** [`devlog/2026-07-28-c8-4-bounded-streams/`](../devlog/2026-07-28-c8-4-bounded-streams/index.md)

**Depends on:** C8.3.

### C8.5 — Reliable, retained, and timed QoS

**Status:** Complete.
**Delivered:** A bounded credit/acknowledgement protocol so RELIABLE
delivery never busy-retries a full channel and BEST_EFFORT never acquires
retry state; unacknowledged and durability history retained within fixed
sample, byte, buffer, loan, retry, and event ceilings; deadline, lifespan,
liveliness, and lease transitions driven only from the explicit monotonic-
time capability with deterministic tie ordering.
**Exit condition (observed):** Compatible endpoints exchange data under
bounded RELIABLE/BEST_EFFORT, VOLATILE/retained, deadline, lifespan, and
liveliness semantics without busy-polling or unbounded history; every
terminal or degradation condition has a distinct deterministic event.
**Gates:** `just fabric_qos_check`
**Evidence:** [`devlog/2026-07-28-c8-5-fabric-qos/`](../devlog/2026-07-28-c8-5-fabric-qos/index.md)

**Depends on:** C8.4.

### C8.6 — Bounded native calls

**Status:** Complete.
**Delivered:** `Call<Request, Reply>` endpoint matching with
generation/session-qualified request identities and a fixed in-flight table
per route, client, and server; inline and shared-sample requests/replies
under distinct client and server authority; one terminal result per
request with bounded cancellation, timeout, and duplicate/stale rejection.
**Exit condition (observed):** Generation-authorized clients and servers
exchange bounded typed requests and replies with exact correlation and one
terminal result; duplicate, timeout, cancellation, rejection, and
peer-fault paths remain isolated and fully reclaimed.
**Gates:** `just fabric_call_check`
**Evidence:** [`devlog/2026-07-28-c8-6-bounded-native-calls/`](../devlog/2026-07-28-c8-6-bounded-native-calls/index.md)

**Depends on:** C8.3 and C8.5's event/time semantics.

### C8.7 — Native operations

**Status:** Complete.
**Delivered:** `Operation<Goal, Feedback, Result>` composed from a bounded
start-goal call, operation-keyed feedback stream, result call, and
cancellation request, with generation/session-qualified operation
identities and bounds on active operations, feedback depth/bytes,
cancellation state, terminal and retained results, retries, and events.
**Exit condition (observed):** Authorized components start, observe,
cancel, and retrieve bounded native operations with exact correlation and
authority; transport outcomes remain deterministic while application and
ROS goal policy stay outside the fabric.
**Gates:** `just fabric_operation_check`
**Evidence:** [`devlog/2026-07-29-c8-7-native-operations/`](../devlog/2026-07-29-c8-7-native-operations/index.md)

**Depends on:** C8.4 and C8.6.

### C8.8 — Filtered introspection and declared interposition

**Status:** Complete.
**Delivered:** A read-only graph introspection service filtered to the
caller's exact generation-declared visibility grants, reporting only
admitted route/schema/contract-kind/match/QoS/event metadata and never a
capability; every recorder, replay membrane, or protocol gateway compiled
into an explicit acyclic route chain whose proxy receives only its
narrowed declared capabilities, with direct bypass endpoints omitted when
interposition is declared.
**Exit condition (observed):** Read-only graph views reveal exactly the
caller's visibility grant, and every declared interposer occupies the only
authorized route path with no ambient discovery, bypass, or widened proxy
authority.
**Gates:** `just fabric_visibility_check`
**Evidence:** [`devlog/2026-07-30-c8-8-filtered-introspection-interposition/`](../devlog/2026-07-30-c8-8-filtered-introspection-interposition/index.md)

**Depends on:** C8.3, C8.4, and C8.6.

### C8.9 — Typed full-profile and resource-bound closure

**Status:** Complete.
**Delivered:** The generation profile and shared-buffer-budget fields
formalized in the generation schema; one named full-graph profile resolved
once, deriving both authenticated graph bytes and the userspace build
profile from that resolved value; every fabric limit consumed by later
slices (queue depth, sample bytes, buffers, mappings, loans, capability
slots) emitted and checked for mutual satisfiability before launch.
**Exit condition (observed):** One typed generation source deterministically
fixes the full fabric profile, normalized schemas, runtime tables, and
satisfiable resource ceilings; host, kernel, and userspace cannot select or
interpret different graph authority.
**Gates:** `just data_fabric_profile_check`
**Evidence:** [`devlog/2026-07-30-c8-9-integration-decomposition/`](../devlog/2026-07-30-c8-9-integration-decomposition/index.md), [`devlog/2026-07-30-c8-9-typed-fabric-profile/`](../devlog/2026-07-30-c8-9-typed-fabric-profile/index.md)

**Depends on:** C8.2, C8.7, and C8.8. C8.7 is named explicitly because
`inFlightOperations`, `retainedSamples`, and `eventDepth` are graph limits its
broker consumes, so operation ceilings cannot be proven satisfiable without it.

### C8.10 — Collision-free full-graph bootstrap and bounded route workers

**Status:** Complete.
**Delivered:** One collision-free fabric-only bootstrap layout launches all
declared C8 roles — stream, call, and operation participants, an unauthorized
probe, a filtered-introspection client, and a declared interposition proxy —
in one generation on the seL4 product path, through a 21-slot collision-free
init layout. Init spawns all nineteen children itself, including both bounded
route workers, because a worker's control endpoints are generation-declared
native Endpoints the root installs before any task runs, and its
participants' supervision handles name tasks only `init` holds (B55).
**Exit condition (observed):** One generation boots every C8 role
simultaneously through collision-free, bounded capability layouts, and each
route worker can block on all of its declared sources without polling or
exceeding kernel limits. The x86 oracle's own earlier version of this
milestone (a 53-of-64 `MAX_CAPS` layout reached by an early return in
`launch_init`) described the retired custom kernel and does not apply to the
seL4 product path — that evidence is historical, following the precedent
P2.2 set.
**Gates:** `just data_fabric_profile_check`, `just data_fabric_boot_check`,
`just sel4_boot_check`
**Evidence:**
[`devlog/2026-07-30-c8-10-route-worker-partition/`](../devlog/2026-07-30-c8-10-route-worker-partition/index.md),
[`devlog/2026-07-31-c8-10-full-graph-boot/`](../devlog/2026-07-31-c8-10-full-graph-boot/index.md),
[`devlog/2026-08-15-b55-full-graph-boot-restoration/`](../devlog/2026-08-15-b55-full-graph-boot-restoration/index.md)

### C8.11 — Unified simulated time and deterministic semantic traces

**Status:** Complete.
**Delivered:** [`contracts/fabric-trace/v1/`](../contracts/fabric-trace/v1/) defines one kind-discriminated
64-byte record covering all ten declared trace families, with sink capacity
and overflow fixed as generation facts (`FabricGraph.traceDepth`/
`traceOverflow`). `just data_fabric_trace_check` observes 100 records across
three timed workers in the declared tie order, inside declared depth with
nothing dropped or rejected, byte-identical across two boots of each plane.
**Exit condition (observed):** Every timed C8 worker drives its records from
one explicit simulated clock and emits one bounded, versioned, deterministic
semantic evidence stream; two boots of each fixed generation produce
byte-identical trace artifacts independent of serial-log interleaving. The
clock stays per-worker rather than one shared source — the three workers are
separate tasks with separate capability tables — and what is required and
observed is that they share one *sequence discipline*, not one endpoint.
Four families (schema, visibility, interposition, and a denial naming an
edge the caller already holds) had validator arms and generated codes but no
emitter yet at this point; their emitters land with C8.12.
**Gates:** `just data_fabric_trace_check`, `just sel4_trace_check`
**Evidence:**
[`devlog/2026-08-15-c8-11-semantic-trace/`](../devlog/2026-08-15-c8-11-semantic-trace/index.md)

### C8.12 — Integrated matching, visibility, and denial matrix

**Status:** Complete.
**Delivered:** `sel4-matrix.zti` (generation 34) declares three routes across
seven distinct identities — exact-tuple pairs, an alternate-name pair that
also probes a name mismatch and a conflicting-type mismatch, an ungranted
probe, the declared interposition proxy, and a read-only filtered-visibility
observer. The incompatible-QoS half is proven at admission: a sibling
generation (35) declares an incompatible offered/requested pair and
`slime-root` refuses the generation before any component launches. The four
C8.11 trace families left without an emitter (schema, visibility,
interposition, denial) all emit here.
**Exit condition (observed):** The simultaneous graph matches only exact
authorized contracts; mismatched and ungranted callers acquire neither route
authority nor protected visibility, and declared interposition remains the
only route path.
**Gates:** `just data_fabric_matrix_check`, `just sel4_matrix_check`
**Evidence:**
[`devlog/2026-08-15-c8-12-matrix/`](../devlog/2026-08-15-c8-12-matrix/index.md)

### C8.13 — Concurrent cross-plane traffic and resource ceilings

**Status:** Complete, with two measured walls recorded rather than claimed
closed.
**Delivered:** `just sel4_traffic_check` (`just data_fabric_traffic_check`)
boots a `"traffic"` action reusing C8.10's exact three-worker partition and
requires the stream, call, and operation planes to run their own bounded
C8.4-C8.9 scenarios concurrently, observably interleaved, including the
QoS-timed stream arm's real RELIABLE retry accounting under concurrent load.
All eleven declared resource classes now emit bounded peak(+baseline)
evidence — C8.13.3 supplied the last, `capabilitySlots`.
**Exit condition (observed):** All C8 transport classes carry concurrent
bounded traffic through one fabric while every declared resource stays
inside its manifest ceiling and returns to its declared baseline. Two arms
are narrower than the deliverable text and recorded rather than claimed:
`resourceEvent` has no emitter — a proven wall, not a gap, since the
`ERR_WOULDBLOCK` it depends on is unreachable through a blocking
`seL4_Send`; and `just sel4_saturation_check` drives only 3 of the 11
declared classes (in-flight calls, in-flight operations, retained operation
results) to their exact declared bound, leaving the rest merely observed
under theirs as remaining work. `resourceLoan` is emitted by the stream
broker rather than the call worker, whose trace sink has zero headroom (62
of 64 records already spent on existing verified evidence). Two declared
fields (`queueDepth` and `capabilitySlots`) were found never checked against
real usage at all; C8.13.3 supplied the mechanism for `capabilitySlots`,
`queueDepth` remains unconsumed.
**Gates:** `just data_fabric_traffic_check`, `just data_fabric_saturation_check`
**Evidence:**
[`devlog/2026-08-15-c8-13-traffic/`](../devlog/2026-08-15-c8-13-traffic/index.md),
[`devlog/2026-08-16-c8-13-queue-history-evidence/`](../devlog/2026-08-16-c8-13-queue-history-evidence/index.md),
[`devlog/2026-08-16-c8-13-saturation-ceilings/`](../devlog/2026-08-16-c8-13-saturation-ceilings/index.md),
[`devlog/2026-08-16-c8-13-qos-timed-traffic/`](../devlog/2026-08-16-c8-13-qos-timed-traffic/index.md),
[`devlog/2026-08-16-c8-13-resource-event-loan-walls/`](../devlog/2026-08-16-c8-13-resource-event-loan-walls/index.md),
[`devlog/2026-08-16-c8-13-declared-fields-audit/`](../devlog/2026-08-16-c8-13-declared-fields-audit/index.md),
[`devlog/2026-08-17-c8-13-3-capability-slot-occupancy/`](../devlog/2026-08-17-c8-13-3-capability-slot-occupancy/index.md)

### C8.13.1 -- Self-reported shared-buffer occupancy evidence (narrow)

**Status:** Complete for the stream broker; `fabric-call-worker` deferred to
C8.13.2 on measured trace-sink saturation.
**Delivered:** A self-scoped, read-only shared-buffer query syscall returning
the caller's own live page/buffer/mapping/loan counts, and a
`resourceMapping` trace code following the existing peak+baseline
held-and-released convention; `fabric-service` samples and emits its own
occupancy under the traffic action.
**Exit condition (observed):** One of the two broker holders with existing
trace infrastructure reports real mapping and loan occupancy for itself: the
loan count traffic-varying, the mapping count constant-by-invariant.
Explicitly scoped as 1 of the traffic fixture's 8 declared holders, not full
coverage. Two premises did not survive measurement: `fabric-call-worker`
cannot emit — its sink holds 62 ordinary records plus its terminal against
the schema's page-sized `maxTraceDepth = 64` (measured `capacity=64
records=63`), the same wall `resourceLoan` hit in C8.13, deferred to
C8.13.2; and `resourceMapping` alone is not traffic-varying — an
instrumented boot read pages 8/8, buffers 7/7, mappings 6/6, loans 0/5, so
the mapping count is fixed at provisioning and `resourceLoan` carries the
varying half. `just sel4_traffic_check` and `just sel4_saturation_check`
observe `SHARED BUFFER OCCUPANCY` (label 30) answering `fabric-service`'s
own live charges: `resourceMapping` (constant 6, asserted nonzero) and
`resourceLoan` (peak 5, drained baseline, asserted nonzero peak and bounded
baseline).
**Gates:** `just sel4_traffic_check`, `just data_fabric_traffic_check`
**Evidence:** [`devlog/2026-08-16-c8-13-1-shared-buffer-occupancy/`](../devlog/2026-08-16-c8-13-1-shared-buffer-occupancy/index.md)

**Depends on:** C8.13.

### C8.13.2 -- Full shared-buffer occupancy coverage across all declared holders

**Status:** Complete for the four holders that have occupancy to report; the
other three are recorded as measured walls rather than pending work.
**Delivered:** Trace infrastructure (sink, declared `traceDepth`, emission
path) for the four uninstrumented holders that hold occupancy:
`fabric-publisher`, `fabric-subscriber`, `fabric-subscriber-b`,
`fabric-publisher-b`, each reporting its own `resourceMapping` occupancy
through C8.13.1's self-scoped query.
**Exit condition (observed):** Five of the traffic fixture's 8 declared
shared-buffer holders report real, bounded occupancy evidence: the stream
broker's mapping and loan counts from C8.13.1, and the four participants'
own pinned mapping counts (1, 1, 2, and 2 regions — one per declared route —
each value pinned by the gate, not merely required nonzero). The other three
are measured walls rather than gaps, each for a different reason:
`fabric-call-client` holds nothing at any sampled point (its only charge is
transient, inside one helper that unmaps and releases before returning, so a
report could only be the degenerate `[0, 0]` the trace schema rules out);
`fabric-call-server` cannot reach a flush (it exits mid-loop via
`slime_rt::exit(0)` on the injected peer-death request, so `run_server`
never returns); `fabric-call-worker` has no trace-sink headroom, unchanged
from C8.13.1. The four reporting holders' mapping counts are steady states
sampled at end-of-script, not invariants held throughout — both subscribers
map and unmap a loan, and `fabric-publisher-b` transiently holds a third
mapping it releases before reporting — because a scripted participant has no
sweep loop to sample from mid-run.
**Gates:** `just sel4_traffic_check`
**Evidence:** [`devlog/2026-08-16-c8-13-2-participant-occupancy/`](../devlog/2026-08-16-c8-13-2-participant-occupancy/index.md)

**Depends on:** C8.13.

### C8.13.3 -- Live per-child capability-slot occupancy

**Status:** Complete.
**Delivered:** A root-side mechanism tracking, per child, how many declared
slots are populated across its lifetime (including post-spawn transfers and
mints), with a high-water mark maintained where each half mutates rather
than sampled when read, exposed through a self-scoped query surface
mirroring C8.13.1's discipline.
**Exit condition (observed):** One holder's declared `capabilitySlots`
ceiling is checked against a live, root-tracked occupancy rather than
compared only to a fixed global `LIMIT_*` constant at decode time. `just
sel4_traffic_check` and `just sel4_saturation_check` observe
`resourceCapabilitySlots` (constant 14) from the stream broker — declared
peak 35, live baseline 29 — checked against the `capabilitySlots = 48` the
fixture declares. Scoped to the stream broker: the four instrumented
participants have sink headroom and could report the same counter, and the
peak sits under the declared ceiling rather than at it, so the bound is
checked but not saturated. Two premises did not survive measurement, both
category errors rather than tuning: a child's slots live in two spaces
(declared logical numbering from 0, validated against `MAX_TASK_CAPS`, vs.
the physical CNode — comparing physical occupancy to the declared ceiling
would have failed a satisfied holder and passed only by coincidence; the
reply now carries both counts checked against their own bounds); and the
peak is the root's to track, not sampled twice by the component, since
declared occupancy genuinely rises and falls (measured peak 33 then 35
against baseline 29) as the broker drops supervision handles it no longer
waits on.
**Gates:** `just sel4_traffic_check`, `just sel4_saturation_check`
**Evidence:** [`devlog/2026-08-17-c8-13-3-capability-slot-occupancy/`](../devlog/2026-08-17-c8-13-3-capability-slot-occupancy/index.md)

**Depends on:** none. An independent root mechanism, not gated by C8.13.1/.2.

### C8.14 — Degradation and fault isolation

**Status:** Complete.
**Delivered:** Every degradation and fault path — stalled, malformed,
denied, retry-exhausted, cancelled, rejected, expired, timed-out,
participant-death, server-death, and declared-interposition-hop-death —
exercised against the concurrent graph, with each condition kept a distinct
semantic record and every in-flight correlation, retained result, loan,
mapping, buffer, endpoint, and route-worker capability settled on peer-loss
and terminal fault paths. This milestone read as eleven paths to build and
turned out to be an assertion milestone: measurement found ten of the eleven
already driven by C8.13's concurrent graph through its existing scripted
scenarios, with none of it previously checked; only the declared
interposition hop dying required new injection (a proxy that relays
correctly cannot also be absent, so a dedicated `sel4-fault.zti` build —
`sel4-traffic.zti` with the proxy compiled to die — is required).
**Exit condition (observed):** `just sel4_fault_check` (`just
data_fabric_fault_check`) observes 4 distinct denial codes, 3 distinct QoS
degradations, 3 peer-death faults across all three planes, 8 isolation
markers, and the injected interposition-hop death, on a graph whose seven
trace sinks stay `dropped=0 rejected=0` and whose resource counters all
return to baseline; every declared degradation and fault path stays bounded,
distinguishable, and fully reclaimed. Distinctness is asserted as
disjointness of codes within a family rather than as presence. Two arms are
narrower than the deliverable text and recorded rather than claimed: the
stream broker reports no `kind=qos` degradation of its own on this plane, so
the QoS-distinctness assertion covers only the call and operation planes;
and the hop death is the only *injected* fault — a stalled subscriber and a
faulting (rather than exiting) participant remain unexercised as injections,
though the scripted peer deaths cover the settlement path either would take.
**Gates:** `just sel4_fault_check`, `just data_fabric_fault_check`
**Evidence:** [`devlog/2026-08-17-c8-14-fault-isolation/`](../devlog/2026-08-17-c8-14-fault-isolation/index.md)

**Depends on:** C8.13.

### C8.15 — Full-graph determinism and parent close

**Status:** Complete.
**Delivered:** The final generation-declared graph and fixed normal, denial,
stall, malformed-input, and peer/proxy-fault schedules composed from the
preceding slices as a *pair of schedules over one graph* rather than a new
fixture (`sel4-fault.zti` is `sel4-traffic.zti` with `generation` changed
and the interposition hop compiled to die); one aggregate QEMU gate invoking
each plane's own gate in-process against each boot, so the aggregate cannot
drift from what the narrow gates require. The audit deliverable found one
real defect, recorded as backlog B56: `just data_fabric_profile_check` had
been red since B55 because it swept the reference manifest's `unified`
profile through a resolver whose per-plane control-grant holder that
manifest structurally cannot satisfy — C8.9's exit condition was therefore
unobserved on the tree the roadmap recorded it against. Fixed and closed.
**Exit condition (observed):** `just sel4_fabric_aggregate_check` (`just
data_fabric_check`) boots both aggregate schedules twice each — four boots
over one declared composition — and compares their `[trace]` records field by
field: 139 records per traffic boot and 140 per fault boot, 279 semantically
identical in total. A
generation-declared graph of isolated native publishers, subscribers,
service clients/servers, operation participants, introspection clients, and
declared proxies exchanges bounded typed data under explicit QoS and graph
grants; denied and incompatible edges are neither usable nor visible, every
resource and fault path stays bounded and isolated, and identical inputs
produce identical semantic traces. What "semantic" covers is declared by the
gate and was narrowed by B75, which measured three rendered fields to be
observations of the run rather than properties of the composition: a resource
record's `high_water` on the ten counters that are poll samples, the
per-instant arrival ordinal `sequence`, and the stream worker's deferred
peer-death instant. Each record's declared position — its instant and tie
class — is still compared positionally, and its remaining content as a
multiset. Determinism is compared over the
C8.11 trace records alone (which carry simulated time and forbid task ids or
addresses), not serial markers, which several legitimately race; the record
count is pinned so a regression that silenced every worker could not pass as
two identical empty transcripts. Two limits are recorded rather than
claimed: normalized schema artifacts are compared for determinism by `just
generation_check`/`just data_fabric_profile_check` on the host, not across
these boots, so the byte-comparison here is over semantic traces alone; and
the denial, stall, and malformed-input schedules are carried inside the two
aggregate boots rather than run as separate arms, each driven and asserted
by `sel4_fault_check`.
**Gates:** `just sel4_fabric_aggregate_check`, `just data_fabric_check`
**Evidence:** [`devlog/2026-08-17-c8-15-fabric-aggregate/`](../devlog/2026-08-17-c8-15-fabric-aggregate/index.md), [`devlog/2026-08-17-b68-aggregate-trace-determinism/`](../devlog/2026-08-17-b68-aggregate-trace-determinism/index.md)

**Depends on:** C8.9–C8.14.

## C9: Robot runtime authority

**Status:** In progress. Decomposed into C9.1–C9.6; C9.1, C9.2, and C9.3 are
complete, so RP5's two named dependencies on this track are closed and scheduling
class is declared, enforced, and non-self-wideable. C9.4–C9.6 are open.

This track supplies the timing, execution, lifecycle, and observation contracts
needed by robot and mixed interactive workloads. It does not promise hard
real-time behavior from QEMU; deterministic state-machine checks precede
measured latency evidence on a named physical target.

**Depends on:** C8 and architecture-portability milestone P5, which supplies the
privileged timer, interrupt, context-switch, and idle mechanisms from upstream
seL4. C9 defines architecture-neutral timer, wait-set, scheduling-class,
lifecycle, and observation semantics above them; `slime-root` owns the bounded
mechanism that brokers them (`slime-root/src/{platform_timer,timer,event,notification,supervision,fault}.rs`).

R2's ROS 2 managed-node and parameter-service compatibility is expected to be
implemented as a profile over C9's lifecycle-transition and parameter-state
schemas rather than a separate ROS-specific state machine; see
[`devlog/2026-08-17-ros2-transport-zenoh-pivot/`](../devlog/2026-08-17-ros2-transport-zenoh-pivot/index.md).

**Motivation:** every mechanism this track needs at the bottom already exists
and none of it reaches a component. `slime-root` claims the one architected-timer
PPI seL4 leaves userspace, binds it to a notification, programs one-shot
deadlines, and drains a bounded deadline queue emitting totally ordered
scheduling events (`slime-root/src/{platform_timer,timer,event}.rs`), and
`just sel4_root_boot_check` observes the whole path in ordered `SLIME_TIMER`
markers. But the live product use of `TimerScheduler` is that startup proof:
no component-visible operation reads a clock or arms a timer, the generated
syscall ABI has no time label, and a component that wants to wait on several
sources today blocks on one generation-declared badged notification and then
sweeps its endpoints by hand. Restart is the same shape — `fault.rs` carries
`Timeout`, `PeerLoss`, and `Unhealthy` terminal states with no production
caller, and nothing restarts anything.

### Architecture decisions

These four fix the shape of every slice below, and two of them record a wall
rather than a plan.

- **The clock is a service, not a register grant.** C9 gates root-mediated
  monotonic reads, timer arm/cancel, and simulated time behind declared
  authority, and a component absent from that authority gets no service. It
  does *not* claim that raw counter reads are impossible: seL4 sets
  `CNTKCTL_EL1.EL0PCTEN`/`.EL0PTEN` **once, globally, at kernel boot**
  (`armv_init_user_access` in
  `deps/sel4/src/arch/arm/armv/armv8-a/64/user_access.c`), never per-TCB, and
  `sel4/config/qemu-arm-virt.cmake` must enable both because `slime-root`
  programs the EL1 physical timer from EL0 itself. The grant is therefore
  all-or-nothing and the root is inside it. Any EL0 code can execute
  `mrs CNTPCT_EL0`; no shipped component does, and no component API exposes it.
  Revoking the grant would leave the root with no timer at all — PPI 30 is the
  only architected-timer PPI seL4 does not claim under
  `KernelArmHypervisorSupport ON` — so closing this is a kernel-side question,
  not a config edit. C9's clock authority is consequently an *authority over
  the service and its semantics* (simulated time, deadline ordering, recorded
  determinism), and this paragraph is the boundary that claim is read against;
- **Scheduling classes rest on priority, and budgets stay undeclarable.**
  Class assignment maps to seL4 TCB priorities, which are already declared
  generation data and enforced per B48 (`Instance.priority`,
  `Instance.workerPriority`, `SLIME_GRAPH schedule`). Conserved CPU accounts,
  budgets, and periods are **out of C9's scope** while `KernelIsMCS OFF`:
  non-MCS seL4 has no budget to charge, and `ScheduleRecord`'s `budget_us` and
  `period_us` are now *refused* rather than merely written zero — B77 made both
  validators reject a nonzero value (`UndeclarableCpuBudget` from the host
  oracle, `DecodeError::NonZeroReserved` from `Generation::validate`), so no C9
  contract can add a field that pretends otherwise even by accident. Admitting
  MCS is an assurance decision, but the terms are narrower than "trade a
  verified kernel for a scheduling feature": upstream `deps/sel4/CAVEATS.md`
  lists AArch64 MCS proofs as *in
  progress* (RISC-V MCS is already verified), and the QEMU build this
  repository develops against is **already** outside the verified set — it sets
  `KernelVerificationBuild OFF`, `KernelDebugBuild ON`, and `KernelPrinting
  ON`, and `qemu-arm-virt` appears in no verified-platform list, while the
  RPi 5 config does include upstream's `AARCH64_bcm2712_verified.cmake`. So the
  cost of MCS is not uniform across the two platforms this repository builds,
  and the honest framing is per-target rather than global. A budgeted-CPU slice
  is still blocked on that decision, not on C9;
- **Wait sets are userspace, over B46's primitives.** The root already deleted
  its `WaitSet` when B46 replaced logical channels with native seL4 Endpoints
  and badged Notifications. C9 does not restore a root-owned wait set: a
  bounded ready queue with deterministic tie rules is `slime-rt` code over
  `notification_wait` and per-endpoint receive, and the root's contribution is
  the timer source that lets such a queue block on time as well as messages;
- **Restart is a userspace supervisor over root mechanism.** `slime-root`
  observes termination, holds single-assignment terminal state, and reclaims;
  it does not decide that something should run again. Attempt bounds, backoff,
  health dependencies, and lifecycle transitions are generation-declared policy
  executed by a component holding supervision authority, so the root gains no
  restart policy and no notion of a component's health;
- **C9's authorities are rights, and the matrix changes with them.** Unlike C10
  — which added no object kind and no right, so
  `../docs/capability-matrix.md` stayed unchanged — C9.1's monotonic, timer,
  and simulated-time authorities and C9.3's promotion authority each gate a
  root operation on a *named* authority a generation grants, which is what a
  right is. Each is therefore a new bit in `contracts/generation/v5`'s rights
  vocabulary with a matrix row, landing in the same change as the operation and
  gate it guards, as invariant 4 in [`README.md`](README.md) requires. C9 adds
  no new seL4 object kind: the timer is root-brokered state keyed by task, not
  a capability a component can name, transfer, or derive.

### Sequence

1. C9.1 exposes the existing root timer as declared component authority.
2. C9.2 builds bounded userspace wait sets over C9.1's timer source and B46's
   notifications.
3. C9.3 makes scheduling class declared, enforced, and non-self-wideable.
4. C9.4 adds lifecycle transitions and supervised restart with fresh authority.
5. C9.5 adds typed recording and deterministic replay.
6. C9.6 composes the sensor → controller → actuator workload that exercises all
   five under contention and an injected restart.

### C9.1 — Explicit clock and timer service authority

**Status:** Complete.
**Delivered:** A versioned Zutai `clock-authority/v1` generation resource and
five generated root-service operations grant monotonic read, timer use,
simulated read, and simulated advance independently. `slime-root` brokers the
existing physical timer through per-live-task authority and timer quotas,
delivers every already-decided one-shot expiry on each holder's declared
Notification and badge even if a later platform step fails, separates timer
wakes from the bounded component-request iteration count, distinguishes
malformed requests from absent authority, and reclaims live timers on
termination. The contract records the platform wall rather than
overclaiming it: current AArch64 seL4 profiles grant physical counter and timer
register access globally, so these rights gate the service semantics, not
hostile native register access.
**Exit condition (observed):** `just clock_authority_check` boots generation 41
with separate monotonic, timer, simulated-reader, simulated-advancer, and
denied instances; root-attributed evidence observes advancing monotonic time,
cancellation without delivery, per-task quota refusal, one-shot expiry through
badge `0x200`, simulated time changing only under its advancer, every undeclared
operation refused, malformed length distinguished, and one remaining live timer
dropped at the timer holder's exit while the root timer phase remains healthy;
`just test_sel4_root` additionally observes that an IRQ acknowledgement failure
cannot discard the expiry transition it follows.
**Gates:** `just clock_authority_check`, `just sel4_boot_layout_check`, `just sel4_gate_control_check`, `just contracts_check`, `just generation_check`, `just test_sel4_root`
**Evidence:** [`devlog/2026-08-24-c9-1-clock-authority/`](../devlog/2026-08-24-c9-1-clock-authority/index.md)

**Depends on:** P5's timer mechanism, observed by `just sel4_root_boot_check`.

### C9.2 — Bounded userspace wait sets and executors

**Status:** Complete.
**Delivered:** A versioned Zutai `wait-set/v1` generation resource declaring, per
waiter, which badge bit on its one Notification means which source kind and which
of its own slots to drain — data a waiter cannot compute, because `slime-root`
derives a signaller's badge from the *signaller's* declared slot and C9.1's timer
badge is contract data independent of any slot. `boot-contracts` decodes it and
owns the bounded state machine (registration ordered by badge, demultiplexing, the
ready queue, bounded dispatch); `slime-rt`'s `wait_set` is the shell that reads the
declared sources over one new self-scoped root operation (`WAIT_SOURCES`, label 49)
and blocks on the Notification. Three producers write that one word — a declared
peer signaller, C9.1's timer badge, and a new root-signalled supervision badge —
and both the host builder and root admission enforce that a badge belongs to
exactly one of them. The root gains no wait set, no ready queue, and no source
registry: its only addition is the death signal a peer cannot send, gated on the
waiter's own declared slot still holding a supervision capability naming the dead
task, so the badge adds a wake rather than authority. The ready-queue ceiling is
*proven* rather than enforced — register's badge dedup plus `MAX_READY ==
MAX_SOURCES` makes overflow unreachable, so a ceiling error there would be the
dead guard B76 removed — and the wait set's tables stay fixed at the contract's
per-waiter ceiling rather than allocated from the C10 region, because 216 bytes is
not the 29960 C10.4 removed and a `Vec` would put an allocator in every component
that waits.
**Exit condition (observed):** `just wait_set_check` boots generation 42 and
observes a waiter register three sources on one Notification (`mask=0x20208` —
bits 3, 9, 17), refuse a duplicate badge, an undeclared badge, and an over-budget
dispatch while remaining usable, then recover two independently signalled sources
from a single coalesced badge word (`wake ready=2 dispatched=2`, the widest single
poll) and dispatch them in ascending badge order, receive the timer expiry on a
later block through that same Notification, and cover all three sources in two
dispatching passes; the root delivers the peer death itself
(`SLIME_WAIT death task=4 woken=1`), an instance the resource does not name reads
zero sources and registers nothing, and the graph closes
`SLIME_GRAPH HEALTHY generation=42 required=4 live=0 completed=4 failed=0`. Two
review rounds found five issues — a mispinned marker count that aborted the
gate-control negative control, a tie rule resting on a false ordering premise, an
unreachable ceiling error, and two vacuous plane assertions — all applied, with
the tie rule's two failure sequences now pinned by host tests.
**Gates:** `just wait_set_check`, `just sel4_gate_control_check` (37 gates, 1415
mutations), `just sel4_boot_layout_check` (28 plane layouts), `just contracts_check`,
`just generation_check`, `just test_sel4_root` (160), `just test_host` (250)
**Evidence:** [`devlog/2026-08-25-c9-2-bounded-wait-sets/`](../devlog/2026-08-25-c9-2-bounded-wait-sets/index.md)

**Depends on:** C9.1's timer source and B46's native Endpoint/Notification
mechanism.

### C9.3 — Declared scheduling class

**Status:** Complete.
**Delivered:** A versioned Zutai `scheduling-class/v1` generation resource
declaring three things together: the band mapping from each class —
`foreground`, `normal`, `bestEffort` — to its exact seL4 TCB priority, the
per-instance assignment, and the promotion edges naming who may change whose
class. The mapping is manifest data rather than a constant compiled into the
builder and a matching one in the root, which is the deliverable: the builder
reads it **once** and substitutes the resulting priority into the v5
`ScheduleRecord` it already emits, so a class *is* the priority and there is no
second number for the two readers to disagree about. An instance declaring both
a class and a disagreeing priority is refused at build — including a
`workerPriority` that disagrees by inheritance — and the root re-derives what
only the generation knows: every named identity is a declared instance, and every
promotion subject is owned by its holder. "Never its own" is closed three ways —
the decoder refuses a declared self-edge, `SchedulingService::promote` refuses a
caller equal to its subject before any edge lookup, and the subject is named by a
supervision capability the root mints only for a spawner, never for a task
itself. `RIGHT_SCHEDULING_PROMOTE` (bit 30) rides on that handle exactly where
the policy declares the edge, so the right and the edge are one fact with one
source. An instance the policy does not name reads back as a distinct
`undeclared` class at the root's own child priority — not as `normal`, because
naming a band the thread is not in would make promoting it *to* that band look
like a no-op while silently moving its priority. CPU quantity stays bounded by
nothing: `budget_us`/`period_us` remain zero and undeclarable under B77.
**Exit condition (observed):** `just scheduling_class_check` boots generation 43
and observes, in one transcript, a `foreground` component making ordered progress
*between two chunks of a `bestEffort` component's still-running 200M-iteration
burn loop* on one vCPU — the burner then finishing, so the band orders CPU access
rather than denying it — plus a declared promotion applying at its band, the same
edge refused one band above its declared ceiling, `undeclared` refused as a
target, a self-directed promotion refused with the promoter's own class unchanged,
and an unnamed instance reading `undeclared` at 254 while refused all eight swept
slots. Every `SLIME_SCHED class` line is cross-checked against the
`SLIME_GRAPH schedule` record the builder wrote for the same thread, all four
instances, which is the pair three review rounds found disagreeing. Restart
survival is C9.4's to observe, so C9.3 closes without it.
**Gates:** `just scheduling_class_check`, `just sel4_gate_control_check` (38
gates, 1450 mutations), `just sel4_boot_layout_check` (29 plane layouts), `just
contracts_check`, `just generation_check`, `just test_sel4_root` (170), `just
test_host` (265)
**Evidence:** [`devlog/2026-08-25-c9-3-declared-scheduling-class/`](../devlog/2026-08-25-c9-3-declared-scheduling-class/index.md)

**Depends on:** B48's per-thread declared priority. Explicitly **not** MCS: see
the architecture decision above.

### C9.4 — Lifecycle transitions and supervised restart

**Status:** Not started.

**Depends on:** C9.1 (backoff needs a clock), C9.3 (class must survive
restart), and the root's existing termination observation in
`slime-root/src/{fault,supervision}.rs`. Both C9 dependencies are now
**complete**, so nothing on this track gates C9.4. C9.3 deliberately left it the
restart-survival check: `SchedulingService::release` drops a dead task's row so a
restarted instance re-derives its class from the generation rather than
inheriting one, but that is a property of the code and not yet an observation.

#### Deliverables

- define lifecycle transitions, health dependencies, restart attempt bounds,
  backoff policy, and parameter state as versioned Zutai schemas, promoting
  `ComponentSpec.lifecycle`'s declarative state list into an admitted
  transition graph;
- implement restart as a userspace supervisor holding declared supervision
  authority: it observes a termination, consults declared policy, and requests
  a fresh spawn; the root gains no restart policy;
- reissue every authority on restart — endpoints, mappings, buffers, timers,
  device grants — so a restarted component cannot reach a predecessor's state,
  and give the root the mechanism to prove staleness rather than assume it;
- give `fault.rs`'s existing `Timeout`, `PeerLoss`, and `Unhealthy` terminal
  states production callers, or delete them; they are currently dead public API
  with no caller and no test — `fault.rs`'s four tests exercise `ipc_completed`,
  `fault`, and `exit` only;
- make parameter state an *authority*, not merely a schema: a component holding
  no parameter grant cannot read or write another's parameters, which is what
  C9.6's exit condition means by "parameter authority" and what the
  pre-decomposition C9 gated in its first required check.

#### Required checks

- a component killed by fault, by exit, and by declared unhealthiness each
  restarts under its declared policy, and the three are distinguishable;
- restart attempts are bounded and backoff is observed against C9.1's clock,
  not a spin count;
- a restarted component cannot use any predecessor endpoint, mapping, buffer,
  or timer, and an attempt is refused rather than ignored;
- a component whose declared health dependency is down is not started, and is
  started when the dependency recovers;
- restart preserves declared scheduling class and private-memory quota;
- exhausting the attempt bound leaves the graph in a declared terminal state
  rather than restarting forever;
- a component holding no parameter authority cannot read or write another
  component's parameters, and its refusal is distinguishable from a missing key.

#### Planned verification target

```sh
just lifecycle_restart_check
```

#### Exit condition

A supervisor restarts a failed component under generation-declared attempt and
backoff policy, the restarted instance holds entirely fresh authority and its
original class and quota, stale predecessor capabilities are refused, parameter
state is reachable only with declared parameter authority, and attempt
exhaustion terminates deterministically.

### C9.5 — Typed recording and deterministic replay

**Status:** Not started.

**Depends on:** C9.1's clock authority, C9.2's deterministic dispatch order,
and C8.11's trace contract.

#### Deliverables

- record declared fabric routes, clock reads, timer expiries, and lifecycle
  transitions as a typed bounded trace, reusing C8.11's record shape rather
  than a second trace format;
- classify every nondeterminism source as capability-recorded or excluded from
  the deterministic claim, and refuse a generation that declares a component
  deterministic while granting it an unrecorded source;
- replay a recorded trace into a component and compare its typed outputs
  field by field, following C8.15's semantic-comparison pattern;
- bound recorded trace bytes before allocation.

#### Required checks

- a deterministic component replays a recorded trace to byte-identical typed
  outputs across two boots;
- a component granted an unrecorded nondeterminism source cannot be declared
  deterministic;
- a truncated or reordered trace is refused rather than partially replayed;
- the trace ceiling is refused structurally.

#### Planned verification target

```sh
just replay_check
```

#### Exit condition

A component declared deterministic reproduces identical typed outputs from a
complete recorded trace of its routes, clock reads, and lifecycle transitions,
and a generation granting it unrecorded nondeterminism is refused.

### C9.6 — Robot workload composition

**Status:** Not started.

**Depends on:** C9.1–C9.5.

#### Deliverables

- build a simulated sensor → controller → actuator graph over the native
  fabric, with no privileged special-casing, exercising timer, stream, call,
  lifecycle, restart, and contention paths;
- run it under CPU contention from a declared best-effort load and an injected
  controller restart;
- assert the composed envelope the way C8.15 does: one composition, both
  schedules, compared semantically rather than by marker presence alone.

#### Required checks

- the graph runs to completion under contention with declared scheduling order
  preserved;
- an injected controller restart is bounded, reissues authority, and the graph
  resumes;
- deadline miss, timer expiry, liveliness loss, fault, peer loss, and
  cancellation remain distinct at the userspace boundary;
- the semantic corpus is architecture-neutral: no GIC identifier, AArch64
  register frame, or physical address appears in a C9 contract or trace record.

#### Planned verification target

```sh
just robot_runtime_check
```

#### Exit condition

A simulated sensor/controller/actuator graph runs through the native fabric with
explicit time, scheduling, lifecycle, and parameter authority; under CPU
contention and an injected component restart it remains bounded, preserves the
declared scheduling order, restores fresh authority, and reproduces its typed
outputs from a complete recorded input trace.

## C10: Bounded private component memory

**Status:** Complete — C10.1 through C10.4.

**Depends on:** C7's per-holder quota, supervision-subtree accounting, and
reclamation pattern, and backlog item **B9** (resolved 2026-07-28), whose task
teardown/reclamation path C10.1 extends. C10 does not consume C8 or C9 and may
proceed in parallel on the `aarch64-sel4-qemu-virt` product path.

**Motivation:** A component's working memory is fixed at build time. Its stack
size comes from the `SLIMECME` image header's `stack_bytes` field — bounded by
`MAX_STACK_BYTES` and validated in `boot-contracts/src/component_image.rs` —
and its `.data`/`.bss` from the linked ELF; `slime-rt` (`components/runtime`)
installs no `GlobalAlloc`, and no root operation yields a page, so `Vec`, `Box`,
and `String` are unavailable to a native component. Every buffer is therefore
sized for its worst case in every generation that carries that component. A
build service, a filesystem index, and a bounded introspection reply all need
memory proportional to their input, and none can be written under that
constraint. B70's closure is the standing evidence that this is not theoretical:
sizing three brokers' fixed arrays from contract ceilings overflowed the 64 KiB
component stack, and because `.data` sits directly below the stack it presented
as a corrupted `static` and a wild jump rather than as a stack overflow.

The shared-buffer plane is not that mechanism and must not become it. It exists
to move samples *between* components: every region is a nameable, transferable,
loanable object that `slime-root` retypes from one untyped region and tracks
under a 256-page root-wide ceiling
(`slime-root/src/shared_buffer.rs::MAX_TOTAL_PAGES`). Working memory is private,
never transferred, never sealed, and need not come from a single retype.
Overloading one onto the other would attach transfer and loan semantics to a
heap and force whole-region retypes on every allocation.

### Architecture decisions

- component working memory is **one task-private region at a fixed base**, grown
  only at its tail. Native ELF component images link at a fixed VA and hold real
  machine pointers, so a growth that relocated the base would invalidate every
  live pointer; growth past the reserved window fails instead of moving;
- the region is **reserved as address space in the child VSpace at spawn
  (`slime-root/src/child_vspace.rs`) and backed page by page on demand**. Each
  page is a 4 KiB frame the root retypes individually; they need not be
  contiguous and need not come from one untyped region;
- growth is **authorized by a generation-declared page quota, not a capability**.
  The region is not nameable, transferable, loanable, sealable, or shareable, so
  there is no object for a capability to designate; the authority question is
  how many pages a component may hold, which is a budget. This mirrors the
  stack, which is generation-sized and needs no capability, and it leaves
  `../docs/capability-matrix.md` unchanged: C10 adds no seL4 object kind, no
  root-tracked object, and no right;
- the quota is **deny-by-default**. A component absent from the budget resource
  grows nothing, exactly as an absent shared-buffer holder allocates nothing;
- pages are **always user/read-write/no-execute**, preserving W^X. No growth,
  admission, or compilation path may derive an executable mapping from them;
- the root exposes **growth only** — no `malloc`, `free`, arbitrary `mmap`,
  file-backed or executable mappings, and no second region. `free` is a
  userspace free-list operation; the frames return to the root's allocator when
  the task dies, through the same task-arena revocation B9 established;
- allocation policy lives **entirely in `slime-rt`**. The root tracks a page
  count and never an allocation.

This is the WebAssembly linear-memory split — a runtime that grows bounded,
zero-filled pages under a host-enforced limit, and a language runtime that
allocates inside them — with one deliberate divergence. WebAssembly programs
address memory by offset, so a runtime may relocate the base on growth; native
ELF component code cannot, so the base is pinned and the reservation is fixed.

### C10.1 — Task-private growable memory mechanism

**Status:** Complete.

**Delivered:** One task-private region per child: a 2 MiB window — one AArch64
level-2 span, so it costs one extra leaf table and no more — reserved as address
space and translation tables when the VSpace is built
(`slime-root/src/child_vspace.rs::private_window`, a guard granule above the
thread pages and aligned to its own span), backed page by page through
`LIFECYCLE PRIVATE MEMORY GROW` (label 43, `contracts/syscall-abi/v1`), which
answers the previous page count plus the window base so an allocator neither
needs a second call nor recomputes the loader's arithmetic. The mechanism is
`slime-root/src/private_memory.rs`: a per-task `Region`, a root-wide `Table`,
and five distinct refusals. No seL4 object kind, no root-tracked object, and no
right — `../docs/capability-matrix.md` is unchanged, because the authority
question is how many pages a task may hold, which is a budget.

Two pre-existing accounting asymmetries had to close first, neither visible
until something allocated from a task arena *while the task ran*:
`CleanupRecord.slots` was a construction-time snapshot that had diverged from
what the revoke returned, and `ArenaRecord::push_slot` had no inverse, so a
part-way failure could not return its CSlots. The second was found twice — in
the unwind loop, and again where the retype succeeds and the mapping fails.

The generation-declared budget is **C10.2's**, so every declared instance and
every spawned child sits at deny-by-default zero and the live evidence runs on
the root's embedded fixture against a four-page ceiling — the same situation
`SHARED_QUOTA` records for the C7.3 shared-buffer phase.

**Exit condition (observed):** `just sel4_root_boot_check` boots the fixture
graph and observes, in order: a size query answering `pages=0 base=0x400000`
without allocating, two growths reaching `quota=4` with both new pages read as
zero at that base, a pattern written before the second growth still readable
after it (`survived=0x4d454d5f42415345`), the next page refused
`cause=quota detail=QuotaExceeded { pages: 4, delta: 1, quota: 4 }` with the
caller alive to see `-5`, the region unchanged afterwards, and
`enforced quota=4 pages=4 grants=2 grown=4 reclaimed=0` — two grants, so
neither query nor the refusal charged a page — then
`teardown grown=4 reclaimed=4 pages=0`. Each half is proven non-vacuous by
injection: deleting the quota bound fails the gate naming
`missing 0x60 of 0x7f`, and re-backing from the base instead of the tail fails
it naming `missing 0x50 of 0x7f`. Frame exhaustion mid-growth remains reasoned
and unit-tested rather than observed; it needs a fault-injection seam B61
records this repository lacking.

**Gates:** `just sel4_root_boot_check` (58 ordered markers), `just test_sel4_root`
(146 host tests across 16 modules), `just sel4_gate_control_check` (33 gates,
1295 mutations).

**Evidence:** [`devlog/2026-08-23-c10-1-private-memory-mechanism/`](../devlog/2026-08-23-c10-1-private-memory-mechanism/index.md)

**Depends on:** B9 (resolved).

### C10.2 — Generation-declared private-memory budget

**Status:** Complete.

**Delivered:** `contracts/private-memory-budget/v1` — a sibling of
`shared-buffer-budget/v1` rather than a fifth column on it, since the two bound
unrelated mechanisms and most components use one and not the other. Same shape:
32-byte header, sorted-unique 36-byte entries, 32-holder bound, carried as a
`KIND_RESOURCE` object authenticated by the generation's existing digest table.
Its holder identity has its own domain tag (`slime-private-memory-holder-v1`),
so an identity computed for one budget can never be replayed in the other, and a
host test asserts the two never collide on the same name. The schema also
publishes the root's `regionPages`/`totalPages` ceilings, which
`slime-root/src/private_memory.rs` pins against its own constants with
`const _: () = assert!` — a drift between builder and root is a build failure
rather than a boot-time refusal.

Validation is eager and closes the whole generation:
`generation::private_memory_budget_admission` runs inside `Admission::admit`,
before any component launches, and refuses both a quota above the per-task
reservation and B8's aggregate case where holders each fit but cannot all peak
at once. A *malformed* budget fails the generation too, which is deliberately
asymmetric with the C7.3 path that treats one as absent: deny-by-default makes an
undecodable shared-buffer budget harmless, but a private-memory budget that
silently read as absent would be indistinguishable from a quota a component was
promised and never got, and the boot would look healthy.

Both launch paths install the declared ceiling — resolved *before*
`TaskTable::create`, not after it, because `create` feeds the quota into the
arena plan and an arena is fixed at construction. `build-generation.py` mirrors
every rule host-side and refuses a manifest declaring holders without the
resource object, or the object without holders.

**Exit condition (observed):** `just private_memory_check` boots
`sel4-private-memory.zti`, which declares one executable twice — as a granted
holder and an omitted one. The gate reads the declared quota out of the fixture
rather than restating it, and the probe discovers its own ceiling by growing one
page at a time until refused, so the assertion is a measurement against the
generation rather than two copies of a constant. Observed: `init` and the omitted
holder at `declared=0 installed=0 base=0x0` (no window at all, not merely a zero
quota), the granted holder at `declared=3 installed=3 base=0x400000`, a size
query answering without allocating, three growths, every fresh page read as zero,
a pattern written before the second growth still readable after the third,
`cause=quota detail=QuotaExceeded { pages: 3, delta: 1, quota: 3 }`, the region
unchanged after the refusal, and the omitted holder refused its first page by
`cause=reservation`. Every page charged is attributed to a holder by resolving
each growth's task id through the root's own quota records, so the right total
charged to the wrong holder fails.

"A generation declaring no budget at all boots with every component denied" is
asserted on `just sel4_component_graph_check`, which boots one: the root reports
`SLIME_MEM budget holders=0 declared=0`, and two failure markers reject any
installed ceiling or served growth on that plane. Three injections proved the
evidence non-vacuous: lowering the fixture's `pageQuota` 3 → 2 moved the measured
ceiling to 2; making the root ignore the budget failed the gate with `declared=2
installed=2` on `init` and on the omitted holder; and a hand-built transcript
charging the omitted holder the granted holder's pages is refused by name.

**Gates:** `just private_memory_check` (11 markers across 4 causal chains),
`just sel4_component_graph_check` (31 markers), `just test_sel4_root` (149 host
tests), `just test_host` (10 new decoder tests), `just contracts_check`,
`just generation_check`, `just sel4_gate_control_check` (34 gates, 1318
mutations).

**Depends on:** C10.1.

### C10.3 — Userspace allocator and live quota evidence

**Status:** Complete.

**Delivered:** `components/runtime/src/private_heap.rs` — a `GlobalAlloc` over
the task-private region: a first-fit free list in address order, coalescing on
both boundaries when a block returns, with a growth appended at the tail so
`release` merges it into the trailing free block rather than fragmenting it.

A second allocator rather than a configurable one. `#[global_allocator]` is a
single symbol per link and the choice belongs to the component, so `slime-rt`
now carries two mutually exclusive features and `lib.rs` refuses both with a
`compile_error!`. CP3's store-plane bump allocator stays exactly as it was: its
premise is that nothing outlives the component, which makes a free list pure
cost. A component bound by a *declared ceiling* is the opposite case — its bound
is a small policy number a generation chose, and reuse is the only way to keep
running under it. Both the builder and `lint_sel4_root` gained a third group for
the same reason they already had a second: Cargo unifies features across every
package in one invocation, and here that unification does not merely over-link,
it fails to compile.

Batching is userspace policy over a per-page ABI. `GROWTH_PAGES` is four
granules, and `grow` retries at the exact size when a batch is refused — a
component with three pages left must not be denied an allocation its ceiling can
still serve, so batching stays an optimization rather than a lowered ceiling.
The declared operation still counts single pages, so changing the policy changes
no contract and no fixture.

**Exit condition (observed):** `just private_memory_check` now boots four
application instances over one budget: C10.2's pair growing raw pages, and
C10.3's `private-heap-probe` twice — once named in the budget at 24 pages, once
omitted. The granted instance ran `Vec`, `Box`, and `String` across
reallocations, took 22 of its 24 pages in four batched growths (`4+4+5+9`), read
back every element after the reallocations that crossed a growth, then freed
everything and reallocated a comparable amount with the root serving *no*
further page; it was then refused a deliberate over-ceiling request
(`cause=quota detail=QuotaExceeded { pages: 22, delta: 257, quota: 24 }`),
stayed alive, reallocated after the refusal to prove the heap was not poisoned,
and reported. The omitted instance found no region at all: `denied pages=0
growths=0 refused=1`.

Reuse and batching are asserted from the *root's* `SLIME_MEM grown` records, not
from the component's own counters — a first draft asserted reuse from the probe's
`reuse_growths` field, which an allocator that lost its freed spans and
under-counted itself would satisfy. The probe now brackets its reuse phase with a
console line and the gate requires zero growth inside that window. Five
injections proved the new evidence non-vacuous: a growth served inside the reuse
window, a first growth of one page, a removed boundary line, and either new
instance's missing ceiling record are each refused by name.

"A zero-quota component is byte-identical to its pre-C10 build" holds
structurally rather than by measurement: `private-heap` is opt-in per crate, and
the one crate that declares it is new, so no pre-existing component's image
changes at all.

**Gates:** `just private_memory_check` (19 markers across 6 causal chains),
`just component_crate_split_check` (two allocator groups, each matching the
builder's), `just sel4_gate_control_check` (34 gates, 1331 mutations),
`just lint_all`, `just test_sel4_root` (149 host tests).

**Depends on:** C10.2.

### C10.4 — Adoption, reclamation, and leak evidence

**Status:** Complete.

**Delivered:** `fabric-service` — the graph's own broker, in ten shipped
fixtures — now sizes its role and frame tables from the participant rows the
generation declared instead of from the contract's ceilings. Static footprint
falls 29960 bytes, `.bss` plus `.data` 145912 → 115952 on `sel4-boot`, and the
largest declared graph takes 4 of its 16 declared pages.

The first *product* component on the private region, which is the point: C10.3
proved the mechanism with a probe, and a mechanism only becomes load-bearing
when something that ships depends on it. B70 is why this component was the
right one — sizing three brokers' fixed arrays from contract ceilings overflowed
the 64 KiB stack and presented as a corrupted `static`, so `.bss` was where
those tables went. `.bss` fixed the corruption but not the cause: the
reservation was still the contract's worst case in every generation, and not one
of the ten declares it.

**What the conversion cost, and what it bought.** Removing a fixed array
removes a bound the compiler was enforcing, and two demands the old
`[Frame; 32]` had silently absorbed had to become explicit. A `retained`
publisher pins its own `retainedDepth` frames *concurrently* with every
subscriber's queue, and `provision_edge` floors each subscriber's history at
`MIN_RING_SLOTS`. Both were found in review; each would have presented as the
deadlock the frame bound exists to make unreachable rather than as a refusal.
Admission and storage are now separate figures — the unfloored declared sum is
what the ceiling admits, so no toolchain-certified graph is refused, and the
floored sum is what is allocated. The builder's `ring_capacity` sums the same
two terms, because a builder admitting a wider set than the component is a
generation the toolchain approves and the graph's own holder then kills.

**Exit condition (observed):** `just dango_check` drives five scripted lines
through Dango's profile — three launches, one denied at resolution, one parse
error — the third launch repeating the first. `SLIME_ROOT reclaim
census` publishes the allocator's own watermarks at each reclamation, and the
repeat's census equals the previous cycle's exactly — `slots=2799
bytes=527489392 live_objects=302` — with `arena_reuses` advanced, which is what
proves the repeat took the released arena rather than a fresh one that cost the
same. Read from watermarks rather than from the counts the root tracks, because
B9's thirteen-frame-per-spawn leak was invisible to every counter the root
printed and they all agreed with each other throughout.

`just private_memory_check` adds a holder the generation names in *both*
budgets. It exhausts each plane and uses the other, and is refused a shared
buffer mapped at its own private window's base — `SLIME_MEM mapping refused
task=1 base=0x400000 end=0x401000 window=0x400000..0x600000` — while the same
buffer maps, reads back, seals, unmaps, releases, and has its allowance reused
outside it. The address space was the two planes' last shared resource; the
window is reserved space whose frames arrive on demand, so an address the
allocator has not yet grown into was simply unmapped and a buffer landing there
would have been indistinguishable from heap.

**Gates:** `just private_memory_check` (22 markers, 7 chains), `just
dango_check` (16 markers), `just sel4_stream_check` / `sel4_qos_check` /
`sel4_traffic_check` / `sel4_visibility_check` / `sel4_matrix_check` /
`sel4_call_check` / `sel4_operation_check` (the converted component under real
traffic on every plane that carries it), `just system_spec_check` (20
mutations), `just component_spec_check` (43), `just sel4_gate_control_check`
(34 gates, 1338 mutations), `just test_sel4_root` (152).

**Depends on:** C10.3.

### Exit condition

A generation declares per-component private-memory ceilings; components allocate
dynamically sized working data through ordinary language collections inside
them; growth is zero-filled, pinned at a fixed base, never executable, and
fails closed on every bound; the region is invisible to every capability and
transfer path; and a repeated spawn/exit workload returns every page, leaving
the frame allocator where it started.

## Core verification stack

Each slice runs its narrowest QEMU target. Changes to generation v3 or IPC schemas additionally run:

```sh
just contracts_check
just generation_check
just fmt_check
just lint
just fmt_check_components
just lint_components
```

No core-runtime result by itself claims ROS wire interoperability, a physical-board boot, or physical real-time performance. Architecture-qualified releases additionally satisfy the corresponding P4 or P5 gate.
