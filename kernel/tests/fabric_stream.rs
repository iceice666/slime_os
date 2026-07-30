#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(slime_os_kernel::test_runner)]
#![reexport_test_harness_main = "test_main"]

//! C8.4 bounded many-to-many stream invariants.
//!
//! Exercises the exit condition from `roadmap/02-core-runtime.md` (C8.4): a
//! generation-declared many-to-many stream moves bounded typed inline and
//! shared samples under exact route authority; KEEP_LAST and BEST_EFFORT
//! behaviour is deterministic, and a stalled or faulting participant cannot
//! grow or disturb unrelated state.
//!
//! `just fabric_stream_check` is the live arm: it boots the real graph and
//! asserts the transcript five components produce. This file is the
//! structural arm, and covers the two things a transcript cannot show:
//!
//! 1. **The graph the boot admitted really declares the fan-out.** Counting
//!    markers proves samples moved; only reading the authenticated resource
//!    proves they moved along edges the generation fixed, with two publishers
//!    and two subscribers matched on one route and a second route that shares
//!    a participant but not a type.
//! 2. **The declared bounds are the ones the kernel can honour.** Every
//!    subscriber's KEEP_LAST depth, the per-graph loan and mapping budget, and
//!    the fan-out's demand on them are checked against this kernel's real
//!    ceilings rather than against a copy that could drift.
//!
//! The eviction rule itself is unit-tested in `boot-contracts`
//! (`stream_history`), where it runs without a boot.

extern crate alloc;
use boot_contracts::fabric_graph::{
    CONTRACT_KIND_STREAM, DIRECTION_PUBLISH, DIRECTION_SUBSCRIBE, TransportQos, component_identity,
    grant_identity, route_identity,
};
use boot_contracts::stream_history::{HistoryEntry, StreamHistory};
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

/// The telemetry route's identity, folded exactly as every participant folds it.
fn telemetry_route() -> [u8; 32] {
    route_identity(
        "telemetry",
        &slime_proto::interface_schema::telemetry_stream::INTERFACE_IDENTITY,
        CONTRACT_KIND_STREAM,
    )
}

fn diagnostics_route() -> [u8; 32] {
    route_identity(
        "diagnostics",
        &slime_proto::interface_schema::diagnostics_stream::INTERFACE_IDENTITY,
        CONTRACT_KIND_STREAM,
    )
}

fn graph_of(
    generation: &boot_contracts::generation::Generation<'static>,
) -> boot_contracts::fabric_graph::FabricGraph<'static> {
    slime_os_kernel::generation::fabric_graph(generation).expect("generation declares a graph")
}

fn booted() -> boot_contracts::generation::Generation<'static> {
    slime_os_kernel::generation::decode(slime_os_kernel::boot::generation())
        .expect("booted generation decodes")
}

/// The booted graph declares a real many-to-many stream: two publishers and
/// three subscribers on one route, each as its own authority tuple.
///
/// This is the structural form of the milestone's "two publishers and two
/// subscribers" check, now carrying C8.10's filtered-introspection client as a
/// third declared subscriber. A transcript can show components talking; only the
/// resource shows that the generation — not the service — decided they match.
#[test_case]
fn telemetry_route_declares_two_publishers_and_three_subscribers() {
    let generation = booted();
    let graph = graph_of(&generation);
    let route = telemetry_route();

    let mut publishers = 0;
    let mut subscribers = 0;
    for index in 0..graph.participant_count() {
        let entry = graph.participant(index).expect("participant");
        let route_entry = graph.route(entry.route_index as usize).expect("route");
        if route_entry.route_identity != route {
            continue;
        }
        match entry.direction {
            DIRECTION_PUBLISH => publishers += 1,
            DIRECTION_SUBSCRIBE => subscribers += 1,
            other => panic!("a stream route declared direction {other}"),
        }
    }
    assert_eq!(publishers, 2, "telemetry declares two publishers");
    assert_eq!(subscribers, 3, "telemetry declares three subscribers");
}

