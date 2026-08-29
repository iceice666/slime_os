//! Declares the `kani` cfg so `just lint_all` does not reject it.
//!
//! `src/virtio_mmio.rs` carries an IO7 proof module behind `#[cfg(kani)]`,
//! which no product build ever sets. The workspace lints deny warnings and
//! include `unexpected_cfgs`, so without this declaration every product build
//! of this crate fails on a cfg name it has no way to know is intentional.
//!
//! Declared here rather than in `[lints.rust]` because this crate takes
//! `[lints] workspace = true`, and Cargo rejects a package that both inherits
//! workspace lints and overrides them locally. This mirrors
//! `components/proto/build.rs`, which does the same for that crate's harnesses.
//!
//! `verification/virtio-proofs/build.rs` is a separate file with the same one
//! line: it declares the cfg for the proof crate, which compiles this module
//! under its own manifest and never runs this script.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(kani)");
    // Nothing here depends on the source, so never re-run on source changes.
    println!("cargo::rerun-if-changed=build.rs");
}
