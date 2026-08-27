use super::*;

/// Drive the P5.4.9 full-graph boot: every C8 role in one generation.
///
/// Only reachable for the authenticated `boot` action declared by
/// `contracts/generation-manifest/v1/compositions/sel4-boot.zti`. Every participant reads
/// the same generation-derived action from its generated profile.
///
/// **Every control endpoint is placed by the generation, not by init.** Each is
/// a declared native seL4 Endpoint whose two halves the root installs into the
/// instances that declared them, before either runs. Init holds no route
/// capability and mints nothing, so `SEL4_BOOT_LAYOUT` numbers only the
/// twenty-one things the generation places. That is also what binds a control
/// endpoint to one component identity: a worker answers "which component is
/// asking" from the endpoint a request arrived on, and no party can forge,
/// share, or re-derive one.
///
/// **Init spawns all nineteen, including both route workers.** A supervision
/// handle cannot exist before its subject task, and a minted binding is only
/// satisfiable by the instance that owns it — `preflight_spawn_grants` refuses
/// `declared.owner != caller_instance`. The workers need handles naming call and
/// operation participants, and only init ever holds those, so only init can
/// spawn the workers. Having `fabric-service` spawn them (as the retired
/// custom-kernel graph did) left both workers with no control endpoints and no
/// handles, because neither an endpoint grant nor an endpoint-kind minted
/// binding can cross a spawn boundary (B55).
///
/// **Spawn order is load-bearing.** Every task whose supervision handle appears
/// in a later grant vector must already exist, so the six stream-ring and proxy
/// identities precede `fabric-service`, and the seven call and operation
/// identities precede their workers.
///
/// Init does not exit. The gate's exit condition is the whole graph at healthy
/// blocked idle, so init parks on the fabric's handle — a component terminating
/// here is a failure, not something to wait for.
pub(super) fn drive_boot_plane() -> ! {
    // Every control endpoint below is a generation-declared native seL4
    // Endpoint the root installed into both declaring instances before this
    // task, or any child it spawns, ran at all. Init places nothing.
    slime_rt::debug_write(b"[init] fabric boot control channels minted\n");
    // The stream broker's loan receivers. Spawn order is the order the
    // fixture's minted bindings declare, ascending by slot, because the root
    // matches a spawn's grant vector positionally against those declarations.
    let publisher = spawn_boot(b"executable:fabric-publisher");
    let subscriber = spawn_boot(b"executable:fabric-subscriber");
    let publisher_b = spawn_boot(b"executable:fabric-publisher-b");
    let subscriber_b = spawn_boot(b"executable:fabric-subscriber-b");
    let observer = spawn_boot(b"executable:fabric-observer");
    let proxy = spawn_boot(b"executable:fabric-proxy");
    // Holds a real control endpoint and is granted no edge; its denial is the
    // plane's authority evidence, so it needs no handle from anyone.
    spawn_boot(b"executable:fabric-probe");
    slime_rt::debug_write(b"[init] fabric boot stream participants spawned\n");

    // Matched positionally against the child's declarations in ascending
    // destination-slot order: the factory at 1, then the six handles at 9..14.
    let fabric = spawn_boot_with(
        b"executable:fabric-service",
        &[
            grant(resolve_own_buffer_factory(), RIGHT_BUFFER_CREATE),
            grant(publisher, RIGHT_SUPERVISE),
            grant(subscriber, RIGHT_SUPERVISE),
            grant(publisher_b, RIGHT_SUPERVISE),
            grant(subscriber_b, RIGHT_SUPERVISE),
            grant(observer, RIGHT_SUPERVISE),
            grant(proxy, RIGHT_SUPERVISE),
        ],
    );
    slime_rt::debug_write(b"[init] fabric boot stream broker spawned\n");

    let call_client = spawn_boot(b"executable:fabric-call-client");
    let call_client_b = spawn_boot(b"executable:fabric-call-client-b");
    let call_server = spawn_boot(b"executable:fabric-call-server");
    // The clock precedes the worker for the same reason the participants do: the
    // worker is granted a supervision handle naming it, and a handle cannot
    // exist before its task (B76). Its exit is not observable any other way --
    // a native Endpoint reports no peer death, and the control endpoint's own
    // state reports nothing at all.
    let call_time = spawn_boot(b"executable:fabric-call-time");
    // The call worker copies large payloads, so it holds buffer-creation
    // authority of its own, bounded by its declared quota.
    spawn_boot_with(
        b"executable:fabric-call-worker",
        &[
            grant(resolve_own_buffer_factory(), RIGHT_BUFFER_CREATE),
            grant(call_client, RIGHT_SUPERVISE),
            grant(call_client_b, RIGHT_SUPERVISE),
            grant(call_server, RIGHT_SUPERVISE),
            grant(call_time, RIGHT_SUPERVISE),
        ],
    );
    slime_rt::debug_write(b"[init] fabric boot call plane spawned\n");

    let op_client = spawn_boot(b"executable:fabric-op-client");
    let op_client_b = spawn_boot(b"executable:fabric-op-client-b");
    let op_server = spawn_boot(b"executable:fabric-op-server");
    spawn_boot(b"executable:fabric-op-time");
    let op_restart = spawn_boot(b"executable:fabric-op-client-b-restart");
    spawn_boot_with(
        b"executable:fabric-op-worker",
        &[
            grant(op_client, RIGHT_SUPERVISE),
            grant(op_client_b, RIGHT_SUPERVISE),
            grant(op_server, RIGHT_SUPERVISE),
            grant(op_restart, RIGHT_SUPERVISE),
        ],
    );
    slime_rt::debug_write(b"[init] fabric boot operation plane spawned\n");
    slime_rt::debug_write(b"[init] fabric boot graph spawned with static endpoints\n");
    loop {
        match slime_rt::supervision_status(fabric) {
            Ok(None) => slime_rt::yield_now(),
            _ => fail_boot(b"fabric left healthy idle"),
        }
    }
}

