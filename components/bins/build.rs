fn main() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    println!("cargo:rustc-link-arg=-T{manifest_dir}/../component.ld");
    println!("cargo:rerun-if-changed={manifest_dir}/../component.ld");
}
