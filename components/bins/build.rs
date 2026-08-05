/// Cargo names a JSON target specification by its file stem, so this is what
/// `TARGET` reads as for `aarch64-sel4-minimal.json`.
const SEL4_TARGET: &str = "aarch64-sel4-minimal";

fn main() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
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
        _ => panic!("unsupported component target {}", target),
    };
    if let Some(linker_script) = linker_script {
        println!("cargo:rustc-link-arg=-T{manifest_dir}/../{linker_script}");
        println!("cargo:rerun-if-changed={manifest_dir}/../{linker_script}");
    }
    println!("cargo:rerun-if-env-changed=SLIME_TARGET_PROFILE");
    match std::env::var("SLIME_TARGET_PROFILE") {
        Ok(profile) => println!("cargo:rustc-env=SLIME_TARGET_PROFILE={profile}"),
        Err(_) if target == "aarch64-unknown-none" || target == SEL4_TARGET => {
            panic!("SLIME_TARGET_PROFILE is required for AArch64 component builds")
        }
        Err(_) => {}
    }
    println!("cargo:rerun-if-env-changed=SLIME_GENERATION_NUMBER");
    println!("cargo:rerun-if-env-changed=SLIME_RECOVERY_INTERRUPT");
    println!("cargo:rerun-if-env-changed=SLIME_RECOVERY_IMAGE");
    println!("cargo:rerun-if-env-changed=SLIME_DANGO_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_GENERATION_CMD_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_POWERBOX_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_SAMPLE_PLANE_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_SEL4_CHANNEL_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_SEL4_LOAN_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_SEL4_SPAWN_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_SEL4_SAMPLE_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_SEL4_STREAM_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_FABRIC_AUTHORITY_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_FABRIC_STREAM_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_FABRIC_QOS_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_FABRIC_CALL_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_FABRIC_OPERATION_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_FABRIC_VISIBILITY_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_FABRIC_BOOT_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_DATA_FABRIC_PROFILE");
    println!("cargo:rerun-if-env-changed=SLIME_BOOT_LAYOUT");
    println!("cargo:rerun-if-env-changed=SLIME_FABRIC_PROXY_EARLY_EXIT");
    println!("cargo:rerun-if-env-changed=SLIME_GENERATION_CANDIDATE");
    println!("cargo:rerun-if-env-changed=SLIME_GENERATION_CMD_SCENARIO");
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
    if let Ok(value) = std::env::var("SLIME_GENERATION_CMD_CHECK") {
        println!("cargo:rustc-env=SLIME_GENERATION_CMD_CHECK={value}");
    }
    if let Ok(value) = std::env::var("SLIME_POWERBOX_CHECK") {
        println!("cargo:rustc-env=SLIME_POWERBOX_CHECK={value}");
    }
    if let Ok(value) = std::env::var("SLIME_SAMPLE_PLANE_CHECK") {
        println!("cargo:rustc-env=SLIME_SAMPLE_PLANE_CHECK={value}");
    }
    if let Ok(value) = std::env::var("SLIME_SEL4_CHANNEL_CHECK") {
        println!("cargo:rustc-env=SLIME_SEL4_CHANNEL_CHECK={value}");
    }
    if let Ok(value) = std::env::var("SLIME_SEL4_LOAN_CHECK") {
        println!("cargo:rustc-env=SLIME_SEL4_LOAN_CHECK={value}");
    }
    if let Ok(value) = std::env::var("SLIME_SEL4_SPAWN_CHECK") {
        println!("cargo:rustc-env=SLIME_SEL4_SPAWN_CHECK={value}");
    }
    if let Ok(value) = std::env::var("SLIME_SEL4_SAMPLE_CHECK") {
        println!("cargo:rustc-env=SLIME_SEL4_SAMPLE_CHECK={value}");
    }
    if let Ok(value) = std::env::var("SLIME_SEL4_STREAM_CHECK") {
        println!("cargo:rustc-env=SLIME_SEL4_STREAM_CHECK={value}");
    }
    if let Ok(value) = std::env::var("SLIME_FABRIC_AUTHORITY_CHECK") {
        println!("cargo:rustc-env=SLIME_FABRIC_AUTHORITY_CHECK={value}");
    }
    if let Ok(value) = std::env::var("SLIME_FABRIC_STREAM_CHECK") {
        println!("cargo:rustc-env=SLIME_FABRIC_STREAM_CHECK={value}");
    }
    if let Ok(value) = std::env::var("SLIME_FABRIC_QOS_CHECK") {
        println!("cargo:rustc-env=SLIME_FABRIC_QOS_CHECK={value}");
    }
    if let Ok(value) = std::env::var("SLIME_FABRIC_CALL_CHECK") {
        println!("cargo:rustc-env=SLIME_FABRIC_CALL_CHECK={value}");
    }
    if let Ok(value) = std::env::var("SLIME_FABRIC_VISIBILITY_CHECK") {
        println!("cargo:rustc-env=SLIME_FABRIC_VISIBILITY_CHECK={value}");
    }
    if let Ok(value) = std::env::var("SLIME_FABRIC_BOOT_CHECK") {
        println!("cargo:rustc-env=SLIME_FABRIC_BOOT_CHECK={value}");
    }
    if let Ok(value) = std::env::var("SLIME_FABRIC_PROXY_EARLY_EXIT") {
        println!("cargo:rustc-env=SLIME_FABRIC_PROXY_EARLY_EXIT={value}");
    }
    if let Ok(value) = std::env::var("SLIME_GENERATION_CANDIDATE") {
        println!("cargo:rustc-env=SLIME_GENERATION_CANDIDATE={value}");
    }
    if let Ok(value) = std::env::var("SLIME_GENERATION_CMD_SCENARIO") {
        println!("cargo:rustc-env=SLIME_GENERATION_CMD_SCENARIO={value}");
    }
    generate_command_profile(manifest_dir);
    generate_fabric_profile(manifest_dir);
    generate_boot_layout(manifest_dir);
}

