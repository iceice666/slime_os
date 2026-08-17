#![no_std]
#![no_main]

//! The seL4 directory plane's subject: M6.3's capability mechanism (P5.4.3).
//!
//! M6.3 is deliberately split, and this plane is the half the *root* owns. What
//! a directory contains — entries, names, object identities — is a filesystem
//! component's business, built over the object store, and none of it is here.
//! What is here is the part that must be unforgeable:
//!
//! * a **shared namespace root**, so two holders see each other's commits;
//! * **scoped views** that derivation may only narrow, never widen or escape;
//! * an **atomic compare-and-swap** commit, so a writer building on a stale
//!   root is refused rather than silently discarding another's work;
//! * a commit gate requiring an **unscoped** writer, so a holder of `docs`
//!   cannot promote its subtree to the whole namespace.
//!
//! The oracle proves the same properties in `kernel/src/capability/mod.rs` and
//! three syscalls. Nothing about them needed a kernel — they needed a place
//! neither holder controls, which on seL4 is the root task.

use slime_rt::{DIRECTORY_ROOT_BYTES, MAX_DIRECTORY_PATH};
// B59: rights bit numbering is generated from
// `contracts/generation/v5/schema.zt`. The powerbox/fs protocols carry a
// 32-bit rights field, so the generated `u64` constants are narrowed at the
// declaration rather than re-spelled as separate `u32` literals.
const RIGHT_DIRECTORY_READ: u32 = boot_contracts::generation::RIGHT_DIRECTORY_READ as u32;
const RIGHT_DIRECTORY_WRITE: u32 = boot_contracts::generation::RIGHT_DIRECTORY_WRITE as u32;
const RIGHT_DIRECTORY_LIST: u32 = boot_contracts::generation::RIGHT_DIRECTORY_LIST as u32;
const RIGHT_DIRECTORY_DERIVE: u32 = boot_contracts::generation::RIGHT_DIRECTORY_DERIVE as u32;
const RIGHT_TRANSFER: u32 = boot_contracts::generation::RIGHT_TRANSFER as u32;

/// The unscoped directory capability the generation grants this component.
///
/// Slot 1: the component declares no executables, so its first runtime grant
/// lands above them exactly as the storage planes' block capability does.
const ROOT_DIRECTORY_SLOT: u32 = 1;
/// The run token: init's declared edge to the instance that runs the scenario.
///
/// This is also the discriminator. The plane declares this executable twice —
/// the instance init spawns, and a root-owned `idle` instance holding the same
/// directory authority over a loopback endpoint nobody ever sends on. Both
/// hold a real endpoint here, so the token's *arrival* rather than its presence
/// separates them: the root delivers a nonzero boot action only to the
/// bootstrap instance, so `startup_arg` cannot.
const RUN_TOKEN_SLOT: u32 = 0;
/// Yields given up before concluding no run token will arrive. The idle
/// instance always exhausts this bound, so it is a latency rather than a
/// safety margin.
const RUN_TOKEN_YIELDS: usize = 64;

const RIGHTS_ALL: u32 =
    RIGHT_DIRECTORY_READ | RIGHT_DIRECTORY_WRITE | RIGHT_DIRECTORY_LIST | RIGHT_DIRECTORY_DERIVE;

/// Two roots this component commits. Opaque to the mechanism — the root never
/// interprets them — so any distinct non-equal pair exercises the compare.
const FIRST_ROOT: [u8; 32] = [0xA1; 32];
const SECOND_ROOT: [u8; 32] = [0xB2; 32];

slime_rt::entry!(main);

