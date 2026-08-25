#!/usr/bin/env python3
"""CP5: pinned SDK consumption from a distinct component repository."""

from __future__ import annotations

import copy
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path
from typing import NoReturn

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts" / "lib"))

import component_sdk  # noqa: E402
from component_sdk import ComponentSdkError  # noqa: E402
from component_spec import admit_specs  # noqa: E402
from harness import load_script  # noqa: E402

BUILDER = ROOT / "scripts" / "build" / "build-generation.py"
SEL4_PIN_CHECK = ROOT / "scripts" / "check" / "check-sel4-pins.py"
SEL4_BUILDER = ROOT / "scripts" / "build" / "build-sel4.py"
DEMO = load_script("component_sdk_demo_check", "check/check-sel4-demo-plane.py")
CHECK = load_script("component_sdk_generation_check", "check/check-generation.py")
EXTERNAL_COMPONENTS = ("fabric-publisher-b", "fabric-subscriber")
EXTERNAL_MARKERS = ("[cp5-external-producer] done", "[cp5-external-consumer] done")
PINS = ROOT / "sel4" / "pins.toml"
# CP6: this gate no longer constructs its own SDK. It consumes
# `scripts/lib/component_sdk.py`'s exporter, so the bundle CP5's out-of-tree
# proof consumes is byte-for-byte the one CP7 publishes -- which is the point of
# CP6's exit condition, since a test-local alternate recipe could pass here and
# differ from every released SDK.
SDK_VERSION = "1.0.0"
SDK_REPOSITORY = "https://github.com/iceice666/slime_os-component_sdk"
PROFILE = "aarch64-sel4-qemu-virt"


def fail(message: str) -> NoReturn:
    raise SystemExit(f"component SDK out-of-tree check: {message}")


