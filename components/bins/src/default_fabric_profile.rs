// @generated from the canonical C8.9 resolved fabric profile; do not edit.
#[allow(dead_code)]
mod generated_fabric_profile {
pub const FABRIC_PROFILE_NAME: &str = "default";
#[allow(dead_code)]
pub const GENERATION_BOOT_ACTION: &str = "product";
#[allow(dead_code)]
pub const FABRIC_SCHEMAS: &[(&str, &str, u64, u32, u32)] = &[
    ("ParameterCall", "8f23bd8cdf77d1ff3c62409514dbb9c2e0b66ef4707d81dbef0cb001301fb83f", 0xd7eabf1a3dd69200, 2, 40),
    ("NavigationOperation", "9b49ef2096b025e9a07bd5c2693793c833c953e370075556a575501f846cb9bd", 0x645b4bb431761df9, 3, 16),
    ("TelemetryStream", "f6e951eb0e36539002a32aff3f33df1082ea2ecc2413430f2d686f92e141ba25", 0x1164153908db137b, 1, 64),
];
#[allow(dead_code)]
pub const FABRIC_ROUTES: &[(&str, &str, &str, u32)] = &[
    ("telemetry", "TelemetryStream", "13702c6b4405defa3d1881b897f825fa8c66aa2e0dde1b32af9e450505882a0e", 1),
    ("parameters", "ParameterCall", "75bf9caaa956949e50b0ef099df2b3ee155e3dcdfa70b0cd7290135506369b44", 2),
    ("nav-backup", "NavigationOperation", "7c7c3c6671a5de402ab153fb8e5eedca498016c0c4aaa3d7e20c775a410cc448", 3),
    ("navigation", "NavigationOperation", "fef4f95b51f51190b8b30b8cc4ae06aedbce2de0589cf168c366243320258215", 3),
];
pub const FABRIC_PARTICIPANTS: &[(&[u8], &str, &str, u32)] = &[
    (b"fabric-publisher", "telemetry", "TelemetryStream", 1),
    (b"fabric-subscriber", "telemetry", "TelemetryStream", 2),
    (b"fabric-call-client", "parameters", "ParameterCall", 3),
    (b"fabric-call-server", "parameters", "ParameterCall", 4),
    (b"fabric-op-client", "nav-backup", "NavigationOperation", 3),
    (b"fabric-op-client", "navigation", "NavigationOperation", 3),
    (b"fabric-op-server", "navigation", "NavigationOperation", 4),
];
pub type FabricNotificationBindingRow = (&'static [u8], &'static str, u32, u32, u32);
pub const FABRIC_NOTIFICATION_BINDINGS: &[FabricNotificationBindingRow] = &[
    (b"fabric-publisher", "telemetry", 1, 0, 1),
    (b"fabric-subscriber", "telemetry", 2, 2, 3),
];

pub type FabricQosRow = (&'static [u8], &'static str, u64, u64, u64, u32, u32, u8, u8, u8);
pub const FABRIC_QOS: &[FabricQosRow] = &[
    (b"fabric-publisher", "telemetry", 0, 300, 0, 4, 2, 1, 2, 1),
    (b"fabric-subscriber", "telemetry", 0, 0, 0, 8, 0, 1, 1, 1),
    (b"fabric-call-client", "parameters", 1000000, 2000000, 0, 4, 0, 2, 1, 1),
    (b"fabric-call-server", "parameters", 1000000, 2000000, 5000000, 4, 0, 2, 1, 2),
    (b"fabric-op-client", "nav-backup", 1000000, 2000000, 0, 4, 0, 2, 1, 1),
    (b"fabric-op-client", "navigation", 1000000, 2000000, 0, 4, 0, 2, 1, 1),
    (b"fabric-op-server", "navigation", 1000000, 2000000, 5000000, 4, 0, 2, 1, 2),
];
pub const FABRIC_VISIBILITY: &[(&[u8], &str, u8)] = &[
    (b"fabric-publisher", "telemetry", 2),
    (b"fabric-subscriber", "telemetry", 1),
    (b"fabric-call-client", "parameters", 1),
    (b"fabric-call-server", "parameters", 1),
    (b"fabric-op-client", "nav-backup", 1),
    (b"fabric-op-client", "navigation", 1),
    (b"fabric-op-server", "navigation", 1),
];
pub type FabricWorkerRow = (&'static str, &'static [&'static str], usize);
pub const FABRIC_WORKERS: &[FabricWorkerRow] = &[
    ("stream", &["telemetry"], 3),
    ("call", &["parameters"], 7),
    ("operation", &["nav-backup", "navigation"], 9),
];
/// The wake sources the generation declares one worker parks on at once, or
/// `WORKER_ABSENT` when this graph declares no route that worker carries.
///
/// `const fn` so a broker can bind its own notification array to this number in a
/// `const _: () = assert!(..)`. The declared peak and the array that has to hold
/// it then cannot drift apart silently: a broker that grows its park set past
/// what the generation resolved stops compiling instead of overflowing at boot.
///
/// Absent is a real answer rather than a panic, because a broker is a *module*
/// of `fabric-service` and is therefore compiled into every graph, including
/// ones that declare no route for it. A stream-only graph has no call or
/// operation plane; panicking here would make such a graph fail to build over a
/// constant nothing in it ever reads. The asserts that consume this admit
/// `WORKER_ABSENT` and keep their exact check for every graph that does declare
/// the plane, so the drift they exist to catch is still caught.
#[allow(dead_code)]
pub const WORKER_ABSENT: usize = usize::MAX;
#[allow(dead_code)]
pub const fn fabric_worker_wait_sources(name: &str) -> usize {
    let mut index = 0;
    while index < FABRIC_WORKERS.len() {
        let (candidate, _, sources) = FABRIC_WORKERS[index];
        if konst_str_eq(candidate, name) {
            return sources;
        }
        index += 1;
    }
    WORKER_ABSENT
}

/// `str` equality usable in a `const fn`; `==` on `&str` is not yet const.
const fn konst_str_eq(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}
pub const FABRIC_CLIENTS: &[&[u8]] = &[
    b"fabric-publisher",
    b"fabric-subscriber",
];
pub const FABRIC_CALL_CLIENTS: &[&[u8]] = &[
    b"fabric-call-client",
    b"fabric-call-server",
    b"fabric-call-time",
];
pub const FABRIC_OPERATION_CLIENTS: &[&[u8]] = &[
    b"fabric-op-client",
    b"fabric-op-server",
    b"fabric-op-time",
];
/// How many capabilities each child's owner must hand it at spawn: its minted
/// bindings plus its non-endpoint, non-self-loop grant bindings. This is the
/// total `preflight_spawn_grants` checks a request against, so it is the one
/// number an owner must agree with. A child absent from this table is spawned
/// with nothing.
#[allow(dead_code)]
pub const FABRIC_MINTED_GRANTS: &[(&[u8], usize)] = &[
    (b"console", 0),
    (b"dango", 3),
    (b"echo-agent", 0),
    (b"fabric-call-client", 1),
    (b"fabric-call-server", 1),
    (b"fabric-call-time", 0),
    (b"fabric-call-worker", 1),
    (b"fabric-op-client", 0),
    (b"fabric-op-server", 0),
    (b"fabric-op-time", 0),
    (b"fabric-op-worker", 0),
    (b"fabric-publisher", 0),
    (b"fabric-service", 1),
    (b"fabric-subscriber", 0),
    (b"filesystem-service", 0),
    (b"generation-inspect", 0),
    (b"generation-list", 0),
    (b"generation-manager", 0),
    (b"generation-rollback", 0),
    (b"generation-select", 0),
    (b"generation-stage", 0),
    (b"init", 2),
    (b"powerbox-chooser", 2),
    (b"spawn-service", 3),
    (b"sysinfo", 0),
];
#[allow(dead_code)]
pub const FABRIC_MAX_ROUTES: usize = 8;
#[allow(dead_code)]
pub const FABRIC_MAX_INGRESS_SOURCES: usize = 9;
pub const FABRIC_MAX_PUBLISHERS: usize = 3;
pub const FABRIC_MAX_SUBSCRIBERS: usize = 4;
#[allow(dead_code)]
pub const FABRIC_MAX_CLIENTS: usize = 6;
#[allow(dead_code)]
pub const FABRIC_MAX_SERVERS: usize = 2;
pub const FABRIC_MAX_SAMPLE_BYTES: usize = 8192;
#[allow(dead_code)]
pub const FABRIC_MAX_QUEUE_DEPTH: usize = 8;
#[allow(dead_code)]
pub const FABRIC_MAX_HISTORY_DEPTH: usize = 8;
pub const FABRIC_MAX_EVENT_DEPTH: usize = 8;
pub const FABRIC_MAX_RETAINED_SAMPLES: usize = 4;
pub const FABRIC_MAX_RETRIES: u8 = 4;
pub const FABRIC_MAX_IN_FLIGHT_CALLS: usize = 4;
pub const FABRIC_MAX_IN_FLIGHT_OPERATIONS: usize = 4;
pub const FABRIC_MAX_BUFFER_PAGES: usize = 28;
pub const FABRIC_MAX_BUFFERS: usize = 14;
#[allow(dead_code)]
pub const FABRIC_MAX_MAPPINGS: usize = 14;
#[allow(dead_code)]
pub const FABRIC_MAX_LOANS: usize = 14;
pub const FABRIC_MAX_CAPABILITY_SLOTS: usize = 48;
pub const FABRIC_REQUIRED_CAPABILITY_SLOTS: usize = 28;
pub const FABRIC_FRAME_CAPACITY: usize = 32;
pub const FABRIC_COPY_PAGES: usize = 2;
/// C8.11: the declared depth of one worker's bounded semantic-trace sink, and
/// the overflow code it applies when that depth is reached. A worker sizes its
/// sink array from this constant, so the generation and the array cannot drift.
pub const FABRIC_TRACE_DEPTH: usize = 16;
pub const FABRIC_TRACE_OVERFLOW: u32 = 1;
/// No request/response route of this class exists in the resolved graph.
pub const FABRIC_DEADLINE_ABSENT: u64 = u64::MAX;
pub const FABRIC_CALL_DEADLINE_NS: u64 = 1000000;
pub const FABRIC_OPERATION_DEADLINE_NS: u64 = 1000000;
pub const FABRIC_FIRST_CONTROL_SLOT: u32 = 2;
}
#[allow(unused_imports)]
pub use generated_fabric_profile::*;
