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
const GENERATION_MANAGER_CAPS: [SpawnGrant; 6] = [
    grant(31, RIGHT_SEND | RIGHT_RECV),
    grant(11, RIGHT_HEALTH_CONFIRM | RIGHT_BOOT_UPDATE),
    grant(32, RIGHT_SEND | RIGHT_RECV),
    grant(33, RIGHT_SEND | RIGHT_RECV),
    grant(34, RIGHT_SEND | RIGHT_RECV),
    grant(35, RIGHT_SEND | RIGHT_RECV),
];
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

const GENERATION_LIST_CAPS: [SpawnGrant; 1] = [grant(26, RIGHT_SEND | RIGHT_RECV)];
const GENERATION_INSPECT_CAPS: [SpawnGrant; 1] = [grant(27, RIGHT_SEND | RIGHT_RECV)];
const GENERATION_STAGE_CAPS: [SpawnGrant; 1] = [grant(28, RIGHT_SEND | RIGHT_RECV)];
const GENERATION_SELECT_CAPS: [SpawnGrant; 1] = [grant(29, RIGHT_SEND | RIGHT_RECV)];
const GENERATION_ROLLBACK_CAPS: [SpawnGrant; 1] = [grant(30, RIGHT_SEND | RIGHT_RECV)];

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
    if option_env!("SLIME_GENERATION_CMD_CHECK") != Some("1") {
        spawn_or_fail(1, &CONSOLE_CAPS);
        spawn_or_fail(3, &dango_caps());
        spawn_or_fail(5, &spawn_service_caps());
        if option_env!("SLIME_DANGO_CHECK") != Some("1") {
            spawn_or_fail(8, storage_caps());
        }
    }
    if option_env!("SLIME_GENERATION_CMD_CHECK") != Some("1") {
        spawn_or_fail(10, &GENERATION_MANAGER_CAPS);
    }
    if option_env!("SLIME_GENERATION_CMD_CHECK") == Some("1") {
        let negative_scenario = matches!(
            option_env!("SLIME_GENERATION_CMD_SCENARIO"),
            Some("bad-closure" | "bad-release")
        );
        spawn_or_fail(10, &GENERATION_MANAGER_CAPS);
        spawn_and_wait(21, &GENERATION_LIST_CAPS);
        if !negative_scenario {
            spawn_and_wait(22, &GENERATION_INSPECT_CAPS);
        }
        spawn_and_wait(23, &GENERATION_STAGE_CAPS);
        if negative_scenario {
            slime_rt::debug_write(b"[init] negative generation scenario complete\n");
            slime_rt::exit(0);
        }
        spawn_and_wait(24, &GENERATION_SELECT_CAPS);
        spawn_and_wait(25, &GENERATION_ROLLBACK_CAPS);
    }
    slime_rt::debug_write(b"[init] spawn graph launched\n");
    slime_rt::exit(0);
}
fn spawn_or_fail(executable_slot: u32, grants: &[SpawnGrant]) {
    let spawned = slime_rt::spawn(executable_slot, grants).unwrap_or_else(|error| {
        slime_rt::debug_write(b"[init] spawn failed slot=");
        write_u32(executable_slot);
        slime_rt::debug_write(b" error=");
        write_i64(error);
        slime_rt::debug_write(b"\n");
        slime_rt::exit(1)
    });
    if slime_rt::cap_drop(spawned.supervision_slot) < 0 {
        slime_rt::exit(1);
    }
}

fn spawn_and_wait(executable_slot: u32, grants: &[SpawnGrant]) {
    let spawned = slime_rt::spawn(executable_slot, grants).unwrap_or_else(|error| {
        slime_rt::debug_write(b"[init] spawn failed slot=");
        write_u32(executable_slot);
        slime_rt::debug_write(b" error=");
        write_i64(error);
        slime_rt::debug_write(b"\n");
        slime_rt::exit(1)
    });
    loop {
        match slime_rt::supervision_status(spawned.supervision_slot) {
            Ok(None) => slime_rt::yield_now(),
            Ok(Some(slime_rt::Termination::Exit(0))) => return,
            _ => slime_rt::exit(1),
        }
    }
}

fn write_i64(value: i64) {
    if value < 0 {
        slime_rt::debug_write(b"-");
        write_u32(value.unsigned_abs() as u32);
    } else {
        write_u32(value as u32);
    }
}

fn write_u32(mut value: u32) {
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
