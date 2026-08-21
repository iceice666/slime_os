//! Build-time support every Slime component crate shares.
//!
//! # Why this crate exists
//!
//! Before CP3 this code was a private module inside `components/bins`'s build
//! script. A component could therefore only be built as a `[[bin]]` of that one
//! crate, because nothing else could reach the target selection, the linker
//! script choice, the compile-time knob propagation, or the generation-manifest
//! parser that produces `spawn-service`'s and `dango`'s command tables. That is
//! the coupling B70's problem statement names and [CP3] removes: the parser is
//! now a documented library with a stable entry point, so a component crate
//! outside this workspace can depend on it by pinned commit and reproduce the
//! same build environment.
//!
//! # What a component's `build.rs` owes
//!
//! Every component crate's build script is exactly:
//!
//! ```ignore
//! fn main() {
//!     slime_build_support::configure();
//! }
//! ```
//!
//! A component that needs the generated command tables adds the matching
//! generator call after it — see [`emit_command_profile`] and
//! [`emit_dango_profile`]. Nothing else is permitted to read the generation
//! manifest: those two functions are the whole of the manifest-derived surface
//! that survives, and CP2 retired the rest in favour of root-served runtime
//! queries.
//!
//! # What is *not* here
//!
//! The fabric profile is not generated from the manifest by any build script.
//! `scripts/build/build-generation.py` renders it per plane and points
//! `SLIME_DATA_FABRIC_PROFILE` at the result; [`emit_fabric_profile`] only
//! copies those bytes into `OUT_DIR`. The distinction matters because it is the
//! one remaining `include!` surface B70 tracks, and calling it a build-script
//! derivation would misattribute where that data comes from.
//!
//! [CP3]: ../../../roadmap/10-component-platform.md

use std::path::{Path, PathBuf};

/// Cargo names a JSON target specification by its file stem, so this is what
/// `TARGET` reads as for `aarch64-sel4-minimal.json`.
pub const SEL4_TARGET: &str = "aarch64-sel4-minimal";

/// Compile-time knobs a gate sets to build a deliberately misbehaving component.
///
/// Each is read by component source through `option_env!`, so a component only
/// observes the knob when the builder exported it for that build. They are
/// declared centrally rather than per crate because the set is a property of
/// the gate suite, not of any one component, and a crate that forgot to
/// re-export one would compile into the *passing* branch and make its gate
/// vacuous.
const COMPILE_TIME_KNOBS: &[&str] = &[
    "SLIME_FABRIC_PROXY_EARLY_EXIT",
    "SLIME_FABRIC_STREAM_EARLY_EXIT",
    "SLIME_GENERATION_CANDIDATE",
    "SLIME_GENERATION_CMD_SCENARIO",
    "SLIME_BOOT_SELECTION_FAIL",
    "SLIME_RECOVERY_IMAGE",
];

/// Select the component target's linker script and propagate the build knobs.
///
/// This is every component crate's whole build script. It panics rather than
/// degrading on an unsupported target or a missing `SLIME_TARGET_PROFILE`,
/// because both produce an image that boots and then misbehaves:
/// `boot_contracts::target_profile` reads the profile through `option_env!` and
/// a component built without it would qualify against the wrong target.
pub fn configure() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
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
        other => panic!("unsupported component target {other}"),
    };
    if let Some(linker_script) = linker_script {
        let script = components_dir(&manifest_dir).join(linker_script);
        println!("cargo:rustc-link-arg=-T{}", script.display());
        println!("cargo:rerun-if-changed={}", script.display());
    }
    println!("cargo:rerun-if-env-changed=SLIME_TARGET_PROFILE");
    match std::env::var("SLIME_TARGET_PROFILE") {
        Ok(profile) => println!("cargo:rustc-env=SLIME_TARGET_PROFILE={profile}"),
        Err(_) if target == "aarch64-unknown-none" || target == SEL4_TARGET => {
            panic!("SLIME_TARGET_PROFILE is required for AArch64 component builds")
        }
        Err(_) => {}
    }
    for knob in COMPILE_TIME_KNOBS {
        println!("cargo:rerun-if-env-changed={knob}");
        if let Ok(value) = std::env::var(knob) {
            println!("cargo:rustc-env={knob}={value}");
        }
    }
}

