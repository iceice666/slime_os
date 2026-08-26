#![no_std]
#![no_main]

//! C8.3/C8.4 fabric: attenuated endpoint provisioning and bounded many-to-many
//! stream brokering.
//!
//! A userspace service that owns every route endpoint in the generation's data
//! fabric, hands each participant exactly one non-transferable role capability,
//! and then brokers the samples those roles carry. The kernel supplies two
//! generic mechanisms — `SYS_CAP_TRANSFER`, a bounded narrow-on-transfer move,
//! and the C7 shared-buffer loan lifecycle — and knows nothing of routes,
//! schemas, graph roles, QoS, or matching; all of that policy lives here.
//!
//! **Provisioning (C8.3).** Three properties this service exists to make true:
//!
//! 1. **A role is one direction.** A publisher's endpoint carries `RIGHT_SEND`
//!    and nothing else; a subscriber's carries `RIGHT_RECV`. The two halves of
//!    a route are separate kernel endpoints, so a publisher cannot receive on
//!    its route even by misusing the capability it holds.
//! 2. **A provisioned endpoint is terminal.** Every move omits
//!    `RIGHT_TRANSFER`, so a participant cannot re-delegate its role or mint a
//!    downstream edge. Non-delegability is enforced by the kernel at the moment
//!    of transfer, not by convention afterwards.
//! 3. **Names grant nothing.** A client is authenticated by the
//!    generation-provisioned control endpoint its request arrived on — the
//!    binding init established at spawn — never by the route name, direction,
//!    or type identity the request carries. Those fields are read and ignored:
//!    the answer comes from the graph table, keyed by the caller's identity.
//!
//! **Brokering (C8.4).** Matching is the exact tuple the graph declares: a
//! publisher and a subscriber exchange data only when they name one route, and
//! a route is (name, full interface identity, contract kind). Two participants
//! on different routes never see each other's samples even though one service
//! moves both, because a sample is dispatched by the route index its ingress
//! endpoint belongs to — never by anything the sample itself claims.
//!
//! A sample travels one of two ways, decided by size alone:
//!
//! - **Inline.** A payload within `MAX_INLINE_BYTES` rides in the fixed
//!   `StreamSample` control message, one kernel message per subscriber.
//! - **Shared.** A payload larger than the control-message bound arrives as a
//!   C7.6 descriptor naming a receiver-bound loan. The fabric maps that loan
//!   read-only, copies the bytes **once** into a fabric-owned sealed buffer,
//!   and then creates one independently accounted downstream loan per matched
//!   subscriber. One publisher sample is therefore one copy and N loans, never
//!   N copies, and the upstream loan is returned as soon as the copy lands.
//!
//! Delivery is bounded per subscriber by its declared KEEP_LAST depth. A
//! subscriber releases a delivery slot with a `StreamAck`; until it does, the
//! fabric holds at most `history_depth` samples for it and evicts the oldest to

//! admit a newer one. Eviction is counted, and one stall produces exactly one
//! `SAMPLE_LOST` event when delivery resumes — never a growing queue and never
//! a retry.

#[path = "../../../lib/src/call_broker.rs"]
mod call_broker;

#[path = "../../../lib/src/matrix_broker.rs"]
mod matrix_broker;
#[path = "../../../lib/src/operation_broker.rs"]
mod operation_broker;
#[path = "../../../lib/src/visibility_broker.rs"]
mod visibility_broker;
// C8.11's trace emitter. Included by path like the brokers: this binary is the
// stream worker, and it holds its own sink.
#[path = "../../../lib/src/fabric_trace_log.rs"]
mod trace_log;

extern crate alloc;

use boot_contracts::fabric_graph::{
    CONTRACT_KIND_STREAM, DIRECTION_PUBLISH, DIRECTION_SUBSCRIBE, DURABILITY_RETAINED,
    FRAME_CAPACITY, MAX_ROLE_PARTICIPANTS, RELIABILITY_RELIABLE, RuntimeLimits, TransportQos,
    route_identity,
};
use boot_contracts::stream_history::{HistoryEntry, StreamHistory};
use slime_proto::capability_transfer::{
    CAPABILITY_TRANSFER_MAGIC, FABRIC_REQUEST_MAGIC, FLAG_RETAIN_TRANSFER, FORMAT_VERSION,
    OBJECT_KIND_SHARED_BUFFER_LOAN, REQUEST_LEN, TRANSFER_LEN, WireCapabilityTransfer,
    WireFabricRequest,
};
use slime_proto::fabric_qos::{
    EVENT_DEADLINE_MISSED, EVENT_INCOMPATIBLE_QOS, EVENT_LIFESPAN_EXPIRED, EVENT_LIVELINESS_LOST,
    EVENT_MATCHED, EVENT_PEER_DEAD, EVENT_RETRY_EXHAUSTED, EVENT_UNMATCHED,
    FORMAT_VERSION as QOS_FORMAT_VERSION, QOS_EVENT_MAGIC, WireQosEvent,
};
use slime_proto::fabric_stream::{
    EVENT_SAMPLE_LOST, EVENT_SAMPLE_TAKEN, EVENT_STREAM_END, FLAG_LAST, MAX_INLINE_BYTES,
    STREAM_EVENT_MAGIC, WireStreamEvent,
};
use slime_proto::fabric_time::WireTimeAdvance;
use slime_proto::interface_schema::{diagnostics_stream, telemetry_stream};
use slime_proto::ring::{Ring, RingError};
use slime_proto::sample_descriptor::{
    CAPABILITY_KIND_LOAN, SAMPLE_DESCRIPTOR_MAGIC, WireSampleDescriptor,
};
use slime_proto::{valid_fabric_request, valid_sample_descriptor};
use slime_rt::{
    CapabilityDisposition, ERR_OUT_OF_MEMORY, ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG,
    MAX_MSG,
};
// B59: the capability-rights vocabulary is generated from
// `contracts/generation/v5/schema.zt`; these were local copies of the same
// bit numbering.
use boot_contracts::generation::{BootAction, RIGHT_BUFFER_MAP, RIGHT_BUFFER_WRITE};

slime_rt::entry!(main);

/// This generation's authenticated fabric ceilings (B70/CP2).
///
/// Every number this worker admits traffic against comes from here rather than
/// from a per-plane table a build script rendered into `OUT_DIR`: the ceilings
/// are a property of the graph the root authenticated, so a component built
/// outside this repository resolves them the same way.
fn load_runtime_limits() -> RuntimeLimits {
    RuntimeLimits::load(slime_rt::graph_query).unwrap_or_else(|_| fail(b"fabric graph limit query"))
}

/// The sink capacity this generation declares, refused rather than clamped when
/// it falls outside the contract.
///
/// `RuntimeLimits::trace_sink_depth` owns both comparisons, so this worker does
/// not restate the bounds the contract already fixes. Reaching `None` means the
/// root answered a depth no admitted graph declares, which is a refusal: a
/// depth past the sink's storage does not fit, and one at or below the terminal
/// reservation leaves no slot for ordinary evidence.
fn declared_trace_depth(limits: &RuntimeLimits) -> usize {
    if !limits.trace_overflow_is_saturate() {
        fail(b"generation declares an unimplemented trace overflow");
    }
    limits
        .trace_sink_depth()
        .unwrap_or_else(|| fail(b"declared trace depth is outside the contract"))
}

/// Pages in one fabric-owned copy buffer, derived from the generation's own
/// declared `sampleBytes` ceiling instead of a builder constant compiled in.
///
/// It sizes no array -- the two scratch windows below are mapped at whatever
/// length the admitted descriptor names -- so nothing here needs it before the
/// graph has been read, and it stays a runtime number rather than a published
/// one.
fn copy_pages(limits: &RuntimeLimits) -> u64 {
    let pages = u64::from(limits.max_sample_bytes).div_ceil(PAGE);
    if pages == 0 || pages > u64::from(limits.max_buffer_pages) {
        fail(b"declared sample ceiling does not fit the declared page budget");
    }
    pages
}

/// `SharedBufferFactory`, granted by the generation. Every declared stream edge
/// gets one page-backed v2 ring; the participant receives a narrowed copy over
/// its already-installed direct control endpoint.
///
/// Resolved by *what the capability is* rather than by the number one manifest
/// binds it at, over both declaration tables the manifests actually use. The
/// `sel4-*` planes and `valid.zti` declare it as an ordinary grant under two
/// different names -- `fabric-service-shared-buffer-factory` and
/// `fabric-shared-buffer-factory` -- which is why the `kind:` axis rather than
/// a name answers that half. `sel4-qos`, `sel4-call`, and `sel4-operation`
/// declare it as a *minted* binding instead, a separate table the `kind:` axis
/// does not search, and there the name is fixed by the `<holder>-<role>`
/// convention `minted:<component>-supervision` already relies on. Asking only
/// one way resolves nothing on the other planes.
///
/// Cached like `time_slot`: every ring provisioned and every large sample
/// allocates through it.
static mut BUFFER_FACTORY_CACHE: u32 = u32::MAX;

fn buffer_factory_slot() -> u32 {
    // SAFETY: single-threaded, and every reader is on the one dispatch loop.
    let cached = unsafe { *core::ptr::addr_of!(BUFFER_FACTORY_CACHE) };
    if cached != u32::MAX {
        return cached;
    }
    let slot = slime_rt::resolve_binding(b"kind:sharedBufferFactory+bufferCreate")
        .or_else(|_| slime_rt::resolve_binding(b"minted:fabric-service-shared-buffer-factory"))
        .unwrap_or_else(|_| fail(b"shared-buffer factory grant"));
    // SAFETY: as above.
    unsafe { *core::ptr::addr_of_mut!(BUFFER_FACTORY_CACHE) = slot };
    slot
}
/// The simulated clock's control endpoint, resolved from the root by the name
/// the generation gives that edge.
///
/// This was `FABRIC_FIRST_CONTROL_SLOT + FABRIC_CLIENTS.len() +
/// FABRIC_SUPERVISION.len()`, which replaced an earlier hardcoded 9 that a new
/// ring participant silently moved supervision onto (B50/R2). The derivation
/// fixed the racing constant but kept the broker reconstructing the builder's
/// layout rule from two generated tables; the edge has a declared name, so the
/// name answers it outright and neither table is consulted.
///
/// Both readers sit behind `qos_check()`, which is `"qos" || "traffic"` — the
/// only two graphs declaring a clock, and both name this grant identically.
/// Verified equal to the derived number before the change:
/// `fabric-publisher-b-clock` is 11 under `sel4-qos.zti`.
/// Resolved on first use and cached, following `fabric_call_scenario`'s
/// `WAKE_SLOT`: `receive_time` runs on every iteration of the broker's dispatch
/// loop, so resolving per call would put a syscall where a constant was.
static mut TIME_SLOT_CACHE: u32 = u32::MAX;

fn time_slot() -> u32 {
    // SAFETY: single-threaded, and every reader is on the one dispatch loop.
    let cached = unsafe { *core::ptr::addr_of!(TIME_SLOT_CACHE) };
    if cached != u32::MAX {
        return cached;
    }
    let slot = slime_rt::resolve_binding(b"fabric-publisher-b-clock")
        .unwrap_or_else(|_| fail(b"clock control slot"));
    // SAFETY: as above.
    unsafe { *core::ptr::addr_of_mut!(TIME_SLOT_CACHE) = slot };
    slot
}
/// The component that owns the other end of `time_slot()`. Named rather than
/// numbered because its supervision handle is what reports the clock's exit:
/// no `ERR_PEER_DEAD` reaches a native Endpoint.
const TIME_COMPONENT: &[u8] = b"fabric-publisher-b";
/// Operation-plane endpoint that releases the supervised replacement only
/// after the broker has installed its fresh route and supervision identity.
///
/// Asked for by the name the generation gives that edge rather than written as
/// a literal 12, which was the builder's own slot numbering restated inside a
/// component. Only `sel4-operation` and `sel4-traffic` reach this arm and both
/// declare the grant outright.
fn operation_replacement_start_slot() -> u32 {
    slime_rt::resolve_binding(b"fabric-op-restart-start")
        .unwrap_or_else(|_| fail(b"operation replacement release endpoint"))
}

const PAGE: u64 = 4096;
/// `notification_slots` returns this on both sides when the graph declares no
/// ready/credit pair for an edge. Never a real slot: the root's per-task
/// notification table is bounded well under `u32::MAX`, so a caller that
/// mistakenly signalled or polled it gets a bounds error rather than another
/// component's handle.
const NOTIFICATION_ABSENT: u32 = u32::MAX;
const RING_BASE: u64 = 0x0000_0010_0000_0000;
/// The routes this generation declares. Folded at runtime with the generated
/// C8.1 interface identities so a route identity cannot drift from the admitted
/// schema. Index into this table *is* the route identity for dispatch: a sample
/// is routed by the ingress it arrived on, never by anything it claims.
const ROUTE_NAMES: [&str; 2] = ["telemetry", "diagnostics"];
const ROUTE_COUNT: usize = ROUTE_NAMES.len();

/// Provisioning denial. Distinct from a malformed request so the transcript
/// shows *why* an edge was refused.
const STATUS_NOT_GRANTED: i32 = -1;
const STATUS_BAD_REQUEST: i32 = -2;

/// Brokering capacity *ceilings*. These bound what a graph may declare; the
/// tables themselves are sized from what a graph actually declares (C10.4).
///
/// Before C10.4 there was a third constant here, `MAX_PARTICIPANTS`, summing
/// the two role ceilings because one array had to hold both roles' worst case
/// at once. The tables are now per-role and sized from the declared counts, so
/// each role is admitted against its own ceiling and the sum has no referent.
///
/// `MAX_ROLE_PARTICIPANTS` is the ceiling `validate_declared_limits` admits a
/// graph's `publishers` and `subscribers` against, and it exists precisely
/// because one record of either role carries a full `LIMIT_HISTORY_DEPTH`
/// history: a graph promising more than this describes storage no component
/// has.
///
/// `MAX_FRAMES` bounds the fabric-owned sample frames, each holding one inline
/// payload or naming one fabric-owned buffer; a frame is freed when its last
/// reference is delivered or evicted. It is the graph's `historyDepth` ceiling
/// times the subscriber ceiling it can face at once. A frame is referenced by
/// every subscriber ring holding it, so a table smaller than the summed
/// declared depths would let the rings fill while no frame is free — and with
/// the stalled subscriber holding its ring and the publishers blocked, nothing
/// would ever wake the fabric again. That is a deadlock, not backpressure, so
/// `declared_capacity` refuses a graph whose summed depths pass this rather
/// than discovering it later.
const MAX_FRAMES: usize = FRAME_CAPACITY;
/// Scratch window where the fabric maps an upstream loan and its own copy
/// buffer. Two disjoint ranges, both unmapped before the next sample.
const UPSTREAM_BASE: u64 = 0x0000_000B_0000_0000;
const COPY_BASE: u64 = 0x0000_000C_0000_0000;

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[fabric] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