/// Drive the C8.13 concurrent traffic plane: the C8.10 three-worker layout,
/// spawned exactly as `drive_boot_plane` spawns it, driving every non-parked
/// worker's real scenario at once instead of parking.
///
/// Only reachable for the authenticated `traffic` action declared by
/// `contracts/generation-manifest/v1/compositions/sel4-traffic.zti`, which is
/// `sel4-boot.zti` with `bootAction`/`generation` changed plus the additional
/// grants real traffic needs that the parked boot scenario never exercised:
/// call-plane phase-barrier edges, an operation-plane restart-release edge,
/// three shared-buffer factories, and `transferable = true` on the call
/// control grants (a downstream loan crosses authority, so delegating one
/// needs the right the parked scenario never asked for). The milestone's
/// requirement is that the *same* declared partition C8.10 already proved
/// collision-free now carries real call and operation traffic concurrently
/// with *untimed* stream traffic (`fabric_boot::active()` reading `"traffic"`
/// differently than `"boot"` is what makes every participant but the observer
/// and proxy run its real per-plane scenario instead of
/// `provision_and_park`/`park_only`) under one fixed schedule. QoS-timed
/// stream traffic is deliberately absent: `fabric-service::qos_check` stays
/// `"qos"`-only, because timing it here needs its own generation-level
/// clock-grant wiring this milestone does not yet do -- see the C8.13 devlog
/// entry's open risks.
///
/// **Why `wait_clean` and not `drive_boot_plane`'s infinite idle loop.** The
/// boot plane's participants never produce traffic, so its brokers never
/// reach their own `finished()` and the plane is a permanent snapshot a gate
/// inspects from outside. Here every participant runs its real scenario to
/// completion, so every worker's own `run()` returns and every handle below
/// is expected to settle clean -- the ordinary completion path every other
/// concurrent plane in this file uses.
pub(super) fn drive_traffic_plane() -> ! {
    slime_rt::debug_write(b"[init] traffic control channels minted\n");
    let publisher = spawn_boot(b"executable:fabric-publisher");
    let subscriber = spawn_boot(b"executable:fabric-subscriber");
    let publisher_b = spawn_boot_with(
        b"executable:fabric-publisher-b",
        &[grant(resolve_own_buffer_factory(), RIGHT_BUFFER_CREATE)],
    );
    let subscriber_b = spawn_boot(b"executable:fabric-subscriber-b");
    let observer = spawn_boot(b"executable:fabric-observer");
    let proxy = spawn_boot(b"executable:fabric-proxy");
    let probe = spawn_boot(b"executable:fabric-probe");
    slime_rt::debug_write(b"[init] traffic stream participants spawned\n");

    let fabric = spawn_boot_with(
        b"executable:fabric-service",
        &[
            grant(resolve_own_buffer_factory(), RIGHT_BUFFER_CREATE),
            grant(publisher, RIGHT_SUPERVISE),
            grant(subscriber, RIGHT_SUPERVISE),
            grant(publisher_b, RIGHT_SUPERVISE),
            grant(subscriber_b, RIGHT_SUPERVISE),
            grant(observer, RIGHT_SUPERVISE),
            grant(proxy, RIGHT_SUPERVISE),
        ],
    );
    slime_rt::debug_write(b"[init] traffic stream broker spawned\n");

    let call_client = spawn_boot_with(
        b"executable:fabric-call-client",
        &[grant(resolve_own_buffer_factory(), RIGHT_BUFFER_CREATE)],
    );
    let call_client_b = spawn_boot(b"executable:fabric-call-client-b");
    let call_server = spawn_boot_with(
        b"executable:fabric-call-server",
        &[grant(resolve_own_buffer_factory(), RIGHT_BUFFER_CREATE)],
    );
    let call_time = spawn_boot(b"executable:fabric-call-time");
    let call_worker = spawn_boot_with(
        b"executable:fabric-call-worker",
        &[
            grant(resolve_own_buffer_factory(), RIGHT_BUFFER_CREATE),
            grant(call_client, RIGHT_SUPERVISE),
            grant(call_client_b, RIGHT_SUPERVISE),
            grant(call_server, RIGHT_SUPERVISE),
            grant(call_time, RIGHT_SUPERVISE),
        ],
    );
    slime_rt::debug_write(b"[init] traffic call plane spawned\n");

    let op_client = spawn_boot(b"executable:fabric-op-client");
    let op_client_b = spawn_boot(b"executable:fabric-op-client-b");
    let op_server = spawn_boot(b"executable:fabric-op-server");
    let op_time = spawn_boot(b"executable:fabric-op-time");
    let op_restart = spawn_boot(b"executable:fabric-op-client-b-restart");
    let op_worker = spawn_boot_with(
        b"executable:fabric-op-worker",
        &[
            grant(op_client, RIGHT_SUPERVISE),
            grant(op_client_b, RIGHT_SUPERVISE),
            grant(op_server, RIGHT_SUPERVISE),
            grant(op_restart, RIGHT_SUPERVISE),
        ],
    );
    slime_rt::debug_write(b"[init] traffic operation plane spawned\n");
    slime_rt::debug_write(b"[init] traffic graph spawned with static endpoints\n");

    // Neither the proxy nor the observer ever contacts the stream broker under
    // `"traffic"` (`fabric_boot::full_graph_active` parks both without
    // requesting a role -- see `fabric-observer::main` and
    // `fabric-service::traffic_graph` for why the observer cannot join real
    // delivery here the way `boot_graph`'s permanently parked copy safely
    // does), so both are the spawned tasks this plane does not wait to exit --
    // each is expected to still be healthy-idle once everything else has
    // settled.
    //
    // C8.14 inverts that for the proxy alone. Its fault plane is this same
    // composition built with the interposition hop compiled to exit instead of
    // park, because a hop dying is the one degradation no participant can
    // script for itself. So on that build the hop is *waited on* rather than
    // checked idle: an injected departure is a declared clean exit, and
    // requiring it here is what proves the graph observed the hop leave rather
    // than merely tolerating a task it stopped tracking.
    const PROXY_DIES: bool = option_env!("SLIME_FABRIC_PROXY_EARLY_EXIT").is_some();
    wait_clean(&[
        publisher,
        subscriber,
        publisher_b,
        subscriber_b,
        probe,
        fabric,
        call_client,
        call_client_b,
        call_server,
        call_time,
        call_worker,
        op_client,
        op_client_b,
        op_server,
        op_time,
        op_restart,
        op_worker,
    ]);
    if PROXY_DIES {
        wait_clean(&[proxy]);
    } else {
        expect_parked(proxy);
    }
    expect_parked(observer);
    slime_rt::debug_write(b"[init] traffic plane reclaimed\n");
    slime_rt::exit(0)
}

