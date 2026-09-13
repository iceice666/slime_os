//! Startup self-check proving a component's private-memory quota is live and
//! usable through ordinary Rust collections (C10.3).
//!
//! The C7 shared-buffer probe's shape, and for the same reason: a mechanism
//! exercised only by `slime-root`'s host unit tests is not known to work for
//! components (backlog B5). C10.2 already proved the *declared ceiling* is the
//! live one by growing raw pages. What that cannot show is the thing C10.3 adds
//! — that the ceiling is reachable by `Vec`, `Box`, and `String`, that freeing
//! returns memory the allocator hands out again, and that hitting the ceiling is
//! an error a component observes rather than a fault.
//!
//! Deliberately bounded and peer-independent, like the C7 probe: it allocates
//! inside the component's own declared quota, frees everything it took, and
//! needs nothing else to be running, so it is safe before a component enters its
//! main loop.
//!
//! # Why this lives beside the allocator
//!
//! The C7 probe sits in `components/lib`, which every component links. This one
//! cannot: it calls `alloc`, so it is only compilable where a
//! `#[global_allocator]` exists, and `just component_crate_split_check` pins
//! that `components/lib` must never name an allocator feature — a `heap` there
//! would put the allocator back in all 52 components. So it ships with the
//! allocator it checks, behind the same feature.
//!
//! # Why this reports instead of exiting
//!
//! [`probe`] returns an outcome and [`probe_and_report`] prints it; neither ends
//! the component. The caller decides, so "no quota declared" remains distinct
//! from failure for components that legitimately have none.

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use crate::private_heap::{GROWTH_PAGES, private_heap_stats};
use crate::runtime::GRANULE;
use crate::syscall::debug_write;

/// Result of the startup private-heap self-check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// Collections allocated, grew across a batch boundary, and freed memory the
    /// allocator reused. The declared quota is live.
    Ok,
    /// The generation declares no private-memory quota for this component, so it
    /// has no region at all. Not a failure: the deny-by-default answer.
    Denied,
    /// A collection failed to allocate before the check completed. The component
    /// has a region but less usable memory than the check needs.
    Exhausted,
    /// The allocator served the allocations but broke one of its own
    /// invariants — data did not survive, or growth was requested when the free
    /// list should have served the request.
    Failed,
}

/// What the check observed, alongside the outcome, so a caller can report
/// numbers rather than an adjective.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeReport {
    pub outcome: ProbeOutcome,
    /// Pages the root had backed when the check finished.
    pub pages: usize,
    /// Growth requests the root served during the check.
    pub growths: usize,
    /// Growth requests served during the *reuse* phase, which must be zero: the
    /// phase reallocates what it just freed, so a nonzero count is the free list
    /// failing to give memory back.
    pub reuse_growths: usize,
    /// Bytes still handed out after the check released everything. Zero unless
    /// the allocator lost a span.
    pub leaked: usize,
}

