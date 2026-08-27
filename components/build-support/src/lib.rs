//! Build-time support every Slime component crate shares.
//!
//! # Why this crate exists
//!
//! Before CP3 this code was a private module inside `components/bins`'s build
//! script. A component could therefore only be built as a `[[bin]]` of that one
//! crate, because nothing else could reach the target selection, the linker
//! script choice, or the compile-time knob propagation. That is the coupling
//! B70's problem statement names and [CP3] removes: this is a documented
//! library with a stable entry point, so a component crate outside this
//! workspace can depend on it by pinned commit and reproduce the same build
//! environment.
//!
//! # What a component's `build.rs` owes
//!
//! Every component crate's build script is exactly:
//!
//! ```ignore
//! fn main() {
//!     slime_build_support::configure();
//! }
//! ```
//!
//! Nothing here reads a generation manifest. Historical generators derived
//! command tables from one fixture and wrote them into component `OUT_DIR`s;
//! B70 replaced that coupling with root-served runtime queries, so components
//! read the authenticated generation they are actually running inside.
//!
//! # What is *not* here
//!
//! No manifest-derived data at all. `emit_fabric_profile` copied a per-plane
//! Rust constant table `scripts/build/build-generation.py` rendered from the
//! resolved fabric graph, which every fabric component `include!`d; that was
//! B70's last such surface and it is gone. A component now reads its
//! composition, its declared ceilings, and its own graph rows from the
//! authenticated generation at runtime, so the same image boots under any
//! plane that declares it.
//!
//! [CP3]: ../../../roadmap/10-component-platform.md

use std::path::{Path, PathBuf};

/// Cargo names a JSON target specification by its file stem, so this is what
/// `TARGET` reads as for `aarch64-sel4-minimal.json`.
pub const SEL4_TARGET: &str = "aarch64-sel4-minimal";

/// Compile-time knobs a gate sets to build a deliberately misbehaving component.
///
/// Each is read by component source through `option_env!`, so a component only
/// observes the knob when the builder exported it for that build. They are
/// declared centrally rather than per crate because the set is a property of
/// the gate suite, not of any one component, and a crate that forgot to
/// re-export one would compile into the *passing* branch and make its gate
/// vacuous.
const COMPILE_TIME_KNOBS: &[&str] = &[
    "SLIME_FABRIC_PROXY_EARLY_EXIT",
    "SLIME_FABRIC_STREAM_EARLY_EXIT",
    "SLIME_GENERATION_CANDIDATE",
    "SLIME_GENERATION_CMD_SCENARIO",
    "SLIME_BOOT_SELECTION_FAIL",
    "SLIME_RECOVERY_IMAGE",
];

/// Select the component target's linker script and propagate the build knobs.
///
/// This is every component crate's whole build script. It panics rather than
/// degrading on an unsupported target or a missing `SLIME_TARGET_PROFILE`,
/// because both produce an image that boots and then misbehaves:
/// `boot_contracts::target_profile` reads the profile through `option_env!` and
/// a component built without it would qualify against the wrong target.
pub fn configure() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let target = std::env::var("TARGET").expect("TARGET");
    // The seL4 target deliberately has no Slime linker script. A component
    // there is an ordinary seL4 task loaded by `slime-root`'s ELF loader at
    // whatever address it links to, not an image the retired kernel re-based
    // onto a fixed component load base. `sel4-runtime-common` also expects the
    // default layout — its `declare_stack!` and the `_end`-relative IPC buffer
    // and transfer window all come from rust-lld's own script.
    let linker_script = match target.as_str() {
        "x86_64-unknown-none" => Some("component.ld"),
        "aarch64-unknown-none" => Some("component-aarch64.ld"),
        SEL4_TARGET => None,
        other => panic!("unsupported component target {other}"),
    };
    if let Some(linker_script) = linker_script {
        let script = linker_script_dir(&manifest_dir).join(linker_script);
        // Refused here rather than at link time. A wrong or stale
        // `SLIME_COMPONENT_LINKER_DIR` otherwise surfaces either as a linker
        // error far from its cause or, worse, as a component linked at the
        // default base that the generation builder later refuses with "invalid
        // component load layout" — the silent wrong answer this override exists
        // to prevent.
        assert!(
            script.is_file(),
            "component linker script {} not found; set SLIME_COMPONENT_LINKER_DIR \
             to the directory holding it",
            script.display()
        );
        println!("cargo:rustc-link-arg=-T{}", script.display());
        println!("cargo:rerun-if-changed={}", script.display());
    }
    println!("cargo:rerun-if-env-changed=SLIME_COMPONENT_LINKER_DIR");
    println!("cargo:rerun-if-env-changed=SLIME_TARGET_PROFILE");
    match std::env::var("SLIME_TARGET_PROFILE") {
        Ok(profile) => println!("cargo:rustc-env=SLIME_TARGET_PROFILE={profile}"),
        Err(_) if target == "aarch64-unknown-none" || target == SEL4_TARGET => {
            panic!("SLIME_TARGET_PROFILE is required for AArch64 component builds")
        }
        Err(_) => {}
    }
    for knob in COMPILE_TIME_KNOBS {
        println!("cargo:rerun-if-env-changed={knob}");
        if let Ok(value) = std::env::var(knob) {
            println!("cargo:rustc-env={knob}={value}");
        }
    }
}

/// Where the component linker scripts live.
///
/// `SLIME_COMPONENT_LINKER_DIR` wins when set, and the component SDK's build
/// entry point sets it (CP8). The scripts are repository-level build inputs
/// shared by every component -- an `aarch64-unknown-none` component links at the
/// fixed component base its target profile declares -- so an out-of-tree crate
/// cannot find them relative to its own manifest, which is where they were
/// looked for before. Without the override, a crate outside this workspace
/// building for that target failed at link time with a missing linker script,
/// or worse would have linked at the wrong base and been refused later by the
/// generation builder.
///
/// The in-tree fallback walks upward to the `components` directory that owns
/// the linker scripts, independent of the crate's lifecycle category depth.
fn linker_script_dir(manifest_dir: &str) -> PathBuf {
    if let Ok(directory) = std::env::var("SLIME_COMPONENT_LINKER_DIR") {
        return PathBuf::from(directory);
    }
    Path::new(manifest_dir)
        .ancestors()
        .find(|directory| {
            directory.join("component.ld").is_file()
                && directory.join("component-aarch64.ld").is_file()
        })
        .expect("component crate is below the components linker-script root")
        .to_path_buf()
}
