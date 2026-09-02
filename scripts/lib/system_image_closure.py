"""Canonical CP11 system-image closure and test-run resolution."""

from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType

import system_image_closure_contract as image_contract
import system_test_run_contract as test_contract
from component_spec import admit_specs, interface_catalogue
from component_sdk import tree_digest
from harness import ROOT
from system_spec import CompiledSystem, compile_system, derive_manifest
from zutai_cli import STDLIB, binary

IMAGE_CONTRACT_ROOT = ROOT / "contracts" / "system-image-closure" / "v1"
TEST_CONTRACT_ROOT = ROOT / "contracts" / "system-test-run" / "v1"
IMAGE_CHECKER = IMAGE_CONTRACT_ROOT / "check.zt"
TEST_CHECKER = TEST_CONTRACT_ROOT / "check.zt"
SYSTEM_ROOT = ROOT / "contracts" / "system-spec" / "v1" / "systems"
_NAME = re.compile(r"^[a-z][a-z0-9-]*$")
_SHA256 = re.compile(r"^[0-9a-f]{64}$")

_IMAGE_FIELDS = {
    "formatVersion",
    "name",
    "systemSpec",
    "systemIdentity",
    "implementations",
    "target",
    "root",
    "loader",
    "releaseInputs",
    "buildParameters",
    "expectedOutputs",
}
_ARTIFACT_FIELDS = {"path", "kind", "identity"}
_IMPLEMENTATION_FIELDS = {"component", "provider", "artifact", "identity", "buildProfile"}
_TARGET_FIELDS = {
    "profile",
    "platform",
    "sdkRelease",
    "prefix",
    "toolchain",
    "rustSel4Commit",
}
_ROLE_FIELDS = {"role", "implementation", "parameters"}
_NAMED_INPUT_FIELDS = {"name", "artifact"}
_PARAMETER_FIELDS = {"name", "value"}
_TEST_FIELDS = {
    "formatVersion",
    "name",
    "imageClosureIdentity",
    "executionKind",
    "executionProfile",
    "disks",
    "networks",
    "devices",
    "faultControls",
    "timeoutSeconds",
    "markerContractIdentity",
    "forbiddenOutcomes",
}
_FIXTURE_FIELDS = {"name", "path", "identity", "writable"}
_FAULT_FIELDS = {"kind", "target", "value"}


class SystemImageClosureError(ValueError):
    pass


@dataclass(frozen=True)
class CompiledClosure:
    path: Path
    value: dict
    normalized: bytes
    identity: bytes


@dataclass(frozen=True)
class CompiledTestRun:
    path: Path
    value: dict
    normalized: bytes
    identity: bytes


@dataclass(frozen=True)
class ResolvedClosure:
    compiled: CompiledClosure
    system: CompiledSystem
    manifest: dict
    artifacts: dict[str, Path]
    external_components: dict[str, Path]
    build_parameters: dict[str, str]
    # Per-component build profile, keyed by component. `default` for every
    # ordinary implementation; a scenario profile changes that component's ELF
    # bytes, so it belongs to the identity that selected it.
    build_profiles: dict[str, str]
    # What the root task is built to be, and the closed platform-qualified
    # parameters that vary it. `embedded-generation` with no parameters is the
    # ordinary case; the rest were `build-sel4.py` variant branches.
    root_role: str
    root_parameters: tuple[str, ...]


def _fail(message: str) -> None:
    raise SystemImageClosureError(message)


def normalize(value: object) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True) + "\n").encode(
        "utf-8"
    )


def tree_identity(path: Path) -> str:
    if not path.is_dir():
        _fail(f"missing tree artifact: {path}")
    return tree_digest(path)


def artifact_identity(path: Path, kind: str) -> str:
    if kind == "file":
        if not path.is_file():
            _fail(f"missing file artifact: {path}")
        return hashlib.sha256(path.read_bytes()).hexdigest()
    if kind == "tree":
        return tree_identity(path)
    _fail(f"unknown artifact kind {kind!r}")