/// Launch the C8.7 operation plane: one fabric brokering two clients and one
/// server on the declared `navigation` operation route, plus the capability-routed
/// clock that makes expiry and timeout deterministic.
///
/// Init holds no route capability. Every control is a generation-declared native
/// Endpoint whose two halves the root installs into the fabric and the
/// participant that declared them — so "which component is asking" is a
/// capability fact established by the manifest, not a claim in a message. That
/// binding is exactly what makes the milestone's authority denials hold: client
/// B cannot observe, retrieve, or cancel client A's operation even knowing its
/// identity, because its requests arrive on a different endpoint.
///
/// **Spawn order is load-bearing.** The fabric starts before any participant so
/// no goal can arrive before there is a broker to correlate it, and the clock
/// starts last so no time advance precedes the operations it must expire.
fn launch_fabric_operations() {
    // Controls are generation-declared native Endpoints, installed by the
    // root into both halves before anything runs. Init mints nothing.
    slime_rt::debug_write(b"[init] operation control channels minted\n");
    // The participants precede the broker because it is granted a supervision
    // handle naming each of them, and a handle cannot exist before its task. A
    // native Endpoint reports no peer death, so those handles are the only way
    // the broker observes a participant exit rather than blocking on it.
    let client = slime_rt::spawn(resolve_executable(b"executable:fabric-op-client"), &[])
        .unwrap_or_else(|_| slime_rt::exit(1));
    let client_b = slime_rt::spawn(resolve_executable(b"executable:fabric-op-client-b"), &[])
        .unwrap_or_else(|_| slime_rt::exit(1));
    let server = slime_rt::spawn(resolve_executable(b"executable:fabric-op-server"), &[])
        .unwrap_or_else(|_| slime_rt::exit(1));
    let replacement = slime_rt::spawn(
        resolve_executable(b"executable:fabric-op-client-b-restart"),
        &[],
    )
    .unwrap_or_else(|_| slime_rt::exit(1));
    slime_rt::debug_write(b"[init] operation participants spawned\n");
    slime_rt::debug_write(b"[init] operation replacement introduced\n");
    // What init still passes is exactly what the generation cannot place: the
    // shared-buffer factory it holds, and one supervision handle per
    // participant, which only exist once those tasks do. Matching is positional
    // against ascending declared slot: factory at 1, then the handles.
    let service = slime_rt::spawn(
        resolve_executable(b"executable:fabric-service"),
        &[
            grant(resolve_buffer_factory(), RIGHT_BUFFER_CREATE),
            grant(client.supervision_slot, RIGHT_SUPERVISE),
            grant(client_b.supervision_slot, RIGHT_SUPERVISE),
            grant(server.supervision_slot, RIGHT_SUPERVISE),
            grant(replacement.supervision_slot, RIGHT_SUPERVISE),
        ],
    )
    .unwrap_or_else(|_| slime_rt::exit(1));
    slime_rt::debug_write(b"[init] operation fabric spawned\n");
    slime_rt::debug_write(b"[init] operation supervision delegated\n");
    let time = slime_rt::spawn(resolve_executable(b"executable:fabric-op-time"), &[])
        .unwrap_or_else(|_| slime_rt::exit(1));
    slime_rt::debug_write(b"[init] operation replacement released\n");
    wait_clean(&[
        client.supervision_slot,
        client_b.supervision_slot,
        server.supervision_slot,
        replacement.supervision_slot,
        time.supervision_slot,
        service.supervision_slot,
    ]);
}

