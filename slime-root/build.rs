//! Selects which generation `slime-root` embeds.
//!
//! The root task admits and launches its generation from bytes compiled into
//! it, so the choice has to be made at build time. `SLIME_GENERATION` names one
//! — `scripts/build/build-sel4.py` points it at the `aarch64-sel4-qemu-virt`
//! generation it just built — and without it the checked-in fixture is used.
//!
//! This is a `cfg` rather than a `match` in the source because the two
//! `include_bytes!` produce arrays of different lengths, which are different
//! types and will not unify in a match arm.

fn main() {
    println!("cargo:rerun-if-env-changed=SLIME_GENERATION");
    println!("cargo::rustc-check-cfg=cfg(slime_generation_supplied)");
    if let Ok(path) = std::env::var("SLIME_GENERATION") {
        println!("cargo:rerun-if-changed={path}");
        println!("cargo:rustc-cfg=slime_generation_supplied");
    }
}
