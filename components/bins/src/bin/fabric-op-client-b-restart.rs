#![no_std]
#![no_main]

#[path = "../fabric_operation_scenario.rs"]
mod scenario;

slime_rt::entry!(main);

fn main() {
    if slime_components::fabric_boot::active() {
        // The replacement exists so the operation worker has a channel to park
        // on while client B's slot is vacant — that source is part of the
        // worker's declared peak of 9. In the boot graph client B never leaves,
        // so the replacement holds its declared control endpoint and parks
        // without requesting a role.
        slime_components::fabric_boot::park_only(b"fabric-op-client-b-restart");
    }
    scenario::run_client_b_restarted();
}
