// @generated from contracts/boot-layout/v1 by scripts/build/boot_layout.py;
// do not edit. Regenerate through `just generation_check`.

/// A slot this generation's boot layout does not declare. Using one is a
/// component asking for authority this profile never granted it.
#[allow(dead_code)]
pub const SLOT_ABSENT: u32 = u32::MAX;

/// The generation this table was emitted for.
#[allow(dead_code)]
pub const BOOT_LAYOUT_GENERATION: u64 = 1;

#[allow(dead_code)]
pub const CONSOLE_SLOT: u32 = 1;
#[allow(dead_code)]
pub const CONSOLE_OUTPUT_SLOT: u32 = 2;
#[allow(dead_code)]
pub const CROSSING_PEER_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const DANGO_SLOT: u32 = 3;
#[allow(dead_code)]
pub const DANGO_OUTPUT_SLOT: u32 = 4;
#[allow(dead_code)]
pub const DANGO_SPAWN_SLOT: u32 = 11;
#[allow(dead_code)]
pub const DIRECTORY_CLIENT_SLOT: u32 = 14;
#[allow(dead_code)]
pub const DIRECTORY_PROBE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const DIRECTORY_SERVICE_SLOT: u32 = 15;
#[allow(dead_code)]
pub const ECHO_AGENT_SLOT: u32 = 7;
#[allow(dead_code)]
pub const FABRIC_CALL_CLIENT_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_CALL_CLIENT_B_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_CALL_CLIENT_B_CONTROL_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_CALL_CLIENT_B_CONTROL_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_CALL_CLIENT_B_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_CALL_CLIENT_CONTROL_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_CALL_CLIENT_CONTROL_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_CALL_CLIENT_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_CALL_PHASE_CLIENT_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_CALL_PHASE_TIME_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_CALL_SERVER_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_CALL_SERVER_CONTROL_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_CALL_SERVER_CONTROL_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_CALL_SERVER_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_CALL_TIME_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_CALL_TIME_CONTROL_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_CALL_TIME_CONTROL_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_CALL_TIME_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_CALL_WORKER_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_INTRUDER_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_INTRUDER_CLIENT_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_INTRUDER_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_OBSERVER_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_OBSERVER_CONTROL_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_OBSERVER_CONTROL_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_OP_CLIENT_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_OP_CLIENT_B_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_OP_CLIENT_B_CONTROL_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_OP_CLIENT_B_CONTROL_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_OP_CLIENT_B_RESTART_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_OP_CLIENT_B_RESTART_CONTROL_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_OP_CLIENT_B_RESTART_CONTROL_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_OP_CLIENT_B_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_OP_CLIENT_CONTROL_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_OP_CLIENT_CONTROL_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_OP_CLIENT_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_OP_SERVER_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_OP_SERVER_CONTROL_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_OP_SERVER_CONTROL_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_OP_SERVER_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_OP_TIME_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_OP_TIME_CONTROL_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_OP_TIME_CONTROL_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_OP_TIME_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_OP_WORKER_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_PROBE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_PROBE_CONTROL_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_PROBE_CONTROL_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_PROXY_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_PROXY_CONTROL_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_PROXY_CONTROL_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_PUBLISHER_SLOT: u32 = 39;
#[allow(dead_code)]
pub const FABRIC_PUBLISHER_B_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_PUBLISHER_B_CLIENT_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_PUBLISHER_B_CONTROL_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_PUBLISHER_B_CONTROL_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_PUBLISHER_B_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_PUBLISHER_CLIENT_SLOT: u32 = 41;
#[allow(dead_code)]
pub const FABRIC_PUBLISHER_CONTROL_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_PUBLISHER_CONTROL_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_PUBLISHER_SERVICE_SLOT: u32 = 43;
#[allow(dead_code)]
pub const FABRIC_SERVICE_SLOT: u32 = 38;
#[allow(dead_code)]
pub const FABRIC_SUBSCRIBER_SLOT: u32 = 40;
#[allow(dead_code)]
pub const FABRIC_SUBSCRIBER_B_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_SUBSCRIBER_B_CLIENT_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_SUBSCRIBER_B_CONTROL_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_SUBSCRIBER_B_CONTROL_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_SUBSCRIBER_B_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_SUBSCRIBER_CLIENT_SLOT: u32 = 42;
#[allow(dead_code)]
pub const FABRIC_SUBSCRIBER_CONTROL_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_SUBSCRIBER_CONTROL_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_SUBSCRIBER_SERVICE_SLOT: u32 = 44;
#[allow(dead_code)]
pub const FABRIC_TIME_CLIENT_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FABRIC_TIME_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const FILESYSTEM_SERVICE_SLOT: u32 = 13;
#[allow(dead_code)]
pub const GENERATION_INSPECT_SLOT: u32 = 20;
#[allow(dead_code)]
pub const GENERATION_INSPECT_CLIENT_SLOT: u32 = 25;
#[allow(dead_code)]
pub const GENERATION_INSPECT_SERVICE_SLOT: u32 = 30;
#[allow(dead_code)]
pub const GENERATION_LIST_SLOT: u32 = 19;
#[allow(dead_code)]
pub const GENERATION_LIST_CLIENT_SLOT: u32 = 24;
#[allow(dead_code)]
pub const GENERATION_LIST_SERVICE_SLOT: u32 = 29;
#[allow(dead_code)]
pub const GENERATION_MANAGER_SLOT: u32 = 9;
#[allow(dead_code)]
pub const GENERATION_ROLLBACK_SLOT: u32 = 23;
#[allow(dead_code)]
pub const GENERATION_ROLLBACK_CLIENT_SLOT: u32 = 28;
#[allow(dead_code)]
pub const GENERATION_ROLLBACK_SERVICE_SLOT: u32 = 33;
#[allow(dead_code)]
pub const GENERATION_SELECT_SLOT: u32 = 22;
#[allow(dead_code)]
pub const GENERATION_SELECT_CLIENT_SLOT: u32 = 27;
#[allow(dead_code)]
pub const GENERATION_SELECT_SERVICE_SLOT: u32 = 32;
#[allow(dead_code)]
pub const GENERATION_STAGE_SLOT: u32 = 21;
#[allow(dead_code)]
pub const GENERATION_STAGE_CLIENT_SLOT: u32 = 26;
#[allow(dead_code)]
pub const GENERATION_STAGE_SERVICE_SLOT: u32 = 31;
#[allow(dead_code)]
pub const POWERBOX_CHOOSER_SLOT: u32 = 34;
#[allow(dead_code)]
pub const POWERBOX_CLIENT_SLOT: u32 = 35;
#[allow(dead_code)]
pub const POWERBOX_PROBE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const POWERBOX_SERVICE_SLOT: u32 = 36;
#[allow(dead_code)]
pub const RECLAMATION_FAULT_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const SAMPLE_LENDER_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const SAMPLE_LENDER_SIDE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const SAMPLE_RECEIVER_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const SAMPLE_RECEIVER_SIDE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const SEL4_DIRECTORY_PROBE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const SEL4_FILESYSTEM_SERVICE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const SEL4_GENERATION_CLIENT_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const SEL4_GENERATION_MANAGER_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const SEL4_INPUT_PROBE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const SEL4_RECOVERY_PROBE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const SEL4_ROLLBACK_PROBE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const SEL4_STORAGE_PROBE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const SEL4_STORE_PROBE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const SEL4_TRANSFER_PROBE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const SERVICE_SPAWN_SLOT: u32 = 12;
#[allow(dead_code)]
pub const SPAWN_SERVICE_SLOT: u32 = 5;
#[allow(dead_code)]
pub const SPAWN_SERVICE_RPC_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const STORAGE_FAULT_PROBE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const STORAGE_PROBE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const STORAGE_STORE_PROBE_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const STORAGE_WRITER_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const SUPERVISION_CHILD_SLOT: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const SYSINFO_SLOT: u32 = 6;

