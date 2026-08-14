# Raspberry Pi 5 ROS 2 two-node demo track

**Purpose:** Drive the near-term project toward one observed robotics workload: Slime OS running on a Raspberry Pi 5 with two local ROS 2 nodes exchanging bounded topic data through a minimal DDSI-RTPS/XCDR profile.

**Status:** In progress — RP0 and RP1 complete.

**Acceptance target:** A reproducible Raspberry Pi 5 boot runs a declared publisher node and subscriber node. The publisher emits a bounded ROS 2 topic stream through the admitted minimal DDS/RTPS profile, the subscriber observes the expected samples in order under the declared QoS/profile, and the run records image identity, board/firmware/media identity, generation/release identity, serial transcript, DDS/semantic trace, and every device/storage/datagram/network capability involved.

This track intentionally does **not** claim full ROS 2 compatibility, arbitrary DDS discovery, unmodified desktop ROS packages, Python support, Gazebo, Wi-Fi, GPU acceleration, or Framework daily-driver support. It is the shortest defensible path to a DDS-backed two-node RPi5 demo while preserving Slime's capability, component, generation, and schema invariants.

## Boundaries

- Raspberry Pi 5 is a named physical target, not a synonym for generic Arm support.
- The first ROS 2 route is **minimal DDSI-RTPS/XCDR first**, not a local-only `rmw_slime` shortcut. Native C8 fabric may back internal delivery, but the demo profile must define DDS participants, writers/readers, topic mapping, XCDR payloads, sequence behavior, and bounded discovery/teardown.
- R0 may use constrained static discovery, fixed participants, fixed locators, and one admitted message type. Arbitrary LAN discovery, DDS Security, multiple middleware vendors, services/actions, and unmodified existing ROS packages are later milestones.
- ROS node names, topic names, types, parameters, executors, DDS QoS policy, and graph metadata stay in userspace. The kernel remains unaware of ROS and DDS concepts.
- DDS/RTPS datagram or network authority is explicit generation data. A loopback endpoint, UDP port, multicast group, or peer locator grants no authority unless the generation names it.
- QEMU AArch64 evidence is required to de-risk architecture work, but a physical Raspberry Pi 5 run is the only exit evidence for board support.
- Storage starts from reproducible removable media with no claim of internal-device writes. Any persistent state used by the demo is generation-declared and rollbackable.

## Sequencing

1. RP0 freezes the demo acceptance contract so every later milestone has a stable target, including the minimal DDS/RTPS profile.
2. RP1 consumes P0/P1 target and architecture-boundary work so every executable is admitted for one exact profile.
3. RP2 proves the architecture-neutral kernel and component semantics under `aarch64-qemu-virt`.
4. RP3 promotes the minimum physical Raspberry Pi 5 boot path: firmware handoff, serial, memory map, timer, interrupt controller, and device tree.
5. RP4 replays the two-component C7/C8 data path on Arm and then on the board.
6. RP5 adds the minimum node runtime plus datagram/network envelope needed to host the DDS-backed ROS 2 node route.
7. RP6 implements the minimal DDS/RTPS topic profile and packages the two ROS 2 nodes on the Slime component model.
8. RP7 records the first complete physical DDS-backed data-transfer demo.
9. RP8 makes the demo repeatable and bounded enough to serve as the new roadmap baseline.

## RP0 — Demo contract and acceptance fixture

**Status:** Complete.

**Depends on:** Cleared backlog, completed M6/C7 baseline, and the C8.10 full-graph boot baseline as current regression evidence.

### Deliverables

- pin one exact Raspberry Pi 5 hardware revision or accepted revision set, boot firmware path, removable media path, serial/UART path, memory-map source, interrupt controller, generic timer, and minimum device list;
- choose the ROS 2 distribution baseline, initially Jazzy unless a later decision records a different pinned target;
- define ROS 2 Profile 0: DDSI-RTPS version, XCDR representation, domain id, participant ids, locators, writer/reader ids, discovery mode, required RTPS submessages, QoS subset, sequence/retry behavior, teardown behavior, and resource ceilings;
- define the two-node workload: node names, topic name, message type, publish count/rate or event sequence, QoS/profile, expected subscriber output, and all allowed failure markers;
- define a versioned bounded DDS/semantic trace for demo evidence under `contracts/`, or explicitly reuse an existing C8 trace contract if it already covers the needed records;
- write the exact operator-visible success condition so a serial transcript can distinguish success, denial, timeout, wrong-board, wrong-target, malformed RTPS/CDR, and malformed-generation failures.

