fn main() {
    // read env variables that were set in build script
    let uefi_path = env!("UEFI_PATH");
    let bios_path = env!("BIOS_PATH");


    let mut cmd = std::process::Command::new("qemu-system-x86_64");
    #[cfg(feature = "uefi")]
    {
        cmd.arg("-bios").arg(ovmf_prebuilt::ovmf_pure_efi());
        cmd.arg("-drive")
            .arg(format!("format=raw,file={uefi_path}"));
    }
    #[cfg(feature = "bios")]
    {
        cmd.arg("-drive")
            .arg(format!("format=raw,file={bios_path}"));
    }

    // Prevent QEMU from rebooting on guest error
    cmd.arg("-no-reboot");

    // For Debugging
    cmd.args(["-s", "-S"]);

    println!(
        "Full command: {} {}",
        cmd.get_program().to_string_lossy(),
        cmd.get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<String>>()
            .join(" ")
    );

    let mut child = cmd.spawn().unwrap();
    child.wait().unwrap();
}