def run(
    command: list[str],
    *,
    cwd: Path,
    env: dict[str, str] | None = None,
    description: str,
) -> subprocess.CompletedProcess[str]:
    process = subprocess.run(
        command,
        cwd=cwd,
        env=env,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if process.returncode != 0:
        fail(f"{description} failed:\n{process.stdout}")
    return process


def git(command: list[str], *, cwd: Path, description: str) -> str:
    return run(["git", *command], cwd=cwd, description=description).stdout.strip()


def create_sdk(root: Path) -> tuple[Path, str]:
    """Export the repository-owned SDK and commit it as a pinnable repository.

    CP6 owns everything about the tree's contents, identity, and release record;
    this function only turns the export into a git commit an external checkout
    can pin. The pin check runs first because the exporter records
    `sel4/pins.toml`'s values into the release record, and a record naming
    unverified pins would be worse than no record.
    """
    run(
        [sys.executable, str(SEL4_PIN_CHECK)],
        cwd=ROOT,
        description="verify SDK seL4 source pins",
    )
    sdk = root / "component-sdk-v1"
    try:
        exported = component_sdk.export(
            sdk,
            version=SDK_VERSION,
            sdk_repository=SDK_REPOSITORY,
            profiles=(PROFILE,),
            source=ROOT,
        )
    except ComponentSdkError as error:
        fail(f"SDK export failed: {error}")
    component_sdk.verify_tree(exported.root, exported.record)
    git(["init", "-q"], cwd=sdk, description="initialize SDK repository")
    git(["config", "user.email", "cp5@example.invalid"], cwd=sdk, description="configure SDK git email")
    git(["config", "user.name", "CP5 gate"], cwd=sdk, description="configure SDK git name")
    git(["add", "."], cwd=sdk, description="stage SDK bundle")
    git(["commit", "-qm", f"sdk: component {SDK_VERSION}"], cwd=sdk, description="commit SDK bundle")
    commit = git(["rev-parse", "HEAD"], cwd=sdk, description="read SDK commit")
    if re.fullmatch(r"[0-9a-f]{40}", commit) is None:
        fail(f"SDK commit is not a full SHA-1 identity: {commit!r}")
    return sdk, commit


def external_workspace_manifest(sdk: Path, revision: str) -> str:
    sdk_url = sdk.resolve().as_uri()
    dependency = f'{{ git = "{sdk_url}", rev = "{revision}" }}'
    members = ", ".join(f'"{name}"' for name in EXTERNAL_COMPONENTS)
    return f"""[workspace]
resolver = "3"
members = [{members}]

[workspace.dependencies]
boot-contracts = {dependency}
slime-proto = {dependency}
slime-components = {dependency}
slime-rt = {dependency}

[workspace.dependencies.slime-build-support]
git = "{sdk_url}"
rev = "{revision}"

[profile.release]
panic = "abort"
opt-level = "s"
codegen-units = 1
debug = false
"""


def component_manifest(name: str) -> str:
    return f"""[package]
name = "cp5-{name}"
version = "0.1.0"
edition = "2024"
publish = false
rust-version = "1.96"
build = "build.rs"

[[bin]]
name = "{name}"
path = "src/main.rs"
test = false

[dependencies]
boot-contracts.workspace = true
slime-components.workspace = true
slime-proto.workspace = true
slime-rt.workspace = true

[build-dependencies]
slime-build-support.workspace = true
"""

def replace_once(text: str, old: str, new: str, *, label: str) -> str:
    if text.count(old) != 1:
        fail(f"cannot instrument {label}: expected exactly one source anchor")
    return text.replace(old, new, 1)


def instrument_external_source(text: str, name: str) -> str:
    if name == "fabric-publisher-b":
        text = replace_once(
            text,
            "use boot_contracts::generation::{BootAction, RIGHT_SEND};",
            "use boot_contracts::generation::{BootAction, RIGHT_BUFFER_MAP, RIGHT_SEND};",
            label="publisher authority imports",
        )
        text = replace_once(
            text,
            "slime_rt::entry!(main);\n",
            "slime_rt::entry!(main);\n\n"
            'const CP5_MODE: &str = match option_env!("CP5_MODE") { Some(mode) => mode, None => "baseline" };\n',
            label="publisher scenario selector",
        )
        text = replace_once(
            text,
            "fn publish_large(route_slot: u32, credit_slot: u32) {",
            "fn publish_large(route_slot: u32, credit_slot: u32, sequence: u64, reject: bool) {",
            label="publisher sample scenario parameters",
        )
        text = replace_once(
            text,
            '        if descriptor.status != 0 {\n            fail(b"declared publisher was denied");\n        }',
            '        if descriptor.status != 0 || descriptor.direction != DIRECTION_PUBLISH {\n'
            '            fail(b"publisher received subscriber authority");\n'
            '        }',
            label="publisher role direction",
        )
        text = replace_once(
            text,
            '    slime_rt::debug_write(b"[fabric-publisher-b] both publish roles received\\n");\n',
            '    slime_rt::debug_write(b"[fabric-publisher-b] both publish roles received\\n");\n'
            '    let forged_subscriber = WireCapabilityTransfer {\n'
            '        magic: slime_proto::capability_transfer::CAPABILITY_TRANSFER_MAGIC,\n'
            '        version: FORMAT_VERSION, status: 0,\n'
            '        flags: slime_proto::capability_transfer::FLAG_RETAIN_TRANSFER,\n'
            '        object_kind: OBJECT_KIND_SHARED_BUFFER_LOAN,\n'
            '        direction: boot_contracts::fabric_graph::DIRECTION_SUBSCRIBE,\n'
            '        rights_mask: RIGHT_BUFFER_MAP, route_identity: telemetry,\n'
            '    };\n'
            '    if slime_rt::capability_delegate(\n'
            '        CONTROL_SLOT, telemetry_slot, CapabilityDisposition::Retain,\n'
            '        OBJECT_KIND_SHARED_BUFFER_LOAN, RIGHT_BUFFER_MAP, &forged_subscriber.encode(),\n'
            '    ) == ERR_SUCCESS {\n'
            '        fail(b"publisher re-delegated subscriber authority");\n'
            '    }\n'
            '    slime_rt::debug_write(b"[cp5-external-producer] subscriber authority denied\\n");\n',
            label="publisher opposite-authority probe",
        )
        text = replace_once(
            text,
            '    publish_large(CONTROL_SLOT, CONTROL_SLOT);\n'
            '    slime_rt::debug_write(b"[fabric-publisher-b] large sample published\\n");\n',
            '    if CP5_MODE == "malformed" || CP5_MODE == "wrong-type" {\n'
            '        publish_large(CONTROL_SLOT, CONTROL_SLOT, 1, true);\n'
            '    }\n'
            '    let sequence = if CP5_MODE == "malformed" || CP5_MODE == "wrong-type" { 2 } else { 1 };\n'
            '    publish_large(CONTROL_SLOT, CONTROL_SLOT, sequence, false);\n'
            '    slime_rt::debug_write(b"[fabric-publisher-b] large sample published\\n");\n'
            '    if CP5_MODE == "peer-death" {\n'
            '        slime_rt::debug_write(b"[cp5-external-producer] peer death injected\\n");\n'
            '        slime_rt::exit(0);\n'
            '    }\n',
            label="publisher scenarios",
        )
        text = replace_once(
            text,
            "        flags: FLAG_LAST,\n"
            "        capability_kind: CAPABILITY_KIND_LOAN,\n"
            "        loan_id: loan.id,\n"
            "        offset: PAYLOAD_OFFSET,\n"
            "        length: PAYLOAD_LEN,\n"
            "        type_identity: telemetry_stream::TYPE_TAG,\n"
            "        sequence: 1,",
            '        flags: if CP5_MODE == "peer-death" || reject { 0 } else { FLAG_LAST },\n'
            '        capability_kind: CAPABILITY_KIND_LOAN,\n'
            '        loan_id: loan.id,\n'
            '        offset: PAYLOAD_OFFSET,\n'
            '        length: PAYLOAD_LEN,\n'
            '        type_identity: telemetry_stream::TYPE_TAG,\n'
            '        sequence,',
            label="publisher descriptor scenario fields",
        )
        text = replace_once(
            text,
            '    if slime_rt::shared_buffer_seal(buffer.slot) != ERR_SUCCESS {\n        fail(b"seal");\n    }\n',
            '    if slime_rt::shared_buffer_seal(buffer.slot) != ERR_SUCCESS {\n'
            '        fail(b"seal");\n'
            '    }\n'
            '    if slime_rt::shared_buffer_create(FACTORY_SLOT, 1, true).is_ok() {\n'
            '        fail(b"buffer quota did not refuse a second live buffer");\n'
            '    }\n'
            '    slime_rt::debug_write(b"[cp5-external-producer] buffer quota denied\\n");\n',
            label="publisher quota probe",
        )
        text = replace_once(
            text,
            '    if slime_rt::capability_delegate(\n'
            '        route_slot,\n'
            '        loan.slot,\n'
            '        CapabilityDisposition::Move,\n'
            '        OBJECT_KIND_SHARED_BUFFER_LOAN,\n'
            '        1 << 9,\n'
            '        &descriptor.encode(),\n'
            '    ) != ERR_SUCCESS\n'
            '    {\n'
            '        fail(b"publish descriptor");\n'
            '    }\n',
            '    let descriptor = if reject && CP5_MODE == "malformed" {\n'
            '        WireSampleDescriptor { version: FORMAT_VERSION + 1, ..descriptor }\n'
            '    } else if reject && CP5_MODE == "wrong-type" {\n'
            '        WireSampleDescriptor { type_identity: u64::MAX, ..descriptor }\n'
            '    } else {\n'
            '        descriptor\n'
            '    };\n'
            '    if slime_rt::capability_delegate(\n'
            '        route_slot,\n'
            '        loan.slot,\n'
            '        CapabilityDisposition::Move,\n'
            '        OBJECT_KIND_SHARED_BUFFER_LOAN,\n'
            '        1 << 9,\n'
            '        &descriptor.encode(),\n'
            '    ) != ERR_SUCCESS\n'
            '    {\n'
            '        fail(b"publish descriptor");\n'
            '    }\n',
            label="publisher rejected descriptor",
        )
        text = replace_once(
            text,
            "        if !valid_stream_event(&event, telemetry_stream::TYPE_TAG)\n"
            "            || event.event != EVENT_SAMPLE_TAKEN\n"
            "            || event.sequence != descriptor.sequence\n"
            "        {",
            "        if event.event != EVENT_SAMPLE_TAKEN\n"
            "            || event.sequence != descriptor.sequence\n"
            "            || (!reject && !valid_stream_event(&event, telemetry_stream::TYPE_TAG))\n"
            "        {",
            label="publisher rejected descriptor credit",
        )
        text = replace_once(
            text,
            '    slime_rt::debug_write(b"[fabric-publisher-b] loan settled by fabric\\n");\n',
            '    slime_rt::debug_write(b"[fabric-publisher-b] loan settled by fabric\\n");\n'
            '    if reject {\n'
            '        slime_rt::debug_write(if CP5_MODE == "malformed" {\n'
            '            b"[cp5-external-producer] malformed descriptor denied\\n"\n'
            '        } else {\n'
            '            b"[cp5-external-producer] wrong type denied\\n"\n'
            '        });\n'
            '    }\n',
            label="publisher rejection observation",
        )
    else:
        text = replace_once(
            text,
            "use slime_proto::fabric_qos::{QOS_EVENT_MAGIC, WireQosEvent};",
            "use slime_proto::fabric_qos::{EVENT_PEER_DEAD, QOS_EVENT_MAGIC, WireQosEvent};",
            label="subscriber peer-death import",
        )
        text = replace_once(
            text,
            '                let _ = event;\n                slime_rt::debug_write(b"[fabric-subscriber] QoS matched\\n");',
            '                if event.event == EVENT_PEER_DEAD {\n'
            '                    slime_rt::debug_write(b"[cp5-external-consumer] peer death observed\\n");\n'
            '                } else {\n'
            '                    slime_rt::debug_write(b"[fabric-subscriber] QoS matched\\n");\n'
            '                }',
            label="subscriber peer-death observation",
        )
    return text

def strip_external_trace_helpers(text: str, name: str) -> str:
    text = re.sub(
        r'// C8\.13\.2: this participant.*?mod occupancy_trace;\n\n',
        "",
        text,
        count=1,
        flags=re.DOTALL,
    )
    label = "publisher-b" if name == "fabric-publisher-b" else "subscriber"
    text = re.sub(
        rf'    // C8\.13\.2: gated to the traffic plane.*?occupancy_trace::report\(b"{label}"\).*?\n    \}}\n',
        "",
        text,
        count=1,
        flags=re.DOTALL,
    )
    return text

def create_external_checkout(root: Path, sdk: Path, revision: str) -> Path:
    checkout = root / "rp4-components"
    checkout.mkdir()
    (checkout / "Cargo.toml").write_text(
        external_workspace_manifest(sdk, revision), encoding="utf-8"
    )
    for name in EXTERNAL_COMPONENTS:
        crate = checkout / name
        (crate / "src").mkdir(parents=True)
        (crate / "Cargo.toml").write_text(component_manifest(name), encoding="utf-8")
        (crate / "build.rs").write_text(
            "fn main() {\n    slime_build_support::configure();\n}\n",
            encoding="utf-8",
        )
        source = ROOT / "components" / "bins" / name / "src" / "main.rs"
        text = source.read_text(encoding="utf-8")
        text = instrument_external_source(strip_external_trace_helpers(text, name), name)
        role = "producer" if name == "fabric-publisher-b" else "consumer"
        marker = f'    slime_rt::debug_write(b"[cp5-external-{role}] done\\n");\n'
        terminal = f'    slime_rt::debug_write(b"[{name}] done\\n");\n'
        if terminal not in text:
            fail(f"cannot place the external marker in {name}")
        text = text.replace(terminal, terminal + marker, 1)
        (crate / "src" / "main.rs").write_text(text, encoding="utf-8")
    git(["init", "-q"], cwd=checkout, description="initialize external component repository")
    git(["config", "user.email", "rp4@example.invalid"], cwd=checkout, description="configure external git email")
    git(["config", "user.name", "RP4 external components"], cwd=checkout, description="configure external git name")
    git(["add", "."], cwd=checkout, description="stage external components")
    git(["commit", "-qm", "feat: add RP4 data-path components"], cwd=checkout, description="commit external components")
    return checkout


def assert_external_boundary(checkout: Path, sdk: Path, revision: str) -> None:
    if (checkout.resolve().is_relative_to(ROOT.resolve())):
        fail("external component repository lives inside the Slime checkout")
    toplevel = Path(
        git(
            ["rev-parse", "--show-toplevel"],
            cwd=checkout,
            description="locate external repository",
        )
    ).resolve()
    if toplevel != checkout.resolve():
        fail("external component directory is not its own git repository")
    metadata = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--quiet"],
        cwd=checkout,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
    )
    data = json.loads(metadata.stdout)
    external_root = checkout.resolve()
    sdk_root = sdk.resolve()
    sdk_packages = {
        "boot-contracts",
        "slime-build-support",
        "slime-components",
        "slime-proto",
        "slime-rt",
    }
    for package in data["packages"]:
        manifest = Path(package["manifest_path"]).resolve()
        if manifest.is_relative_to(ROOT.resolve()) and not manifest.is_relative_to(sdk_root):
            fail(f"external dependency escaped the SDK boundary: {manifest}")
        if package["name"] in sdk_packages:
            source = package.get("source") or ""
            if not source.startswith("git+") or f"#{revision}" not in source:
                fail(f"{package['name']} did not resolve through the pinned SDK commit")
    for path in checkout.rglob("*"):
        if not path.is_file() or ".git" in path.parts:
            continue
        text = path.read_text(encoding="utf-8", errors="ignore")
        if str(ROOT / "components") in text:
            fail(f"external checkout names the repository components tree: {path}")
    if not external_root.is_dir():
        fail("external checkout disappeared before build")


