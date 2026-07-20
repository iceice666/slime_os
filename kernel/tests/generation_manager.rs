#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(slime_os_kernel::test_runner)]
#![reexport_test_harness_main = "test_main"]

//! M5.6 generation-management policy and GC-reachability behavior.
//!
//! These run against the live boot generation module so the state-policy
//! plan and garbage-collection root set are exercised with real, decoded
//! manifest data rather than hand-built fixtures:
//! - `state_plan` maps each of the five declared state policies to its
//!   upgrade action, and flips `discardOnRollback` only on a rollback;
//! - `collect_unreachable` keeps every object reachable from a retained
//!   root (here the running generation identity) and returns only orphans.

extern crate alloc;

use slime_os_kernel::generation;
use slime_os_kernel::generation_manager::{self, StateAction};
use slime_os_kernel::{boot, gdt, interrupts, memory};

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    slime_os_kernel::limine::ensure_linked();
    unsafe { boot::init_from_limine() };
    gdt::init();
    interrupts::init();
    memory::init();
    test_main();
    slime_os_kernel::hlt_loop()
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    slime_os_kernel::test_panic_handler(info)
}

fn action_for<'a>(
    plan: &'a [generation_manager::StatePlan<'a>],
    name: &str,
) -> Option<StateAction> {
    plan.iter()
        .find(|entry| entry.name == name)
        .map(|entry| entry.action)
}

#[test_case]
fn state_plan_maps_each_policy_to_its_action() {
    let bytes = boot::generation();
    let decoded = generation::decode(bytes).expect("generation decodes");

    let upgrade = generation_manager::state_plan(&decoded, false);
    assert_eq!(
        action_for(&upgrade, "dango-history"),
        Some(StateAction::Reuse)
    );
    assert_eq!(
        action_for(&upgrade, "echo-agent-session"),
        Some(StateAction::CreateEmpty)
    );
    assert_eq!(
        action_for(&upgrade, "immutable-config"),
        Some(StateAction::Reuse)
    );
    assert_eq!(
        action_for(&upgrade, "upgrade-snapshot"),
        Some(StateAction::Snapshot)
    );
    // discardOnRollback keeps its state on a forward upgrade.
    assert_eq!(
        action_for(&upgrade, "rollback-cache"),
        Some(StateAction::Reuse)
    );

    // ...and discards it only when the transition is a rollback.
    let rollback = generation_manager::state_plan(&decoded, true);
    assert_eq!(
        action_for(&rollback, "rollback-cache"),
        Some(StateAction::DiscardOnRollback)
    );
}

#[test_case]
fn collect_keeps_reachable_and_returns_only_orphans() {
    generation_manager::init();
    let reachable = boot::generation_identity();
    let orphan = [0xaa; 32];
    let unreachable = generation_manager::collect_unreachable(&[reachable, orphan]);
    assert!(unreachable.contains(&orphan));
    assert!(!unreachable.contains(&reachable));
}
