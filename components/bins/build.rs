fn main() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let target = std::env::var("TARGET").expect("TARGET");
    if target == "x86_64-unknown-none" {
        println!("cargo:rustc-link-arg=-T{manifest_dir}/../component.ld");
        println!("cargo:rerun-if-changed={manifest_dir}/../component.ld");
    }
    println!("cargo:rerun-if-env-changed=SLIME_GENERATION_NUMBER");
    println!("cargo:rerun-if-env-changed=SLIME_RECOVERY_INTERRUPT");
    println!("cargo:rerun-if-env-changed=SLIME_RECOVERY_IMAGE");
    println!("cargo:rerun-if-env-changed=SLIME_DANGO_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_GENERATION_CMD_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_POWERBOX_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_SAMPLE_PLANE_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_FABRIC_AUTHORITY_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_FABRIC_STREAM_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_FABRIC_QOS_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_FABRIC_CALL_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_FABRIC_OPERATION_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_FABRIC_VISIBILITY_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_FABRIC_PROXY_EARLY_EXIT");
    println!("cargo:rerun-if-env-changed=SLIME_GENERATION_CANDIDATE");
    println!("cargo:rerun-if-env-changed=SLIME_GENERATION_CMD_SCENARIO");
    if let Ok(number) = std::env::var("SLIME_GENERATION_NUMBER") {
        println!("cargo:rustc-env=SLIME_GENERATION_NUMBER={number}");
    }
    if let Ok(value) = std::env::var("SLIME_RECOVERY_IMAGE") {
        println!("cargo:rustc-env=SLIME_RECOVERY_IMAGE={value}");
    }
    if let Ok(value) = std::env::var("SLIME_RECOVERY_INTERRUPT") {
        println!("cargo:rustc-env=SLIME_RECOVERY_INTERRUPT={value}");
    }
    if let Ok(value) = std::env::var("SLIME_DANGO_CHECK") {
        println!("cargo:rustc-env=SLIME_DANGO_CHECK={value}");
    }
    if let Ok(value) = std::env::var("SLIME_GENERATION_CMD_CHECK") {
        println!("cargo:rustc-env=SLIME_GENERATION_CMD_CHECK={value}");
    }
    if let Ok(value) = std::env::var("SLIME_POWERBOX_CHECK") {
        println!("cargo:rustc-env=SLIME_POWERBOX_CHECK={value}");
    }
    if let Ok(value) = std::env::var("SLIME_SAMPLE_PLANE_CHECK") {
        println!("cargo:rustc-env=SLIME_SAMPLE_PLANE_CHECK={value}");
    }
    if let Ok(value) = std::env::var("SLIME_FABRIC_AUTHORITY_CHECK") {
        println!("cargo:rustc-env=SLIME_FABRIC_AUTHORITY_CHECK={value}");
    }
    if let Ok(value) = std::env::var("SLIME_FABRIC_STREAM_CHECK") {
        println!("cargo:rustc-env=SLIME_FABRIC_STREAM_CHECK={value}");
    }
    if let Ok(value) = std::env::var("SLIME_FABRIC_QOS_CHECK") {
        println!("cargo:rustc-env=SLIME_FABRIC_QOS_CHECK={value}");
    }
    if let Ok(value) = std::env::var("SLIME_FABRIC_CALL_CHECK") {
        println!("cargo:rustc-env=SLIME_FABRIC_CALL_CHECK={value}");
    }
    if let Ok(value) = std::env::var("SLIME_FABRIC_VISIBILITY_CHECK") {
        println!("cargo:rustc-env=SLIME_FABRIC_VISIBILITY_CHECK={value}");
    }
    if let Ok(value) = std::env::var("SLIME_FABRIC_PROXY_EARLY_EXIT") {
        println!("cargo:rustc-env=SLIME_FABRIC_PROXY_EARLY_EXIT={value}");
    }
    if let Ok(value) = std::env::var("SLIME_GENERATION_CANDIDATE") {
        println!("cargo:rustc-env=SLIME_GENERATION_CANDIDATE={value}");
    }
    if let Ok(value) = std::env::var("SLIME_GENERATION_CMD_SCENARIO") {
        println!("cargo:rustc-env=SLIME_GENERATION_CMD_SCENARIO={value}");
    }
    generate_command_profile(manifest_dir);
    generate_fabric_profile(manifest_dir);
}

