#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(slime_os_kernel::test_runner)]
#![reexport_test_harness_main = "test_main"]

//! C8.2 fabric-graph generation admission invariants.
//!
//! Exercises the exit condition from `roadmap/02-core-runtime.md` (C8.2): one
//! authenticated generation resource deterministically fixes every native
//! interface, graph edge, direction, QoS policy, visibility grant,
//! interposition hop, and resource ceiling; malformed, unauthorized, or
//! globally impossible graphs fail before component launch.
//!
//! The structural corpus (malformed tables, forged grants, QoS incoherence,
//! interposition cycles, impossible aggregates) lives in the `boot-contracts`
//! lib tests, which run the same decoder without a boot. This file is the
//! live-path arm: it proves the graph a real boot admitted came from the
//! generation resource object rather than from a test fixture, and that the
//! kernel's own ceilings are the ones it was validated against.

extern crate alloc;
use boot_contracts::fabric_graph::{
    CONTRACT_KIND_CALL, CONTRACT_KIND_STREAM, DIRECTION_CLIENT, DIRECTION_PUBLISH,
    DIRECTION_SERVER, DIRECTION_SUBSCRIBE, INTERPOSITION_NONE, component_identity, grant_identity,
};
use slime_os_kernel::capability::MAX_CAPS;
use slime_os_kernel::ipc::MAX_MSG;
use slime_os_kernel::memory::shared_buffer::{MAX_LOANS, MAX_MAPPINGS, MAX_TOTAL_PAGES};
use slime_os_kernel::syscall::MAX_WAIT_SOURCES;
use slime_os_kernel::{gdt, interrupts, memory};

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    slime_os_kernel::limine::ensure_linked();
    unsafe { slime_os_kernel::boot::init_from_limine() };
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

/// The booted generation carries exactly one authenticated fabric graph, and it
/// is satisfiable under this kernel's real ceilings. `decode` already ran
/// `validate_against` before any component launched; re-running it here against
/// the same constants proves the admission used the kernel's numbers, not a
/// copy that could drift.
#[test_case]
fn booted_generation_declares_an_admitted_fabric_graph() {
    let generation = slime_os_kernel::generation::decode(slime_os_kernel::boot::generation())
        .expect("booted generation decodes");
    let graph = slime_os_kernel::generation::fabric_graph(&generation)
        .expect("generation declares a graph");

    // Three declared routes: two C8.4 streams and one C8.6 call. Eight
    // participants across them, so the fan-out under test is a real
    // many-to-many graph rather than one edge per route.
    assert_eq!(graph.schema_count(), 3);
    assert_eq!(graph.route_count(), 3);
    assert_eq!(graph.participant_count(), 8);
    assert_eq!(
        graph.fabric_component_identity(),
        component_identity("fabric-service"),
        "the fabric host is the component the manifest names"
    );

    graph
        .validate_against(
            MAX_WAIT_SOURCES as u32,
            MAX_CAPS as u32,
            MAX_TOTAL_PAGES as u32,
            MAX_MAPPINGS as u32,
            MAX_LOANS as u32,
            MAX_MSG as u32,
        )
        .expect("the booted graph is satisfiable under the kernel's ceilings");

    let limits = graph.limits();
    assert!(
        limits.ingress_sources as usize <= MAX_WAIT_SOURCES,
        "a graph the fabric cannot block on would have to poll"
    );
    assert!(limits.capability_slots as usize <= MAX_CAPS);
    assert!(limits.buffer_pages as usize <= MAX_TOTAL_PAGES);
}