### Required checks

- the demo fixture rejects unknown board/firmware/profile identifiers instead of silently selecting a nearby target;
- the ROS 2 route states the exact DDS/RMW boundary and whether nodes are unmodified upstream packages, source-compatible ports, or Slime-native ROS-compatible components;
- every message, string, sequence, RTPS record, trace, fragment, queue, and retry bound is known before activation;
- the fixture names every capability the two nodes, DDS runtime, board services, storage path, datagram/network path, and trace sink may hold.

### Planned verification target

```sh
just rpi5_ros2_demo_contract_check
```

### Exit condition

The repository contains a bounded, versioned, target-qualified demo contract that says exactly what “two ROS 2 nodes exchange DDS-backed topic data on Raspberry Pi 5” means and how success or failure is observed.

## RP1 — Target-qualified build and admission path

**Status:** Complete.

**Depends on:** P0 and P1 from [Architecture portability](07-architecture-portability.md).

### Deliverables

- make the generation `target`, release metadata, kernel image, component image, DDS runtime, and node executable closure identify the exact `aarch64-rpi5` profile;
- reject wrong-architecture kernels, components, ROS node artifacts, DDS runtime artifacts, page profiles, required ISA features, and ABI variants before mapping executable bytes;
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

## RP2 — AArch64 QEMU kernel and component vertical slice

**Status:** Not started.

**Depends on:** RP1 and P2.

### Deliverables

- boot the verified generation path on `aarch64-qemu-virt` with EL1 kernel and EL0 components;
- implement or connect AArch64 exception vectors, `svc` syscalls, context switching, address-space switching, 4 KiB translation tables, TLB maintenance, interrupt masking, idle/wake behavior, GICv3, generic timer, PL011 serial, and QEMU exit;
- launch at least two target-qualified components and exercise IPC, faults, wait/wake, timer preemption, supervision, and rollback semantics;
- replay the C7 sample-plane exchange and the C8 route provisioning/data path required by RP4/RP6;
- produce normalized semantic events comparable to x86 without requiring byte-identical register or physical-address traces.

### Required checks

- the AArch64 QEMU profile launches isolated EL0 components from a verified generation;
- invalid instruction, data abort, permission fault, malformed user pointer, and component crash report or terminate the responsible component without corrupting another component or the kernel;
- endpoint wake, timer wake, supervision wake, and idle exit do not busy-poll or lose wakeups;
- two components exchange and return a payload larger than the control-message bound with the same quota and reclamation semantics as x86;
- a failing pending AArch64 generation rolls back to a verified AArch64 known-good generation, while x86 artifacts are rejected as wrong-target.

### Planned verification target

```sh
just aarch64_qemu_check
```

### Exit condition

The AArch64 QEMU profile runs the architecture-neutral Slime component model and data path needed by the demo with the same authority, lifecycle, and rollback semantics as the existing x86 baseline.

## RP3 — Raspberry Pi 5 serial boot and minimum board services

**Status:** Not started.

**Depends on:** RP2 and P4's Raspberry Pi 5 qualification slice.

### Deliverables

- select and document the Raspberry Pi 5 firmware/boot handoff path, kernel load address rules, device-tree source, serial console, and removable media image format;
- parse the board device tree with strict bounds and identify memory regions, reserved regions, UART, GIC, generic timer, mailbox/power/reset interfaces if used, and any storage or network/datagram path used by the demo;
- bring up early serial diagnostics, exception reporting, MMU mappings, timer interrupts, idle/wake, and shutdown/reboot or operator-visible stop behavior on physical hardware;
- preserve the no-ambient-storage boundary: the first board demos boot from reproducible removable media and do not claim unqualified writes to other devices;
- record board revision, firmware version, image identity, generation identity, serial output, and device-tree identity.

### Required checks

- wrong board revision, missing required device-tree nodes, unsupported page/interrupt/timer profile, or incompatible firmware handoff fails with bounded diagnostics;
- timer interrupts and serial logging continue after entering the normal scheduler path;
- a faulting early component or malformed user pointer is reported on serial without wedging the board;
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

**Depends on:** RP2, RP3, C7, and the C8 stream/fabric slices consumed by the DDS runtime.

### Deliverables