/// `components/`, from a component crate's own manifest directory.
///
/// The linker scripts and the generation fixtures are repository-level inputs
/// shared by every component, so they are located relative to this tree rather
/// than copied per crate. An out-of-tree crate overrides the fixture location
/// through `SLIME_COMMAND_PROFILE_MANIFEST_PATH`; it needs no linker script,
/// since only the retired bare-metal targets use one.
fn components_dir(manifest_dir: &str) -> PathBuf {
    // `components/bins/<crate>` -> `components`
    Path::new(manifest_dir)
        .parent()
        .and_then(Path::parent)
        .expect("component crate lives under components/bins/<crate>")
        .to_path_buf()
}

/// Read the generation manifest this build derives its command tables from.
///
/// `SLIME_COMMAND_PROFILE_MANIFEST` names a fixture inside this repository, as
/// the builder sets it per plane. `SLIME_COMMAND_PROFILE_MANIFEST_PATH` names
/// an absolute path instead, which is what an out-of-tree component crate uses:
/// it has no `contracts/` directory of its own.
fn read_manifest(manifest_dir: &str) -> String {
    println!("cargo:rerun-if-env-changed=SLIME_COMMAND_PROFILE_MANIFEST");
    println!("cargo:rerun-if-env-changed=SLIME_COMMAND_PROFILE_MANIFEST_PATH");
    let path = match std::env::var_os("SLIME_COMMAND_PROFILE_MANIFEST_PATH") {
        Some(path) => PathBuf::from(path),
        None => {
            let name = std::env::var("SLIME_COMMAND_PROFILE_MANIFEST")
                .unwrap_or_else(|_| "valid.zti".to_string());
            components_dir(manifest_dir)
                .parent()
                .expect("components lives inside the repository")
                .join("contracts/generation/v1/fixtures")
                .join(name)
        }
    };
    println!("cargo:rerun-if-changed={}", path.display());
    std::fs::read_to_string(&path).expect("read generation manifest")
}

fn out_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"))
}

/// Emit `command_profile.rs`: `spawn-service`'s client budget, RPC slot, and
/// command-name-to-executable-slot table.
///
/// Called only by `spawn-service`'s build script. The three symbols stay
/// build-time derived for reasons CP2 measured and recorded: `RPC_SLOT` cannot
/// use the `kind:endpoint+send,recv` role because `sel4-dango.zti` grants the
/// component three such endpoints and the query correctly refuses an ambiguous
/// role; `COMMAND_PROFILE` maps a command *name* to an executable, which is a
/// graph-shape fact rather than a property of a capability; and `CLIENT_BUDGET`
/// sizes a fixed array in type position on a 16 KiB stack, so no runtime answer
/// can replace it.
pub fn emit_command_profile() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let manifest = read_manifest(&manifest_dir);
    let out = out_dir();
    let Some(profile) = resolve_command_profile(&manifest) else {
        // A manifest declaring no command profile still builds the service: it
        // is then a spawn service with no commands, and the empty table says so
        // rather than failing the build of a plane that never spawns one.
        let client_budget = executable_block(&manifest, "spawn-service")
            .and_then(|service| field_int(service, "spawnBudget"))
            .unwrap_or(0);
        std::fs::write(
            out.join("command_profile.rs"),
            format!(
                "pub const CLIENT_BUDGET: usize = {client_budget};\n\
                 pub const RPC_SLOT: u32 = u32::MAX;\n\
                 pub const SHARED_BUFFER_FACTORY_SLOT: u32 = u32::MAX;\n\
                 pub const COMMAND_PROFILE: &[(&[u8], &[u8], u32)] = &[];\n"
            ),
        )
        .expect("write service-only command profile");
        return;
    };
    let rows = profile
        .commands
        .iter()
        .map(|(name, object, slot)| format!("    (b\"{name}\", b\"{object}\", {slot}),\n"))
        .collect::<String>();
    std::fs::write(
        out.join("command_profile.rs"),
        format!(
            "pub const CLIENT_BUDGET: usize = {};\n\
             pub const RPC_SLOT: u32 = {};\n\
             pub const COMMAND_PROFILE: &[(&[u8], &[u8], u32)] = &[\n{rows}];\n",
            profile.client_budget, profile.rpc_slot
        ),
    )
    .expect("write command profile");
}