fn main(_startup_arg: u32) {
    if !spawned_instance() {
        slime_rt::debug_write(b"[sel4-directory-probe] idle without a run token\n");
        slime_rt::exit(0);
    }

    // The granted view: namespace root, unscoped. The boot seeds the namespace
    // with the directory fixture's root, so what this asserts is the *shape* —
    // unscoped, and a root the mechanism reports consistently — rather than a
    // particular identity, which belongs to the filesystem plane.
    let (initial_root, scope) = inspect(ROOT_DIRECTORY_SLOT, RIGHTS_ALL);
    if !scope.is_empty() {
        fail(b"initial view is scoped");
    }
    slime_rt::debug_write(b"[sel4-directory-probe] unscoped view of the namespace\n");

    // Asking for rights the capability does not carry is refused. The grant
    // carries all four, so this asks for a bit outside the directory set.
    if slime_rt::directory_inspect(
        ROOT_DIRECTORY_SLOT,
        RIGHT_TRANSFER,
        &mut [0; DIRECTORY_ROOT_BYTES],
        &mut [0; MAX_DIRECTORY_PATH],
    )
    .is_ok()
    {
        fail(b"inspect with foreign rights accepted");
    }
    slime_rt::debug_write(b"[sel4-directory-probe] inspect outside the rights set refused\n");

    // Commit a root through the unscoped writer, against the root just read.
    if slime_rt::directory_commit(ROOT_DIRECTORY_SLOT, &initial_root, &FIRST_ROOT) < 0 {
        fail(b"first commit");
    }
    let (root, _) = inspect(ROOT_DIRECTORY_SLOT, RIGHT_DIRECTORY_READ);
    if root != FIRST_ROOT {
        fail(b"commit not visible");
    }
    slime_rt::debug_write(b"[sel4-directory-probe] root committed and visible\n");

    // The compare. A writer holding the *previous* root and only now trying to
    // install is building on a parent that no longer exists, so its commit is
    // refused — without disturbing what is live.
    if slime_rt::directory_commit(ROOT_DIRECTORY_SLOT, &initial_root, &SECOND_ROOT) >= 0 {
        fail(b"stale commit accepted");
    }
    let (root, _) = inspect(ROOT_DIRECTORY_SLOT, RIGHT_DIRECTORY_READ);
    if root != FIRST_ROOT {
        fail(b"stale commit disturbed the root");
    }
    slime_rt::debug_write(b"[sel4-directory-probe] stale commit refused\n");

    // Derive a narrower view. The scope lengthens; the namespace is the same
    // one, which the shared root below proves.
    let docs = derive(ROOT_DIRECTORY_SLOT, b"docs", RIGHTS_ALL);
    let (root, scope) = inspect(docs, RIGHT_DIRECTORY_READ);
    if root != FIRST_ROOT {
        fail(b"a derived view sees a different namespace");
    }
    if &*scope != b"docs" {
        fail(b"derived scope");
    }
    slime_rt::debug_write(b"[sel4-directory-probe] derived a scoped view\n");

    // Derivation composes, and only forward: `docs` derives `docs/notes`.
    let notes = derive(
        docs,
        b"notes",
        RIGHT_DIRECTORY_READ | RIGHT_DIRECTORY_DERIVE,
    );
    let (_, scope) = inspect(notes, RIGHT_DIRECTORY_READ);
    if &*scope != b"docs/notes" {
        fail(b"nested scope");
    }
    slime_rt::debug_write(b"[sel4-directory-probe] scopes compose forward\n");

    // No escape. `..` is not a path the validator admits, so there is no
    // request that walks a scope outward — the narrowing is syntactic, not a
    // check the caller could phrase around.
    for escape in [
        b"..".as_slice(),
        b"../docs",
        b"/docs",
        b"docs/",
        b"docs//notes",
    ] {
        if slime_rt::directory_derive(docs, escape, RIGHT_DIRECTORY_READ).is_ok() {
            fail(b"an escaping path was accepted");
        }
    }
    slime_rt::debug_write(b"[sel4-directory-probe] escaping paths refused\n");

    // No widening. `notes` was derived without `directoryWrite`, so it cannot
    // derive a child that has it.
    if slime_rt::directory_derive(notes, b"drafts", RIGHTS_ALL).is_ok() {
        fail(b"widening derive accepted");
    }
    slime_rt::debug_write(b"[sel4-directory-probe] widening derivation refused\n");

    // Deriving requires `directoryDerive` specifically: holding a view is not
    // authority to hand out narrower ones.
    let opaque = derive(ROOT_DIRECTORY_SLOT, b"opaque", RIGHT_DIRECTORY_READ);
    if slime_rt::directory_derive(opaque, b"child", RIGHT_DIRECTORY_READ).is_ok() {
        fail(b"derive without the derive right accepted");
    }
    slime_rt::debug_write(b"[sel4-directory-probe] derivation without the right refused\n");

    // A scoped writer cannot commit. `docs` carries `directoryWrite` and is
    // still refused, because committing through it would replace the
    // namespace-wide root with a subtree — deleting everything beside it.
    if slime_rt::directory_commit(docs, &FIRST_ROOT, &SECOND_ROOT) >= 0 {
        fail(b"scoped commit accepted");
    }
    let (root, _) = inspect(ROOT_DIRECTORY_SLOT, RIGHT_DIRECTORY_READ);
    if root != FIRST_ROOT {
        fail(b"a refused scoped commit disturbed the root");
    }
    slime_rt::debug_write(b"[sel4-directory-probe] scoped commit refused\n");

    // A reader cannot commit either.
    if slime_rt::directory_commit(opaque, &FIRST_ROOT, &SECOND_ROOT) >= 0 {
        fail(b"read-only commit accepted");
    }
    slime_rt::debug_write(b"[sel4-directory-probe] read-only commit refused\n");

    // The namespace is shared, not copied: a commit through the unscoped view
    // is visible through the scoped one, which is the property that makes a
    // directory capability a view rather than a snapshot.
    if slime_rt::directory_commit(ROOT_DIRECTORY_SLOT, &FIRST_ROOT, &SECOND_ROOT) < 0 {
        fail(b"second commit");
    }
    let (root, scope) = inspect(docs, RIGHT_DIRECTORY_READ);
    if root != SECOND_ROOT {
        fail(b"the scoped view did not see the commit");
    }
    if &*scope != b"docs" {
        fail(b"the commit changed a scope");
    }
    slime_rt::debug_write(b"[sel4-directory-probe] the namespace is shared across views\n");

    slime_rt::debug_write(b"[sel4-directory-probe] directory plane complete\n");
}