fn generate_command_profile(manifest_dir: &str) {
    let manifest_path =
        std::path::Path::new(manifest_dir).join("../../contracts/generation/v1/fixtures/valid.zti");
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    let manifest = std::fs::read_to_string(&manifest_path).expect("read generation manifest");
    let dango = component_block(&manifest, "dango").expect("dango component");
    let profile = field_list(dango, "commandProfile").expect("dango command profile");
    let client_budget = field_int(dango, "spawnBudget").expect("dango spawn budget");
    let entries = profile
        .iter()
        .map(|command| {
            let target = if *command == "echo" {
                "echo-agent"
            } else {
                command
            };
            let slot = component_slot(&manifest, target).expect("profile executable component");
            let block = component_block(&manifest, target).expect("profile executable component");
            let object = field(block, "object").expect("component object");
            (*command, object, slot)
        })
        .collect::<Vec<_>>();
    let generated = entries
        .iter()
        .map(|(name, object, slot)| format!("    (b\"{name}\", b\"{object}\", {slot}),\n"))
        .collect::<String>();
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(
        out.join("command_profile.rs"),
        format!(
            "pub const CLIENT_BUDGET: usize = {client_budget};\npub const COMMAND_PROFILE: &[(&[u8], &[u8], u32)] = &[\n{generated}];\n"
        ),
    )
    .expect("write command profile");
    let generated_names = entries
        .iter()
        .map(|(name, _, _)| format!("    b\"{name}\",\n"))
        .collect::<String>();
    std::fs::write(
        out.join("dango_profile.rs"),
        format!(
            "pub const CLIENT_BUDGET: u8 = {client_budget};\npub const COMMAND_NAMES: &[&[u8]] = &[\n{generated_names}];\n"
        ),
    )
    .expect("write dango profile");
}