/// Every stream participant's authority is the exact fold of its tuple, and no
/// component holds a role on a route the generation did not name it on.
///
/// The negative half is the load-bearing one: `fabric-publisher` is declared
/// only on telemetry, so a grant identity naming it on diagnostics must be
/// absent even though both routes are streams carrying the same contract kind.
#[test_case]
fn stream_authority_does_not_cross_routes() {
    let generation = booted();
    let graph = graph_of(&generation);
    let telemetry = telemetry_route();
    let diagnostics = diagnostics_route();

    for (component, direction) in [
        ("fabric-publisher", DIRECTION_PUBLISH),
        ("fabric-publisher-b", DIRECTION_PUBLISH),
        ("fabric-subscriber", DIRECTION_SUBSCRIBE),
        ("fabric-subscriber-b", DIRECTION_SUBSCRIBE),
    ] {
        let identity = component_identity(component);
        assert!(
            graph
                .participant_for(&grant_identity(&telemetry, &identity, direction))
                .is_some(),
            "the generation declares this telemetry edge"
        );
    }

    // Only the -b pair spans both routes. The single-route pair holds nothing
    // on diagnostics, and neither pair holds the opposite direction anywhere.
    let publisher = component_identity("fabric-publisher");
    let subscriber = component_identity("fabric-subscriber");
    for (identity, route, direction) in [
        (&publisher, &diagnostics, DIRECTION_PUBLISH),
        (&subscriber, &diagnostics, DIRECTION_SUBSCRIBE),
        (&publisher, &telemetry, DIRECTION_SUBSCRIBE),
        (&subscriber, &telemetry, DIRECTION_PUBLISH),
    ] {
        assert!(
            graph
                .participant_for(&grant_identity(route, identity, direction))
                .is_none(),
            "an undeclared edge must derive no authority"
        );
    }

    // The second route carries a different interface, so its identity differs
    // even though both are streams: a name alone never selects a route.
    assert_ne!(telemetry, diagnostics);
}

/// Every declared subscriber's KEEP_LAST depth is finite, at least one, and
/// within the per-graph history ceiling — so the fabric can size a bounded ring
/// from it, and a stall costs a fixed number of entries.
#[test_case]
fn every_subscriber_declares_a_usable_keep_last_depth() {
    let generation = booted();
    let graph = graph_of(&generation);
    let limits = graph.limits();

    for index in 0..graph.participant_count() {
        let entry = graph.participant(index).expect("participant");
        if entry.direction != DIRECTION_SUBSCRIBE {
            continue;
        }
        assert!(
            entry.qos.history_depth >= 1,
            "KEEP_LAST has no unbounded form"
        );
        assert!(entry.qos.history_depth <= limits.history_depth);
        // The ring the fabric builds from this depth must actually exist.
        StreamHistory::new(entry.qos.history_depth as usize)
            .expect("a declared depth sizes a real ring");
    }
}

/// The declared fan-out is one the kernel can honour: one downstream loan and
/// one mapping per matched subscriber, inside both the per-graph budget and the
/// kernel's fixed tables.
///
/// C8.2 validated the graph against these ceilings at decode. Re-deriving the
/// stream-specific demand here is what ties the milestone's "one quota-charged
/// receiver-bound loan per subscriber" to a number the kernel can actually
/// grant, rather than to the transcript's say-so.
#[test_case]
fn stream_fan_out_fits_the_kernel_and_the_declared_budget() {
    let generation = booted();
    let graph = graph_of(&generation);
    let limits = graph.limits();

    let mut subscribers: u32 = 0;
    for index in 0..graph.participant_count() {
        let entry = graph.participant(index).expect("participant");
        if entry.direction == DIRECTION_SUBSCRIBE {
            subscribers += 1;
        }
    }
    assert!(subscribers >= 2, "the graph declares a real fan-out");
    // One loan and one mapping per matched subscriber is the fabric's worst
    // case for a single large sample in flight.
    assert!(subscribers <= limits.loans);
    assert!(subscribers <= limits.mappings);
    assert!(limits.loans as usize <= MAX_LOANS);
    assert!(limits.mappings as usize <= MAX_MAPPINGS);
    // A sample larger than the control-message bound must be carriable at all,
    // which means the graph has to budget pages for the fabric's own copy.
    assert!(limits.sample_bytes > MAX_MSG as u32);
    assert!(limits.buffer_pages > 0);
    assert!(limits.buffer_pages as usize <= MAX_TOTAL_PAGES);
    assert!(limits.capability_slots as usize <= MAX_CAPS);
    // Every publisher is one live fabric ingress, and the fabric parks across
    // all of them at once: a graph past `MAX_WAIT_SOURCES` would have to poll.
    assert!(limits.ingress_sources as usize <= MAX_WAIT_SOURCES);
}