#[allow(dead_code)]
pub const DIRECTORY_SLOT: u32 = 17;
#[allow(dead_code)]
pub const DIRECTORY_SLOT_0: u32 = 17;
#[allow(dead_code)]
pub const DIRECTORY_SLOT_1: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const ENDPOINT_FACTORY_SLOT: u32 = 0;
#[allow(dead_code)]
pub const ENDPOINT_FACTORY_SLOT_0: u32 = 0;
#[allow(dead_code)]
pub const ENDPOINT_FACTORY_SLOT_1: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const GENERATION_CONTROL_SLOT: u32 = 10;
#[allow(dead_code)]
pub const GENERATION_CONTROL_SLOT_0: u32 = 10;
#[allow(dead_code)]
pub const GENERATION_CONTROL_SLOT_1: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const INPUT_SLOT: u32 = 18;
#[allow(dead_code)]
pub const INPUT_SLOT_0: u32 = 18;
#[allow(dead_code)]
pub const INPUT_SLOT_1: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const OBJECT_STORE_SLOT: u32 = 16;
#[allow(dead_code)]
pub const OBJECT_STORE_SLOT_0: u32 = 16;
#[allow(dead_code)]
pub const OBJECT_STORE_SLOT_1: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const SHARED_BUFFER_FACTORY_SLOT: u32 = 37;
#[allow(dead_code)]
pub const SHARED_BUFFER_FACTORY_SLOT_0: u32 = 37;
#[allow(dead_code)]
pub const SHARED_BUFFER_FACTORY_SLOT_1: u32 = SLOT_ABSENT;
#[allow(dead_code)]
pub const STORAGE_CAPABILITY_SLOT: u32 = 8;
#[allow(dead_code)]
pub const STORAGE_CAPABILITY_SLOT_0: u32 = 8;
#[allow(dead_code)]
pub const STORAGE_CAPABILITY_SLOT_1: u32 = SLOT_ABSENT;