fn launch_fabric_calls() {
    // Every control endpoint is a generation-declared native Endpoint: the
    // root materializes both halves from the manifest's grants and installs
    // each side into the instance that declared it, before any of them runs.
    // Init therefore holds no route capability and mints nothing.
    slime_rt::debug_write(b"[init] call control channels minted\n");
    // The participants precede the fabric because the broker is granted a
    // supervision handle naming each of them, and a handle cannot exist before
    // its task. A native Endpoint reports no peer death, so those handles are
    // the only way the broker observes a participant exit rather than blocking
    // on it forever.
    let client = slime_rt::spawn(
        resolve_executable(b"executable:fabric-call-client"),
        &[grant(resolve_buffer_factory(), RIGHT_BUFFER_CREATE)],
    )
    .unwrap_or_else(|_| slime_rt::exit(1));
    let client_b = slime_rt::spawn(resolve_executable(b"executable:fabric-call-client-b"), &[])
        .unwrap_or_else(|_| slime_rt::exit(1));
    let server = slime_rt::spawn(
        resolve_executable(b"executable:fabric-call-server"),
        &[grant(resolve_buffer_factory(), RIGHT_BUFFER_CREATE)],
    )
    .unwrap_or_else(|_| slime_rt::exit(1));
    slime_rt::debug_write(b"[init] call participants spawned\n");
    // What init still passes is exactly what the generation cannot place: the
    // shared-buffer factory it holds, and one supervision handle per
    // participant, which only exist once those tasks do. Matching is positional
    // against ascending declared slot: factory at 1, then the handles.
    //
    // The clock is spawned here, before the broker, for the same reason the
    // three participants are: the broker is granted a supervision handle naming
    // it too (B76). It used to be spawned last, when nothing named it -- and
    // the broker then inferred the clock's exit from the *server's* handle,
    // which names a different task. A clock that exits while the server lives
    // was observed by nothing, and the exit predicate that gates the trace
    // flush waited forever.
    let time = slime_rt::spawn(resolve_executable(b"executable:fabric-call-time"), &[])
        .unwrap_or_else(|_| slime_rt::exit(1));
    let service = slime_rt::spawn(
        resolve_executable(b"executable:fabric-service"),
        &[
            grant(resolve_buffer_factory(), RIGHT_BUFFER_CREATE),
            grant(client.supervision_slot, RIGHT_SUPERVISE),
            grant(client_b.supervision_slot, RIGHT_SUPERVISE),
            grant(server.supervision_slot, RIGHT_SUPERVISE),
            grant(time.supervision_slot, RIGHT_SUPERVISE),
        ],
    )
    .unwrap_or_else(|_| slime_rt::exit(1));
    slime_rt::debug_write(b"[init] call fabric spawned\n");
    slime_rt::debug_write(b"[init] call supervision delegated\n");
    wait_clean(&[
        client.supervision_slot,
        client_b.supervision_slot,
        server.supervision_slot,
        time.supervision_slot,
        service.supervision_slot,
    ]);
}