/// Emit the C8.3 fabric participant table from the same generation manifest the
/// host builder encodes into the authenticated fabric-graph resource.
///
/// The fabric service must know which (component, route, direction) edges the
/// generation declared without asking the kernel: the kernel is unaware of
/// routes by design, so there is no syscall to read the graph. Deriving the
/// table here from the manifest keeps one source of truth — a route renamed or
/// a participant removed changes both the resource and this table in the same
/// build. The full interface identity is not restated: the fabric folds the
/// route identity at runtime from the generated C8.1 `INTERFACE_IDENTITY`, so
/// the identity can never drift from the admitted schema.
fn generate_fabric_profile(manifest_dir: &str) {
    let manifest_path =
        std::path::Path::new(manifest_dir).join("../../contracts/generation/v1/fixtures/valid.zti");
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    let manifest = std::fs::read_to_string(&manifest_path).expect("read generation manifest");
    let limits = fabric_limits(&manifest);
    let call_deadline_ns = participants_call_deadline(&manifest, "parameters");
    let operation_deadline_ns = participants_call_deadline(&manifest, "navigation");
    let participants = fabric_participants(&manifest);
    if std::env::var("SLIME_FABRIC_VISIBILITY_CHECK").as_deref() == Ok("1") {
        assert!(
            participants
                .iter()
                .all(|(_, route, _, _)| route.len() <= 16),
            "visibility profile route name exceeds 16-byte record bound"
        );
    }
    // The parser is indentation-keyed, so its failure mode is a short table
    // rather than an empty one — and a short table is silent: the fabric denies
    // by default, so a dropped participant becomes a refused component with no
    // diagnostic. Counting `component = "..."` lines inside the same block is
    // an independent reading of the same bytes, so the two disagree exactly
    // when the structural parse lost an entry. Interposition hops also declare
    // a component, so they are excluded by depth the same way the parser
    // includes participants.
    let declared = declared_participant_count(&manifest);
    assert!(
        declared > 0,
        "generation manifest declares no fabric participants"
    );
    assert_eq!(
        participants.len(),
        declared,
        "fabric participant parse lost entries; the manifest declares {declared}"
    );
    let rows = participants
        .iter()
        .map(|(component, route, interface, direction)| {
            format!("    (b\"{component}\", \"{route}\", \"{interface}\", {direction}),\n")
        })
        .collect::<String>();
    // KEEP_LAST depth is per (component, route): the same component may sit on
    // two routes with different declared depths, and the fabric sizes each
    // subscriber's ring from its own entry rather than from a shared default.
    //
    // The two scans are independent, so a positional `zip` would silently
    // truncate — and a mis-sized ring is exactly the kind of quiet wrong answer
    // KEEP_LAST must not have. Assert the lengths agree before pairing them.
    let qos = fabric_qos(&manifest);
    assert_eq!(
        participants.len(),
        qos.len(),
        "every fabric participant declares one complete QoS policy"
    );
    let depths = participants
        .iter()
        .zip(qos.iter())
        .map(|((component, route, _, _), entry)| {
            format!(
                "    (b\"{component}\", \"{route}\", {}),\n",
                entry.history_depth
            )
        })
        .collect::<String>();
    let qos_rows = participants
        .iter()
        .zip(qos.iter())
        .map(|((component, route, _, _), entry)| {
            format!(
                "    (b\"{component}\", \"{route}\", {}, {}, {}, {}, {}, {}, {}, {}),\n",
                entry.deadline_ns,
                entry.lifespan_ns,
                entry.lease_ns,
                entry.history_depth,
                entry.retained_depth,
                entry.reliability,
                entry.durability,
                entry.liveliness,
            )
        })
        .collect::<String>();
    let visibility = fabric_visibility(&manifest);
    assert_eq!(
        participants.len(),
        visibility.len(),
        "every fabric participant declares one visibility policy"
    );
    let visibility_rows = participants
        .iter()
        .zip(visibility.iter())
        .map(
            |((component, route, _, _), (visible_component, visible_route, policy))| {
                assert_eq!(
                    component, visible_component,
                    "visibility component order drift"
                );
                assert_eq!(route, visible_route, "visibility route order drift");
                format!("    (b\"{component}\", \"{route}\", {policy}),\n")
            },
        )
        .collect::<String>();
    let interpositions = fabric_interpositions(&manifest);
    let interposition_rows = interpositions
        .iter()
        .map(|(component, route, chain)| {
            let hops = chain
                .iter()
                .map(|hop| format!("b\"{hop}\" as &[u8]"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("    (b\"{component}\", \"{route}\", &[{hops}]),\n")
        })
        .collect::<String>();
    // Control endpoints, in the order init grants them, one set per plane.
    // Derived from the manifest's `*-control` grants rather than from the
    // participant table: `fabric-intruder` holds a real control endpoint and
    // appears in no route, which is exactly the denial C8.3 tests, and each
    // plane's time service drives a clock from no route at all. Reading
    // participants here would drop both.
    let clients = fabric_control_clients(&manifest, Plane::Stream, false);
    let call_clients = fabric_control_clients(&manifest, Plane::Call, false);
    let operation_clients = fabric_control_clients(&manifest, Plane::Operation, false);
    assert!(
        !clients.is_empty(),
        "generation manifest declares no fabric control endpoints"
    );
    assert!(
        !call_clients.is_empty(),
        "generation manifest declares no fabric call control endpoints"
    );
    assert!(
        !operation_clients.is_empty(),
        "generation manifest declares no fabric operation control endpoints"
    );
    // Every route participant needs a control endpoint on its own plane: that
    // channel is how it asks for its role and how the fabric authenticates it.
    // Determine the plane from the participant's actual control grant, never
    // from its component-name spelling.
    for (component, _, _, _) in participants.iter() {
        let planes = [Plane::Stream, Plane::Call, Plane::Operation]
            .into_iter()
            .filter(|candidate| {
                fabric_control_clients(&manifest, *candidate, true)
                    .iter()
                    .any(|client| client == component)
            })
            .count();
        assert_eq!(
            planes, 1,
            "fabric participant {component} must have exactly one control-endpoint plane"
        );
    }
    let client_rows = clients
        .iter()
        .map(|component| format!("    b\"{component}\",\n"))
        .collect::<String>();
    let call_client_rows = call_clients
        .iter()
        .map(|component| format!("    b\"{component}\",\n"))
        .collect::<String>();
    let operation_client_rows = operation_clients
        .iter()
        .map(|component| format!("    b\"{component}\",\n"))
        .collect::<String>();
    // Supervision handles the fabric holds for its subscribers, at the slots
    // init grants after the control endpoints. A downstream loan names its
    // receiver through one of these rather than through an ambient task id.
    // Only stream subscribers appear: a call or operation server receives no
    // brokered sample and so needs no loan receiver binding.
    let mut subscribers: Vec<&String> = Vec::new();
    for (component, _, _, direction) in participants.iter() {
        if *direction == 2 && !subscribers.contains(&component) {
            subscribers.push(component);
        }
    }
    let supervision_rows = subscribers
        .iter()
        .enumerate()
        .map(|(index, component)| {
            let slot = FABRIC_FIRST_CONTROL_SLOT + clients.len() + index;
            format!("    (b\"{component}\", {slot}),\n")
        })
        .collect::<String>();
    let subscriber_rows = subscribers
        .iter()
        .map(|component| format!("    b\"{component}\",\n"))
        .collect::<String>();
    assert!(
        limits.in_flight_calls > 0,
        "fabric graph admits no in-flight calls"
    );
    assert!(
        limits.in_flight_operations > 0,
        "fabric graph admits no in-flight operations"
    );
    assert!(limits.retries > 0, "fabric graph admits no call retries");
    assert!(
        limits.retained_samples > 0,
        "fabric graph admits no retained results"
    );
    assert!(
        limits.event_depth > 0,
        "fabric graph admits no operation events"
    );
    assert!(call_deadline_ns > 0, "call route declares no deadline");
    assert!(
        operation_deadline_ns > 0,
        "operation route declares no deadline"
    );
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(
        out.join("fabric_profile.rs"),
        format!(
            "/// Every (component, route name, interface name, direction) edge the\n\
             /// generation declares. Deny by default: a component absent from this\n\
             /// table holds no route authority, whatever it asks for.\n\
             pub const FABRIC_PARTICIPANTS: &[(&[u8], &str, &str, u32)] = &[\n{rows}];\n\
             \n\
             /// The KEEP_LAST depth the generation declares for each edge.\n\
             pub const FABRIC_HISTORY_DEPTHS: &[(&[u8], &str, u32)] = &[\n{depths}];\n\
             /// Complete generation-declared QoS per participant.\n\
             pub type FabricQosRow = (&'static [u8], &'static str, u64, u64, u64, u32, u32, u8, u8, u8);\n\
             pub const FABRIC_QOS: &[FabricQosRow] = &[\n{qos_rows}];\n\
             /// Generation-declared graph visibility for every participant,\n\
             /// positionally cross-checked against `FABRIC_PARTICIPANTS`.\n\
             pub const FABRIC_VISIBILITY: &[(&[u8], &str, u8)] = &[\n{visibility_rows}];\n\
             \n\
             /// Non-empty declared interposition chains. The first tuple member\n\
             /// is the downstream participant whose only path traverses `chain`.\n\
             pub type FabricInterpositionRow = (&'static [u8], &'static str, &'static [&'static [u8]]);\n\
             pub const FABRIC_INTERPOSITIONS: &[FabricInterpositionRow] = &[\n{interposition_rows}];\n\
             \n\
             \n\
             \n\
             /// Every distinct component holding a fabric control endpoint, in the\n\
             /// order init grants those endpoints.\n\
             pub const FABRIC_CLIENTS: &[&[u8]] = &[\n{client_rows}];\n\
             \n\
             /// Supervision handle slots for every declared subscriber.\n\
             pub const FABRIC_SUPERVISION: &[(&[u8], u32)] = &[\n{supervision_rows}];\n\
             \n\
             /// Every distinct subscriber, in the order init spawns them. Init\n\
             /// spawns each before the fabric so their supervision handles exist\n\
             /// when it grants them.\n\
             pub const FABRIC_SUBSCRIBERS: &[&[u8]] = &[\n{subscriber_rows}];\n\
             \n\
             /// Every distinct component holding a fabric *call*-plane control\n\
             /// endpoint, in the order init grants them. Separate from the stream\n\
             /// set because the planes are mutually exclusive profiles that each\n\
             /// grant from `FABRIC_FIRST_CONTROL_SLOT` upward.\n\
             pub const FABRIC_CALL_CLIENTS: &[&[u8]] = &[\n{call_client_rows}];\n\
             \n\
             /// Every distinct component holding a fabric *operation*-plane control\n\
             /// endpoint, in the order init grants them.\n\
             pub const FABRIC_OPERATION_CLIENTS: &[&[u8]] = &[\n{operation_client_rows}];\n\
             \n\
             /// C8.6/C8.7 bounds taken from the authenticated generation graph.\n\
             pub const FABRIC_MAX_IN_FLIGHT_CALLS: usize = {};\n\
             pub const FABRIC_MAX_IN_FLIGHT_OPERATIONS: usize = {};\n\
             pub const FABRIC_MAX_RETRIES: u8 = {};\n\
             pub const FABRIC_CALL_DEADLINE_NS: u64 = {};\n\
             pub const FABRIC_OPERATION_DEADLINE_NS: u64 = {};\n\
             pub const FABRIC_MAX_RETAINED_SAMPLES: usize = {};\n\
             pub const FABRIC_MAX_EVENT_DEPTH: usize = {};\n\
             /// The fabric's first control-endpoint slot, shared with init so the\n\
             /// two cannot disagree about the grant order.\n\
             pub const FABRIC_FIRST_CONTROL_SLOT: u32 = {FABRIC_FIRST_CONTROL_SLOT};\n",
            limits.in_flight_calls,
            limits.in_flight_operations,
            limits.retries,
            call_deadline_ns,
            operation_deadline_ns,
            limits.retained_samples,
            limits.event_depth,
        ),
    )
    .expect("write fabric profile");
}

/// The fabric's own capability layout: slot 0 is its endpoint factory, slot 1
/// its shared-buffer factory, and control endpoints start here. Declared once
/// and emitted into the generated profile so `fabric-service` and `init` read
/// the same number rather than each hard-coding it.
const FABRIC_FIRST_CONTROL_SLOT: usize = 2;

/// Which fabric plane a control-endpoint grant belongs to.
///
/// The three planes are mutually exclusive generation profiles: `init` launches
/// exactly one of them and grants its control endpoints from
/// `FABRIC_FIRST_CONTROL_SLOT` upward in the order the plane's own table lists
/// them. So the planes must be enumerated separately. Folding them into one
/// table makes a later plane's clients shift an earlier plane's slot numbers,
/// and `fabric-service::control_clients` turns those numbers into real
/// receives: the fabric then reads a slot init never granted it, which is
/// `[fabric] fail: control recv` followed by every participant failing its
/// provisioning request.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Plane {
    Stream,
    Call,
    Operation,
}

/// The plane a `fabric-*-control` grant serves, named by the grant itself.
///
/// Keyed on the grant name rather than inferred from the participant table,
/// because the two time services (`fabric-call-time`, `fabric-op-time`) sit on
/// no route at all — they drive their plane's simulated clock — and so are
/// indistinguishable from `fabric-intruder` by route membership alone.
fn control_grant_plane(grant: &str) -> Plane {
    if grant.starts_with("fabric-call-") {
        Plane::Call
    } else if grant.starts_with("fabric-op-") {
        Plane::Operation
    } else {
        Plane::Stream
    }
}

/// Every component the manifest grants a control endpoint on one plane, in
/// manifest order.
///
/// `fabric-intruder` is deliberately part of the stream plane: it holds a real
/// control endpoint and is declared on no route, which is exactly the C8.3
/// denial under test — the refusal is "no declared edge", not "no channel".
fn fabric_control_clients(manifest: &str, plane: Plane, include_replacements: bool) -> Vec<String> {
    let mut clients = Vec::new();
    let mut source = None;
    let mut grant_plane = None;
    let mut replacement = false;
    for line in manifest.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("name = \"") {
            // A new grant record begins: remember which plane it serves, and
            // drop any source carried over from the previous one. Replacement
            // controls authenticate a later respawn and are not initial slots.
            grant_plane = (rest.starts_with("fabric-") && rest.contains("-control\""))
                .then(|| control_grant_plane(rest));
            replacement = rest.contains("-restart-control\"");
            source = None;
        } else if let Some(name) = trimmed.strip_prefix("source = \"") {
            source = grant_plane
                .is_some_and(|candidate| candidate == plane)
                .then(|| name.trim_end_matches("\";").to_owned());
        } else if trimmed.starts_with("target = \"fabric-service\"")
            && let Some(client) = source.take()
            && (include_replacements || !replacement)
            && !clients.contains(&client)
        {
            clients.push(client);
        }
    }
    clients
}