def build_external_components(
    root: Path,
    checkout: Path,
    sdk: Path,
    *,
    mode: str = "baseline",
) -> dict[str, Path]:
    if mode not in {"baseline", "peer-death", "malformed", "wrong-type"}:
        fail(f"unknown external scenario {mode!r}")
    pins = tomllib.loads(PINS.read_text(encoding="utf-8"))
    target = sdk / "targets/aarch64-sel4-minimal.json"
    target_dir = root / f"external-target-{mode}"
    environment = os.environ.copy()
    environment["RUSTUP_TOOLCHAIN"] = pins["rust_sel4"]["toolchain"]
    environment["SEL4_PREFIX"] = str(ROOT / "build" / "sel4-prefix")
    environment["SLIME_TARGET_PROFILE"] = "aarch64-sel4-qemu-virt"
    environment["CARGO_TARGET_DIR"] = str(target_dir)
    environment["CP5_MODE"] = mode
    environment["RUSTFLAGS"] = " ".join(
        (
            "-C link-arg=--build-id=none",
            f"--remap-path-prefix={checkout}=./rp4-components",
            f"--remap-path-prefix={sdk}=./slime-sdk-v1",
        )
    )
    run(
        [
            "cargo",
            "build",
            "--release",
            "--target",
            str(target),
            "-Z",
            "json-target-spec",
            "-Z",
            "build-std=core,alloc,compiler_builtins",
            "-Z",
            "build-std-features=compiler-builtins-mem",
        ],
        cwd=checkout,
        env=environment,
        description=f"build out-of-tree RP4 components ({mode})",
    )
    release = target_dir / target.stem / "release"
    built = {name: release / f"{name}.elf" for name in EXTERNAL_COMPONENTS}
    for name, elf in built.items():
        if not elf.is_file():
            fail(f"external build produced no {name} ELF")
        workspace = (
            ROOT
            / "target"
            / "components"
            / "aarch64-sel4-qemu-virt"
            / "sel4-demo-1"
            / "aarch64-sel4-minimal"
            / "release"
            / f"{name}.elf"
        )
        if workspace.is_file() and workspace.read_bytes() == elf.read_bytes():
            fail(f"external {name} ELF is byte-identical to the workspace artifact")
    return built


