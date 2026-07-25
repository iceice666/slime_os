fn main() {
    // The kernel links with a bare-metal script. Emit an absolute path from
    // the crate manifest so the link works regardless of the cwd Cargo picks
    // for rustc (the workspace root, not this crate directory).
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    println!("cargo:rustc-link-arg=-T{manifest_dir}/linker.ld");
    println!("cargo:rerun-if-changed={manifest_dir}/linker.ld");
}