/// Decimal `u32` on the debug sink. Diagnostic only: slot numbers and error
/// codes are what make a refusal actionable, and neither fits a fixed string.
fn write_u32(mut value: u32) {
    if value == 0 {
        slime_rt::debug_write(b"0");
        return;
    }
    let mut buffer = [0u8; 10];
    let mut cursor = buffer.len();
    while value != 0 {
        cursor -= 1;
        buffer[cursor] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    slime_rt::debug_write(&buffer[cursor..]);
}

fn write_i64(value: i64) {
    if value < 0 {
        slime_rt::debug_write(b"-");
    }
    write_u32(value.unsigned_abs() as u32);
}

fn qos_check() -> bool {
    slime_components::generation_composition::is(BootAction::Qos)
        || slime_components::generation_composition::is(BootAction::Traffic)
}

/// One client's control binding: the slot init gave the fabric for it, and the
/// component identity that slot authenticates.
struct Client {
    control_slot: u32,
    component: &'static [u8],
    /// Set once this control endpoint has been answered. A route role is minted
    /// once per declared edge; a further request over the same endpoint is
    /// refused rather than silently issuing a duplicate edge.
    answered: bool,
}

/// One provisioned publisher-to-fabric ring. The publisher owns `head`; this
/// service owns `tail` and drains until the ring itself says empty.
struct Publisher {
    /// Identity authenticated by `control_slot`. Kept on the role so a fresh
    /// request can replace only this participant rather than disturbing every
    /// edge on its route.
    component: &'static [u8],
    control_slot: u32,
    ring_base: u64,
    /// Fabric-owned ring buffer and the outstanding participant loan. A
    /// replacement must settle both before minting another role or each restart
    /// consumes one more frame-table/buffer-budget charge.
    buffer_slot: u32,
    loan_id: u64,
    ring_slots: usize,
    ready_slot: u32,
    credit_slot: u32,
    route: usize,
    supervision_slot: Option<u32>,
    finished: bool,
    /// Set when the publisher exited without ending its route. C8.5 requires
    /// peer death to stay distinguishable from an orderly end, and a native
    /// Endpoint reports neither: the supervision handle is the only observer.
    died: bool,
    /// Set once supervision has reported this publisher terminated. The
    /// observation is latched rather than acted on, because it races the ring:
    /// a peer that wrote its last sample and exited is terminated while that
    /// sample is still queued, and concluding death there would both lose the
    /// sample's orderly end and make the trace depend on which side won.
    terminated: bool,
    /// Set when the last pump consumed this publisher's ring to `Empty`, and
    /// cleared when it stopped early on frame exhaustion or when `terminated`
    /// latches. Death is concluded only from a drain that ran *after* the
    /// termination observation, so an emptiness observed before the peer's
    /// final write cannot authorise it.
    drained: bool,
    qos: TransportQos,
    last_assertion_ns: u64,
    retained: StreamHistory,
}

/// One provisioned fabric-to-subscriber ring. QoS records and correlated large
/// descriptors remain structured direct-control messages on `control_slot`;
/// ordinary samples never use that endpoint.
struct Subscriber {
    /// Identity authenticated by `control_slot`; see [`Publisher::component`].
    component: &'static [u8],
    control_slot: u32,
    ring_base: u64,
    /// Fabric-owned ring buffer and the outstanding participant loan. See the
    /// publisher fields for why both identities are retained.
    buffer_slot: u32,
    loan_id: u64,
    ring_slots: usize,
    ready_slot: u32,
    credit_slot: u32,
    route: usize,
    supervision_slot: Option<u32>,
    history: StreamHistory,
    in_flight: usize,
    ended: bool,
    qos: TransportQos,
    matched_publishers: u32,
    deadline_reported: bool,
    liveliness_reported: bool,
    retry_count: u32,
    terminal: bool,
    retry_interval_ns: u64,
    last_retry_ns: u64,
}

#[derive(Clone, Copy)]
struct LateSubscriber {
    history: StreamHistory,
    qos: TransportQos,
    received: bool,
    delivered: bool,
}

/// One fabric-owned sample frame. `refs` is the number of subscriber histories
/// still naming it; the frame is released when that reaches zero, so an evicted
/// sample frees its storage without disturbing a subscriber still holding it.
#[derive(Clone, Copy)]
struct Frame {
    refs: usize,
    sequence: u64,
    type_identity: u64,
    flags: u32,
    /// Inline payload bytes, valid for `payload_len` when `buffer_slot` is
    /// `None`.
    payload: [u8; MAX_INLINE_BYTES],
    payload_len: usize,
    /// Fabric-owned sealed buffer holding a large sample's single copy.
    buffer_slot: Option<u32>,
    buffer_len: u64,
    admitted_ns: u64,
}

impl Frame {
    const EMPTY: Self = Self {
        refs: 0,
        sequence: 0,
        type_identity: 0,
        flags: 0,
        payload: [0; MAX_INLINE_BYTES],
        payload_len: 0,
        buffer_slot: None,
        buffer_len: 0,
        admitted_ns: 0,
    };
}

/// The stream plane's role and frame tables, sized from the graph this
/// generation declared rather than from the contract's ceilings (C10.4).
///
/// These three are by far the largest objects the plane holds. They began as
/// locals of `main`/`traffic_graph`: one `Publisher` or `Subscriber` carries a
/// whole `StreamHistory` -- `LIMIT_HISTORY_DEPTH` entries, about 1.5 KiB -- so
/// two `MAX_PARTICIPANTS`-wide role arrays plus `[Frame; FRAME_CAPACITY]` came
/// to roughly 30 KiB of a component's 64 KiB stack, held for the entire life of
/// the plane while `provision`, `broker`, and their callees each layered their
/// own frames on top. That overflowed into the `.data` immediately below the
/// stack, and the first symptom was not a fault but silent corruption:
/// `BUFFER_FACTORY_CACHE` read back as garbage, so `shared_buffer_create` was
/// refused `ungranted` and provisioning failed with a message that named the
/// wrong cause. B70 moved them to `.bss`, which fixed that: the storage is the
/// same bytes either way, but there the linker reserves it and the image
/// accounts for it rather than it competing with every frame the dispatch loop
/// pushes.
///
/// What `.bss` could not fix is that the reservation was for the contract's
/// worst case in *every* generation carrying this component. Ten fixtures ship
/// fabric-service and not one declares the role ceilings: the largest,
/// `sel4-boot.zti`, declares three publishers and four subscribers with a
/// summed frame demand of 22 against a 32-frame table. So the bytes were
/// reserved against a graph no generation had, and every one of them paid the
/// reservation at build time — measured as 29960 bytes of `.bss` plus `.data`,
/// 145912 down to 115952 on `sel4-boot`.
///
/// They are now allocated from the task-private region, sized from the same
/// participant rows `provision` reads. Three properties are preserved exactly:
///
/// * **Still claimed once, and now checked by the compiler.** Each plane entry
///   point takes the tables by value and passes ordinary `&mut` borrows down, so
///   nothing below can reach the storage twice — that part is the borrow
///   checker's, where before it rested on a `static mut` and a hand-written
///   soundness argument. The latch survives as a *liveness* check rather than a
///   soundness one: a second claim would allocate a second set of tables and
///   silently provision into whichever the caller happened to hold, so it fails
///   instead. This component is single-threaded on every plane it runs — no
///   fixture gives it `extraThreads` — so there is no second thread for either
///   the latch or the borrows to race with.
/// * **Still bounded before anything is provisioned.** The declared counts are
///   admitted against the contract ceilings *before* a table is allocated, so a
///   graph promising more than this component can hold is refused rather than
///   discovered when the table is full — and refused identically to how it was
///   when the ceilings were the array sizes.
/// * **Still fixed for the life of the plane.** Nothing here grows with
///   traffic. The tables are sized once from the graph and never resized, so
///   a sample cannot cause an allocation.
///
/// Neither the declared stack nor any authority check moves for this.
struct StreamTables {
    publishers: alloc::boxed::Box<[Option<Publisher>]>,
    subscribers: alloc::boxed::Box<[Option<Subscriber>]>,
    frames: alloc::boxed::Box<[Frame]>,
}

/// Set by the one successful [`claim_stream_tables`] call.
static mut STREAM_TABLES_CLAIMED: bool = false;

/// How large each table must be for the graph this generation declared.
///
/// One pass over the participant rows, counting only the routes this service
/// carries — the same `local_route_index` filter `provision` applies, so a
/// component on a call or operation route contributes no stream storage.
struct DeclaredCapacity {
    publishers: usize,
    subscribers: usize,
    frames: usize,
}

/// Measure the declared graph, and refuse one this component cannot hold
/// (B70/CP2).
///
/// The refusals are the ceilings that used to be the array sizes, so a graph
/// admitted before C10.4 is admitted now and one refused before is refused now
/// — and strictly earlier than before, since a graph this component cannot hold
/// is now refused before its storage exists rather than before an edge is
/// handed out. The role bounds are `MAX_ROLE_PARTICIPANTS`, which is what
/// `validate_declared_limits` admits a graph against on the builder's side.
///
/// The builder checks all three for every manifest it renders, but a component
/// built against a generation this repository did not produce has no such
/// guarantee — and the failure the frame bound protects against is a deadlock,
/// which is the one class worth spending a boot-time check on: a table smaller
/// than the summed declared depths lets the rings fill while no frame is free,
/// and with the stalled subscriber holding its ring and the publishers blocked
/// nothing would ever wake the fabric again.
///
/// Sums the *declared* `historyDepth`, which is exactly what
/// `resolve_fabric_profile`'s `ring_capacity` sums, and deliberately not the
/// `MIN_RING_SLOTS`-floored depth `provision_edge` allocates a ring at. The two
/// differ for any participant declaring a shallower history than the ring's
/// structural minimum, and using the floored figure here would refuse graphs
/// the builder accepts — a component rejecting a generation the toolchain
/// certified. The floor costs ring *slots*, which the fabric does not own; a
/// frame is referenced once per queued sample, so the declared depth is what
/// bounds this table.
///
/// A subscriber history is not the only thing that pins a frame, and this is the
/// part the old fixed table hid. A `retained` publisher holds its last
/// `retainedDepth` samples for late joiners — `retain_for_late_joiners` takes a
/// reference per entry and `release_retained` drops them at teardown — so those
/// frames are live *concurrently* with every subscriber's queue rather than
/// instead of it. `FRAME_CAPACITY` covered both because it was one number large
/// enough for the contract's worst case; a table sized only from subscriber
/// depths would be short by exactly the retained total, and would present as
/// admission refusing a sample with no free frame — the deadlock the bound
/// exists to make unreachable. Every shipped fixture declares retained
/// publishers, so this is the live case rather than a hypothetical one.
///
/// `retained_depth.max(1)` mirrors `provision_edge`'s own
/// `StreamHistory::new(qos.retained_depth.max(1))`: a retained publisher
/// declaring zero depth still gets a one-entry ring there, so sizing must
/// charge it one.
fn declared_capacity(rows: &[slime_components::fabric_self_view::Row]) -> DeclaredCapacity {
    let mut capacity = DeclaredCapacity {
        publishers: 0,
        subscribers: 0,
        frames: 0,
    };
    // Admission is on the *declared* sum; storage is on what `provision_edge`
    // will actually allocate. The two differ by the ring floor and must not be
    // conflated in either direction — see the note below.
    let mut declared_frames = 0usize;
    for row in rows
        .iter()
        .filter(|row| local_route_index(row.route_index).is_some())
    {
        match row.direction {
            DIRECTION_PUBLISH => {
                capacity.publishers += 1;
                if row.qos.durability as u32 == DURABILITY_RETAINED {
                    let retained = row.qos.retained_depth.max(1) as usize;
                    capacity.frames += retained;
                    declared_frames += retained;
                }
            }
            DIRECTION_SUBSCRIBE => {
                capacity.subscribers += 1;
                declared_frames += row.history_depth;
                // Sized to the history `provision_edge` builds, which is
                // `StreamHistory::new(ring_slots)` after `ring_slots` has been
                // floored at `MIN_RING_SLOTS`. A subscriber declaring a
                // shallower history than that floor still gets a ring that deep,
                // and `fan_out` takes a frame reference per queued entry — so
                // charging the declared depth would leave the table short by the
                // floor's slack, and short exactly where it deadlocks rather
                // than where it reports.
                capacity.frames += row
                    .history_depth
                    .max(slime_proto::fabric_ring::MIN_RING_SLOTS);
            }
            // A call or operation route declared in the same graph. Not this
            // service's to provision, and not its storage to size.
            _ => {}
        }
    }
    if capacity.publishers > MAX_ROLE_PARTICIPANTS {
        fail(b"declared publishers exceed the role ceiling");
    }
    if capacity.subscribers > MAX_ROLE_PARTICIPANTS {
        fail(b"declared subscribers exceed the role ceiling");
    }
    // Against the *declared* sum, deliberately, which is what the builder
    // admits a graph on. Refusing on the floored figure would reject graphs the
    // toolchain certified — a component second-guessing the generation that
    // composed it — while the table is heap-allocated and so is bounded by the
    // declared private-memory quota rather than by this constant.
    if declared_frames > MAX_FRAMES {
        fail(b"declared frame demand exceeds the frame table");
    }
    capacity
}

/// Allocate the plane's tables for the graph this generation declared.
///
/// Reads the participant rows itself rather than taking them, because the two
/// callers that need the tables read the graph at different points and a table
/// sized from a *different* read than the one `provision` uses would be sized
/// from a graph that had changed underneath it. The root answers the same rows
/// every time, so this is one extra read rather than a second statement of the
/// graph.
fn claim_stream_tables() -> StreamTables {
    // SAFETY: a component is single-threaded and this is the only reference
    // taken to the latch.
    unsafe {
        if *core::ptr::addr_of!(STREAM_TABLES_CLAIMED) {
            fail(b"stream tables claimed twice");
        }
        *core::ptr::addr_of_mut!(STREAM_TABLES_CLAIMED) = true;
    }
    let mut rows = slime_components::fabric_self_view::EMPTY_ROWS;
    let Ok(count) = slime_components::fabric_self_view::rows(&mut rows) else {
        fail(b"fabric graph read did not complete");
    };
    let capacity = declared_capacity(&rows[..count]);
    // `try_reserve` then fill, rather than `vec![...]`: an allocation this
    // component's declared quota cannot serve must be a named refusal on the
    // console, not `handle_alloc_error`'s abort. The plane cannot run without
    // its tables, so the outcome is still fatal — but a fatal this component
    // reports is diagnosable, and one the allocator aborts on is not.
    let mut publishers = alloc::vec::Vec::new();
    if publishers.try_reserve_exact(capacity.publishers).is_err() {
        fail(b"publisher table exceeds the declared private-memory quota");
    }
    publishers.resize_with(capacity.publishers, || None);
    let mut subscribers = alloc::vec::Vec::new();
    if subscribers.try_reserve_exact(capacity.subscribers).is_err() {
        fail(b"subscriber table exceeds the declared private-memory quota");
    }
    subscribers.resize_with(capacity.subscribers, || None);
    let mut frames = alloc::vec::Vec::new();
    if frames.try_reserve_exact(capacity.frames).is_err() {
        fail(b"frame table exceeds the declared private-memory quota");
    }
    frames.resize(capacity.frames, Frame::EMPTY);
    slime_rt::debug_write(b"[fabric] tables sized from the declared graph publishers=");
    write_u32(capacity.publishers as u32);
    slime_rt::debug_write(b" subscribers=");
    write_u32(capacity.subscribers as u32);
    slime_rt::debug_write(b" frames=");
    write_u32(capacity.frames as u32);
    slime_rt::debug_write(b" ceilings=");
    write_u32(MAX_ROLE_PARTICIPANTS as u32);
    slime_rt::debug_write(b"/");
    write_u32(MAX_FRAMES as u32);
    slime_rt::debug_write(b"\n");
    StreamTables {
        publishers: publishers.into_boxed_slice(),
        subscribers: subscribers.into_boxed_slice(),
        frames: frames.into_boxed_slice(),
    }
}

/// Prove the root answers this component the *whole* graph (B70/CP2).
///
/// Compared against `FABRIC_PARTICIPANTS` while both statements of the graph
/// existed, because agreement between two independently-derived tables was the
/// only evidence available before the consumers migrated. The consumers have
/// migrated and the table is gone, so the row count this component expects can
/// no longer be derived from anywhere but the reply -- and a reply checked
/// against a number read out of that same reply asserts nothing. The cardinality
/// claim is therefore dropped rather than restated circularly.
///
/// What survives is the property the holder scope actually grants, and it is
/// checked in the one direction that stays independent of the root: this
/// component declares *no* participant row on any plane -- it is the graph's
/// `fabricComponent`, never a participant -- so every row it can see belongs to
/// somebody else. A non-empty answer none of whose rows are its own is exactly
/// what whole-table scope means here, and `component_identity` folding a name
/// this component spells itself is what makes the check independent.
fn prove_graph_read() {
    let mut rows = slime_components::fabric_self_view::EMPTY_ROWS;
    let count = slime_components::fabric_self_view::rows(&mut rows)
        .unwrap_or_else(|_| fail(b"graph read refused for the declared holder"));
    // An empty answer and a refused one are different failures, and only the
    // refusal surfaces itself. Every plane declares participants, so zero rows to
    // the holder means the scope collapsed, not that the graph is empty.
    if count == 0 {
        fail(b"graph read answered the declared holder no rows");
    }
    let own = boot_contracts::fabric_graph::component_identity("fabric-service");
    for row in rows.iter().take(count) {
        if row.component_identity == own {
            fail(b"graph read named the broker as a participant");
        }
    }
    slime_rt::debug_write(b"[fabric] graph read answers the declared holder the whole graph\n");
}

fn main(_startup_arg: u32) {
    prove_graph_read();
    if slime_components::generation_composition::is(BootAction::Boot) {
        boot_graph();
        return;
    }
    if slime_components::generation_composition::is(BootAction::Traffic) {
        traffic_graph();
        return;
    }
    if slime_components::generation_composition::is(BootAction::Visibility) {
        visibility_broker::run();
        return;
    }
    if slime_components::generation_composition::is(BootAction::Matrix) {
        matrix_broker::run();
        return;
    }
    if slime_components::generation_composition::is(BootAction::Call) {
        run_call_plane();
        return;
    }
    if slime_components::generation_composition::is(BootAction::Operation) {
        run_operation_plane();
        return;
    }
    let limits = load_runtime_limits();
    let routes: [[u8; 32]; ROUTE_COUNT] = [
        route_identity(
            ROUTE_NAMES[0],
            &telemetry_stream::INTERFACE_IDENTITY,
            CONTRACT_KIND_STREAM,
        ),
        route_identity(
            ROUTE_NAMES[1],
            &diagnostics_stream::INTERFACE_IDENTITY,
            CONTRACT_KIND_STREAM,
        ),
    ];
    let type_tags: [u64; ROUTE_COUNT] = [telemetry_stream::TYPE_TAG, diagnostics_stream::TYPE_TAG];

    let mut clients = control_clients();
    let mut tables = claim_stream_tables();
    let publishers = &mut *tables.publishers;
    let subscribers = &mut *tables.subscribers;
    let frames = &mut *tables.frames;

    provision(&mut clients, &routes, publishers, subscribers);
    slime_rt::debug_write(b"[fabric] every declared stream edge provisioned\n");

    broker(
        &routes,
        &mut clients,
        &type_tags,
        publishers,
        subscribers,
        frames,
        &limits,
    );
    // Outlive every participant holding one of this component's rings. Exiting
    // first reclaims the fabric's shared-buffer charges, and a loan mapping
    // torn out from under a task still executing against it faults that task —
    // observed as an execute fault at a null address in both subscribers, right
    // after `holder reclaimed`. The supervision handles the generation granted
    // for loan addressing answer "is that task gone", so they order the
    // teardown too.
    await_participants(publishers, subscribers);
    slime_rt::debug_write(b"[fabric] stream plane complete\n");
}

/// C8.6's bounded call plane, in its own frame.
///
/// `#[inline(never)]` is load-bearing rather than stylistic. `Broker::new`
/// returns a `Broker` by value — a table of `LIMIT_IN_FLIGHT` `Call` records —
/// and LLVM sinks that temporary into the *caller's* frame. Inlined into
/// `main`, the call and operation brokers' two temporaries were allocated in
/// `main`'s prologue and held for the whole run, on every plane: `main` claimed
/// ~57 KiB of a 64 KiB stack before executing a single statement, and the
/// stream arm — which constructs neither broker — paid for both. That left
/// roughly a kilobyte of headroom, so `provision` wrote its frame straight
/// through the bottom of the stack and into `.bss`, corrupting the tables
/// below it.
///
/// Splitting each arm into its own function makes the allocation live only
/// while that plane runs, which is the plane that actually needs it.
#[inline(never)]
fn run_call_plane() {
    let controls = request_response_controls(CALL_PLANE);
    call_broker::Broker::new(
        buffer_factory_slot(),
        [Some(controls.clients[0]), Some(controls.clients[1])],
        [Some(CALL_PLANE[0]), Some(CALL_PLANE[1])],
        controls.server,
        CALL_PLANE[2],
        controls.time,
        // Client A, client B, server, then the clock's own handle: a separately
        // declared instance whose exit the server's handle does not report
        // (B76).
        [
            Some(supervision_slot_for(b"fabric-call-client")),
            Some(supervision_slot_for(b"fabric-call-client-b")),
            Some(supervision_slot_for(b"fabric-call-server")),
            Some(supervision_slot_for(b"fabric-call-time")),
        ],
        [None, None],
        b"notification:fabric-service-parameters-ready",
        load_runtime_limits(),
    )
    .run();
    slime_rt::debug_write(b"[fabric] call plane complete\n");
}

/// C8.7's bounded operation plane, in its own frame. See [`run_call_plane`] for
/// why this is a separate `#[inline(never)]` function rather than an arm of
/// `main`.
#[inline(never)]
fn run_operation_plane() {
    let controls = request_response_controls(OPERATION_PLANE);
    operation_broker::Broker::new(
        operation_broker::Wiring {
            clients: controls.clients,
            server: controls.server,
            time_control: controls.time,
            replacement_control: control_slot_of(b"fabric-op-client-b-restart")
                .unwrap_or_else(|| fail(b"operation replacement control endpoint")),
            replacement_start: Some(operation_replacement_start_slot()),
            backup_route: slime_rt::resolve_binding(b"fabric-op-client-backup")
                .unwrap_or_else(|_| fail(b"operation backup route endpoint")),
            supervision: [
                supervision_slot_for(b"fabric-op-client"),
                supervision_slot_for(b"fabric-op-client-b"),
                supervision_slot_for(b"fabric-op-server"),
            ],
            replacement_supervision: supervision_slot_for(b"fabric-op-client-b-restart"),
        },
        load_runtime_limits(),
    )
    .run();
    slime_rt::debug_write(b"[fabric] operation plane complete\n");
}

/// Block until every participant this service provisioned a ring for has ended.
///
/// The set comes from the provisioned arrays rather than from the generated
/// supervision table the walk used to read. The two agreed, but as an
/// observation rather than a construction: a row existed in that table for every
/// component holding a ring or an interposition chain
/// (`resolve_fabric_profile`'s `holders`), and an entry exists here for every
/// component that asked for and received a ring role — which coincide only
/// because every declared ring participant does in fact provision. Checked on
/// each plane reaching a teardown: on `stream` and `qos` the two sets are equal,
/// and on `traffic` the table's extra rows are precisely the components that
/// never provision.
///
/// That last point is why this replaces a hardcoded skip list rather than merely
/// restating one. The traffic walk named `fabric-proxy` and `fabric-observer` as
/// literals to avoid waiting forever on two tasks the milestone requires to stay
/// parked; iterating what was actually provisioned makes their absence
/// structural, so a composition that parks a *different* component needs no edit
/// here.
///
/// Each entry carries the supervision handle its provisioning installed, so this
/// resolves nothing: a native Endpoint reports no peer death, and the handle is
/// the only observation that distinguishes an exited participant from a quiet
/// one.
fn await_participants(publishers: &[Option<Publisher>], subscribers: &[Option<Subscriber>]) {
    let slots = publishers
        .iter()
        .filter_map(|entry| {
            entry
                .as_ref()
                .and_then(|publisher| publisher.supervision_slot)
        })
        .chain(subscribers.iter().filter_map(|entry| {
            entry
                .as_ref()
                .and_then(|subscriber| subscriber.supervision_slot)
        }));
    for supervision in slots {
        while let Ok(None) = slime_rt::supervision_status(supervision) {
            slime_rt::yield_now();
        }
    }
}

/// C8.10 full-graph boot: run the stream plane, which is this task's whole
/// share of the graph.
///
/// **Why three tasks and not one.** The generation declares peaks of 8, 7, and 9
/// wake sources for the stream, call, and operation workers against a
/// `MAX_WAIT_SOURCES` of 9. One task cannot park on 24 sources, so the split is
/// forced by the kernel bound, and the partition itself is a validated
/// generation fact (`FABRIC_WORKERS`) rather than a choice made here.
///
/// **Why this task does not spawn the other two.** A worker authenticates each
/// client by the control endpoint its request arrived on, and those endpoints
/// are generation-declared: the root installs both halves before either task
/// runs. Neither half can be handed on afterwards — `grant_crosses_spawn`
/// excludes endpoint grants from a spawn request, and `nth_declared_capability`
/// skips endpoint-kind minted bindings — so a fabric-spawned worker would start
/// with an empty control block. Each worker's supervision handles have the same
/// constraint from the other direction: they name call and operation
/// participants, which only init ever holds. Init therefore spawns all three
/// (B55).
///
/// The stream plane stays in this task rather than becoming a fourth binary: it
/// is the one plane whose provisioning path four earlier gates run, and moving
/// ~1500 lines of it would put that evidence at risk to gain nothing the
/// declared partition does not already state.
fn boot_graph() {
    let routes: [[u8; 32]; ROUTE_COUNT] = [
        route_identity(
            ROUTE_NAMES[0],
            &telemetry_stream::INTERFACE_IDENTITY,
            CONTRACT_KIND_STREAM,
        ),
        route_identity(
            ROUTE_NAMES[1],
            &diagnostics_stream::INTERFACE_IDENTITY,
            CONTRACT_KIND_STREAM,
        ),
    ];
    let mut clients = control_clients();
    // The declared interposition proxy holds a real control endpoint but is a
    // chain hop, not a route participant — under boot it parks without ever
    // contacting this broker (`fabric-proxy::main`'s `park_only` arm), which is
    // exactly what the milestone requires of it. `provision`'s completion
    // condition is "every registered client answered", and a client that never
    // sends anything can never satisfy that through the normal request path,
    // so it is a graph fact declared here rather than something `provision`
    // discovers per request. The unauthorized probe is not this: it actively
    // sends a request and is denied, so it reaches `answered = true` on its
    // own.
    if let Some(proxy) = clients
        .iter_mut()
        .flatten()
        .find(|client| client.component == b"fabric-proxy")
    {
        proxy.answered = true;
    }
    let mut tables = claim_stream_tables();
    let publishers = &mut *tables.publishers;
    let subscribers = &mut *tables.subscribers;
    // `provision` answers each control endpoint and returns once every one has
    // been answered or died. The unauthorized probe holds a real control
    // endpoint and asks for an ungranted route, so its denial is what answers
    // it; the proxy is pre-marked above. So returning here already is
    // "provisioned and idle with no traffic", the required evidence the graph
    // reached its declared idle state rather than merely existing.
    //
    // Each edge is announced as it is provisioned, so the transcript shows the
    // whole graph.
    provision(&mut clients, &routes, publishers, subscribers);
    slime_rt::debug_write(b"[fabric] idle: parked on control endpoints\n");
    // The gate's exit condition: every declared edge already minted, nothing
    // left to answer, so the worker just holds its control set idle.
    loop {
        slime_rt::yield_now();
    }
}

/// Drive the C8.13 concurrent traffic plane's stream class: the identical
/// declared partition [`boot_graph`] proves collision-free, now carrying real
/// stream traffic instead of parking.
///
/// Only reachable for the authenticated `traffic` action declared by
/// `contracts/generation/v1/fixtures/sel4-traffic.zti`, which is `sel4-boot.zti`
/// with `bootAction` and `generation` changed plus the additional grants real
/// traffic needs (§ the fixture's own history).
///
/// **Why not the default composition below.** That composition assumes every
/// declared route participant answers `provision` through the normal request
/// path. The declared interposition proxy is real here too -- a chain hop, not
/// a route participant -- and under `"traffic"` it still parks without ever
/// contacting this broker (`fabric-proxy::main`'s `full_graph_active` arm), so
/// it needs the identical pre-mark [`boot_graph`] applies.
///
/// **Why not `boot_graph` itself.** Its whole point is provisioning without
/// traffic: it returns as soon as every edge is handed out and never touches
/// `broker`, then parks forever. This calls `broker` for the real relay loop
/// and then waits for every participant's task to end -- the same completion
/// the default composition uses -- rather than looping.
fn traffic_graph() {
    let routes: [[u8; 32]; ROUTE_COUNT] = [
        route_identity(
            ROUTE_NAMES[0],
            &telemetry_stream::INTERFACE_IDENTITY,
            CONTRACT_KIND_STREAM,
        ),
        route_identity(
            ROUTE_NAMES[1],
            &diagnostics_stream::INTERFACE_IDENTITY,
            CONTRACT_KIND_STREAM,
        ),
    ];
    let type_tags: [u64; ROUTE_COUNT] = [telemetry_stream::TYPE_TAG, diagnostics_stream::TYPE_TAG];
    let mut clients = control_clients();
    // Neither the proxy nor the observer ever contacts this broker under
    // `"traffic"` (`fabric_boot::full_graph_active` parks both without
    // requesting a role -- see `fabric-observer::main` for why the observer
    // cannot request its declared subscription here the way `boot_graph`'s
    // parked copy safely does), so both are pre-marked the same way
    // `boot_graph` pre-marks the proxy alone.
    for client in clients.iter_mut().flatten().filter(|client| {
        client.component == b"fabric-proxy" || client.component == b"fabric-observer"
    }) {
        client.answered = true;
    }
    let limits = load_runtime_limits();
    let mut tables = claim_stream_tables();
    let publishers = &mut *tables.publishers;
    let subscribers = &mut *tables.subscribers;
    let frames = &mut *tables.frames;

    provision(&mut clients, &routes, publishers, subscribers);
    slime_rt::debug_write(b"[fabric] traffic: every declared stream edge provisioned\n");

    broker(
        &routes,
        &mut clients,
        &type_tags,
        publishers,
        subscribers,
        frames,
        &limits,
    );
    // Neither the proxy nor the observer ever contacts this broker under
    // `"traffic"` (both parked above without requesting a role), so neither
    // dies either; waiting on either here would hang forever on a task the
    // milestone requires to stay parked. They are absent from the provisioned
    // arrays for exactly that reason, so `await_participants` skips them
    // structurally — this used to name both as literals.
    await_participants(publishers, subscribers);
    slime_rt::debug_write(b"[fabric] traffic stream plane complete\n");
}

struct RequestResponseControls {
    clients: [u32; 2],
    server: u32,
    time: u32,
}

/// The four participants one request/response plane declares, in the role order
/// its broker takes them: client A, client B, server, clock.
///
/// A role list rather than a slot list. The table this replaced was the
/// builder's own control ordering, and the broker recovered each slot as
/// `FABRIC_FIRST_CONTROL_SLOT + position` -- a component restating the layout
/// rule that produced the numbers, so a reordered grant moved every slot under
/// it silently. Each control edge is a declared grant named
/// `<component>-control` by every manifest that declares the plane, so the name
/// answers it outright and the order here is only which role the broker binds
/// each to.
const CALL_PLANE: [&[u8]; 4] = [
    b"fabric-call-client",
    b"fabric-call-client-b",
    b"fabric-call-server",
    b"fabric-call-time",
];
const OPERATION_PLANE: [&[u8]; 4] = [
    b"fabric-op-client",
    b"fabric-op-client-b",
    b"fabric-op-server",
    b"fabric-op-time",
];

/// Resolve one request/response plane's authenticated control slots by grant
/// name. A plane this generation does not declare cannot be reached here --
/// `main` dispatches on the authenticated boot action -- so an unresolved name
/// is a composition defect rather than a plane that is simply absent.
fn request_response_controls(plane: [&[u8]; 4]) -> RequestResponseControls {
    let slot = |component: &[u8]| {
        control_slot_of(component).unwrap_or_else(|| fail(b"request/response control missing"))
    };
    RequestResponseControls {
        clients: [slot(plane[0]), slot(plane[1])],
        server: slot(plane[2]),
        time: slot(plane[3]),
    }
}

/// The slot this component's own control endpoint to `component` occupies, or
/// `None` when this generation declares no such edge.
///
/// `<component>-control` is the name every manifest gives a participant's
/// control edge, the same convention `supervision_slot_for` relies on for
/// `<component>-supervision`. Scoped by the root to this instance's own
/// bindings, so a name that resolves is an edge this broker really holds.
pub(crate) fn control_slot_of(component: &[u8]) -> Option<u32> {
    const SUFFIX: &[u8] = b"-control";
    let mut name = [0u8; 64];
    let end = component.len() + SUFFIX.len();
    if end > name.len() {
        fail(b"control name exceeds bound");
    }
    name[..component.len()].copy_from_slice(component);
    name[component.len()..end].copy_from_slice(SUFFIX);
    slime_rt::resolve_binding(&name[..end]).ok()
}

/// Every participant this broker can carry a stream control edge for.
///
/// A roster of component *roles*, not of slots: which of these a generation
/// declares varies by plane -- five under the standalone stream, QoS, and
/// visibility fixtures, seven under the full-graph boot, the traffic plane, and
/// the matrix -- and each entry is registered only if the root resolves its
/// control grant for this instance. Absence is therefore an ordinary answer
/// rather than a hole in a table, which is what lets one binary serve every
/// composition that names a subset.
///
/// This replaces `FABRIC_CLIENTS`, whose *position* carried the slot number.
/// Nothing positional survives: each entry resolves its own slot, so a
/// composition that adds, drops, or reorders a control edge needs no change
/// here and cannot shift another participant's authority.
const STREAM_CONTROL_ROSTER: [&[u8]; 10] = [
    b"fabric-publisher",
    b"fabric-subscriber",
    b"fabric-intruder",
    b"fabric-publisher-b",
    b"fabric-subscriber-b",
    b"fabric-observer",
    b"fabric-probe",
    b"fabric-proxy",
    // C9.6's robot graph: the sensor publishes and the controller subscribes on
    // the same declared `telemetry` route the C8 planes use. Adding a role here
    // changes no other composition — an entry is registered only if the root
    // resolves its control grant for *this* instance, so every generation that
    // names neither continues to see the same eight.
    b"robot-sensor",
    b"robot-controller",
];
pub(crate) const MAX_STREAM_CONTROLS: usize = STREAM_CONTROL_ROSTER.len();

/// The control endpoints this generation actually granted this broker, each
/// paired with the component identity that endpoint authenticates.
///
/// A component the generation does not declare an edge for is absent rather
/// than present-and-unreachable, so `provision`'s completion condition still
/// means "every declared control endpoint has been answered".
pub(crate) fn control_clients() -> [Option<Client>; MAX_STREAM_CONTROLS] {
    let mut clients = [const { None }; MAX_STREAM_CONTROLS];
    for (entry, component) in clients.iter_mut().zip(STREAM_CONTROL_ROSTER) {
        let Some(control_slot) = control_slot_of(component) else {
            continue;
        };
        *entry = Some(Client {
            control_slot,
            component,
            answered: false,
        });
    }
    clients
}

/// C8.3 provisioning round: mint both halves of every declared edge and move
/// each participant its exact narrowed role.
///
/// The fabric keeps the opposite half of every edge — that is what lets it
/// broker later — and hands out only the participant's side. Requests are
/// answered until every control endpoint has been answered or died, so the
/// service never proceeds to brokering with an unclaimed declared edge.
fn provision(
    clients: &mut [Option<Client>],
    routes: &[[u8; 32]; ROUTE_COUNT],
    publishers: &mut [Option<Publisher>],
    subscribers: &mut [Option<Subscriber>],
) {
    // Read once, before any client is answered.
    //
    // `graph_read` stages its reply through this component's single transfer
    // window, which `provision_edge` also uses to hand out role descriptors. A
    // read inside the loop had the two contending for one window and left three
    // of six edges unprovisioned — so hoisting is not an optimization here, it
    // is what keeps the two uses disjoint.
    let mut graph_rows = slime_components::fabric_self_view::EMPTY_ROWS;
    let Ok(row_count) = slime_components::fabric_self_view::rows(&mut graph_rows) else {
        fail(b"fabric graph read did not complete");
    };
    // The declared histories were admitted against the frame table before it
    // was allocated (`declared_capacity`), which is strictly earlier than this
    // point: a graph this component cannot hold is refused before it has
    // storage, let alone before an edge is handed out.
    while clients.iter().flatten().any(|client| !client.answered) {
        // Sweep every unanswered control endpoint through its non-blocking ABI
        // first. Only when all of them would block is parking correct: probing
        // before parking is what closes the lost-wakeup window.
        let mut progressed = false;
        for client in clients
            .iter_mut()
            .flatten()
            .filter(|client| !client.answered)
        {
            let mut message = [0u8; MAX_MSG];
            let mut received = [0u64; MAX_CAPS_PER_MSG];
            let control_slot = client.control_slot;
            let length = match slime_rt::recv(control_slot, &mut message, &mut received) {
                ERR_WOULDBLOCK => continue,
                // Name the slot and the code. A control endpoint is the one
                // authority binding this service has, so "which one, refused
                // how" is the whole diagnosis; a bare reason string sent the
                // reader guessing between a dead peer and a bad capability.
                error if error < 0 => {
                    slime_rt::debug_write(b"[fabric] control recv slot=");
                    write_u32(control_slot);
                    slime_rt::debug_write(b" error=");
                    write_i64(error);
                    slime_rt::debug_write(b"\n");
                    fail(b"control recv")
                }
                n => n as usize,
            };
            progressed = true;
            client.answered = true;
            release_received(&received);
            let request = match WireFabricRequest::decode(&message[..length.min(MAX_MSG)]) {
                Some(request) if length == REQUEST_LEN && valid_fabric_request(&request) => request,
                _ => {
                    deny(control_slot, &routes[0], STATUS_BAD_REQUEST);
                    continue;
                }
            };

            // The request's own route name, direction, and type identity are
            // read here only to be discarded. Authority comes from the caller's
            // control endpoint and the generation graph, so a component
            // supplying the exact strings of a route it was never granted gets
            // the same answer as one supplying nothing.
            let _ = (request.direction, request.type_identity, request.route_name);

            if declared_edges(&graph_rows[..row_count], client.component) == 0 {
                slime_rt::debug_write(b"[fabric] ungranted component denied: ");
                slime_rt::debug_write(client.component);
                slime_rt::debug_write(b"\n");
                deny(control_slot, &routes[0], STATUS_NOT_GRANTED);
                continue;
            }

            // One request provisions every edge the graph declares for this
            // component: a participant on two routes receives two roles, each
            // narrowed on its own. The client learns how many to expect from
            // the same graph, so no count crosses as authority. The rows come
            // from the graph resource, in its identity-sorted order; nothing
            // downstream may depend on provisioning order, and the descriptor
            // demultiplexing in `pump_publisher` is what makes that true.
            let identity = boot_contracts::fabric_graph::component_identity(
                core::str::from_utf8(client.component)
                    .unwrap_or_else(|_| fail(b"component name is not utf-8")),
            );
            for row in graph_rows[..row_count].iter() {
                if row.component_identity != identity {
                    continue;
                }
                // A route this service does not carry is not this service's to
                // provision. Call and operation routes are declared in the same
                // graph and owned by C8.6/C8.7; skipping them here is why a
                // component on one holds no stream authority by accident.
                let Some(route) = local_route_index(row.route_index) else {
                    continue;
                };
                provision_edge(
                    client.component,
                    control_slot,
                    &routes[route],
                    route,
                    row,
                    publishers,
                    subscribers,
                    false,
                );
            }
        }
        if !progressed {
            slime_rt::yield_now();
        }
    }
}

/// Receiver authority for a provisioned ring.
///
/// The ordinary planes use the supervision handle init granted this broker.
/// C9.6 cannot: `robot-controller-supervision` is minted by and held by
/// `robot-supervisor`, and `validate_supervision_binding_names` rejects a handle
/// minted by any owner other than the supervised instance's owner. The rejected
/// alternative was therefore a second fabric-held supervision binding; it is
/// not expressible without lying about ownership.
///
/// A declared endpoint is the honest alternative the root permits. Its loan
/// receiver lookup follows the endpoint's peer through `LaunchedInstances`, so
/// after `PEER_ENDPOINTS.install_instance` installs the generation-owned object
/// into a replacement, this same broker slot names the replacement task rather
/// than the predecessor. The grant must carry `RIGHT_TRANSFER`, because a loan
/// crosses authority; the robot fixture therefore must declare
/// `robot-controller-control` with `transferable = true`.
fn ring_receiver_slot(component: &'static [u8], control_slot: u32) -> u32 {
    if component == b"robot-controller"
        && slime_components::generation_composition::is(BootAction::RobotRuntime)
    {
        control_slot
    } else {
        supervision_slot_for(component)
    }
}

/// Supervision is also the liveness proof for blocking control events and final
/// teardown. The restartable controller's handle is intentionally absent from
/// this holder, so its role uses ring/control observations instead.
fn role_supervision_slot(component: &'static [u8]) -> Option<u32> {
    if component == b"robot-controller"
        && slime_components::generation_composition::is(BootAction::RobotRuntime)
    {
        None
    } else {
        Some(supervision_slot_for(component))
    }
}

/// Provision one v2 shared ring on the participant's already-installed control
/// edge. `capability_delegate` narrows/copies the buffer handle and correlates
/// it with the typed descriptor in one direct Endpoint transaction.
fn provision_edge(
    component: &'static [u8],
    control_slot: u32,
    route: &[u8; 32],
    route_index: usize,
    row: &slime_components::fabric_self_view::Row,
    publishers: &mut [Option<Publisher>],
    subscribers: &mut [Option<Subscriber>],
    reprovisioned: bool,
) {
    // Direction, QoS, and KEEP_LAST depth all come off the one graph row this
    // edge was selected by (B70/CP2). They were three lookups into two
    // generated tables joined by (component, route name) plus a second graph
    // read, which could disagree about which participant they described; here
    // they are three fields of one record and cannot.
    let direction = row.direction;
    let qos = row.qos;
    // `Row::history_depth` and `qos.history_depth` are the same declared
    // number, so both directions read the depth the generation gave *this*
    // participant rather than one direction reading a table and the other the
    // graph.
    let ring_slots = match direction {
        DIRECTION_PUBLISH | DIRECTION_SUBSCRIBE => row.history_depth,
        _ => fail(b"stream route declares a non-stream direction"),
    };
    let (ready_slot, credit_slot) = notification_slots(component, ROUTE_NAMES[route_index]);
    let ring_slots = ring_slots.max(slime_proto::fabric_ring::MIN_RING_SLOTS);
    let ordinal = publishers.iter().filter(|entry| entry.is_some()).count()
        + subscribers.iter().filter(|entry| entry.is_some()).count();
    let ring_base = RING_BASE + ordinal as u64 * PAGE;
    let buffer = slime_rt::shared_buffer_create(buffer_factory_slot(), 1, true)
        .unwrap_or_else(|_| fail(b"stream ring create"));
    if slime_rt::shared_buffer_map(buffer.slot, ring_base, 0, PAGE, true) != ERR_SUCCESS {
        fail(b"stream ring map");
    }
    let bytes = unsafe { core::slice::from_raw_parts_mut(ring_base as *mut u8, PAGE as usize) };
    Ring::format(
        bytes,
        if route_index == 0 {
            telemetry_stream::TYPE_TAG
        } else {
            diagnostics_stream::TYPE_TAG
        },
        ring_slots,
    )
    .unwrap_or_else(|_| fail(b"stream ring format"));
    // The ring crosses as a *writable loan*, not as the buffer handle: a
    // shared-buffer handle is owner-bound, so a peer handed one is refused
    // when it maps. A loan is the primitive for exactly this — the fabric
    // stays the region's owner and accountable holder, and the participant
    // gets a receiver-bound reference over the declared range. Writable
    // because the two peers advance disjoint header fields of one ring.
    let receiver_slot = ring_receiver_slot(component, control_slot);
    let loan = slime_rt::shared_buffer_loan(buffer.slot, receiver_slot, 0, PAGE, true)
        .unwrap_or_else(|_| fail(b"stream ring loan"));
    let descriptor = WireCapabilityTransfer {
        magic: CAPABILITY_TRANSFER_MAGIC,
        version: FORMAT_VERSION,
        status: 0,
        flags: FLAG_RETAIN_TRANSFER,
        object_kind: slime_proto::capability_transfer::OBJECT_KIND_SHARED_BUFFER_LOAN,
        direction,
        rights_mask: RIGHT_BUFFER_MAP | RIGHT_BUFFER_WRITE,
        route_identity: *route,
    };
    if slime_rt::capability_delegate(
        control_slot,
        loan.slot,
        CapabilityDisposition::Move,
        slime_proto::capability_transfer::OBJECT_KIND_SHARED_BUFFER_LOAN,
        RIGHT_BUFFER_MAP | RIGHT_BUFFER_WRITE,
        &descriptor.encode(),
    ) != ERR_SUCCESS
    {
        fail(b"stream ring delegation");
    }

    match direction {
        DIRECTION_PUBLISH => {
            let free = publishers
                .iter()
                .position(Option::is_none)
                .unwrap_or_else(|| fail(b"publisher table exhausted"));
            publishers[free] = Some(Publisher {
                component,
                control_slot,
                ring_base,
                buffer_slot: buffer.slot,
                loan_id: loan.id,
                ring_slots,
                ready_slot,
                credit_slot,
                route: route_index,
                supervision_slot: role_supervision_slot(component),
                finished: false,
                died: false,
                terminated: false,
                drained: false,
                qos,
                last_assertion_ns: 0,
                retained: StreamHistory::new(qos.retained_depth.max(1) as usize)
                    .unwrap_or_else(|| fail(b"declared retained depth")),
            });
        }
        DIRECTION_SUBSCRIBE => {
            let free = subscribers
                .iter()
                .position(Option::is_none)
                .unwrap_or_else(|| fail(b"subscriber table exhausted"));
            subscribers[free] = Some(Subscriber {
                component,
                control_slot,
                ring_base,
                buffer_slot: buffer.slot,
                loan_id: loan.id,
                ring_slots,
                ready_slot,
                credit_slot,
                route: route_index,
                supervision_slot: role_supervision_slot(component),
                history: StreamHistory::new(ring_slots)
                    .unwrap_or_else(|| fail(b"declared history depth")),
                in_flight: 0,
                ended: false,
                retry_interval_ns: qos.deadline_ns.max(1),
                qos,
                matched_publishers: 0,
                deadline_reported: false,
                liveliness_reported: false,
                retry_count: 0,
                terminal: false,
                last_retry_ns: 0,
            });
        }
        _ => unreachable!(),
    }
    refresh_matches(route_index, publishers, subscribers);
    slime_rt::debug_write(if reprovisioned {
        b"[fabric] reprovisioned "
    } else {
        b"[fabric] provisioned "
    });
    slime_rt::debug_write(component);
    slime_rt::debug_write(b" ");
    slime_rt::debug_write(ROUTE_NAMES[route_index].as_bytes());
    slime_rt::debug_write(if direction == DIRECTION_PUBLISH {
        b" publish ring\n"
    } else {
        b" subscribe ring\n"
    });
}

/// Everything the stale incarnation's subscriber role had not yet delivered to
/// its consumer, keyed by route: whatever the fabric had already written into
/// the ring but the dead task never consumed, followed by whatever was still
/// queued waiting to be written.
///
/// The ring's remaining contents are drained through this broker's own
/// writer-side mapping of the same shared buffer — the dead task's reader-side
/// mapping died with it, so this is the only live view left — and re-admitted
/// into fresh frames the same way `pump_publisher` admits a new sample, so the
/// replacement's fresh ring is populated by the ordinary `deliver` path rather
/// than a second copy of that logic. Drained entries are pushed ahead of
/// whatever `history` still queued: `history` names samples admitted but not
/// yet written to the ring, so it is strictly newer than anything the ring
/// already held.
///
/// Without this, a restart mid-stream would silently drop every sample the
/// fabric had already handed to the dead incarnation's ring but that
/// incarnation had not yet read — which is exactly what let the publisher's
/// already-`finished` record convince `announce_end` the route was drained
/// while the replacement's fresh, empty ring still had nothing to show it.
fn salvage_stale_subscriber(
    component: &'static [u8],
    now_ns: u64,
    publishers: &[Option<Publisher>],
    subscribers: &mut [Option<Subscriber>],
    frames: &mut [Frame],
) -> [Option<StreamHistory>; ROUTE_COUNT] {
    let mut preserved = [None; ROUTE_COUNT];
    for subscriber in subscribers
        .iter_mut()
        .flatten()
        .filter(|subscriber| subscriber.component == component)
    {
        let route = subscriber.route;
        let mut salvaged = StreamHistory::new(subscriber.history.depth())
            .unwrap_or_else(|| fail(b"declared history depth"));
        let publisher_index = publishers
            .iter()
            .position(|entry| {
                entry
                    .as_ref()
                    .is_some_and(|publisher| publisher.route == route)
            })
            .unwrap_or_else(|| fail(b"salvaged ring has no publisher"))
            as u32;
        let type_identity = route_type_tag(route);
        let bytes = unsafe {
            core::slice::from_raw_parts_mut(subscriber.ring_base as *mut u8, PAGE as usize)
        };
        let mut ring = Ring::attach(bytes, type_identity, subscriber.ring_slots)
            .unwrap_or_else(|_| fail(b"salvage ring attach"));
        let mut sequence = 0u64;
        // Reserve the destination *before* consuming. `Ring::consume` advances
        // the shared tail, so checking for a free frame afterwards would already
        // have destroyed the sample it then failed to keep -- and `consume` bumps
        // no loss counter, so the drop would also be unaccounted. Reserving first
        // means an exhausted frame table leaves the remaining samples in the
        // ring, where they are counted as lost below rather than silently
        // discarded.
        while let Some(free) = frames.iter().position(|frame| frame.refs == 0) {
            let mut payload = [0u8; MAX_INLINE_BYTES];
            let (length, last) = match ring.consume(&mut payload) {
                Ok(value) => value,
                Err(RingError::Empty) => break,
                Err(_) => fail(b"salvage ring consume"),
            };
            sequence += 1;
            frames[free] = Frame {
                refs: 1,
                sequence,
                type_identity,
                flags: if last { FLAG_LAST } else { 0 },
                payload,
                payload_len: length,
                buffer_slot: None,
                buffer_len: 0,
                admitted_ns: now_ns,
            };
            if let Some(evicted) = salvaged.push(HistoryEntry {
                sequence,
                publisher: publisher_index,
                slot: free as u32,
                inline: true,
            }) {
                release_frame(evicted.slot as usize, frames);
            }
        }
        while let Some(entry) = subscriber.history.pop() {
            if let Some(evicted) = salvaged.push(entry) {
                release_frame(evicted.slot as usize, frames);
            }
        }
        // Anything still in the old ring dies with it, and the stale
        // subscriber's own pending report would die with the record it lived on.
        // Both are re-attributed to the replacement's history, so a salvage that
        // could not keep everything still reports a gap rather than presenting a
        // truncated stream as complete.
        let mut abandoned = 0u64;
        let mut payload = [0u8; MAX_INLINE_BYTES];
        while ring.consume(&mut payload).is_ok() {
            abandoned = abandoned.saturating_add(1);
        }
        salvaged.note_loss(abandoned, subscriber.history.take_loss());
        preserved[route] = Some(salvaged);
    }
    preserved
}

/// Poll the restartable composition's already-provisioned control endpoints for
/// a replacement task's fresh role request.
///
/// This must remain non-blocking. A blocking receive on any quiet participant
/// would wedge the broker before it could sweep publishers, acknowledgements,
/// or the other control endpoints. `ERR_WOULDBLOCK` therefore preserves the
/// broker's existing `progressed`/yield discipline.
fn reprovision_participants(
    clients: &[Option<Client>],
    routes: &[[u8; 32]; ROUTE_COUNT],
    graph_rows: &[slime_components::fabric_self_view::Row],
    now_ns: u64,
    publishers: &mut [Option<Publisher>],
    subscribers: &mut [Option<Subscriber>],
    frames: &mut [Frame],
) -> bool {
    let mut progressed = false;
    for client in clients
        .iter()
        .flatten()
        .filter(|client| client.answered && client.component == b"robot-controller")
    {
        let mut message = [0u8; MAX_MSG];
        let mut received = [0u64; MAX_CAPS_PER_MSG];
        let length = match slime_rt::recv(client.control_slot, &mut message, &mut received) {
            ERR_WOULDBLOCK => continue,
            error if error < 0 => fail(b"reprovision control recv"),
            length => length as usize,
        };
        release_received(&received);
        let Some(request) = WireFabricRequest::decode(&message[..length.min(MAX_MSG)]) else {
            continue;
        };
        if length != REQUEST_LEN
            || request.magic != FABRIC_REQUEST_MAGIC
            || !valid_fabric_request(&request)
        {
            continue;
        }

        // Authority remains the generation row selected by the authenticated
        // endpoint; request fields are not allowed to choose a different role.
        let _ = (request.direction, request.type_identity, request.route_name);
        let preserved =
            salvage_stale_subscriber(client.component, now_ns, publishers, subscribers, frames);
        reclaim_component(client.component, publishers, subscribers, frames);
        let identity = boot_contracts::fabric_graph::component_identity(
            core::str::from_utf8(client.component)
                .unwrap_or_else(|_| fail(b"component name is not utf-8")),
        );
        for row in graph_rows
            .iter()
            .filter(|row| row.component_identity == identity)
        {
            let Some(route) = local_route_index(row.route_index) else {
                continue;
            };
            provision_edge(
                client.component,
                client.control_slot,
                &routes[route],
                route,
                row,
                publishers,
                subscribers,
                true,
            );
            // Hand the preserved backlog to the freshly-provisioned role: the
            // publisher side is unaware a restart happened at all, so nothing
            // else will ever re-admit these samples.
            if let Some(history) = preserved[route]
                && let Some(subscriber) = subscribers.iter_mut().flatten().find(|subscriber| {
                    subscriber.component == client.component && subscriber.route == route
                })
            {
                subscriber.history = history;
            }
        }
        progressed = true;
    }
    progressed
}

/// Reclaim every resource held by one stale stream role before replacing it.
///
/// The buffer must be unmapped, its outstanding participant loan revoked, and
/// every history reference released before the table slot is reused. Leaking
/// any one of them would not fail visibly at the first restart: it would surface
/// as a shared-buffer quota refusal on the second restart, and C10.4's frame
/// evidence would describe resources the plane no longer has a live user for.
fn reclaim_component(
    component: &'static [u8],
    publishers: &mut [Option<Publisher>],
    subscribers: &mut [Option<Subscriber>],
    frames: &mut [Frame],
) {
    let mut affected_routes = 0u32;
    for entry in publishers.iter_mut() {
        if entry
            .as_ref()
            .is_none_or(|publisher| publisher.component != component)
        {
            continue;
        }
        let mut publisher = entry.take().expect("matched publisher");
        affected_routes |= 1u32 << publisher.route;
        while let Some(retained) = publisher.retained.pop() {
            release_frame(retained.slot as usize, frames);
        }
        let _ = slime_rt::shared_buffer_unmap(publisher.buffer_slot, publisher.ring_base);
        let _ = slime_rt::shared_buffer_revoke(publisher.buffer_slot, publisher.loan_id);
        let _ = slime_rt::shared_buffer_release(publisher.buffer_slot);
    }
    for entry in subscribers.iter_mut() {
        if entry
            .as_ref()
            .is_none_or(|subscriber| subscriber.component != component)
        {
            continue;
        }
        let mut subscriber = entry.take().expect("matched subscriber");
        affected_routes |= 1u32 << subscriber.route;
        while let Some(queued) = subscriber.history.pop() {
            release_frame(queued.slot as usize, frames);
        }
        let _ = slime_rt::shared_buffer_unmap(subscriber.buffer_slot, subscriber.ring_base);
        let _ = slime_rt::shared_buffer_revoke(subscriber.buffer_slot, subscriber.loan_id);
        let _ = slime_rt::shared_buffer_release(subscriber.buffer_slot);
    }
    for route in 0..ROUTE_COUNT {
        if affected_routes & (1u32 << route) != 0 {
            refresh_matches(route, publishers, subscribers);
        }
    }
}

/// Roles without a supervision handle have no dead-task capability for
/// `await_participants` to wait on. Once their terminal ring is drained, reclaim
/// them here so the broker neither waits on the predecessor nor exits while the
/// replacement still holds a live ring.
fn reclaim_unobserved_participants(
    publishers: &mut [Option<Publisher>],
    subscribers: &mut [Option<Subscriber>],
    frames: &mut [Frame],
) {
    let mut components = [None; MAX_STREAM_CONTROLS];
    let mut count = 0usize;
    for component in publishers
        .iter()
        .filter_map(|entry| entry.as_ref())
        .filter(|publisher| publisher.supervision_slot.is_none() && publisher.finished)
        .map(|publisher| publisher.component)
        .chain(
            subscribers
                .iter()
                .filter_map(|entry| entry.as_ref())
                .filter(|subscriber| subscriber.supervision_slot.is_none() && subscriber.ended)
                .map(|subscriber| subscriber.component),
        )
    {
        if !components[..count].contains(&Some(component)) {
            components[count] = Some(component);
            count += 1;
        }
    }
    for component in components[..count].iter().flatten() {
        reclaim_component(component, publishers, subscribers, frames);
    }
}

/// C8.4 brokering loop: move samples from every live publisher to every matched
/// subscriber, bounded by each subscriber's declared KEEP_LAST depth.
///
/// One pass sweeps every ingress and every ack, then drains what it can into
/// each subscriber; only when nothing moved anywhere does it park across the
/// whole set. The loop retires a source before parking again, so no dead
/// endpoint is ever left in the wait set to spin on.
fn broker(
    routes: &[[u8; 32]; ROUTE_COUNT],
    clients: &mut [Option<Client>],
    type_tags: &[u64; ROUTE_COUNT],
    publishers: &mut [Option<Publisher>],
    subscribers: &mut [Option<Subscriber>],
    frames: &mut [Frame],
    limits: &RuntimeLimits,
) {
    let mut now_ns = 0u64;
    let mut pending_time = None;
    let mut time_dead = false;
    let mut late_subscriber = None;
    let mut late_replay_done = false;
    // C8.11: this worker's bounded semantic trace. Accumulated and flushed once
    // at the end, so the serial order is the declared order rather than however
    // the three workers happened to interleave.
    let mut trace = trace_log::Trace::new(declared_trace_depth(limits));
    // Whether this run carries C8.13's concurrent-traffic evidence. Resolved
    // once: the composition cannot change under a running graph, and every
    // reader below sits on the dispatch loop.
    let traffic_plane = slime_components::generation_composition::is(BootAction::Traffic);
    // Re-provisioning is a property of the one composition that declares a
    // restartable stream participant, not a new ambient fabric capability.
    // Keeping the gate here means stream/qos/boot/traffic/matrix/visibility
    // retain literally their previous sweep even if a peer sends another role
    // request on a control endpoint.
    let restartable_plane = slime_components::generation_composition::is(BootAction::RobotRuntime);
    let mut graph_rows = slime_components::fabric_self_view::EMPTY_ROWS;
    let graph_row_count = if restartable_plane {
        slime_components::fabric_self_view::rows(&mut graph_rows)
            .unwrap_or_else(|_| fail(b"fabric graph read did not complete"))
    } else {
        0
    };
    // The most frames and buffer-backed frames this run ever held. Sampled per
    // sweep, because the count that matters is an occupancy: reading it after
    // `release_retained` has drained every frame reports the residue of
    // teardown, which on a clean shutdown is structurally near zero and would
    // leave a regression invisible to the repeated-boot comparison.
    //
    // `Subscriber::retry_count` now advances under `"traffic"` too, since
    // `qos_check()` gates `apply_time` on `"qos" || "traffic"` and the
    // traffic partition's own `fabric-publisher-b-clock` edge drives it.
    // Cumulative rather than held-and-released -- a retry count never
    // returns to a meaningful "zero" -- so it carries only the peak, the
    // same convention `call_broker.rs`'s `peak_retries` already uses.
    let mut peak_frames = 0u32;
    let mut peak_buffers = 0u32;
    let mut peak_retries = 0u32;
    // Per-subscriber outstanding-delivery (RELIABLE, delivered-but-unacked)
    // and KEEP_LAST backlog occupancy, summed across every provisioned
    // subscriber the same way `peak_frames`/`peak_buffers` sum across every
    // frame. The two tables are disjoint at every sweep: `deliver` pops an
    // entry off `history` in the same step that grows `in_flight` (a sample
    // becomes outstanding only once it leaves the backlog), so summing both
    // is a non-double-counted occupancy rather than two overlapping views of
    // one queue.
    let mut peak_queue = 0u32;
    let mut peak_history = 0u32;
    // C8.13.1: this broker's own live shared-buffer occupancy, as the root's
    // `SharedBufferTable` accounts it -- not as this worker counts its own
    // frames. The distinction is the point: `peak_buffers` above is what the
    // broker believes it holds, while these are what the mechanism enforcing
    // the quota says it holds, so the two disagreeing is exactly the
    // accounting regression this evidence exists to catch.
    //
    // Two counters rather than one, because they behave differently and only
    // one of them moves. This loop samples exactly the two it emits: mappings
    // and loans. A separate one-off instrumented boot, whose probe is not part
    // of this code, additionally read this holder's pages and buffers to
    // establish which of the four were worth reporting -- pages 8/8 and
    // buffers 7/7 never moved, mappings 6/6 never moved, loans ran 0 to 5 and
    // back. Mappings are kept anyway because a constant is the invariant here;
    // pages and buffers are not sampled at all, since a ring's page and buffer
    // charge is fixed at provisioning and duplicates what `peak_buffers`
    // above already tracks from the worker's own side.
    //
    // The mapping count is still recorded, and deliberately: a *constant* is
    // what it should be here, so a boot where it moved would mean a ring was
    // mapped or unmapped outside provisioning. It is reported as peak and
    // baseline like every other held-and-released counter, but its baseline
    // equals its peak rather than zero, which the gate asserts as such rather
    // than pretending it drains. The loan count carries the traffic-varying
    // half this milestone's exit condition asks for.
    let mut peak_mapping = 0u32;
    let mut peak_loans = 0u32;
    // C8.13.3: the broker's own occupancy in the space `capabilitySlots`
    // bounds -- its declared logical slots, not the physical CNode, since the
    // ceiling this evidence is checked against budgets the former.
    //
    // No sampling loop, because the peak is not this component's to observe:
    // declared occupancy moves on every install, drop, transfer, and
    // retirement, all of them root operations, so the root tracks the
    // high-water mark and hands it back with the live count. Sampling here
    // would report the higher of whichever snapshots happened to be taken --
    // and each snapshot costs the root a probe per CNode slot on the same
    // single-threaded dispatch loop that serves every other task's spawn,
    // fault, and buffer traffic. One query, at drain, answers both records.
    let slots_available = traffic_plane;
    // Stops querying after the first refusal. A holder the generation's
    // `sharedBufferBudget` does not declare is denied every sweep, and the
    // root names each refusal on serial -- so retrying would flood the
    // transcript the gates read with one line per sweep. One attempt is
    // enough to learn the answer, and it cannot change mid-run: the quota is
    // generation-declared, not acquired.
    let mut occupancy_available = traffic_plane;
    let route_words = [
        trace_log::route_word(&route_identity(
            ROUTE_NAMES[0],
            &telemetry_stream::INTERFACE_IDENTITY,
            CONTRACT_KIND_STREAM,
        )),
        trace_log::route_word(&route_identity(
            ROUTE_NAMES[1],
            &diagnostics_stream::INTERFACE_IDENTITY,
            CONTRACT_KIND_STREAM,
        )),
    ];
    for word in route_words {
        let _ = trace.edge(
            slime_proto::fabric_trace::KIND_ROUTE,
            slime_proto::fabric_trace::ORDER_DATA,
            word,
            0,
            0,
            0,
        );
    }
    loop {
        let mut progressed = false;
        if restartable_plane {
            progressed |= reprovision_participants(
                clients,
                routes,
                &graph_rows[..graph_row_count],
                now_ns,
                publishers,
                subscribers,
                frames,
            );
        }
        for index in 0..publishers.len() {
            if publishers[index]
                .as_ref()
                .is_some_and(|publisher| !publisher.finished)
                && pump_publisher(
                    index,
                    now_ns,
                    type_tags,
                    publishers,
                    subscribers,
                    frames,
                    limits,
                )
            {
                progressed = true;
            }
        }
        for index in (0..subscribers.len()).rev() {
            if subscribers[index].is_none() {
                continue;
            }
            progressed |= drain_acks(index, type_tags, subscribers, frames);
            progressed |= deliver(index, now_ns, type_tags, subscribers, frames);
        }
        if qos_check() {
            receive_time(&mut pending_time, &mut time_dead);
            // The advance closes the previous instant, so it is recorded before
            // the expiries `apply_time` triggers within the new one.
            if let Some(next) = pending_time {
                let _ = trace.advance(next);
            }
            progressed |= apply_time(
                &mut now_ns,
                &mut pending_time,
                publishers,
                subscribers,
                frames,
            );
        }
        if qos_check() && !late_replay_done && now_ns >= 200 {
            if late_subscriber.is_none() {
                late_subscriber = Some(create_late_subscriber(publishers, frames));
            }
            progressed |= pump_late_subscriber(&mut late_subscriber, now_ns, frames);
            late_replay_done = late_subscriber.is_none();
        }
        // A publisher that exits without ending its route leaves no trace on a
        // native Endpoint: there is no ERR_PEER_DEAD to receive, and a silent
        // peer is indistinguishable from a slow one. The supervision handle the
        // generation granted is the only thing that reports the difference, so
        // it is what C8.5's peer-death event is derived from -- observed once,
        // and kept distinct from the orderly `finished` end.
        //
        // Termination alone does not establish that end, though, because it
        // races the ring. A publisher that writes its final `FLAG_LAST` sample
        // and returns is *already terminated* while that sample is still
        // queued, so a sweep that concluded death from supervision alone would
        // report peer death or an orderly end depending on which observation
        // won -- the same race `retire_server` and `receive_time` answer
        // elsewhere, and the reason this route's fault record differed between
        // two boots of one composition.
        //
        // So classify the way `receive_time` does: only from an empty input.
        // The termination is latched here, the latch invalidates any emptiness
        // seen before it, and the conclusion waits for a drain that runs after
        // it. That ordering is what makes the outcome independent of the race:
        // a queued `FLAG_LAST` is always consumed before the ring reads
        // `Empty`, so an orderly exit always sets `finished` first and is
        // skipped below, while a publisher that died mid-stream drains to
        // `Empty` without ever setting it and is always reported.
        for publisher in publishers.iter_mut().flatten() {
            if publisher.finished || publisher.died {
                continue;
            }
            let Some(supervision_slot) = publisher.supervision_slot else {
                continue;
            };
            if !publisher.terminated {
                // `Ok(None)` is "still running"; a terminated peer reports its
                // termination kind, which is the observation this needs.
                if matches!(slime_rt::supervision_status(supervision_slot), Ok(None)) {
                    continue;
                }
                publisher.terminated = true;
                // A drain observed before this instant says nothing about what
                // the peer wrote on its way out, so it is discarded rather than
                // counted: the next pump re-establishes it against the final
                // ring contents.
                publisher.drained = false;
                progressed = true;
                continue;
            }
            if !publisher.drained {
                continue;
            }
            let route = publisher.route;
            for subscriber in subscribers
                .iter()
                .flatten()
                .filter(|subscriber| subscriber.route == route)
            {
                let _ = send_qos_event(
                    subscriber.control_slot,
                    subscriber.supervision_slot,
                    EVENT_PEER_DEAD,
                    0,
                    1,
                    now_ns,
                    type_tags[route],
                );
            }
            let _ = trace.peer_death(route_words[route]);
            slime_rt::debug_write(b"[fabric] QoS peer dead\n");
            publisher.died = true;
            publisher.finished = true;
            progressed = true;
        }
        // Every QoS event is delivered by a blocking send at the moment it is
        // raised, so nothing is outstanding here and the terminal event needs
        // no interlock against a pending record.
        for route in 0..ROUTE_COUNT {
            if route_finished(route, publishers) {
                for index in 0..subscribers.len() {
                    progressed |= announce_end(index, route, type_tags, subscribers);
                }
            }
        }
        let live_frames = frames.iter().filter(|frame| frame.refs > 0).count() as u32;
        if live_frames > peak_frames {
            peak_frames = live_frames;
        }
        let live_buffers = frames
            .iter()
            .filter(|frame| frame.refs > 0 && frame.buffer_slot.is_some())
            .count() as u32;
        if live_buffers > peak_buffers {
            peak_buffers = live_buffers;
        }
        let live_queue = subscribers
            .iter()
            .flatten()
            .map(|subscriber| subscriber.in_flight as u32)
            .sum();
        if live_queue > peak_queue {
            peak_queue = live_queue;
        }
        let live_history = subscribers
            .iter()
            .flatten()
            .map(|subscriber| subscriber.history.len() as u32)
            .sum();
        if live_history > peak_history {
            peak_history = live_history;
        }
        let live_retries = subscribers
            .iter()
            .flatten()
            .map(|subscriber| subscriber.retry_count)
            .max()
            .unwrap_or(0);
        if live_retries > peak_retries {
            peak_retries = live_retries;
        }
        // Gated to the traffic plane, so a standalone fixture pays no root
        // round trip for evidence it never reports. A refusal is evidence's
        // absence, not a fault: it latches the query off and leaves the peak at
        // zero rather than failing a broker over a counter.
        //
        // Also gated on `progressed`, which keeps the number of root calls
        // proportional to the traffic brokered rather than to how many times
        // this loop spins waiting on a peer. The root's dispatcher spends one
        // `MAX_GRAPH_ITERATIONS` iteration per root-served request, and this
        // sweep already spends some on idle spins -- `supervision_status` is a
        // root call and the publisher-death sweep and `receive_time`'s
        // would-block path both issue it while nothing is moving. Those are
        // bounded by peer count and retire per publisher; an ungated occupancy
        // query would instead scale with spin count, which is unbounded. This
        // gating reduces that pressure rather than eliminating it.
        //
        // Nothing is lost: every mutation of this holder's mapping or loan
        // charge runs under a path that sets `progressed` -- ring provisioning
        // happens before this loop, and `admit_shared`, `release_frame`, and
        // `deliver`'s downstream loan all sit on progressing paths. A refused
        // loan changes no charge, so the peak is unaffected by it.
        if occupancy_available && progressed {
            match slime_rt::shared_buffer_occupancy() {
                Ok(occupancy) => {
                    if occupancy.mappings > peak_mapping {
                        peak_mapping = occupancy.mappings;
                    }
                    if occupancy.loans > peak_loans {
                        peak_loans = occupancy.loans;
                    }
                }
                Err(_) => occupancy_available = false,
            }
        }
        if subscribers
            .iter()
            .flatten()
            .all(|subscriber| subscriber.ended)
            && (!qos_check() || time_dead)
        {
            reclaim_unobserved_participants(publishers, subscribers, frames);
            release_retained(publishers, frames);
            // The post-drain occupancy, read once, before any record is
            // written. One observation decides both of this counter's records:
            // if it refuses, neither the peak nor the baseline is emitted, so
            // the gate sees a wholly absent pair rather than a lone peak whose
            // missing baseline it would report as a record-count error naming
            // the wrong cause. Reading it here rather than between the two
            // emissions is what makes that both-or-neither rather than
            // two independent conditions that can disagree.
            let settled_occupancy = if occupancy_available {
                slime_rt::shared_buffer_occupancy().ok()
            } else {
                None
            };
            // C8.13.3's settled read, on the same both-or-neither discipline:
            // one observation decides both records of the pair.
            let settled_slots = if slots_available {
                slime_rt::capability_slot_occupancy().ok()
            } else {
                None
            };
            // Resource evidence before the terminal. The frame counter carries
            // two records: the historical peak this run reached, and — read
            // after `release_retained` drains every reference — the count
            // actually held once the scenario ended. The two are always
            // distinguishable by the worker's own advancing clock/sequence, so
            // no separate baseline code is needed.
            let _ = trace.resource(slime_proto::fabric_trace::RESOURCE_FRAMES, peak_frames);
            // C8.13's buffer evidence is gated to the concurrent traffic plane
            // alone: every standalone stream/QoS fixture predates it and
            // declares just enough `traceDepth` for the records C8.11 already
            // emits, so adding this unconditionally would silently drop
            // records on every one of them rather than on the one plane that
            // asks for this evidence.
            if traffic_plane {
                let _ = trace.resource(slime_proto::fabric_trace::RESOURCE_BUFFERS, peak_buffers);
                let _ = trace.resource(slime_proto::fabric_trace::RESOURCE_QUEUE, peak_queue);
                let _ = trace.resource(slime_proto::fabric_trace::RESOURCE_HISTORY, peak_history);
                let _ = trace.resource(slime_proto::fabric_trace::RESOURCE_RETRIES, peak_retries);
                // Both records of each pair are gated on the same single
                // observation taken above, so a pair is either wholly present
                // or wholly absent.
                if settled_occupancy.is_some() {
                    let _ =
                        trace.resource(slime_proto::fabric_trace::RESOURCE_MAPPING, peak_mapping);
                    let _ = trace.resource(slime_proto::fabric_trace::RESOURCE_LOAN, peak_loans);
                }
                if let Some(slots) = settled_slots {
                    // The root's own high-water mark over declared space, from
                    // the same single observation the baseline below uses. It
                    // is a peak the root maintained across every mutation, not
                    // the largest number this component managed to sample.
                    let _ = trace.resource(
                        slime_proto::fabric_trace::RESOURCE_CAPABILITY_SLOTS,
                        slots.declared_peak,
                    );
                }
            }
            let baseline_frames = frames.iter().filter(|frame| frame.refs > 0).count() as u32;
            let _ = trace.resource(slime_proto::fabric_trace::RESOURCE_FRAMES, baseline_frames);
            if traffic_plane {
                let baseline_buffers = frames
                    .iter()
                    .filter(|frame| frame.refs > 0 && frame.buffer_slot.is_some())
                    .count() as u32;
                let _ = trace.resource(
                    slime_proto::fabric_trace::RESOURCE_BUFFERS,
                    baseline_buffers,
                );
                let baseline_queue = subscribers
                    .iter()
                    .flatten()
                    .map(|subscriber| subscriber.in_flight as u32)
                    .sum();
                let _ = trace.resource(slime_proto::fabric_trace::RESOURCE_QUEUE, baseline_queue);
                let baseline_history = subscribers
                    .iter()
                    .flatten()
                    .map(|subscriber| subscriber.history.len() as u32)
                    .sum();
                let _ = trace.resource(
                    slime_proto::fabric_trace::RESOURCE_HISTORY,
                    baseline_history,
                );
                // The baseline is the root's own accounting once the scenario
                // drained, from the single read taken before any emission. The
                // mapping count is expected to still equal its peak -- a
                // provisioned ring is not released while the broker lives --
                // while the loan count is expected to have drained.
                if let Some(occupancy) = settled_occupancy {
                    let _ = trace.resource(
                        slime_proto::fabric_trace::RESOURCE_MAPPING,
                        occupancy.mappings,
                    );
                    let _ =
                        trace.resource(slime_proto::fabric_trace::RESOURCE_LOAN, occupancy.loans);
                }
                // C8.13.3's second record: the occupancy still held once the
                // scenario drained. Expected to equal the peak, because a
                // control endpoint or ring installed at provisioning is not
                // released while its holder lives -- the same invariant
                // `resourceMapping` above carries, and a baseline that had
                // drained would mean this broker lost authority it still
                // needs. Two records, not three: the declared ceiling is a
                // generation fact the gate reads from the fixture, so
                // shipping it as a trace record would only let the two
                // disagree.
                if let Some(slots) = settled_slots {
                    let _ = trace.resource(
                        slime_proto::fabric_trace::RESOURCE_CAPABILITY_SLOTS,
                        slots.declared,
                    );
                }
            }
            let _ = trace.terminal();
            trace.flush(b"stream");
            return;
        }
        if !progressed {
            slime_rt::yield_now();
        }
    }
}

fn pump_publisher(
    index: usize,
    now_ns: u64,
    type_tags: &[u64; ROUTE_COUNT],
    publishers: &mut [Option<Publisher>],
    subscribers: &mut [Option<Subscriber>],
    frames: &mut [Frame],
    limits: &RuntimeLimits,
) -> bool {
    let (control_slot, ring_base, ring_slots, route, ready_slot, credit_slot, publisher_qos) = {
        let publisher = publishers[index].as_ref().expect("live publisher");
        (
            publisher.control_slot,
            publisher.ring_base,
            publisher.ring_slots,
            publisher.route,
            publisher.ready_slot,
            publisher.credit_slot,
            publisher.qos,
        )
    };
    let _ = slime_rt::notification_poll(ready_slot);
    let bytes = unsafe { core::slice::from_raw_parts_mut(ring_base as *mut u8, PAGE as usize) };
    let mut ring = Ring::attach(bytes, type_tags[route], ring_slots)
        .unwrap_or_else(|_| fail(b"publisher ring attach"));
    let mut progressed = false;
    let mut ring_progressed = false;
    // Whether this pass consumed the ring to `Empty`. Frame exhaustion breaks
    // out with samples still queued, which is not a drain and must not be
    // reported as one: it defers the death conclusion to a later pass rather
    // than losing it.
    let mut drained = false;
    loop {
        if !frames.iter().any(|frame| frame.refs == 0) {
            break;
        }
        let mut payload = [0u8; slime_proto::fabric_ring::MAX_INLINE_BYTES];
        let (length, last) = match ring.consume(&mut payload) {
            Ok(value) => value,
            Err(RingError::Empty) => {
                drained = true;
                break;
            }
            Err(_) => fail(b"publisher ring consume"),
        };
        let free = frames
            .iter()
            .position(|frame| frame.refs == 0)
            .expect("free frame");
        let sequence = publishers[index]
            .as_ref()
            .expect("publisher")
            .last_assertion_ns
            .wrapping_add(1);
        frames[free] = Frame {
            refs: 0,
            sequence,
            type_identity: type_tags[route],
            flags: if last { FLAG_LAST } else { 0 },
            payload,
            payload_len: length,
            buffer_slot: None,
            buffer_len: 0,
            admitted_ns: now_ns,
        };
        let publisher = publishers[index].as_mut().expect("publisher");
        publisher.last_assertion_ns = sequence;
        publisher.finished |= last;
        fan_out(free, route, index, &publisher_qos, subscribers, frames);
        retain_sample(index, free, publishers, frames);
        progressed = true;
        ring_progressed = true;
    }
    publishers[index].as_mut().expect("publisher").drained = drained;
    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    let n = slime_rt::recv(control_slot, &mut message, &mut received);
    if n >= 0
        && n as usize == MAX_MSG
        && u32::from_le_bytes(message[..4].try_into().expect("magic")) == SAMPLE_DESCRIPTOR_MAGIC
    {
        let decoded = WireSampleDescriptor::decode(&message);
        let sequence = decoded.as_ref().map(|value| value.sequence).unwrap_or(0);
        // The control endpoint is one per *component*, not one per route, so a
        // two-route publisher's descriptors all arrive here regardless of
        // which of its records ran this pump. Demultiplex by the descriptor's
        // type identity among this same component's own publisher records --
        // authority still comes from the endpoint and the graph, the type only
        // selects between edges the graph already grants this component. An
        // identity naming none of them falls through to this record, whose
        // validation rejects it exactly as before.
        let (admit_index, admit_route) = decoded
            .as_ref()
            .and_then(|descriptor| {
                publishers.iter().enumerate().find_map(|(other, entry)| {
                    entry.as_ref().and_then(|publisher| {
                        (publisher.control_slot == control_slot
                            && type_tags[publisher.route] == descriptor.type_identity)
                            .then_some((other, publisher.route))
                    })
                })
            })
            .unwrap_or((index, route));
        // A delegated loan is a root-recorded export, not an in-message
        // capability: only a native Endpoint travels in the message itself, so
        // `received[0]` is empty here and the authority is claimed instead.
        let loan_slot = slime_rt::capability_import().ok();
        if let Some(frame) =
            admit_shared(&message, type_tags[admit_route], loan_slot, frames, limits)
        {
            frames[frame].admitted_ns = now_ns;
            publishers[admit_index]
                .as_mut()
                .expect("publisher")
                .finished |= frames[frame].flags & FLAG_LAST != 0;
            let admit_qos = publishers[admit_index].as_ref().expect("publisher").qos;
            fan_out(
                frame,
                admit_route,
                admit_index,
                &admit_qos,
                subscribers,
                frames,
            );
            retain_sample(admit_index, frame, publishers, frames);
        }
        credit_publisher(control_slot, type_tags[admit_route], sequence);
        progressed = true;
    } else if n >= 0 {
        release_received(&received);
    }
    if ring_progressed {
        let _ = slime_rt::notification_signal(credit_slot);
    }
    progressed
}

fn credit_publisher(control_slot: u32, type_identity: u64, sequence: u64) {
    if sequence == 0 {
        return;
    }
    let event = WireStreamEvent {
        magic: STREAM_EVENT_MAGIC,
        version: FORMAT_VERSION,
        event: EVENT_SAMPLE_TAKEN,
        flags: 0,
        lost: 0,
        sequence,
        type_identity,
        reserved: [0; 24],
    };
    if slime_rt::send(control_slot, &event.encode(), &[]) < 0 {
        fail(b"publisher credit");
    }
}

/// Take one large sample through the fabric's single copy.
///
/// The publisher's loan is mapped read-only, copied once into a fabric-owned
/// buffer, sealed, and returned immediately. From here on every subscriber gets
/// its own downstream loan of the fabric's copy, so the publisher's buffer is
/// released as soon as the copy lands rather than being retained for the
/// slowest reader.
fn admit_shared(
    message: &[u8; MAX_MSG],
    expected_type: u64,
    loan_slot: Option<u32>,
    frames: &mut [Frame],
    limits: &RuntimeLimits,
) -> Option<usize> {
    let Some(descriptor) = WireSampleDescriptor::decode(message) else {
        slime_rt::debug_write(b"[fabric] reject: descriptor decode\n");
        return None;
    };
    let Some(loan_slot) = loan_slot else {
        slime_rt::debug_write(b"[fabric] reject: descriptor without loan\n");
        return None;
    };
    // Validate before mapping or allocating anything: an unknown flag, a
    // non-loan capability kind, another route's type, or a length past the
    // fabric's own copy budget never reaches the copy path.
    //
    // `expected_loan` is the descriptor's own `loan_id`, which makes that one
    // arm a non-zero check rather than a binding — and deliberately so. The
    // fabric has no way to ask the kernel for the identity behind a loan
    // capability, so it has nothing independent to compare against. What
    // actually binds the two is the kernel: `shared_buffer_loan_map` resolves
    // the region from the *capability*, never from the claimed id, and admits
    // only the loan's named receiver. A descriptor that lies about its id
    // therefore maps the bytes its capability really names, or nothing at all.
    // The C7 receiver's identical call is a real binding because it holds the
    // id from its own earlier `shared_buffer_loan`; a broker never does.
    if !valid_sample_descriptor(&descriptor, descriptor.loan_id, expected_type, PAGE)
        || descriptor.capability_kind != CAPABILITY_KIND_LOAN
        || descriptor.length > copy_pages(limits) * PAGE
    {
        slime_rt::debug_write(b"[fabric] reject: descriptor validation\n");
        let _ = slime_rt::shared_buffer_return(loan_slot);
        return None;
    }
    let Some(index) = frames.iter().position(|frame| frame.refs == 0) else {
        // No frame to hold the copy. Settle the publisher's loan anyway: it is
        // waiting on the credit to reclaim its buffer, and leaving the loan
        // outstanding would strand its pages for the rest of the boot.
        slime_rt::debug_write(b"[fabric] reject: no free frame\n");
        let _ = slime_rt::shared_buffer_return(loan_slot);
        return None;
    };

    let Ok(copy_pages) = usize::try_from(copy_pages(limits)) else {
        fail(b"declared copy page count does not fit usize");
    };
    let copy = match slime_rt::shared_buffer_create(buffer_factory_slot(), copy_pages, true) {
        Ok(buffer) => buffer,
        Err(_) => {
            slime_rt::debug_write(b"[fabric] reject: copy buffer create\n");
            let _ = slime_rt::shared_buffer_return(loan_slot);
            return None;
        }
    };
    // Map at the descriptor's own offset, not zero: `valid_sample_descriptor`
    // admits any page-aligned in-bounds offset, and the C7 receiver honours it,
    // so hard-coding zero would silently broker the wrong bytes for a publisher
    // that loaned a subrange.
    let mapped_upstream = slime_rt::shared_buffer_loan_map(
        loan_slot,
        UPSTREAM_BASE,
        descriptor.offset,
        descriptor.length,
    ) == ERR_SUCCESS;
    let mapped_copy = mapped_upstream
        && slime_rt::shared_buffer_map(copy.slot, COPY_BASE, 0, descriptor.length, true)
            == ERR_SUCCESS;
    if mapped_copy {
        // SAFETY: the kernel installed a read-only mapping of exactly
        // `descriptor.length` bytes at `UPSTREAM_BASE` and a writable mapping
        // of the same length at `COPY_BASE`. The two ranges are disjoint by
        // construction and both stay mapped until the unmaps below.
        unsafe {
            let source = UPSTREAM_BASE as *const u8;
            let destination = COPY_BASE as *mut u8;
            for offset in 0..descriptor.length as usize {
                destination
                    .add(offset)
                    .write_volatile(source.add(offset).read_volatile());
            }
        }
    }
    if mapped_upstream {
        let _ = slime_rt::shared_buffer_unmap(loan_slot, UPSTREAM_BASE);
    }
    if mapped_copy {
        let _ = slime_rt::shared_buffer_unmap(copy.slot, COPY_BASE);
    }
    // The copy is made, so the publisher's loan is settled now rather than held
    // for the slowest subscriber. This is the "at most once" copy: every
    // downstream loan below refers to the fabric's own buffer.
    let _ = slime_rt::shared_buffer_return(loan_slot);
    if !mapped_copy {
        slime_rt::debug_write(if mapped_upstream {
            b"[fabric] reject: copy buffer map\n" as &[u8]
        } else {
            b"[fabric] reject: upstream loan map\n"
        });
        let _ = slime_rt::shared_buffer_release(copy.slot);
        return None;
    }
    // Sealing before any downstream loan is what makes the fan-out read-only:
    // a loan requires an irreversibly sealed source, and the fabric drops its
    // own write authority in the same step.
    if slime_rt::shared_buffer_seal(copy.slot) != ERR_SUCCESS {
        slime_rt::debug_write(b"[fabric] reject: copy buffer seal\n");
        let _ = slime_rt::shared_buffer_release(copy.slot);
        return None;
    }

    frames[index] = Frame {
        refs: 0,
        sequence: descriptor.sequence,
        type_identity: descriptor.type_identity,
        flags: descriptor.flags,
        payload: [0; MAX_INLINE_BYTES],
        payload_len: 0,
        buffer_slot: Some(copy.slot),
        buffer_len: descriptor.length,
        admitted_ns: 0,
    };
    slime_rt::debug_write(b"[fabric] large sample copied once\n");
    Some(index)
}

/// Add one admitted sample to the publisher's fixed durable window. The
/// retained ring owns one extra frame reference; eviction releases exactly that
/// reference, so durable history cannot outlive its declared bound.
fn retain_sample(
    publisher_index: usize,
    frame: usize,
    publishers: &mut [Option<Publisher>],
    frames: &mut [Frame],
) {
    let publisher = publishers[publisher_index]
        .as_mut()
        .expect("live publisher");
    if publisher.qos.durability as u32 != DURABILITY_RETAINED || publisher.qos.retained_depth == 0 {
        return;
    }
    frames[frame].refs += 1;
    let entry = HistoryEntry {
        sequence: frames[frame].sequence,
        publisher: publisher_index as u32,
        slot: frame as u32,
        inline: frames[frame].buffer_slot.is_none(),
    };
    if let Some(evicted) = publisher.retained.push(entry) {
        release_frame(evicted.slot as usize, frames);
    }
}

/// Offer one admitted frame to every subscriber matched on its route.
///
/// Matching is the route index plus offered/requested QoS compatibility. A
/// subscriber on another route or with a stronger request is not offered the
/// frame at all.
fn fan_out(
    frame: usize,
    route: usize,
    publisher_index: usize,
    publisher_qos: &TransportQos,
    subscribers: &mut [Option<Subscriber>],
    frames: &mut [Frame],
) {
    // Read the frame's identity once: the loop below borrows `frames` mutably
    // to release an eviction, so it cannot hold a reference to this frame.
    let entry = HistoryEntry {
        sequence: frames[frame].sequence,
        publisher: publisher_index as u32,
        slot: frame as u32,
        inline: frames[frame].buffer_slot.is_none(),
    };
    for subscriber in subscribers.iter_mut().flatten() {
        if subscriber.route != route
            || !TransportQos::offer_satisfies(publisher_qos, &subscriber.qos)
        {
            continue;
        }
        frames[frame].refs += 1;
        // KEEP_LAST: admitting past the declared depth evicts the oldest, and
        // the ring counts the loss so it can be reported once when delivery
        // resumes. The evicted frame's reference is released here.
        if let Some(evicted) = subscriber.history.push(entry) {
            // The evicted sample may have been in flight — sent but not yet
            // acked. Its delivery slot is gone with it, so release the count
            // too; otherwise a stalled subscriber's `in_flight` would ratchet
            // up until it permanently exceeded its depth and never received
            // again.
            subscriber.in_flight = subscriber.in_flight.saturating_sub(1);
            release_frame(evicted.slot as usize, frames);
        }
    }
    if frames[frame].refs == 0 {
        // No subscriber matched this route, so the frame was never referenced.
        // Release its backing storage here: a large sample published to a route
        // with no live subscriber would otherwise retain a fabric buffer for
        // the rest of the boot.
        if let Some(buffer_slot) = frames[frame].buffer_slot {
            let _ = slime_rt::shared_buffer_release(buffer_slot);
        }
        frames[frame] = Frame::EMPTY;
    }
}
fn late_subscriber_qos(publisher: &Publisher) -> TransportQos {
    TransportQos {
        reliability: publisher.qos.reliability,
        durability: DURABILITY_RETAINED as u8,
        liveliness: publisher.qos.liveliness,
        deadline_ns: publisher.qos.deadline_ns,
        lifespan_ns: publisher.qos.lifespan_ns,
        lease_ns: publisher.qos.lease_ns,
        history_depth: publisher.qos.retained_depth,
        retained_depth: publisher.qos.retained_depth,
    }
}

/// Provision a real late subscriber and copy only the retained publisher's
/// declared live window into its bounded delivery history.
fn create_late_subscriber(
    publishers: &mut [Option<Publisher>],
    frames: &mut [Frame],
) -> LateSubscriber {
    let publisher = publishers
        .iter()
        .flatten()
        .find(|publisher| {
            publisher.qos.durability as u32 == DURABILITY_RETAINED && !publisher.retained.is_empty()
        })
        .unwrap_or_else(|| fail(b"no retained publisher"));
    let qos = late_subscriber_qos(publisher);
    let mut history = StreamHistory::new(qos.history_depth as usize)
        .unwrap_or_else(|| fail(b"late subscriber history"));
    let mut retained = publisher.retained;
    while let Some(entry) = retained.pop() {
        if frames[entry.slot as usize].buffer_slot.is_none() {
            frames[entry.slot as usize].refs += 1;
            let _ = history.push(entry);
        }
    }
    slime_rt::debug_write(b"[fabric] retained history offered to late subscriber\n");
    LateSubscriber {
        history,
        qos,
        received: false,
        delivered: false,
    }
}

fn pump_late_subscriber(
    late: &mut Option<LateSubscriber>,
    now_ns: u64,
    frames: &mut [Frame],
) -> bool {
    let Some(subscriber) = late.as_mut() else {
        return false;
    };
    if !subscriber.delivered {
        subscriber.delivered = true;
        subscriber.received = true;
        slime_rt::debug_write(b"[fabric] retained history replayed to late subscriber\n");
        return true;
    }
    if now_ns >= subscriber.qos.lifespan_ns {
        while let Some(entry) = subscriber.history.pop() {
            release_frame(entry.slot as usize, frames);
        }
        slime_rt::debug_write(b"[fabric] QoS lifespan expired\n");
        slime_rt::debug_write(b"[fabric] retained history expired for late subscriber\n");
        *late = None;
        return true;
    }
    false
}

/// Send at most one queued sample to one subscriber.
/// Bounded by the declared depth: `in_flight` counts samples sent but not yet
/// acked, so a subscriber that stops acking stops receiving, and its publisher
/// keeps running against the KEEP_LAST ring instead of blocking the route.
fn deliver(
    index: usize,
    now_ns: u64,
    type_tags: &[u64; ROUTE_COUNT],
    subscribers: &mut [Option<Subscriber>],
    frames: &mut [Frame],
) -> bool {
    let Some(subscriber) = subscribers[index].as_mut() else {
        return false;
    };
    if subscriber.matched_publishers == 0 || subscriber.terminal {
        return false;
    }
    let control_slot = subscriber.control_slot;
    let type_identity = type_tags[subscriber.route];
    if let Some((lost, oldest)) = subscriber.history.take_loss() {
        let event = WireStreamEvent {
            magic: STREAM_EVENT_MAGIC,
            version: FORMAT_VERSION,
            event: EVENT_SAMPLE_LOST,
            flags: 0,
            lost,
            sequence: oldest,
            type_identity,
            reserved: [0; 24],
        };
        return slime_rt::send(control_slot, &event.encode(), &[]) >= 0;
    }
    let Some(entry) = subscriber.history.peek() else {
        return false;
    };
    let frame = entry.slot as usize;
    if subscriber.qos.lifespan_ns != 0
        && now_ns.saturating_sub(frames[frame].admitted_ns) >= subscriber.qos.lifespan_ns
    {
        let expired = subscriber.history.pop().expect("queued frame");
        release_frame(expired.slot as usize, frames);
        return send_qos_event(
            control_slot,
            subscriber.supervision_slot,
            EVENT_LIFESPAN_EXPIRED,
            entry.sequence,
            0,
            now_ns,
            type_identity,
        );
    }
    if let Some(buffer_slot) = frames[frame].buffer_slot {
        let loan = match slime_rt::shared_buffer_loan(
            buffer_slot,
            subscriber
                .supervision_slot
                .unwrap_or(subscriber.control_slot),
            0,
            frames[frame].buffer_len,
            false,
        ) {
            Ok(loan) => loan,
            Err(ERR_WOULDBLOCK) | Err(ERR_OUT_OF_MEMORY) => return false,
            Err(_) => fail(b"downstream loan"),
        };
        let descriptor = WireSampleDescriptor {
            magic: SAMPLE_DESCRIPTOR_MAGIC,
            version: FORMAT_VERSION,
            flags: frames[frame].flags,
            capability_kind: CAPABILITY_KIND_LOAN,
            loan_id: loan.id,
            offset: 0,
            length: frames[frame].buffer_len,
            type_identity: frames[frame].type_identity,
            sequence: frames[frame].sequence,
            reserved: [0; 8],
        };
        if slime_rt::capability_delegate(
            control_slot,
            loan.slot,
            CapabilityDisposition::Move,
            OBJECT_KIND_SHARED_BUFFER_LOAN,
            RIGHT_BUFFER_MAP,
            &descriptor.encode(),
        ) != ERR_SUCCESS
        {
            let _ = slime_rt::shared_buffer_revoke(buffer_slot, loan.id);
            return false;
        }
        slime_rt::debug_write(b"[fabric] downstream loan created\n");
    } else {
        let bytes = unsafe {
            core::slice::from_raw_parts_mut(subscriber.ring_base as *mut u8, PAGE as usize)
        };
        let mut ring = Ring::attach(bytes, type_identity, subscriber.ring_slots)
            .unwrap_or_else(|_| fail(b"subscriber ring attach"));
        match ring.publish(
            &frames[frame].payload[..frames[frame].payload_len],
            frames[frame].flags & FLAG_LAST != 0,
        ) {
            Ok(_) => {}
            Err(RingError::Full) if subscriber.qos.reliability as u32 != RELIABILITY_RELIABLE => {
                let mut dropped = [0u8; slime_proto::fabric_ring::MAX_INLINE_BYTES];
                ring.consume(&mut dropped)
                    .unwrap_or_else(|_| fail(b"best effort drop"));
                ring.publish(
                    &frames[frame].payload[..frames[frame].payload_len],
                    frames[frame].flags & FLAG_LAST != 0,
                )
                .unwrap_or_else(|_| fail(b"best effort publish"));
            }
            Err(RingError::Full) => {
                slime_rt::debug_write(b"[fabric] terminal delivery ring backpressured\n");
                return false;
            }
            Err(_) => fail(b"subscriber ring publish"),
        }
        let _ = slime_rt::notification_signal(subscriber.ready_slot);
    }
    subscriber.history.pop();
    release_frame(frame, frames);
    subscriber.deadline_reported = false;
    // A RELIABLE subscriber owes an acknowledgement for what it was sent, and
    // this is where the sample becomes outstanding. Without it `in_flight` was
    // only ever decremented, so it could not leave zero -- and every rule that
    // reads it (retry accounting, retry exhaustion, and holding the terminal
    // event back until the queue drains) was unreachable for that reason
    // rather than because the condition did not hold.
    if subscriber.qos.reliability as u32 == RELIABILITY_RELIABLE {
        subscriber.in_flight = subscriber.in_flight.saturating_add(1);
    }
    true
}

fn drain_acks(
    index: usize,
    _type_tags: &[u64; ROUTE_COUNT],
    subscribers: &mut [Option<Subscriber>],
    _frames: &mut [Frame],
) -> bool {
    let Some(subscriber) = subscribers[index].as_mut() else {
        return false;
    };
    // The credit notification is the subscriber saying it consumed what it was
    // given, which is what retires the outstanding count. A signal is a level,
    // not a tally -- it coalesces -- so this clears the balance rather than
    // decrementing once per delivery.
    if matches!(
        slime_rt::notification_poll(subscriber.credit_slot),
        Ok(Some(_))
    ) {
        subscriber.in_flight = 0;
        return true;
    }
    false
}

/// Emit one terminal event for a finished route, once per subscriber.
fn announce_end(
    index: usize,
    route: usize,
    type_tags: &[u64; ROUTE_COUNT],
    subscribers: &mut [Option<Subscriber>],
) -> bool {
    let Some(subscriber) = subscribers[index].as_mut() else {
        return false;
    };
    if subscriber.route != route || subscriber.ended {
        return false;
    }
    // Everything queued must reach the subscriber before it is told the stream
    // ended, except after a declared terminal QoS transition has reclaimed the
    // queue itself. A lifespan-expired publisher is already absent from this
    // subscriber's queue, so retained history cannot keep it waiting.
    if !subscriber.terminal && (!subscriber.history.is_empty() || subscriber.in_flight != 0) {
        return false;
    }
    let event = WireStreamEvent {
        magic: STREAM_EVENT_MAGIC,
        version: FORMAT_VERSION,
        event: EVENT_STREAM_END,
        flags: 0,
        lost: 0,
        sequence: 0,
        type_identity: type_tags[route],
        reserved: [0; 24],
    };
    // Terminal information the subscriber genuinely waits for, so it is
    // re-offered every pass rather than sent once: `seL4_NBSend` discards the
    // message when the peer is not yet blocked on receive and reports nothing
    // either way, so a single attempt would silently lose it. The route is
    // retired only when the peer is gone, which is the one observation that
    // means it can no longer be waiting. The broker's own loop supplies the
    // retry, so this never spins.
    let _ = slime_rt::try_send(subscriber.control_slot, &event.encode(), &[]);
    if let Some(supervision_slot) = subscriber.supervision_slot {
        if matches!(slime_rt::supervision_status(supervision_slot), Ok(None)) {
            return false;
        }
    } else {
        // The restartable controller's owner is another component, so this
        // broker cannot hold its supervision handle. Its BEST_EFFORT ring is
        // safe to retire only after the receiver advanced the shared tail past
        // every sample; doing so earlier could reclaim the mapping under the
        // live replacement.
        let bytes = unsafe {
            core::slice::from_raw_parts_mut(subscriber.ring_base as *mut u8, PAGE as usize)
        };
        let ring = Ring::attach(bytes, type_tags[route], subscriber.ring_slots)
            .unwrap_or_else(|_| fail(b"subscriber ring attach"));
        if ring.occupancy() != 0 {
            return false;
        }
    }
    subscriber.ended = true;
    true
}

/// Drop the durable-history references once the broker is finished. Retained
/// samples are live only for this fabric instance; shutdown releases their
/// fixed frame and buffer charges before the component exits.
fn release_retained(publishers: &mut [Option<Publisher>], frames: &mut [Frame]) {
    for publisher in publishers.iter_mut().flatten() {
        while let Some(entry) = publisher.retained.pop() {
            release_frame(entry.slot as usize, frames);
        }
    }
}

/// Whether every publisher declared on `route` has finished or died.
fn route_finished(route: usize, publishers: &[Option<Publisher>]) -> bool {
    publishers
        .iter()
        .flatten()
        .filter(|publisher| publisher.route == route)
        .all(|publisher| publisher.finished)
}

/// Drop one reference to a fabric frame, releasing its storage at zero.
fn release_frame(frame: usize, frames: &mut [Frame]) {
    if frames[frame].refs == 0 {
        return;
    }
    frames[frame].refs -= 1;
    if frames[frame].refs != 0 {
        return;
    }
    if let Some(buffer_slot) = frames[frame].buffer_slot {
        // Release the fabric's own copy. Pages stay retained by the kernel
        // while any downstream loan is outstanding, so a subscriber still
        // mapping this sample keeps working and the charge settles when it
        // returns its loan.
        let _ = slime_rt::shared_buffer_release(buffer_slot);
    }
    frames[frame] = Frame::EMPTY;
}

/// Drop any capability that arrived on a message that had no business carrying
/// one, so a malformed peer cannot strand kernel objects in the fabric.
fn release_received(received: &[u64]) {
    for slot in received.iter().filter(|slot| **slot != 0) {
        let _ = slime_rt::cap_drop(*slot as u32);
    }
}

/// The (ready, credit) notification slot pair the generation declares for one
/// (component, route) edge, or `NOTIFICATION_ABSENT` on both sides when it
/// declares none.
///
/// Resolved through the root by the grant names the generation gives the pair —
/// `<component>-<route>-ready` and `-credit` — rather than read out of a
/// per-plane table (B70/CP2). One notification grant binds a slot in *both*
/// peers, and the root answers per holder, so asking by the grant name returns
/// this broker's own slot: the participant asking for the same name gets its
/// own. That is why the direction argument is gone. It selected a row in the
/// generated table; here the holder scope is what distinguishes the two ends,
/// and a direction would only restate which end this component is.
///
/// Declaring no pair is a legitimate graph shape, not a defect: a full-graph
/// boot profile can provision a stream role over its control endpoint without
/// ever driving samples through it, and `boot_graph`'s participants never reach
/// the broker loop that would read these slots. The two callers that do read
/// them — `pump_publisher` and delivery — only run for a participant whose
/// graph declared the pair, so the sentinel is never dereferenced there.
fn notification_slots(component: &[u8], route: &str) -> (u32, u32) {
    let ready = notification_slot(component, route, b"-ready");
    let credit = notification_slot(component, route, b"-credit");
    // Half a pair is not a usable edge: a ring that can signal readiness but
    // never take credit stalls its peer instead of failing. Both or neither.
    match (ready, credit) {
        (Some(ready), Some(credit)) => (ready, credit),
        (None, None) => (NOTIFICATION_ABSENT, NOTIFICATION_ABSENT),
        _ => fail(b"generation declares half a notification pair"),
    }
}

/// One `notification:<component>-<route><suffix>` slot, or `None` where this
/// generation declares the grant no binding for this holder.
pub(crate) fn notification_slot(component: &[u8], route: &str, suffix: &[u8]) -> Option<u32> {
    const PREFIX: &[u8] = b"notification:";
    let route = route.as_bytes();
    let mut name = [0u8; 64];
    let end = PREFIX.len() + component.len() + 1 + route.len() + suffix.len();
    if end > name.len() {
        fail(b"notification name exceeds bound");
    }
    let mut cursor = 0;
    for part in [PREFIX, component, b"-", route, suffix] {
        name[cursor..cursor + part.len()].copy_from_slice(part);
        cursor += part.len();
    }
    slime_rt::resolve_binding(&name[..end]).ok()
}

/// How many edges on a route *this service carries* the generation declares for
/// `component`. Zero is a denial: authority is never ambient, so absence from
/// the table is not a default role — and a component declared only on a call or
/// operation route holds no stream authority either.
fn declared_edges(rows: &[slime_components::fabric_self_view::Row], component: &[u8]) -> usize {
    let identity = boot_contracts::fabric_graph::component_identity(
        core::str::from_utf8(component).unwrap_or_else(|_| fail(b"component name is not utf-8")),
    );
    rows.iter()
        .filter(|row| {
            row.component_identity == identity && local_route_index(row.route_index).is_some()
        })
        .count()
}

/// This service's own index for a route the graph names by *its* index.
///
/// The two orderings differ and neither is derivable from the other: the
/// resource sorts routes by identity, while `ROUTE_NAMES` is this service's
/// dispatch order. Translating through the identity both sides agree on is what
/// keeps a row's route meaningful here — and it is also the "does this service
/// carry that route" test, since a call or operation route folds an identity
/// `ROUTE_NAMES` never names and so resolves to nothing.
fn local_route_index(graph_route: u32) -> Option<usize> {
    (0..ROUTE_COUNT).find(|local| graph_route_index_of(*local) == Some(graph_route))
}

/// The graph's index for one of this service's own routes.
fn graph_route_index_of(local: usize) -> Option<u32> {
    let identity = route_identity(
        ROUTE_NAMES[local],
        if local == 0 {
            &telemetry_stream::INTERFACE_IDENTITY
        } else {
            &diagnostics_stream::INTERFACE_IDENTITY
        },
        CONTRACT_KIND_STREAM,
    );
    slime_rt::graph_route_index(&identity)
        .ok()
        .map(|i| i as u32)
}

fn refresh_matches(
    route: usize,
    publishers: &[Option<Publisher>],
    subscribers: &mut [Option<Subscriber>],
) {
    for subscriber in subscribers
        .iter_mut()
        .flatten()
        .filter(|subscriber| subscriber.route == route)
    {
        let old = subscriber.matched_publishers;
        let matched = publishers
            .iter()
            .flatten()
            .filter(|publisher| publisher.route == route)
            .filter(|publisher| TransportQos::offer_satisfies(&publisher.qos, &subscriber.qos))
            .count() as u32;
        let incompatible = publishers
            .iter()
            .flatten()
            .filter(|publisher| publisher.route == route)
            .count() as u32
            - matched;
        subscriber.matched_publishers = matched;
        if matched != old {
            let event = if matched == 0 {
                EVENT_UNMATCHED
            } else {
                EVENT_MATCHED
            };
            if send_qos_event(
                subscriber.control_slot,
                subscriber.supervision_slot,
                event,
                0,
                matched as u64,
                0,
                route_type_tag(route),
            ) {
                slime_rt::debug_write(if event == EVENT_MATCHED {
                    b"[fabric] QoS matched\n" as &[u8]
                } else {
                    b"[fabric] QoS unmatched\n"
                });
            }
        }
        if incompatible != 0
            && send_qos_event(
                subscriber.control_slot,
                subscriber.supervision_slot,
                EVENT_INCOMPATIBLE_QOS,
                0,
                incompatible as u64,
                0,
                route_type_tag(route),
            )
        {
            slime_rt::debug_write(b"[fabric] QoS incompatible\n");
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TimeReceive {
    WouldBlock,
    PeerDead,
    Advance(u64),
}

const fn update_time_liveness(
    pending_time: &mut Option<u64>,
    time_dead: &mut bool,
    received: TimeReceive,
) {
    match received {
        TimeReceive::WouldBlock => {}
        TimeReceive::PeerDead => *time_dead = true,
        TimeReceive::Advance(now) => *pending_time = Some(now),
    }
}

fn receive_time(pending_time: &mut Option<u64>, time_dead: &mut bool) {
    if pending_time.is_some() || *time_dead {
        return;
    }
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    let length = match slime_rt::recv(time_slot(), &mut bytes, &mut caps) {
        ERR_WOULDBLOCK => {
            // A native Endpoint has no `ERR_PEER_DEAD`: an exited clock is
            // indistinguishable from a silent one, so a receive alone can never
            // retire this input and the broker would run forever waiting for a
            // time source that is gone. The clock peer's supervision handle is
            // the observation that reports the difference, which is the same
            // answer publisher death needed above.
            if !matches!(
                slime_rt::supervision_status(supervision_slot_for(TIME_COMPONENT)),
                Ok(None)
            ) {
                update_time_liveness(pending_time, time_dead, TimeReceive::PeerDead);
                return;
            }
            update_time_liveness(pending_time, time_dead, TimeReceive::WouldBlock);
            return;
        }
        // No `ERR_PEER_DEAD` arm. The status exists in the ABI but nothing on
        // this transport produces it, so an arm here was unreachable
        // redundancy beside the supervision check above -- which is the whole
        // detection mechanism, not a fallback (B76). `TimeReceive::PeerDead`
        // remains: it is this function's own classification of what that check
        // found, not a status read off the endpoint.
        n if n < 0 => fail(b"time recv"),
        n => n as usize,
    };
    release_received(&caps);
    let Some(value) = WireTimeAdvance::decode(&bytes[..length]) else {
        fail(b"time decode")
    };
    if !slime_proto::valid_time_advance(&value) {
        fail(b"non-monotonic time")
    }
    update_time_liveness(pending_time, time_dead, TimeReceive::Advance(value.now_ns));
}

fn apply_time(
    now_ns: &mut u64,
    pending_time: &mut Option<u64>,
    publishers: &mut [Option<Publisher>],
    subscribers: &mut [Option<Subscriber>],
    frames: &mut [Frame],
) -> bool {
    let Some(next) = pending_time.take() else {
        return false;
    };
    if next < *now_ns {
        fail(b"non-monotonic time")
    }
    *now_ns = next;

    // Tie order after the broker's data/ack sweep: lifespan, retry exhaustion,
    // deadline, then liveliness/lease.
    for subscriber in subscribers.iter_mut().flatten() {
        while let Some(entry) = subscriber.history.peek() {
            let frame = entry.slot as usize;
            if subscriber.qos.lifespan_ns == 0
                || now_ns.saturating_sub(frames[frame].admitted_ns) < subscriber.qos.lifespan_ns
            {
                break;
            }
            let expired = subscriber.history.pop().expect("queued frame");
            subscriber.in_flight = subscriber.in_flight.saturating_sub(1);
            let publisher_index = expired.publisher as usize;
            if publishers.get(publisher_index).is_none_or(Option::is_none) {
                fail(b"expired sample has no publisher");
            }
            release_frame(expired.slot as usize, frames);
            if send_qos_event(
                subscriber.control_slot,
                subscriber.supervision_slot,
                EVENT_LIFESPAN_EXPIRED,
                expired.sequence,
                0,
                *now_ns,
                route_type_tag(subscriber.route),
            ) {
                slime_rt::debug_write(b"[fabric] QoS lifespan expired\n");
            }
        }
    }

    for subscriber in subscribers.iter_mut().flatten() {
        if subscriber.terminal
            || subscriber.qos.reliability as u32 != RELIABILITY_RELIABLE
            || subscriber.in_flight == 0
        {
            continue;
        }
        if now_ns.saturating_sub(subscriber.last_retry_ns) < subscriber.retry_interval_ns {
            continue;
        }
        subscriber.retry_count = subscriber.retry_count.saturating_add(1);
        subscriber.last_retry_ns = *now_ns;
        slime_rt::debug_write(b"[fabric] reliable retry accounted\n");
        if subscriber.retry_count < 4 {
            continue;
        }

        let mut exhausted = None;
        while let Some(entry) = subscriber.history.pop() {
            exhausted.get_or_insert(entry.sequence);
            release_frame(entry.slot as usize, frames);
        }
        subscriber.in_flight = 0;
        subscriber.terminal = true;
        // Retry exhaustion is a statement about the retries, not about a frame
        // that happens to survive them: an earlier lifespan expiry can already
        // have drained this queue, and reporting nothing then would make the
        // condition invisible exactly when it was reached the hard way. The
        // sequence is the last sample still queued if there is one, and zero
        // when the queue is already empty.
        if send_qos_event(
            subscriber.control_slot,
            subscriber.supervision_slot,
            EVENT_RETRY_EXHAUSTED,
            exhausted.unwrap_or(0),
            subscriber.retry_count as u64,
            *now_ns,
            route_type_tag(subscriber.route),
        ) {
            slime_rt::debug_write(b"[fabric] QoS retry exhausted\n");
        }
    }

    for subscriber in subscribers.iter_mut().flatten() {
        if subscriber.qos.deadline_ns != 0
            && !subscriber.deadline_reported
            && *now_ns >= subscriber.qos.deadline_ns
        {
            subscriber.deadline_reported = true;
            if send_qos_event(
                subscriber.control_slot,
                subscriber.supervision_slot,
                EVENT_DEADLINE_MISSED,
                0,
                0,
                *now_ns,
                route_type_tag(subscriber.route),
            ) {
                slime_rt::debug_write(b"[fabric] QoS deadline missed\n");
            }
        }
    }

    for publisher in publishers.iter().flatten() {
        if publisher.qos.lease_ns != 0
            && now_ns.saturating_sub(publisher.last_assertion_ns) >= publisher.qos.lease_ns
        {
            for subscriber in subscribers.iter_mut().flatten().filter(|subscriber| {
                subscriber.route == publisher.route && !subscriber.liveliness_reported
            }) {
                subscriber.liveliness_reported = true;
                if send_qos_event(
                    subscriber.control_slot,
                    subscriber.supervision_slot,
                    EVENT_LIVELINESS_LOST,
                    0,
                    0,
                    *now_ns,
                    route_type_tag(subscriber.route),
                ) {
                    slime_rt::debug_write(b"[fabric] QoS liveliness lost\n");
                }
            }
        }
    }
    let credit = WireTimeAdvance {
        magic: slime_proto::fabric_time::TIME_ADVANCE_MAGIC,
        version: slime_proto::fabric_time::FORMAT_VERSION,
        flags: 0,
        reserved0: 0,
        now_ns: *now_ns,
        reserved: [0; 40],
    };
    match slime_rt::send(time_slot(), &credit.encode(), &[]) {
        ERR_SUCCESS => {}
        ERR_WOULDBLOCK => fail(b"time credit blocked"),
        _ => fail(b"time credit"),
    }
    true
}

fn route_type_tag(route: usize) -> u64 {
    match route {
        0 => telemetry_stream::TYPE_TAG,
        1 => diagnostics_stream::TYPE_TAG,
        _ => fail(b"route tag"),
    }
}

/// Deliver one QoS event to a subscriber, blocking until it is taken.
///
/// A declared QoS condition is an obligation, not a hint: the plane's contract
/// is that the subscriber observes each one. `seL4_NBSend` cannot carry an
/// obligation — it delivers only to a peer *already* blocked on the endpoint,
/// discards otherwise, and reports nothing either way, so `try_send` returns
/// `ERR_SUCCESS` for "attempted" and the caller cannot tell a delivery from a
/// drop. Retaining and re-offering was built on that distinction and could
/// never fire.
///
/// A blocking send is the primitive that carries one, and it is what the
/// `EVENT_SAMPLE_LOST` path above already uses on this same endpoint. It
/// rendezvous safely because every reader on the other side returns to its
/// control endpoint once its ring is drained, and a two-route reader files
/// whichever record arrives under the route that record names.
///
/// The one case a blocking send cannot survive is a peer that will never
/// receive again. A native Endpoint does not report that — there is no
/// `ERR_PEER_DEAD` to read — so the subscriber's supervision handle is checked
/// first, which is the same answer peer death needed everywhere else in this
/// cutover. `false` therefore means "this subscriber is gone", not "try later".
fn send_qos_event(
    slot: u32,
    supervision_slot: Option<u32>,
    event: u32,
    sequence: u64,
    value: u64,
    timestamp_ns: u64,
    type_identity: u64,
) -> bool {
    // `Ok(None)` is "still running". Anything else means the peer has
    // terminated and no send to it can ever rendezvous. A composition-owned
    // restartable role may instead be endpoint-bound because its supervisor is
    // another task. It deliberately skips blocking QoS records: without a
    // supervision observation there is no safe proof that a send can
    // rendezvous, and blocking the broker here would wedge the whole plane.
    let Some(supervision_slot) = supervision_slot else {
        return false;
    };
    if !matches!(slime_rt::supervision_status(supervision_slot), Ok(None)) {
        return false;
    }
    let record = WireQosEvent {
        magic: QOS_EVENT_MAGIC,
        version: QOS_FORMAT_VERSION,
        event,
        flags: 0,
        sequence,
        value,
        timestamp_ns,
        type_identity,
        reserved: [0; 16],
    };
    match slime_rt::send(slot, &record.encode(), &[]) {
        ERR_SUCCESS => true,
        // No `ERR_PEER_DEAD` arm: the supervision check above is what reports a
        // gone subscriber; the endpoint never does (B76).
        _ => fail(b"QoS event"),
    }
}

/// The supervision handle init granted the fabric for one subscriber. Init
/// spawns each client and hands the fabric its supervision capability, so the
/// fabric can name a loan receiver by capability rather than by task id.
///
/// Resolved from the root at runtime rather than read out of a generated table:
/// a supervision binding is named for the task it supervises
/// (`<supervised-instance>-supervision`, asserted by
/// `build-generation.py`'s `validate_supervision_binding_names`), so the
/// component this function is already given *is* the name to ask for. That name
/// means the same thing under every generation declaring it, which the three
/// spellings this convention replaced did not.
fn supervision_slot_for(component: &'static [u8]) -> u32 {
    if let Some(slot) = memoized_supervision_slot(component) {
        return slot;
    }
    let slot = resolve_supervision_slot(component);
    memoize_supervision_slot(component, slot);
    slot
}

/// Component identities are `&'static [u8]` — every caller passes a row of the
/// generated profile or a `const` in this file — so the memo stores the identity
/// itself and needs no copy and no unsafe lifetime widening. Requiring `'static`
/// at the signature is what makes that sound rather than asserted.
///
/// Sized above what this memo can ever hold, which is not the size of a plane's
/// supervision table: entries are added only by `provision_edge` and by the
/// `TIME_COMPONENT` clock read, so the ceiling is `MAX_PARTICIPANTS + 1` = 8,
/// derived from the generation's own declared publisher and subscriber counts.
/// (An earlier comment here justified 12 as headroom over "7, the largest
/// supervision table"; that count was wrong -- `sel4-boot.zti` and
/// `sel4-traffic.zti` each declare 13 -- and it was the wrong set to count.)
/// Overflow degrades to re-resolving rather than failing: the memo is an
/// optimization, and a graph past this bound should get slower, not refuse to
/// boot.
static mut SUPERVISION_MEMO: [(&[u8], u32); 12] = [(b"", u32::MAX); 12];
static mut SUPERVISION_MEMO_LEN: usize = 0;

fn memoized_supervision_slot(component: &'static [u8]) -> Option<u32> {
    // SAFETY: single-threaded, and every caller is on the one dispatch loop.
    let (memo, len) = unsafe {
        (
            core::ptr::addr_of!(SUPERVISION_MEMO).read(),
            *core::ptr::addr_of!(SUPERVISION_MEMO_LEN),
        )
    };
    memo[..len]
        .iter()
        .find(|(name, _)| *name == component)
        .map(|(_, slot)| *slot)
}

fn memoize_supervision_slot(component: &'static [u8], slot: u32) {
    // SAFETY: as above.
    unsafe {
        let len = *core::ptr::addr_of!(SUPERVISION_MEMO_LEN);
        let memo = &mut *core::ptr::addr_of_mut!(SUPERVISION_MEMO);
        if let Some(entry) = memo.get_mut(len) {
            *entry = (component, slot);
            *core::ptr::addr_of_mut!(SUPERVISION_MEMO_LEN) = len + 1;
        }
    }
}

/// Ask the root, formatting the `minted:` name the convention fixes.
///
/// Split from the memo so the resolve path stays readable: this is what runs
/// once per component, and `supervision_slot_for` is what the hot loops call.
fn resolve_supervision_slot(component: &'static [u8]) -> u32 {
    // `minted:` + the longest component name + `-supervision`. A supervision
    // object cannot exist before the task it names, so the generation declares
    // where it will land rather than granting it, and `minted:` is the axis that
    // reads that table.
    const PREFIX: &[u8] = b"minted:";
    const SUFFIX: &[u8] = b"-supervision";
    // 64 is `SUPERVISION_RESOLVE_NAME_BYTES` in `build-generation.py`, which
    // refuses any supervision binding whose resolve string would not fit here.
    // A `no_std` component has no allocator, so this buffer is fixed; bounding it
    // at build time is what keeps the `fail()` below unreachable on a generation
    // the builder accepted, rather than a boot-time surprise.
    let mut name = [0u8; 64];
    let end = PREFIX.len() + component.len() + SUFFIX.len();
    if end > name.len() {
        fail(b"supervision name exceeds bound");
    }
    name[..PREFIX.len()].copy_from_slice(PREFIX);
    name[PREFIX.len()..PREFIX.len() + component.len()].copy_from_slice(component);
    name[PREFIX.len() + component.len()..end].copy_from_slice(SUFFIX);
    slime_rt::resolve_binding(&name[..end])
        .unwrap_or_else(|_| fail(b"subscriber has no supervision handle"))
}

/// Answer a request the graph does not authorize. A denial is the same record
/// with a nonzero status, an empty rights mask, and no capability attached: the
/// caller learns it was refused without learning anything about the route.
fn deny(control_slot: u32, route: &[u8; 32], status: i32) {
    let descriptor = WireCapabilityTransfer {
        magic: CAPABILITY_TRANSFER_MAGIC,
        version: FORMAT_VERSION,
        status,
        flags: 0,
        object_kind: 0,
        direction: 0,
        rights_mask: 0,
        route_identity: *route,
    };
    if slime_rt::send(control_slot, &descriptor.encode(), &[]) < 0 {
        fail(b"deny reply");
    }
}

const _: () = assert!(REQUEST_LEN == MAX_MSG);
const _: () = assert!(TRANSFER_LEN == MAX_MSG);
const _: () = assert!(slime_proto::fabric_stream::EVENT_LEN == MAX_MSG);
// The frame table must cover every reference the declared rings can hold at
// once, or a full set of rings would leave the fabric with no free frame while
// its publishers block. That is a deadlock rather than backpressure, so the
// table is sized to make it unreachable.
//
// `FRAME_CAPACITY` is the contract's own published number
// (`contracts/fabric-graph/v1`), which is also what `build-generation.py`
// admits a manifest against: it sums `historyDepth` over every
// `DIRECTION_SUBSCRIBE` participant and refuses a graph exceeding it. This file
// read that number out of a per-plane generated profile, so the builder's
// admission bound and the array it protected were two copies. They are now one
// declaration, and `admit_declared_frames` above re-checks the same sum at boot
// against the same constant -- which is what an out-of-tree component gets,
// since it cannot rely on this repository's builder having run.
//
// The contract's absolute worst case, `MAX_PARTICIPANTS * LIMIT_HISTORY_DEPTH`,
// is far larger than this table. That ceiling is deliberately not the array
// size: it is why `admit_*` refuses a sample when no frame is free and settles
// the publisher's loan instead of blocking, and why a graph declaring more than
// this table holds is refused at boot rather than deadlocked at run time.
const _: () = assert!(
    MAX_FRAMES
        <= boot_contracts::fabric_graph::MAX_PARTICIPANTS
            * boot_contracts::fabric_graph::LIMIT_HISTORY_DEPTH as usize
);

#[cfg(test)]
mod tests {
    use super::{TimeReceive, update_time_liveness};

    #[test]
    fn queued_advance_is_preserved_as_application_data() {
        let mut pending = None;
        let mut dead = false;
        update_time_liveness(&mut pending, &mut dead, TimeReceive::Advance(42));
        assert_eq!(pending, Some(42));
        assert!(!dead);
    }

    #[test]
    fn only_peer_dead_marks_clock_dead() {
        let mut pending = None;
        let mut dead = false;
        update_time_liveness(&mut pending, &mut dead, TimeReceive::WouldBlock);
        assert!(!dead);
        update_time_liveness(&mut pending, &mut dead, TimeReceive::PeerDead);
        assert!(dead);
    }
}

// The graph contract and the trace contract state the same three facts, and
// `declared_trace_depth` checks the graph's spelling while the storage it
// protects is sized by the trace's. Pinned here so a divergence fails this
// build rather than a boot: a depth admitted by one and refused by the other
// would be a `TraceSink` assert firing as a boot panic inside a `no_std`
// component. These items are evaluated at compile time unconditionally.
const _: () = assert!(
    boot_contracts::fabric_graph::LIMIT_TRACE_DEPTH as usize
        == slime_proto::fabric_trace::MAX_TRACE_DEPTH
);
const _: () = assert!(
    boot_contracts::fabric_graph::TRACE_TERMINAL_RESERVE as usize
        == slime_proto::fabric_trace::TERMINAL_RESERVE
);
const _: () = assert!(
    boot_contracts::fabric_graph::TRACE_OVERFLOW_SATURATE
        == slime_proto::fabric_trace::OVERFLOW_SATURATE
);
