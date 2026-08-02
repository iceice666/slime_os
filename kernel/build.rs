fn main() {
    // The kernel links with a bare-metal script. Emit an absolute path from
    // the crate manifest so the link works regardless of the cwd Cargo picks
    // for rustc (the workspace root, not this crate directory).
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let target = std::env::var("TARGET").expect("TARGET");
    println!("cargo:rerun-if-env-changed=SLIME_TARGET_PROFILE");
    match std::env::var("SLIME_TARGET_PROFILE") {
        Ok(profile) => println!("cargo:rustc-env=SLIME_TARGET_PROFILE={profile}"),
        Err(_) if target == "aarch64-unknown-none" => {
            panic!("SLIME_TARGET_PROFILE is required for AArch64 kernel builds")
        }
        Err(_) => {}
    }
    let linker_script = match target.as_str() {
        "x86_64-unknown-none" => "linker.ld",
        "aarch64-unknown-none" => "linker-aarch64.ld",
        _ => panic!("unsupported kernel target {}", target),
    };
    println!("cargo:rustc-link-arg=-T{manifest_dir}/{linker_script}");
    println!("cargo:rerun-if-changed={manifest_dir}/{linker_script}");
}
