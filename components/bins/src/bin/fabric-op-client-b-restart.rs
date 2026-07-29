#![no_std]
#![no_main]

#[path = "../fabric_operation_scenario.rs"]
mod scenario;

slime_rt::entry!(main);

fn main() {
    scenario::run_client_b_restarted();
}