/// The declared KEEP_LAST depth of each participant, in the same order
/// [`fabric_participants`] returns them.
///
/// Read as its own pass rather than folded into the participant parse: the
/// depth sits at the same indentation as every other QoS field, so keying on
/// it there would make the participant tuple depend on field order within a
/// block. Counting `historyDepth` lines inside the same block keeps the two
/// readings independent, and a mismatch in length is caught by the caller's
/// `zip` truncating — which the participant/`declared` cross-check above
/// already makes impossible to reach silently.
#[derive(Clone, Copy)]
struct FabricQos {
    deadline_ns: u64,
    lifespan_ns: u64,
    lease_ns: u64,
    history_depth: u32,
    retained_depth: u32,
    reliability: u8,
    durability: u8,
    liveliness: u8,
}

struct FabricLimits {
    retries: u8,
    in_flight_calls: usize,
    in_flight_operations: usize,
    retained_samples: usize,
    event_depth: usize,
}

fn fabric_limits(manifest: &str) -> FabricLimits {
    let block = fabric_graph_block(manifest);
    let limits = block
        .split_once("    limits = {")
        .and_then(|(_, rest)| rest.split_once("    };").map(|(limits, _)| limits))
        .expect("fabric graph limits");
    let mut retries = None;
    let mut in_flight_calls = None;
    let mut in_flight_operations = None;
    let mut retained_samples = None;
    let mut event_depth = None;
    for line in limits.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("retries = ") {
            retries = Some(value.trim_end_matches(';').parse().expect("retries"));
        } else if let Some(value) = trimmed.strip_prefix("inFlightCalls = ") {
            in_flight_calls = Some(value.trim_end_matches(';').parse().expect("inFlightCalls"));
        } else if let Some(value) = trimmed.strip_prefix("inFlightOperations = ") {
            in_flight_operations = Some(
                value
                    .trim_end_matches(';')
                    .parse()
                    .expect("inFlightOperations"),
            );
        } else if let Some(value) = trimmed.strip_prefix("retainedSamples = ") {
            retained_samples = Some(
                value
                    .trim_end_matches(';')
                    .parse()
                    .expect("retainedSamples"),
            );
        } else if let Some(value) = trimmed.strip_prefix("eventDepth = ") {
            event_depth = Some(value.trim_end_matches(';').parse().expect("eventDepth"));
        }
    }
    FabricLimits {
        retries: retries.expect("fabric graph retries"),
        in_flight_calls: in_flight_calls.expect("fabric graph inFlightCalls"),
        in_flight_operations: in_flight_operations.expect("fabric graph inFlightOperations"),
        retained_samples: retained_samples.expect("fabric graph retainedSamples"),
        event_depth: event_depth.expect("fabric graph eventDepth"),
    }
}

