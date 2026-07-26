# Core runtime track

**Status:** C7 reopened. C7.1–C7.7 all landed and every gate now passes, including the full-graph boot checks: a 2026-07-26 audit found the C7.5 boot wedge (backlog B3), which is **fixed** — `just transfer_check`, `just spawn_service_check`, and `just dango_check` are green again. The gate stays open on evidence, not on a red suite: the C7.3/C7.7 exit conditions are still proven only inside the kernel test harness, because no generation declares a shared-buffer budget and no `SharedBufferFactory` is minted (backlog B4). Remaining items B4–B8 in `roadmap/00-backlog.md`; see `devlog/2026-07-26-c7-audit/`. C8 does not open until B4 closes.

This track turns the existing bounded channels, capabilities, components, and generations into a native typed communication runtime. It is local-first: C7 and C8 require no network or physical driver, and they do not wait for unrelated display, audio, wireless, or GPU work.

ROS 2 compatibility in [`03-ros2-compatibility.md`](03-ros2-compatibility.md) is a userspace profile over this runtime. The kernel never learns nodes, topics, services, actions, graph discovery, message types, or transport QoS policy.

## Boundaries

- Kernel IPC remains a small control plane. The current 64-byte message bound is not enlarged for sensor or image data.
- Bulk samples live in bounded shared buffers referenced by typed control messages.
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
4. H2 consumes C7's generation-v3/shared-buffer foundation and P1's extracted architecture/platform boundary for userspace drivers.
5. ROS R1 consumes C8 and H6 networking; it does not block C9 and its initial wire-conformance gate does not require a non-x86 boot.

## C7: Bounded resource and shared-sample plane

**Status:** Reopened 2026-07-26. Decomposed into C7.1–C7.7 so each slice introduces one primary state surface and owns an independently reviewable QEMU check, mirroring the M5/M6 sub-slice convention. Every gate now passes, including the full-graph boot checks: C7.5's boot wedge is fixed (backlog B3, resolved). The milestone gate remains open because the shared-buffer plane is dormant on the live boot path — no generation declares a `shared-buffer-budget/v1` resource, no `SharedBufferFactory` is minted, and no test reaches the `SYS_SHARED_BUFFER_*` syscalls (backlog B4/B5), so C7.3's and C7.7's exit conditions hold only against in-harness tables and `u64` owner constants. Evidence: `devlog/2026-07-26-c7-audit/` and `devlog/2026-07-26-b3-shared-buffer-table-stack-overflow/`.

**Depends on:** the M6 endpoint factory, spawn accounting, supervision, and generation machinery.

**Sequencing:** C7.1 lands the v3 generation format and `u64` rights that every later slice consumes. C7.2 introduces the shared-buffer capability objects and factory-authorized allocation under fixed kernel bounds. C7.3 adds generation-declared quotas and supervision-subtree accounting. C7.4 adds map, unmap, and irreversible read-only sealing. C7.5 adds loan/return ownership and fault reclamation. C7.6 defines and validates the sample-descriptor contract over that lifecycle. C7.7 composes the slices into the two-component exit condition and owns `just sample_plane_check`.

### C7.1 — Generation format v3 and u64 rights

**Status:** Complete with two corrections (2026-07-26 audit). Generation format v3 with `u64` rights is built and byte-identical across two builds; retained v2 generations still **decode** (dual-version decode + version-branched authority hash) — the "and boot" half of the original claim is unproven, since the builder emits v3 only and every v2 artifact is hand-built in memory (backlog B6). The kernel constant is renamed to `RIGHT_BUFFER_MAP`, but the manifest right is still spelled `map` in `scripts/build/build-generation.py`, so the rename did not reach the host vocabulary (backlog B7). Verified under `just generation_check`, `just contracts_check` (including boot-contracts v2/v3 decode tests), and `just test`; `just transfer_check` passed at C7.1 but regressed at C7.5 (backlog B3).

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

A v3 generation built from normalized input is byte-identical across two builds, boots the existing vertical slice with `u64` rights, and a retained v2 known-good artifact still decodes and boots; an unsupported version and an unknown rights bit both fail closed.

### C7.2 — Shared-buffer authority and factory allocation

**Status:** Complete (mechanism), with a coverage caveat from the 2026-07-26 audit. A distinct `SharedBufferFactory` kernel object gates `SYS_SHARED_BUFFER_CREATE`/`SYS_SHARED_BUFFER_RELEASE` behind `RIGHT_BUFFER_CREATE`; buffers carry a kernel-assigned unforgeable identity and only narrow-only `RIGHT_BUFFER_WRITE`/`RIGHT_BUFFER_MAP`/`RIGHT_TRANSFER`. Allocation is bounded by fixed global ceilings (`MAX_SHARED_BUFFERS`=32, `MAX_TOTAL_PAGES`=256, `MAX_BUFFER_PAGES`=64) checked before any frame is pulled, returning structured `SharedBufferError`; DMA and shared-sample authority remain distinct capability kinds. Caveat: no `SharedBufferFactory` is ever minted on the live boot path (backlog B4) and neither syscall is reachable from any test — the gate exercises `CapabilityTable` and `SharedBufferTable` directly, so the rights gate and the create-insert-failure rollback are uncovered (backlog B5). Verified under `just shared_buffer_factory_check` (8 QEMU cases), with `just test`, `just contracts_check`, `just generation_check`, `just fmt_check`, `just lint`, and `just framework_safety_check` clean.

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