/// Drive the P5.5.2 stream plane: the full C8.4 graph the x86 oracle builds —
/// two publishers, two subscribers, two routes, the `>MAX_INLINE_BYTES`
/// descriptor and loan path, and KEEP_LAST eviction under a stalled subscriber.
///
/// Only reachable for the authenticated `stream` or `qos` action declared by
/// the corresponding seL4 generation manifest.
///
/// This is `launch_fabric_graph`'s shape and its spawn order, on the same
/// authority argument. Init holds **no route capability at all**: it mints one
/// control channel per participant, and the binding between a control endpoint
/// and a component identity is established here, at spawn. That is what the
/// fabric authenticates against — a client cannot forge, share, or re-derive
/// one, so "which component is asking" is a capability fact rather than a claim
/// in a message.
///
/// **Spawn order is load-bearing**, exactly as it is on x86. Both subscribers
/// start before the fabric, because a downstream loan names its receiver
/// through a `RIGHT_SUPERVISE` capability rather than an ambient task id, so
/// those handles must exist before the service does. The publishers follow the
/// fabric, so no sample arrives before there is a broker for it.
///
/// `fabric-intruder` is spawned holding a real control endpoint on purpose.
/// The denial under test is not "no channel" but "no declared edge".
/// `qos` records which plane booted; both compose identically here, because the
/// QoS graph's extra edge — the capability-routed clock between
/// `fabric-publisher-b` and the broker — is a declared grant the root installs
/// rather than anything init places. The distinction is the generation's own
/// `bootAction`, delivered at activation, so the QoS generation needs no paired
/// build flag: the service and participants select behavior from the same
/// generated boot action.
pub(super) fn drive_stream_plane(_qos: bool) {
    launch_fabric_graph(b"fabric", b" service spawned\n");
}

/// Drive the P5.4.6 call plane: the C8.6 bounded-native-call graph the x86
/// oracle builds — two clients, a server, and a capability-routed clock over
/// one `ParameterCall` route.
///
/// Only reachable for the authenticated `call` action declared by
/// `contracts/generation-manifest/v1/compositions/sel4-call.zti`.
///
/// **Why the generation declares these controls rather than init minting them.**
/// An earlier version had init mint all five and hand out the halves at spawn,
/// because a root-materialized grant landed at the fabric's own channel cursor —
/// which resumed *above* the factory grants staging installed, so the fabric
/// received `[0, 3, 4, 5, 6]` and no `base + index` describes a set with a hole
/// in it. Declared native Endpoints removed both that hole and the ordering
/// hazard behind it (`build_generation` sorts grants by `(name, source, target)`,
/// so `fabric-call-client-b-control` sorted ahead of
/// `fabric-call-client-control` and would have bound client B's identity to
/// client A's slot): each side is installed at the slot its own binding names,
/// so the fabric's controls are contiguous from `FABRIC_FIRST_CONTROL_SLOT` in
/// grant order, which is what the broker compiles against.
///
/// **Spawn order matches the x86 oracle.** A spawn grant is a non-consuming
/// copy. Init spawns each participant, then the fabric, and moves each
/// supervision handle to the broker — the one authority the generation cannot
/// place, because it does not exist until the task does. The broker still
/// receives the request first and the matching identity second; no participant
/// needs authority naming itself.
pub(super) fn drive_call_plane() {
    launch_fabric_calls();
}

/// Drive the P5.4.7 operation plane: the C8.7 bounded-native-operation graph
/// the x86 oracle builds — two clients, a supervised replacement for the
/// second, a server, and a capability-routed clock over the `navigation` route
/// plus client A's private `nav-backup` route.
///
/// Only reachable for the authenticated `operation` action declared by
/// `contracts/generation-manifest/v1/compositions/sel4-operation.zti`; broker and participants
/// read the same generated profile while only this composition differs.
///
/// **Why the generation declares the control channels.** `drive_call_plane`'s
/// reason, which applies unchanged: each control is a native Endpoint the root
/// installs into both declaring instances at the slot each binding names, so the
/// broker's controls are contiguous from `base` in grant order. The grant names
/// are also what `_control_sources` derives `FABRIC_OPERATION_CLIENTS` from —
/// the table the broker maps a control slot to a caller identity with.
///
/// **The replacement is the restart arm.** C8.7 requires a participant restart
/// to be deterministic: the broker keeps client B's authenticated index, its
/// correlation high-water mark, and its retained results, while the replacement
/// receives a *fresh* non-delegable role. Init spawns the replacement on its own
/// authenticated control and vouches for it exactly as for the others, then
/// releases it through a private barrier so its role request cannot reach the
/// broker before the original client has produced the retained result the
/// replacement is supposed to find.
pub(super) fn drive_operation_plane() {
    launch_fabric_operations();
}

