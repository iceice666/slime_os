//! C8.13.2: one participant's own shared-buffer occupancy, as trace evidence.
//!
//! C8.13.1 gave the stream broker this evidence by hand, because a broker has a
//! sweep loop to sample from and several counters to report. A participant has
//! neither: it runs a scripted sequence, holds exactly one occupancy worth
//! reporting, and ends. Four of them need the identical shape, so it lives here
//! once instead of four times.
//!
//! Included per binary with `#[path]`, beside `fabric_trace_log.rs` and for the
//! same reason: a file may be a module only once per crate, and the trace sink
//! it builds on is itself a binary-local module rather than part of
//! `slime_components`.
//!
//! # What a participant reports
//!
//! Its own mapping count at the instant it reports, measured across a traffic
//! boot as `fabric-publisher` 1, `fabric-subscriber` 1, `fabric-subscriber-b`
//! 2, `fabric-publisher-b` 2 — one region per declared route. That steady-state
//! number is the role's provisioned shape, which is why the gate pins the exact
//! value per participant rather than accepting any nonzero constant.
//!
//! It is a steady state, not an invariant held throughout. Three of the four
//! transiently hold more: both subscribers map an arriving loan and unmap it
//! again (`fabric-subscriber.rs:422`/`:431`,
//! `fabric-subscriber-b.rs:670`/`:681`), since `map_loan` charges the
//! receiver's mapping counter; and `fabric-publisher-b` creates, maps, seals,
//! and lends a copy for its `>MAX_MSG` sample, holding a third mapping plus a
//! buffer and a *lender* loan charge until the fabric settles it
//! (`fabric-publisher-b.rs:444` — the root charges the lender, which is why the
//! fixture declares it `loanCount = 1` where the other three declare 0). Only
//! `fabric-publisher` never moves.
//!
//! Those peaks are deliberately unreported rather than missed. A participant
//! emits once, at the end, because serial writes must stay off the path of the
//! traffic they describe, so the only number it can honestly report is the one
//! it holds then. Reporting a sampled peak would mean querying per sweep, which
//! is what a broker does and what a scripted participant has no loop for.
//!
//! No `loan` record is emitted for any of the four: three never lend, and
//! `fabric-publisher-b`'s single lend has drained by emission time, so a loan
//! pair here could only be a measured `[0, 0]` — the degenerate evidence the
//! trace schema rules out. The traffic-varying half of this evidence is the
//! fabric's own loan count, which C8.13.1 already reports.
//!
//! # Why emission is one call at the end
//!
//! `fabric_trace_log`'s contract: recording performs no IPC, `flush` is the
//! only serial writer, and it runs once after the work it describes is done. A
//! participant that flushed mid-script would put a root round trip on the path
//! of the traffic it reports on. No reply capability can be pending at that
//! point, and the condition worth propagating is about the *peers*, not about
//! these four: on the non-MCS kernel a thread holds a reply capability as the
//! callee of someone else's `seL4_Call`, latched by the receive that took the
//! message. Every message reaching these participants is a `seL4_Send` or
//! `seL4_NBSend` from the fabric (`fabric-service.rs` uses `slime_rt::send` and
//! `try_send` exclusively), which stores no reply capability. So the query
//! cannot clobber a pending reply the way it could inside a call broker, which
//! does hold one and therefore replies before it records. A participant added
//! here later that receives from a `slime_rt::call`ing peer would *not* be
//! safe, and must reply before it reports.

#![allow(dead_code)]

use super::trace_log::Trace;
use slime_proto::fabric_trace::RESOURCE_MAPPING;

/// The declared depth is inside the contract, checked at build time.
///
/// `TraceSink::with_const_capacity` asserts the same bound, but it is reached
/// from `fn main`, so its assert evaluates at runtime and an over-declared
/// depth would be a boot panic instead of a failed build. Every trace host
/// carries this pair for that reason (`call_broker.rs`, `fabric-service.rs`);
/// hosting it here covers all four participants that include this module,
/// since each supplies the same generation constant.
const _: () = assert!(super::FABRIC_TRACE_DEPTH <= slime_proto::fabric_trace::MAX_TRACE_DEPTH);
const _: () = assert!(super::FABRIC_TRACE_DEPTH > slime_proto::fabric_trace::TERMINAL_RESERVE);

/// Emit one participant's occupancy evidence and close its trace.
///
/// `family` is the name the gate keys records by, and it is the short role name
/// (`publisher`, `subscriber-b`) rather than the component name, matching the
/// brokers' own `stream`/`call`/`operation`. Not a capacity constraint: the
/// longest record line is about 231 bytes before the name, so even
/// `fabric-subscriber-b` would fit `Line`'s 256 bytes. The reason is that the
/// family is the trace's own axis — a role in the graph — and a trace that
/// named components rather than roles would not compare across graphs that
/// bind the same role to a different component.
///
/// `depth` is the generation's declared `traceDepth`, taken as an argument
/// because `Trace::new` takes one. That signature exists so `fabric_trace_log`
/// stays generation-independent for binaries a fabric-less manifest still
/// builds; this module is not one of those — its `const _: ()` guards above
/// read `FABRIC_TRACE_DEPTH` directly, so it compiles only where a fabric is
/// declared, exactly as the brokers do.
///
/// Emits nothing when the query is refused. Under the traffic action every
/// caller is a declared `sharedBufferBudget` holder, so the root's only refusal
/// path — an undeclared holder's deny-by-default quota — is unreachable here: a
/// refusal would mean the query, the badge-to-holder mapping, or the budget
/// regressed. Reporting nothing rather than a fabricated zero is what makes
/// that visible, since the gate then fails on this family's empty record set
/// instead of accepting a zero it cannot distinguish from a measurement.
pub fn report(family: &[u8], depth: usize) {
    let Ok(occupancy) = slime_rt::shared_buffer_occupancy() else {
        return;
    };
    let mut trace = Trace::new(depth);
    // The same observation twice, deliberately, and the consequence is stated
    // rather than glossed: the pair is byte-identical by construction, so a
    // reader must not take the equality as a measured drain. The
    // held-and-released convention wants two records under one counter, and for
    // a count that is fixed at the reporting point the honest pair is one
    // number twice. Two separate reads here would be two syscalls describing
    // one unchanged fact while *looking* like a peak and a drained baseline,
    // which is the stronger claim this cannot support. What carries the
    // evidence is the value itself, which the gate pins per participant.
    let _ = trace.resource(RESOURCE_MAPPING, occupancy.mappings);
    let _ = trace.resource(RESOURCE_MAPPING, occupancy.mappings);
    let _ = trace.terminal();
    trace.flush(family);
}