- run two isolated AArch64 components that exchange bounded typed data through the same C7/C8 path the DDS/ROS nodes will use behind their participant/writer/reader roles;
- exercise inline samples and, if the demo message can exceed the IPC control bound, a shared-buffer-backed sample descriptor;
- prove endpoint, buffer, mapping, loan, event, and queue accounting on Arm rather than inheriting x86-only evidence;
- record the same scenario under `aarch64-qemu-virt` and on Raspberry Pi 5, with physical traces labeled separately;
- keep this probe below the DDS layer so architecture/data-path failures remain separable from DDS/RMW failures.

### Required checks

- the publisher component cannot receive or re-delegate subscriber authority, and the subscriber cannot publish unless explicitly granted;
- malformed descriptors, wrong type tags, quota exhaustion, peer death, and route denial fail closed and reclaim resources;
- the Raspberry Pi 5 run observes the same semantic data-transfer records as the AArch64 QEMU run, excluding architecture-specific register and address detail;
- the board remains responsive after the exchange and emits an operator-visible completion marker.

### Planned verification target

```sh
just rpi5_data_path_check
```

### Exit condition

Before DDS/ROS is introduced, two isolated components exchange the demo-shaped bounded data path on AArch64 QEMU and on Raspberry Pi 5 with explicit route authority and resource reclamation.

## RP5 — Node and DDS transport runtime envelope

**Status:** Not started.

**Depends on:** RP4 and the subset of C10/private-memory, clock/timer, and datagram/network service required by R0.

### Deliverables

- provide the minimum allocator, startup, argument/configuration, clock/timer, logging, executor/wait-set, and lifecycle hooks required by the two ROS 2 nodes and the minimal DDS runtime;
- provide the exact bounded datagram path needed by DDSI-RTPS, such as a loopback/local UDP profile or target-specific network service, without granting raw packets or wildcard destinations;
- package node and DDS participant configuration as deterministic generation data rather than environment variables, global paths, package indexes, or host filesystem state;
- map every needed clock, parameter, filesystem/object, graph, participant, locator, and logging operation to an explicit capability or a stable structured denial;
- bound heap pages, executor queues, timers, log bytes, parameter bytes, RTPS receive queues, writer/reader history, fragments, retries, and outstanding messages before activation;
- keep unsupported POSIX, dynamic loading, package discovery, arbitrary multicast discovery, plugin behavior, or vendor-specific ambient DDS configuration visibly unsupported rather than accidentally ambient.

### Required checks

- a node can allocate, initialize, spin, publish/subscribe through the DDS runtime, log, and exit within its declared resource budget;
- missing clock, route, participant, locator, datagram/network, parameter, file, or logging authority fails with a named error rather than fallback state;
- an allocation, RTPS queue, fragment table, or retry beyond the declared quota leaves the node/runtime alive enough to report the failure when the selected API permits it;
- two node instances receive distinct endpoint, participant, writer/reader, and resource authority on restart and cannot reuse stale handles.

### Planned verification target

```sh
just rpi5_node_runtime_check
```

### Exit condition

The minimal ROS 2/DDS node route has a bounded Slime runtime envelope sufficient for two local nodes, with no hidden dependency on ambient filesystem, environment, network, package-discovery, clock, or DDS discovery authority.

## RP6 — Minimal DDS/RTPS topic profile and two ROS 2 nodes

**Status:** Not started.

**Depends on:** RP5 and R0 from [ROS 2 compatibility](03-ros2-compatibility.md).

### Deliverables

- define the local ROS 2 DDS profile for nodes, names, topic mapping, admitted message type, QoS subset, type identity, XCDR1 representation, participant/writer/reader identities, locators, and bounded discovery;
- package the publisher node and subscriber node as target-qualified generation components with explicit startup configuration and grants;
- map their DDS writer/reader roles onto the C8 fabric and bounded datagram service without giving either node ambient discovery, graph mutation, filesystem, wildcard network, or broader route authority;
- expose enough ROS 2-visible node behavior to justify calling them ROS 2 nodes under the DDS/RMW boundary, and document every unsupported ROS/DDS feature outside the demo profile;
- add host or QEMU fixtures that compare ROS name mapping, DDS topic mapping, XCDR bytes, RTPS records, message field values, ordering, and QoS behavior against the pinned RP0 contract.

### Required checks

- the publisher sends only the declared topic/type through its declared writer and locator; the subscriber receives only through its declared reader and locator;
- alternate domain, participant id, locator, topic name, type, node name, QoS, direction, or undeclared graph operation fails without exposing protected graph metadata;
- message serialization and RTPS framing are deterministic and bounded;
- repeated node/runtime restart does not retain stale DDS identities, endpoints, loans, buffers, timers, fragments, retries, or parameters;
- the same node generation boots and passes the R0 DDS profile under AArch64 QEMU before being promoted to Raspberry Pi 5.