/// Emit `dango_profile.rs`: the command names `dango` offers and its budget.
///
/// A separate entry point from [`emit_command_profile`] rather than one call
/// writing both files, because after CP3 the two live in different crates and
/// each crate's `OUT_DIR` is its own. Both derive from the same manifest
/// resolution, so the two tables cannot disagree about which commands exist.
pub fn emit_dango_profile() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let manifest = read_manifest(&manifest_dir);
    let out = out_dir();
    let (client_budget, names) = match resolve_command_profile(&manifest) {
        Some(profile) => (
            profile.client_budget,
            profile
                .commands
                .iter()
                .map(|(name, _, _)| format!("    b\"{name}\",\n"))
                .collect::<String>(),
        ),
        None => (0, String::new()),
    };
    std::fs::write(
        out.join("dango_profile.rs"),
        format!(
            "pub const CLIENT_BUDGET: u8 = {client_budget};\n\
             pub const COMMAND_NAMES: &[&[u8]] = &[\n{names}];\n"
        ),
    )
    .expect("write dango profile");
}

/// Copy the per-plane fabric profile the host builder rendered into `OUT_DIR`.
///
/// Not a derivation: `scripts/build/build-generation.py` renders this file from
/// the resolved fabric graph and points `SLIME_DATA_FABRIC_PROFILE` at it. The
/// checked-in `default_fabric_profile.rs` in `slime-components` is the
/// plain-`cargo` fallback, used when no builder ran.
pub fn emit_fabric_profile() {
    println!("cargo:rerun-if-env-changed=SLIME_DATA_FABRIC_PROFILE");
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let fallback = components_dir(&manifest_dir).join("lib/src/default_fabric_profile.rs");
    let profile_path = std::env::var_os("SLIME_DATA_FABRIC_PROFILE")
        .map(PathBuf::from)
        .unwrap_or(fallback);
    println!("cargo:rerun-if-changed={}", profile_path.display());
    let profile = std::fs::read(&profile_path).expect("read generated fabric profile");
    std::fs::write(out_dir().join("fabric_profile.rs"), profile).expect("write fabric profile");
}

/// One resolved command profile: the budget, the RPC slot, and each command's
/// `(name, executable object, executable slot)`.
struct CommandProfile<'a> {
    client_budget: usize,
    rpc_slot: usize,
    commands: Vec<(&'a str, &'a str, usize)>,
}

