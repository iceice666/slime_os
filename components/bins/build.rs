fn main() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    println!("cargo:rustc-link-arg=-T{manifest_dir}/../component.ld");
    println!("cargo:rerun-if-changed={manifest_dir}/../component.ld");
    println!("cargo:rerun-if-env-changed=SLIME_GENERATION_NUMBER");
    if let Ok(number) = std::env::var("SLIME_GENERATION_NUMBER") {
        println!("cargo:rustc-env=SLIME_GENERATION_NUMBER={number}");
    }
}