/// Drive the P5.4.8 visibility plane: the C8.8 filtered-introspection and
/// declared-interposition graph the x86 oracle builds — the telemetry and
/// diagnostics routes with `fabric-intruder` as the *declared proxy* on the
/// telemetry subscriber's chain.
///
/// Only reachable for the authenticated `visibility` action declared by
/// `contracts/generation-manifest/v1/compositions/sel4-visibility.zti`; broker and participants
/// read the same generated profile while only this composition differs.
///
/// This is `drive_stream_plane`'s shape with two differences, both consequences
/// of what the visibility broker does rather than choices made here:
///
/// * **No supervision handles.** The stream fabric is granted one per subscriber
///   so it can name a loan receiver. The visibility broker mints every route
///   half itself and hands out narrowed, non-delegable roles, so it needs no
///   handle and init keeps all six.
/// * **No spawn ordering constraint.** Nothing in this plane requires a task to
///   exist before another's grants are built, because no grant names a task.
///   Participants are spawned in control-slot order purely so the transcript
///   reads in the order the broker will answer them.
pub(super) fn drive_visibility_plane() {
    launch_fabric_graph(b"visibility", b" fabric spawned\n");
}

/// Drive C9.6's robot workload: spawn every dynamically constructed
/// participant, in the one order the mechanism requires, and hand each
/// broker the supervision handle it needs over its own dependents.
///
/// **Why this plane cannot be root-launched, unlike C9.1-C9.5's planes.**
/// Every prior C9 plane's participants held their declared authority directly
/// and needed no capability *from* another participant, so every instance
/// could be root-autostart and init could exit immediately. `fabric-service`
/// and `fabric-call-worker` are different: they must hold a *supervision*
/// handle over `robot-sensor` and over `robot-actuator`/`robot-clock`
/// respectively, to bind each ring's loan to its receiver and to observe peer
/// death. A supervision handle exists only as the result of a `spawn()` call —
/// there is no boot-time mechanism that hands one root-autostart instance
/// authority over another — so the subjects must be spawned, and the same
/// spawner must then mint that handle into the holder it spawns next.
///
/// **Spawn order is load-bearing**, for exactly that reason: `robot-sensor`
/// before `fabric-service`, and `robot-actuator`/`robot-clock` before
/// `fabric-call-worker`. `robot-supervisor` and `robot-burner` need no
/// capability from init at all — the first receives its authority over
/// `robot-controller` from a self-sourced grant the generation installs
/// directly at its own (root-autostart) construction, and the second needs
/// none — so both are declared root-autostart instead and this function never
/// names them.
///
/// `robot-controller` is spawned by neither: it is `robot-supervisor`'s own
/// dependent, restarted under the declared lifecycle policy, and no capability
/// it holds can cross through init.
pub(super) fn drive_robot_runtime_plane() {
    let sensor = spawn_boot(b"executable:robot-sensor");
    let actuator = spawn_boot(b"executable:robot-actuator");
    let clock = spawn_boot(b"executable:robot-clock");
    slime_rt::debug_write(b"[init] robot runtime sensors spawned\n");

    let fabric = spawn_boot_with(
        b"executable:fabric-service",
        &[
            grant(resolve_own_buffer_factory(), RIGHT_BUFFER_CREATE),
            grant(sensor, RIGHT_SUPERVISE),
        ],
    );
    let call_worker = spawn_boot_with(
        b"executable:fabric-call-worker",
        &[
            grant(resolve_own_buffer_factory(), RIGHT_BUFFER_CREATE),
            grant(actuator, RIGHT_SUPERVISE),
            grant(clock, RIGHT_SUPERVISE),
        ],
    );
    slime_rt::debug_write(b"[init] robot runtime brokers spawned\n");

    // `robot-supervisor` and `robot-burner` run independently of everything
    // spawned here: both are root-autostart, so the root constructed them
    // before this function ran, and their completion is tracked by the
    // generation's own required-instance health rather than by this wait.
    //
    // Init does not `wait_clean` here, unlike the C8 fabric planes: those
    // planes' participants share one undeclared default priority band with
    // each other, strictly below init's, so init spinning a busy `yield_now`
    // loop still lets that one band run. This plane declares participants
    // across foreground/normal/bestEffort, and the burner's whole claim is
    // that a foreground band's *blocking* wait — never a spin — is what lets
    // it run at all; a spinning waiter one band above everything only adds a
    // second thread the scheduler must service, at init's own high priority,
    // for no observable benefit. Init's job was placing the graph's authority;
    // the graph's own completion is what `SLIME_GRAPH HEALTHY` reports,
    // independent of whether init is still watching.
    let _ = (sensor, actuator, clock, fabric, call_worker);
}

