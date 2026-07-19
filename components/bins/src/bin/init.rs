#![no_std]
#![no_main]

slime_rt::entry!(main);

// Capability-slot grants per spawned component, and the name-id each is
// recorded under (see `component_name_from_id` in kernel/src/syscall/mod.rs:
// 1=console, 2=dango, 3=sysinfo, 4=echo-agent, 5=storage-probe). Order and
// values are generation-manifest-defined and must match exactly.
const CONSOLE_CAPS: [u32; 1] = [1];
const DANGO_CAPS: [u32; 3] = [3, 4, 5];
const SYSINFO_CAPS: [u32; 1] = [7];
const ECHO_CAPS: [u32; 1] = [9];
const STORAGE_PROBE_CAPS: [u32; 1] = [11];

fn main() {
    slime_rt::debug_write(b"[init] launching component graph\n");

    spawn_or_fail(0, &CONSOLE_CAPS, 1);
    spawn_or_fail(2, &DANGO_CAPS, 2);
    spawn_or_fail(6, &SYSINFO_CAPS, 3);
    spawn_or_fail(8, &ECHO_CAPS, 4);
    spawn_or_fail(10, &STORAGE_PROBE_CAPS, 5);
}

fn spawn_or_fail(executable_slot: u32, cap_slots: &[u32], name_id: u64) {
    if slime_rt::spawn(executable_slot, cap_slots, name_id) < 0 {
        slime_rt::exit(1);
    }
}
