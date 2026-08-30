"""CP1 system-specification compiler and generation derivation.

Decodes `contracts/system-spec/v1/systems/*.zti`, validates it against the
component specs it references, and derives a `contracts/generation-manifest/v1`
`GenerationManifest` from the pair.

The derivation is the point of the milestone, so what it derives rather than
copies is worth stating plainly. Derived:

  - `executables`  — one per referenced component spec: `role` from
    `componentType`, `spawnBudget` and `stackBytes` from `runtime.resource`,
    `commandProfile` from the system's command bindings.
  - `instances`    — `executable`, `health`, and `dependencies` from the
    component spec; `owner` and the thread/priority fields from the system's
    placement; and every `bindings` entry from the grant table, since a grant
    materializes in whichever instance holds it.
  - `objects`      — one per component plus the resource objects the presence of
    a budget or a fabric graph implies.
  - `sharedBufferBudget` — from each component spec's `runtime.resource`.
  - `health.requiredInstances` — the components whose spec says `required`.

Declared, because no component spec can know it: the grant table (authority
between components is a property of the composition), the notification objects,
the fabric graph, the boot profiles, and the persisted state bindings.

Slot numbers are ordinarily assigned by `build-generation.py`'s own
`assign_declared_slots`, which is a function of the manifest alone. A system may
pin individual slots, and does for `valid.zti`, whose numbers
`contracts/boot-layout/v1/fixtures/*.layout` freeze byte for byte.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType

import system_spec_contract as default_contract
from component_spec import CompiledSpec, admit_specs, interface_catalogue
from harness import GENERATION_COMPOSITIONS, GENERATION_FIXTURES, ROOT, load_script
from zutai_cli import STDLIB, binary

CONTRACT_ROOT = ROOT / "contracts" / "system-spec" / "v1"
CHECKER = CONTRACT_ROOT / "check.zt"
SYSTEM_ROOT = CONTRACT_ROOT / "systems"
INTERFACE_SCHEMA_ROOT = ROOT / "contracts" / "interface-schema" / "v1" / "interfaces"

# Which committed `contracts/generation-manifest/v1` fixture each system spec derives.
#
# CP1 converts the reference manifest and the smallest seL4 manifest; converting
# the remaining `sel4-*.zti` fixtures is deferred follow-on work. An explicit
# table rather than a glob, so "which fixtures are generated" is a stated fact
# and a system spec that derives nothing is a gate failure rather than a silent
# no-op. Both the gate and the generator read it from here.
DERIVED_GENERATION_FIXTURES = {
    "reference": "valid.zti",
    "sel4-channel": "sel4-channel.zti",
}

def derived_manifest_path(fixture: str) -> Path:
    """Where a derived manifest is committed.

    The two schema-conformance fixtures live in `fixtures/`; every plane
    composition lives in `compositions/`. The derivation table names bare
    filenames because `check-system-spec.py` also uses them to find the frozen
    `contracts/system-spec/v1/baselines/` copy, which is a flat directory.
    """
    root = GENERATION_FIXTURES if fixture in {"valid.zti", "invalid.zti"} else GENERATION_COMPOSITIONS
    return root / fixture

_NAME = re.compile(r"^[a-z][a-z0-9-]*$")

_builder = load_script("system_spec_generation_builder", "build/build-generation.py")
# The target-profile table and the source `formatVersion` both reach us through
# the builder, which already imports the generated `target_profile` bindings and
# is the module that refuses an unknown target or an unsupported source format.
_TARGET_PROFILES = _builder.TARGET_PROFILES_BY_NAME
# `contracts/generation-manifest/v1/schema.zt` is format 1 and `build-generation.py`
# refuses anything else at load time; a system spec produces that format or it
# produces something the builder will not read.
_GENERATION_SOURCE_FORMAT = 1

_SPEC_FIELDS = {
    "formatVersion",
    "name",
    "generation",
    "targetRequirement",
    "bootAction",
    "bootstrapInstance",
    "bootAttempts",
    "components",
    "placements",
    "grants",
    "slotPins",
    "commandBindings",
    "defaultImageBytes",
    "imageSizes",
    "sharedBufferBudgetObject",
    "interfaceSchemas",
    "state",
    "notifications",
    "notificationBindings",
    "bootProfiles",
    "fabricGraph",
    "deploymentConstraint",
    "acceptanceCriteria",
}


class SystemSpecError(ValueError):
    pass


@dataclass(frozen=True)
class CompiledSystem:
    name: str
    spec: dict
    components: dict[str, dict]
    normalized: bytes
    identity: bytes


def _fail(message: str) -> None:
    raise SystemSpecError(message)


def _run_zutai(path: Path, command: str, *, contract: ModuleType) -> str:
    if not path.is_file():
        _fail(f"system spec not found: {path}")
    if path.stat().st_size > contract.MAX_SOURCE_BYTES:
        _fail(f"{path}: source exceeds bound")
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    environment["SLIME_SYSTEM_SPEC_PATH"] = str(path)
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
        _fail(f"{path}: input does not match the system-spec schema")
    raw = _run_zutai(path, "json", contract=contract)
    try:
        value = json.loads(raw)
    except json.JSONDecodeError as error:
        _fail(f"{path}: invalid Zutai JSON projection: {error}")
    if not isinstance(value, dict):
        _fail(f"{path}: expected a record")
    # `fabricGraph` is optional, so its absence is a legitimate shape.
    unexpected = set(value) - _SPEC_FIELDS
    missing = _SPEC_FIELDS - set(value) - {"fabricGraph"}
    if unexpected or missing:
        _fail(f"{path}: unexpected {sorted(unexpected)}, missing {sorted(missing)}")
    return value


def _validate(spec: dict, components: dict[str, dict], contract: ModuleType) -> None:
    """Everything the Zutai type cannot express, checked before anything derives."""
    if spec["formatVersion"] != contract.FORMAT_VERSION:
        _fail(f"unsupported system spec version {spec['formatVersion']}")
    if not _NAME.match(spec["name"]):
        _fail(f"name: {spec['name']!r} is not a system identifier")
    if spec["generation"] <= 0:
        _fail("generation: must be positive")
    if spec["bootAttempts"] <= 0:
        _fail("bootAttempts: must be positive")

    profile = _TARGET_PROFILES.get(spec["targetRequirement"])
    if profile is None:
        _fail(
            f"targetRequirement: {spec['targetRequirement']!r} names no profile in "
            "contracts/target-profile/v1"
        )

    declared = spec["components"]
    if len(declared) > contract.MAX_COMPONENTS:
        _fail("components: exceeds bound")
    if len(set(declared)) != len(declared):
        _fail("components: duplicate entry")
    if declared != sorted(declared):
        _fail("components: must be sorted")
    unknown = [name for name in declared if name not in components]
    if unknown:
        _fail(f"components: no component spec declares {unknown}")

    admitted = set(declared)
    # A component's `runtime.executionEnvironment` is *not* required to equal
    # this system's target, and the attempt to require it is what disproved the
    # assumption: `console`, `init`, and the rest compose into both
    # `valid.zti` (x86_64-qemu-virtio) and `sel4-channel.zti`
    # (aarch64-sel4-qemu-virt) from the same sources. A component spec's
    # environment records the reference generation it was authored against, not
    # an exclusive claim, and CP0's corpus is authored against `valid.zti`.
    #
    # What *is* enforced is that the target exists in the profile table, checked
    # above. Per-image target qualification belongs to immutable selector/root
    # admission and is already gated: `contracts/component/v2`'s header carries
    # architecture, ABI, page profile, and required features, and admission
    # compares them by equality before mapping executable bytes. Restating a
    # weaker version here would be a second authority on it, and a wrong one.

    if spec["bootstrapInstance"] not in admitted:
        _fail(f"bootstrapInstance: {spec['bootstrapInstance']!r} is not an admitted component")
    if components[spec["bootstrapInstance"]]["componentType"] != "init":
        _fail(f"bootstrapInstance: {spec['bootstrapInstance']!r} is not an init component")

    # A dependency must be admitted too, or the derived instance graph names an
    # instance the generation does not contain.
    for name in declared:
        for dependency in components[name]["dependencies"]:
            if dependency not in admitted:
                _fail(f"{name}: depends on {dependency!r}, which this system does not admit")

    placements = spec["placements"]
    seen_placements = [entry["component"] for entry in placements]
    if len(set(seen_placements)) != len(seen_placements):
        _fail("placements: duplicate component")
    for entry in placements:
        if entry["component"] not in admitted:
            _fail(f"placements: {entry['component']!r} is not an admitted component")

    grant_names: set[str] = set()
    for grant in spec["grants"]:
        if grant["name"] in grant_names:
            _fail(f"grants: duplicate grant {grant['name']!r}")
        grant_names.add(grant["name"])
        if grant["capabilityKind"] not in _builder.CAPABILITY_KIND:
            _fail(f"grants: {grant['name']}: unknown capability kind {grant['capabilityKind']!r}")
        if grant["source"] not in admitted:
            _fail(f"grants: {grant['name']}: source {grant['source']!r} is not admitted")
        if grant["target"] not in admitted:
            _fail(f"grants: {grant['name']}: target {grant['target']!r} is not admitted")
        # The rights vocabulary and the per-kind mask are the builder's, checked
        # here so a malformed grant is refused before a manifest exists rather
        # than after one is written.
        rights = 0
        for right in grant["rights"]:
            if right not in _builder.RIGHT:
                _fail(f"grants: {grant['name']}: unknown right {right!r}")
            rights |= _builder.RIGHT[right]
        if grant["transferable"]:
            rights |= _builder.RIGHT_TRANSFER
        # The builder raises `SystemExit` on refusal, which would escape a caller
        # catching `SystemSpecError` and abort the process instead of reporting a
        # rejected spec. Reusing its rule is the point; adopting its exit
        # behaviour is not.
        try:
            _builder.validate_capability_rights(
                grant["name"], grant["capabilityKind"], rights
            )
        except SystemExit as error:
            _fail(f"grants: {grant['name']}: {error}")
    if len(grant_names) > contract.MAX_GRANTS:
        _fail("grants: exceeds bound")

    # No capability agreement between a system's grants and a component spec's
    # `provides`/`requires` is enforced here, and that is a finding rather than
    # an omission.
    #
    # Three progressively weaker rules were tried and each was disproved by the
    # two real fixtures:
    #
    #   - per-role equality: `console` *provides* an endpoint under `valid.zti`,
    #     where it owns the edge to `dango`, and *receives* one under the channel
    #     plane, where `init` owns it. Same component, opposite role.
    #   - per-role containment: same counterexample, since the role moved rather
    #     than narrowed.
    #   - kind containment: `init` holds an `endpoint` kind under the channel
    #     plane and none at all under `valid.zti`.
    #
    # The root cause is that CP0's corpus is authored against exactly one
    # generation. A component spec's capability sets record what `valid.zti`
    # grants that component, not what the component supports across every
    # composition, so any cross-system claim built on them is false today. The
    # honest position is to enforce nothing here and say why.
    #
    # `just component_spec_check` keeps the exact per-role match against
    # `valid.zti` itself, where it is true and where it caught a real defect.
    # Making the sets composition-independent — so a system could be checked
    # against them — needs the corpus to describe components rather than one
    # generation's use of them, which is CP3/CP5 work.

    bindings = derive_bindings(spec["grants"], admitted)
    for pin in spec["slotPins"]:
        if pin["grant"] not in bindings.get(pin["holder"], set()):
            _fail(
                f"slotPins: {pin['holder']}/{pin['grant']} pins a slot for a binding the "
                "grant table does not produce"
            )
        if pin["slot"] < 0:
            _fail(f"slotPins: {pin['holder']}/{pin['grant']}: negative slot")
        # Vocabulary only. Whether the stated reason is *true* is a property of
        # the derived manifest, not of this spec, so the generation builder
        # re-derives it and refuses a mislabelled pin (B91).
        if pin["reason"] not in _builder.SLOT_REASONS:
            _fail(
                f"slotPins: {pin['holder']}/{pin['grant']}: unknown reason {pin['reason']!r}; "
                f"expected one of {', '.join(_builder.SLOT_REASONS)}"
            )
    pin_keys = [(pin["holder"], pin["grant"]) for pin in spec["slotPins"]]
    if len(set(pin_keys)) != len(pin_keys):
        _fail("slotPins: duplicate (holder, grant)")
    for holder, pins in _grouped(spec["slotPins"], "holder").items():
        slots = [pin["slot"] for pin in pins]
        if len(set(slots)) != len(slots):
            _fail(f"slotPins: {holder} pins one slot twice")

    for entry in spec["commandBindings"]:
        if entry["component"] not in admitted:
            _fail(f"commandBindings: {entry['component']!r} is not an admitted component")
        if entry["commands"] != sorted(set(entry["commands"])):
            _fail(f"commandBindings: {entry['component']}: commands must be sorted and unique")
        # A command name is not an executable name, and assuming it was is what
        # this rule got wrong first: `dango`'s profile is `["echo", "sysinfo"]`,
        # but `echo` is served by the `echo-agent` executable, reached through
        # `spawn-service`'s exec grant rather than one `dango` holds. The
        # component that runs a command and the component that holds authority
        # over it are routinely different, which is the whole point of a spawn
        # service.
        #
        # What is checkable without inventing a naming convention: some admitted
        # component must hold an exec grant, or the profile advertises commands
        # no one in this system can launch. Binding each command name to its
        # executable is CP2's runtime resolution, not a host-side string rule.
        spawnable = {
            grant["target"]
            for grant in spec["grants"]
            if grant["capabilityKind"] == "executable"
        }
        if entry["commands"] and not spawnable:
            _fail(
                f"commandBindings: {entry['component']} declares commands, but this system "
                "grants no executable authority at all"
            )

    for entry in spec["imageSizes"]:
        if entry["component"] not in admitted:
            _fail(f"imageSizes: {entry['component']!r} is not an admitted component")
        if entry["bytes"] <= 0:
            _fail(f"imageSizes: {entry['component']}: must be positive")
    if spec["defaultImageBytes"] <= 0:
        _fail("defaultImageBytes: must be positive")

    for binding in spec["state"]:
        if binding["owner"] not in admitted:
            _fail(f"state: {binding['name']}: owner {binding['owner']!r} is not admitted")
        if binding["policy"] not in _builder.POLICY:
            _fail(f"state: {binding['name']}: unknown policy {binding['policy']!r}")
    # The builder refuses duplicate state names (`unique_sorted`), so a spec that
    # produced them could never encode; refusing here names the spec instead of
    # the manifest it would have written.
    state_names = [binding["name"] for binding in spec["state"]]
    if len(set(state_names)) != len(state_names):
        _fail("state: duplicate binding name")
    for binding in spec["state"]:
        if binding["schemaVersion"] <= 0:
            _fail(f"state: {binding['name']}: schemaVersion must be positive")
    command_components = [entry["component"] for entry in spec["commandBindings"]]
    if len(set(command_components)) != len(command_components):
        _fail("commandBindings: duplicate component")
    sized = [entry["component"] for entry in spec["imageSizes"]]
    if len(set(sized)) != len(sized):
        _fail("imageSizes: duplicate component")

    _validate_bounds(spec, contract)
    _validate_notifications(spec, admitted)
    _validate_interfaces(spec, components, admitted, contract)
    _validate_boot_profiles(spec, admitted)


def _validate_bounds(spec: dict, contract: ModuleType) -> None:
    """Every ceiling `schema.zt` declares, enforced in one place.

    A bound that is declared and never checked is not a bound. These are
    enforced here rather than scattered through the rules above so the set is
    auditable against the contract: each entry names a list and its declared
    ceiling, and the text bounds cover every field that reaches a manifest
    string.
    """
    for field, ceiling in (
        ("grants", contract.MAX_GRANTS),
        ("slotPins", contract.MAX_SLOT_PINS),
        ("notifications", contract.MAX_NOTIFICATIONS),
        ("notificationBindings", contract.MAX_NOTIFICATION_BINDINGS),
        ("bootProfiles", contract.MAX_BOOT_PROFILES),
        ("state", contract.MAX_STATE_BINDINGS),
        ("commandBindings", contract.MAX_COMMAND_BINDINGS),
        ("imageSizes", contract.MAX_IMAGE_SIZES),
    ):
        if len(spec[field]) > ceiling:
            _fail(f"{field}: {len(spec[field])} entries exceeds the declared bound of {ceiling}")

    graph = spec.get("fabricGraph")
    if graph is not None:
        if len(graph["routes"]) > contract.MAX_ROUTES:
            _fail(f"fabricGraph.routes: exceeds the declared bound of {contract.MAX_ROUTES}")
        if len(graph["profiles"]) > contract.MAX_PROFILES:
            _fail(f"fabricGraph.profiles: exceeds the declared bound of {contract.MAX_PROFILES}")
        for route in graph["routes"]:
            if len(route["participants"]) > contract.MAX_PARTICIPANTS_PER_ROUTE:
                _fail(
                    f"fabricGraph: route {route['name']} has more participants than the "
                    f"declared bound of {contract.MAX_PARTICIPANTS_PER_ROUTE}"
                )

    # Identifier-shaped fields against `maxNameBytes`, free text against
    # `maxTextBytes`. Both end up in the manifest, which the generation encoder
    # bounds again — but a spec that cannot produce an admissible manifest should
    # be refused here, naming the field, rather than at encode time.
    def bounded(value: str, ceiling: int, label: str) -> None:
        if len(value.encode("utf-8")) > ceiling:
            _fail(f"{label}: exceeds {ceiling} bytes")

    names = contract.MAX_NAME_BYTES
    text = contract.MAX_TEXT_BYTES
    bounded(spec["name"], names, "name")
    bounded(spec["targetRequirement"], names, "targetRequirement")
    bounded(spec["bootAction"], names, "bootAction")
    bounded(spec["bootstrapInstance"], names, "bootstrapInstance")
    bounded(spec["deploymentConstraint"], text, "deploymentConstraint")
    bounded(spec["acceptanceCriteria"], text, "acceptanceCriteria")
    for component in spec["components"]:
        bounded(component, names, f"components[{component}]")
    for grant in spec["grants"]:
        bounded(grant["name"], names, f"grants[{grant['name']}].name")
    for entry in spec["interfaceSchemas"]:
        bounded(entry, text, "interfaceSchemas")
    for binding in spec["state"]:
        bounded(binding["name"], names, f"state[{binding['name']}].name")
    for notification in spec["notifications"]:
        bounded(notification["name"], names, f"notifications[{notification['name']}].name")


def _grouped(entries: list[dict], key: str) -> dict[str, list[dict]]:
    out: dict[str, list[dict]] = {}
    for entry in entries:
        out.setdefault(entry[key], []).append(entry)
    return out


def _validate_notifications(spec: dict, admitted: set[str]) -> None:
    names: set[str] = set()
    for notification in spec["notifications"]:
        if notification["name"] in names:
            _fail(f"notifications: duplicate {notification['name']!r}")
        names.add(notification["name"])
        for role in ("source", "target"):
            if notification[role] not in admitted:
                _fail(f"notifications: {notification['name']}: {role} is not admitted")
        if notification["source"] == notification["target"]:
            _fail(f"notifications: {notification['name']}: source and target are the same")
    by_grant = _grouped(spec["notificationBindings"], "grant")
    for name in sorted(names):
        holders = by_grant.get(name, [])
        roles = sorted(entry["role"] for entry in holders)
        if roles != ["signal", "wait"]:
            _fail(f"notifications: {name}: needs exactly one signal and one wait binding")
    for binding in spec["notificationBindings"]:
        if binding["grant"] not in names:
            _fail(f"notificationBindings: {binding['grant']!r} names no declared notification")
        if binding["holder"] not in admitted:
            _fail(f"notificationBindings: {binding['grant']}: holder is not admitted")
        if binding["role"] not in ("signal", "wait"):
            _fail(f"notificationBindings: {binding['grant']}: unknown role {binding['role']!r}")
        notification = next(n for n in spec["notifications"] if n["name"] == binding["grant"])
        expected = notification["source"] if binding["role"] == "signal" else notification["target"]
        if binding["holder"] != expected:
            _fail(
                f"notificationBindings: {binding['grant']}: the {binding['role']} holder must be "
                f"{expected!r}, not {binding['holder']!r}"
            )


def _validate_interfaces(
    spec: dict, components: dict[str, dict], admitted: set[str], contract: ModuleType
) -> None:
    if len(spec["interfaceSchemas"]) > contract.MAX_INTERFACE_SCHEMAS:
        _fail("interfaceSchemas: exceeds bound")
    catalogue: dict[str, str] = {}
    for entry in spec["interfaceSchemas"]:
        path = (ROOT / entry).resolve()
        if not path.is_relative_to(INTERFACE_SCHEMA_ROOT) or not path.is_file():
            _fail(f"interfaceSchemas: {entry!r} is no declared interface schema")
        catalogue[path.stem] = entry

    graph = spec.get("fabricGraph")
    if graph is None:
        # No graph means no component may hold a route role, or the spec would
        # claim an interface this system never establishes.
        for name in sorted(admitted):
            if components[name]["interfaces"]:
                _fail(
                    f"{name}: declares interface entries, but this system declares no fabric graph"
                )
        return

    if graph["fabricComponent"] not in admitted:
        _fail("fabricGraph: fabricComponent is not admitted")
    names = [route["name"] for route in graph["routes"]]
    if len(set(names)) != len(names):
        _fail("fabricGraph: duplicate route name")
    declared_interfaces = {
        _interface_name(entry) for entry in spec["interfaceSchemas"]
    }
    for route in graph["routes"]:
        if route["interface"] not in declared_interfaces:
            _fail(
                f"fabricGraph: route {route['name']} names interface {route['interface']!r}, "
                "which this system does not admit"
            )
        for participant in route["participants"]:
            if participant["component"] not in admitted:
                _fail(f"fabricGraph: {route['name']}: participant is not admitted")
            for hop in participant["interposition"]:
                if hop not in admitted:
                    _fail(f"fabricGraph: {route['name']}: interposition hop {hop!r} is not admitted")
            # The route role a system gives a component and the role its own
            # spec declares are the same fact; CP0's gate checks it against
            # `valid.zti`, and this checks it against the system that produces a
            # manifest at all.
            tag = _DIRECTION_TAGS[participant["direction"]]
            entries = components[participant["component"]]["interfaces"]
            if not any(
                item["name"] == route["name"]
                and item["tag"] == tag
                and item["interface"] == route["interface"]
                for item in entries
            ):
                _fail(
                    f"{participant['component']}: this system gives it a {tag} role on route "
                    f"{route['name']}, which its component spec does not declare"
                )
    for profile in graph["profiles"]:
        for interposition in profile["interpositions"]:
            if interposition["route"] not in set(names):
                _fail(f"fabricGraph: profile {profile['name']} interposes an unknown route")
            if not interposition["chain"]:
                _fail(f"fabricGraph: profile {profile['name']} declares an empty chain")
            for hop in interposition["chain"]:
                if hop not in admitted:
                    _fail(f"fabricGraph: profile {profile['name']}: hop {hop!r} is not admitted")
        for control in profile["streamControls"]:
            if control not in {grant["name"] for grant in spec["grants"]}:
                _fail(
                    f"fabricGraph: profile {profile['name']} indexes control {control!r}, "
                    "which is no declared grant"
                )


def _interface_name(entry: str) -> str:
    """The `InterfaceSchema.name` a declared path resolves to."""
    import interface_schema

    return interface_schema.compile_interface((ROOT / entry).resolve()).name


_DIRECTION_TAGS = {
    "publish": "output",
    "subscribe": "input",
    "client": "command",
    "server": "event",
}


def _validate_boot_profiles(spec: dict, admitted: set[str]) -> None:
    profile_names = [profile["name"] for profile in spec["bootProfiles"]]
    if len(set(profile_names)) != len(profile_names):
        _fail("bootProfiles: duplicate name")
    graph = spec.get("fabricGraph")
    fabric_profiles = {profile["name"] for profile in graph["profiles"]} if graph else set()
    for profile in spec["bootProfiles"]:
        if graph is not None and profile["fabricProfile"] not in fabric_profiles:
            _fail(f"bootProfiles: {profile['name']} names unknown fabric profile")
        for name in profile["instances"] + profile["requiredInstances"]:
            if name not in admitted:
                _fail(f"bootProfiles: {profile['name']} names unadmitted component {name!r}")


def _capability_sets(
    grants: list[dict], admitted: set[str]
) -> tuple[dict[str, set[str]], dict[str, set[str]]]:
    """Which capability kinds each component provides and requires.

    The grant table's own semantics: for every kind but `executable`, `source`
    owns the object and `target` receives it. An `executable` grant's target is
    an executable name rather than an instance, and the `exec`/`spawn` authority
    is held by the spawner, so it is a requirement of the source and a provision
    of nobody. Identical to `check-component-spec.py`'s derivation, because it is
    the same fact read from the same table.
    """
    provided: dict[str, set[str]] = {name: set() for name in admitted}
    required: dict[str, set[str]] = {name: set() for name in admitted}
    for grant in grants:
        kind = grant["capabilityKind"]
        if kind == "executable":
            required[grant["source"]].add(kind)
            continue
        provided[grant["source"]].add(kind)
        required[grant["target"]].add(kind)
    return provided, required


def derive_bindings(grants: list[dict], admitted: set[str]) -> dict[str, set[str]]:
    """Which grants each instance binds.

    A grant materializes in whichever instance holds it: an `executable` grant in
    its spawner, an `endpoint` in both ends, and every delegated authority in its
    target. This mirrors `build-generation.py`'s own holder resolution, which
    fails a build when an authority-bearing grant has no concrete binding.
    """
    bindings: dict[str, set[str]] = {name: set() for name in admitted}
    for grant in grants:
        kind = grant["capabilityKind"]
        if kind == "executable":
            bindings[grant["source"]].add(grant["name"])
            continue
        if kind == "endpoint":
            bindings[grant["source"]].add(grant["name"])
        bindings[grant["target"]].add(grant["name"])
    return bindings


def derive_manifest(system: CompiledSystem) -> dict:
    """The `contracts/generation-manifest/v1` manifest this system spec describes."""
    spec, components = system.spec, system.components
    admitted = set(spec["components"])
    placements = {entry["component"]: entry for entry in spec["placements"]}
    commands = {entry["component"]: entry["commands"] for entry in spec["commandBindings"]}
    sizes = {entry["component"]: entry["bytes"] for entry in spec["imageSizes"]}
    bindings = derive_bindings(spec["grants"], admitted)
    pins = {(pin["holder"], pin["grant"]): pin for pin in spec["slotPins"]}

    executables = []
    objects = []
    instances = []
    budget = []
    private_budget = []
    for name in spec["components"]:
        component = components[name]
        resource = component["runtime"]["resource"]
        placement = placements.get(name, {})
        executable = {
            "commandProfile": commands.get(name, []),
            "name": name,
            "object": f"sha256:{name}",
            "role": component["componentType"],
            # How many children this composition launches, which varies by
            # system: `init` is 1 under the channel plane and 18 under the
            # reference generation. The component spec's value is the reference
            # default.
            "spawnBudget": placement.get("spawnBudget", resource["spawnBudget"]),
        }
        # Only a non-default stack is carried, matching the manifest's optional
        # field: emitting the default everywhere would change the fixture bytes
        # without changing any admitted image.
        if resource["stackBytes"] != _builder.COMPONENT_DEFAULT_STACK_BYTES:
            executable["stackBytes"] = resource["stackBytes"]
        executables.append(executable)
        objects.append(
            {
                "id": f"sha256:{name}",
                "kind": "bootstrap" if component["componentType"] == "init" else "component",
                "size": sizes.get(name, spec["defaultImageBytes"]),
            }
        )
        instance = {
            "autostart": placement.get("autostart", True),
            "bindings": [
                {
                    "grant": grant,
                    "slot": pins[(name, grant)]["slot"],
                    "slotReason": pins[(name, grant)]["reason"],
                }
                if (name, grant) in pins
                else {"grant": grant}
                for grant in sorted(bindings[name])
            ],
            "dependencies": component["dependencies"],
            "executable": name,
            "health": component["health"],
            "name": name,
            "owner": placement.get("owner", component["owner"]),
        }
        for field in ("priority", "extraThreads", "workerPriority"):
            if field in placement:
                instance[field] = placement[field]
        instances.append(instance)
        if any(
            (
                resource["bufferBytePages"],
                resource["bufferCount"],
                resource["mappingCount"],
                resource["loanCount"],
            )
        ):
            budget.append(
                {
                    "bufferCount": resource["bufferCount"],
                    "bytePages": resource["bufferBytePages"],
                    "holder": name,
                    "loanCount": resource["loanCount"],
                    "mappingCount": resource["mappingCount"],
                }
            )
        # C10.4: the same derivation, from the same record, for the other memory
        # plane. A separate list rather than a column of the shared-buffer one
        # because the two are separately accounted — a component may hold either,
        # both, or neither — and because a holder with no quota must be *absent*
        # rather than present with a zero, which is what deny-by-default means
        # here.
        if resource["privatePageQuota"]:
            private_budget.append(
                {
                    "holder": name,
                    "pageQuota": resource["privatePageQuota"],
                }
            )

    # `fabric-graph`'s presence is strictly derived: the builder refuses a graph
    # without the object and an object without the graph, so there is nothing to
    # choose. `shared-buffer-budget` is declared, because its presence is what
    # makes the builder encode a budget payload independently of whether any
    # component has a quota, and the existing fixtures disagree about the
    # correlation. `boot-layout` is carried by every generation.
    if spec["sharedBufferBudgetObject"]:
        objects.append({"id": "shared-buffer-budget", "kind": "resource", "size": 4096})
    if spec.get("fabricGraph") is not None:
        objects.append({"id": "fabric-graph", "kind": "resource", "size": 4096})
    objects.append({"id": "boot-layout", "kind": "resource", "size": 4096})
    # C10.4: strictly derived, unlike `shared-buffer-budget` above. The builder
    # refuses a `privateMemoryBudget` without this object and encodes nothing
    # from an object with no holders, so there is nothing for a spec to choose:
    # the object is present exactly when some component declared a quota.
    if private_budget:
        objects.append({"id": "private-memory-budget", "kind": "resource", "size": 4096})

    # Canonical order throughout. `build-generation.py` sorts `objects`,
    # `executables`, `instances`, `grants`, and `state` before encoding
    # (`unique_sorted`, and grants by name/source/target), so emitting sorted
    # output changes no admitted byte while removing the hand-authored ordering
    # that made two fixtures differing only in list order look like different
    # generations. `just generation_check` compares the built bytes and is the
    # evidence for that claim.
    objects.sort(key=lambda entry: entry["id"])
    executables.sort(key=lambda entry: entry["name"])
    instances.sort(key=lambda entry: entry["name"])
    budget.sort(key=lambda entry: entry["holder"])
    private_budget.sort(key=lambda entry: entry["holder"])

    manifest = {
        "bootAction": spec["bootAction"],
        "bootstrapInstance": spec["bootstrapInstance"],
        "executables": executables,
        "formatVersion": _GENERATION_SOURCE_FORMAT,
        "generation": spec["generation"],
        "grants": [
            {
                "name": grant["name"],
                "capabilityKind": grant["capabilityKind"],
                "rights": grant["rights"],
                "source": grant["source"],
                "target": grant["target"],
                "transferable": grant["transferable"],
            }
            for grant in sorted(
                spec["grants"],
                key=lambda entry: (entry["name"], entry["source"], entry["target"]),
            )
        ],
        "health": {
            "bootAttempts": spec["bootAttempts"],
            "requiredInstances": sorted(
                name for name in spec["components"] if components[name]["health"] == "required"
            ),
        },
        "instances": instances,
        "mintedBindings": [],
        "interfaceSchemas": spec["interfaceSchemas"],
        "objects": objects,
        "privateMemoryBudget": private_budget,
        "sharedBufferBudget": budget,
        "state": sorted(spec["state"], key=lambda entry: entry["name"]),
        "target": spec["targetRequirement"],
    }
    if spec["bootProfiles"]:
        manifest["bootProfiles"] = spec["bootProfiles"]
    if spec["notifications"]:
        manifest["notificationGrants"] = spec["notifications"]
        manifest["notificationBindings"] = spec["notificationBindings"]
    graph = spec.get("fabricGraph")
    if graph is not None:
        manifest["fabricGraph"] = graph
    return manifest


def compile_system(
    path: Path,
    *,
    components: dict[str, dict] | None = None,
    contract: ModuleType = default_contract,
) -> CompiledSystem:
    table = components
    if table is None:
        catalogue = interface_catalogue()
        table = {entry.name: entry.spec for entry in admit_specs(catalogue=catalogue)}
    spec = _load(path.resolve(), contract)
    if spec["name"] != path.stem:
        _fail(f"{path}: declares system {spec['name']!r}, so its file name must match")
    _validate(spec, table, contract)
    normalized = (
        json.dumps(spec, sort_keys=True, separators=(",", ":"), ensure_ascii=True) + "\n"
    ).encode("utf-8")
    identity = hashlib.sha256(contract.IDENTITY_DOMAIN + normalized).digest()
    return CompiledSystem(
        name=spec["name"],
        spec=spec,
        components={name: table[name] for name in spec["components"]},
        normalized=normalized,
        identity=identity,
    )


def system_paths(root: Path = SYSTEM_ROOT) -> list[Path]:
    return sorted(root.glob("*.zti"))


def compiled_specs() -> dict[str, CompiledSpec]:
    return {entry.name: entry for entry in admit_specs(catalogue=interface_catalogue())}
