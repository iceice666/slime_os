//! P5.3.3: userspace-initiated spawn and supervised collection.
//!
//! Extracted from `init.rs` by B65: 21 plane launchers in one 2286-line
//! binary meant every plane's edit shared a file with every other plane's.
//! Holds this plane and the helpers only it uses.
//!
//! Init's slot numbers arrive by `include!` of the generated per-generation
//! boot layout into `init.rs`'s scope, so anything from it is reached through
//! `super` — there is no path naming that layout independently of its binary.

use super::{
    CONSOLE_SLOT, RIGHT_DIRECTORY_READ, RIGHT_DIRECTORY_WRITE, RIGHT_EXEC, RIGHT_SPAWN,
    RIGHT_TRANSFER, SYSINFO_SLOT, grant, wait_clean,
};

fn fail_spawn(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[init] spawn plane fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}
/// Drive the P5.3.3 spawn plane: construct children from grant-resolved
/// executables, hand each one the capabilities its declaration names, and
/// observe termination through a supervision handle.
///
/// Only reachable for the authenticated `spawn` action declared by
/// `contracts/generation/v1/fixtures/sel4-spawn.zti`; see the `.md` beside it.
///
/// The two children are `console` and `sysinfo`, both **unmodified** — the same
/// binaries the x86 oracle runs. That is the milestone's claim: a component
/// written against the retired kernel's spawn ABI is started by `slime-root`
/// with no seL4 branch in it. `sysinfo` is the useful one to wait on, because
/// it runs to completion and exits 0 of its own accord; `console` loops until
/// its peer dies, which is what makes it the right subject for the
/// still-live arm.
///
/// What crosses at spawn is *transferable directory authority*, not endpoint
/// halves: an endpoint is a generation-declared seL4 Endpoint the root installs
/// into both ends itself, so a parent has none to hand over. Six views is B15's
/// own exit-condition number — the grant array crosses the transfer window as a
/// staged payload, and six records are 96 bytes, past the 64-byte message bound
/// a narrower reader would apply.
pub fn drive_spawn_plane() {
    if slime_rt::spawn(63, &[]).is_ok() {
        fail_spawn(b"an empty slot named an executable");
    }
    // A slot holding real authority of another kind. Init genuinely holds its
    // console control endpoint at slot 3, so this is a check on kind rather
    // than on possession.
    if slime_rt::spawn(3, &[]).is_ok() {
        fail_spawn(b"a non-executable capability named an executable");
    }
    slime_rt::debug_write(b"[init] ungranted executable refused\n");
    // The narrowing rule: a grant's rights must be a subset of what the parent
    // holds. Init holds this view with `directoryRead | transfer` alone, so
    // asking to pass on write authority is asking the root to manufacture
    // authority no generation declared.
    if slime_rt::spawn(
        CONSOLE_SLOT,
        &[grant(5, RIGHT_DIRECTORY_READ | RIGHT_DIRECTORY_WRITE)],
    )
    .is_ok()
    {
        fail_spawn(b"a widened grant was accepted");
    }
    slime_rt::debug_write(b"[init] widened grant refused\n");
    // The executable slot is authority to create this child; passing it on
    // would let the child re-spawn its own image outside its parent's budget.
    if slime_rt::spawn(
        CONSOLE_SLOT,
        &[grant(CONSOLE_SLOT, RIGHT_EXEC | RIGHT_SPAWN)],
    )
    .is_ok()
    {
        fail_spawn(b"a child was granted its own executable");
    }
    slime_rt::debug_write(b"[init] self-executable grant refused\n");
    let console = slime_rt::spawn(
        CONSOLE_SLOT,
        &[grant(5, RIGHT_DIRECTORY_READ | RIGHT_TRANSFER)],
    )
    .unwrap_or_else(|_| fail_spawn(b"console"));
    slime_rt::debug_write(b"[init] console spawned\n");
    // A live child has no outcome, and the query says so rather than blocking
    // or inventing one.
    match slime_rt::supervision_status(console.supervision_slot) {
        Ok(None) => {
            slime_rt::debug_write(b"[init] live child reports no outcome\n");
        }
        _ => fail_spawn(b"a live child reported an outcome"),
    }
    // A spawn grant is a copy: the parent can still resolve the slot it
    // granted from.
    let mut root = [0u8; 32];
    let mut scope = [0u8; slime_rt::MAX_DIRECTORY_PATH];
    if slime_rt::directory_inspect(5, RIGHT_DIRECTORY_READ as u32, &mut root, &mut scope).is_err() {
        fail_spawn(b"the granted view stopped resolving");
    }
    slime_rt::debug_write(b"[init] granted view retained\n");
    let wide = [
        grant(6, RIGHT_DIRECTORY_READ | RIGHT_TRANSFER),
        grant(7, RIGHT_DIRECTORY_READ | RIGHT_TRANSFER),
        grant(8, RIGHT_DIRECTORY_READ | RIGHT_TRANSFER),
        grant(9, RIGHT_DIRECTORY_READ | RIGHT_TRANSFER),
        grant(10, RIGHT_DIRECTORY_READ | RIGHT_TRANSFER),
        grant(11, RIGHT_DIRECTORY_READ | RIGHT_TRANSFER),
    ];
    let sysinfo = slime_rt::spawn(SYSINFO_SLOT, &wide).unwrap_or_else(|_| fail_spawn(b"sysinfo"));
    slime_rt::debug_write(b"[init] sysinfo spawned\n");
    for slot in 6..=11 {
        if slime_rt::directory_inspect(slot, RIGHT_DIRECTORY_READ as u32, &mut root, &mut scope)
            .is_err()
        {
            fail_spawn(b"a copied view stopped resolving");
        }
    }
    slime_rt::debug_write(b"[init] six grants copied\n");
    // The launch context, sent down init's own end of the declared endpoint.
    // `sysinfo` is blocked in `recv` on the end the root installed for it.
    if slime_rt::send(4, &launch_context(), &[]) != slime_rt::ERR_SUCCESS {
        fail_spawn(b"deliver the launch context");
    }
    slime_rt::debug_write(b"[init] launch context sent\n");
    wait_clean(&[sysinfo.supervision_slot]);
    slime_rt::debug_write(b"[init] sysinfo outcome collected\n");
    // Collecting consumes the handle, so the outcome is single-use rather than
    // a fact the parent can re-read forever.
    if slime_rt::supervision_status(sysinfo.supervision_slot).is_ok() {
        fail_spawn(b"a collected handle answered twice");
    }
    slime_rt::debug_write(b"[init] collected handle consumed\n");
    // End to end through the unmodified child: `console.rs` `debug_write`s
    // whatever arrives on its slot 0, so this is the child *reading* the
    // endpoint the root installed for it rather than the root reporting it
    // installed one.
    if slime_rt::send(3, b"[console] spawned child reached\n", &[]) != slime_rt::ERR_SUCCESS {
        fail_spawn(b"reach the spawned console");
    }
    // `cap_drop` on a *live* child's handle, exactly as `spawn_or_fail` does on
    // every product boot. Dropped before the close below, so the child is
    // certainly still running: collecting an outcome consumes the handle, which
    // would make this test a no-op on an already-collected one.
    if slime_rt::cap_drop(console.supervision_slot) < 0 {
        fail_spawn(b"drop a live child's handle");
    }
    slime_rt::debug_write(b"[init] dropped handle released\n");
    // The close lets the child exit of its own accord, so the graph reaches the
    // quiescent accounting the gate asserts. Nobody waits on it: the handle is
    // gone, and the root records the termination either way.
    if slime_rt::send(3, b"SLIME.CONSOLE.CLOSE", &[]) != slime_rt::ERR_SUCCESS {
        fail_spawn(b"close the spawned console");
    }
}
/// The launch context `sysinfo` decodes through `launch_context::receive`.
fn launch_context() -> [u8; slime_proto::spawn::REQUEST_LEN] {
    let mut command = [0u8; 16];
    command[..7].copy_from_slice(b"sysinfo");
    slime_proto::spawn::WireSpawnRequest {
        magic: slime_proto::spawn::SPAWN_MAGIC,
        version: slime_proto::spawn::FORMAT_VERSION,
        flags: 0,
        command_len: 7,
        argument_count: 0,
        environment_count: 0,
        capability_roles: 0,
        client_budget: 0,
        command,
        arguments: [0u8; 8],
        environment: [0u8; 8],
        grant_rights: 0,
        reserved: [0u8; 6],
    }
    .encode()
}
