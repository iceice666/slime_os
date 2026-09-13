//! Declares the `kani` cfg so `just lint_all` does not reject it.
//!
//! `src/io_queue_proofs.rs` is gated behind `#[cfg(kani)]`, which no product
//! build ever sets. The workspace lints deny warnings and include
//! `unexpected_cfgs`, so without this declaration every product build of this
//! crate fails on a cfg name it has no way to know is intentional.
//!
//! Declared here rather than in `[lints.rust]` because this crate takes
//! `[lints] workspace = true`, and Cargo rejects a package that both inherits
//! workspace lints and overrides them locally. This is the same mechanism
//! `deps/rust-sel4/crates/sel4/bitfield-ops/build.rs` uses for its own Kani
//! harnesses, minus the `rustversion` guard: the MSRV this crate declares is
//! far past the release that introduced `cargo::rustc-check-cfg`.
//!
//! Deliberately dependency-free, so the crate keeps the zero-dependency shape
//! that lets `slime-rt` depend on it.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(kani)");
    // Nothing here depends on the source, so never re-run on source changes.
    println!("cargo::rerun-if-changed=build.rs");
}
