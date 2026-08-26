"""CP0 component-specification compiler.

Decodes `contracts/component-spec/v1/components/*.zti`, validates the semantics
`schema.zt` describes but cannot express as a type, and computes each record's
authoritative identity.

Identity is SHA-256 over `identityDomain` followed by the normalized record
bytes: sorted-key, whitespace-free, ASCII-escaped UTF-8 JSON plus one trailing
newline. That is `contracts/interface-schema/v1`'s convention verbatim rather
than a second normalizer, so a component identity and an interface identity
cannot drift apart in how they are computed.

Every vocabulary and bound comes from the generated
`component_spec_contract.py`. Nothing here restates one: a checker holding its
own copy of a closed value set is a second authority on it, which is the shape
that produced B57 and B60.
"""

from __future__ import annotations

import ast
import hashlib
import json
import os
import re
import subprocess
from dataclasses import dataclass
from functools import lru_cache
from pathlib import Path
from types import ModuleType

import component_spec_contract as default_contract
from boot_contracts import COMPONENT_MAX_STACK_BYTES, PRIVATE_MEMORY_ROOT_REGION_PAGES
from harness import CHECK_SCRIPTS, ROOT, load_script
from zutai_cli import STDLIB, binary

CONTRACT_ROOT = ROOT / "contracts" / "component-spec" / "v1"
CHECKER = CONTRACT_ROOT / "check.zt"
SPEC_ROOT = CONTRACT_ROOT / "components"
INTERFACE_SCHEMA_ROOT = ROOT / "contracts" / "interface-schema" / "v1" / "interfaces"
JUSTFILE = ROOT / "Justfile"
COMPONENT_CRATE_ROOT = ROOT / "components" / "bins"

_NAME = re.compile(r"^[a-z][a-z0-9-]*$")
_VERSION = re.compile(r"^\d+\.\d+\.\d+$")
_SHA256 = re.compile(r"^[0-9a-f]{64}$")
_CONTRACT_PATH = re.compile(r"^contracts/[a-z][a-z0-9-]*/v\d+$")
_JUST_TARGET = re.compile(r"^([a-z][a-z0-9_]*)\s*(?::|\s)", re.MULTILINE)
# CP3: a component is its own crate under `components/bins/<name>/`, declaring
# exactly one `[[bin]]`. Discovery is therefore a directory walk plus a
# per-manifest read, not a scan of one shared `[[bin]]` table. The regex still
# matches a single entry rather than trusting the directory name, so a crate
# whose bin name disagrees with its directory is a resolution failure instead of
# an invisible rename.
_BIN_ENTRY = re.compile(
    r"\[\[bin\]\]\s*\nname\s*=\s*\"([^\"]+)\"\s*\npath\s*=\s*\"([^\"]+)\""
)

_SPEC_FIELDS = {
    "formatVersion",
    "name",
    "componentType",
    "version",
    "owner",
    "purpose",
    "implementation",
    "provides",
    "requires",
    "interfaces",
    "dependencies",
    "communication",
    "configuration",
    "lifecycle",
    "runtime",
    "health",
    "compatibility",
    "test",
}


class ComponentSpecError(ValueError):
    pass


@dataclass(frozen=True)
class CompiledSpec:
    name: str
    normalized: bytes
    identity: bytes
    spec: dict


def _fail(message: str) -> None:
    raise ComponentSpecError(message)


def _exact_record(value: object, keys: set[str], label: str) -> dict:
    if not isinstance(value, dict) or set(value) != keys:
        _fail(f"{label}: expected fields {sorted(keys)}")
    return value


def _text(value: object, label: str, contract: ModuleType, *, limit: int | None = None) -> str:
    if not isinstance(value, str):
        _fail(f"{label}: expected text")
    ceiling = contract.MAX_TEXT_BYTES if limit is None else limit
    if len(value.encode("utf-8")) > ceiling:
        _fail(f"{label}: exceeds {ceiling} bytes")
    return value


