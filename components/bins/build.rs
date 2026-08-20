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
    println!("cargo:rerun-if-env-changed=SLIME_FABRIC_PROXY_EARLY_EXIT");
    println!("cargo:rerun-if-env-changed=SLIME_FABRIC_STREAM_EARLY_EXIT");
    println!("cargo:rerun-if-env-changed=SLIME_GENERATION_CANDIDATE");
    println!("cargo:rerun-if-env-changed=SLIME_GENERATION_CMD_SCENARIO");
    if let Ok(value) = std::env::var("SLIME_FABRIC_PROXY_EARLY_EXIT") {
        println!("cargo:rustc-env=SLIME_FABRIC_PROXY_EARLY_EXIT={value}");
    }
    if let Ok(value) = std::env::var("SLIME_FABRIC_STREAM_EARLY_EXIT") {
        println!("cargo:rustc-env=SLIME_FABRIC_STREAM_EARLY_EXIT={value}");
    }
    if let Ok(value) = std::env::var("SLIME_GENERATION_CANDIDATE") {
        println!("cargo:rustc-env=SLIME_GENERATION_CANDIDATE={value}");
    }
    if let Ok(value) = std::env::var("SLIME_GENERATION_CMD_SCENARIO") {
        println!("cargo:rustc-env=SLIME_GENERATION_CMD_SCENARIO={value}");
    }
    generate_command_profile(manifest_dir);
    generate_fabric_profile(manifest_dir);
}

