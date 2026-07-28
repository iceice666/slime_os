# Core runtime track

**Status:** C7 complete; its two blocking audit findings are resolved. A 2026-07-26 audit of C7.1–C7.7 found a C7.5 full-graph boot wedge (backlog B3) and a shared-buffer plane that was dormant on the live boot path (backlog B4); both are fixed, and every gate — including `just transfer_check`, `just spawn_service_check`, and `just dango_check` — is green. The live boot now declares a real per-holder budget, mints the factory through generation grants, and two components prove their quotas at startup. Remaining C7 debt is evidence and hygiene, not capability: B5 (no test drives the syscall layer; C7.7 composes owner ids rather than tasks), B6, B7, and B8 in `roadmap/00-backlog.md`. C8 may open. See `devlog/2026-07-26-c7-audit/`, `devlog/2026-07-26-b3-shared-buffer-table-stack-overflow/`, and `devlog/2026-07-26-b4-live-shared-buffer-budget/`.

This track turns the existing bounded channels, capabilities, components, and generations into a native typed communication runtime. It is local-first: C7 and C8 require no network or physical driver, and they do not wait for unrelated display, audio, wireless, or GPU work.

ROS 2 compatibility in [`03-ros2-compatibility.md`](03-ros2-compatibility.md) is a userspace profile over this runtime. The kernel never learns nodes, topics, services, actions, graph discovery, message types, or transport QoS policy.

## Boundaries

- Kernel IPC remains a small control plane. The current 64-byte message bound is not enlarged for sensor or image data.
- Bulk samples live in bounded shared buffers referenced by typed control messages.
- Component working memory is task-private and non-transferable. Shared buffers carry samples *between* components; they are not a general allocator, and neither mechanism may be reinterpreted as the other.
- Topic names and types are userspace metadata. Authority is carried by SEND/RECV endpoint capabilities minted or distributed by the declared fabric service.
- The generation declares which component may publish, subscribe, call, serve, inspect, or administer each graph edge.
- `TransportQoS` controls message delivery. `SchedulingClass` controls CPU ordering. They are separate contracts and namespaces.
- Slime capability transfer is native-only. A protocol gateway may retain and proxy a capability but may never serialize a kernel capability as application data.
- Capability, IPC, shared-sample, schema, QoS, lifecycle, and scheduling-policy semantics are architecture-neutral. Trap registers, syscall entry, context switching, page tables, interrupt controllers, and timer mechanisms belong to [`07-architecture-portability.md`](07-architecture-portability.md).
- C7 and B2 continue on the x86-64 reference path. New low-level work must not add uncontained x86 assumptions outside the architecture/platform boundary that P1 will enforce.

## Sequencing

1. C7 consumes the M6 endpoint factory, spawn accounting, supervision, and generation machinery.
2. C8 consumes C7's bounded sample plane.
3. C9 consumes C8 plus the scheduler and time mechanisms from M1/M2, after P1 has made their architecture boundary explicit.
4. C10 consumes C7's per-holder quota and accounting pattern only. It does not consume C8 or C9 and may proceed in parallel with the remaining fabric slices.
5. H2 consumes C7's generation-v3/shared-buffer foundation and P1's extracted architecture/platform boundary for userspace drivers.
6. ROS R1 consumes C8 and H6 networking; it does not block C9 and its initial wire-conformance gate does not require a non-x86 boot.
omp --resume 019fa694-98d1-7000-93b1-e54ee0f898fd
## C7: Bounded resource and shared-sample plane

**Status:** Complete. Decomposed into C7.1–C7.7 so each slice introduces one primary state surface and owns an independently reviewable QEMU check, mirroring the M5/M6 sub-slice convention. Every gate passes, including the full-graph boot checks. The 2026-07-26 audit reopened this gate on three findings, all now resolved: C7.5's boot wedge (backlog B3), the dormant live-path shared-buffer plane (backlog B4), and the absence of any syscall-level or real-component evidence (backlog B5). A built generation carries a digest-authenticated `shared-buffer-budget/v1` resource; `bootstrap` mints a `SharedBufferFactory` and validates its generation grants; `dango` and `spawn-service` boot with distinct non-`DENY` quotas; and `sample-lender`/`sample-receiver` move a `>MAX_MSG` payload through the real `SYS_SHARED_BUFFER_*` syscalls under `just sample_plane_live_check`. Residual debt is narrow and recorded rather than open: `SYS_SHARED_BUFFER_REVOKE` has no live caller, and the two insert-failure rollback paths are uncovered. Evidence: `devlog/2026-07-26-c7-audit/`, `devlog/2026-07-26-b3-shared-buffer-table-stack-overflow/`, `devlog/2026-07-26-b4-live-shared-buffer-budget/`, `devlog/2026-07-26-b5-live-sample-plane/`.

**Depends on:** the M6 endpoint factory, spawn accounting, supervision, and generation machinery.

**Sequencing:** C7.1 lands the v3 generation format and `u64` rights that every later slice consumes. C7.2 introduces the shared-buffer capability objects and factory-authorized allocation under fixed kernel bounds. C7.3 adds generation-declared quotas and supervision-subtree accounting. C7.4 adds map, unmap, and irreversible read-only sealing. C7.5 adds loan/return ownership and fault reclamation. C7.6 defines and validates the sample-descriptor contract over that lifecycle. C7.7 composes the slices into the two-component exit condition and owns `just sample_plane_check`.

### C7.1 — Generation format v3 and u64 rights

**Status:** Complete, with one correction outstanding (2026-07-26 audit). Generation format v3 with `u64` rights is built and byte-identical across two builds; retained v2 generations still decode, keep their signed release authorized (the authority hash stays 32-bit for v2 — pinned by `retained_v2_authority_manifest_is_width_stable`), and pass the stage-0 admission chain (`retained_v2_generation_passes_stage0_admission`). The original "and boots" wording was scoped to admission rather than a completed boot: no v2 artifact exists to boot, and each generation embeds the kernel it runs, so a v2 rollback executes its own v2-era kernel (backlog B6, resolved). The `RIGHT_MAP` rename reached the host vocabulary too: the manifest key is `bufferMap` (backlog B7, resolved). Verified under `just generation_check`, `just contracts_check` (including the boot-contracts v2/v3 decode and admission tests), `just test`, and `just transfer_check`.

**Depends on:** M6.1 generation format v2 and the capability/rights foundation.

### Deliverables

- introduce deterministic generation format v3 with `u64` rights; retain decoding of known-good v2 generations for the bounded rollback window rather than changing v2 meanings;
- migrate manifest rights strings deterministically and reject unknown or meaningless v3 rights bits;
- replace the grandfathered generic `RIGHT_MAP` name with an object-specific shared-buffer map right when the v3 mapping lands.

### Required checks

- two builds from identical normalized v3 input are byte-identical, retained v2 known-good artifacts still boot during the rollback window, and unsupported versions fail closed;
- unknown or object-meaningless v3 rights bits are rejected at decode and at `CapabilityTable::insert`;
- the renamed shared-buffer map right gates exactly the buffer map operation and no other object.

### Verification target

```sh
just generation_check
just contracts_check
```

### Exit condition

