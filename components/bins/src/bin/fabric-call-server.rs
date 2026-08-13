#![no_std]
#![no_main]

#[path = "../fabric_call_scenario.rs"]
mod scenario;

slime_rt::entry!(main);

fn main(_startup_arg: u32) {
    // The boot plane declares this component but gives it no work, and the
    // discriminator is the build profile rather than a startup argument: the
    // root delivers a nonzero action only to the bootstrap instance, so every
    // participant on every plane read zero and parked.
    if slime_components::fabric_boot::active() {
        slime_components::fabric_boot::park_only(b"fabric-call-server");
    }
    scenario::run_server();
}
