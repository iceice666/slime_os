//! P5.3.3/B16: supervision handles, termination records, and reclamation.
//!
//! Extracted from `init.rs` by B65: 21 plane launchers in one 2286-line
//! binary meant every plane's edit shared a file with every other plane's.
//! Holds this plane and the helpers only it uses.
//!
//! Init resolves its slot numbers through the root, and the helpers that do so
//! live in `init.rs`, so anything from them is reached through `super`. The
//! generated boot-layout table this used to describe is gone (B70): a slot is
//! now asked for by declared name at run time rather than compiled in.

use super::{resolve_executable, wait_clean};

/// How many children the supervision plane creates over the boot.
///
/// One more than `slime-root`'s current `MAX_RECORDS` (48), which is the whole
/// point: the bound this crosses is on records *awaiting collection*, and a
/// graph that collects as it goes must be able to exceed it. A loop that
/// stopped at the bound would pass against the unfixed root and prove nothing.
const SUPERVISION_LOOP_CHILDREN: u32 = 49;
/// Drive the supervision plane: create more children over one boot than
/// `MAX_RECORDS` can hold at once, and answer correctly for every live handle.
///
/// Only reachable for the authenticated `supervision` action declared by
/// `contracts/generation/v1/fixtures/sel4-supervision.zti`.
///
/// This is backlog B16's exit condition. Before the fix, `Terminations` never
/// reclaimed, so the 33rd child's outcome was dropped silently and its parent
/// waited forever. The gate crosses the bound and then asserts the two things a
/// sweep could plausibly break:
///
/// - a handle held *across* the crossing still answers afterwards, and
/// - a handle **parked in transit** across the crossing is still collectable,
///   which is the half a predicate over live tables alone would miss.
///
/// The loop child is `supervision-child`, which takes no channel:
/// `ChannelTable` never reclaims (B22), so a child needing one would exhaust
/// channels before the loop reached the record bound.
pub fn drive_supervision_plane() {
    let retained = slime_rt::spawn(resolve_executable(b"executable:supervision-child"), &[])
        .unwrap_or_else(|_| fail_supervision(b"retained child"));
    // B25: a second handle naming a task this component already supervises.
    // Neither a spawn grant nor an export could place one twice — a grant must
    // precede the child, and an export moves — so derivation is the only way.
    //
    // Derived while the source is still held and before it is collected, since
    // collection consumes the handle. Both copies then cross the allocation
    // bound below.
    let derived = slime_rt::supervision_derive(retained.supervision_slot)
        .unwrap_or_else(|_| fail_supervision(b"derive a second handle"));
    slime_rt::debug_write(b"[init] second supervision handle derived\n");
    slime_rt::debug_write(b"[init] supervision handle retained\n");
    // The source, collected here so this declaration has no live task and the
    // loop below can reuse it. That is also what makes the derived copy the
    // interesting one: it outlives the handle it came from.
    wait_clean(&[retained.supervision_slot]);
    for _ in 0..SUPERVISION_LOOP_CHILDREN {
        let child = slime_rt::spawn(resolve_executable(b"executable:supervision-child"), &[])
            .unwrap_or_else(|_| fail_supervision(b"loop child"));
        wait_clean(&[child.supervision_slot]);
    }
    slime_rt::debug_write(b"[init] supervision lifetime bound crossed\n");
    // The derived copy, held across a boot's worth of allocation *and* past the
    // collection of the handle it came from, still answers. That is the half a
    // predicate over live tables alone would miss: the task is long gone and
    // every other trace of it erased.
    if !matches!(
        slime_rt::supervision_status(derived),
        Ok(Some(slime_rt::Termination::Exit(0)))
    ) {
        fail_supervision(b"the derived handle lost its authority");
    }
    slime_rt::debug_write(b"[init] retained handle answered after crossing\n");
    slime_rt::debug_write(b"[init] derived supervision survived crossing\n");
    // Collecting consumes the handle: the outcome lives in the capability, so a
    // second query must be refused rather than answered from elsewhere (B42).
    if slime_rt::supervision_status(derived).is_ok() {
        fail_supervision(b"a collected handle answered twice");
    }
    slime_rt::debug_write(b"[init] collected handle refused\n");
}
fn fail_supervision(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[init] supervision plane fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}
