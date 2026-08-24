# Raspberry Pi 5 ROS 2 two-node demo track

**Purpose:** Drive the near-term project toward one observed robotics workload: Slime OS running on a Raspberry Pi 5 with two local ROS 2 nodes exchanging bounded topic data through a minimal bounded Zenoh profile with classic CDR payloads.

**Status:** In progress — RP0, RP1, and RP2 complete. RP0 was reissued as contract format 2 when the transport pivoted from DDSI-RTPS to Zenoh; see [`devlog/2026-08-17-ros2-transport-zenoh-pivot/`](../devlog/2026-08-17-ros2-transport-zenoh-pivot/index.md). RP2 closed 2026-08-20 on `aarch64-sel4-qemu-virt`; RP3 is next and waits on P4's board qualification.

**Acceptance target:** A reproducible Raspberry Pi 5 boot runs a declared publisher node and subscriber node. The publisher emits a bounded ROS 2 topic stream through the admitted transport profile, the subscriber observes the expected samples in order under the declared QoS/profile, and the run records image identity, board/firmware/media identity, generation/release identity, serial transcript, semantic/wire trace, and every device/storage/stream/network capability involved.

This track intentionally does **not** claim full ROS 2 compatibility, arbitrary middleware discovery, unmodified desktop ROS packages, Python support, Gazebo, Wi-Fi, GPU acceleration, or Framework daily-driver support. It is the shortest defensible path to a middleware-backed two-node RPi5 demo while preserving Slime's capability, component, generation, and schema invariants.

## Boundaries

- Raspberry Pi 5 is a named physical target, not a synonym for generic Arm support.
- The first ROS 2 route is a **real ROS 2 middleware wire path first**, not a local-only `rmw_slime` shortcut. Native C8 fabric may back internal delivery, but the demo profile must define transport sessions, publishers/subscribers, key-expression mapping, CDR payloads, sequence behavior, and bounded teardown.
- The transport family is generation data. `contracts/rpi5-ros2-demo/v2` names it through a closed discriminator plus one optional profile record per admitted family, so replacing Zenoh with DDSI-RTPS — or the reverse — is a fixture change, not a contract migration. A userspace transport that could only be swapped by rewriting the acceptance contract would not be replaceable.
- R0 uses static generation-declared peers, fixed endpoints, fixed key expressions, and one admitted message type. Arbitrary LAN discovery, Zenoh routers, gossip or multicast scouting, liveliness tokens, transport security, multiple middleware vendors, services/actions, and unmodified existing ROS packages are later milestones.
- ROS node names, topic names, types, parameters, executors, QoS policy, and graph metadata stay in userspace. The kernel remains unaware of ROS and middleware concepts.
- Stream, datagram, or network authority is explicit generation data. A loopback endpoint, TCP port, multicast group, or peer locator grants no authority unless the generation names it, and no component in the demo graph holds router, gossip, scouting, or discovery authority at all.
- QEMU AArch64 evidence is required to de-risk architecture work, but a physical Raspberry Pi 5 run is the only exit evidence for board support.
- Storage starts from reproducible removable media with no claim of internal-device writes. Any persistent state used by the demo is generation-declared and rollbackable.

## Sequencing

1. RP0 freezes the demo acceptance contract so every later milestone has a stable target, including the transport profile.
2. RP1 consumes P0/P1 target and architecture-boundary work so every executable is admitted for one exact profile.
3. RP2 proves the architecture-neutral capability, component, and generation semantics on `aarch64-sel4-qemu-virt`, where upstream seL4 supplies the privileged mechanism.
4. RP3 promotes the minimum physical Raspberry Pi 5 boot path: firmware handoff, serial, memory map, timer, interrupt controller, and device tree.
5. RP4 replays the two-component C7/C8 data path on Arm and then on the board.
6. RP5 adds the minimum node runtime plus stream/network envelope needed to host the ROS 2 node route.
7. RP6 implements the minimal transport topic profile and packages the two ROS 2 nodes on the Slime component model.
8. RP7 records the first complete physical middleware-backed data-transfer demo.
9. RP8 makes the demo repeatable and bounded enough to serve as the new roadmap baseline.

