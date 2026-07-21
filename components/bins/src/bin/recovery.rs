#![no_std]
#![no_main]

slime_rt::entry!(main);

const GENERATION_CONTROL_SLOT: u32 = 0;
const REPAIR_BLOCK_SLOT: u32 = 1;
const INTERRUPT_AFTER_FIRST_SLOT: u32 = 1;

fn main() {
    let flags = if option_env!("SLIME_RECOVERY_INTERRUPT") == Some("1") {
        INTERRUPT_AFTER_FIRST_SLOT
    } else {
        0
    };
    slime_rt::debug_write(b"[recovery] scrub requested\n");
    if slime_rt::recovery_reconstruct(GENERATION_CONTROL_SLOT, REPAIR_BLOCK_SLOT, flags) < 0 {
        if flags != 0 {
            slime_rt::debug_write(b"[recovery] interrupted after first slot\n");
            return;
        }
        slime_rt::debug_write(b"[recovery] scrub rejected\n");
        slime_rt::exit(1);
    }
    slime_rt::debug_write(b"[recovery] reconstruction complete\n");
}