fn participants_call_deadline(manifest: &str, route_name: &str) -> u64 {
    let participants = fabric_participants(manifest);
    let qos = fabric_qos(manifest);
    participants
        .iter()
        .zip(qos.iter())
        .filter(|((_, route, _, direction), _)| route == route_name && matches!(*direction, 3 | 4))
        .map(|(_, entry)| entry.deadline_ns)
        .min()
        .expect("call route participant deadline")
}

fn fabric_qos(manifest: &str) -> Vec<FabricQos> {
    let mut values = Vec::new();
    let mut reliability = 0;
    let mut durability = 0;
    let mut liveliness = 0;
    let mut history_depth = 0;
    let mut retained_depth = 0;
    let mut deadline_ns = 0;
    let mut lifespan_ns = 0;
    for line in fabric_graph_block(manifest).lines() {
        let trimmed = line.trim_start();
        if line.len() - trimmed.len() != 12 {
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("reliability = ") {
            reliability = if value.starts_with("\"reliable") {
                2
            } else {
                1
            };
        } else if let Some(value) = trimmed.strip_prefix("durability = ") {
            durability = if value.starts_with("\"retained") {
                2
            } else {
                1
            };
        } else if let Some(value) = trimmed.strip_prefix("liveliness = ") {
            liveliness = if value.starts_with("\"manual") { 2 } else { 1 };
        } else if let Some(value) = trimmed.strip_prefix("historyDepth = ") {
            history_depth = value.trim_end_matches(';').parse().expect("historyDepth");
        } else if let Some(value) = trimmed.strip_prefix("retainedDepth = ") {
            retained_depth = value.trim_end_matches(';').parse().expect("retainedDepth");
        } else if let Some(value) = trimmed.strip_prefix("deadlineNs = ") {
            deadline_ns = value.trim_end_matches(';').parse().expect("deadlineNs");
        } else if let Some(value) = trimmed.strip_prefix("lifespanNs = ") {
            lifespan_ns = value.trim_end_matches(';').parse().expect("lifespanNs");
        } else if let Some(value) = trimmed.strip_prefix("leaseNs = ") {
            let lease_ns = value.trim_end_matches(';').parse().expect("leaseNs");
            values.push(FabricQos {
                deadline_ns,
                lifespan_ns,
                lease_ns,
                history_depth,
                retained_depth,
                reliability,
                durability,
                liveliness,
            });
        }
    }
    values
}

/// Read the visibility policy beside every participant without changing the
/// authority tuple parser. The caller cross-checks keys and length before
/// emitting either table, so an indentation drift stops the build.
fn fabric_visibility(manifest: &str) -> Vec<(String, String, u8)> {
    let mut rows = Vec::new();
    let mut route = String::new();
    let mut component = String::new();
    for line in fabric_graph_block(manifest).lines() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if indent == 8 && trimmed.starts_with("name = \"") {
            route = quoted(trimmed).unwrap_or_default();
        } else if indent == 12 && trimmed.starts_with("component = \"") {
            component = quoted(trimmed).unwrap_or_default();
        } else if indent == 12 && trimmed.starts_with("visibility = \"") {
            let policy = match quoted(trimmed).unwrap_or_default().as_str() {
                "private" => 1,
                "graph" => 2,
                other => panic!("unknown fabric visibility {other}"),
            };
            assert!(
                !component.is_empty() && !route.is_empty(),
                "fabric visibility is missing its component or route"
            );
            rows.push((component.clone(), route.clone(), policy));
        }
    }
    rows
}