A v3 generation built from normalized input is byte-identical across two builds, boots the existing vertical slice with `u64` rights, and a retained v2 known-good artifact still decodes, keeps its signed release authorized, and passes the stage-0 admission chain; an unsupported version and an unknown rights bit both fail closed. The v2 arm is proven to admission, not to a completed boot: no v2 artifact exists to boot, because the builder has only ever emitted v3 and each generation embeds the kernel it runs, so a v2 rollback would execute its own v2-era kernel rather than this tree's (backlog B6, resolved with that scope recorded).

### C7.2 — Shared-buffer authority and factory allocation

**Status:** Complete. A distinct `SharedBufferFactory` kernel object gates `SYS_SHARED_BUFFER_CREATE`/`SYS_SHARED_BUFFER_RELEASE` behind `RIGHT_BUFFER_CREATE`; buffers carry a kernel-assigned unforgeable identity and only narrow-only `RIGHT_BUFFER_WRITE`/`RIGHT_BUFFER_MAP`/`RIGHT_TRANSFER`. Allocation is bounded by fixed global ceilings (`MAX_SHARED_BUFFERS`=32, `MAX_TOTAL_PAGES`=256, `MAX_BUFFER_PAGES`=64) checked before any frame is pulled, returning structured `SharedBufferError`; DMA and shared-sample authority remain distinct capability kinds. As of 2026-07-26 (backlog B4) the factory is minted on the live boot path and granted through the generation to `dango` and `spawn-service`, both of which allocate and release through the real syscalls at startup. Both syscalls are additionally driven by real components under `just sample_plane_live_check` (B5), including the denial arm where a factory capability is named where a buffer is expected; the create-insert-failure rollback remains uncovered. Verified under `just shared_buffer_factory_check` (8 QEMU cases), with `just test`, `just spawn_service_check`, `just contracts_check`, `just generation_check`, `just fmt_check`, `just lint`, and `just framework_safety_check` clean.

**Depends on:** C7.1 v3 rights and the M6.1 factory-capability pattern.

### Deliverables

- add a distinct `SharedBufferFactory` kernel object and formalize the existing `SharedBuffer` object with object-specific create, map, write, and transfer authority;
- expose bounded create and release operations behind a named factory capability, with fixed kernel-wide byte and object ceilings returning structured exhaustion;
- keep buffer identity kernel-created and unforgeable; derivation and transfer may only narrow rights, and release invalidates the releasing holder's capability;
- keep DMA buffers and ordinary shared samples as distinct authority even if later slices reuse memory-accounting machinery.

### Required checks

- a component without the factory capability cannot allocate a shared buffer;
- allocation cannot exceed fixed kernel byte or object bounds and exhaustion does not disturb an unrelated holder;
- deriving or transferring a buffer checks `RIGHT_TRANSFER`, never widens rights, and cannot invent a buffer identity;
- DMA authority and shared-sample authority remain distinct capability kinds.

### Verification target

```sh
just shared_buffer_factory_check
```

### Exit condition

A factory-authorized holder creates and releases a kernel-identified shared buffer within fixed global bounds; an unauthorized component is denied, exhaustion is structured and isolated, and no derivation or transfer widens authority.

### C7.3 — Generation quotas and supervision accounting

**Status:** Complete. A versioned Zutai shared-buffer budget contract (`contracts/shared-buffer-budget/v1/`) is stored as a generation `KIND_RESOURCE` object, authenticated through the generation's existing per-object digest table; it declares per-holder `byte_pages`, `buffer_count`, `mapping_count`, and `loan_count` quotas. A present budget is validated deterministically at generation decode and rejects missing, malformed, unsorted/duplicate, or per-holder-impossible limits before any component launches (the validator bounds each holder and, since backlog B8, also sums holders so a validating budget can be honoured with every holder at its ceiling at once). `SharedBufferTable::create` charges each allocation to the creating supervision-subtree owner against its `HolderQuota` (deny-by-default when absent), enforced before the global ceiling and side-effect-free on rejection; `reclaim_owner` returns every unloaned page and charge on release, peer death, supervised restart, and revocation (via `task::terminate`) without disturbing another subtree. The live boot path declares a real budget as of 2026-07-26 (backlog B4): the built generation carries one digest-authenticated budget object, `dango` and `spawn-service` boot with distinct non-`DENY` quotas, and each proves its own with a create/map/write/seal/release self-check at startup. Verified under `just shared_buffer_accounting_check` (8 QEMU cases, including `booted_generation_declares_distinct_holder_budgets`) plus `just contracts_check`, `just generation_check`, `just test`, `just spawn_service_check`, `just fmt_check`, `just lint`, and `just framework_safety_check` clean. See `devlog/2026-07-24-c7-3-shared-buffer-accounting/` and `devlog/2026-07-26-b4-live-shared-buffer-budget/`.

**Depends on:** C7.2 factory allocation; M6.1 supervision and per-spawner accounting.

### Deliverables

- define a versioned Zutai shared-buffer budget contract stored as a generation resource object, with per-holder byte, buffer-count, mapping-count, and outstanding-loan quotas; generation v3 references and authenticates the resource through its existing object table rather than adding ad-hoc record fields;
- validate every budget deterministically before component launch and reject missing, malformed, overflowing, or globally impossible limits;
- charge allocated pages and resource counters to the creating supervision subtree rather than to an ambient global owner;
- reclaim unloaned buffers and charges after explicit release, peer death, supervised restart, or explicit revocation; C7.5 extends the rule to outstanding loans.

### Required checks

- a holder cannot exceed its manifest byte or buffer-count quota even while another holder remains below its own quota;
- malformed or impossible generation budgets fail before allocation or component launch;
- peer death, supervised restart, and revocation reclaim every unloaned page and charge in the affected subtree without changing another subtree's account;
- mapping-count and outstanding-loan quotas are present and bounded before the operations that consume them land.

### Verification target

```sh
just shared_buffer_accounting_check
```

### Exit condition

Two holders receive distinct generation-declared budgets; one reaches byte or buffer-count exhaustion without affecting the other, and termination of its supervision subtree returns every unloaned page and charge.

### C7.4 — Mapping and read-only sealing

**Status:** Complete (mechanism), with a coverage caveat from the 2026-07-26 audit. Shared buffers now expose bounded `SYS_SHARED_BUFFER_MAP`/`SYS_SHARED_BUFFER_UNMAP`/`SYS_SHARED_BUFFER_SEAL`. Mapping installs only page-aligned, non-executable, exact-frame user PTEs for the named buffer capability, gated by `RIGHT_BUFFER_MAP` (writable additionally by `RIGHT_BUFFER_WRITE`) and charged one unit against the holder's `mapping_count` quota under `MAX_MAPPINGS`=64; offset/length/base are range- and overflow-checked and confined to the user half before any page-table change, and a partial map is fully rolled back. Sealing is an irreversible Arc-shared read-only transition that downgrades every live writable PTE before publishing the seal; a created-read-only or sealed region can never obtain a writable mapping. Unmap, release, and supervision-subtree reclamation remove the exact PTEs before returning frames, without disturbing an unrelated mapping. All three syscalls are driven at the syscall boundary by real components under `just sample_plane_live_check` (B5), which asserts that a writable mapping cannot be obtained after sealing. Verified under `just shared_buffer_mapping_check` (8 QEMU cases), with `just test`, `just shared_buffer_accounting_check`, `just shared_buffer_factory_check`, `just contracts_check`, `just generation_check`, `just fmt_check`, `just lint`, and `just framework_safety_check` clean.