## RP0 — Demo contract and acceptance fixture

**Status:** Complete.

**Depends on:** Cleared backlog, completed M6/C7 baseline, and the C8.10 full-graph boot baseline as current regression evidence.

### Deliverables

- pin one exact Raspberry Pi 5 hardware revision or accepted revision set, boot firmware path, removable media path, serial/UART path, memory-map source, interrupt controller, generic timer, and minimum device list;
- choose the ROS 2 distribution baseline. Format 2 pins Kilted Kaiju, because REP-2000 lists `rmw_zenoh_cpp` Tier 1 under Kilted and Rolling and omits it from the Jazzy Jalisco middleware table; format 1 pinned Jazzy for the DDSI-RTPS profile;
- define ROS 2 Profile 0: transport family, wire protocol version, session mode, link protocol, domain id, endpoints, key-expression format, discovery mode, admitted session and declaration messages, QoS subset, message attachment layout, sequence/retry behavior, teardown behavior, and resource ceilings;
- define the two-node workload: node names, topic name, message type, publish count/rate or event sequence, QoS/profile, expected subscriber output, the exact CDR bytes of every sample, and all allowed failure markers;
- define a versioned bounded semantic/wire trace for demo evidence under `contracts/`, or explicitly reuse an existing C8 trace contract if it already covers the needed records;
- write the exact operator-visible success condition so a serial transcript can distinguish success, denial, timeout, wrong-board, wrong-target, wrong-transport, malformed-wire, malformed-payload, and malformed-generation failures.

### Required checks

- the demo fixture rejects unknown board/firmware/profile/transport identifiers instead of silently selecting a nearby target;
- the ROS 2 route states the exact RMW boundary and whether nodes are unmodified upstream packages, source-compatible ports, or Slime-native ROS-compatible components;
- every message, string, sequence, key expression, attachment, trace, fragment, queue, and retry bound is known before activation;
- every wire constant the fixture freezes is derived from its upstream definition by the gate rather than transcribed, and the derivation is itself validated against an upstream fixture;
- the fixture names every capability the two nodes, transport runtime, board services, storage path, stream/network path, and trace sink may hold, and names no discovery authority.

### Planned verification target

```sh
just rpi5_ros2_demo_contract_v2_check
```

### Exit condition

The repository contains a bounded, versioned, target-qualified demo contract that says exactly what “two ROS 2 nodes exchange middleware-backed topic data on Raspberry Pi 5” means and how success or failure is observed, with the transport family carried as replaceable generation data.

## RP1 — Target-qualified build and admission path

**Status:** Complete.

**Depends on:** P0 and P1 from [Architecture portability](07-architecture-portability.md).

### Deliverables

- make the generation `target`, release metadata, kernel image, component image, transport runtime, and node executable closure identify the exact `aarch64-rpi5` profile;
- reject wrong-architecture kernels, components, ROS node artifacts, transport runtime artifacts, page profiles, required ISA features, and ABI variants before mapping executable bytes;
- parameterize Cargo targets, linker scripts, image conversion, component packaging, object identities, and QEMU/physical image builders by exact profile;
- keep architecture-neutral resources byte-identical where valid, while executable objects and complete generations become target-specific;
- document the syscall calling convention and userspace runtime ABI for AArch64 without changing the semantic syscall table.

### Required checks

- an x86-64 component or kernel cannot be staged into an AArch64/RPi5 generation even if its content hash is otherwise valid;
- an AArch64 QEMU artifact cannot be silently accepted for the Raspberry Pi 5 profile if the page, firmware, device-tree, or required-feature contract differs;
- two builds from identical normalized RPi5 input are byte-identical;
- changing only the target changes the authenticated executable and generation identities while preserving shared resource identities where allowed.

### Planned verification target

```sh
just rpi5_artifact_check
```

### Exit condition