/// Route authority is the exact tuple, and only the tuple. A component that
/// holds the route name, the interface, and the contract kind still derives
/// nothing unless the generation declared its exact (component, direction) edge.
#[test_case]
fn route_authority_is_the_exact_tuple() {
    let generation = slime_os_kernel::generation::decode(slime_os_kernel::boot::generation())
        .expect("booted generation decodes");
    let graph = slime_os_kernel::generation::fabric_graph(&generation)
        .expect("generation declares a graph");

    let mut stream_routes = 0;
    let mut call_routes = 0;
    for index in 0..graph.route_count() {
        let route = graph.route(index).expect("route in range");
        match route.contract_kind {
            CONTRACT_KIND_STREAM => stream_routes += 1,
            CONTRACT_KIND_CALL => call_routes += 1,
            other => panic!("unexpected contract kind {other}"),
        }

        // The declared participants resolve; every other component and every
        // other direction on the same route does not.
        for participant in 0..graph.participant_count() {
            let entry = graph
                .participant(participant)
                .expect("participant in range");
            if entry.route_index as usize != index {
                continue;
            }
            assert_eq!(
                graph
                    .participant_for(&entry.grant_identity)
                    .expect("declared edge resolves")
                    .component_identity,
                entry.component_identity
            );
            // Same component, opposite role on the same route: not granted.
            let flipped = match entry.direction {
                DIRECTION_PUBLISH => DIRECTION_SUBSCRIBE,
                DIRECTION_SUBSCRIBE => DIRECTION_PUBLISH,
                DIRECTION_CLIENT => DIRECTION_SERVER,
                _ => DIRECTION_CLIENT,
            };
            assert!(
                graph
                    .participant_for(&grant_identity(
                        &route.route_identity,
                        &entry.component_identity,
                        flipped,
                    ))
                    .is_none(),
                "a granted role must not imply the opposite role"
            );
        }

        // A component that is not on this route derives nothing from the name.
        for direction in [
            DIRECTION_PUBLISH,
            DIRECTION_SUBSCRIBE,
            DIRECTION_CLIENT,
            DIRECTION_SERVER,
        ] {
            assert!(
                graph
                    .participant_for(&grant_identity(
                        &route.route_identity,
                        &component_identity("console"),
                        direction,
                    ))
                    .is_none(),
                "an ungranted component must derive no route authority"
            );
        }
    }
    // Two stream routes (C8.4 telemetry and diagnostics) and one call route
    // (C8.6 parameters), so the loop above covered both contract kinds.
    assert_eq!(stream_routes, 2);
    assert_eq!(call_routes, 1);
}

/// The declared interposition chain is present, terminates, and every hop names
/// a component distinct from the participant it proxies for.
#[test_case]
fn declared_interposition_chain_terminates_without_bypass() {
    let generation = slime_os_kernel::generation::decode(slime_os_kernel::boot::generation())
        .expect("booted generation decodes");
    let graph = slime_os_kernel::generation::fabric_graph(&generation)
        .expect("generation declares a graph");

    assert_eq!(graph.interposition_count(), 1);
    let mut interposed = 0;
    for index in 0..graph.participant_count() {
        let entry = graph.participant(index).expect("participant in range");
        let mut cursor = entry.interposition_head;
        if cursor == INTERPOSITION_NONE {
            continue;
        }
        interposed += 1;
        let mut steps = 0;
        while cursor != INTERPOSITION_NONE {
            let hop = graph
                .interposition(cursor as usize)
                .expect("hop resolves inside the table");
            assert_ne!(
                hop.component_identity, entry.component_identity,
                "a self-hop would be a bypass, not a proxy"
            );
            cursor = hop.next_hop;
            steps += 1;
            assert!(steps <= graph.interposition_count(), "chain must terminate");
        }
    }
    assert_eq!(interposed, 1, "exactly one participant is interposed");
}

/// Every matched pair in the booted graph has compatible offered/requested QoS.
/// C8.5 turns an incompatible pair into a structured event; a shipped
/// generation should not carry one.
#[test_case]
fn booted_graph_qos_pairs_are_compatible() {
    let generation = slime_os_kernel::generation::decode(slime_os_kernel::boot::generation())
        .expect("booted generation decodes");
    let graph = slime_os_kernel::generation::fabric_graph(&generation)
        .expect("generation declares a graph");
    assert!(graph.all_pairs_qos_compatible());
}