def external_specs(root: Path, elves: dict[str, Path]) -> None:
    for entry in admit_specs():
        spec = copy.deepcopy(entry.spec)
        if entry.name in elves:
            spec["implementation"] = {
                "provider": "external",
                "binary": f"cp5-{entry.name}",
                "contentHash": hashlib.sha256(elves[entry.name].read_bytes()).hexdigest(),
            }
        (root / f"{entry.name}.zti").write_text(component_sdk.zti(spec) + "\n", encoding="utf-8")


def build_generation(output: Path, specs: Path, elves: dict[str, Path]) -> subprocess.CompletedProcess[str]:
    environment = os.environ.copy()
    environment["SLIME_TARGET_PROFILE"] = "aarch64-sel4-qemu-virt"
    environment["SLIME_SEL4_MANIFEST"] = "sel4-demo"
    command = [
        sys.executable,
        str(BUILDER),
        "--component-spec-root",
        str(specs),
    ]
    for name in EXTERNAL_COMPONENTS:
        command += ["--external-component", f"cp5-{name}={elves[name]}"]
    command.append(str(output))
    return subprocess.run(
        command,
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )


def prove_mixed_generation(
    root: Path,
    elves: dict[str, Path],
    *,
    label: str,
) -> Path:
    specs = root / f"component-specs-{label}"
    specs.mkdir()
    external_specs(specs, elves)
    output = root / f"mixed-generation-{label}"
    built = build_generation(output, specs, elves)
    if built.returncode != 0:
        fail(f"mixed external generation build failed:\n{built.stdout}")
    for name in EXTERNAL_COMPONENTS:
        marker = f"implementation=cp5-{name} provider=external"
        if marker not in built.stdout:
            fail(f"generation builder did not report {name} as externally sourced")
    generation = (output / "generation.bin").read_bytes()
    bootstore_path = output / "boot-store.bin"
    if not bootstore_path.is_file():
        fail("mixed build omitted its signed boot store")
    checked_generation = CHECK.check_generation(generation)
    checked_store = CHECK.check_bootstore(bootstore_path.read_bytes())
    if checked_store["selected"]["identity"] != checked_generation["identity"]:
        fail("signed boot store did not select the mixed external generation")
    generation_identity = checked_generation["identity"].hex()
    run(
        [
            sys.executable,
            str(SEL4_BUILDER),
            "--demo-plane",
            "--prebuilt-generation",
            str(output / "generation.bin"),
        ],
        cwd=ROOT,
        description=f"embed mixed external generation ({label})",
    )
    identity_manifest = json.loads(
        (ROOT / "build" / "slime-sel4-demo.identity.json").read_text(encoding="utf-8")
    )
    embedded = identity_manifest.get("generation")
    if not isinstance(embedded, dict) or embedded.get("identity") != generation_identity:
        fail("demo image did not embed the exact signed external generation")
    pins = DEMO.load_pins()
    profile = pins["qemu_arm_virt"]
    transcript = DEMO.boot(profile, DEMO.IMAGE, terminal=DEMO.TERMINAL_MARKER)
    DEMO.check_transcript(transcript)
    expected = {
        "baseline": EXTERNAL_MARKERS
        + (
            "[cp5-external-producer] subscriber authority denied",
            "[cp5-external-producer] buffer quota denied",
            "[fabric-subscriber] route publish denied",
            "[fabric-subscriber] re-delegation denied",
            "[fabric] ungranted component denied: fabric-intruder",
        ),
        "peer-death": (
            "[cp5-external-producer] peer death injected",
            "[cp5-external-consumer] peer death observed",
            "[cp5-external-consumer] done",
        ),
        "malformed": (
            "[fabric] reject: descriptor validation",
            "[cp5-external-producer] malformed descriptor denied",
            "[cp5-external-consumer] done",
        ),
        "wrong-type": (
            "[fabric] reject: descriptor validation",
            "[cp5-external-producer] wrong type denied",
            "[cp5-external-consumer] done",
        ),
    }[label]
    for marker in expected:
        if marker not in transcript:
            fail(f"{label} boot omitted {marker}")
    return output / "generation.bin"

