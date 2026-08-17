//! `slime-root`'s mechanism surface, as a library.
//!
//! Every module here is bounded, and most are pure state machines over fixed
//! arrays: task-owned typed authority, shared-buffer accounting, supervision
//! records, and timer scheduling. `main.rs` is the
//! seL4 root task that drives them — startup staging, the dispatch loop, and
//! the seL4 object plumbing — and it is deliberately not part of this crate's
//! testable surface.
//!
//! # Why a library at all (B23)
//!
//! These modules carried 102 `#[test]` functions that nothing compiled and
//! nothing ran. `main.rs` is unconditionally `#![no_std]` and `#![no_main]`,
//! the package declared no lib target, and the crate built only for a custom
//! seL4 JSON target with no `libtest` — so `cargo test` could not reach them by
//! any route. Every `slime-root` change inherited that blind spot: B16's fix
//! added four tests documenting its sweep's contract that could not be claimed
//! as verification, which is why that gate carries two fault injections rather
//! than one.
//!
//! Splitting the modules out is what gives them a gate: `just test_sel4_root`.
//!
//! # This is the same code the image links
//!
//! The library is `#![no_std]` in both configurations and the binary links it
//! rather than recompiling the modules, so a host test result is evidence about
//! the shipped root rather than about a parallel implementation. Nothing is
//! `cfg`-ed out for the host: the `sel4` crate builds for a host target given
//! `SEL4_PREFIX`, so the modules that touch seL4 types — `ipc`, `task`,
//! `fault`, `child_vspace`, `object_allocator`, `platform_timer`, and
//! `buffer_adapter` — compile unchanged.
//!
//! What the host cannot do is *invoke* seL4. No test here performs a syscall;
//! they exercise the state machines, which is the whole of what they ever
//! claimed to cover. The behavior that needs a running kernel stays the seL4
//! gates' job, and `just test_sel4_root` does not weaken that division — it closes
//! the gap where a pure-logic regression was caught by neither.

#![no_std]
// The modules below are `slime-root`'s mechanism surface: bounded, pure, and
// unit-tested in place. Startup exercises the allocation, task, IPC, and fault
// paths; the scheduling, timer, and shared-buffer state machines are owned here
// but driven by callers a parent integration adds, so not every item is
// reachable from `main`.
#![allow(dead_code)]

extern crate alloc;

pub mod boot_selector;
pub mod buffer_adapter;
pub mod child_vspace;
pub mod console;
pub mod cspace;
pub mod device;
pub mod directory;
pub mod event;
pub mod fault;
pub mod generation;
pub mod graph;
pub mod ipc;
pub mod launched;
pub mod notification;
pub mod object_allocator;
pub mod peer_endpoint;
pub mod platform_timer;
pub mod shared_buffer;
pub mod supervision;
pub mod task;
pub mod timer;
pub mod transfer_window;
pub mod virtio_blk;
