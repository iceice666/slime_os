# Raspberry Pi 5 ROS 2 two-node demo track

> **H2 routing — completed slices archived, unextracted detail retained.** The
> [RPi5 demo plan](../docs/plans/rpi5-ros2-demo.md) owns the goal, exclusions,
> broad sequence and physical-evidence rule; the
> [format-2 demo contract](../contracts/rpi5-ros2-demo/v2/README.md) owns the exact
> active acceptance profile. Current target/build boundaries are in
> [targets](../docs/architecture/targets-and-portability.md) and
> [component/system/image architecture](../docs/architecture/component-system-image.md).
> RP3–RP8 runtime-envelope, trace, fault, repeatability and physical acceptance
> detail remains authoritative here. Completed RP0–RP2 delivery history and the
> original transport-pivot/status narrative live in the immutable source linked
> below. See the [file classification](README.md).

## RP3 — Raspberry Pi 5 serial boot and minimum board services

**Depends on:** RP2 and P4's Raspberry Pi 5 qualification slice.

### Deliverables

- retain the existing `bcm2712` seL4 kernel/loader build route selecting
  `sel4/config/bcm2712-rpi5.cmake`, with pinned artifact digests as for
  `qemu-arm-virt`; qualify the resulting image on the named physical board;
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

**Depends on:** RP2, RP3, C7, the C8 stream/fabric slices consumed by the transport runtime, and [CP5](../contracts/component-sdk-release/v1/) from the Component platform track.

### Deliverables

- run two isolated AArch64 components that exchange bounded typed data through the same C7/C8 path the ROS nodes will use behind their publisher/subscriber roles;
- author and build both components entirely outside this repository through CP5's out-of-tree component SDK path, not as in-tree component crates; this requirement is scoped to RP4's two components only and does not extend to RP6's ROS 2 node components;
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

**Depends on:** RP4, C10's private memory, [IO0](11-io-substrate.md#io0--queue-identity-and-buffer-lease-contract), and [IO4](11-io-substrate.md#io4--network-service-and-exact-destination-authority). The clock/timer and executor halves are [C9.1](../docs/architecture/runtime-authority.md#clock-and-timers) and [C9.2](../docs/architecture/runtime-authority.md#wait-sets).

### Deliverables

- provide the minimum allocator, startup, argument/configuration, clock/timer, logging, executor/wait-set, and lifecycle hooks required by the two ROS 2 nodes and the minimal transport runtime;
- consume the exact IO4 bounded byte-stream path the transport needs — for Zenoh Profile 0, one TCP connection carrying length-prefixed batches — without granting raw packets, arbitrary DNS, listen authority, or wildcard destinations;
- package node and session configuration as deterministic generation data rather than environment variables, global paths, package indexes, or host filesystem state;
- map every needed clock, parameter, filesystem/object, graph, session, endpoint, and logging operation to an explicit capability or a stable structured denial;
- bound heap pages, executor queues, timers, log bytes, parameter bytes, transport receive queues, publisher/subscriber history, batches, retries, and outstanding messages before activation;
- keep unsupported POSIX, dynamic loading, package discovery, router or scouting-based discovery, plugin behavior, or vendor-specific ambient middleware configuration visibly unsupported rather than accidentally ambient.

There is still no async runtime anywhere in `components/`, and that remains deliberate — a transport implementation assuming `async fn` and a reactor is out of scope for RP5, while one assuming a heap is a declared quota and one assuming multi-source blocking is now a declared source table. Profile 0's blocking, fixed-buffer shape is chosen to need neither.

### Required checks

- a node can allocate, initialize, spin, publish/subscribe through the transport runtime, log, and exit within its declared resource budget;
- missing clock, route, session, endpoint, stream/network, parameter, file, or logging authority fails with a named error rather than fallback state;
- an allocation, receive queue, batch buffer, or retry beyond the declared quota leaves the node/runtime alive enough to report the failure when the selected API permits it;
- two node instances receive distinct endpoint, session, publisher/subscriber, IO request/connection epochs, and resource authority on restart and cannot reuse stale handles or completions.

### Planned verification target

```sh
just rpi5_node_runtime_check
```

### Exit condition

The minimal ROS 2 node route has a bounded Slime runtime envelope sufficient for two local nodes, with no hidden dependency on ambient filesystem, environment, network, package-discovery, clock, or middleware discovery authority.

## RP6 — Minimal transport topic profile and two ROS 2 nodes

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

**Depends on:** RP7.

### Deliverables

- repeat the physical demo across cold boots and warm restarts, recording normalized semantic/wire traces and resource high-water marks;
- inject or simulate denied route, denied endpoint, malformed batch length, malformed declaration, malformed CDR payload, malformed attachment, subscriber restart, publisher restart, queue exhaustion, retry exhaustion, timer delay, and peer-loss cases relevant to the demo profile;
- verify every endpoint, buffer, mapping, loan, timer, heap page, queue entry, fragment, retry record, writer/reader history entry, and trace record returns to its declared baseline after normal completion or restart;
- make the demo gate reproducible enough that later roadmap work can use it as a regression target;
- record the repeated observations, exact image and generation identities, evidence class, scope, and limits in the PR and canonical work item before declaring the near-term release closed.

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