fn generate_command_profile(manifest_dir: &str) {
    println!("cargo:rerun-if-env-changed=SLIME_COMMAND_PROFILE_MANIFEST");
    let manifest_name =
        std::env::var("SLIME_COMMAND_PROFILE_MANIFEST").unwrap_or_else(|_| "valid.zti".to_string());
    let manifest_path = std::path::Path::new(manifest_dir)
        .join("../../contracts/generation/v1/fixtures")
        .join(&manifest_name);
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    let manifest = std::fs::read_to_string(&manifest_path).expect("read generation manifest");
    let Some(profile_owner) = command_profile_executable(&manifest) else {
        let client_budget = executable_block(&manifest, "spawn-service")
            .and_then(|service| field_int(service, "spawnBudget"))
            .unwrap_or(0);
        let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
        std::fs::write(
            out.join("command_profile.rs"),
            format!("pub const CLIENT_BUDGET: usize = {client_budget};\npub const RPC_SLOT: u32 = u32::MAX;\npub const SHARED_BUFFER_FACTORY_SLOT: u32 = u32::MAX;\npub const COMMAND_PROFILE: &[(&[u8], &[u8], u32)] = &[];\n"),
        )
        .expect("write service-only command profile");
        return;
    };
    let profile = field_list(profile_owner, "commandProfile").expect("command profile");
    let client_budget = field_int(profile_owner, "spawnBudget").expect("command spawn budget");
    let profile_executable = field(profile_owner, "name").expect("command profile executable name");
    let profile_instance =
        instance_for_executable(&manifest, profile_executable).expect("command profile instance");
    let profile_instance_name =
        field(profile_instance, "name").expect("command profile instance name");
    let targets = profile
        .iter()
        .map(|command| {
            if *command == "echo" {
                "echo-agent"
            } else {
                command
            }
        })
        .collect::<Vec<_>>();
    let launcher = executable_launcher(&manifest, &targets).expect("command launcher instance");
    // `spawn-service.rs` is the only consumer of this file and resolves both
    // slots in its own CSpace, so they must come from whichever instance runs
    // it. Where the spawn service owns the command profile that is the profile
    // instance; where a client owns one, the spawn service is the launcher
    // serving it. Deriving from the wrong side yields another instance's slot
    // numbering, which agrees only while the two share one layout.
    // Identify the consumer by the executable it runs, not by its role in the
    // profile: `launcher` is whoever *sources* the executable grants, which may
    // be the instance that spawned the service rather than the service itself.
    let consumer = manifest
        .split("\n    {\n")
        .skip(1)
        .find(|block| field(block, "executable") == Some("spawn-service"))
        .and_then(|block| field(block, "name"))
        .expect("spawn-service command-profile consumer");
    let entries = profile
        .iter()
        .zip(targets.iter())
        .map(|(command, target)| {
            let grant =
                executable_grant(&manifest, launcher, target).expect("profile executable grant");
            // The consumer's binding, not the launcher's: `spawn-service.rs`
            // resolves these in its own CSpace, and the instance that sources
            // the grant may be whoever spawned it.
            let slot = binding_slot(&manifest, consumer, grant)
                .expect("consumer profile executable binding");
            let block = executable_block(&manifest, target).expect("profile executable");
            let object = field(block, "object").expect("executable object");
            (*command, object, slot)
        })
        .collect::<Vec<_>>();
    let peer = if consumer == profile_instance_name {
        launcher
    } else {
        profile_instance_name
    };
    // A runtime-minted channel declares the consumer's slot as a
    // `mintedBindings` entry rather than a grant binding: the edge and slot are
    // fixed, only the object waits for its minter. Either spelling answers the
    // same question, so both are consulted.
    //
    // This stays a build-time derivation while the RPC endpoint cannot be named
    // at runtime: `sel4-dango.zti` grants `spawn-service` three `send`+`recv`
    // endpoints -- the RPC channel plus one context endpoint per command -- so
    // CP2's `kind:endpoint+send,recv` role is ambiguous and is refused. Which of
    // the three is "the RPC one" is a fact about the graph's shape, not about the
    // capability, so it needs the peer relation this reads.
    let rpc_slot = related_binding_slot(&manifest, consumer, peer, &["send", "recv"])
        .or_else(|| minted_binding_slot(&manifest, consumer, &["send", "recv"]))
        .expect("command RPC binding");
    let generated = entries
        .iter()
        .map(|(name, object, slot)| format!("    (b\"{name}\", b\"{object}\", {slot}),\n"))
        .collect::<String>();
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(
        out.join("command_profile.rs"),
        format!(
            "pub const CLIENT_BUDGET: usize = {client_budget};\npub const RPC_SLOT: u32 = {rpc_slot};\npub const COMMAND_PROFILE: &[(&[u8], &[u8], u32)] = &[\n{generated}];\n"
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

fn executable_grant<'a>(manifest: &'a str, holder: &str, wanted: &str) -> Option<&'a str> {
    manifest.split("\n    {\n").skip(1).find_map(|block| {
        let name = field(block, "name")?;
        let source = field(block, "source")?;
        let target = field(block, "target")?;
        let rights = field_list(block, "rights")?;
        (source == holder && target == wanted && rights.contains(&"exec")).then_some(name)
    })
}

#[cfg(test)]
fn executable_slot(manifest: &str, holder: &str, wanted: &str) -> Option<usize> {
    binding_slot(
        manifest,
        holder,
        executable_grant(manifest, holder, wanted)?,
    )
}

fn related_binding_slot(
    manifest: &str,
    holder: &str,
    peer: &str,
    rights: &[&str],
) -> Option<usize> {
    manifest.split("\n    {\n").skip(1).find_map(|block| {
        let name = field(block, "name")?;
        let source = field(block, "source")?;
        let target = field(block, "target")?;
        let declared = field_list(block, "rights")?;
        ((source == holder && target == peer || source == peer && target == holder)
            && rights.iter().all(|right| declared.contains(right)))
        .then(|| binding_slot(manifest, holder, name))
        .flatten()
    })
}

fn minted_binding_slot(manifest: &str, holder: &str, rights: &[&str]) -> Option<usize> {
    let section = manifest.split("mintedBindings = [").nth(1)?;
    let section = section.split("\n  ];").next()?;
    section.split("\n    {\n").skip(1).find_map(|block| {
        let declared = field_list(block, "rights")?;
        (field(block, "holder")? == holder && rights.iter().all(|r| declared.contains(r)))
            .then(|| field_int(block, "slot"))
            .flatten()
    })
}

fn binding_slot(manifest: &str, holder: &str, grant: &str) -> Option<usize> {
    let instance = instance_block(manifest, holder)?;
    instance.split("\n        {\n").skip(1).find_map(|block| {
        (field(block, "grant")? == grant)
            .then(|| field_int(block, "slot"))
            .flatten()
    })
}

fn instance_for_executable<'a>(manifest: &'a str, executable: &str) -> Option<&'a str> {
    manifest.split("\n    {\n").skip(1).find(|block| {
        field(block, "executable") == Some(executable) && field(block, "name").is_some()
    })
}

