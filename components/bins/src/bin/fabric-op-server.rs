#![no_std]
#![no_main]

#[path = "../fabric_operation_scenario.rs"]
mod scenario;

slime_rt::entry!(main);

fn main(_startup_arg: u32) {
    scenario::run_server();
}
