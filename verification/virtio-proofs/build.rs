//! Declares the `kani` cfg for the proof build of `virtio_mmio.rs`.
//!
//! `slime-components` carries no `build.rs` of its own — it is a plain library
//! in the components workspace — so unlike `slime-proto` there is no existing
//! place for this declaration. It lives here because only this crate ever
//! compiles the module with `--cfg kani`.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(kani)");
    println!("cargo::rerun-if-changed=build.rs");
}