/// Extract every non-empty interposition chain with the participant edge it
/// guards. The manifest and boot decoder already reject unknown hops, cycles,
/// and bypasses; this build-time profile is the userspace service's exact copy
/// of the authenticated declaration.
fn fabric_interpositions(manifest: &str) -> Vec<(String, String, Vec<String>)> {
    let mut rows = Vec::new();
    let mut route = String::new();
    let mut component = String::new();
    let mut chain: Option<Vec<String>> = None;
    for line in fabric_graph_block(manifest).lines() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if indent == 8 && trimmed.starts_with("name = \"") {
            route = quoted(trimmed).unwrap_or_default();
        } else if indent == 12 && trimmed.starts_with("component = \"") {
            component = quoted(trimmed).unwrap_or_default();
        } else if indent == 12 && trimmed == "interposition = [" {
            chain = Some(Vec::new());
        } else if indent == 14 && chain.is_some() && trimmed.starts_with('"') {
            chain
                .as_mut()
                .expect("interposition chain")
                .push(quoted(trimmed).expect("interposition component"));
        } else if indent == 12
            && trimmed == "];"
            && let Some(completed) = chain.take()
            && !completed.is_empty()
        {
            assert!(
                !component.is_empty() && !route.is_empty(),
                "fabric interposition is missing its participant or route"
            );
            rows.push((component.clone(), route.clone(), completed));
        }
    }
    assert!(chain.is_none(), "unterminated fabric interposition chain");
    if std::env::var("SLIME_FABRIC_VISIBILITY_CHECK").as_deref() == Ok("1") {
        let overrides = fabric_profile_interpositions(manifest, "visibility");
        assert_eq!(overrides.len(), 1, "visibility profile interposition count");
        for (component, route, replacement) in overrides {
            let matches = rows
                .iter_mut()
                .filter(|(declared_component, declared_route, _)| {
                    *declared_component == component && *declared_route == route
                })
                .collect::<Vec<_>>();
            assert_eq!(
                matches.len(),
                1,
                "profile interposition must name exactly one participant"
            );
            matches.into_iter().next().expect("profile interposition").2 = replacement;
        }
    }
    rows
}