def _exact(value: object, fields: set[str], label: str) -> dict:
    if not isinstance(value, dict):
        _fail(f"{label}: expected a record")
    actual = set(value)
    if actual != fields:
        _fail(f"{label}: fields are {sorted(actual)}, expected {sorted(fields)}")
    return value


def _bounded_text(value: object, bound: int, label: str, *, empty: bool = False) -> str:
    if not isinstance(value, str) or (not empty and not value):
        _fail(f"{label}: expected {'possibly empty ' if empty else ''}text")
    if len(value.encode("utf-8")) > bound:
        _fail(f"{label}: text exceeds bound")
    return value


def _digest(value: object, label: str, contract: ModuleType = image_contract) -> str:
    text = _bounded_text(value, contract.MAX_DIGEST_BYTES, label)
    if _SHA256.fullmatch(text) is None:
        _fail(f"{label}: expected lowercase SHA-256")
    return text


def _list(value: object, bound: int, label: str) -> list:
    if not isinstance(value, list):
        _fail(f"{label}: expected a list")
    if len(value) > bound:
        _fail(f"{label}: count exceeds bound")
    return value


def _run_zutai(path: Path, checker: Path, variable: str, source_bound: int) -> dict:
    if not path.is_file():
        _fail(f"record not found: {path}")
    if path.stat().st_size > source_bound:
        _fail(f"{path}: source exceeds bound")
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    environment[variable] = str(path)
    process = subprocess.run(
        [str(binary()), "run", str(checker)],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode != 0 or not process.stdout.startswith("#valid"):
        detail = (process.stderr or process.stdout).strip()
        _fail(f"{path}: malformed Zutai input: {detail}")
    process = subprocess.run(
        [str(binary()), "json", str(path)],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode != 0:
        _fail(f"{path}: invalid Zutai JSON projection: {(process.stderr or process.stdout).strip()}")
    try:
        value = json.loads(process.stdout)
    except json.JSONDecodeError as error:
        _fail(f"{path}: invalid Zutai JSON projection: {error}")
    return value


def _artifact(raw: object, label: str, *, base: Path | None = None) -> tuple[dict, Path | None]:
    value = _exact(raw, _ARTIFACT_FIELDS, label)
    path_text = _bounded_text(value["path"], image_contract.MAX_PATH_BYTES, f"{label}.path")
    kind = _bounded_text(value["kind"], image_contract.MAX_NAME_BYTES, f"{label}.kind")
    if kind not in image_contract.ARTIFACT_KINDS:
        _fail(f"{label}.kind: unknown artifact kind {kind!r}")
    _digest(value["identity"], f"{label}.identity")
    if base is None:
        return value, None
    relative = Path(path_text)
    if relative.is_absolute() or ".." in relative.parts:
        _fail(f"{label}.path: path must be normalized and closure-relative")
    resolved = (base / relative).resolve()
    if not resolved.is_relative_to(base.resolve()):
        _fail(f"{label}.path: path escapes closure root")
    observed = artifact_identity(resolved, kind)
    if observed != value["identity"]:
        _fail(f"{label}: identity mismatch for {path_text}")
    return value, resolved


def compile_closure(path: Path, contract: ModuleType = image_contract) -> CompiledClosure:
    value = _exact(
        _run_zutai(path.resolve(), IMAGE_CHECKER, "SLIME_SYSTEM_IMAGE_CLOSURE_PATH", contract.MAX_SOURCE_BYTES),
        _IMAGE_FIELDS,
        str(path),
    )
    if value["formatVersion"] != contract.FORMAT_VERSION:
        _fail(f"unsupported image closure version {value['formatVersion']}")
    name = _bounded_text(value["name"], contract.MAX_NAME_BYTES, "name")
    if _NAME.fullmatch(name) is None or name != path.stem:
        _fail("closure name must match its file name and use canonical spelling")
    _digest(value["systemIdentity"], "systemIdentity")
    _artifact(value["systemSpec"], "systemSpec")
    implementations = _list(value["implementations"], contract.MAX_IMPLEMENTATIONS, "implementations")
    component_names: list[str] = []
    for index, raw in enumerate(implementations):
        entry = _exact(raw, _IMPLEMENTATION_FIELDS, f"implementations[{index}]")
        component_names.append(_bounded_text(entry["component"], contract.MAX_NAME_BYTES, "component"))
        provider = _bounded_text(entry["provider"], contract.MAX_NAME_BYTES, "provider")
        if provider not in contract.PROVIDERS:
            _fail(f"implementations[{index}].provider: unknown provider {provider!r}")
        _artifact(entry["artifact"], f"implementations[{index}].artifact")
        _digest(entry["identity"], f"implementations[{index}].identity")
        profile = _bounded_text(
            entry["buildProfile"], contract.MAX_NAME_BYTES, "buildProfile", empty=True
        )
        if profile not in contract.BUILD_PROFILES:
            _fail(
                f"implementations[{index}].buildProfile: unknown profile {profile!r}; "
                f"expected one of {sorted(contract.BUILD_PROFILES)}"
            )
    if component_names != sorted(component_names) or len(set(component_names)) != len(component_names):
        _fail("implementations must be uniquely keyed and sorted by component")
    target = _exact(value["target"], _TARGET_FIELDS, "target")
    _bounded_text(target["profile"], contract.MAX_NAME_BYTES, "target.profile")
    _bounded_text(target["platform"], contract.MAX_NAME_BYTES, "target.platform")
    _artifact(target["sdkRelease"], "target.sdkRelease")
    _artifact(target["prefix"], "target.prefix")
    _bounded_text(target["toolchain"], contract.MAX_TEXT_BYTES, "target.toolchain")
    _bounded_text(target["rustSel4Commit"], contract.MAX_TEXT_BYTES, "target.rustSel4Commit")
    for role_name, allowed in (("root", contract.ROOT_ROLES), ("loader", contract.LOADER_ROLES)):
        role = _exact(value[role_name], _ROLE_FIELDS, role_name)
        if role["role"] not in allowed:
            _fail(f"{role_name}.role: unknown role {role['role']!r}")
        _artifact(role["implementation"], f"{role_name}.implementation")
        role_parameters = _list(
            role["parameters"], contract.MAX_ROLE_PARAMETERS, f"{role_name}.parameters"
        )
        for index, parameter in enumerate(role_parameters):
            _bounded_text(parameter, contract.MAX_TEXT_BYTES, f"{role_name}.parameters[{index}]")
    release_inputs = _list(value["releaseInputs"], contract.MAX_RELEASE_INPUTS, "releaseInputs")
    release_names = []
    for index, raw in enumerate(release_inputs):
        entry = _exact(raw, _NAMED_INPUT_FIELDS, f"releaseInputs[{index}]")
        release_names.append(_bounded_text(entry["name"], contract.MAX_NAME_BYTES, "release input name"))
        _artifact(entry["artifact"], f"releaseInputs[{index}].artifact")
    if release_names != sorted(release_names) or len(set(release_names)) != len(release_names):
        _fail("releaseInputs must be uniquely keyed and sorted by name")
    parameters = _list(value["buildParameters"], contract.MAX_BUILD_PARAMETERS, "buildParameters")
    parameter_names = []
    for index, raw in enumerate(parameters):
        entry = _exact(raw, _PARAMETER_FIELDS, f"buildParameters[{index}]")
        parameter_names.append(_bounded_text(entry["name"], contract.MAX_NAME_BYTES, "parameter name"))
        _bounded_text(entry["value"], contract.MAX_TEXT_BYTES, "parameter value", empty=True)
    if parameter_names != sorted(parameter_names) or len(set(parameter_names)) != len(parameter_names):
        _fail("buildParameters must be uniquely keyed and sorted by name")
    if parameter_names:
        unknown = sorted(set(parameter_names) - set(contract.BUILD_PARAMETERS))
        if unknown:
            _fail(
                f"buildParameters names {unknown}, which this closure version does not admit; "
                f"expected a subset of {sorted(contract.BUILD_PARAMETERS)}"
            )
    outputs = _list(value["expectedOutputs"], contract.MAX_OUTPUTS, "expectedOutputs")
    if outputs != list(contract.OUTPUT_CLASSES):
        _fail("expectedOutputs must name the complete canonical output set in contract order")
    normalized = normalize(value)
    if len(normalized) > contract.MAX_NORMALIZED_BYTES:
        _fail("normalized closure exceeds bound")
    return CompiledClosure(
        path.resolve(),
        value,
        normalized,
        hashlib.sha256(contract.IDENTITY_DOMAIN + normalized).digest(),
    )


def compile_test_run(path: Path, contract: ModuleType = test_contract) -> CompiledTestRun:
    value = _exact(
        _run_zutai(path.resolve(), TEST_CHECKER, "SLIME_SYSTEM_TEST_RUN_PATH", contract.MAX_SOURCE_BYTES),
        _TEST_FIELDS,
        str(path),
    )
    if value["formatVersion"] != contract.FORMAT_VERSION:
        _fail(f"unsupported test-run version {value['formatVersion']}")
    name = _bounded_text(value["name"], contract.MAX_NAME_BYTES, "name")
    if _NAME.fullmatch(name) is None or name != path.stem:
        _fail("test-run name must match its file name and use canonical spelling")
    _digest(value["imageClosureIdentity"], "imageClosureIdentity", contract)
    if value["executionKind"] not in contract.EXECUTION_KINDS:
        _fail(f"unknown execution kind {value['executionKind']!r}")
    _bounded_text(value["executionProfile"], contract.MAX_NAME_BYTES, "executionProfile")
    for field in ("disks", "networks", "devices"):
        fixtures = _list(value[field], contract.MAX_FIXTURES, field)
        names = []
        for index, raw in enumerate(fixtures):
            entry = _exact(raw, _FIXTURE_FIELDS, f"{field}[{index}]")
            names.append(_bounded_text(entry["name"], contract.MAX_NAME_BYTES, "fixture name"))
            _bounded_text(entry["path"], contract.MAX_PATH_BYTES, "fixture path")
            _digest(entry["identity"], "fixture identity", contract)
            if not isinstance(entry["writable"], bool):
                _fail("fixture writable must be boolean")
        if names != sorted(names) or len(set(names)) != len(names):
            _fail(f"{field} must be uniquely keyed and sorted by name")
    faults = _list(value["faultControls"], contract.MAX_FAULT_CONTROLS, "faultControls")
    for index, raw in enumerate(faults):
        entry = _exact(raw, _FAULT_FIELDS, f"faultControls[{index}]")
        if entry["kind"] not in contract.FAULT_KINDS:
            _fail(f"faultControls[{index}]: unknown kind {entry['kind']!r}")
        _bounded_text(entry["target"], contract.MAX_TEXT_BYTES, "fault target")
        _bounded_text(entry["value"], contract.MAX_TEXT_BYTES, "fault value")
    timeout = value["timeoutSeconds"]
    if not isinstance(timeout, int) or isinstance(timeout, bool) or not 0 < timeout <= contract.MAX_TIMEOUT_SECONDS:
        _fail("timeoutSeconds is outside the declared bound")
    _digest(value["markerContractIdentity"], "markerContractIdentity", contract)
    forbidden = _list(value["forbiddenOutcomes"], contract.MAX_FORBIDDEN_OUTCOMES, "forbiddenOutcomes")
    for index, outcome in enumerate(forbidden):
        _bounded_text(outcome, contract.MAX_TEXT_BYTES, f"forbiddenOutcomes[{index}]")
    normalized = normalize(value)
    if len(normalized) > contract.MAX_NORMALIZED_BYTES:
        _fail("normalized test run exceeds bound")
    return CompiledTestRun(path.resolve(), value, normalized, hashlib.sha256(contract.IDENTITY_DOMAIN + normalized).digest())


def resolve_closure(path: Path, *, source_root: Path = ROOT) -> ResolvedClosure:
    compiled = compile_closure(path)
    value = compiled.value
    base = source_root.resolve()
    _, system_path = _artifact(value["systemSpec"], "systemSpec", base=base)
    assert system_path is not None
    components = {entry.name: entry.spec for entry in admit_specs(catalogue=interface_catalogue())}
    system = compile_system(system_path, components=components)
    if system.identity.hex() != value["systemIdentity"]:
        _fail("systemIdentity does not match the compiled system")
    if system.spec["targetRequirement"] != value["target"]["profile"]:
        _fail("system target requirement does not match the closure target profile")
    artifacts: dict[str, Path] = {"systemSpec": system_path}
    _, sdk = _artifact(value["target"]["sdkRelease"], "target.sdkRelease", base=base)
    _, prefix = _artifact(value["target"]["prefix"], "target.prefix", base=base)
    assert sdk is not None and prefix is not None
    artifacts["sdkRelease"] = sdk
    artifacts["prefix"] = prefix
    sdk_record = json.loads(sdk.read_text(encoding="utf-8"))
    if normalize(sdk_record) != sdk.read_bytes():
        _fail("SDK release record is not canonical JSON")
    profiles = [entry for entry in sdk_record.get("profiles", []) if entry.get("profile") == value["target"]["profile"]]
    if len(profiles) != 1:
        _fail("SDK release does not contain exactly one selected target profile")
    profile = profiles[0]
    if profile.get("platform") != value["target"]["platform"]:
        _fail("target profile and platform asset do not pair")
    if sdk_record.get("toolchain") != value["target"]["toolchain"]:
        _fail("closure toolchain does not match the SDK release")
    if sdk_record.get("rustSel4", {}).get("commit") != value["target"]["rustSel4Commit"]:
        _fail("closure rust-sel4 commit does not match the SDK release")
    if profile.get("prefix", {}).get("treeHash") != value["target"]["prefix"]["identity"]:
        _fail("closure prefix identity does not match the selected SDK asset")
    specs = {entry.name: entry for entry in admit_specs(catalogue=interface_catalogue())}
    selections = {entry["component"]: entry for entry in value["implementations"]}
    if set(selections) != set(system.spec["components"]):
        _fail("implementation selections do not exactly cover the system components")
    external: dict[str, Path] = {}
    for component in system.spec["components"]:
        spec = specs[component]
        selection = selections[component]
        implementation = spec.spec["implementation"]
        if selection["provider"] != implementation["provider"]:
            _fail(f"{component}: closure provider disagrees with the component spec")
        if selection["identity"] != spec.identity.hex():
            _fail(f"{component}: component-spec identity mismatch")
        _, implementation_artifact = _artifact(
            selection["artifact"], f"implementations[{component}].artifact", base=base
        )
        assert implementation_artifact is not None
        artifacts[f"component:{component}"] = implementation_artifact
        if implementation["provider"] == image_contract.PROVIDER_EXTERNAL:
            if implementation_artifact.is_dir():
                _fail(f"{component}: external implementation artifact must be a file")
            if hashlib.sha256(implementation_artifact.read_bytes()).hexdigest() != implementation["contentHash"]:
                _fail(f"{component}: external ELF hash disagrees with component spec")
            external[implementation["binary"]] = implementation_artifact
    for name in ("root", "loader"):
        _, artifact = _artifact(value[name]["implementation"], f"{name}.implementation", base=base)
        assert artifact is not None
        artifacts[name] = artifact
    for entry in value["releaseInputs"]:
        _, artifact = _artifact(entry["artifact"], f"releaseInputs[{entry['name']}].artifact", base=base)
        assert artifact is not None
        artifacts[f"release:{entry['name']}"] = artifact
    release_names = [entry["name"] for entry in value["releaseInputs"]]
    required_release_inputs = {
        "boot-contracts",
        "cargo-lock",
        "component-build-support",
        "component-cargo-config",
        "component-library",
        "component-proto",
        "component-runtime",
        "component-spec-contract",
        "interface-schema-contract",
        "just-recipes",
        "justfile",
        "root-child",
        "root-target",
        "target-spec",
        "workspace-manifest",
    }
    if set(release_names) != required_release_inputs:
        _fail(
            "releaseInputs must exactly cover the canonical workspace build inputs; "
            f"missing={sorted(required_release_inputs - set(release_names))} "
            f"extra={sorted(set(release_names) - required_release_inputs)}"
        )
    root_role = value["root"]["role"]
    if root_role not in image_contract.ROOT_ROLES:
        _fail(f"unknown root role {root_role!r}")
    root_parameters = value["root"]["parameters"]
    unknown_root = sorted(set(root_parameters) - set(image_contract.ROOT_PARAMETERS))
    if unknown_root:
        _fail(
            f"root.parameters names {unknown_root}, which this closure version does not "
            f"admit; expected a subset of {sorted(image_contract.ROOT_PARAMETERS)}"
        )
    if len(set(root_parameters)) != len(root_parameters):
        _fail("root.parameters must be unique")
    if root_parameters != sorted(root_parameters):
        _fail("root.parameters must be sorted")
    # Both parameters compile a platform-specific address or marker into the
    # root task, so the wrong platform is refused before Cargo runs rather than
    # producing a root that reads a device it does not have.
    platform_name = value["target"]["platform"]
    for parameter, required_platform in (
        (image_contract.ROOT_PARAMETER_QEMU_KEYBOARD, "qemu-arm-virt"),
        (image_contract.ROOT_PARAMETER_DUO_TEST_TERMINATOR, "cv1800b-duo"),
    ):
        if parameter in root_parameters and platform_name != required_platform:
            _fail(
                f"root.parameters: {parameter!r} requires platform {required_platform!r}, "
                f"not {platform_name!r}"
            )
    if artifacts["root"] != (base / "slime-root").resolve():
        _fail("every root role's implementation must be the declared slime-root tree")
    if value["loader"]["role"] != image_contract.LOADER_ROLE_KERNEL_LOADER:
        _fail("this closure requires the kernel-loader role")
    if value["loader"]["parameters"]:
        _fail("kernel-loader accepts no role parameters in closure version 1")
    if artifacts["loader"] != (base / "deps" / "rust-sel4").resolve():
        _fail("kernel-loader implementation must be the declared rust-sel4 tree")
    if artifacts["release:target-spec"].name != Path(profile["cargoTarget"]).name:
        _fail("target-spec release input does not match the selected SDK profile")
    if artifact_identity(artifacts["release:target-spec"], "file") != profile["targetSpecHash"]:
        _fail("selected SDK target specification hash does not match its bytes")
    expected_root_target = (
        "aarch64-sel4-roottask-minimal.json"
        if value["target"]["profile"].startswith("aarch64-")
        else "riscv64imac-sel4-roottask-minimal.json"
    )
    if artifacts["release:root-target"].name != expected_root_target:
        _fail("root target specification does not match the selected target profile")
    parameters = {entry["name"]: entry["value"] for entry in value["buildParameters"]}
    profiles = {entry["component"]: entry["buildProfile"] for entry in value["implementations"]}
    manifest = derive_manifest(system)
    return ResolvedClosure(
        compiled,
        system,
        manifest,
        artifacts,
        external,
        parameters,
        profiles,
        root_role,
        tuple(root_parameters),
    )


def make_build_result(
    resolved: ResolvedClosure,
    *,
    output_root: Path,
    generation: Path,
    root: Path,
    loader: Path,
    image: Path,
    identity_manifest: Path,
) -> tuple[dict, bytes, str]:
    def record(path: Path) -> dict:
        payload = path.read_bytes()
        return {
            "path": path.relative_to(output_root).as_posix(),
            "bytes": len(payload),
            "sha256": hashlib.sha256(payload).hexdigest(),
        }

    value = {
        "formatVersion": image_contract.FORMAT_VERSION,
        "closureIdentity": resolved.compiled.identity.hex(),
        "systemIdentity": resolved.system.identity.hex(),
        "platform": resolved.compiled.value["target"]["platform"],
        "rootRole": resolved.compiled.value["root"]["role"],
        "loaderRole": resolved.compiled.value["loader"]["role"],
        "targetProfile": resolved.compiled.value["target"]["profile"],
        "generation": record(generation),
        "root": record(root),
        "loader": record(loader),
        "image": record(image),
        "identityManifest": record(identity_manifest),
    }
    normalized = normalize(value)
    return value, normalized, hashlib.sha256(image_contract.BUILD_RESULT_IDENTITY_DOMAIN + normalized).hexdigest()