/// The declared publisher/subscriber pairs on each stream route are QoS
/// compatible, and the BEST_EFFORT subscriber is genuinely the weaker request.
///
/// The milestone's loss arm is only meaningful if the graph really declares a
/// BEST_EFFORT reader: a RELIABLE one would make dropping a sample a fault
/// rather than the declared behaviour under test.
#[test_case]
fn declared_stream_qos_admits_the_best_effort_reader() {
    let generation = booted();
    let graph = graph_of(&generation);
    assert!(
        graph.all_pairs_qos_compatible(),
        "a shipped generation should carry no incompatible pair"
    );

    let route = telemetry_route();
    let identity = component_identity("fabric-subscriber-b");
    let entry = graph
        .participant_for(&grant_identity(&route, &identity, DIRECTION_SUBSCRIBE))
        .expect("the stalling subscriber is declared");
    assert_eq!(
        entry.qos.reliability as u32,
        boot_contracts::fabric_graph::RELIABILITY_BEST_EFFORT,
        "the loss arm needs a declared BEST_EFFORT reader"
    );

    // A RELIABLE offer satisfies a BEST_EFFORT request, which is why this pair
    // matches at all; the reverse does not, and that asymmetry is the truth
    // table rather than a policy the fabric chose.
    let publisher = component_identity("fabric-publisher");
    let offer = graph
        .participant_for(&grant_identity(&route, &publisher, DIRECTION_PUBLISH))
        .expect("the publisher is declared");
    assert!(TransportQos::offer_satisfies(&offer.qos, &entry.qos));
    assert!(!TransportQos::offer_satisfies(&entry.qos, &offer.qos));
}

/// A ring sized from the booted graph evicts the exact oldest sequence at its
/// declared depth, and reports the loss once.
///
/// The `boot-contracts` unit tests cover the ring in isolation. This runs it at
/// the depth a real generation declared, so a manifest change that made the
/// declared depth unusable — or made eviction unreachable — fails on the boot
/// path rather than only in a host test with a synthetic depth.
#[test_case]
fn the_declared_depth_evicts_the_oldest_sequence() {
    let generation = booted();
    let graph = graph_of(&generation);
    let route = telemetry_route();
    let identity = component_identity("fabric-subscriber-b");
    let entry = graph
        .participant_for(&grant_identity(&route, &identity, DIRECTION_SUBSCRIBE))
        .expect("the stalling subscriber is declared");
    let depth = entry.qos.history_depth as usize;

    let mut history = StreamHistory::new(depth).expect("declared depth");
    for sequence in 1..=depth as u64 {
        assert_eq!(
            history.push(HistoryEntry {
                sequence,
                publisher: 0,
                slot: sequence as u32,
                inline: true,
            }),
            None,
            "nothing is evicted below the declared depth"
        );
    }
    // One past the depth evicts sequence 1 — the exact oldest, and only it.
    let evicted = history
        .push(HistoryEntry {
            sequence: depth as u64 + 1,
            publisher: 0,
            slot: 0,
            inline: true,
        })
        .expect("at depth, admitting one more evicts");
    assert_eq!(evicted.sequence, 1);
    assert_eq!(history.len(), depth);
    assert_eq!(history.take_loss(), Some((1, 1)));
    assert_eq!(history.take_loss(), None, "one stall reports once");
}

