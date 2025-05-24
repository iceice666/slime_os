fn main() {
    let mut cmd = std::process::Command::new("qemu-system-x86_64");

    let uefi_path = env!("UEFI_PATH");
    cmd.arg("-bios").arg(ovmf_prebuilt::ovmf_pure_efi());
    cmd.arg("-drive")
        .arg(format!("format=raw,file={uefi_path}"));

    println!("BIOS: {}", ovmf_prebuilt::ovmf_pure_efi().display());
    println!("DRIVE: {uefi_path}");

    cmd.arg("-no-reboot");

    // For Debugging
    #[cfg(debug_assertions)]
    {
        cmd.args(["-s", "-S"]);
        println!("GDB server is set.")
    }

    let mut child = cmd.spawn().unwrap();
    child.wait().unwrap();
}