/// Resolve the command profile a manifest declares, in the consumer's own
/// CSpace numbering.
///
/// Kept as one function feeding both generators so `spawn-service`'s table and
/// `dango`'s name list cannot disagree. Every slot is read from the *consumer's*
/// bindings — the instance running `spawn-service` — because that is the CSpace
/// the generated numbers are resolved in; deriving from the profile owner's side
/// yields another instance's numbering, which agrees only while the two share
/// one layout.
fn resolve_command_profile(manifest: &str) -> Option<CommandProfile<'_>> {
    let profile_owner = command_profile_executable(manifest)?;
    let profile = field_list(profile_owner, "commandProfile").expect("command profile");
    let client_budget = field_int(profile_owner, "spawnBudget").expect("command spawn budget");
    let profile_executable = field(profile_owner, "name").expect("command profile executable name");
    let profile_instance =
        instance_for_executable(manifest, profile_executable).expect("command profile instance");
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
    let launcher = executable_launcher(manifest, &targets).expect("command launcher instance");
    // Identify the consumer by the executable it runs, not by its role in the
    // profile: `launcher` is whoever *sources* the executable grants, which may
    // be the instance that spawned the service rather than the service itself.
    let consumer = manifest
        .split("\n    {\n")
        .skip(1)
        .find(|block| field(block, "executable") == Some("spawn-service"))
        .and_then(|block| field(block, "name"))
        .expect("spawn-service command-profile consumer");
    let commands = profile
        .iter()
        .zip(targets.iter())
        .map(|(command, target)| {
            let grant =
                executable_grant(manifest, launcher, target).expect("profile executable grant");
            let slot = binding_slot(manifest, consumer, grant)
                .expect("consumer profile executable binding");
            let block = executable_block(manifest, target).expect("profile executable");
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
    let rpc_slot = related_binding_slot(manifest, consumer, peer, &["send", "recv"])
        .or_else(|| minted_binding_slot(manifest, consumer, &["send", "recv"]))
        .expect("command RPC binding");
    Some(CommandProfile {
        client_budget,
        rpc_slot,
        commands,
    })
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
    use super::{binding_slot, executable_grant, resolve_command_profile};

    fn executable_slot(manifest: &str, holder: &str, wanted: &str) -> Option<usize> {
        binding_slot(
            manifest,
            holder,
            executable_grant(manifest, holder, wanted)?,
        )
    }

    /// A manifest in the shape `zti` actually renders.
    ///
    /// The block separators these parsers split on (`"\n    {\n"`,
    /// `"\n        {\n"`) are the renderer's own indentation, so a fixture
    /// written in compact single-line form parses as *one* block and every
    /// lookup returns `None`. Both tests here were previously written that way
    /// and asserted real slot numbers, so they could only have failed — and
    /// they never ran, because `cargo test` does not build a `build.rs` as a
    /// test target and nothing else compiled this code. Extracting the parser
    /// into a library crate is what made them execute; the fixture is now
    /// generated to match the renderer instead of hand-written beside it.
    fn manifest(
        instances: &[(&str, &[(&str, usize)])],
        grants: &[(&str, &str, &str, &[&str])],
    ) -> String {
        let mut text = String::from("executables = [\n");
        for (name, _) in grants.iter().map(|(_, _, target, _)| (target, ())) {
            text.push_str(&format!(
                "    {{\n      name = \"{name}\";\n      object = \"sha256:{name}\";\n    }};\n"
            ));
        }
        text.push_str("  ];\ninstances = [\n");
        for (name, bindings) in instances {
            text.push_str(&format!(
                "    {{\n      bindings = [\n{}      ];\n      executable = \"{name}\";\n      name = \"{name}\";\n    }};\n",
                bindings
                    .iter()
                    .map(|(grant, slot)| format!(
                        "        {{\n          grant = \"{grant}\";\n          slot = {slot};\n        }};\n"
                    ))
                    .collect::<String>()
            ));
        }
        text.push_str("  ];\ngrants = [\n");
        for (name, source, target, rights) in grants {
            let declared = rights
                .iter()
                .map(|right| format!("\"{right}\"; "))
                .collect::<String>();
            text.push_str(&format!(
                "    {{\n      name = \"{name}\";\n      rights = [{declared}];\n      source = \"{source}\";\n      target = \"{target}\";\n    }};\n"
            ));
        }
        text.push_str("  ];\n");
        text
    }

    const EXEC: &[&str] = &["exec", "spawn"];

    #[test]
    fn command_slots_follow_explicit_instance_bindings() {
        // The property: a command's slot is read from the consumer's declared
        // binding, not from the order its grant happens to be declared in. The
        // two grants are declared sysinfo-then-echo while their slots are 7 and
        // 3, so a positional read would answer 3 for sysinfo.
        let text = manifest(
            &[("spawn-service", &[("z-sysinfo", 7), ("a-echo", 3)])],
            &[
                ("z-sysinfo", "spawn-service", "sysinfo", EXEC),
                ("a-echo", "spawn-service", "echo-agent", EXEC),
            ],
        );
        assert_eq!(
            executable_slot(&text, "spawn-service", "echo-agent"),
            Some(3)
        );
        assert_eq!(executable_slot(&text, "spawn-service", "sysinfo"), Some(7));
    }

    #[test]
    fn grant_declaration_order_does_not_change_command_slots() {
        let text = manifest(
            &[("spawn-service", &[("b-command", 11)])],
            &[("b-command", "spawn-service", "custom-command", EXEC)],
        );
        assert_eq!(
            executable_slot(&text, "spawn-service", "custom-command"),
            Some(11)
        );
        assert_eq!(executable_slot(&text, "spawn-service", "missing"), None);
    }

    #[test]
    fn only_an_exec_grant_names_a_command_executable() {
        // A command spawns through an `exec` grant. An edge to the same target
        // carrying send/recv is a different authority entirely, and resolving a
        // command through it would compile a spawn slot from a message channel.
        // Both grants below name `custom-command`, and only one is executable.
        let text = manifest(
            &[("spawn-service", &[("talk", 4), ("launch", 11)])],
            &[
                (
                    "talk",
                    "spawn-service",
                    "custom-command",
                    &["send", "recv"] as &[&str],
                ),
                ("launch", "spawn-service", "custom-command", EXEC),
            ],
        );
        assert_eq!(
            executable_slot(&text, "spawn-service", "custom-command"),
            Some(11)
        );
    }

    #[test]
    fn a_manifest_declaring_no_command_profile_resolves_none() {
        // The `None` arm the service-only generator branch depends on: a
        // manifest with instances and grants but no `commandProfile` must be
        // reported as absent rather than resolved from an unrelated executable.
        let text = manifest(
            &[("spawn-service", &[("b-command", 11)])],
            &[("b-command", "spawn-service", "custom-command", EXEC)],
        );
        assert!(resolve_command_profile(&text).is_none());
    }
}