/// Every malformed descriptor the milestone names is refused *before* the
/// fabric would map or allocate anything.
///
/// The live gate forbids every rejection marker, which proves no component
/// under test emits a bad record — but a forbidden marker is not a test that
/// the check works. This is that test: it drives `valid_sample_descriptor`,
/// the exact predicate `admit_shared` gates on, with each malformed shape the
/// required checks list. A validator that stopped rejecting one of them would
/// pass the transcript gate unchanged and fail here.
#[test_case]
fn malformed_descriptors_are_refused_before_mapping() {
    use slime_proto::sample_descriptor::{
        CAPABILITY_KIND_LOAN, FLAG_LAST, FORMAT_VERSION, MAX_SAMPLE_BYTES, SAMPLE_DESCRIPTOR_MAGIC,
        WireSampleDescriptor,
    };
    use slime_proto::valid_sample_descriptor;

    const PAGE: u64 = 4096;
    let tag = slime_proto::interface_schema::telemetry_stream::TYPE_TAG;
    let admitted = WireSampleDescriptor {
        magic: SAMPLE_DESCRIPTOR_MAGIC,
        version: FORMAT_VERSION,
        flags: FLAG_LAST,
        capability_kind: CAPABILITY_KIND_LOAN,
        loan_id: 0x51,
        offset: 0,
        length: 2 * PAGE,
        type_identity: tag,
        sequence: 1,
        reserved: [0; 8],
    };
    assert!(
        valid_sample_descriptor(&admitted, admitted.loan_id, tag, PAGE),
        "the well-formed descriptor must be admitted, or the negatives prove nothing"
    );

    // Wrong type tag: another route's samples cannot enter this one.
    let wrong_tag = WireSampleDescriptor {
        type_identity: slime_proto::interface_schema::diagnostics_stream::TYPE_TAG,
        ..admitted
    };
    assert!(!valid_sample_descriptor(
        &wrong_tag,
        wrong_tag.loan_id,
        tag,
        PAGE
    ));

    // Stale loan identity: the descriptor names a loan the holder does not hold.
    let stale = WireSampleDescriptor {
        loan_id: admitted.loan_id ^ 1,
        ..admitted
    };
    assert!(!valid_sample_descriptor(
        &stale,
        admitted.loan_id,
        tag,
        PAGE
    ));

    // A zero identity is never a live loan.
    let unbound = WireSampleDescriptor {
        loan_id: 0,
        ..admitted
    };
    assert!(!valid_sample_descriptor(&unbound, 0, tag, PAGE));

    for malformed in [
        // Unknown flag bit.
        WireSampleDescriptor {
            flags: !0,
            ..admitted
        },
        // Unsupported version.
        WireSampleDescriptor {
            version: FORMAT_VERSION + 1,
            ..admitted
        },
        // Not a descriptor at all.
        WireSampleDescriptor {
            magic: SAMPLE_DESCRIPTOR_MAGIC ^ 1,
            ..admitted
        },
        // A capability kind this contract does not carry.
        WireSampleDescriptor {
            capability_kind: CAPABILITY_KIND_LOAN + 1,
            ..admitted
        },
        // Unaligned offset and length: a mapping must be page-exact.
        WireSampleDescriptor {
            offset: 1,
            ..admitted
        },
        WireSampleDescriptor {
            length: PAGE + 1,
            ..admitted
        },
        // Zero length maps nothing.
        WireSampleDescriptor {
            length: 0,
            ..admitted
        },
        // Offset + length overflows, and past the contract's sample ceiling.
        WireSampleDescriptor {
            offset: u64::MAX & !(PAGE - 1),
            ..admitted
        },
        WireSampleDescriptor {
            length: MAX_SAMPLE_BYTES as u64 + PAGE,
            ..admitted
        },
        // Reserved bytes are canonical zeros.
        WireSampleDescriptor {
            reserved: [1; 8],
            ..admitted
        },
    ] {
        assert!(
            !valid_sample_descriptor(&malformed, admitted.loan_id, tag, PAGE),
            "a malformed descriptor must be refused before mapping or allocating"
        );
    }
}