**Depends on:** C7.2 shared-buffer objects and C7.3 accounting.

### Deliverables

- expose bounded map and unmap operations charged against the holder's mapping-count quota;
- validate offset and length before page-table changes, map only pages belonging to the exact buffer capability, and require object-specific map/write rights;
- make sealing an irreversible transition to read-only: existing writable mappings are removed or downgraded before the seal succeeds, and no later operation restores write access;
- reclaim mappings and mapping charges on unmap, release, peer death, supervised restart, or revocation.

### Required checks

- a holder cannot map outside the granted buffer, overflow offset/length arithmetic, exceed its mapping quota, or widen read-only access to writable;
- sealing fails safely or removes every writable mapping before publishing the sealed state;
- map-after-seal can produce only read-only access, and use-after-unmap or use-after-release fails with a structured error;
- mapping cleanup in one supervision subtree does not disturb an unrelated mapping.

### Verification target

```sh
just shared_buffer_mapping_check
```

### Exit condition

A holder maps only an in-bounds region charged to its manifest quota, seals the buffer read-only, and cannot recover write access; malformed ranges and lifecycle misuse fail before page-table changes.

### C7.5 — Loan/return lifecycle and fault reclamation

**Status:** Complete (mechanism); its boot regression is fixed. C7.5 originally wedged every full-graph boot — a 10520-byte `SharedBufferTable` published through a `LazyLock` was first constructed on a 32 KiB unguarded task kernel stack inside `task::terminate`, overflowing it silently so the ready queue never drained to `on_idle`. Fixed 2026-07-26 by const-initializing the table into `.bss` (backlog B3, resolved; `devlog/2026-07-26-b3-shared-buffer-table-stack-overflow/`). Bounded loan/return over an exact sealed subrange lands as `SYS_SHARED_BUFFER_LOAN`/`SYS_SHARED_BUFFER_LOAN_MAP`/`SYS_SHARED_BUFFER_RETURN`/`SYS_SHARED_BUFFER_REVOKE` behind the new object-specific `RIGHT_BUFFER_LOAN` (bit 25) and a receiver-bound `SharedBufferLoan` kernel object. A loan requires an irreversibly sealed source region, names its receiver through a `RIGHT_SUPERVISE` capability (never an ambient task id), charges one unit against the lender's `loan_count` quota under `MAX_LOANS`=64, and carries a kernel-assigned unforgeable single-return identity. `release_by` retains the creator's pages and buffer charge while any loan is outstanding; the final settle finalizes the region. `map_loan` confines the receiver to the loaned subrange and is always read-only; duplicate, stale, and wrong-buffer returns fail closed without changing accounting. `reclaim_owner` settles every loan naming a dying task as lender or receiver. All four syscalls are driven by real components under `just sample_plane_live_check` (B5): a loan is refused over an unsealed region, confined to its subrange, kept read-only, and returned exactly once. Verified under `just shared_buffer_loan_check` (7 QEMU cases) plus, after the fix, `just transfer_check`, `just spawn_service_check`, and `just dango_check` — the full-graph boot gates that were not run for this slice, which is how the regression shipped.

**Depends on:** C7.3 accounting and C7.4 sealed mappings.

### Deliverables

- expose bounded loan and return operations for an exact sealed buffer region, charged against the lender's outstanding-loan quota;
- retain pages and accounting while any valid loan is outstanding even if the creator releases its local capability;
- make each loan identity unforgeable and single-return, with receiver authority restricted to the loaned region and granted rights;
- settle or revoke loans and reclaim unreachable mappings, pages, and charges on receiver death, lender death, supervised restart, or explicit revocation.

### Required checks

- the creator cannot reclaim pages while a valid loan is outstanding and cannot exceed its outstanding-loan quota;
- stale loans, duplicate returns, wrong-buffer returns, and use-after-return fail with structured errors without changing accounting;
- receiver mappings cannot escape the loaned region or become writable after a read-only loan;
- every peer-death, restart, and revocation path returns the loan and resource counters to their pre-loan values.

### Verification target

```sh
just shared_buffer_loan_check
```

### Exit condition

A lender loans one sealed region and cannot reclaim its pages until the receiver returns it; duplicate, stale, and wrong-buffer returns fail closed, while peer death deterministically settles the loan and restores every charge.

### C7.6 — Versioned sample descriptor

**Status:** Complete. A versioned Zutai sample-descriptor contract (`contracts/sample-descriptor/v1/`) renders byte-identical `slime-proto` bindings (`WireSampleDescriptor`) whose fixed control message is exactly the channel bound (`DESCRIPTOR_LEN == MAX_MSG == 64`), referencing a transferred `SharedBufferLoan` by capability kind, unforgeable loan identity, page-aligned offset/length, type identity, sequence, and known flags. `valid_sample_descriptor` rejects bad magic/version, wrong capability kind, unknown flag bits, dirty reserved bytes, zero/mismatched loan and type identities, non-power-of-two page size, checked-add offset/length overflow, zero or misaligned length, and length beyond `MAX_SAMPLE_BYTES` before any mapping or allocation; the kernel `map_loan` independently re-validates loan identity, receiver binding, bounds, and read-only mapping. A payload larger than `MAX_MSG` (8192 bytes) traverses descriptor plus shared buffer without widening `MAX_MSG` or copying payload bytes through the kernel queue. Verified under `just sample_descriptor_check` (4 QEMU cases), `just contracts_check` (byte-identical bindings), and the full gate stack.

**Depends on:** C7.4 sealed mappings and C7.5 loan/return lifecycle.

### Deliverables

- define a versioned Zutai sample-descriptor contract that fits the existing channel control-message bound and references an exact transferred shared-buffer capability, offset, length, type identity, sequence, and declared flags;
- validate version, flags, capability kind, loan identity, offset, length, and type identity before mapping or allocating receiver state;
- send descriptor control data through ordinary channels while payload bytes remain in the shared buffer, without increasing `MAX_MSG` or copying payload bytes through the kernel queue.

### Required checks

- byte-identical bindings round-trip every admitted descriptor and reject unsupported versions or unknown flags;
- overflowed offset/length, stale loan identity, wrong capability kind, and mismatched type identity fail before mapping or allocation;
- a payload larger than the kernel message bound traverses descriptor plus shared buffer without increasing `MAX_MSG` or copying payload bytes through the kernel queue.

### Verification target

```sh
just sample_descriptor_check
```

### Exit condition

A receiver validates a bounded versioned descriptor, maps only the exact loaned bytes, and observes a payload larger than the control-message bound; every malformed descriptor fails before mapping or allocation.

### C7.7 — Sample-plane integration and isolation