fn executable_launcher<'a>(manifest: &'a str, targets: &[&str]) -> Option<&'a str> {
    manifest.split("\n    {\n").skip(1).find_map(|block| {
        let name = field(block, "name")?;
        field(block, "executable")?;
        targets
            .iter()
            .all(|target| executable_grant(manifest, name, target).is_some())
            .then_some(name)
    })
}

fn executable_block<'a>(manifest: &'a str, wanted: &str) -> Option<&'a str> {
    manifest
        .split("    {")
        .skip(1)
        .find(|block| field(block, "name") == Some(wanted) && field(block, "object").is_some())
}

fn command_profile_executable(manifest: &str) -> Option<&str> {
    manifest.split("\n    {\n").skip(1).find(|block| {
        field(block, "object").is_some()
            && field_list(block, "commandProfile").is_some_and(|profile| !profile.is_empty())
    })
}

fn instance_block<'a>(manifest: &'a str, wanted: &str) -> Option<&'a str> {
    manifest.split("\n    {\n").skip(1).find(|block| {
        field(block, "name") == Some(wanted)
            && block
                .lines()
                .any(|line| line.trim_start().starts_with("executable = \""))
    })
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
    let start = block.find(&prefix)? + prefix.len();
    let value = block.get(start..)?.split_once("];")?.0;
    Some(value.split('"').skip(1).step_by(2).collect())
}

#[cfg(test)]
mod tests {
    use super::executable_slot;

    #[test]
    fn command_slots_follow_explicit_instance_bindings() {
        let manifest = r#"
    { name = "sysinfo"; object = "sha256:sysinfo"; }
    { name = "echo-agent"; object = "sha256:echo"; }
    {
      name = "spawn-service";
      executable = "spawn-service";
      bindings = [
        { grant = "z-sysinfo"; slot = 7; };
        { grant = "a-echo"; slot = 3; };
      ];
    };
    { name = "z-sysinfo"; source = "spawn-service"; target = "sysinfo"; rights = ["exec"; "spawn";]; };
    { name = "a-echo"; source = "spawn-service"; target = "echo-agent"; rights = ["exec"; "spawn";]; };
"#;
        assert_eq!(
            executable_slot(manifest, "spawn-service", "echo-agent"),
            Some(3)
        );
        assert_eq!(
            executable_slot(manifest, "spawn-service", "sysinfo"),
            Some(7)
        );
    }

    #[test]
    fn grant_declaration_order_does_not_change_command_slots() {
        let manifest = r#"
    { name = "custom-command"; object = "sha256:custom"; }
    {
      name = "spawn-service";
      executable = "spawn-service";
      bindings = [
        { grant = "b-command"; slot = 11; };
      ];
    };
    { name = "b-command"; source = "spawn-service"; target = "custom-command"; rights = ["exec"; "spawn";]; };
"#;
        assert_eq!(
            executable_slot(manifest, "spawn-service", "custom-command"),
            Some(11)
        );
        assert_eq!(executable_slot(manifest, "spawn-service", "missing"), None);
    }
}