/// A stream record is refused unless every field that could steer a copy is
/// bounded, and an event is never mistaken for data.
#[test_case]
fn malformed_stream_records_are_refused() {
    use slime_proto::fabric_stream::{
        EVENT_SAMPLE_LOST, EVENT_SAMPLE_TAKEN, EVENT_STREAM_END, FORMAT_VERSION, MAX_INLINE_BYTES,
        STREAM_EVENT_MAGIC, STREAM_SAMPLE_MAGIC, WireStreamEvent, WireStreamSample,
    };
    use slime_proto::{valid_stream_event, valid_stream_sample};

    let tag = slime_proto::interface_schema::telemetry_stream::TYPE_TAG;
    let sample = WireStreamSample {
        magic: STREAM_SAMPLE_MAGIC,
        version: FORMAT_VERSION,
        flags: 0,
        payload_len: MAX_INLINE_BYTES as u32,
        sequence: 1,
        type_identity: tag,
        payload: [7; MAX_INLINE_BYTES],
    };
    assert!(valid_stream_sample(&sample, tag, MAX_INLINE_BYTES));

    for malformed in [
        // A payload longer than the record can hold.
        WireStreamSample {
            payload_len: MAX_INLINE_BYTES as u32 + 1,
            ..sample
        },
        // An empty payload carries no sample.
        WireStreamSample {
            payload_len: 0,
            ..sample
        },
        // Another route's type.
        WireStreamSample {
            type_identity: slime_proto::interface_schema::diagnostics_stream::TYPE_TAG,
            ..sample
        },
        // Unknown flag, unsupported version, wrong record.
        WireStreamSample {
            flags: !0,
            ..sample
        },
        WireStreamSample {
            version: FORMAT_VERSION + 1,
            ..sample
        },
        WireStreamSample {
            magic: STREAM_EVENT_MAGIC,
            ..sample
        },
    ] {
        assert!(!valid_stream_sample(&malformed, tag, MAX_INLINE_BYTES));
    }

    // Non-zero padding past the declared length: two byte-distinct samples must
    // not decode to one payload.
    let mut padded = sample;
    padded.payload_len = 4;
    assert!(!valid_stream_sample(&padded, tag, MAX_INLINE_BYTES));

    // Each event kind is bound to the fields it may name, so no kind can be
    // forged into another's meaning.
    let lost = WireStreamEvent {
        magic: STREAM_EVENT_MAGIC,
        version: FORMAT_VERSION,
        event: EVENT_SAMPLE_LOST,
        flags: 0,
        lost: 3,
        sequence: 1,
        type_identity: tag,
        reserved: [0; 24],
    };
    assert!(valid_stream_event(&lost, tag));
    // A loss report that names no loss, and a terminal notice that names one.
    assert!(!valid_stream_event(
        &WireStreamEvent { lost: 0, ..lost },
        tag
    ));
    assert!(!valid_stream_event(
        &WireStreamEvent {
            event: EVENT_STREAM_END,
            ..lost
        },
        tag
    ));
    // A credit must name the sample it settles.
    assert!(valid_stream_event(
        &WireStreamEvent {
            event: EVENT_SAMPLE_TAKEN,
            lost: 0,
            ..lost
        },
        tag
    ));
    assert!(!valid_stream_event(
        &WireStreamEvent {
            event: EVENT_SAMPLE_TAKEN,
            lost: 0,
            sequence: 0,
            ..lost
        },
        tag
    ));
    // An undefined kind is refused rather than defaulted.
    assert!(!valid_stream_event(
        &WireStreamEvent { event: 0, ..lost },
        tag
    ));
}