**Status:** Complete. `kernel/tests/sample_plane.rs` composes the C7.2 factory allocation, C7.3 per-holder quotas, C7.4 mapping/sealing, C7.5 loan/return lifecycle, and the C7.6 sample descriptor into two holders that exchange a `>MAX_MSG` payload: only the 64-byte descriptor crosses a real IPC channel while the receiver reconstructs the full two-page payload from the quota-charged sealed loaned buffer through exact read-only page-table translations. A malformed (stale-identity) descriptor delivered over the channel is rejected by validation and by the loan-aware map path before any mapping or allocation, leaving the loan intact. Every quota class (byte-pages, buffer-count, mapping-count, loan-count) fails with `QuotaExceeded` at ceiling+1 without disturbing an unrelated owner's buffer, mapping, or channel; and a retained v2 known-good generation decodes byte-identically before and after a full sample-plane exchange. That gate composes `u64` owner ids; since 2026-07-26 (backlog B5) it is paired with `just sample_plane_live_check`, where two separately spawned components — holding only generation-granted capabilities — run the same exchange through the real `SYS_SHARED_BUFFER_*` syscalls, with the loan receiver named by a `RIGHT_SUPERVISE` capability and six denial arms asserted in order. The retained-v2 arm is a decode probe rather than a boot, which is the correct scope: no v2 artifact exists to boot, and a v2 rollback would run its own embedded kernel (backlog B6, resolved). Verified under `just sample_plane_check` (5 QEMU cases) and `just sample_plane_live_check`, with `just test`, `just fmt_check`, and `just lint` clean.

**Depends on:** C7.1–C7.6.

### Deliverables

- compose the factory, quotas, mapping, sealing, loan lifecycle, and descriptor into two isolated components that exchange and return a payload larger than the kernel IPC message bound;
- prove malformed descriptors, every quota class, and peer death remain bounded and reclaim all resources without disturbing an unrelated channel or the retained v2 known-good boot path.

### Required checks

- two isolated components exchange and return a payload larger than `MAX_MSG` through a quota-charged shared buffer;
- malformed descriptors, byte/buffer/mapping/loan quota exhaustion, and peer death remain bounded, reclaim all resources, and do not disturb an unrelated channel;
- the retained v2 known-good boot path is unaffected by the sample-plane exercise.

### Verification target

```sh
just sample_plane_check
```

### Exit condition

Two isolated components exchange and return a payload larger than the kernel IPC message bound through a quota-charged shared buffer; malformed descriptors, every quota class, and peer death remain bounded, reclaim all resources, and do not disturb an unrelated channel or the retained v2 known-good boot path.

## C8: Native typed data fabric

**Status:** In progress. Decomposed into C8.1–C8.9. C8.1 (deterministic
interface schemas and native bindings), C8.2 (authenticated fabric-graph
resource with per-entry and aggregate admission), and C8.3 (attenuated
endpoint provisioning and the live control plane) are complete and gated by
`just interface_schema_check`, `just fabric_manifest_check`, and `just
fabric_authority_check`. C8.4–C8.9 remain planned contracts and gates, not
implemented capability.

**Depends on:** C7's bounded sample plane and backlog item **B2** (scheduler
`Blocked` state / `SYS_WAIT` wait-set). Both are complete. C8 remains
local-first and may proceed on `x86_64-qemu-virtio` while architecture
portability work continues.

### Architecture decisions

- the authoritative `InterfaceSchema` identity is a domain-separated SHA-256
  digest of versioned normalized Zutai schema bytes; generated bindings embed
  the full identity;
- C7's existing 64-bit sample-descriptor `type_identity` remains wire-stable
  and becomes a generation-local type tag derived from the full identity;
  generation admission rejects tag collisions between distinct admitted
  schemas, and route matching never treats the tag alone as authority;
- the kernel remains unaware of schemas, graph names, route kinds, QoS, and
  correlation policy. Its only new C8 mechanism is a generic bounded
  narrow-on-transfer operation so a userspace service can move a capability
  with an exact non-widening rights mask;
- the fabric brokers large samples through C7's receiver-bound loans. It maps a
  publisher loan read-only, makes one bounded copy into a fabric-owned sealed
  buffer, and creates one receiver-bound downstream loan per subscriber. C8
  does not add multi-receiver loans or transferable ambient supervision;
- timed QoS consumes an explicit capability-routed monotonic-time input. The
  C8 corpus drives it with deterministic simulated time; C9 later supplies the
  standard component-facing monotonic and simulated-time services without
  changing C8 QoS state-machine meanings;
- the initial fabric graph admits at most the existing `SYS_WAIT` bound of
  eight live ingress sources per fabric instance. Admission rejects a graph
  whose endpoint and control topology cannot block without polling; expanding
  the generic wait-set or introducing bounded route workers requires a later
  observed profile need;
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
   QoS.
5. C8.6 establishes calls; C8.7 composes calls and streams into operations.
6. C8.8 adds filtered introspection and declared interposition.
7. C8.9 closes the parent milestone with the full QEMU graph, fault, denial,
   and determinism corpus.

### C8.1 — Deterministic interface schemas and native bindings

**Status:** Complete. `just interface_schema_check` and the live sample-plane gate pass with one deterministic normal form, full identity, generated local tag, and native binding set; malformed, unsupported, over-bound, duplicate, and forced-collision inputs fail before output.

#### Deliverables

- define a bounded versioned Zutai normal form for native interface schemas and
  derive the authoritative `InterfaceSchema` identity from the exact normalized
  bytes with a domain-separated SHA-256 digest;
- generate or deterministically validate Rust bindings and embedded identities
  for `Stream<T>`, `Call<Request, Reply>`, and
  `Operation<Goal, Feedback, Result>`;
- define the generation-local 64-bit type-tag derivation used by the retained
  C7 sample descriptor and reject any collision between distinct admitted full
  identities before building the generation;
- bound schema depth, fields, names, sequences, encoded size, generated output,
  and the total admitted schema set before allocation.

#### Required checks

- equivalent schema input produces byte-identical normalized bytes, full
  identity, type tag, and generated bindings across two runs;
- field order, width, signedness, bounds, nesting, or contract kind changes the
  full identity, while source formatting and declaration order that normalize
  equivalently do not;
- malformed, unsupported, over-bound, duplicate, and forced type-tag-collision
  inputs fail before emitting bindings or a generation artifact.

#### Planned verification target

```sh
just interface_schema_check
```

#### Exit condition

Equivalent bounded Zutai interfaces produce one byte-identical normal form,
full identity, generation-local tag, and native binding set; conflicting
layouts cannot reuse an admitted identity or tag.

### C8.2 — Generation graph, QoS, and aggregate admission

**Status:** Complete. A versioned Zutai fabric-graph contract
(`contracts/fabric-graph/v1/`) is stored as a generation `KIND_RESOURCE`
object, authenticated through the generation's existing per-object digest
table. It fixes the admitted schema set (full C8.1 identities, collision-checked
generation-local tags, contract kinds, encoded bounds), the route table, the
participant table with exact `TransportQoS`, visibility, and interposition
chains, and every per-graph resource ceiling. Route authority is the fold of
(route name, full interface identity, contract kind); participant authority
additionally folds in component identity and direction, so a name, a type, or
a graph observation grants nothing. A present graph is validated
deterministically at generation decode against the kernel's own
`MAX_WAIT_SOURCES`/`MAX_CAPS`/`MAX_TOTAL_PAGES`/`MAX_MAPPINGS`/`MAX_LOANS`/
`MAX_MSG` before any component launches, and the host builder enforces the same
rule set so a malformed graph fails the build rather than the boot; the
contract's copies of the kernel bounds are pinned by `const _: () = assert!` in
`kernel/src/runtime/generation.rs`. `just fabric_manifest_check` passes: a
deterministic 896-byte resource with 2 schemas, 2 routes, 4 participants, and
one interposition hop, a 35-case negative corpus each rejected by its intended
check, 18 `boot-contracts` decoder tests, and 4 QEMU tests against the booted
generation.