/// Copy the generation's slot table into `OUT_DIR` for `init.rs` to include.
///
/// `build-generation.py` emits it per generation number and passes the path,
/// because components are compiled before the generation is assembled and so
/// cannot read the layout resource out of it. The checked-in fallback keeps a
/// plain `cargo build` working; it holds the default profile's slots, which is
/// what generation 1 declares.
fn generate_boot_layout(manifest_dir: &str) {
    let fallback = std::path::Path::new(manifest_dir).join("src/default_boot_layout.rs");
    let layout_path = std::env::var_os("SLIME_BOOT_LAYOUT")
        .map(std::path::PathBuf::from)
        .unwrap_or(fallback);
    println!("cargo:rerun-if-changed={}", layout_path.display());
    let layout = std::fs::read(&layout_path).expect("read generated boot layout");
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(out.join("boot_layout.rs"), layout).expect("write boot layout");
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

/// Emit the C8.3 fabric participant table from the same generation manifest the
/// host builder encodes into the authenticated fabric-graph resource.
///
/// The fabric service must know which (component, route, direction) edges the
/// generation declared without asking the kernel: the kernel is unaware of
/// routes by design, so there is no syscall to read the graph. Deriving the
/// table here from the manifest keeps one source of truth — a route renamed or
/// a participant removed changes both the resource and this table in the same
/// build. The full interface identity is not restated: the fabric folds the
/// route identity at runtime from the generated C8.1 `INTERFACE_IDENTITY`, so
/// the identity can never drift from the admitted schema.
fn generate_fabric_profile(manifest_dir: &str) {
    let fallback = std::path::Path::new(manifest_dir).join("src/default_fabric_profile.rs");
    let profile_path = std::env::var_os("SLIME_DATA_FABRIC_PROFILE")
        .map(std::path::PathBuf::from)
        .unwrap_or(fallback);
    println!("cargo:rerun-if-changed={}", profile_path.display());
    let profile = std::fs::read(&profile_path).expect("read generated fabric profile");
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(out.join("fabric_profile.rs"), profile).expect("write fabric profile");
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