**Status:** Reopened 2026-07-26 — mechanism complete, exit condition unmet. A versioned Zutai shared-buffer budget contract (`contracts/shared-buffer-budget/v1/`) is *defined* to be stored as a generation `KIND_RESOURCE` object, authenticated through the generation's existing per-object digest table; it declares per-holder `byte_pages`, `buffer_count`, `mapping_count`, and `loan_count` quotas. A present budget is validated deterministically at generation decode and rejects missing, malformed, unsorted/duplicate, or per-holder-impossible limits before any component launches (the validator bounds each holder but never sums holders — backlog B8). `SharedBufferTable::create` charges each allocation to the creating supervision-subtree owner against its `HolderQuota` (deny-by-default when absent), enforced before the global ceiling and side-effect-free on rejection; `reclaim_owner` returns every unloaned page and charge on release, peer death, supervised restart, and revocation (via `task::terminate`) without disturbing another subtree. **No generation actually declares a budget:** the built `generation-1.bin` contains zero `KIND_RESOURCE` objects, `build-generation.py` has no budget emitter, and the manifest grants no `bufferCreate`, so every live holder is `HolderQuota::DENY` and the exit condition ("two holders receive distinct generation-declared budgets") holds only against in-harness tables (backlog B4). Mapping-count and outstanding-loan quotas are present and bounded for C7.4/C7.5. Verified under `just shared_buffer_accounting_check` (7 QEMU cases) plus `just contracts_check`, `just generation_check`, `just test`, `just shared_buffer_factory_check`, `just fmt_check`, `just lint`, and `just framework_safety_check` clean. See `devlog/2026-07-24-c7-3-shared-buffer-accounting.md` and `devlog/2026-07-26-c7-audit/`.

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

**Status:** Complete (mechanism), with a coverage caveat from the 2026-07-26 audit. Shared buffers now expose bounded `SYS_SHARED_BUFFER_MAP`/`SYS_SHARED_BUFFER_UNMAP`/`SYS_SHARED_BUFFER_SEAL`. Mapping installs only page-aligned, non-executable, exact-frame user PTEs for the named buffer capability, gated by `RIGHT_BUFFER_MAP` (writable additionally by `RIGHT_BUFFER_WRITE`) and charged one unit against the holder's `mapping_count` quota under `MAX_MAPPINGS`=64; offset/length/base are range- and overflow-checked and confined to the user half before any page-table change, and a partial map is fully rolled back. Sealing is an irreversible Arc-shared read-only transition that downgrades every live writable PTE before publishing the seal; a created-read-only or sealed region can never obtain a writable mapping. Unmap, release, and supervision-subtree reclamation remove the exact PTEs before returning frames, without disturbing an unrelated mapping. Caveat: none of the three syscalls is reachable from any test, so the `RIGHT_BUFFER_MAP`/`RIGHT_BUFFER_WRITE` gates are proven only as `valid_rights`/`derive` behavior, not at the syscall boundary (backlog B5). Verified under `just shared_buffer_mapping_check` (8 QEMU cases), with `just test`, `just shared_buffer_accounting_check`, `just shared_buffer_factory_check`, `just contracts_check`, `just generation_check`, `just fmt_check`, `just lint`, and `just framework_safety_check` clean.

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

**Status:** Complete (mechanism); its boot regression is fixed. C7.5 originally wedged every full-graph boot — a 10520-byte `SharedBufferTable` published through a `LazyLock` was first constructed on a 32 KiB unguarded task kernel stack inside `task::terminate`, overflowing it silently so the ready queue never drained to `on_idle`. Fixed 2026-07-26 by const-initializing the table into `.bss` (backlog B3, resolved; `devlog/2026-07-26-b3-shared-buffer-table-stack-overflow/`). Bounded loan/return over an exact sealed subrange lands as `SYS_SHARED_BUFFER_LOAN`/`SYS_SHARED_BUFFER_LOAN_MAP`/`SYS_SHARED_BUFFER_RETURN`/`SYS_SHARED_BUFFER_REVOKE` behind the new object-specific `RIGHT_BUFFER_LOAN` (bit 25) and a receiver-bound `SharedBufferLoan` kernel object. A loan requires an irreversibly sealed source region, names its receiver through a `RIGHT_SUPERVISE` capability (never an ambient task id), charges one unit against the lender's `loan_count` quota under `MAX_LOANS`=64, and carries a kernel-assigned unforgeable single-return identity. `release_by` retains the creator's pages and buffer charge while any loan is outstanding; the final settle finalizes the region. `map_loan` confines the receiver to the loaned subrange and is always read-only; duplicate, stale, and wrong-buffer returns fail closed without changing accounting. `reclaim_owner` settles every loan naming a dying task as lender or receiver. Caveat: none of the four new syscalls is reachable from any test (backlog B5); the gate drives `SharedBufferTable` directly. Verified under `just shared_buffer_loan_check` (7 QEMU cases) plus, after the fix, `just transfer_check`, `just spawn_service_check`, and `just dango_check` — the full-graph boot gates that were not run for this slice, which is how the regression shipped.

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

