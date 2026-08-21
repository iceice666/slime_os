// @generated from the canonical C8.9 resolved fabric profile; do not edit.
#[allow(dead_code)]
mod generated_fabric_profile {
#[allow(dead_code)]
pub const GENERATION_BOOT_ACTION: &str = "product";
#[allow(dead_code)]
pub const FABRIC_SCHEMAS: &[(&str, &str, u64, u32, u32)] = &[
    ("ParameterCall", "8f23bd8cdf77d1ff3c62409514dbb9c2e0b66ef4707d81dbef0cb001301fb83f", 0xd7eabf1a3dd69200, 2, 40),
    ("NavigationOperation", "9b49ef2096b025e9a07bd5c2693793c833c953e370075556a575501f846cb9bd", 0x645b4bb431761df9, 3, 16),
    ("TelemetryStream", "f6e951eb0e36539002a32aff3f33df1082ea2ecc2413430f2d686f92e141ba25", 0x1164153908db137b, 1, 64),
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
pub const FABRIC_MAX_PUBLISHERS: usize = 3;
pub const FABRIC_MAX_SUBSCRIBERS: usize = 4;
pub const FABRIC_MAX_SAMPLE_BYTES: usize = 8192;
pub const FABRIC_MAX_EVENT_DEPTH: usize = 8;
pub const FABRIC_MAX_RETAINED_SAMPLES: usize = 4;
pub const FABRIC_MAX_RETRIES: u8 = 4;
pub const FABRIC_MAX_IN_FLIGHT_CALLS: usize = 4;
pub const FABRIC_MAX_IN_FLIGHT_OPERATIONS: usize = 4;
pub const FABRIC_MAX_BUFFER_PAGES: usize = 28;
pub const FABRIC_MAX_BUFFERS: usize = 14;
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
pub const FABRIC_CALL_DEADLINE_NS: u64 = 1000000;
pub const FABRIC_OPERATION_DEADLINE_NS: u64 = 1000000;
pub const FABRIC_FIRST_CONTROL_SLOT: u32 = 2;
}
#[allow(unused_imports)]
pub use generated_fabric_profile::*;