/// Drive the C8.12 matrix plane: matching, filtered visibility, and denial
/// exercised together against one graph.
///
/// Only reachable for the authenticated `matrix` action declared by
/// `contracts/generation-manifest/v1/compositions/sel4-matrix.zti`.
///
/// The composition is `launch_fabric_graph`'s, with three differences the
/// milestone forces rather than choices made here:
///
/// * **Seven participants, not five.** The ungranted probe, the declared
///   interposition proxy, and the read-only observer are three distinct task
///   identities with non-overlapping grants, so a denial can never be confused
///   for a role the graph granted somewhere else. `fabric-intruder`, which
///   carried all three roles at once behind an env switch, is absent.
/// * **The probe holds a real control endpoint.** The denial under test is "no
///   declared edge", not "no channel": a component refused for lack of a
///   channel would prove nothing about the graph.
/// * **One supervision handle per ring participant and the proxy.** The broker
///   needs the proxy's to observe a hop through a dead one — a native Endpoint
///   reports no peer death — and each ring holder's to name a loan receiver.
///   The probe gets none: it asks for nothing the broker must reclaim.
pub(super) fn drive_matrix_plane() {
    plane_marker(b"matrix", b" control channels minted\n");
    let publisher = spawn_boot(b"executable:fabric-publisher");
    let subscriber = spawn_boot(b"executable:fabric-subscriber");
    let publisher_b = spawn_boot_with(
        b"executable:fabric-publisher-b",
        &[grant(resolve_buffer_factory(), RIGHT_BUFFER_CREATE)],
    );
    let subscriber_b = spawn_boot(b"executable:fabric-subscriber-b");
    let observer = spawn_boot(b"executable:fabric-observer");
    let proxy = spawn_boot(b"executable:fabric-proxy");
    let probe = spawn_boot(b"executable:fabric-probe");
    plane_marker(b"matrix", b" participants spawned\n");

    // Positional against the child's ascending declared slot, exactly as every
    // other fabric plane: the factory first, then one supervision handle per
    // holder. A composition that drifted from the fixture is refused at spawn
    // rather than mis-bound -- by the root, which derives the expected count
    // from the generation in `preflight_spawn_grants` and reports both operands.
    // Init carried a copy of that check against a generated table; both sides
    // regenerated from one manifest together, so it could only confirm the
    // table agreed with itself.
    //
    // The probe's handle is in the vector even though it holds no edge. The
    // broker's dispatch loop needs it to know the refused caller has stopped
    // asking — a native Endpoint reports no peer death — and it grants the probe
    // nothing: the fabric holds the handle, not the probe.
    let grants = [
        grant(resolve_buffer_factory(), RIGHT_BUFFER_CREATE),
        grant(publisher, RIGHT_SUPERVISE),
        grant(subscriber, RIGHT_SUPERVISE),
        grant(publisher_b, RIGHT_SUPERVISE),
        grant(subscriber_b, RIGHT_SUPERVISE),
        grant(observer, RIGHT_SUPERVISE),
        grant(probe, RIGHT_SUPERVISE),
        grant(proxy, RIGHT_SUPERVISE),
    ];
    let service = spawn_boot_with(b"executable:fabric-service", &grants);
    plane_marker(b"matrix", b" fabric spawned\n");

    wait_clean(&[
        publisher,
        subscriber,
        publisher_b,
        subscriber_b,
        observer,
        proxy,
        probe,
        service,
    ]);
}