/// Inspect a view, failing the plane if the mechanism refuses.
///
/// Returns the scope by length rather than as the padded buffer: the operation
/// answers with the length, and comparing the whole 128-byte array against a
/// short literal would never match.
fn inspect(slot: u32, rights: u32) -> ([u8; DIRECTORY_ROOT_BYTES], Scope) {
    let mut root = [0u8; DIRECTORY_ROOT_BYTES];
    let mut bytes = [0u8; MAX_DIRECTORY_PATH];
    let Ok(len) = slime_rt::directory_inspect(slot, rights, &mut root, &mut bytes) else {
        fail(b"inspect");
    };
    if len > MAX_DIRECTORY_PATH {
        fail(b"inspect length");
    }
    (root, Scope { bytes, len })
}

/// A scope and its length, so a comparison is against the path rather than the
/// padding behind it.
struct Scope {
    bytes: [u8; MAX_DIRECTORY_PATH],
    len: usize,
}

impl core::ops::Deref for Scope {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

/// Derive a view, failing the plane if the mechanism refuses.
fn derive(slot: u32, relative: &[u8], rights: u32) -> u32 {
    let Ok(derived) = slime_rt::directory_derive(slot, relative, rights) else {
        fail(b"derive");
    };
    derived
}

fn spawned_instance() -> bool {
    let mut bytes = [0u8; slime_rt::MAX_MSG];
    let mut caps = [0u64; slime_rt::MAX_CAPS_PER_MSG];
    for _ in 0..RUN_TOKEN_YIELDS {
        match slime_rt::recv(RUN_TOKEN_SLOT, &mut bytes, &mut caps) {
            slime_rt::ERR_WOULDBLOCK => slime_rt::yield_now(),
            result if result < 0 => return false,
            _ => return true,
        }
    }
    false
}

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[sel4-directory-probe] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}
