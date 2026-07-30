// @generated from the canonical C8.9 resolved fabric profile; do not edit.
#[allow(dead_code)]
pub const FABRIC_PROFILE_NAME: &str = "default";
#[allow(dead_code)]
pub const FABRIC_SCHEMAS: &[(&str, &str, u64, u32, u32)] = &[
    ("ParameterCall", "8f23bd8cdf77d1ff3c62409514dbb9c2e0b66ef4707d81dbef0cb001301fb83f", 0xd7eabf1a3dd69200, 2, 40),
    ("DiagnosticsStream", "96db0f1542edcd4d80a82ef27f1fe6b8e6fcc35af4c7f27030aabfebbb811294", 0xc5508e6fa99ba2bc, 1, 28),
    ("NavigationOperation", "9b49ef2096b025e9a07bd5c2693793c833c953e370075556a575501f846cb9bd", 0x645b4bb431761df9, 3, 16),
    ("TelemetryStream", "f6e951eb0e36539002a32aff3f33df1082ea2ecc2413430f2d686f92e141ba25", 0x1164153908db137b, 1, 64),
];
#[allow(dead_code)]
pub const FABRIC_ROUTES: &[(&str, &str, &str, u32)] = &[
    ("telemetry", "TelemetryStream", "13702c6b4405defa3d1881b897f825fa8c66aa2e0dde1b32af9e450505882a0e", 1),
    ("parameters", "ParameterCall", "75bf9caaa956949e50b0ef099df2b3ee155e3dcdfa70b0cd7290135506369b44", 2),
    ("nav-backup", "NavigationOperation", "7c7c3c6671a5de402ab153fb8e5eedca498016c0c4aaa3d7e20c775a410cc448", 3),
    ("diagnostics", "DiagnosticsStream", "cfa46cba9393af8dab0587b0eb77118bcf840102d2fce2548abbc1c277c8f5c5", 1),
    ("navigation", "NavigationOperation", "fef4f95b51f51190b8b30b8cc4ae06aedbce2de0589cf168c366243320258215", 3),
];
pub const FABRIC_PARTICIPANTS: &[(&[u8], &str, &str, u32)] = &[
    (b"fabric-publisher", "telemetry", "TelemetryStream", 1),
    (b"fabric-subscriber", "telemetry", "TelemetryStream", 2),
    (b"fabric-publisher-b", "telemetry", "TelemetryStream", 1),
    (b"fabric-subscriber-b", "telemetry", "TelemetryStream", 2),
    (b"fabric-call-client", "parameters", "ParameterCall", 3),
    (b"fabric-call-client-b", "parameters", "ParameterCall", 3),
    (b"fabric-call-server", "parameters", "ParameterCall", 4),
    (b"fabric-op-client", "nav-backup", "NavigationOperation", 3),
    (b"fabric-publisher-b", "diagnostics", "DiagnosticsStream", 1),
    (b"fabric-subscriber-b", "diagnostics", "DiagnosticsStream", 2),
    (b"fabric-op-client", "navigation", "NavigationOperation", 3),
    (b"fabric-op-client-b", "navigation", "NavigationOperation", 3),
    (b"fabric-op-client-b-restart", "navigation", "NavigationOperation", 3),
    (b"fabric-op-server", "navigation", "NavigationOperation", 4),
];
pub const FABRIC_HISTORY_DEPTHS: &[(&[u8], &str, u32)] = &[
    (b"fabric-publisher", "telemetry", 4),
    (b"fabric-subscriber", "telemetry", 8),
    (b"fabric-publisher-b", "telemetry", 4),
    (b"fabric-subscriber-b", "telemetry", 4),
    (b"fabric-call-client", "parameters", 4),
    (b"fabric-call-client-b", "parameters", 4),
    (b"fabric-call-server", "parameters", 4),
    (b"fabric-op-client", "nav-backup", 4),
    (b"fabric-publisher-b", "diagnostics", 2),
    (b"fabric-subscriber-b", "diagnostics", 2),
    (b"fabric-op-client", "navigation", 4),
    (b"fabric-op-client-b", "navigation", 4),
    (b"fabric-op-client-b-restart", "navigation", 4),
    (b"fabric-op-server", "navigation", 4),
];
pub type FabricQosRow = (&'static [u8], &'static str, u64, u64, u64, u32, u32, u8, u8, u8);
pub const FABRIC_QOS: &[FabricQosRow] = &[
    (b"fabric-publisher", "telemetry", 0, 300, 0, 4, 2, 1, 2, 1),
    (b"fabric-subscriber", "telemetry", 0, 0, 0, 8, 0, 1, 1, 1),
    (b"fabric-publisher-b", "telemetry", 100, 300, 200, 4, 2, 2, 2, 2),
    (b"fabric-subscriber-b", "telemetry", 0, 0, 0, 4, 0, 1, 1, 1),
    (b"fabric-call-client", "parameters", 1000000, 2000000, 0, 4, 0, 2, 1, 1),
    (b"fabric-call-client-b", "parameters", 1000000, 2000000, 0, 4, 0, 2, 1, 1),
    (b"fabric-call-server", "parameters", 1000000, 2000000, 5000000, 4, 0, 2, 1, 2),
    (b"fabric-op-client", "nav-backup", 1000000, 2000000, 0, 4, 0, 2, 1, 1),
    (b"fabric-publisher-b", "diagnostics", 100, 0, 200, 2, 0, 2, 1, 2),
    (b"fabric-subscriber-b", "diagnostics", 100, 0, 200, 2, 0, 2, 1, 2),
    (b"fabric-op-client", "navigation", 1000000, 2000000, 0, 4, 0, 2, 1, 1),
    (b"fabric-op-client-b", "navigation", 1000000, 2000000, 0, 4, 0, 2, 1, 1),
    (b"fabric-op-client-b-restart", "navigation", 1000000, 2000000, 0, 4, 0, 2, 1, 1),
    (b"fabric-op-server", "navigation", 1000000, 2000000, 5000000, 4, 0, 2, 1, 2),
];
pub const FABRIC_VISIBILITY: &[(&[u8], &str, u8)] = &[
    (b"fabric-publisher", "telemetry", 2),
    (b"fabric-subscriber", "telemetry", 1),
    (b"fabric-publisher-b", "telemetry", 2),
    (b"fabric-subscriber-b", "telemetry", 2),
    (b"fabric-call-client", "parameters", 1),
    (b"fabric-call-client-b", "parameters", 1),
    (b"fabric-call-server", "parameters", 1),
    (b"fabric-op-client", "nav-backup", 1),
    (b"fabric-publisher-b", "diagnostics", 2),
    (b"fabric-subscriber-b", "diagnostics", 2),
    (b"fabric-op-client", "navigation", 1),
    (b"fabric-op-client-b", "navigation", 1),
    (b"fabric-op-client-b-restart", "navigation", 1),
    (b"fabric-op-server", "navigation", 1),
];
pub type FabricInterpositionRow = (&'static [u8], &'static str, &'static [&'static [u8]]);
pub const FABRIC_INTERPOSITIONS: &[FabricInterpositionRow] = &[
    (b"fabric-subscriber", "telemetry", &[b"fabric-service" as &[u8]]),
];
pub const FABRIC_CLIENTS: &[&[u8]] = &[
    b"fabric-publisher",
    b"fabric-subscriber",
    b"fabric-intruder",
    b"fabric-publisher-b",
    b"fabric-subscriber-b",
];
pub const FABRIC_CALL_CLIENTS: &[&[u8]] = &[
    b"fabric-call-client",
    b"fabric-call-client-b",
    b"fabric-call-server",
    b"fabric-call-time",
];
pub const FABRIC_OPERATION_CLIENTS: &[&[u8]] = &[
    b"fabric-op-client",
    b"fabric-op-client-b",
    b"fabric-op-server",
    b"fabric-op-time",
];
pub const FABRIC_SUPERVISION: &[(&[u8], u32)] = &[
    (b"fabric-subscriber", 7),
    (b"fabric-subscriber-b", 8),
];
pub const FABRIC_SUBSCRIBERS: &[&[u8]] = &[
    b"fabric-subscriber",
    b"fabric-subscriber-b",
];
#[allow(dead_code)]
pub const FABRIC_MAX_ROUTES: usize = 8;
#[allow(dead_code)]
pub const FABRIC_MAX_INGRESS_SOURCES: usize = 9;
pub const FABRIC_MAX_PUBLISHERS: usize = 3;
pub const FABRIC_MAX_SUBSCRIBERS: usize = 3;
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
pub const FABRIC_CALL_DEADLINE_NS: u64 = 1000000;
pub const FABRIC_OPERATION_DEADLINE_NS: u64 = 1000000;
pub const FABRIC_FIRST_CONTROL_SLOT: u32 = 2;