**Depends on:** C8.1.

#### Deliverables

- define a versioned Zutai fabric-graph resource containing admitted schemas,
  exact endpoint grants, route name, full schema identity, contract kind,
  direction, offered/requested `TransportQoS`, graph visibility, optional
  interposition chain, and component identity;
- define `TransportQoS` with bounded KEEP_LAST depth, RELIABLE or BEST_EFFORT
  delivery, VOLATILE or bounded retained durability, deadline, lifespan,
  liveliness kind, and lease duration;
- make route count, live ingress sources, publisher/subscriber/client/server
  count, sample bytes, queue/history/event depth, retained samples, retries,
  in-flight calls/operations, shared-buffer pages, mappings, and loans explicit
  generation limits;
- validate per-entry and aggregate memory, capability-slot, wait-source, buffer,
  and loan requirements before any fabric or client component launches;
- derive route authority from the exact tuple of route name, full interface
  identity, contract kind, component identity, and direction. A name, type
  string, or graph observation grants nothing.

#### Required checks

- identical normalized graph input produces a byte-identical authenticated
  resource object across two builds;
- missing references, duplicate grants, impossible aggregate limits, cycles or
  bypasses in an interposition chain, unsupported QoS, and more than eight live
  fabric ingress sources fail before launch;
- alternate names with the same type and conflicting types with the same name
  remain distinct authority and matching domains;
- offered/requested compatibility is a fixed truth table with no implicit
  defaults or ROS/DDS policy leaking into the native contract.

#### Planned verification target

```sh
just fabric_manifest_check
```

#### Exit condition

One authenticated generation resource deterministically fixes every native
interface, graph edge, direction, QoS policy, visibility grant, interposition
hop, and resource ceiling; malformed, unauthorized, or globally impossible
graphs fail before component launch.

### C8.3 — Attenuated endpoint provisioning and control plane

**Status:** Complete. A versioned Zutai capability-transfer contract
(`contracts/capability-transfer/v1/`) defines the provisioning request and the
descriptor that accompanies one bounded move. The kernel's only new C8
mechanism, `SYS_CAP_TRANSFER` (30), requires `RIGHT_TRANSFER` at the source,
rejects any mask outside the source rights or the object's meaningful rights,
requires the descriptor's declared object kind to be the moved capability's
real kind, consumes the source, and restores it at full rights on a failed
send; `RIGHT_TRANSFER` is dropped at the destination unless
`FLAG_RETAIN_TRANSFER` is set, so a provisioned role is non-delegable by
default. The kernel gained no knowledge of routes, schemas, or graph roles —
`route_identity` and `direction` ride in the descriptor as bytes it never
interprets. A userspace `fabric-service` owns both halves of the declared
telemetry route, hands `fabric-publisher` `RIGHT_SEND` only and
`fabric-subscriber` `RIGHT_RECV` only, and authenticates each client by the
generation-provisioned control endpoint its request arrived on rather than the
route name, direction, or type identity the request carries. It sweeps every
control endpoint through the non-blocking ABI and parks in `SYS_WAIT` across
the whole set. `just fabric_authority_check` passes: 7 kernel rights-algebra
tests plus a live boot in which each participant observes its own denials —
no opposite-direction authority, no re-delegation, no widening — before
publishing, and `fabric-intruder`, holding a real control endpoint and
supplying byte-identical route strings, receives a denial with no capability
attached. The service provisions one round and exits by design; C8.4 makes its
loop unbounded when it gains sample brokering. See
`devlog/2026-07-27-c8-3-fabric-authority/`.

**Depends on:** C8.2.

#### Deliverables

- define a versioned Zutai capability-transfer descriptor and implement a
  bounded move operation whose destination rights are an exact subset of the
  source rights and object-specific rights mask;
- require transfer authority at the source, consume the moved source
  capability, and omit transfer authority at the destination unless it was
  both held and explicitly retained;
- implement a long-lived userspace fabric service that consumes the generation
  graph, endpoint factory, participant supervision capabilities, shared-buffer
  budget/factory, and explicit time input;
- create route data, acknowledgement/event, call, and operation endpoints and
  hand each participant only its exact non-transferable route role;
- authenticate a client by its generation-provisioned control endpoint rather
  than a caller-supplied component name, route name, or type identity.

#### Required checks

- a publisher receives no route receive authority, a subscriber receives no
  route publish authority, and neither can retransfer its endpoint or create an
  undeclared edge;
- masked transfer cannot widen rights, change object identity, retain an
  unauthorized transfer bit, duplicate a moved capability, or disturb an
  unrelated capability;
- an ungranted component cannot register, request, discover, or receive a
  protected endpoint even when it supplies the exact route and schema strings;
- the idle service parks through `SYS_WAIT`, wakes on every admitted source or
  peer death, and consumes no CPU through a poll/yield loop.

#### Verification target

```sh
just fabric_authority_check
```

#### Exit condition

