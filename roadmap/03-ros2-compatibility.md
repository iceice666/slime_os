# ROS 2 compatibility track

**Status:** Not started. The first ROS milestone is now R0: a minimal bounded Zenoh topic profile sufficient for two ROS 2 nodes on Raspberry Pi 5 to exchange bounded data.

This track makes ROS 2 a bounded userspace compatibility profile over Slime's native typed data fabric and explicit network/datagram authority. It does not make ROS, a middleware wire protocol, a ROS graph, or a topic namespace part of the kernel ABI. Existing unmodified ROS binaries remain later R3 scope unless the RPi5 demo explicitly selects a Linux-personality route, but the first demo is no longer local-only: it must exercise a real ROS 2 middleware topic path.

**The transport is generation data, not an architectural commitment.** `contracts/rpi5-ros2-demo/v2` carries the transport family as a closed `transport` discriminator plus one optional profile record per admitted family, so replacing it is a change of declared data rather than a contract migration. Zenoh is the selected family; DDSI-RTPS remains expressible for the same reason. Every userspace mechanism in Slime is meant to be replaceable, and a transport that could only be changed by rewriting the acceptance contract would not be.

## Near-term compatibility baseline

The near-term target is pinned by [RP0](09-rpi5-ros2-demo.md#rp0--demo-contract-and-acceptance-fixture), not described as generic “ROS 2 support”:

- Raspberry Pi 5 physical target;
- two local nodes in one Slime generation;
- one declared topic and bounded message type;
- ROS 2 Kilted-compatible type/name/QoS behavior for the admitted subset;
- minimal bounded Zenoh topic transport with classic CDR payloads;
- fixed session, link, key-expression, publisher, subscriber, sequence, and teardown bounds;
- exact generation-declared node identities, route authority, session authority, stream/datagram authority, type identity, parameters/configuration, clocks/timers, resource budgets, and evidence trace.

R0 uses static generation-declared peers. It does not require arbitrary LAN discovery, a Zenoh router, gossip or multicast scouting, liveliness tokens, graph introspection, multiple middleware vendors, services/actions, Python, dynamic plugins, package indexes, Gazebo, or unmodified desktop ROS packages. Those are later compatibility profiles.

## Compatibility layers

Passing one layer never implies support for the others:

1. **R0 minimal Zenoh topic profile:** enough ROS 2 node identity, topic, type, QoS, CDR serialization, and bounded Zenoh session/declaration/put behavior for two local nodes on Raspberry Pi 5.
2. **R1 broader topic wire profile:** interoperability with pinned external ROS 2 peers running `rmw_zenoh`, whose data key expression, type hash, and message attachment R0 already emits.
3. **R2 service and action profile:** ROS service/action mapping, state machines, QoS, and bounded restart behavior.
4. **R3 existing workload route:** unmodified or mostly unmodified ROS packages through an admitted native RMW, Linux-personality, or VM route.

## Authority boundary

- Native components receive route endpoint capabilities from the C8 fabric service; a ROS name string grants nothing.
- The R0 transport runtime holds only the exact C8 routes, session/publisher/subscriber roles, clocks, parameters, logging sinks, filesystem/object handles, and stream/datagram destinations declared by the generation.
- The wire protocol carries typed data, not Slime capabilities. Slime capabilities never appear in CDR payloads, key expressions, or message attachments.
- Fixed discovery metadata and graph introspection are filtered to the caller's exact visibility grants.
- A configured loopback endpoint, peer address, or TCP port is an exact stream/datagram destination grant, not ambient LAN access.
- No component holds router, gossip, multicast, scouting, or discovery authority. R0's graph is generation data, so there is nothing for such authority to do, and the v2 contract gate rejects any grant naming it.
- Transport-level security is not claimed by R0/R1/R2 unless a later milestone explicitly admits and verifies it.

## Admitted interface subset

R0 and R1 accept only normalized types whose maximum CDR serialized size is known before activation:

- booleans, integers, floating-point values, bytes, characters, and admitted time/duration structures;
- nested admitted messages;
- fixed-size arrays;
- bounded strings and bounded sequences;
- deterministic field order, alignment, encapsulation, endianness, and type identity.

They reject before staging:

- unbounded strings or sequences;
- recursive layouts without a finite declared bound;
- unsupported unions, annotations, representations, or extensibility modes;
- duplicate type names with different normalized layouts;
- any message whose declared maximum exceeds the route, heap, queue, fragment, datagram, or shared-buffer quota.

The importer produces the same C8 `InterfaceSchema` identity used by native components, plus the deterministic ROS type identity (RIHS01) and DDS-mangled type name the selected transport's key expression embeds. It does not create an unchecked second type system.

## Sequencing

1. R0 consumes C8's native fabric, the RP5 runtime envelope, [IO4's bounded network service and exact destination authority](11-io-substrate.md#io4--network-service-and-exact-destination-authority), and target-qualified AArch64/RPi5 artifacts. Its completion is part of the RPi5 demo path and is observed first under AArch64 QEMU, then under physical Raspberry Pi 5 through RP7.
2. R1 broadens R0 into external topic wire compatibility after the local RPi5 demo is stable unless explicitly reprioritized. It consumes the same IO4 service; a physical target additionally needs its own qualified `LinkDevice` backend, such as Framework H6.
3. R2 services/actions depend on R1 and consume C8 operations plus C9 lifecycle/time contracts where applicable — ROS 2 managed-node (`change_state`/`get_state`) and parameter services are expected to map onto C8 `Call<Request, Reply>` routes backed by C9's lifecycle-transition and parameter-state schemas rather than a ROS-specific reimplementation; see [`devlog/2026-08-17-ros2-transport-zenoh-pivot/`](../devlog/2026-08-17-ros2-transport-zenoh-pivot/index.md).
4. R3 existing workloads depend on R2 and on the selected local execution route. If X1 is used on Raspberry Pi 5, its loader, syscall-service mapping, and executable closure must be admitted for AArch64/RPi5 rather than inherited from x86-64.
5. None of R0–R2 depends on compositor, audio, Wi-Fi, GPU, local Python, an on-device ROS build toolchain, or Framework daily-driver support.

## R0: Minimal Raspberry Pi 5 Zenoh ROS 2 topic profile

**Status:** Not started.

**Depends on:** RP5 from [RPi5 ROS 2 demo](09-rpi5-ros2-demo.md#rp5--node-and-transport-runtime-envelope), C8 stream/fabric mechanisms used behind the gateway/runtime, [IO4](11-io-substrate.md#io4--network-service-and-exact-destination-authority) for the selected bounded stream/network transport and exact destination grant, and target-qualified AArch64/RPi5 artifacts.

A bounded self-built Zenoh subset was selected over the self-built DDSI-RTPS/XCDR profile this track previously specified, and over hosting the official Zenoh runtime or `rmw_zenoh` under the X1 Linux personality; see [`devlog/2026-08-17-ros2-transport-zenoh-pivot/`](../devlog/2026-08-17-ros2-transport-zenoh-pivot/index.md) for the comparison, the upstream evidence, and the costs accepted. The transport is carried as generation data by `contracts/rpi5-ros2-demo/v2`, so a future reversal is a fixture change rather than a new contract format.

### Deliverables

- define a versioned ROS 2 Profile 0 containing ROS distribution baseline, RMW boundary, transport family, wire protocol version, session mode, link protocol, admitted session and declaration messages, key-expression format, admitted type, topic name, direction, QoS subset, serialized-size bounds, attachment layout, discovery mode, resource ceilings, and trace records;
- implement a deterministic bounded ROSIDL/IDL importer into C8 `InterfaceSchema`, plus generated or validated Rust bindings, the RIHS01 type hash, the DDS-mangled on-wire type name, and golden CDR fixtures for the admitted message type;
- implement the minimal classic-CDR serializer/deserializer needed by the demo, including explicit encapsulation, endianness, alignment, string termination, sequence, nesting, and maximum-size checks before allocation;
- implement the minimal Zenoh subset needed for one static bounded peer link: the `INIT`/`OPEN` handshake, `FRAME` batching against the 2-byte little-endian stream length, `DECLARE_SUBSCRIBER`/`UNDECLARE_SUBSCRIBER`, `PUSH` with a `Put` body, `CLOSE`, LEB128 primitive decoding that bound-checks before allocating, and teardown;
- emit the per-message attachment `rmw_zenoh` expects — sequence number, source timestamp, and source GID through Zenoh's own value serialization, whose fixed-size arrays still carry a length prefix — so R1 is reachable without changing the payload path;
- implement or port the minimal ROS 2 node API needed for the publisher/subscriber demo: initialization, node identity, publisher/subscriber creation, executor/spin or wait-set behavior, publish, receive callback, logging, and shutdown;
- map publishers/subscribers to C8 `Stream<T>` endpoints without exposing graph mutation, ambient discovery, raw network sockets, filesystem/package lookup, or undeclared parameters;
- package the publisher and subscriber as target-qualified components with deterministic startup data and explicit route, session, stream/network, clock, log, and trace grants;
- provide host/AArch64 QEMU fixtures for CDR bytes, key expressions, attachment bytes, session and declaration framing, message identity, field values, ordering, QoS compatibility, denial, malformed-payload, and malformed-framing cases;
- record unsupported ROS and transport features as stable structured denials or build-time rejections rather than silent omissions.

### Required checks

- the publisher creates only its declared session/publisher/topic/type and cannot subscribe, inspect hidden graph state, open an undeclared endpoint, or delegate route authority unless explicitly granted;
- the subscriber creates only its declared session/subscriber/topic/type and cannot publish or observe unrelated routes;
- alternate domain, key expression, endpoint, port, topic name, type identity, type hash, QoS profile, node identity, graph operation, or direction fails without leaking protected metadata;
- bounded messages serialize to the pinned CDR bytes and traverse the Zenoh/C8 route with deterministic identity and values;
- malformed batch lengths, message headers, LEB128 lengths, key expressions, attachments, declaration bodies, and CDR alignment fail before allocation, mapping, or out-of-bounds access;
- a wildcard key expression, a router or gossip endpoint, and a multicast scouting attempt are all rejected rather than tolerated;
- node restart issues fresh sessions, endpoints, timers, heap state, and parameters and cannot replay stale samples;
- the same profile passes under AArch64 QEMU before being promoted to the physical RP7 demo.

### Planned verification target

```sh
just rpi5_ros2_zenoh_check
```

### Exit condition

Two target-qualified local ROS 2 node components exchange the declared bounded topic through a minimal bounded Zenoh profile with classic CDR payloads under AArch64 QEMU, with exact generation-declared graph and stream authority, no router or discovery authority anywhere in the graph, explicit denial for unsupported ROS and transport features, and finite resource bounds.

## R1: Broader ROS 2 topic wire profile

**Status:** Deferred until the RPi5 minimal two-node demo is stable unless explicitly reprioritized.

R1 expands R0 into external topic interoperability with pinned ROS 2 peers running `rmw_zenoh`, which REP-2000 lists Tier 1 for Kilted Kaiju and Rolling. It is not allowed to weaken R0's bounds or authority model.

R0 already emits the three things such a peer matches on — the `<domainId>/<topic>/<typeNameOnWire>/<typeHash>` key expression, the RIHS01 type hash, and the per-message attachment — so R1 adds discovery and peer behavior rather than a second wire format.

### Deliverables

- extend the versioned ROS 2 wire profile to cover the pinned host peer image, fixed probes, packet/result capture, and one command that selects the peer's RMW without changing the fixture image;
- implement the liveliness-token declarations `rmw_zenoh` builds its node graph from, under the `@ros2_lv` admin-space grammar, including the QoS substring encoding and the name mangling that keeps a `/` out of a key-expression field;
- decide and record how the peer reaches the Slime node without granting ambient discovery: either an explicitly granted router endpoint, or direct static peer endpoints on both sides. Upstream documents router gossip as the supported discovery path and ships multicast scouting disabled, and whether two directly connected peers populate the graph cache without gossip is unverified — R1 must establish this by observation before claiming interoperability;
- broaden duplicate/reorder handling, peer restart, retry exhaustion, and teardown within declared resource limits;
- keep native C8 Stream endpoints mapped to publishers/subscribers without exposing raw sockets or arbitrary graph creation;
- preserve the same ROSIDL/IDL importer, RIHS01 derivation, and CDR layout rules as R0.

### Required checks

- a pinned `rmw_zenoh` publisher sends an admitted topic to a native Slime subscriber and a native publisher sends the same type to a pinned `rmw_zenoh` subscriber;
- reliable and best-effort routes pass independently with declared finite resource behavior;
- name mapping, type identity, RIHS01 type hash, CDR bytes, attachment fields, and requested/offered matching agree with pinned peer fixtures;
- alternate domain, key expression, peer address, port, topic, type, direction, and QoS attempts fail closed;
- a denied local route emits no corresponding Zenoh `Put`, and an undeclared remote publisher cannot inject a native sample;
- malformed batch lengths, headers, key expressions, attachments, declaration bodies, and CDR alignment fail before out-of-bounds access or unbounded allocation;
- any router or discovery authority R1 admits is an exact generation-declared endpoint grant, and its absence is a named denial rather than a fallback to scouting.

### Planned verification target

```sh
just ros2_topic_check
```

### Exit condition

The content-addressed pinned ROS 2 peer container exchanges admitted bounded topics bidirectionally with native Slime components over the admitted Zenoh profile through exact graph and network grants; reliable and best-effort behavior matches the profile, denied routes emit no data message, and malformed or exhausted transport state cannot escape declared resource bounds.

## R2: ROS 2 services and actions profile

**Status:** Deferred.

### Deliverables

- implement standard ROS service request/reply topic mapping, request identity, client routing, response correlation, timeout, cancellation, duplicate handling, and bounded server concurrency;
- map a ROS service onto a C8 `Call<Request, Reply>` route while preserving native peer-death, timeout, cancellation, and authorization errors;
- implement ROS actions as the standard three services and two topics: send goal, cancel goal, get result, feedback, and status;
- implement the accepted/executing/canceling/succeeded/aborted/canceled goal state machine with UUID validation, explicit transition checks, bounded active-goal count, and bounded result retention;
- extend the pinned `rmw_zenoh` conformance corpus with service, action, restart, duplicate, timeout, and cancellation fixtures.

### Required checks

- native and pinned upstream peers call an admitted service in both directions and preserve request/response identity under concurrent clients;
- duplicate or stale requests never execute a declared non-idempotent operation twice;
- action accept, reject, feedback, status, success, abort, cancel, and result retrieval agree with the pinned upstream peer;
- an unauthorized client cannot send a goal, cancel another goal, retrieve a result, observe feedback/status, mutate parameters, or change lifecycle state;
- malformed UUIDs, illegal goal transitions, cancellation races, duplicate results, expired results, and transient-local replay fail deterministically without leaking active-goal state.

### Planned verification target

```sh
just ros2_service_action_check
```

### Exit condition

The content-addressed pinned ROS 2 peer container and native Slime components call services and execute, observe, cancel, and retrieve actions bidirectionally under admitted RMW selections through declared graph and network authority, with exact correlation and finite resource bounds.

## Embedded companion boundary

MCU-class ROS devices without the admitted 64-bit MMU isolation baseline are external peers, not local R3 workloads and not reduced-security Slime ports. A future companion profile may admit micro-ROS/XRCE-DDS or a smaller Zutai protocol only through an exact serial, CAN, USB, or `NetworkDestination` capability.

The generation must bound peer identity, admitted types, route direction, payload size, frequency, queue depth, timeout, reconnect/reset behavior, and actuator authority. Malformed traffic, resource exhaustion, disconnect, and MCU reboot become structured C8/C9 events. The companion receives no ambient discovery domain, graph creation, raw network, storage, or device authority.

## R3: Existing ROS workload route

**Status:** Deferred unless RP0 selects a Linux-personality or source-compatible existing-package route for the two-node demo.

R3 runs existing ROS client-library workloads locally. It is not required for R0 if the first demo uses minimal Slime-native ROS 2 nodes over the R0 transport profile.

### Supported routes

- a native RMW/client-library surface that speaks the R0/R1 transport profile through Slime services;
- X1 personality: run a Linux userspace ROS process with filesystem, network, clock, randomness, and process behavior translated to explicit Slime services; or
- a separately admitted VM or target-specific backend, never inherited from an x86-only AMD-V claim.

The selected route is generation data. Moving a workload between routes cannot widen its grants. The selected route is also architecture-qualified: a package proven on x86-64 is not admitted on AArch64/RPi5 until its executable closure and route pass the corresponding target checks.

### Deliverables

- pin exact upstream distribution packages, build artifacts, route, supported client-library surface, and rejected operating-system assumptions;
- package every executable, shared object, interface, configuration, and resource as content-addressed generation objects or explicitly granted state;
- map ROS domain, remapping rules, parameters, logs, clocks, files, network peers, devices, process creation, and scheduling class to explicit generation data and capabilities;
- deny unsupported syscalls, package discovery, plugin loading, dynamic types, transports, or middleware options with stable structured/errno behavior;
- run pinned demo topic/service/parameter workloads through the selected route and expose the workload's complete possible authority to manifest graph and authority-diff tooling.

### Required checks

- pinned packages run without adding native global paths, package indexes, environment inheritance, raw sockets, or unrestricted middleware discovery;
- child processes, composed nodes, plugins, and dynamically created ROS entities cannot exceed the parent workload's declared grants or quotas;
- missing filesystem, network, clock, randomness, device, or scheduling authority fails through the selected compatibility boundary rather than being fabricated;
- workload restart, generation rollback, and route change retain only declared persistent state and cannot retain stale endpoints, secrets, buffers, or network sessions.

### Planned verification target

```sh
just ros2_workload_check
```

### Exit condition

Pinned ROS workloads run locally through one declared compatibility route, interoperate with native C8 components, and remain confined to generation-declared filesystem, network, clock, randomness, scheduling, graph, and device authority with stable rejection for everything else.

## Conformance references

- [ROS 2 middleware/RMW boundary](https://docs.ros.org/en/kilted/Concepts/Intermediate/About-Different-Middleware-Vendors.html)
- [ROS 2 QoS policies and compatibility](https://docs.ros.org/en/kilted/Concepts/Intermediate/About-Quality-of-Service-Settings.html)
- [ROS topic and service name mapping](https://design.ros2.org/articles/topic_and_service_names.html)
- [ROS 2 action protocol](https://design.ros2.org/articles/actions.html)
- [REP-2000 distribution and middleware support tiers](https://www.ros.org/reps/rep-2000.html)
- [Zenoh protocol specification 1.0.0](https://spec.zenoh.io/spec/1.0.0/)
- [`ros2/rmw_zenoh`](https://github.com/ros2/rmw_zenoh)

These references define targets to test against; they do not waive Slime's deterministic bounds or authority invariants.
