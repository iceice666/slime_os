#![no_std]
#![no_main]

slime_rt::entry!(main);

use slime_rt::SpawnGrant;

const RIGHT_SEND: u32 = 1;
const RIGHT_RECV: u32 = 2;
const RIGHT_BLOCK_READ: u32 = 1 << 10;
const RIGHT_BLOCK_WRITE: u32 = 1 << 11;
const RIGHT_STORE_READ: u32 = 1 << 12;
const RIGHT_STORE_WRITE: u32 = 1 << 13;
const RIGHT_HEALTH_CONFIRM: u32 = 1 << 14;
const RIGHT_BOOT_UPDATE: u32 = 1 << 15;

// Manifest-derived bootstrap slot order is emitted by the host builder.
const CONSOLE_CAPS: [SpawnGrant; 1] = [grant(2, RIGHT_RECV)];
const DANGO_CAPS: [SpawnGrant; 3] = [
    grant(4, RIGHT_RECV),
    grant(5, RIGHT_RECV),
    grant(6, RIGHT_SEND),
];
const SYSINFO_CAPS: [SpawnGrant; 1] = [grant(8, RIGHT_SEND)];
const ECHO_CAPS: [SpawnGrant; 1] = [grant(10, RIGHT_SEND)];
const STORAGE_PROBE_READ_CAPS: [SpawnGrant; 1] = [grant(12, RIGHT_BLOCK_READ)];
const STORAGE_PROBE_WRITE_CAPS: [SpawnGrant; 1] = [grant(12, RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE)];
const STORAGE_PROBE_STORE_CAPS: [SpawnGrant; 1] = [grant(12, RIGHT_STORE_READ | RIGHT_STORE_WRITE)];
const GENERATION_MANAGER_CAPS: [SpawnGrant; 1] = [grant(14, RIGHT_HEALTH_CONFIRM)];
const RECOVERY_CAPS: [SpawnGrant; 2] = [
    grant(2, RIGHT_BOOT_UPDATE),
    grant(3, RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE),
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

    spawn_or_fail(1, &CONSOLE_CAPS);
    spawn_or_fail(3, &DANGO_CAPS);
    spawn_or_fail(7, &SYSINFO_CAPS);
    spawn_or_fail(9, &ECHO_CAPS);
    spawn_or_fail(11, storage_caps());
    spawn_or_fail(13, &GENERATION_MANAGER_CAPS);
}

fn spawn_or_fail(executable_slot: u32, grants: &[SpawnGrant]) {
    if slime_rt::spawn(executable_slot, grants).is_err() {
        slime_rt::exit(1);
    }
}