The live fabric derives exact non-widening, non-transferable route endpoints
from the authenticated generation graph; possession of names or generic
channel authority cannot mint, widen, or delegate a graph edge. Observed: on a
real boot the publisher holds `RIGHT_SEND` only and the subscriber
`RIGHT_RECV` only, neither can re-delegate or widen its role, and
`fabric-intruder` — holding a real generation-provisioned control endpoint and
supplying byte-identical route name, direction, and type identity — receives a
denial carrying no capability. The "consumes no CPU through a poll/yield loop"
arm is proven by the service's sweep-then-park loop plus a source lint, not by
a measurement: the gate rejects any fabric component containing `yield_now` or
lacking a `SYS_WAIT` park, which is a necessary condition rather than a proof
(see the devlog entry's open risks).

### C8.4 — Bounded many-to-many streams

**Status:** Not started.

**Depends on:** C8.3.

#### Deliverables

- implement bounded many-to-many `Stream<T>` matching on exact route name, full
  interface identity, and compatible requested/offered QoS;
- carry control-bound samples inline over ordinary channels and payloads larger
  than `MAX_MSG` through validated C7 sample descriptors and receiver-bound
  shared-buffer loans;
- copy each admitted large publisher sample at most once into a fabric-owned
  sealed buffer, then create an independently accounted downstream loan for
  each matched subscriber;
- implement deterministic KEEP_LAST eviction and BEST_EFFORT delivery with
  bounded queues, loss accounting, and event delivery;
- reclaim inline queue entries, fabric buffers, mappings, downstream loans, and
  event slots on normal return, unmatch, participant death, or route teardown.

#### Required checks

- two publishers and two subscribers exchange both inline and `>MAX_MSG`
  samples without a participant obtaining authority over another route;
- KEEP_LAST evicts the exact oldest sequence at the declared depth and a
  BEST_EFFORT stalled subscriber reports bounded loss without retry growth;
- one large sample incurs one fabric payload copy and one quota-charged
  receiver-bound loan per subscriber; every return and peer-death path settles
  all charges;
- malformed descriptors, wrong tags, stale loans, sequence misuse, queue
  exhaustion, and one participant fault do not disturb an unrelated stream.

#### Planned verification target

```sh
just fabric_stream_check
```

#### Exit condition

A generation-declared many-to-many stream moves bounded typed inline and shared
samples under exact route authority; KEEP_LAST and BEST_EFFORT behavior is
deterministic, and a stalled or faulting participant cannot grow or disturb
unrelated state.

### C8.5 — Reliable, retained, and timed QoS

**Status:** Not started.

**Depends on:** C8.4.

#### Deliverables

- implement a bounded credit/acknowledgement protocol so RELIABLE delivery never
  busy-retries a full channel and BEST_EFFORT never acquires retry state;
- retain unacknowledged and durability history within fixed sample, byte,
  buffer, loan, retry, and event ceilings;
- implement offered/requested QoS matching, matched/unmatched notifications,
  incompatible-QoS events, fixed retry exhaustion, and bounded retained replay;
- drive deadline, lifespan, liveliness, and lease transitions only from the
  explicit monotonic-time capability and preserve deterministic tie ordering
  when data, acknowledgement, peer-death, and time events coincide;
- keep loss, expiry, retry exhaustion, deadline miss, liveliness loss,
  incompatible QoS, and peer death as distinct structured events.

#### Required checks

- RELIABLE delivery advances only with declared credit, retains no more than
  its fixed history, and ends at success, expiry, peer death, or fixed retry
  exhaustion without a yield/poll loop;
- BEST_EFFORT reports loss at zero credit without allocating retry/history
  state, while bounded retained durability replays only the declared live
  history;
- deterministic simulated time distinguishes deadline, lifespan, liveliness,
  and lease boundaries including equal-timestamp tie cases;
- a stalled subscriber cannot grow publisher, fabric, shared-buffer, loan, or
  event memory beyond generation bounds.

#### Planned verification target

```sh
just fabric_qos_check
```

#### Exit condition

Compatible endpoints exchange data under bounded RELIABLE/BEST_EFFORT,
VOLATILE/retained, deadline, lifespan, and liveliness semantics without
busy-polling or unbounded history; every terminal or degradation condition has
a distinct deterministic event.

### C8.6 — Bounded native calls

**Status:** Not started.

**Depends on:** C8.3 and C8.5's event/time semantics.

#### Deliverables

- implement `Call<Request, Reply>` endpoint matching and generation/session-
  qualified request identities with a fixed in-flight table per route, client,
  and server;
- route inline and shared-sample requests/replies under distinct client and
  server authority, preserving the C7 receiver binding on every large payload;
- implement one terminal result per request, bounded cancellation and timeout,
  duplicate/stale request and reply rejection, server rejection, and peer-death
  propagation;
- prevent a duplicate or stale request from re-executing a declared
  non-idempotent operation and reclaim every correlation, buffer, loan, retry,
  and event entry on all terminal paths.

#### Required checks

- concurrent clients receive only their correlated replies and cannot answer,
  cancel, or observe another client's request;
- duplicate/stale request and reply identities fail deterministically, and a
  non-idempotent server sees one execution;
- success, server rejection, timeout, cancellation, retry exhaustion, malformed
  reply, and peer death remain distinct;
- server or client death reclaims all in-flight state without terminating the
  fabric or an unrelated call route.

#### Planned verification target

```sh
just fabric_call_check
```

#### Exit condition

Generation-authorized clients and servers exchange bounded typed requests and
replies with exact correlation and one terminal result; duplicate, timeout,
cancellation, rejection, and peer-fault paths remain isolated and fully
reclaimed.

### C8.7 — Native operations

**Status:** Not started.

**Depends on:** C8.4 and C8.6.

#### Deliverables

- compose `Operation<Goal, Feedback, Result>` from a bounded start-goal call,
  operation-keyed feedback stream, result call, and cancellation request;
- assign generation/session-qualified operation identities and bound active
  operations, feedback depth/bytes, cancellation state, terminal results,
  retained results, retries, and events before admission;
- route each goal, feedback sample, result, and cancellation only to holders of
  the exact operation-role capability;
- define transport-level accepted/rejected, active, cancel-requested, terminal,
  expired, and peer-lost outcomes without embedding application goal policy or
  the ROS action state machine in the fabric.

#### Required checks

- two concurrent operations cannot cross-correlate feedback, result, or cancel
  authority;
- unauthorized observation, result retrieval, and cancellation fail even when
  the caller knows the operation identity;
- duplicate goals, feedback after terminal state, duplicate results,
  cancellation races, result expiry, and participant restart are deterministic
  and bounded;
- peer death settles every active operation and leaves unrelated stream, call,
  and operation routes live.

#### Planned verification target

```sh
just fabric_operation_check
```

#### Exit condition

Authorized components start, observe, cancel, and retrieve bounded native
operations with exact correlation and authority; transport outcomes remain
deterministic while application and ROS goal policy stay outside the fabric.

### C8.8 — Filtered introspection and declared interposition

**Status:** Not started.

**Depends on:** C8.3, C8.4, and C8.6.

#### Deliverables

- expose graph introspection through a read-only service whose bounded result is
  filtered to the caller's exact generation-declared visibility grants;
- report only admitted route, schema identity, contract kind, match, QoS, and
  event metadata; never return a capability or make an observed name/type into
  authority;
- compile each recorder, replay membrane, or protocol gateway into an explicit
  acyclic route chain whose proxy receives only the narrowed upstream receive,
  downstream send, acknowledgement/event, and visibility capabilities it
  requires;
- omit every direct bypass endpoint when interposition is declared and isolate
  proxy failure to the affected route chain.

#### Required checks

- two callers with different visibility grants receive different bounded graph
  views, and an ungranted caller cannot infer the protected route through
  counts, names, types, match events, or error detail;
- a proxy can relay only its declared route/direction and cannot publish,
  subscribe, call, serve, inspect, or retransfer outside that chain;
- publisher and subscriber cannot bypass a declared proxy, while proxy death
  emits a route event without terminating unrelated routes or the fabric;
- a fixed graph and request order produces byte-identical introspection and
  interposition trace records.

#### Planned verification target

```sh
just fabric_visibility_check
```

#### Exit condition

Read-only graph views reveal exactly the caller's visibility grant, and every
declared interposer occupies the only authorized route path with no ambient
discovery, bypass, or widened proxy authority.

### C8.9 — Full-graph integration, determinism, and fault isolation

**Status:** Not started.

**Depends on:** C8.5–C8.8.

#### Deliverables

- compose isolated native publishers, subscribers, call clients/servers,
  operation participants, an unauthorized probe, a stalled subscriber, a
  filtered introspection client, and an interposed route in one
  generation-declared graph;
- exercise inline and shared samples, compatible and incompatible endpoints,
  KEEP_LAST, BEST_EFFORT, RELIABLE, retained durability, timed QoS, calls,
  operations, visibility, interposition, denial, and participant faults;
- capture normalized schema artifacts and bounded IPC/event trace records for a
  fixed graph, input, and simulated-time sequence;
- prove every route, queue, history, retry, retained sample, event, in-flight
  request/operation, shared-buffer, mapping, and loan ceiling under normal,
  stalled, malformed, denied, and peer-death paths.

#### Required checks

- publishers and subscribers match only when name, full type identity, contract
  kind, and requested/offered QoS are compatible;
- an ungranted component cannot create, discover, publish, subscribe, call,
  serve, operate, cancel, retrieve, or inspect the protected route;
- alternate names with the same type and conflicting types with the same name
  do not alias matching, visibility, or authority;
- deadline, lifespan, liveliness loss, incompatible QoS, loss, expiry, retry
  exhaustion, timeout, cancellation, rejection, and peer death remain
  distinguishable;
- one participant or proxy may stall or fault without exceeding any manifest
  bound or terminating another route, the fabric service, or the kernel;
- the same graph, input, and simulated-time sequence produces byte-identical
  normalized schema artifacts and deterministic IPC/event trace records.

#### Planned verification target

```sh
just data_fabric_check
```

### Exit condition

A generation-declared graph of isolated native publishers, subscribers, service
clients/servers, and operation participants exchanges bounded typed data under
explicit QoS and graph grants; denied graph edges are neither usable nor
visible, incompatible endpoints do not match, and a stalled or faulting
participant cannot exceed its quota or disrupt unrelated routes.

## C9: Robot runtime authority

**Status:** Not started.

This slice supplies the timing, execution, lifecycle, and observation contracts needed by robot and mixed interactive workloads. It does not promise hard real-time behavior from QEMU; deterministic state-machine checks precede measured latency evidence on a named physical target.

**Depends on:** C8 and architecture-portability milestone P1. C9 defines architecture-neutral timer, wait-set, scheduling-class, lifecycle, and observation semantics; each admitted ISA supplies the privileged timer, interrupt, context-switch, and idle mechanisms behind P1's boundary.

### Deliverables

- expose monotonic time, optional wall time, timers, and simulated time as distinct explicit service capabilities; a component with no clock grant cannot observe time implicitly;
- implement userspace wait sets/executors over stream, call, operation, timer, supervision, and QoS-event endpoints with bounded ready queues and deterministic tie rules;
- add manifest-declared `SchedulingClass` per component or supervision subtree, initially foreground, normal, and best-effort, plus conserved CPU resource accounts;
- keep scheduling mechanism in the kernel while class assignment, dynamic promotion, and workload policy remain generation/userspace decisions; a component cannot widen its own class;
- preserve class and resource-account bounds across supervised restart while issuing fresh endpoint, mapping, and device authority;
- define component lifecycle transitions, health dependencies, bounded restart/backoff policy, and parameter state as versioned userspace schemas rather than kernel policy;
- add typed recording and replay for declared fabric routes; clock, entropy, device input, and other nondeterminism must be either capability-recorded or explicitly excluded from a deterministic claim;
- build a simulated sensor → controller → actuator workload that exercises timer, stream, call, lifecycle, restart, and contention paths without special kernel treatment.
- keep timer delivery, interrupt acknowledgement, context switching, CPU idle, and preemption behind the admitted architecture mechanism; no APIC vector, x86 register frame, CR3 operation, GIC identifier, or RISC-V trap field appears in the C9 userspace contracts;

### Required checks

- a component without clock, parameter, lifecycle-control, recorder, or scheduling-promotion authority cannot exercise that operation through another ambient API;
- a best-effort workload saturating available CPU cannot claim foreground class, escape its conserved account, or prevent the declared control workload from being scheduled according to the selected class contract;
- wait-set queues, timer counts, callbacks per wake, parameter bytes, restart attempts, backoff duration, and recorded trace bytes are bounded before allocation;
- supervised restart preserves declared class and graph shape but cannot reuse stale buffers, endpoints, timers, or device mappings;
- identical recorded typed inputs, clock events, and lifecycle transitions produce identical replayed component outputs for a manifest-declared deterministic component;
- deadline misses, timer expiry, liveliness loss, process fault, peer loss, cancellation, and scheduling-budget exhaustion remain distinct at the userspace boundary.
- the C9 semantic corpus can be replayed on later AArch64 and RV64 profiles without changing syscall meanings, event kinds, scheduling classes, bounds, or generated schemas; raw register and physical-address traces are not cross-architecture equality inputs;

### Planned verification target

```sh
just robot_runtime_check
```

### Exit condition

A simulated sensor/controller/actuator graph runs through the native fabric with explicit time, scheduling, lifecycle, and parameter authority; under CPU contention and an injected component restart it remains bounded, preserves the declared scheduling order, restores fresh authority, and reproduces its typed outputs from a complete recorded input trace.

## C10: Bounded private component memory

**Status:** Not started. Decomposed into C10.1–C10.4.

**Depends on:** C7's per-holder quota, supervision-subtree accounting, and
reclamation pattern; and backlog item **B9**, which must land first because
C10.1 extends the same task-teardown path B9 repairs. C10 does not consume C8
or C9 and may proceed in parallel with the remaining fabric slices on
`x86_64-qemu-virtio`.

**Motivation:** A component's working memory is fixed at build time. Its stack
comes from the `SLIMECMP` header and its `.data`/`.bss` from the linked image;
`components/runtime` installs no `GlobalAlloc`, and no syscall yields a page, so
`Vec`, `Box`, and `String` are unavailable to a native component. Every buffer
is therefore sized for its worst case in every generation that carries that
component. A build service, a filesystem index, and a bounded introspection
reply all need memory proportional to their input, and none can be written under
that constraint.

The shared-buffer plane is not that mechanism and must not become it. It exists
to move samples *between* components: every region is a nameable, transferable,
loanable kernel object drawn from the contiguous frame allocator under a
256-page kernel-wide ceiling. Working memory is private, never transferred,
never sealed, and needs no physical contiguity. Overloading one onto the other
would attach transfer and loan semantics to a heap and force fragmentation-prone
contiguous runs on every allocation.

### Architecture decisions

- component working memory is **one task-private region at a fixed base**, grown
  only at its tail. `SLIMECMP` images link at a fixed VA and hold real machine
  pointers, so a growth that relocated the base would invalidate every live
  pointer; growth past the reserved window fails instead of moving;
- the region is **reserved as address space at spawn and backed page by page on
  demand**. Frames are drawn individually and need not be contiguous;
- growth is **authorized by a generation-declared page quota, not a capability**.
  The region is not nameable, transferable, loanable, sealable, or shareable, so
  there is no object for a capability to designate; the authority question is
  how many pages a component may hold, which is a budget. This mirrors the
  stack, which is generation-sized and needs no capability, and it leaves
  `../docs/capability-matrix.md` unchanged: C10 adds no kernel object and no
  right;
- the quota is **deny-by-default**. A component absent from the budget resource
  grows nothing, exactly as an absent shared-buffer holder allocates nothing;
- pages are **always user/read-write/no-execute**, preserving W^X. No growth,
  admission, or compilation path may derive an executable mapping from them;
- the kernel exposes **growth only** — no `malloc`, `free`, arbitrary `mmap`,
  file-backed or executable mappings, and no second region. `free` is a
  userspace free-list operation; the pages return to the kernel when the task
  dies;
- allocation policy lives **entirely in `slime-rt`**. The kernel tracks a page
  count and never an allocation.

This is the WebAssembly linear-memory split — a runtime that grows bounded,
zero-filled pages under a host-enforced limit, and a language runtime that
allocates inside them — with one deliberate divergence. WebAssembly programs
address memory by offset, so a runtime may relocate the base on growth; native
`SLIMECMP` code cannot, so the base is pinned and the reservation is fixed.

### C10.1 — Task-private growable memory mechanism

**Status:** Not started.

**Depends on:** B9.

#### Deliverables

- reserve a fixed per-task private-memory window in the component address space,
  clear of the image, the shared-buffer mapping convention, and the stack, with
  unmapped address space on both sides serving as its guard;
- add one growth syscall taking a page delta and returning the previous page
  count, with distinct structured errors for delta overflow, reservation
  overrun, quota exhaustion, and frame exhaustion;
- back each new page with a freshly zeroed frame mapped user/read-write/
  no-execute, and never move the base;
- make growth all-or-nothing: a failure part-way through unmaps and returns
  every frame the attempt took, leaving the page count and every existing
  mapping unchanged;
- charge growth to a per-task page count bounded by both the declared quota and
  a fixed kernel-wide ceiling, and return every page on termination through the
  reclamation path B9 establishes;
- treat a zero delta as a size query, so an allocator can read its current
  extent without a second call.

#### Required checks

- growth returns the previous page count and every new page reads as zero;
- the base address and previously written contents survive repeated growths;
- leaf mappings carry user, write, and no-execute, and never an executable bit;
- a component with no budget entry cannot grow at all;
- quota overrun, reservation overrun, and delta overflow each fail with their
  own structured error and leave the page count unchanged;
- frame exhaustion part-way through a multi-page growth returns every frame it
  had taken, observable as an unchanged free-frame count;
- the kernel-wide ceiling holds across several components, and one component's
  exhaustion leaves every other component's region intact;
- termination returns every private page, observable as a free-frame count that
  comes back to its pre-spawn value.

#### Planned verification target

```sh
just private_memory_check
```

#### Exit condition

A task grows a private region repeatedly at a fixed base, reads zeros from every
new page, cannot obtain an executable mapping of it, fails closed on quota,
reservation, overflow, and frame exhaustion with no partial effect, and returns
every page to the frame allocator when it terminates.

### C10.2 — Generation-declared private-memory budget

**Status:** Not started.

**Depends on:** C10.1.

#### Deliverables

- define a versioned Zutai private-memory budget resource under `../contracts/`,
  carried as a generation `KIND_RESOURCE` object and authenticated by the
  existing object digest table, reusing the domain-separated holder identity,
  sorted unique entries, and bounded holder count of `shared-buffer-budget/v1`
  rather than widening that contract;
- validate the resource eagerly while decoding the generation, so a malformed,
  unsorted, duplicated, or globally impossible budget fails the whole generation
  closed before any component launches;
- reject aggregate over-commitment as B8 requires: the summed holder quotas must
  fit the kernel-wide ceiling, so a budget that validates is one the kernel can
  honour with every holder at its ceiling at once;
- install each component's quota at spawn and leave a holder absent from the
  resource at its deny-by-default zero;
- mirror the encoding and bound rules host-side, so builder/kernel drift fails
  in `just generation_check` instead of at boot.

#### Required checks

- malformed, unsorted, duplicated, over-bound, and aggregate-over-committed
  budgets each fail generation decode;
- two builds from identical normalized input emit byte-identical resource bytes
  and object identities;
- a component named in the budget boots with exactly its declared ceiling, and
  one omitted from it grows nothing;
- lowering one holder's declared quota lowers exactly that holder's ceiling and
  leaves every other holder unchanged;
- a generation declaring no budget at all boots with every component denied.

#### Planned verification target

```sh
just private_memory_check
```

#### Exit condition

One authenticated generation resource fixes every component's private-memory
ceiling; the declared quota is the live ceiling on the running system, an
undeclared component allocates nothing, and every malformed or over-committed
budget fails the generation closed rather than degrading into
first-come-first-served.

### C10.3 — Userspace allocator and live quota evidence

**Status:** Not started.

**Depends on:** C10.2.

#### Deliverables

- add a `GlobalAlloc` to `components/runtime` backed by the private region: a
  first-fit free list ordered by address with boundary coalescing on free,
  matching the audited kernel heap, extended so a growth appends to the tail of
  the list and merges with the trailing free block;
- request growth in batches rather than per allocation, keeping the syscall ABI
  in target pages while the batching policy stays in userspace, so a later page
  profile changes no contract;
- surface exhaustion as a structured allocation failure that a component can
  observe, never a fault, silent truncation, or hang;
- add a startup self-check component proving the declared quota is live on the
  real boot path, in the same shape as the C7 shared-buffer probe, so the
  syscall is exercised by a real component and not only by kernel tests
  (backlog B5's lesson);
- leave components that declare no quota untouched: no growth call, no allocator
  use, and no change to their image or behavior.

#### Required checks

- `Vec`, `Box`, and `String` work in a component across reallocation, including
  growth that crosses a batch boundary;
- the allocator requests growth only when its free list cannot serve a request,
  and freed memory is reused without further growth;
- an allocation beyond the declared quota fails structurally and the component
  stays alive to observe it;
- the startup probe fails the boot when the generation granted a quota the
  kernel does not honour;
- a zero-quota component that never allocates is byte-identical in behavior to
  its pre-C10 build.

#### Planned verification target

```sh
just private_memory_check
```

#### Exit condition

A real component allocates and frees dynamically sized data through ordinary
Rust collections under its generation-declared ceiling, reuses freed memory
without growing, observes exhaustion as a structured error rather than a fault,
and proves its quota live at startup on the ordinary boot path.

### C10.4 — Adoption, reclamation, and leak evidence

**Status:** Not started.

**Depends on:** C10.3.

#### Deliverables

- convert at least one existing worst-case-sized static buffer in a real
  component to input-proportional allocation, removing the reserved `.bss` from
  every generation that carries it;
- drive a repeated spawn/exit workload through Dango's command path, where the
  B9 leak and any C10 reclamation defect both manifest, and record the
  free-frame count across the cycle;
- confirm the private region is absent from every capability path: it cannot be
  named, transferred, loaned, sealed, mapped by another component, or made
  executable;
- confirm shared-buffer and private-memory accounting stay independent, so
  exhausting one leaves the other's declared ceilings intact.

#### Required checks

- a repeated spawn/exit cycle returns the free-frame count to its starting value
  and does not drift across iterations;
- a component holding a private region and a shared buffer at once charges each
  to its own account, and exhausting either leaves the other usable;
- no syscall, transfer descriptor, or spawn grant can name another component's
  private region;
- the converted component behaves identically to its static-buffer predecessor
  on the same inputs while its image declares less `.bss`.

#### Planned verification target

```sh
just private_memory_check
```

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

No core-runtime result by itself claims ROS wire interoperability, a non-x86 boot, or physical real-time performance. Architecture-qualified releases additionally satisfy the corresponding P1, P2, or P3 gate.