fn fabric_profile_interpositions(
    manifest: &str,
    wanted: &str,
) -> Vec<(String, String, Vec<String>)> {
    let mut rows = Vec::new();
    let mut selected = false;
    let mut route = String::new();
    let mut component = String::new();
    for line in fabric_graph_block(manifest).lines() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if indent == 8 && trimmed.starts_with("name = \"") {
            selected = quoted(trimmed).as_deref() == Some(wanted);
        } else if selected && indent == 12 && trimmed.starts_with("route = \"") {
            route = quoted(trimmed).unwrap_or_default();
        } else if selected && indent == 12 && trimmed.starts_with("participant = \"") {
            component = quoted(trimmed).unwrap_or_default();
        } else if selected && indent == 12 && trimmed.starts_with("chain = [") {
            let chain = trimmed
                .split('"')
                .skip(1)
                .step_by(2)
                .map(str::to_owned)
                .collect::<Vec<_>>();
            assert!(
                !component.is_empty() && !route.is_empty() && !chain.is_empty(),
                "fabric profile interposition is incomplete"
            );
            rows.push((
                core::mem::take(&mut component),
                core::mem::take(&mut route),
                chain,
            ));
        }
    }
    rows
}

/// Scan the manifest's `fabricGraph` block for its declared participants.
///
/// Indentation-keyed rather than a real parser, matching the other manifest
/// readers in this file: the manifest is a fixed in-tree fixture. Because the
/// failure mode of an indentation key is a *short* table rather than an empty
/// one, the caller cross-checks the result against
/// [`declared_participant_count`], which reads the same block by a different
/// rule. Neither reading is trusted alone.
fn fabric_participants(manifest: &str) -> Vec<(String, String, String, u32)> {
    let mut participants = Vec::new();
    let mut route = String::new();
    let mut interface = String::new();
    let mut component = String::new();
    for line in fabric_graph_block(manifest).lines() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        match () {
            // Route header fields sit one level inside `routes = [ { ... } ]`.
            _ if indent == 8 && trimmed.starts_with("name = \"") => {
                route = quoted(trimmed).unwrap_or_default();
            }
            _ if indent == 8 && trimmed.starts_with("interface = \"") => {
                interface = quoted(trimmed).unwrap_or_default();
            }
            // Participant fields sit one level further in. `component` always
            // precedes `direction`, so the pair completes on `direction`.
            _ if indent == 12 && trimmed.starts_with("component = \"") => {
                component = quoted(trimmed).unwrap_or_default();
            }
            _ if indent == 12 && trimmed.starts_with("direction = \"") => {
                let direction = match quoted(trimmed).unwrap_or_default().as_str() {
                    "publish" => 1,
                    "subscribe" => 2,
                    "client" => 3,
                    "server" => 4,
                    other => panic!("unknown fabric direction {other}"),
                };
                assert!(
                    !component.is_empty() && !route.is_empty() && !interface.is_empty(),
                    "fabric participant is missing its component, route, or interface"
                );
                participants.push((
                    core::mem::take(&mut component),
                    route.clone(),
                    interface.clone(),
                    direction,
                ));
            }
            _ => {}
        }
    }
    participants
}

