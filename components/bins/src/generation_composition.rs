//! Which composition this component was booted into, asked of the root.
//!
//! Every fabric component branches on the generation's boot action to pick
//! which plane's schedule to run. Until B70's `BOOT_ACTION` query each one read
//! `GENERATION_BOOT_ACTION`, a `&str` `components/bins/build.rs` copied out of
//! one per-plane profile into `OUT_DIR` and each component `include!`d — so a
//! component's *behavior* was selected at compile time by one manifest, and no
//! component could be built outside this crate.
//!
//! The root already knew the answer and already delivered it, but only to the
//! bootstrap instance: `main.rs` passes `boot_action.id()` as the first C
//! parameter and zero to every other task. The eleven participants that branch
//! on the composition are not that instance, so they could not ask. The query
//! widens who may ask, not what is disclosed.
//!
//! # One decode, here
//!
//! `slime-rt` answers a raw id because it does not depend on `boot-contracts`.
//! Folding it back to [`BootAction`] happens exactly once, in this module, and
//! every caller names a variant. That is the whole point: a per-component
//! `match id { 28 => ... }` would put the manifest's numbering back into
//! component source, which is the coupling this replaces rather than moves.
//!
//! The fold itself is `BootAction::from_id`, in `boot-contracts` beside the
//! enum, not a table here. A component-side list is a second vocabulary that
//! goes stale in one direction only: a composition added to the contract but
//! missing from the copy folds to `None` and reads at every call site as "some
//! older generation" rather than as the new plane. Keeping the inverse next to
//! the declaration makes adding a variant fail that crate's exhaustive `match`
//! to compile, which is a guarantee this crate cannot state about the contract.
//!
//! # Resolved once, cached
//!
//! [`is`] and its callers — `fabric_boot::active`, `fabric_matrix::active`,
//! and the branches in `console`, `dango`, and `fabric-intruder` — sit on
//! dispatch loops that ran against a `&str` comparison, so resolving per call
//! would put a syscall where a constant was. The memo follows
//! `fabric-service`'s `TIME_SLOT_CACHE` and `fabric_call_scenario`'s
//! `WAKE_SLOT`: components are single-threaded, and a generation's boot action
//! cannot change under a running graph.

use boot_contracts::generation::BootAction;

/// The resolved answer, or `None` until the root has been asked.
///
/// Holds the decoded [`BootAction`] rather than its wire id, so a cached read
/// costs a load and nothing else. Storing the id instead would make every call
/// on a dispatch loop re-run `from_id`'s scan over the contract's 29 variants —
/// a cheaper cost than the syscall it replaced, but not the "resolved once"
/// this memo exists to provide, and not what `fabric-service`'s
/// `TIME_SLOT_CACHE` does with its cached slot.
///
/// Two-level: the outer `Option` is "has this been asked", the inner one is the
/// answer, so a refusal is remembered instead of re-asked. `is()` sits on loops
/// that previously read a compile-time constant, and re-asking would put a
/// syscall in every iteration and make the root log a refusal line into the
/// serial transcript the plane gates parse. `fabric-service` records the same
/// rule for its occupancy query: stop after the first refusal.
static mut BOOT_ACTION_MEMO: Option<Option<BootAction>> = None;

/// The composition the authenticated generation declares.
///
/// `None` where this root does not serve the query at all. That is a real
/// answer rather than a failure to hide: it is what a component built against
/// this ABI observes under an older root, and every caller has a defined
/// behavior for "not this plane". Defaulting to some plane instead would
/// silently run one composition's schedule under another.
///
/// An *authority* denial is not that case and does not return. See below.
pub fn boot_action() -> Option<BootAction> {
    // SAFETY: components are single-threaded; every reader is on the one
    // dispatch loop that owns this task.
    if let Some(resolved) = unsafe { *core::ptr::addr_of!(BOOT_ACTION_MEMO) } {
        return resolved;
    }
    let answered = match slime_rt::boot_action() {
        Ok(id) => Some(id),
        // The generation declared this instance no way to learn its own
        // composition, and a component that cannot ask must not quietly answer
        // "some other plane" — that selects a schedule by an unrelated grant
        // shape. Fatal, on the rule `console` already applies to
        // `resolve_binding`: `is_err()` alone would accept the authority gate's
        // refusal as an ordinary negative, so the status is discriminated
        // rather than collapsed.
        Err(slime_rt::ERR_BAD_CAP) => {
            slime_rt::debug_write(
                b"[component] the generation grants no boot-action query to this instance\n",
            );
            slime_rt::exit(1)
        }
        Err(_) => None,
    };
    // An id this component's `boot-contracts` does not know folds to `None`
    // rather than to a plausible action: a generation declaring a composition
    // newer than the component is exactly the case a guess would hide.
    let resolved = answered.and_then(BootAction::from_id);
    // SAFETY: as above.
    unsafe { *core::ptr::addr_of_mut!(BOOT_ACTION_MEMO) = Some(resolved) };
    resolved
}

/// Whether the generation declares exactly `expected`.
///
/// The predicate every caller actually wants, so no call site repeats the
/// `Option` handling. A root that does not serve the query is not the expected
/// action, which is the same answer the string comparison gave when the profile
/// named another plane.
pub fn is(expected: BootAction) -> bool {
    boot_action() == Some(expected)
}
