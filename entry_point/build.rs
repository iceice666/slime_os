use std::path::PathBuf;

fn main() {
    // set by cargo, build scripts should use this directory for output files
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    // set by cargo's artifact dependency feature, see
    // https://doc.rust-lang.org/nightly/cargo/reference/unstable.html#artifact-dependencies
    let kernel = PathBuf::from(std::env::var_os("CARGO_BIN_FILE_SLIME_OS_KERNEL").unwrap());

    // create an UEFI disk image (optional)
    let uefi_path = out_dir.join("uefi.img");
    bootloader::UefiBoot::new(&kernel)
        .create_disk_image(&uefi_path)
        .unwrap();

    // create a BIOS disk image
    let bios_path = out_dir.join("bios.img");
    bootloader::BiosBoot::new(&kernel)
        .create_disk_image(&bios_path)
        .unwrap();

    let kernel_path = kernel.display().to_string();
    let kernel_path = kernel_path.strip_prefix("/workspaces/slime_os/").unwrap();
    let content = format!(
        r#"#!/bin/bash
lldb \
-o "target create {kernel_path}" \
-o "target modules load --file $PWD/{kernel_path} --slide 0x8000000000" \
-o "gdb-remote localhost:1234" \
-o "b kernel_main" \
-o "c""#
    );
    std::fs::write("../debug.sh", content).expect("unable to create debug file");

    // pass the disk image paths as env variables to the `main.rs`
    println!("cargo:rustc-env=UEFI_PATH={}", uefi_path.display());
    println!("cargo:rustc-env=BIOS_PATH={}", bios_path.display());
}