Every executable byte in the future demo generation is admitted for one exact AArch64/Raspberry Pi 5 profile, and wrong-target artifacts fail closed before execution.

## RP2 — AArch64 QEMU product vertical slice

**Status:** Complete.

**Delivered:** A `demo` boot action and
`contracts/generation/v1/fixtures/sel4-demo.zti` — the first generation that
carries the C7 bounded data path, the C8 stream route graph, *and* the product
component graph together, so "the component-launch and data path under one
demo-scoped generation" is a property of one admitted manifest rather than an
inference across `sel4-sample`, `sel4-stream`, and `sel4`. Both compositions are
the existing ones (`drive_sample_plane`'s exchange and `launch_fabric_graph`'s
graph): what RP2 makes new is the generation, so the slice reuses the
compositions those gates exercise — the provisioning, denial, and loan paths,
though not their scripted mid-stream death, which `build-sel4.py` arms for
`stream`/`qos`/`fault` only — and `fabric-service` needed no new branch because
`demo` falls through to its stream composition. Two build-time arms complete it:
the rollback pair reuses the B35 selector image over two *demo* generations, and
`SLIME_WRONG_TARGET_EXECUTABLE` re-qualifies one declared executable for another
admitted profile so the root's own refusal is observable on a boot.

**Exit condition (observed):** `just sel4_demo_check` boots one demo-scoped
`aarch64-sel4-qemu-virt` generation (`executables=13 instances=13 grants=26`,
`fabric graph=admitted schemas=2 routes=2 participants=6`) and observes, in one
transcript and in order: the C7 pair moving an 8192-byte sealed loan mapped
read-only and returned exactly once, the C8 sweep provisioning every declared
stream edge and denying `fabric-intruder`'s undeclared edge, and `console` and
`spawn-service` running and shutting down — terminating `SLIME_GRAPH HEALTHY
generation=1 required=4 live=0 completed=4 failed=0` with `loans=0 mappings=0
regions=0 orphans=0` and `tasks reclaimed live=0`. A failing pending demo
generation (number 99) is selected twice, consumes both attempts, and rolls back
to the verified demo known-good root (number 1) across fresh QEMU processes with
only BootState sectors mutated. A `sample-lender` image qualified for
`aarch64-rpi5` is counted `wrong_target=1`, excluded from the loadable set
(`elf=12`, `loadable_executables=12`), and its spawn fails closed with no byte of
it executed.

**Gates:** `just sel4_demo_check`, `just sel4_boot_layout_check` (26 plane
layouts), `just sel4_gate_control_check` (33 gates).

**Evidence:** [`devlog/2026-08-20-rp2-demo-scoped-arm-slice/`](../devlog/2026-08-20-rp2-demo-scoped-arm-slice/index.md)

**Depends on:** RP1 and P5. P2's custom-kernel bring-up deliverables are
superseded: seL4 supplies that mechanism, so re-deriving it is explicitly out of
scope.

## RP3 — Raspberry Pi 5 serial boot and minimum board services

**Status:** Not started.

**Depends on:** RP2 and P4's Raspberry Pi 5 qualification slice.

### Deliverables

- build a `bcm2712` seL4 kernel and loader image from the existing pins, using the
  already-present `sel4/config/bcm2712-rpi5.cmake` platform configuration that no
  build path currently selects, and pin its artifact digests the way
  `qemu-arm-virt` is pinned;
- select and document the Raspberry Pi 5 firmware/boot handoff path, image load
  address rules, device-tree source, serial console, and removable media image
  format;
- identify the board's memory regions, reserved regions, UART, GIC, generic
  timer, and any storage or datagram path the demo uses, taking them from seL4's
  bootinfo and its platform configuration rather than re-deriving a device-tree
  parser in the root;
- bring up early serial diagnostics through the root, exception reporting via
  seL4's fault messages, timer interrupts, and an operator-visible stop behavior
  on physical hardware;
- preserve the no-ambient-storage boundary: the first board demos boot from
  reproducible removable media and do not claim unqualified writes to other
  devices;
