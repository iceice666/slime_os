#![no_std]
#![no_main]

#[path = "../fabric_call_scenario.rs"]
mod scenario;

slime_rt::entry!(main);

fn main(startup_arg: u32) {
    if startup_arg == 0 {
        slime_components::fabric_boot::park_only(b"fabric-call-client-b");
    }
    scenario::run_client_b();
}
