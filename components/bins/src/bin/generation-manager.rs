#![no_std]
#![no_main]

slime_rt::entry!(main);

const GENERATION_CONTROL_SLOT: u32 = 0;
const FAILING_PENDING_GENERATION: u64 = 99;
const KNOWN_GOOD_GENERATION: u64 = 1;

fn main() {
    let generation = option_env!("SLIME_GENERATION_NUMBER")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(KNOWN_GOOD_GENERATION);
    if generation == KNOWN_GOOD_GENERATION {
        slime_rt::debug_write(b"[generation-manager] known-good generation active\n");
        return;
    }
    if generation == FAILING_PENDING_GENERATION {
        slime_rt::debug_write(b"[generation-manager] explicit unhealthy status\n");
        slime_rt::unhealthy();
    }
    slime_rt::debug_write(b"[generation-manager] confirming pending generation\n");
    if slime_rt::health_confirm(GENERATION_CONTROL_SLOT) < 0 {
        slime_rt::debug_write(b"[generation-manager] confirmation rejected\n");
        slime_rt::exit(1);
    }
    slime_rt::debug_write(b"[generation-manager] pending generation confirmed\n");
}