/// C8.5's control records reject malformed time and ambiguous QoS events.
#[test_case]
fn malformed_qos_and_time_records_are_refused() {
    use slime_proto::fabric_qos::{
        EVENT_DEADLINE_MISSED, EVENT_LIFESPAN_EXPIRED, EVENT_MATCHED, FORMAT_VERSION,
        QOS_EVENT_MAGIC, WireQosEvent,
    };
    use slime_proto::fabric_time::{TIME_ADVANCE_MAGIC, WireTimeAdvance};
    use slime_proto::{valid_qos_event, valid_time_advance};

    let tag = slime_proto::interface_schema::telemetry_stream::TYPE_TAG;
    let time = WireTimeAdvance {
        magic: TIME_ADVANCE_MAGIC,
        version: slime_proto::fabric_time::FORMAT_VERSION,
        flags: 0,
        reserved0: 0,
        now_ns: 100,
        reserved: [0; 40],
    };
    assert!(valid_time_advance(&time));
    for malformed in [
        WireTimeAdvance { magic: 0, ..time },
        WireTimeAdvance {
            version: slime_proto::fabric_time::FORMAT_VERSION + 1,
            ..time
        },
        WireTimeAdvance { flags: 1, ..time },
        WireTimeAdvance {
            reserved0: 1,
            ..time
        },
        WireTimeAdvance {
            reserved: [1; 40],
            ..time
        },
    ] {
        assert!(!valid_time_advance(&malformed));
    }

    let event = WireQosEvent {
        magic: QOS_EVENT_MAGIC,
        version: FORMAT_VERSION,
        event: EVENT_MATCHED,
        flags: 0,
        sequence: 0,
        value: 2,
        timestamp_ns: 0,
        type_identity: tag,
        reserved: [0; 16],
    };
    assert!(valid_qos_event(&event, tag));
    for malformed in [
        WireQosEvent { magic: 0, ..event },
        WireQosEvent {
            version: FORMAT_VERSION + 1,
            ..event
        },
        WireQosEvent { flags: 1, ..event },
        WireQosEvent {
            event: EVENT_MATCHED,
            type_identity: 0,
            ..event
        },
        WireQosEvent { event: 0, ..event },
        WireQosEvent {
            reserved: [1; 16],
            ..event
        },
    ] {
        assert!(!valid_qos_event(&malformed, tag));
    }

    // Distinct timed conditions remain distinct wire values.
    assert_ne!(EVENT_DEADLINE_MISSED, EVENT_LIFESPAN_EXPIRED);
}

/// The booted QoS profile fixes every state bound used by the live scenario.
#[test_case]
fn qos_profile_is_bounded_and_contains_retained_and_best_effort_paths() {
    use boot_contracts::fabric_graph::{
        DURABILITY_RETAINED, DURABILITY_VOLATILE, RELIABILITY_BEST_EFFORT, RELIABILITY_RELIABLE,
    };

    let generation = booted();
    let graph = graph_of(&generation);
    let limits = graph.limits();
    let mut retained = 0;
    let mut best_effort = 0;
    let mut reliable = 0;

    assert!(limits.history_depth > 0);
    assert!(limits.event_depth > 0);
    assert!(limits.retained_samples > 0);
    assert!(limits.retries > 0);
    for index in 0..graph.participant_count() {
        let participant = graph.participant(index).expect("participant");
        assert!(participant.qos.history_depth <= limits.history_depth);
        assert!(participant.qos.retained_depth <= limits.retained_samples);
        match participant.qos.reliability as u32 {
            RELIABILITY_RELIABLE => reliable += 1,
            RELIABILITY_BEST_EFFORT => best_effort += 1,
            other => panic!("unsupported reliability {other}"),
        }
        match participant.qos.durability as u32 {
            DURABILITY_RETAINED => {
                retained += 1;
                assert!(participant.qos.retained_depth > 0);
            }
            DURABILITY_VOLATILE => assert_eq!(participant.qos.retained_depth, 0),
            other => panic!("unsupported durability {other}"),
        }
    }
    assert!(reliable > 0);
    assert!(best_effort > 0);
    assert!(retained > 0);
}
