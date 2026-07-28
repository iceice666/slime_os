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
    println!("cargo:rerun-if-env-changed=SLIME_GENERATION_CMD_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_POWERBOX_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_SAMPLE_PLANE_CHECK");
    println!("cargo:rerun-if-env-changed=SLIME_FABRIC_AUTHORITY_CHECK");
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
    if let Ok(value) = std::env::var("SLIME_FABRIC_AUTHORITY_CHECK") {
        println!("cargo:rustc-env=SLIME_FABRIC_AUTHORITY_CHECK={value}");
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
    let manifest_path =
        std::path::Path::new(manifest_dir).join("../../contracts/generation/v1/fixtures/valid.zti");
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    let manifest = std::fs::read_to_string(&manifest_path).expect("read generation manifest");
    let participants = fabric_participants(&manifest);
    // The parser is indentation-keyed, so its failure mode is a short table
    // rather than an empty one — and a short table is silent: the fabric denies
    // by default, so a dropped participant becomes a refused component with no
    // diagnostic. Counting `component = "..."` lines inside the same block is
    // an independent reading of the same bytes, so the two disagree exactly
    // when the structural parse lost an entry. Interposition hops also declare
    // a component, so they are excluded by depth the same way the parser
    // includes participants.
    let declared = declared_participant_count(&manifest);
    assert!(
        declared > 0,
        "generation manifest declares no fabric participants"
    );
    assert_eq!(
        participants.len(),
        declared,
        "fabric participant parse lost entries; the manifest declares {declared}"
    );
    let rows = participants
        .iter()
        .map(|(component, route, interface, direction)| {
            format!("    (b\"{component}\", \"{route}\", \"{interface}\", {direction}),\n")
        })
        .collect::<String>();
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(
        out.join("fabric_profile.rs"),
        format!(
            "/// Every (component, route name, interface name, direction) edge the\n\
             /// generation declares. Deny by default: a component absent from this\n\
             /// table holds no route authority, whatever it asks for.\n\
             pub const FABRIC_PARTICIPANTS: &[(&[u8], &str, &str, u32)] = &[\n{rows}];\n"
        ),
    )
    .expect("write fabric profile");
}

/// Scan the manifest's `fabricGraph` block for its declared participants.
///
/// Indentation-keyed rather than a real parser, matching the other manifest
/// readers in this file: the manifest is a fixed in-tree fixture. Because the
/// failure mode of an indentation key is a *short* table rather than an empty
/// one, the caller cross-checks the result against
/// [`declared_participant_count`], which reads the same block by a different
/// rule. Neither reading is trusted alone.
fn fabric_participants(manifest: &str) -> Vec<(String, String, String, u32)> {
    let mut participants = Vec::new();
    let mut route = String::new();
    let mut interface = String::new();
    let mut component = String::new();
    for line in fabric_graph_block(manifest).lines() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        match () {
            // Route header fields sit one level inside `routes = [ { ... } ]`.
            _ if indent == 8 && trimmed.starts_with("name = \"") => {
                route = quoted(trimmed).unwrap_or_default();
            }
            _ if indent == 8 && trimmed.starts_with("interface = \"") => {
                interface = quoted(trimmed).unwrap_or_default();
            }
            // Participant fields sit one level further in. `component` always
            // precedes `direction`, so the pair completes on `direction`.
            _ if indent == 12 && trimmed.starts_with("component = \"") => {
                component = quoted(trimmed).unwrap_or_default();
            }
            _ if indent == 12 && trimmed.starts_with("direction = \"") => {
                let direction = match quoted(trimmed).unwrap_or_default().as_str() {
                    "publish" => 1,
                    "subscribe" => 2,
                    "client" => 3,
                    "server" => 4,
                    other => panic!("unknown fabric direction {other}"),
                };
                assert!(
                    !component.is_empty() && !route.is_empty() && !interface.is_empty(),
                    "fabric participant is missing its component, route, or interface"
                );
                participants.push((
                    core::mem::take(&mut component),
                    route.clone(),
                    interface.clone(),
                    direction,
                ));
            }
            _ => {}
        }
    }
    participants
}

/// How many participants the `fabricGraph` block declares, counted without any
/// indentation assumption.
///
/// A participant is the only thing in the block that names a `direction`;
/// interposition hops name only a component. So this is a genuinely
/// independent reading of the same bytes, and it disagrees with
/// [`fabric_participants`] exactly when the structural parse dropped an entry.
fn declared_participant_count(manifest: &str) -> usize {
    fabric_graph_block(manifest)
        .lines()
        .filter(|line| line.trim_start().starts_with("direction = \""))
        .count()
}

/// The manifest's `fabricGraph` block. Panics rather than degrading: this table
/// is the fabric's entire authority set, so a manifest this cannot locate must
/// stop the build, not silently produce a graph with no declared edges.
fn fabric_graph_block(manifest: &str) -> &str {
    let start = manifest
        .find("\n  fabricGraph = {")
        .expect("generation manifest has no fabricGraph block");
    let length = manifest[start..]
        .find("\n  };")
        .expect("generation manifest fabricGraph block is unterminated");
    &manifest[start..start + length]
}

fn quoted(line: &str) -> Option<String> {
    line.split('"').nth(1).map(str::to_owned)
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