/// How many participants the `fabricGraph` block declares, counted without any
/// indentation assumption.
///
/// A participant is the only thing in the block that names a `direction`;
/// interposition hops name only a component. So this is a genuinely
/// independent reading of the same bytes, and it disagrees with
/// [`fabric_participants`] exactly when the structural parse dropped an entry.
fn declared_participant_count(manifest: &str) -> usize {
    fabric_graph_block(manifest)
        .lines()
        .filter(|line| line.trim_start().starts_with("direction = \""))
        .count()
}

/// The manifest's `fabricGraph` block. Panics rather than degrading: this table
/// is the fabric's entire authority set, so a manifest this cannot locate must
/// stop the build, not silently produce a graph with no declared edges.
fn fabric_graph_block(manifest: &str) -> &str {
    let start = manifest
        .find("\n  fabricGraph = {")
        .expect("generation manifest has no fabricGraph block");
    let length = manifest[start..]
        .find("\n  };")
        .expect("generation manifest fabricGraph block is unterminated");
    &manifest[start..start + length]
}

fn quoted(line: &str) -> Option<String> {
    line.split('"').nth(1).map(str::to_owned)
}

fn component_slot(manifest: &str, wanted: &str) -> Option<usize> {
    let present = manifest
        .split("    {")
        .skip(1)
        .filter(|block| field(block, "name").is_some() && field(block, "object").is_some())
        .any(|block| field(block, "name") == Some(wanted));
    if !present {
        return None;
    }
    match wanted {
        "sysinfo" => Some(1),
        "echo-agent" => Some(2),
        _ => None,
    }
}

fn component_block<'a>(manifest: &'a str, wanted: &str) -> Option<&'a str> {
    manifest
        .split("    {")
        .skip(1)
        .find(|block| field(block, "name") == Some(wanted) && field(block, "object").is_some())
}

fn field<'a>(block: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key} = \"");
    let value = block
        .lines()
        .find(|line| line.trim_start().starts_with(&prefix))?;
    value.split('"').nth(1)
}

fn field_int(block: &str, key: &str) -> Option<usize> {
    let prefix = format!("{key} = ");
    let value = block
        .lines()
        .find(|line| line.trim_start().starts_with(&prefix))?;
    value
        .trim_start()
        .strip_prefix(&prefix)?
        .trim_end_matches(';')
        .parse()
        .ok()
}

fn field_list<'a>(block: &'a str, key: &str) -> Option<Vec<&'a str>> {
    let prefix = format!("{key} = [");
    let value = block
        .lines()
        .find(|line| line.trim_start().starts_with(&prefix))?;
    Some(value.split('"').skip(1).step_by(2).collect())
}