/// Launch the C8.3/C8.4 fabric plane: one service that owns every route
/// endpoint, and five clients that can only ask it for one.
///
/// Init deliberately holds no route capability. Every control is a
/// generation-declared native Endpoint whose two halves the root installs into
/// the fabric and the client that declared them, so the binding between a
/// control endpoint and a component identity is a manifest fact the fabric
/// authenticates against — a client cannot forge, share, or re-derive one, so
/// "which component is asking" is a capability fact rather than a claim in a
/// message.
///
/// **Spawn order is load-bearing.** Both subscribers are spawned before the
/// fabric, because the fabric needs their supervision handles: a downstream
/// loan names its receiver through a `RIGHT_SUPERVISE` capability, never
/// through an ambient task id, so the handle must exist before the service
/// starts. The publishers follow the fabric, so no sample can arrive before
/// the service is ready to broker it.
///
/// `fabric-intruder` is spawned with a real control endpoint on purpose: the
/// denial under test is not "no channel" but "no declared edge".
pub(super) fn launch_fabric_graph(plane: &[u8], service_spawned: &[u8]) {
    // Every control endpoint is a generation-declared native Endpoint: the
    // root materializes both halves from the manifest's grants and installs
    // each side into the instance that declared it, before any of them runs.
    // Init therefore holds no route capability and mints nothing -- the
    // binding between a control endpoint and a component identity is a
    // generation fact, which is exactly what the fabric authenticates
    // against.
    plane_marker(plane, b" control channels minted\n");
    // Every ring participant starts before the fabric. A v2 stream edge is a
    // writable shared ring the fabric *loans* to its peer, and a loan names
    // its receiver through a supervision capability — so the handle must
    // exist before the service that will use it does. Under v1 only
    // subscribers received loans, which is why only they had to precede it.
    let publisher = slime_rt::spawn(resolve_executable(b"executable:fabric-publisher"), &[])
        .unwrap_or_else(|_| slime_rt::exit(1));
    let subscriber = slime_rt::spawn(resolve_executable(b"executable:fabric-subscriber"), &[])
        .unwrap_or_else(|_| slime_rt::exit(1));
    let publisher_b = slime_rt::spawn(
        resolve_executable(b"executable:fabric-publisher-b"),
        &[grant(resolve_own_buffer_factory(), RIGHT_BUFFER_CREATE)],
    )
    .unwrap_or_else(|_| slime_rt::exit(1));
    let subscriber_b = slime_rt::spawn(resolve_executable(b"executable:fabric-subscriber-b"), &[])
        .unwrap_or_else(|_| slime_rt::exit(1));
    // The declared interposition proxy precedes the fabric for the same reason
    // every ring participant does: the fabric is granted a supervision handle
    // naming it, and a handle cannot exist before its task. A native Endpoint
    // reports no peer death, so this handle is the only way the broker can
    // observe a hop through a dead proxy rather than blocking on it forever.
    let intruder = slime_rt::spawn(resolve_executable(b"executable:fabric-intruder"), &[])
        .unwrap_or_else(|_| slime_rt::exit(1));
    // What init still passes is exactly what the generation cannot place: the
    // shared-buffer factory it holds, and one supervision handle per ring
    // participant and declared proxy, which only exist once those tasks do.
    // Matching is positional against ascending declared slot, so the order here
    // is the order the fixtures declare: factory first, then publisher(7),
    // subscriber(8), the proxy where a plane interposes one, then the b-pair.
    //
    // Which planes interpose is a manifest fact, and the question is *which*
    // rather than *how many*: `sel4-visibility` declares
    // `fabric-intruder-supervision` between subscriber and publisher-b, and
    // `sel4-stream` declares no such handle at all. Asking the root by name
    // answers that directly. The generated count this replaced could only say
    // "six or five", which happens to discriminate here but is a summary of the
    // composition rather than a statement about it.
    //
    // The factory is resolved by grant name rather than by capability role.
    // `resolve_buffer_factory`'s `kind:` query stood here, and it answered only
    // because the three fixtures that reached *this* launcher —  `sel4-stream`,
    // `sel4-qos`, `sel4-visibility` — bind init one factory each. The ambiguity
    // itself is older than RP2: `sel4-boot` and `sel4-traffic` already bind two,
    // which is why `resolve_own_buffer_factory` existed before this change.
    // `sel4-demo` binds three and reaches here, so the query now refuses on this
    // path too — correctly, since which factory a participant allocates from is
    // a graph fact the manifest states, not a property of the capability.
    //
    // `init-shared-buffer-factory` is bound to init by every fixture that
    // reaches *this* launcher, verified directly: `sel4-stream`, `sel4-qos`,
    // `sel4-visibility`, and `sel4-demo`. The resolver has other callers across
    // the boot and traffic planes; this note surveys only the ones on this path.
    let grants = [
        grant(resolve_own_buffer_factory(), RIGHT_BUFFER_CREATE),
        grant(publisher.supervision_slot, RIGHT_SUPERVISE),
        grant(subscriber.supervision_slot, RIGHT_SUPERVISE),
        grant(intruder.supervision_slot, RIGHT_SUPERVISE),
        grant(publisher_b.supervision_slot, RIGHT_SUPERVISE),
        grant(subscriber_b.supervision_slot, RIGHT_SUPERVISE),
    ];
    let without_proxy = [grants[0], grants[1], grants[2], grants[4], grants[5]];
    // A failed resolve *is* absence, stated by the generation rather than
    // papered over at build time -- the same reading the notification axis
    // established when it replaced an always-emitted `SLOT_ABSENT`.
    let interposes = declares_minted(b"fabric-intruder-supervision");
    let service = slime_rt::spawn(
        resolve_executable(b"executable:fabric-service"),
        if interposes {
            &grants[..]
        } else {
            &without_proxy[..]
        },
    )
    .unwrap_or_else(|_| slime_rt::exit(1));
    plane_marker(plane, service_spawned);
    plane_marker(plane, b" participants spawned\n");
    wait_clean(&[
        publisher.supervision_slot,
        publisher_b.supervision_slot,
        subscriber.supervision_slot,
        subscriber_b.supervision_slot,
        intruder.supervision_slot,
        service.supervision_slot,
    ]);
}

/// One `[init] <plane><suffix>` line. The stream and visibility gates name the
/// same composition differently, so the plane word is a parameter rather than
/// two copies of this launcher.
fn plane_marker(plane: &[u8], suffix: &[u8]) {
    slime_rt::debug_write(b"[init] ");
    slime_rt::debug_write(plane);
    slime_rt::debug_write(suffix);
}

pub(super) fn write_i64(value: i64) {
    if value < 0 {
        slime_rt::debug_write(b"-");
        write_u32(value.unsigned_abs() as u32);
    } else {
        write_u32(value as u32);
    }
}

pub(super) fn write_u32(mut value: u32) {
    let mut buffer = [0u8; 10];
    let mut cursor = buffer.len();
    if value == 0 {
        slime_rt::debug_write(b"0");
        return;
    }
    while value != 0 {
        cursor -= 1;
        buffer[cursor] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    slime_rt::debug_write(&buffer[cursor..]);
}