def prove_fallback() -> None:
    run(
        [sys.executable, str(SEL4_BUILDER), "--demo-plane"],
        cwd=ROOT,
        description="rebuild in-tree fallback demo",
    )
    pins = DEMO.load_pins()
    profile = pins["qemu_arm_virt"]
    transcript = DEMO.boot(profile, DEMO.IMAGE, terminal=DEMO.TERMINAL_MARKER)
    DEMO.check_transcript(transcript)
    DEMO.check_ordered_across_chains(transcript)
    if any(marker in transcript for marker in EXTERNAL_MARKERS):
        fail("fallback demo retained an out-of-tree component marker")


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="slime-component-sdk-") as temporary:
        root = Path(temporary)
        sdk, revision = create_sdk(root)
        checkout = create_external_checkout(root, sdk, revision)
        try:
            assert_external_boundary(checkout, sdk, revision)
            for mode in ("baseline", "peer-death", "malformed", "wrong-type"):
                elves = build_external_components(root, checkout, sdk, mode=mode)
                prove_mixed_generation(root, elves, label=mode)
            shutil.rmtree(checkout)
            if checkout.exists():
                fail("external checkout could not be removed before fallback")
        finally:
            prove_fallback()

    print(
        "component SDK out-of-tree check: a pinned git SDK built the RP4 large-sample "
        "producer and bounded consumer from a distinct repository with no components/ "
        "path dependency; their content-bound ELFs entered one verified signed demo "
        "generation, passed the AArch64 QEMU authority and reclamation gate, and the "
        "in-tree fallback still passed"
    )


if __name__ == "__main__":
    main()
