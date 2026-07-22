#![no_std]
#![no_main]

slime_rt::entry!(main);

use slime_rt::SpawnGrant;

const RIGHT_SEND: u32 = 1;
const RIGHT_RECV: u32 = 2;
const RIGHT_TRANSFER: u32 = 4;
const RIGHT_BLOCK_READ: u32 = 1 << 10;
const RIGHT_BLOCK_WRITE: u32 = 1 << 11;
const RIGHT_STORE_READ: u32 = 1 << 12;
const RIGHT_STORE_WRITE: u32 = 1 << 13;
const RIGHT_HEALTH_CONFIRM: u32 = 1 << 14;
const RIGHT_BOOT_UPDATE: u32 = 1 << 15;
const RIGHT_EXEC: u32 = 1 << 3;
const RIGHT_SPAWN: u32 = 1 << 16;
const RIGHT_ENDPOINT_CREATE: u32 = 1 << 17;
const RIGHT_DIRECTORY_READ: u32 = 1 << 19;
const RIGHT_DIRECTORY_WRITE: u32 = 1 << 20;
const RIGHT_DIRECTORY_LIST: u32 = 1 << 21;
const RIGHT_DIRECTORY_DERIVE: u32 = 1 << 22;
const RIGHT_INPUT_READ: u32 = 1 << 23;

// Manifest-derived bootstrap slot order is emitted by the host builder.
const CONSOLE_CAPS: [SpawnGrant; 1] = [grant(2, RIGHT_RECV)];
const STORAGE_PROBE_READ_CAPS: [SpawnGrant; 1] = [grant(9, RIGHT_BLOCK_READ)];
const STORAGE_PROBE_WRITE_CAPS: [SpawnGrant; 1] = [grant(9, RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE)];
const STORAGE_PROBE_STORE_CAPS: [SpawnGrant; 1] = [grant(9, RIGHT_STORE_READ | RIGHT_STORE_WRITE)];
const GENERATION_MANAGER_CAPS: [SpawnGrant; 1] = [grant(11, RIGHT_HEALTH_CONFIRM)];
const RECOVERY_CAPS: [SpawnGrant; 2] = [
    grant(2, RIGHT_BOOT_UPDATE),
    grant(3, RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE),
];

fn dango_caps() -> [SpawnGrant; 5] {
    [
        grant(12, RIGHT_SEND | RIGHT_RECV),
        grant(4, RIGHT_SEND | RIGHT_TRANSFER),
        grant(20, RIGHT_INPUT_READ),
        grant(
            19,
            RIGHT_DIRECTORY_READ | RIGHT_DIRECTORY_DERIVE | RIGHT_TRANSFER,
        ),
        grant(0, RIGHT_ENDPOINT_CREATE),
    ]
}

fn spawn_service_caps() -> [SpawnGrant; 4] {
    [
        grant(13, RIGHT_SEND | RIGHT_RECV),
        grant(6, RIGHT_EXEC | RIGHT_SPAWN),
        grant(7, RIGHT_EXEC | RIGHT_SPAWN),
        grant(0, RIGHT_ENDPOINT_CREATE),
    ]
}

fn filesystem_caps() -> [SpawnGrant; 2] {
    [
        grant(17, RIGHT_SEND | RIGHT_RECV),
        grant(18, RIGHT_STORE_READ | RIGHT_STORE_WRITE),
    ]
}

const DIRECTORY_PROBE_CAPS: [SpawnGrant; 2] = [
    grant(16, RIGHT_SEND | RIGHT_RECV),
    grant(
        19,
        RIGHT_TRANSFER
            | RIGHT_DIRECTORY_READ
            | RIGHT_DIRECTORY_WRITE
            | RIGHT_DIRECTORY_LIST
            | RIGHT_DIRECTORY_DERIVE,
    ),
];

const fn grant(slot: u32, rights: u32) -> SpawnGrant {
    SpawnGrant { slot, rights }
}

fn storage_caps() -> &'static [SpawnGrant] {
    match option_env!("SLIME_GENERATION_NUMBER") {
        Some("2") | Some("3") => &STORAGE_PROBE_WRITE_CAPS,
        Some("4") => &STORAGE_PROBE_STORE_CAPS,
        _ => &STORAGE_PROBE_READ_CAPS,
    }
}

fn main() {
    if option_env!("SLIME_RECOVERY_IMAGE") == Some("1") {
        slime_rt::debug_write(b"[init] launching recovery graph\n");
        spawn_or_fail(1, &RECOVERY_CAPS);
        return;
    }
    slime_rt::debug_write(b"[init] launching component graph\n");

    if matches!(option_env!("SLIME_GENERATION_NUMBER"), Some("6" | "7")) {
        spawn_or_fail(14, &filesystem_caps());
        spawn_or_fail(15, &DIRECTORY_PROBE_CAPS);
    }
    spawn_or_fail(1, &CONSOLE_CAPS);
    spawn_or_fail(3, &dango_caps());
    spawn_or_fail(5, &spawn_service_caps());
    if option_env!("SLIME_DANGO_CHECK") != Some("1") {
        spawn_or_fail(8, storage_caps());
    }
    spawn_or_fail(10, &GENERATION_MANAGER_CAPS);
    slime_rt::debug_write(b"[init] spawn graph launched\n");
    slime_rt::exit(0);
}

fn spawn_or_fail(executable_slot: u32, grants: &[SpawnGrant]) {
    if slime_rt::spawn(executable_slot, grants).is_err() {
        slime_rt::exit(1);
    }
}