def _integer(value: object, label: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        _fail(f"{label}: expected integer")
    if value < 0:
        _fail(f"{label}: must not be negative")
    return value


def _list(value: object, label: str, bound: int) -> list:
    if not isinstance(value, list):
        _fail(f"{label}: expected list")
    if len(value) > bound:
        _fail(f"{label}: exceeds {bound} entries")
    return value


def _member(value: object, allowed: tuple[str, ...], label: str) -> str:
    if not isinstance(value, str) or value not in allowed:
        _fail(f"{label}: expected one of {list(allowed)}, got {value!r}")
    return value


def _sorted_unique(values: list[str], label: str) -> list[str]:
    if values != sorted(values):
        _fail(f"{label}: must be sorted")
    if len(set(values)) != len(values):
        _fail(f"{label}: duplicate entry")
    return values


def _run_zutai(path: Path, command: str, *, contract: ModuleType) -> str:
    if not path.is_file():
        _fail(f"component spec not found: {path}")
    if path.stat().st_size > contract.MAX_SOURCE_BYTES:
        _fail(f"{path}: source exceeds bound")
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    environment["SLIME_COMPONENT_SPEC_PATH"] = str(path)
    process = subprocess.run(
        [str(binary()), command, str(CHECKER if command == "run" else path)],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode != 0:
        _fail(f"{path}: malformed Zutai input: {(process.stderr or process.stdout).strip()}")
    return process.stdout


def _load(path: Path, contract: ModuleType) -> dict:
    decoded = _run_zutai(path, "run", contract=contract)
    if not decoded.startswith("#valid"):
        _fail(f"{path}: input does not match the component-spec schema")
    raw = _run_zutai(path, "json", contract=contract)
    try:
        value = json.loads(raw)
    except json.JSONDecodeError as error:
        _fail(f"{path}: invalid Zutai JSON projection: {error}")
    return _exact_record(value, _SPEC_FIELDS, str(path))


def interface_catalogue() -> dict[str, str]:
    """Every declared interface, mapped to its contract kind.

    Read from `contracts/interface-schema/v1/interfaces/*.zti` rather than from a
    list here, so an interface reference resolves against the real corpus and a
    deleted interface breaks the specs that name it.
    """
    import interface_schema

    catalogue: dict[str, str] = {}
    for path in sorted(INTERFACE_SCHEMA_ROOT.glob("*.zti")):
        compiled = interface_schema.compile_interface(path)
        catalogue[compiled.name] = compiled.kind
    return catalogue


@lru_cache(maxsize=1)
def just_targets() -> frozenset[str]:
    """Every recipe name the Justfile declares.

    Cached: this is read once per spec otherwise, and the Justfile does not
    change under a single gate run."""
    return frozenset(_JUST_TARGET.findall(JUSTFILE.read_text(encoding="utf-8")))


@lru_cache(maxsize=1)
def workspace_binaries() -> tuple[tuple[str, str], ...]:
    """Every component binary the workspace builds, as `(name, path)` pairs.

    CP3: each component is its own crate under `components/bins/<name>/`, so
    this walks those directories instead of scanning one shared `[[bin]]` table.
    The pair's path is repository-relative. The single-crate table's was
    crate-relative (`src/bin/console.rs`), so this is a deliberate change, not a
    preserved shape: with 52 crates a bare `src/main.rs` would name 52 different
    files. Neither current consumer reads the path — `check-component-spec.py`
    and `check-component-crate-split.py` both take `dict(...)` keys — so the
    change is inert today and stated here for the caller that starts using it.

    A crate declaring anything other than exactly one `[[bin]]`, or whose bin
    name does not match its directory, is a failure rather than a skip. Both
    would otherwise make a component invisible here while still building: the
    spec corpus would resolve `undeclared` for a component that ships, which is
    the drift class B70 records.
    """
    found: list[tuple[str, str]] = []
    for manifest in sorted(COMPONENT_CRATE_ROOT.glob("*/Cargo.toml")):
        entries = _BIN_ENTRY.findall(manifest.read_text(encoding="utf-8"))
        directory = manifest.parent.name
        if len(entries) != 1:
            raise SystemExit(
                f"component crate {directory!r} declares {len(entries)} [[bin]] entries; "
                "each component crate declares exactly one"
            )
        name, path = entries[0]
        if name != directory:
            raise SystemExit(
                f"component crate {directory!r} declares [[bin]] {name!r}; "
                "the crate directory and its binary must share one name"
            )
        found.append((name, str((manifest.parent / path).relative_to(ROOT))))
    return tuple(found)


@lru_cache(maxsize=None)
def gate_markers(target: str) -> frozenset[str]:
    """Every string literal in every check script a Justfile recipe invokes.

    Parsed rather than grepped. A substring test over raw source would accept any
    Python fragment — `def `, `import`, and `#` appear in every script — so it
    would prove the file exists rather than that the gate looks for the marker.
    Extracting literals through `ast` restricts the comparison to text the script
    actually matches against.

    A recipe body runs until the next unindented line. A gate that only declares
    prerequisites (`sample_descriptor_check: contracts_check sel4_sample_check`)
    invokes no script of its own, so its prerequisites are followed one level.
    """
    justfile = JUSTFILE.read_text(encoding="utf-8")
    match = re.search(
        rf"^{re.escape(target)}:([^\n]*)\n((?:[ \t]+[^\n]*\n|\n)*)", justfile, re.MULTILINE
    )
    if match is None:
        _fail(f"Justfile declares no recipe named {target!r}")
    prerequisites, body = match.group(1).split(), match.group(2)
    scripts = re.findall(r"scripts/check/([\w.-]+\.py)", body)
    if not scripts:
        for prerequisite in prerequisites:
            inner = re.search(
                rf"^{re.escape(prerequisite)}:[^\n]*\n((?:[ \t]+[^\n]*\n|\n)*)",
                justfile,
                re.MULTILINE,
            )
            if inner:
                scripts.extend(re.findall(r"scripts/check/([\w.-]+\.py)", inner.group(1)))
    if not scripts:
        _fail(f"{target}: names no check script, so its markers cannot be verified")
    literals: set[str] = set()
    for script in dict.fromkeys(scripts):
        path = CHECK_SCRIPTS / script
        if not path.is_file():
            _fail(f"{target}: invokes {script}, which does not exist")
        for node in ast.walk(ast.parse(path.read_text(encoding="utf-8"), filename=str(path))):
            if isinstance(node, ast.Constant) and isinstance(node.value, str):
                literals.add(node.value)
    return frozenset(literals)


def _observes(criterion: str, literals: frozenset[str]) -> bool:
    """Whether some literal in the gate matches this marker.

    A gate spells a marker as a regex, so `[init] x` is written `\\[init\\] x`.
    Only metacharacters carry a backslash, so this unescapes the *gate's*
    literals rather than stripping backslashes from both sides: a two-sided
    strip is lossy the wrong way, letting a criterion that genuinely contains a
    backslash match text that never had one.
    """
    if any(criterion in literal for literal in literals):
        return True
    return any(criterion in re.sub(r"\\(.)", r"\1", literal) for literal in literals)


def _interface_reference(
    raw: object, index: int, catalogue: dict[str, str], contract: ModuleType
) -> dict:
    label = f"interfaces[{index}]"
    value = _exact_record(raw, {"name", "tag", "interface"}, label)
    name = _text(value["name"], f"{label}.name", contract, limit=contract.MAX_NAME_BYTES)
    if not _NAME.match(name):
        _fail(f"{label}.name: {name!r} is not a route identifier")
    tag = _member(value["tag"], contract.INTERFACE_TAGS, f"{label}.tag")
    interface = _text(
        value["interface"], f"{label}.interface", contract, limit=contract.MAX_NAME_BYTES
    )
    if interface not in catalogue:
        _fail(f"{label}.interface: {interface!r} resolves to no declared interface schema")
    kind = catalogue[interface]
    # A stream carries data; a call/operation carries requests. Tagging a stream
    # `command` would claim a request/reply shape the interface cannot express,
    # so the tag is checked against the interface's own kind rather than trusted.
    admitted = {
        "stream": (contract.INTERFACE_TAG_INPUT, contract.INTERFACE_TAG_OUTPUT),
        "call": (
            contract.INTERFACE_TAG_COMMAND,
            contract.INTERFACE_TAG_EVENT,
            contract.INTERFACE_TAG_INPUT,
            contract.INTERFACE_TAG_OUTPUT,
        ),
        "operation": (
            contract.INTERFACE_TAG_COMMAND,
            contract.INTERFACE_TAG_EVENT,
            contract.INTERFACE_TAG_INPUT,
            contract.INTERFACE_TAG_OUTPUT,
        ),
    }[kind]
    if tag not in admitted:
        _fail(f"{label}: a {kind} interface admits tags {list(admitted)}, not {tag!r}")
    return {"name": name, "tag": tag, "interface": interface}


def _qos_policy(raw: object, index: int, references: set[str], contract: ModuleType) -> dict:
    label = f"communication.qos[{index}]"
    keys = {
        "reference",
        "reliability",
        "durability",
        "liveliness",
        "historyDepth",
        "retainedDepth",
        "deadlineNs",
        "lifespanNs",
        "leaseNs",
    }
    value = _exact_record(raw, keys, label)
    reference = _text(
        value["reference"], f"{label}.reference", contract, limit=contract.MAX_NAME_BYTES
    )
    if reference not in references:
        _fail(f"{label}.reference: {reference!r} names no declared interface entry")
    # The builder's own QoS maps, not a fourth spelling of them. These are the
    # tables `build-generation.py` admits a `FabricParticipant` against, so a
    # spec's policy is checked against exactly the vocabulary the graph's is.
    reliability = _member(value["reliability"], _RELIABILITY, f"{label}.reliability")
    durability = _member(value["durability"], _DURABILITY, f"{label}.durability")
    liveliness = _member(value["liveliness"], _LIVELINESS, f"{label}.liveliness")
    depths = {key: _integer(value[key], f"{label}.{key}") for key in keys - {
        "reference",
        "reliability",
        "durability",
        "liveliness",
    }}
    # The same two agreement rules `build-generation.py::validate_qos` enforces on
    # a graph participant. A spec's policy and a graph's policy are the same
    # vocabulary, so they are the same rules: declaring `retained` with no
    # retained depth, or `manual` liveliness with no lease, is contradictory in
    # either place.
    if (durability == "retained") == (depths["retainedDepth"] == 0):
        _fail(f"{label}: durability and retained depth disagree")
    if (liveliness == "manual") == (depths["leaseNs"] == 0):
        _fail(f"{label}: liveliness and lease disagree")
    if depths["deadlineNs"] and depths["lifespanNs"] and depths["lifespanNs"] < depths["deadlineNs"]:
        _fail(f"{label}: lifespan expires every sample before its deadline")
    return {
        "reference": reference,
        "reliability": reliability,
        "durability": durability,
        "liveliness": liveliness,
        **depths,
    }


def _lifecycle(
    raw: object, spec_type: str, has_parameters: bool, qos: list[dict], stops: bool,
    contract: ModuleType,
) -> list[str]:
    states = _list(raw, "lifecycle", contract.MAX_LIFECYCLE_STATES)
    for index, state in enumerate(states):
        _member(state, contract.LIFECYCLE_STATES, f"lifecycle[{index}]")
    if len(set(states)) != len(states):
        _fail("lifecycle: duplicate state")
    order = {state: index for index, state in enumerate(contract.LIFECYCLE_STATES)}
    if [order[state] for state in states] != sorted(order[state] for state in states):
        _fail("lifecycle: states must appear in canonical order")
    missing = [state for state in contract.LIFECYCLE_REQUIRED if state not in states]
    if missing:
        _fail(f"lifecycle: missing required state(s) {missing}")
    # Each conditional state is tied to a fact the spec already declares, so a
    # lifecycle cannot claim a phase the component has nothing to perform in, nor
    # omit one it demonstrably needs.
    declared = set(states)
    if has_parameters != (contract.LIFECYCLE_CONFIGURE in declared):
        _fail("lifecycle: Configure is declared exactly when configuration parameters are")
    serves = spec_type in (contract.COMPONENT_TYPE_SERVICE, contract.COMPONENT_TYPE_INIT)
    if serves != (contract.LIFECYCLE_READY in declared):
        _fail("lifecycle: Ready is declared exactly by an init or service component")
    degradable = any(
        policy["deadlineNs"] or policy["lifespanNs"] or policy["leaseNs"] for policy in qos
    )
    if degradable != (contract.LIFECYCLE_DEGRADED in declared):
        _fail("lifecycle: Degraded is declared exactly when a QoS policy can expire")
    if stops != (contract.LIFECYCLE_STOP in declared):
        _fail("lifecycle: Stop is declared exactly when supervision authority is declared")
    return states


def _normalize(raw: dict, catalogue: dict[str, str], contract: ModuleType) -> dict:
    version = _integer(raw["formatVersion"], "formatVersion")
    if version != contract.FORMAT_VERSION:
        _fail(f"unsupported component spec version {version}")
    name = _text(raw["name"], "name", contract, limit=contract.MAX_NAME_BYTES)
    if not _NAME.match(name):
        _fail(f"name: {name!r} is not a component identifier")
    component_type = _member(raw["componentType"], contract.COMPONENT_TYPES, "componentType")
    spec_version = _text(raw["version"], "version", contract, limit=contract.MAX_NAME_BYTES)
    if not _VERSION.match(spec_version):
        _fail(f"version: {spec_version!r} is not a three-part version")
    owner = _text(raw["owner"], "owner", contract, limit=contract.MAX_NAME_BYTES)
    if not owner:
        _fail("owner: must be declared")
    purpose = _text(raw["purpose"], "purpose", contract, limit=contract.MAX_PURPOSE_BYTES)
    if not purpose:
        _fail("purpose: must be declared")

    implementation = _exact_record(
        raw["implementation"], {"provider", "binary", "contentHash"}, "implementation"
    )
    provider = _member(implementation["provider"], contract.PROVIDERS, "implementation.provider")
    binary_name = _text(
        implementation["binary"], "implementation.binary", contract, limit=contract.MAX_NAME_BYTES
    )
    content_hash = _text(
        implementation["contentHash"],
        "implementation.contentHash",
        contract,
        limit=contract.MAX_CONTENT_HASH_BYTES,
    )
    binaries = dict(workspace_binaries())
    if provider == contract.PROVIDER_UNDECLARED:
        if binary_name or content_hash:
            _fail("implementation: an undeclared provider names no binary or content hash")
        # An undeclared provider is a recorded gap, so it must be a real one: a
        # component whose binary does exist must not claim to be missing.
        for candidate in (name, f"sel4-{name}"):
            if candidate in binaries:
                _fail(
                    f"implementation: declared undeclared, but [[bin]] {candidate!r} "
                    "exists; record the implementation instead"
                )
    elif not binary_name:
        _fail(f"implementation: provider {provider!r} must name its binary")
    elif provider == contract.PROVIDER_WORKSPACE:
        if binary_name not in binaries:
            _fail(f"implementation.binary: {binary_name!r} is no [[bin]] target")
        if content_hash:
            _fail("implementation: a workspace provider must not pin external content")
    elif not _SHA256.fullmatch(content_hash):
        _fail("implementation.contentHash: external providers require lowercase SHA-256")

    provides = _sorted_unique(
        [
            _member(value, _CAPABILITY_KINDS, f"provides[{index}]")
            for index, value in enumerate(_list(raw["provides"], "provides", contract.MAX_CAPABILITY_KINDS))
        ],
        "provides",
    )
    requires = _sorted_unique(
        [
            _member(value, _CAPABILITY_KINDS, f"requires[{index}]")
            for index, value in enumerate(_list(raw["requires"], "requires", contract.MAX_CAPABILITY_KINDS))
        ],
        "requires",
    )

    interfaces = [
        _interface_reference(value, index, catalogue, contract)
        for index, value in enumerate(_list(raw["interfaces"], "interfaces", contract.MAX_INTERFACES))
    ]
    keys = [(entry["name"], entry["tag"]) for entry in interfaces]
    if len(set(keys)) != len(keys):
        _fail("interfaces: duplicate (name, tag) entry")
    if keys != sorted(keys):
        _fail("interfaces: entries must be sorted by name then tag")
    # One route carries one interface. Two entries naming the same route with
    # different interfaces would make the route's type ambiguous.
    by_route: dict[str, str] = {}
    for entry in interfaces:
        previous = by_route.setdefault(entry["name"], entry["interface"])
        if previous != entry["interface"]:
            _fail(f"interfaces: route {entry['name']!r} names two interfaces")

    dependencies = _list(raw["dependencies"], "dependencies", contract.MAX_DEPENDENCIES)
    dependency_names = [
        _text(value, f"dependencies[{index}]", contract, limit=contract.MAX_NAME_BYTES)
        for index, value in enumerate(dependencies)
    ]
    if len(set(dependency_names)) != len(dependency_names):
        _fail("dependencies: duplicate entry")
    if name in dependency_names:
        _fail("dependencies: a component cannot depend on itself")

    communication = _exact_record(raw["communication"], {"semantic", "qos"}, "communication")
    references = {entry["name"] for entry in interfaces}
    qos = [
        _qos_policy(value, index, references, contract)
        for index, value in enumerate(
            _list(communication["qos"], "communication.qos", contract.MAX_QOS_POLICIES)
        )
    ]
    qos_references = [policy["reference"] for policy in qos]
    if len(set(qos_references)) != len(qos_references):
        _fail("communication.qos: duplicate reference")
    if qos_references != sorted(qos_references):
        _fail("communication.qos: policies must be sorted by reference")
    semantic = _member(communication["semantic"], contract.SEMANTICS, "communication.semantic")
    kinds = {catalogue[entry["interface"]] for entry in interfaces}
    if not kinds:
        expected = contract.SEMANTIC_NONE
    elif len(kinds) == 1:
        expected = next(iter(kinds))
    else:
        expected = contract.SEMANTIC_MIXED
    if semantic != expected:
        _fail(
            f"communication.semantic: {semantic!r} does not match the referenced "
            f"interface kinds, which require {expected!r}"
        )

    parameters = []
    for index, value in enumerate(
        _list(raw["configuration"], "configuration", contract.MAX_PARAMETERS)
    ):
        label = f"configuration[{index}]"
        entry = _exact_record(value, {"name", "default", "minimum", "maximum"}, label)
        parameter_name = _text(
            entry["name"], f"{label}.name", contract, limit=contract.MAX_NAME_BYTES
        )
        minimum = _integer(entry["minimum"], f"{label}.minimum")
        maximum = _integer(entry["maximum"], f"{label}.maximum")
        default = _integer(entry["default"], f"{label}.default")
        if minimum > maximum:
            _fail(f"{label}: minimum exceeds maximum")
        if not minimum <= default <= maximum:
            _fail(f"{label}: default {default} falls outside [{minimum}, {maximum}]")
        parameters.append(
            {
                "name": parameter_name,
                "default": default,
                "minimum": minimum,
                "maximum": maximum,
            }
        )
    parameter_names = [entry["name"] for entry in parameters]
    if len(set(parameter_names)) != len(parameter_names):
        _fail("configuration: duplicate parameter name")

    runtime = _exact_record(
        raw["runtime"], {"executionEnvironment", "resource", "devices"}, "runtime"
    )
    environment = _text(
        runtime["executionEnvironment"],
        "runtime.executionEnvironment",
        contract,
        limit=contract.MAX_NAME_BYTES,
    )
    resource_keys = {
        "stackBytes",
        "spawnBudget",
        "extraThreads",
        "bufferBytePages",
        "bufferCount",
        "mappingCount",
        "loanCount",
        "privatePageQuota",
    }
    resource_raw = _exact_record(runtime["resource"], resource_keys, "runtime.resource")
    resource = {
        key: _integer(resource_raw[key], f"runtime.resource.{key}") for key in sorted(resource_keys)
    }
    if resource["stackBytes"] <= 0:
        _fail("runtime.resource.stackBytes: must be positive")
    if resource["stackBytes"] % 4096:
        _fail("runtime.resource.stackBytes: must be a whole number of pages")
    if resource["stackBytes"] > _MAX_STACK_BYTES:
        _fail(f"runtime.resource.stackBytes: exceeds {_MAX_STACK_BYTES}")
    if resource["spawnBudget"] > _MAX_SPAWN_BUDGET:
        _fail(f"runtime.resource.spawnBudget: exceeds {_MAX_SPAWN_BUDGET}")
    if resource["extraThreads"] > _MAX_EXTRA_THREADS:
        _fail(f"runtime.resource.extraThreads: exceeds {_MAX_EXTRA_THREADS}")
    # A holder that creates buffers must be able to hold the pages they occupy,
    # and one granted no pages cannot create any. Mapping without creating is
    # ordinary: `fabric-subscriber` maps loaned pages it never allocated, which
    # is exactly what `sharedBufferBudget` grants it in
    # `contracts/generation/v1/fixtures/valid.zti`. So the rule is between pages
    # and buffers, not between mappings and buffers.
    if resource["bufferCount"] and not resource["bufferBytePages"]:
        _fail("runtime.resource: buffers declared with no page allowance")
    if resource["bufferBytePages"] > _MAX_TOTAL_PAGES:
        _fail(f"runtime.resource.bufferBytePages: exceeds {_MAX_TOTAL_PAGES}")
    # C10.4: the private-memory ceiling is bounded by the root's own per-task
    # reservation, which is a published contract constant rather than this
    # spec's opinion. A spec declaring more describes a region no root will
    # grant, and the refusal belongs here rather than at boot.
    if resource["privatePageQuota"] > _MAX_PRIVATE_REGION_PAGES:
        _fail(f"runtime.resource.privatePageQuota: exceeds {_MAX_PRIVATE_REGION_PAGES}")
    # A parameter is only a parameter of something. Every name a spec declares
    # must be a `runtime.resource` field, and its default must be the value that
    # field actually holds — otherwise `configuration` is decoration: a record
    # could declare `spawnBudget` defaulting to 3 beside a resource requirement
    # of 18 and nothing would object.
    for entry in parameters:
        if entry["name"] not in resource:
            _fail(
                f"configuration[{entry['name']}]: names no runtime.resource field, so "
                "nothing configures it"
            )
        if entry["default"] != resource[entry["name"]]:
            _fail(
                f"configuration[{entry['name']}]: default {entry['default']} disagrees "
                f"with runtime.resource.{entry['name']} = {resource[entry['name']]}"
            )
    devices = _sorted_unique(
        [
            _member(value, _DEVICE_KINDS, f"runtime.devices[{index}]")
            for index, value in enumerate(
                _list(runtime["devices"], "runtime.devices", contract.MAX_DEVICE_REQUIREMENTS)
            )
        ],
        "runtime.devices",
    )
    # A device requirement is authority, so it must appear in the capability sets
    # too. Otherwise a spec could name a device it never declares needing.
    for device in devices:
        if device not in set(provides) | set(requires):
            _fail(f"runtime.devices: {device!r} appears in neither provides nor requires")

    health = _member(raw["health"], contract.HEALTH_POLICIES, "health")

    compatibility_keys = {"platform", "interface", "dependency", "resource", "runtime", "qos"}
    compatibility_raw = _exact_record(raw["compatibility"], compatibility_keys, "compatibility")
    platform = _text(
        compatibility_raw["platform"], "compatibility.platform", contract,
        limit=contract.MAX_NAME_BYTES,
    )
    if platform != environment:
        _fail("compatibility.platform must equal runtime.executionEnvironment")
    interface_contract = _text(
        compatibility_raw["interface"], "compatibility.interface", contract
    )
    # A versioned contract directory under `contracts/`, not any directory that
    # happens to exist: `/tmp` and `scripts` are directories too, and admitting
    # them would make this field decorative. The shape is the one every contract
    # in this repository has.
    if not _CONTRACT_PATH.match(interface_contract):
        _fail(
            f"compatibility.interface: {interface_contract!r} is not a "
            "`contracts/<name>/v<N>` path"
        )
    resolved_contract = ROOT / interface_contract
    if not resolved_contract.is_dir() or not (resolved_contract / "schema.zt").is_file():
        _fail(f"compatibility.interface: {interface_contract!r} declares no schema.zt")
    compatibility = {
        "platform": platform,
        "interface": interface_contract,
        **{
            key: _member(compatibility_raw[key], contract.CONSTRAINTS, f"compatibility.{key}")
            for key in ("dependency", "resource", "runtime", "qos")
        },
    }
    # Each mode is tied to a fact the record already states, so none is free
    # choice. A component with dependencies must match them exactly (a
    # dependency is a named component, and "at most" a component is meaningless);
    # one with none declares `none`. Resource and runtime requirements are
    # ceilings the platform must meet or exceed, so they are `atMost` whenever
    # the component asks for anything beyond the defaults, and `exact` for the
    # target profile it names, which admission compares by equality.
    expected_dependency = (
        contract.CONSTRAINT_EXACT if dependency_names else contract.CONSTRAINT_NONE
    )
    if compatibility["dependency"] != expected_dependency:
        _fail(
            f"compatibility.dependency must be {expected_dependency!r} for a component "
            f"with {len(dependency_names)} dependencies"
        )
    if compatibility["resource"] != contract.CONSTRAINT_AT_MOST:
        _fail("compatibility.resource must be 'atMost': a resource requirement is a ceiling")
    if compatibility["runtime"] != contract.CONSTRAINT_EXACT:
        _fail(
            "compatibility.runtime must be 'exact': executable admission compares "
            "target profiles by equality, not containment"
        )
    # A component with no policy cannot be QoS-constrained, and one with policy
    # must be: an unconstrained policy is policy nothing enforces.
    if bool(qos) != (compatibility["qos"] != contract.CONSTRAINT_NONE):
        _fail("compatibility.qos is constrained exactly when a QoS policy is declared")

    test_keys = {
        "testCondition",
        "expectedResult",
        "passFailCriteria",
        "requiredTestEnvironment",
    }
    test_raw = _exact_record(raw["test"], test_keys, "test")
    test = {key: _text(test_raw[key], f"test.{key}", contract) for key in sorted(test_keys)}
    for key in sorted(test_keys):
        if not test[key]:
            _fail(f"test.{key}: must be declared")
    # A gate that does not exist is a verification claim the repository cannot
    # honour, on the same terms `just devlog_check` enforces for a devlog's
    # `Gates` front matter. Checked here rather than only at corpus level so a
    # single malformed record is refused on its own.
    if test["requiredTestEnvironment"] not in just_targets():
        _fail(
            f"test.requiredTestEnvironment: {test['requiredTestEnvironment']!r} "
            "is no Justfile target"
        )
    # And the criterion must be a literal that gate actually looks for. A valid
    # target paired with a marker nothing matches is the same unverifiable claim
    # wearing a valid name.
    if not _observes(test["passFailCriteria"], gate_markers(test["requiredTestEnvironment"])):
        _fail(
            f"test.passFailCriteria: {test['passFailCriteria']!r} matches no string "
            f"literal in {test['requiredTestEnvironment']}'s check script"
        )

    stops = "supervision" in set(provides) | set(requires)
    lifecycle = _lifecycle(
        raw["lifecycle"], component_type, bool(parameters), qos, stops, contract
    )

    return {
        "formatVersion": version,
        "name": name,
        "componentType": component_type,
        "version": spec_version,
        "owner": owner,
        "purpose": purpose,
        "implementation": {
            "provider": provider,
            "binary": binary_name,
            "contentHash": content_hash,
        },
        "provides": provides,
        "requires": requires,
        "interfaces": interfaces,
        "dependencies": dependency_names,
        "communication": {"semantic": semantic, "qos": qos},
        "configuration": parameters,
        "lifecycle": lifecycle,
        "runtime": {
            "executionEnvironment": environment,
            "resource": resource,
            "devices": devices,
        },
        "health": health,
        "compatibility": compatibility,
        "test": test,
    }


_builder = load_script("component_spec_generation_builder", "build/build-generation.py")

# The declared capability-kind vocabulary, read from the generation builder's own
# `CAPABILITY_KIND` table rather than restated. That table is what admits a
# manifest's `capabilityKind`, so a spec and a manifest agree on the vocabulary by
# construction instead of by inspection — the property B57/B59/B60 were opened to
# restore, and the one this module's docstring claims.
_CAPABILITY_KINDS = tuple(sorted(_builder.CAPABILITY_KIND))
# Which of those kinds are device authority rather than component-to-component
# authority. Derived, not listed: `boot-contracts/src/generation.rs`'s
# `service_for_capability` routes exactly `Block` to `SERVICE_BLOCK` and `Input`
# to `SERVICE_INPUT`, the two services that front a platform device; every other
# kind is either unmediated (endpoint, executable) or a root-owned memory object.
_DEVICE_KINDS = tuple(
    kind
    for kind in _CAPABILITY_KINDS
    if _builder.SERVICE_BY_CAPABILITY_KIND.get(kind)
    in (_builder.SERVICE_BLOCK, _builder.SERVICE_INPUT)
)
# Resource ceilings, imported from the constants that already enforce them rather
# than retyped: `COMPONENT_MAX_STACK_BYTES` from the generated
# `scripts/lib/boot_contracts.py`, and `MAX_SPAWN_BUDGET` from
# `scripts/build/build-generation.py`, which is the module that refuses an
# over-budget manifest.
_MAX_STACK_BYTES = COMPONENT_MAX_STACK_BYTES
_MAX_SPAWN_BUDGET = _builder.MAX_SPAWN_BUDGET
# `slime-root/src/child_vspace.rs` sets `MAX_CHILD_THREADS = 2`: one main thread
# plus at most one extra, which is what `extraThreads` counts.
_MAX_EXTRA_THREADS = 1
# `slime-root/src/shared_buffer.rs`'s `MAX_TOTAL_PAGES = 256` is the *system-wide*
# live page ceiling, not a per-holder one. It is used here only as the upper bound
# no single holder's declared allowance may exceed, since a holder granted more
# than the whole system has could never be satisfied. The real per-holder ceiling
# is the manifest's own `sharedBufferBudget`, which the gate compares against
# field by field.
_MAX_TOTAL_PAGES = 256
# C10.4's per-task private-memory reservation, published by
# `contracts/private-memory-budget/v1` and pinned against `slime-root`'s own
# `MAX_REGION_PAGES` by a compile-time assert there. The *per-holder* ceiling is
# the manifest's `privateMemoryBudget`; this is the structural bound no holder's
# declaration may exceed, because the window's address space is sized for it
# when the child VSpace is built and a growth past it is refused rather than
# relocated.
_MAX_PRIVATE_REGION_PAGES = PRIVATE_MEMORY_ROOT_REGION_PAGES
# The QoS value sets, read from the builder's `FABRIC_RELIABILITY`,
# `FABRIC_DURABILITY`, and `FABRIC_LIVELINESS` maps. Those are the tables a
# manifest's `FabricParticipant` is admitted against, so consuming them is what
# makes "one QoS vocabulary" true rather than merely intended.
_RELIABILITY = tuple(_builder.FABRIC_RELIABILITY)
_DURABILITY = tuple(_builder.FABRIC_DURABILITY)
_LIVELINESS = tuple(_builder.FABRIC_LIVELINESS)


def compile_spec(path: Path, catalogue: dict[str, str], contract: ModuleType = default_contract) -> CompiledSpec:
    spec = _normalize(_load(path.resolve(), contract), catalogue, contract)
    if spec["name"] != path.stem:
        _fail(f"{path}: declares component {spec['name']!r}, so its file name must match")
    normalized = (
        json.dumps(spec, sort_keys=True, separators=(",", ":"), ensure_ascii=True) + "\n"
    ).encode("utf-8")
    if len(normalized) > contract.MAX_NORMALIZED_BYTES:
        _fail(f"{path}: normalized spec exceeds bound")
    identity = hashlib.sha256(contract.IDENTITY_DOMAIN + normalized).digest()
    return CompiledSpec(
        name=spec["name"], normalized=normalized, identity=identity, spec=spec
    )


def spec_paths(root: Path = SPEC_ROOT) -> list[Path]:
    return sorted(root.glob("*.zti"))


def admit_specs(
    paths: list[Path] | None = None,
    *,
    catalogue: dict[str, str] | None = None,
    contract: ModuleType = default_contract,
) -> list[CompiledSpec]:
    """Compile a whole corpus, enforcing the rules that span more than one spec."""
    resolved = spec_paths() if paths is None else paths
    if len(resolved) > contract.MAX_SPECS:
        _fail("component spec count exceeds bound")
    if not resolved:
        _fail("component spec corpus is empty")
    table = interface_catalogue() if catalogue is None else catalogue
    compiled = [compile_spec(path, table, contract) for path in resolved]
    names = [entry.name for entry in compiled]
    if len(set(names)) != len(names):
        _fail("duplicate component spec name")
    identities = {entry.identity for entry in compiled}
    if len(identities) != len(compiled):
        _fail("two component specs computed the same identity")
    implemented = [
        entry.spec["implementation"]["binary"]
        for entry in compiled
        if entry.spec["implementation"]["provider"] != contract.PROVIDER_UNDECLARED
    ]
    if len(set(implemented)) != len(implemented):
        _fail("two component specs resolve to the same implementation binary")
    cross_domain = sorted(set(names) & set(implemented))
    for value in cross_domain:
        owner = next(entry.name for entry in compiled if entry.spec["implementation"]["binary"] == value)
        if owner != value:
            _fail(
                f"component spec name {value!r} collides with {owner!r}'s implementation binary"
            )
    declared = set(names)
    for entry in compiled:
        unknown = [value for value in entry.spec["dependencies"] if value not in declared]
        if unknown:
            _fail(f"{entry.name}: depends on undeclared component spec(s) {unknown}")
    # A dependency cycle would make no launch order satisfiable, which is a fact
    # about the corpus rather than about any one spec.
    edges = {entry.name: tuple(entry.spec["dependencies"]) for entry in compiled}
    visiting: set[str] = set()
    settled: set[str] = set()

    def visit(node: str, trail: tuple[str, ...]) -> None:
        if node in settled:
            return
        if node in visiting:
            _fail(f"dependency cycle: {' -> '.join(trail + (node,))}")
        visiting.add(node)
        for child in edges[node]:
            visit(child, trail + (node,))
        visiting.discard(node)
        settled.add(node)

    for name in sorted(edges):
        visit(name, ())
    # Every requirement must be provided by some component in the corpus, except
    # `executable`, whose provider is the generation module rather than a
    # component: executable bytes are hash-verified at boot and no component
    # mints them.
    provided = {kind for entry in compiled for kind in entry.spec["provides"]}
    required = {kind for entry in compiled for kind in entry.spec["requires"]}
    unmet = sorted(required - provided - {"executable"})
    if unmet:
        _fail(f"corpus requires capability kind(s) no component provides: {unmet}")
    return compiled
