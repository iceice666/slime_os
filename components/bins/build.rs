fn main() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let target = std::env::var("TARGET").expect("TARGET");
    if target == "x86_64-unknown-none" {
        println!("cargo:rustc-link-arg=-T{manifest_dir}/../component.ld");
        println!("cargo:rerun-if-changed={manifest_dir}/../component.ld");
    }
    println!("cargo:rerun-if-env-changed=SLIME_GENERATION_NUMBER");
    println!("cargo:rerun-if-env-changed=SLIME_RECOVERY_INTERRUPT");
    println!("cargo:rerun-if-env-changed=SLIME_RECOVERY_IMAGE");
    println!("cargo:rerun-if-env-changed=SLIME_DANGO_CHECK");
    if let Ok(number) = std::env::var("SLIME_GENERATION_NUMBER") {
        println!("cargo:rustc-env=SLIME_GENERATION_NUMBER={number}");
    }
    if let Ok(value) = std::env::var("SLIME_RECOVERY_IMAGE") {
        println!("cargo:rustc-env=SLIME_RECOVERY_IMAGE={value}");
    }
    if let Ok(value) = std::env::var("SLIME_RECOVERY_INTERRUPT") {
        println!("cargo:rustc-env=SLIME_RECOVERY_INTERRUPT={value}");
    }
    if let Ok(value) = std::env::var("SLIME_DANGO_CHECK") {
        println!("cargo:rustc-env=SLIME_DANGO_CHECK={value}");
    }
    generate_command_profile(manifest_dir);
}

fn generate_command_profile(manifest_dir: &str) {
    let manifest_path =
        std::path::Path::new(manifest_dir).join("../../contracts/generation/v1/fixtures/valid.zti");
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    let manifest = std::fs::read_to_string(&manifest_path).expect("read generation manifest");
    let dango = component_block(&manifest, "dango").expect("dango component");
    let profile = field_list(dango, "commandProfile").expect("dango command profile");
    let client_budget = field_int(dango, "spawnBudget").expect("dango spawn budget");
    let entries = profile
        .iter()
        .map(|command| {
            let target = if *command == "echo" {
                "echo-agent"
            } else {
                command
            };
            let slot = component_slot(&manifest, target).expect("profile executable component");
            let block = component_block(&manifest, target).expect("profile executable component");
            let object = field(block, "object").expect("component object");
            (*command, object, slot)
        })
        .collect::<Vec<_>>();
    let generated = entries
        .iter()
        .map(|(name, object, slot)| format!("    (b\"{name}\", b\"{object}\", {slot}),\n"))
        .collect::<String>();
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(
        out.join("command_profile.rs"),
        format!(
            "pub const CLIENT_BUDGET: usize = {client_budget};\npub const COMMAND_PROFILE: &[(&[u8], &[u8], u32)] = &[\n{generated}];\n"
        ),
    )
    .expect("write command profile");
    let generated_names = entries
        .iter()
        .map(|(name, _, _)| format!("    b\"{name}\",\n"))
        .collect::<String>();
    std::fs::write(
        out.join("dango_profile.rs"),
        format!(
            "pub const CLIENT_BUDGET: u8 = {client_budget};\npub const COMMAND_NAMES: &[&[u8]] = &[\n{generated_names}];\n"
        ),
    )
    .expect("write dango profile");
}

fn component_slot(manifest: &str, wanted: &str) -> Option<usize> {
    let present = manifest
        .split("    {")
        .skip(1)
        .filter(|block| field(block, "name").is_some() && field(block, "object").is_some())
        .any(|block| field(block, "name") == Some(wanted));
    if !present {
        return None;
    }
    match wanted {
        "sysinfo" => Some(1),
        "echo-agent" => Some(2),
        _ => None,
    }
}

fn component_block<'a>(manifest: &'a str, wanted: &str) -> Option<&'a str> {
    manifest
        .split("    {")
        .skip(1)
        .find(|block| field(block, "name") == Some(wanted) && field(block, "object").is_some())
}

fn field<'a>(block: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key} = \"");
    let value = block
        .lines()
        .find(|line| line.trim_start().starts_with(&prefix))?;
    value.split('"').nth(1)
}

fn field_int(block: &str, key: &str) -> Option<usize> {
    let prefix = format!("{key} = ");
    let value = block
        .lines()
        .find(|line| line.trim_start().starts_with(&prefix))?;
    value
        .trim_start()
        .strip_prefix(&prefix)?
        .trim_end_matches(';')
        .parse()
        .ok()
}

fn field_list<'a>(block: &'a str, key: &str) -> Option<Vec<&'a str>> {
    let prefix = format!("{key} = [");
    let value = block
        .lines()
        .find(|line| line.trim_start().starts_with(&prefix))?;
    Some(value.split('"').skip(1).step_by(2).collect())
}