/// Exercise the component's private heap: grow across a batch boundary, prove
/// the data survives, then prove freed memory is reused without more growth.
///
/// `label` prefixes the one console line this emits — the reuse-phase boundary,
/// which a gate needs in order to attribute the root's growth records to a
/// phase. Taking it as a parameter rather than hardcoding one keeps the label
/// the caller's, exactly as [`probe_and_report`]'s is.
pub fn probe(label: &[u8]) -> ProbeReport {
    let start = private_heap_stats();
    let mut report = ProbeReport {
        outcome: ProbeOutcome::Ok,
        pages: start.pages,
        growths: start.growths,
        reuse_growths: 0,
        leaked: 0,
    };
    if start.base == 0 {
        report.outcome = ProbeOutcome::Denied;
        return report;
    }

    // Phase 1: grow a `Vec` past one batch. `try_reserve` rather than `push`
    // alone, because the point is to observe exhaustion rather than to be
    // terminated by it — and reallocation is what moves data, which phase 2
    // checks survived.
    let elements = (GROWTH_PAGES * GRANULE) / size_of::<u64>() + 64;
    let mut numbers: Vec<u64> = Vec::new();
    for index in 0..elements {
        if numbers.try_reserve(1).is_err() {
            report.outcome = ProbeOutcome::Exhausted;
            report.pages = private_heap_stats().pages;
            return report;
        }
        numbers.push(index as u64);
    }

    // Phase 2: the data survived every reallocation, including the ones that
    // crossed a growth. A `Box` and a `String` alongside it, because they are
    // the other two allocation shapes a component actually uses and they take
    // different paths through `Layout` — a boxed value is one exact-size
    // allocation, a `String` is a byte buffer that reallocates on push.
    for (index, value) in numbers.iter().enumerate() {
        if *value != index as u64 {
            report.outcome = ProbeOutcome::Failed;
            return report;
        }
    }
    let boxed = Box::new([0xA5u8; 512]);
    let mut text = String::new();
    if text.try_reserve(1024).is_err() {
        report.outcome = ProbeOutcome::Exhausted;
        report.pages = private_heap_stats().pages;
        return report;
    }
    // Indexed into a fixed alphabet rather than computed by arithmetic on a
    // `usize`: `(index % 26) as u8` is provably in range but says so only to a
    // reader, and `cast_possible_truncation` is right that the cast itself
    // carries no such proof.
    const ALPHABET: &[u8; 26] = b"abcdefghijklmnopqrstuvwxyz";
    for index in 0..1024usize {
        text.push(ALPHABET[index % ALPHABET.len()] as char);
    }
    if boxed.iter().any(|byte| *byte != 0xA5) || text.len() != 1024 {
        report.outcome = ProbeOutcome::Failed;
        return report;
    }

    let grown = private_heap_stats();
    report.pages = grown.pages;
    report.growths = grown.growths;
    // A batch boundary must actually have been crossed, or the reuse phase below
    // proves nothing: a check that fitted entirely in the first batch would show
    // "no further growth" trivially.
    if grown.growths < 2 {
        report.outcome = ProbeOutcome::Failed;
        return report;
    }

    // Phase 3: give it all back, then take a comparable amount again. The
    // allocator must serve this from its free list — the whole reason a
    // ceiling-bound component needs one — so the growth count must not move.
    //
    // The boundary is announced on the console before the phase starts, so a
    // gate can count the root's *own* growth records between here and the
    // report line and require zero. Without that line the only evidence of
    // reuse would be `reuse_growths` in this component's own report — a number
    // produced by the allocator under test, which an allocator that lost the
    // freed spans and grew again could under-count into agreement.
    debug_write(label);
    debug_write(b" private-heap reuse phase begins\n");
    drop(numbers);
    drop(text);
    drop(boxed);
    let freed = private_heap_stats();
    report.leaked = freed.live;
    if freed.live != start.live {
        report.outcome = ProbeOutcome::Failed;
        return report;
    }

    let mut again: Vec<u64> = Vec::new();
    if again.try_reserve(elements).is_err() {
        report.outcome = ProbeOutcome::Exhausted;
        return report;
    }
    for index in 0..elements {
        again.push(index as u64);
    }
    let reused = private_heap_stats();
    report.pages = reused.pages;
    report.reuse_growths = reused.growths - freed.growths;
    if report.reuse_growths != 0 {
        report.outcome = ProbeOutcome::Failed;
        return report;
    }
    drop(again);

    let end = private_heap_stats();
    report.leaked = end.live;
    if end.live != start.live {
        report.outcome = ProbeOutcome::Failed;
    }
    report
}

/// Run [`probe`] and report it on the debug console under `label`. Returns
/// `true` when the component's private heap is live, or when the generation
/// declares it no quota — in both cases the component may proceed.
pub fn probe_and_report(label: &[u8]) -> bool {
    let report = probe(label);
    debug_write(label);
    debug_write(match report.outcome {
        ProbeOutcome::Ok => b" private-heap quota live pages=" as &[u8],
        ProbeOutcome::Denied => b" private-heap denied pages=",
        ProbeOutcome::Exhausted => b" private-heap exhausted pages=",
        ProbeOutcome::Failed => b" private-heap failed pages=",
    });
    write_decimal(report.pages);
    debug_write(b" growths=");
    write_decimal(report.growths);
    debug_write(b" reuse_growths=");
    write_decimal(report.reuse_growths);
    debug_write(b" leaked=");
    write_decimal(report.leaked);
    debug_write(b"\n");
    matches!(report.outcome, ProbeOutcome::Ok | ProbeOutcome::Denied)
}

fn write_decimal(value: usize) {
    let mut digits = [0u8; 20];
    let mut index = digits.len();
    let mut remaining = value;
    loop {
        index -= 1;
        digits[index] = b'0' + (remaining % 10) as u8;
        remaining /= 10;
        if remaining == 0 {
            break;
        }
    }
    debug_write(&digits[index..]);
}