### Planned verification target

```sh
just rpi5_ros2_dds_nodes_check
```

### Exit condition

Two target-qualified local ROS 2 node components exchange the declared bounded topic through a minimal DDSI-RTPS/XCDR profile under AArch64 QEMU, with every unsupported ROS/DDS feature denied explicitly.

## RP7 — Observed Raspberry Pi 5 ROS 2 data-transfer demo

**Status:** Not started.

**Depends on:** RP6 and RP3/RP4 physical board evidence.

### Deliverables

- build one reproducible Raspberry Pi 5 demo image containing the verified kernel, runtime services, DDS runtime, publisher node, subscriber node, generation graph, datagram/network grants, and demo trace sink;
- boot the image on the named board and run the DDS-backed two-node exchange without manual patching after boot;
- record serial output, semantic trace, RTPS/DDS trace records, generation/release identity, node/component identities, board/firmware/media identity, and storage/no-write evidence;
- normalize the evidence so success can be compared across repeated runs without depending on raw physical addresses, task ids, or timing jitter;
- include negative markers for wrong target, wrong route, wrong participant/locator, timeout, malformed RTPS/CDR, denied grant, and node failure so absence of success is not silent.

### Required checks

- the publisher emits the declared number or sequence of DDS topic samples and the subscriber observes the expected values/order within declared bounds;
- the run reaches a single operator-visible success marker only after the subscriber has validated the data and the trace includes the expected DDS writer/reader events;
- no component holds a capability outside the RP0 manifest contract;
- the board remains in a known state after success or failure and does not write ungranted storage;
- the captured evidence is sufficient for a reviewer to distinguish actual DDS-backed node data transfer from a boot-only, print-only, or C8-only demo.

### Planned verification target

```sh
just rpi5_ros2_demo_check
```

### Exit condition

A Raspberry Pi 5 physically runs Slime OS with two local ROS 2 nodes exchanging the declared data through the minimal DDSI-RTPS/XCDR profile, and the repository contains reproducible evidence of the board, image, generation, capabilities, DDS/semantic trace, and serial success marker.

## RP8 — Repeatability, bounds, and demo baseline hardening

**Status:** Not started.

**Depends on:** RP7.

### Deliverables

- repeat the physical demo across cold boots and warm restarts, recording normalized DDS/semantic traces and resource high-water marks;
- inject or simulate denied route, denied locator, malformed RTPS, malformed XCDR message, subscriber restart, publisher restart, queue exhaustion, retry exhaustion, timer delay, and peer-loss cases relevant to the demo profile;
- verify every endpoint, buffer, mapping, loan, timer, heap page, queue entry, fragment, retry record, writer/reader history entry, and trace record returns to its declared baseline after normal completion or restart;
- make the demo gate reproducible enough that later roadmap work can use it as a regression target;
- write a devlog entry with the evidence chain before declaring the near-term release closed.

### Required checks

- repeated runs produce the same normalized success trace and bounded resource highs;
- each injected failure has a distinct structured failure marker and does not wedge the board or corrupt unrelated state;
- supervised restart gives each node and DDS participant fresh authority and replays no stale samples;
- the demo remains insensitive to host environment, file paths, unrelated network state, and unrelated devices;
- every physical run records enough provenance to reproduce the image and generation.

### Planned verification target

```sh
just rpi5_ros2_demo_stress_check
```

### Exit condition

The Raspberry Pi 5 two-node ROS 2 DDS demo is repeatable, bounded, and reviewable: normal runs match their normalized trace, failure cases are distinguishable, resources are reclaimed, and the physical evidence is strong enough to become the new baseline for subsequent roadmap work.

## Relationship to later tracks

- R1/R2 broaden the minimal R0 DDS path into external peer, multi-vendor, service, and action compatibility after RP8 unless explicitly reprioritized.
- R3 existing unmodified ROS workloads resume only after deciding whether the demo route should become a broader compatibility route.
- Framework H1–H14 daily-driver work is deferred; its completed safety lessons still apply to physical evidence and storage boundaries.
- RV64 and distributed-authority work are deferred until the RPi5 ROS 2 demo is stable.
- Native development/on-device build work may resume after RP8, with the demo as a target workload for build and live-update validation.