**Status:** Reopened 2026-07-26 — the gate does not compose what the exit condition names. `kernel/tests/sample_plane.rs` composes the C7.2 factory allocation, C7.3 per-holder quotas, C7.4 mapping/sealing, C7.5 loan/return lifecycle, and the C7.6 sample descriptor into two **holder ids** that exchange a `>MAX_MSG` payload: only the 64-byte descriptor crosses a real IPC channel while the receiver reconstructs the full two-page payload from the quota-charged sealed loaned buffer through exact read-only page-table translations. A malformed (stale-identity) descriptor delivered over the channel is rejected by validation and by the loan-aware map path before any mapping or allocation, leaving the loan intact. Every quota class (byte-pages, buffer-count, mapping-count, loan-count) fails with `QuotaExceeded` at ceiling+1 without disturbing an unrelated owner's buffer, mapping, or channel; and a retained v2 known-good generation decodes byte-identically before and after a full sample-plane exchange. **The "two isolated components" are the `u64` constants `LENDER = 0x71` and `RECEIVER = 0x72`** — the test never spawns a task, and "peer death" is a direct `reclaim_owner(RECEIVER)` call rather than a termination, so the real reclamation wiring in `task::terminate` is never executed by this gate (backlog B5). The retained-v2 arm is a decode probe, not a boot (backlog B6). Verified under `just sample_plane_check` (5 QEMU cases), with `just test`, `just fmt_check`, and `just lint` clean; the full-graph boot gates are red at this commit (backlog B3). See `devlog/2026-07-26-c7-audit/`.

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

**Status:** Not started.

**Depends on:** C7's bounded sample plane, and backlog item **B2** (scheduler
`Blocked` state / `SYS_WAIT` wait-set). C8's fabric service is the first
long-lived component that no scripted keystroke can terminate, so B2 must land
before this gate opens; otherwise C8's stalled-subscriber, peer-death, and
graph-idle exit conditions cannot be observed under QEMU. C7.2–C7.7 do not
depend on B2 and proceed first.

### Deliverables

- define one deterministic `InterfaceSchema` identity derived from a normalized, bounded schema; equivalent input produces one type identity and conflicting layouts cannot reuse it;
- generate or deterministically validate bindings for three native contracts: `Stream<T>`, `Call<Request, Reply>`, and `Operation<Goal, Feedback, Result>`;
- implement a userspace fabric service that creates per-route endpoint capabilities from generation-declared graph grants; publishers receive only send authority, subscribers only receive authority, and clients cannot mint graph edges themselves;
- implement bounded many-to-many streams and request/reply correlation over ordinary channels, using C7 shared samples when payloads exceed the control-message bound;
- define `TransportQoS` with explicit bounds: KEEP_LAST depth, RELIABLE or BEST_EFFORT delivery, VOLATILE or bounded retained durability, deadline, lifespan, liveliness kind, and lease duration;
- implement requested/offered compatibility, matched/unmatched notifications, incompatible-QoS events, loss/expiry reporting, peer-death propagation, and fixed retry/history/resource ceilings;
- expose graph introspection through a read-only service whose result is filtered to the caller's declared graph visibility; a name or type string is never authority;
- make every route, queue depth, sample-size bound, publisher/subscriber count, retained-history count, retry limit, and event-queue size generation data;
- support transparent userspace interposition so a declared recorder, replay membrane, or protocol gateway receives exactly the narrowed route capabilities it proxies.

### Required checks

- publishers and subscribers match only when name, type identity, and requested/offered QoS are compatible;
- an ungranted component cannot create, discover, publish, subscribe, call, serve, or inspect the protected route;
- alternate names with the same type and conflicting types with the same name do not alias authority;
- KEEP_LAST evicts deterministically at the declared depth, BEST_EFFORT may report loss without retry growth, and RELIABLE exhausts a fixed retry budget with a structured error;
- a stalled subscriber cannot grow publisher, broker, buffer, or event memory beyond manifest bounds;
- deadline, lifespan, liveliness loss, incompatible QoS, and peer death remain distinguishable events;
- one publisher or fabric client may fault without terminating another route, the fabric service, or the kernel;
- a fixed graph and input sequence produces byte-identical normalized schema artifacts and deterministic IPC trace records.

### Planned verification target

```sh
just data_fabric_check
```

### Exit condition

A generation-declared graph of isolated native publishers, subscribers, service clients, and servers exchanges bounded typed data under explicit QoS and graph grants; denied graph edges are neither usable nor visible, incompatible endpoints do not match, and a stalled or faulting participant cannot exceed its quota or disrupt unrelated routes.

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