- record board revision, firmware version, image identity, generation identity,
  serial output, and the resolved platform/device identity.

### Required checks

- wrong board revision, an unsupported page/interrupt/timer profile, or an
  incompatible firmware handoff fails with bounded diagnostics rather than a hang;
- timer interrupts and serial logging continue after the root activates the
  component graph;
- a faulting early component is reported on serial without wedging the board;
- pre/post storage evidence shows no write to any device not explicitly granted by the demo image;
- QEMU AArch64 success is cited only as inherited architecture evidence and not as physical board completion.

### Planned verification target

```sh
just rpi5_boot_check
```

### Exit condition

A named Raspberry Pi 5 boots a verified Slime generation from reproducible media, reaches the scheduler with serial diagnostics and timer/interrupt handling live, and records enough evidence to distinguish this board claim from generic AArch64 QEMU support.

## RP4 — Arm component data path on QEMU and Raspberry Pi 5

**Status:** Not started.

**Depends on:** RP2, RP3, C7, the C8 stream/fabric slices consumed by the transport runtime, and [CP5](10-component-platform.md#cp5--out-of-tree-component-development-proof) from the Component platform track.

### Deliverables

- run two isolated AArch64 components that exchange bounded typed data through the same C7/C8 path the ROS nodes will use behind their publisher/subscriber roles;
- author and build both components entirely outside this repository, through CP5's out-of-tree component SDK path, rather than as `components/bins` in-tree binaries; this requirement is scoped to RP4's two components only and does not extend to RP6's ROS 2 node components;
- exercise inline samples and, if the demo message can exceed the IPC control bound, a shared-buffer-backed sample descriptor;
- prove endpoint, buffer, mapping, loan, event, and queue accounting on Arm rather than inheriting x86-only evidence;
- record the same scenario under `aarch64-qemu-virt` and on Raspberry Pi 5, with physical traces labeled separately;
- keep this probe below the transport layer so architecture/data-path failures remain separable from transport/RMW failures.

### Required checks

- the publisher component cannot receive or re-delegate subscriber authority, and the subscriber cannot publish unless explicitly granted;
- malformed descriptors, wrong type tags, quota exhaustion, peer death, and route denial fail closed and reclaim resources;
- the Raspberry Pi 5 run observes the same semantic data-transfer records as the AArch64 QEMU run, excluding architecture-specific register and address detail;
- the board remains responsive after the exchange and emits an operator-visible completion marker;
- CP5's own required checks pass for these two components specifically: their build uses only the published/vendored SDK with no path reference into this repository's `components/` directory, and removing their out-of-tree checkout and rebuilding from in-tree fallback components still passes every other check in this list.

### Planned verification target

```sh
just rpi5_data_path_check
```

### Exit condition

Before the transport and ROS layers are introduced, two isolated components — authored and built entirely outside this repository against the CP5 component SDK — exchange the demo-shaped bounded data path on AArch64 QEMU and on Raspberry Pi 5 with explicit route authority and resource reclamation.

## RP5 — Node and transport runtime envelope

**Status:** Not started.

**Depends on:** RP4, C10's private memory (complete), and the subset of clock/timer and datagram/network service required by R0. The clock/timer and executor halves are [C9.1](02-core-runtime.md#c91--explicit-clock-and-timer-service-authority) and [C9.2](02-core-runtime.md#c92--bounded-userspace-wait-sets-and-executors); RP5 needs those two slices, not the whole C9 track.

### Deliverables

- provide the minimum allocator, startup, argument/configuration, clock/timer, logging, executor/wait-set, and lifecycle hooks required by the two ROS 2 nodes and the minimal transport runtime;
- provide the exact bounded byte-stream path the transport needs — for Zenoh Profile 0, one TCP link carrying length-prefixed batches — without granting raw packets or wildcard destinations;
- package node and session configuration as deterministic generation data rather than environment variables, global paths, package indexes, or host filesystem state;
- map every needed clock, parameter, filesystem/object, graph, session, endpoint, and logging operation to an explicit capability or a stable structured denial;
- bound heap pages, executor queues, timers, log bytes, parameter bytes, transport receive queues, publisher/subscriber history, batches, retries, and outstanding messages before activation;
- keep unsupported POSIX, dynamic loading, package discovery, router or scouting-based discovery, plugin behavior, or vendor-specific ambient middleware configuration visibly unsupported rather than accidentally ambient.

The envelope must stay within the mechanisms this repository already has, and one half of that constraint lifted with C10. A real general-purpose heap now exists: `components/runtime/src/private_heap.rs` is a free-list `GlobalAlloc` over the task-private region with a working `dealloc`, behind the `private-heap` feature and bounded by a generation-declared page quota — `fabric-service` ships on it. The bump allocator in `components/runtime/src/heap.rs`, whose `dealloc` is a no-op, remains the default for components that do not opt in. What is still absent is an executor: there is no async runtime anywhere in `components/`, and the bounded wait set that would replace one is [C9.2](02-core-runtime.md#c92--bounded-userspace-wait-sets-and-executors). So a transport implementation assuming an async runtime is still out of scope for RP5, while one assuming a heap is now merely a declared quota. Profile 0's blocking, fixed-buffer shape is chosen to need neither.

### Required checks

- a node can allocate, initialize, spin, publish/subscribe through the transport runtime, log, and exit within its declared resource budget;
- missing clock, route, session, endpoint, stream/network, parameter, file, or logging authority fails with a named error rather than fallback state;
- an allocation, receive queue, batch buffer, or retry beyond the declared quota leaves the node/runtime alive enough to report the failure when the selected API permits it;
- two node instances receive distinct endpoint, session, publisher/subscriber, and resource authority on restart and cannot reuse stale handles.

### Planned verification target

```sh
just rpi5_node_runtime_check
```

### Exit condition

The minimal ROS 2 node route has a bounded Slime runtime envelope sufficient for two local nodes, with no hidden dependency on ambient filesystem, environment, network, package-discovery, clock, or middleware discovery authority.

## RP6 — Minimal transport topic profile and two ROS 2 nodes

**Status:** Not started.

**Depends on:** RP5 and R0 from [ROS 2 compatibility](03-ros2-compatibility.md).

### Deliverables

- define the local ROS 2 transport profile for nodes, names, topic mapping, admitted message type, QoS subset, type identity, CDR representation, session/publisher/subscriber identities, key expressions, endpoints, and static declaration;
- package the publisher node and subscriber node as target-qualified generation components with explicit startup configuration and grants;
- map their publisher/subscriber roles onto the C8 fabric and bounded stream service without giving either node ambient discovery, graph mutation, filesystem, wildcard network, or broader route authority;
- expose enough ROS 2-visible node behavior to justify calling them ROS 2 nodes under the RMW boundary, and document every unsupported ROS and transport feature outside the demo profile;
- add host or QEMU fixtures that compare ROS name mapping, key-expression composition, CDR bytes, message attachment bytes, session and declaration framing, message field values, ordering, and QoS behavior against the pinned RP0 contract.

### Required checks

- the publisher sends only the declared topic/type through its declared key expression and endpoint; the subscriber receives only through its declared key expression and endpoint;
- alternate domain, session, endpoint, topic name, type, type hash, node name, QoS, direction, or undeclared graph operation fails without exposing protected graph metadata;
- a wildcard key expression, a router endpoint, and a scouting attempt are each rejected with a named denial;
- message serialization and transport framing are deterministic and bounded;
- repeated node/runtime restart does not retain stale sessions, endpoints, loans, buffers, timers, batches, retries, or parameters;
- the same node generation boots and passes the R0 transport profile under AArch64 QEMU before being promoted to Raspberry Pi 5.

### Planned verification target

```sh
just rpi5_ros2_zenoh_nodes_check
```

### Exit condition

Two target-qualified local ROS 2 node components exchange the declared bounded topic through the minimal bounded Zenoh profile with classic CDR payloads under AArch64 QEMU, with every unsupported ROS and transport feature denied explicitly.

## RP7 — Observed Raspberry Pi 5 ROS 2 data-transfer demo

**Status:** Not started.

**Depends on:** RP6 and RP3/RP4 physical board evidence.

### Deliverables

- build one reproducible Raspberry Pi 5 demo image containing the verified kernel, runtime services, transport runtime, publisher node, subscriber node, generation graph, stream/network grants, and demo trace sink;
- boot the image on the named board and run the middleware-backed two-node exchange without manual patching after boot;
- record serial output, semantic trace, transport wire trace records, generation/release identity, node/component identities, board/firmware/media identity, and storage/no-write evidence;
- normalize the evidence so success can be compared across repeated runs without depending on raw physical addresses, task ids, or timing jitter;
- include negative markers for wrong target, wrong route, wrong transport, wrong session/endpoint, timeout, malformed wire framing, malformed payload, denied grant, and node failure so absence of success is not silent.

### Required checks

- the publisher emits the declared number or sequence of topic samples and the subscriber observes the expected values/order within declared bounds;
- the run reaches a single operator-visible success marker only after the subscriber has validated the data and the trace includes the expected session and publisher/subscriber events;
- no component holds a capability outside the RP0 manifest contract;
- the board remains in a known state after success or failure and does not write ungranted storage;
- the captured evidence is sufficient for a reviewer to distinguish actual middleware-backed node data transfer from a boot-only, print-only, or C8-only demo.

### Planned verification target

```sh
just rpi5_ros2_demo_check
```

### Exit condition

A Raspberry Pi 5 physically runs Slime OS with two local ROS 2 nodes exchanging the declared data through the minimal bounded Zenoh profile, and the repository contains reproducible evidence of the board, image, generation, capabilities, semantic/wire trace, and serial success marker.

## RP8 — Repeatability, bounds, and demo baseline hardening

**Status:** Not started.

**Depends on:** RP7.

### Deliverables

- repeat the physical demo across cold boots and warm restarts, recording normalized semantic/wire traces and resource high-water marks;
- inject or simulate denied route, denied endpoint, malformed batch length, malformed declaration, malformed CDR payload, malformed attachment, subscriber restart, publisher restart, queue exhaustion, retry exhaustion, timer delay, and peer-loss cases relevant to the demo profile;
- verify every endpoint, buffer, mapping, loan, timer, heap page, queue entry, fragment, retry record, writer/reader history entry, and trace record returns to its declared baseline after normal completion or restart;
- make the demo gate reproducible enough that later roadmap work can use it as a regression target;
- write a devlog entry with the evidence chain before declaring the near-term release closed.

### Required checks

- repeated runs produce the same normalized success trace and bounded resource highs;
- each injected failure has a distinct structured failure marker and does not wedge the board or corrupt unrelated state;
- supervised restart gives each node and transport session fresh authority and replays no stale samples;
- the demo remains insensitive to host environment, file paths, unrelated network state, and unrelated devices;
- every physical run records enough provenance to reproduce the image and generation.

### Planned verification target

```sh
just rpi5_ros2_demo_stress_check
```

### Exit condition

The Raspberry Pi 5 two-node ROS 2 demo is repeatable, bounded, and reviewable: normal runs match their normalized trace, failure cases are distinguishable, resources are reclaimed, and the physical evidence is strong enough to become the new baseline for subsequent roadmap work.

## Relationship to later tracks

- R1/R2 broaden the minimal R0 transport path into external peer, service, and action compatibility after RP8 unless explicitly reprioritized. R1's peer target is a pinned `rmw_zenoh` build, which R0's key expression, type hash, and message attachment are already shaped for.
- R3 existing unmodified ROS workloads resume only after deciding whether the demo route should become a broader compatibility route.
- Framework H1–H14 daily-driver work is deferred; its completed safety lessons still apply to physical evidence and storage boundaries.
- RV64 and distributed-authority work are deferred until the RPi5 ROS 2 demo is stable.
- Native development/on-device build work may resume after RP8, with the demo as a target workload for build and live-update validation.
