# Core runtime track

**Status:** Not started.

This track turns the existing bounded channels, capabilities, components, and generations into a native typed communication runtime. It is local-first: C7 and C8 require no network or physical driver, and they do not wait for unrelated display, audio, wireless, or GPU work.

ROS 2 compatibility in [`03-ros2-compatibility.md`](03-ros2-compatibility.md) is a userspace profile over this runtime. The kernel never learns nodes, topics, services, actions, graph discovery, message types, or transport QoS policy.

## Boundaries

- Kernel IPC remains a small control plane. The current 64-byte message bound is not enlarged for sensor or image data.
- Bulk samples live in bounded shared buffers referenced by typed control messages.
- Topic names and types are userspace metadata. Authority is carried by SEND/RECV endpoint capabilities minted or distributed by the declared fabric service.
- The generation declares which component may publish, subscribe, call, serve, inspect, or administer each graph edge.
- `TransportQoS` controls message delivery. `SchedulingClass` controls CPU ordering. They are separate contracts and namespaces.
- Slime capability transfer is native-only. A protocol gateway may retain and proxy a capability but may never serialize a kernel capability as application data.

## Sequencing

1. C7 consumes the M6 endpoint factory, spawn accounting, supervision, and generation machinery.
2. C8 consumes C7's bounded sample plane.
3. C9 consumes C8 plus the scheduler and time mechanisms from M1/M2.
4. H2 consumes C7's generation-v3 and shared-buffer foundation for userspace drivers.
5. ROS R1 consumes C8 and H6 networking; it does not block C9.

## C7: Bounded resource and shared-sample plane

**Status:** Not started.

### Deliverables

- introduce deterministic generation format v3 with `u64` rights; retain decoding of known-good v2 generations for the bounded rollback window rather than changing v2 meanings;
- migrate manifest rights strings deterministically and reject unknown or meaningless v3 rights bits;
- replace the grandfathered generic `RIGHT_MAP` name with an object-specific shared-buffer map right when the v3 mapping lands;
- add a named `SharedBufferFactory` capability with generation-declared per-holder byte, buffer-count, mapping-count, and outstanding-loan quotas;
- expose bounded create, map, unmap, seal/read-only, loan, return, and release operations; userspace cannot invent buffer identity or widen access while deriving or transferring it;
- define a versioned sample-descriptor contract that fits the existing channel control-message bound and references an exact shared-buffer capability, offset, length, type identity, sequence, and declared flags;
- charge physical pages and mappings to the creating supervision subtree, retain them while any valid loan is outstanding, and reclaim them after return, peer death, supervised restart, or explicit revocation;
- keep DMA buffers and ordinary shared samples as distinct authority even if they reuse memory-accounting machinery.

### Required checks

- a component without the factory capability cannot allocate a shared buffer, and a holder cannot exceed any manifest quota;
- a receiver cannot map bytes outside the granted buffer or widen read-only access to writable;
- overflowed offset/length, unknown flags, stale loans, duplicate returns, wrong-buffer returns, and use-after-release fail with structured errors before mapping or allocation;
- the creator cannot reclaim pages while a sample loan is outstanding; peer death and supervised restart reclaim every unreachable mapping and charge;
- transferring or deriving a buffer checks `RIGHT_TRANSFER` and never widens buffer rights;
- payloads larger than the kernel message bound traverse descriptor plus shared buffer without increasing `MAX_MSG` or copying payload bytes through the kernel queue;
- two builds from identical normalized v3 input are byte-identical, retained v2 known-good artifacts still boot during the rollback window, and unsupported versions fail closed.

### Planned verification target

```sh
just sample_plane_check
```

### Exit condition

Two isolated components exchange and return a payload larger than the kernel IPC message bound through a quota-charged shared buffer; malformed descriptors, quota exhaustion, and peer death remain bounded, reclaim all resources, and do not disturb an unrelated channel or the retained v2 known-good boot path.

## C8: Native typed data fabric

**Status:** Not started.

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

### Deliverables

- expose monotonic time, optional wall time, timers, and simulated time as distinct explicit service capabilities; a component with no clock grant cannot observe time implicitly;
- implement userspace wait sets/executors over stream, call, operation, timer, supervision, and QoS-event endpoints with bounded ready queues and deterministic tie rules;
- add manifest-declared `SchedulingClass` per component or supervision subtree, initially foreground, normal, and best-effort, plus conserved CPU resource accounts;
- keep scheduling mechanism in the kernel while class assignment, dynamic promotion, and workload policy remain generation/userspace decisions; a component cannot widen its own class;
- preserve class and resource-account bounds across supervised restart while issuing fresh endpoint, mapping, and device authority;
- define component lifecycle transitions, health dependencies, bounded restart/backoff policy, and parameter state as versioned userspace schemas rather than kernel policy;
- add typed recording and replay for declared fabric routes; clock, entropy, device input, and other nondeterminism must be either capability-recorded or explicitly excluded from a deterministic claim;
- build a simulated sensor → controller → actuator workload that exercises timer, stream, call, lifecycle, restart, and contention paths without special kernel treatment.

### Required checks

- a component without clock, parameter, lifecycle-control, recorder, or scheduling-promotion authority cannot exercise that operation through another ambient API;
- a best-effort workload saturating available CPU cannot claim foreground class, escape its conserved account, or prevent the declared control workload from being scheduled according to the selected class contract;
- wait-set queues, timer counts, callbacks per wake, parameter bytes, restart attempts, backoff duration, and recorded trace bytes are bounded before allocation;
- supervised restart preserves declared class and graph shape but cannot reuse stale buffers, endpoints, timers, or device mappings;
- identical recorded typed inputs, clock events, and lifecycle transitions produce identical replayed component outputs for a manifest-declared deterministic component;
- deadline misses, timer expiry, liveliness loss, process fault, peer loss, cancellation, and scheduling-budget exhaustion remain distinct at the userspace boundary.

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

No core-runtime result by itself claims ROS wire interoperability or physical real-time performance.
