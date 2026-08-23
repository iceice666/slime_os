#!/usr/bin/env python3

"""CP0 component-specification model gate.

Validates every `contracts/component-spec/v1/components/*.zti` record
structurally and semantically, proves the identity computation is stable across
equivalent encodings, proves each named negative case is actually refused, and
proves the corpus covers every component the reference generation declares.

The negative cases matter more than the positive ones. B67 found two gate arms
that computed their victim by restating the predicate they were meant to
violate, so neither could ever fail; every mutation below is therefore checked
to be refused *and* the baseline it mutates is checked to be admitted, so a
mutation that trips some unrelated guard cannot pass for the check it names.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import copy
import json
import os
import subprocess
import tempfile
from pathlib import Path

import component_spec_contract as CONTRACT
from component_spec import (
    ComponentSpecError,
    admit_specs,
    compile_spec,
    interface_catalogue,
    workspace_binaries,
)
from boot_contracts import PRIVATE_MEMORY_ROOT_REGION_PAGES
from harness import ROOT, load_script
from zutai_cli import STDLIB, binary

GENERATION_SOURCE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "valid.zti"
BUILDER = load_script("component_spec_check_builder", "build/build-generation.py")


def fail(message: str) -> None:
    raise SystemExit(f"component spec check: {message}")


def zti(value: object, indent: int = 0) -> str:
    padding = " " * indent
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, str):
        return json.dumps(value, ensure_ascii=True)
    if isinstance(value, list):
        if not value:
            return "[]"
        rows = "".join(f"{padding}  {zti(item, indent + 2)};\n" for item in value)
        return "[\n" + rows + padding + "]"
    if isinstance(value, dict):
        rows = "".join(
            f"{padding}  {key} = {zti(item, indent + 2)};\n" for key, item in value.items()
        )
        return "{\n" + rows + padding + "}"
    raise TypeError(type(value))


def load_manifest() -> dict:
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    process = subprocess.run(
        [str(binary()), "json", str(GENERATION_SOURCE)],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode != 0:
        fail(f"cannot read the reference generation: {process.stderr.strip()}")
    return json.loads(process.stdout)


def write_spec(root: Path, spec: dict) -> Path:
    path = root / f"{spec['name']}.zti"
    path.write_text(zti(spec) + "\n", encoding="utf-8")
    return path


def source_spec(name: str) -> dict:
    """One record's decoded source form, as the corpus stores it.

    Read back through the compiler rather than reconstructed, so a mutation is
    applied to the same shape the committed fixture has.
    """
    return copy.deepcopy(SOURCE_FORMS[name])


def admitted(root: Path, spec: dict) -> None:
    """The baseline a mutation starts from must itself be admitted.

    Without this, a mutation could be refused for a reason unrelated to the one
    it names and the arm would still pass.
    """
    try:
        compile_spec(write_spec(root, spec), CATALOGUE)
    except ComponentSpecError as error:
        fail(f"baseline for a negative case was itself refused: {error}")


def rejected(root: Path, label: str, spec: dict) -> None:
    try:
        compile_spec(write_spec(root, spec), CATALOGUE)
    except ComponentSpecError:
        return
    fail(f"{label} was accepted")


CATALOGUE = interface_catalogue()
try:
    CORPUS = admit_specs(catalogue=CATALOGUE)
except ComponentSpecError as error:
    # The committed corpus failing is the gate's ordinary failure mode, not an
    # internal error, so it is reported with the gate's own prefix rather than a
    # traceback: a suite of 30-odd gates is attributable by that prefix.
    fail(f"the committed corpus was refused: {error}")
BY_NAME = {entry.name: entry for entry in CORPUS}
SOURCE_FORMS = {entry.name: entry.spec for entry in CORPUS}
MANIFEST = load_manifest()

# 1. Corpus coverage: every component the reference generation declares has a
#    spec, and every spec names a component it declares. A corpus that merely
#    overlaps the manifest would leave the uncovered components exactly where
#    B70 found them.
declared_executables = {entry["name"] for entry in MANIFEST["executables"]}
declared_instances = {entry["name"] for entry in MANIFEST["instances"]}
if declared_executables != declared_instances:
    fail("the reference generation's executables and instances name different components")
missing = sorted(declared_instances - set(BY_NAME))
if missing:
    fail(f"no component spec for declared component(s): {missing}")
extra = sorted(set(BY_NAME) - declared_instances)
if extra:
    fail(f"component spec(s) name no declared component: {extra}")

# 2. Each spec agrees with the manifest on the facts both state. A spec free to
#    disagree with the generation that composes it would be documentation, not a
#    contract, and CP1 could not derive one from the other.
executables = {entry["name"]: entry for entry in MANIFEST["executables"]}
instances = {entry["name"]: entry for entry in MANIFEST["instances"]}
budgets = {entry["holder"]: entry for entry in MANIFEST["sharedBufferBudget"]}
private_budgets = {
    entry["holder"]: entry for entry in MANIFEST.get("privateMemoryBudget") or []
}
for name, entry in sorted(BY_NAME.items()):
    spec = entry.spec
    executable = executables[name]
    instance = instances[name]
    if spec["componentType"] != executable["role"]:
        fail(f"{name}: spec type {spec['componentType']!r} != manifest role {executable['role']!r}")
    if spec["owner"] != instance["owner"]:
        fail(f"{name}: spec owner != manifest owner")
    if spec["health"] != instance["health"]:
        fail(f"{name}: spec health != manifest health")
    if spec["dependencies"] != instance["dependencies"]:
        fail(f"{name}: spec dependencies != manifest dependencies")
    resource = spec["runtime"]["resource"]
    if resource["spawnBudget"] != executable["spawnBudget"]:
        fail(f"{name}: spec spawn budget != manifest spawnBudget")
    if resource["stackBytes"] != executable.get("stackBytes", 16384):
        fail(f"{name}: spec stack bytes != manifest stackBytes")
    if resource["extraThreads"] != instance.get("extraThreads", 0):
        fail(f"{name}: spec extra threads != manifest extraThreads")
    budget = budgets.get(name)
    expected = (
        (budget["bytePages"], budget["bufferCount"], budget["mappingCount"], budget["loanCount"])
        if budget
        else (0, 0, 0, 0)
    )
    actual = (
        resource["bufferBytePages"],
        resource["bufferCount"],
        resource["mappingCount"],
        resource["loanCount"],
    )
    if actual != expected:
        fail(f"{name}: spec shared-buffer resource {actual} != manifest budget {expected}")
    # C10.4's private-memory ceiling, on exactly the shared-buffer rule above:
    # absence of a budget entry means zero, because deny-by-default is what an
    # unnamed holder gets. Without this the field would be the one
    # `runtime.resource` number a spec could state and the generation contradict.
    private_budget = private_budgets.get(name)
    declared_pages = private_budget["pageQuota"] if private_budget else 0
    if resource["privatePageQuota"] != declared_pages:
        fail(
            f"{name}: spec privatePageQuota {resource['privatePageQuota']} != manifest "
            f"privateMemoryBudget {declared_pages}"
        )
    if spec["runtime"]["executionEnvironment"] != MANIFEST["target"]:
        fail(f"{name}: spec execution environment != manifest target")

# `provides`/`requires` are derived from the manifest's `grants[]` and compared
# in both directions, so a record can neither claim authority the generation
# never grants it nor omit authority it does. The derivation is the grant table's
# own semantics: for every kind but `executable`, `source` owns the object the
# grant names and `target` receives it. An `executable` grant is the exception —
# its `target` is an executable *name*, not an instance, and the authority
# (`exec`/`spawn`) is held by the `source` that spawns it, so it counts as a
# requirement of the spawner and a provision of nobody.
granted_provides: dict[str, set[str]] = {name: set() for name in BY_NAME}
granted_requires: dict[str, set[str]] = {name: set() for name in BY_NAME}
for grant in MANIFEST["grants"]:
    kind = grant["capabilityKind"]
    if kind == "executable":
        if grant["source"] in granted_requires:
            granted_requires[grant["source"]].add(kind)
        continue
    if grant["source"] in granted_provides:
        granted_provides[grant["source"]].add(kind)
    if grant["target"] in granted_requires:
        granted_requires[grant["target"]].add(kind)
for name, entry in sorted(BY_NAME.items()):
    for label, declared, derived in (
        ("provides", entry.spec["provides"], granted_provides[name]),
        ("requires", entry.spec["requires"], granted_requires[name]),
    ):
        if declared != sorted(derived):
            fail(
                f"{name}: spec {label} {declared} != the manifest's grant-derived "
                f"{sorted(derived)}"
            )

# 3. Every declared route role in the fabric graph appears as an interface
#    reference, tagged for the direction the graph declares. This is what makes
#    the interface section a real projection of the graph rather than prose.
DIRECTION_TAGS = {
    "publish": CONTRACT.INTERFACE_TAG_OUTPUT,
    "subscribe": CONTRACT.INTERFACE_TAG_INPUT,
    "client": CONTRACT.INTERFACE_TAG_COMMAND,
    "server": CONTRACT.INTERFACE_TAG_EVENT,
}
graph = MANIFEST["fabricGraph"]
for route in graph["routes"]:
    for participant in route["participants"]:
        component = participant["component"]
        tag = DIRECTION_TAGS[participant["direction"]]
        entries = BY_NAME[component].spec["interfaces"]
        if not any(
            item["name"] == route["name"]
            and item["tag"] == tag
            and item["interface"] == route["interface"]
            for item in entries
        ):
            fail(
                f"{component}: declares no {tag} entry for route {route['name']} "
                f"({route['interface']}), which the fabric graph gives it"
            )
        policy = next(
            (
                item
                for item in BY_NAME[component].spec["communication"]["qos"]
                if item["reference"] == route["name"]
            ),
            None,
        )
        if policy is None:
            fail(f"{component}: declares no QoS policy for its {route['name']} role")
        for field in (
            "reliability",
            "durability",
            "liveliness",
            "historyDepth",
            "retainedDepth",
            "deadlineNs",
            "lifespanNs",
            "leaseNs",
        ):
            if policy[field] != participant[field]:
                fail(
                    f"{component}: QoS {field} for {route['name']} is {policy[field]!r}, "
                    f"but the graph declares {participant[field]!r}"
                )

# The reverse direction, which is what makes this a projection rather than a
# lower bound. Without it a record could declare a route role and a QoS policy
# the graph never gives it: the forward loop only checks that declared graph
# facts are present, never that present facts are declared by the graph.
#
# Three sources authorize an entry, and no others:
#   - a participant role, tagged by direction, checked above;
#   - an interposition hop, which genuinely carries one route in both
#     directions and so takes one `input` and one `output` entry. Hops come from
#     both `participants[].interposition` and every named profile's
#     `profiles[].interpositions[].chain`, because a profile-declared chain is
#     authority some boot actually grants;
#   - the fabric component itself and the C8.10 route workers, which own whole
#     routes. The worker partition is `build-generation.py`'s own
#     `FABRIC_ROUTE_WORKERS`, read rather than restated, and the worker instance
#     names are that table's keys mapped through the manifest's declared
#     `fabric-<shape>-worker` executables.
authorized: dict[str, set[tuple[str, str, str]]] = {name: set() for name in BY_NAME}


def authorize(component: str, route: str, tag: str, interface: str) -> None:
    if component in authorized:
        authorized[component].add((route, tag, interface))


def authorize_relay(component: str, route: str, interface: str) -> None:
    authorize(component, route, CONTRACT.INTERFACE_TAG_INPUT, interface)
    authorize(component, route, CONTRACT.INTERFACE_TAG_OUTPUT, interface)


ROUTE_INTERFACES = {route["name"]: route["interface"] for route in graph["routes"]}
for route in graph["routes"]:
    for participant in route["participants"]:
        authorize(
            participant["component"],
            route["name"],
            DIRECTION_TAGS[participant["direction"]],
            route["interface"],
        )
        for hop in participant["interposition"]:
            authorize_relay(hop, route["name"], route["interface"])
for profile in graph["profiles"]:
    for interposition in profile["interpositions"]:
        for hop in interposition["chain"]:
            authorize_relay(hop, interposition["route"], ROUTE_INTERFACES[interposition["route"]])
for route_name, interface in ROUTE_INTERFACES.items():
    authorize_relay(graph["fabricComponent"], route_name, interface)
# `FABRIC_ROUTE_WORKERS` is keyed by wait *shape* (`stream`/`call`/`operation`),
# while the instances are named `fabric-call-worker` and `fabric-op-worker` — the
# stream shape has no separate worker instance, since `fabric-service` carries it
# itself. The mapping is stated once here and checked, so a renamed shape fails
# loudly instead of silently authorizing nothing.
WORKER_INSTANCES = {"call": "fabric-call-worker", "operation": "fabric-op-worker"}
shapes = {shape for shape, _routes in BUILDER.FABRIC_ROUTE_WORKERS}
if not set(WORKER_INSTANCES) <= shapes:
    fail(
        f"worker shapes {sorted(set(WORKER_INSTANCES) - shapes)} are no longer declared by "
        "build-generation.py's FABRIC_ROUTE_WORKERS"
    )
for shape, worker_routes in BUILDER.FABRIC_ROUTE_WORKERS:
    worker = WORKER_INSTANCES.get(shape)
    if worker is None or worker not in BY_NAME:
        continue
    for route_name in worker_routes:
        if route_name in ROUTE_INTERFACES:
            authorize_relay(worker, route_name, ROUTE_INTERFACES[route_name])

for name, entry in sorted(BY_NAME.items()):
    for item in entry.spec["interfaces"]:
        key = (item["name"], item["tag"], item["interface"])
        if key not in authorized[name]:
            fail(
                f"{name}: declares a {item['tag']} entry for route {item['name']} "
                f"({item['interface']}) that the fabric graph does not give it"
            )
    declared_routes = {item["name"] for item in entry.spec["interfaces"]}
    for policy in entry.spec["communication"]["qos"]:
        # A policy is only meaningful where the graph declares one. A relay hop
        # and a broker carry no policy of their own: they carry whatever the
        # participants agreed, so a policy on a route this component only relays
        # would be asserting terms it does not set.
        if not any(
            participant["component"] == name and route["name"] == policy["reference"]
            for route in graph["routes"]
            for participant in route["participants"]
        ):
            fail(
                f"{name}: declares a QoS policy for {policy['reference']}, but the "
                "graph names it no participant on that route"
            )
        if policy["reference"] not in declared_routes:
            fail(f"{name}: QoS policy for {policy['reference']} names no declared interface entry")

# `passFailCriteria` resolution against the named gate's own string literals is
# `component_spec.gate_markers`' job, applied per record at compile time and
# exercised by the "a criterion no gate literal contains" arm below. It is not
# repeated here: the corpus admission at the top of this file already ran it for
# all 42 records.

# 4. Corpus-level implementation facts. The per-spec resolution of a gate target
#    and a `[[bin]]` name is `component_spec` compiler work, exercised by the
#    mutation arms below. What only the whole corpus can state is which
#    components this repository declares but ships no implementation for, and
#    that set must be exactly the two the deleted-client devlogs record. A third
#    appearing silently is how B70's class of drift starts.
EXPECTED_UNDECLARED = ("generation-list", "storage-store-probe")
observed_undeclared = tuple(
    sorted(
        name
        for name, entry in BY_NAME.items()
        if entry.spec["implementation"]["provider"] == CONTRACT.PROVIDER_UNDECLARED
    )
)
if observed_undeclared != EXPECTED_UNDECLARED:
    fail(
        f"declared-without-implementation set is {list(observed_undeclared)}, "
        f"expected {list(EXPECTED_UNDECLARED)}; a component gaining or losing an "
        "implementation must move this set in the same change"
    )
# The pinned set is checked against `components/bins/Cargo.toml` rather than
# only asserted, so it stays a derived fact. Asserting the pair alone would go
# stale the moment someone lands one of the two missing components: the corpus
# would still say `undeclared`, the pin would still match, and nothing would
# notice. This loop is what makes the pin bite from the other direction.
BINARIES = dict(workspace_binaries())
for name in EXPECTED_UNDECLARED:
    if name not in declared_instances:
        fail(f"{name} is pinned as implementation-less but the generation no longer declares it")
    present = [candidate for candidate in (name, f"sel4-{name}") if candidate in BINARIES]
    if present:
        fail(
            f"{name} is pinned as implementation-less but [[bin]] {present[0]!r} now exists; "
            "record its implementation and drop it from the pin"
        )
# Every implemented component resolves to a distinct implementation artifact:
# two specs sharing one name would make the manifest's component identity
# ambiguous regardless of whether Cargo or the external mapping supplies it.
resolved = [
    entry.spec["implementation"]["binary"]
    for entry in CORPUS
    if entry.spec["implementation"]["provider"] != CONTRACT.PROVIDER_UNDECLARED
]
if len(set(resolved)) != len(resolved):
    fail("two component specs resolve to the same implementation binary")

# 5. Identity is stable across equivalent encodings and distinct across content.
#    Both are required of an identity: one that changed with whitespace could not
#    be compared across producers, and one that ignored a field would let two
#    different components collide.
with tempfile.TemporaryDirectory(prefix="slime-component-spec-check-") as temporary:
    root = Path(temporary)
    baseline = source_spec("fabric-publisher")
    left = compile_spec(write_spec(root, baseline), CATALOGUE)
    reordered = {key: baseline[key] for key in reversed(list(baseline))}
    reordered_path = root / "reordered.zti"
    reordered_path.write_text(zti(reordered) + "\n", encoding="utf-8")
    # The file name must still match the declared component, so this writes the
    # reordered form under the component's own name in a second directory.
    other = root / "other"
    other.mkdir()
    right = compile_spec(write_spec(other, reordered), CATALOGUE)
    if (left.normalized, left.identity) != (right.normalized, right.identity):
        fail("source field order changed the normalized identity")
    spaced = root / "spaced"
    spaced.mkdir()
    (spaced / "fabric-publisher.zti").write_text(
        "\n\n" + zti(baseline).replace(" = ", "   =   ") + "\n\n", encoding="utf-8"
    )
    spaced_spec = compile_spec(spaced / "fabric-publisher.zti", CATALOGUE)
    if (left.normalized, left.identity) != (spaced_spec.normalized, spaced_spec.identity):
        fail("source formatting changed the normalized identity")
    if left.identity != BY_NAME["fabric-publisher"].identity:
        fail("a relocated copy of a committed spec computed a different identity")
    shifted = copy.deepcopy(baseline)
    shifted["purpose"] = baseline["purpose"] + " Extra."
    shifted_root = root / "shifted"
    shifted_root.mkdir()
    if compile_spec(write_spec(shifted_root, shifted), CATALOGUE).identity == left.identity:
        fail("a changed field did not change the identity")

    # 6. Named negative cases. Each names one rule; each baseline is admitted
    #    first so no arm can pass by tripping an unrelated guard.
    arms = root / "arms"
    arms.mkdir()

    def mutate(label: str, name: str, apply) -> None:
        baseline_spec = source_spec(name)
        admitted(arms, baseline_spec)
        broken = source_spec(name)
        apply(broken)
        rejected(arms, label, broken)

    def drop_identity(spec: dict) -> None:
        del spec["owner"]

    def unknown_interface(spec: dict) -> None:
        spec["interfaces"][0]["interface"] = "NoSuchStream"

    def unknown_lifecycle(spec: dict) -> None:
        spec["lifecycle"] = ["Initialize", "Start", "Running", "Error", "Retired"]

    def unordered_lifecycle(spec: dict) -> None:
        spec["lifecycle"] = ["Start", "Initialize", "Running", "Error"]

    def missing_required_lifecycle(spec: dict) -> None:
        # Drop the first state the contract declares required, whichever it is,
        # so this arm follows `LIFECYCLE_REQUIRED` rather than naming a literal
        # that could fall out of the set without the arm noticing.
        dropped = CONTRACT.LIFECYCLE_REQUIRED[0]
        spec["lifecycle"] = [state for state in spec["lifecycle"] if state != dropped]

    def unknown_capability_kind(spec: dict) -> None:
        spec["requires"] = ["pciFunction"]

    def unsorted_capability_kinds(spec: dict) -> None:
        spec["provides"] = ["endpoint", "block"]

    def wrong_semantic(spec: dict) -> None:
        spec["communication"]["semantic"] = CONTRACT.SEMANTIC_CALL

    def mistagged_interface(spec: dict) -> None:
        spec["interfaces"][0]["tag"] = CONTRACT.INTERFACE_TAG_COMMAND

    def dangling_qos(spec: dict) -> None:
        spec["communication"]["qos"][0]["reference"] = "no-such-route"

    def contradictory_durability(spec: dict) -> None:
        spec["communication"]["qos"][0]["durability"] = "retained"
        spec["communication"]["qos"][0]["retainedDepth"] = 0

    def contradictory_liveliness(spec: dict) -> None:
        spec["communication"]["qos"][0]["liveliness"] = "manual"
        spec["communication"]["qos"][0]["leaseNs"] = 0

    def unknown_dependency(spec: dict) -> None:
        spec["dependencies"] = ["no-such-component"]

    def self_dependency(spec: dict) -> None:
        spec["dependencies"] = [spec["name"]]

    def default_outside_range(spec: dict) -> None:
        spec["configuration"] = [
            {"name": "spawnBudget", "default": 40, "minimum": 0, "maximum": 32}
        ]

    def unknown_platform_constraint(spec: dict) -> None:
        spec["compatibility"]["dependency"] = "preferred"

    def platform_disagreement(spec: dict) -> None:
        spec["compatibility"]["platform"] = "aarch64-rpi5"

    def unknown_interface_contract(spec: dict) -> None:
        spec["compatibility"]["interface"] = "contracts/no-such-contract/v1"

    def non_contract_interface_path(spec: dict) -> None:
        # A real directory that is not a versioned contract root.
        spec["compatibility"]["interface"] = "scripts"

    def wrong_dependency_mode(spec: dict) -> None:
        spec["compatibility"]["dependency"] = (
            CONTRACT.CONSTRAINT_NONE
            if spec["dependencies"]
            else CONTRACT.CONSTRAINT_EXACT
        )

    def wrong_resource_mode(spec: dict) -> None:
        spec["compatibility"]["resource"] = CONTRACT.CONSTRAINT_EXACT

    def parameter_default_disagrees(spec: dict) -> None:
        spec["configuration"] = [
            {
                "name": "spawnBudget",
                "default": spec["runtime"]["resource"]["spawnBudget"] + 1,
                "minimum": 0,
                "maximum": 32,
            }
        ]

    def parameter_names_no_resource(spec: dict) -> None:
        spec["configuration"] = [
            {"name": "retryLimit", "default": 1, "minimum": 0, "maximum": 4}
        ]

    def unobserved_criterion(spec: dict) -> None:
        # A Python fragment present in every gate script, but no marker.
        spec["test"]["passFailCriteria"] = "import"

    def unknown_test_target(spec: dict) -> None:
        spec["test"]["requiredTestEnvironment"] = "no_such_check"

    def empty_criterion(spec: dict) -> None:
        spec["test"]["passFailCriteria"] = ""

    def unpaged_stack(spec: dict) -> None:
        spec["runtime"]["resource"]["stackBytes"] = 16385

    def overbudget_spawn(spec: dict) -> None:
        spec["runtime"]["resource"]["spawnBudget"] = 33

    def buffers_without_pages(spec: dict) -> None:
        spec["runtime"]["resource"]["bufferCount"] = 2
        spec["runtime"]["resource"]["bufferBytePages"] = 0

    def overlarge_private_quota(spec: dict) -> None:
        # One page past the root's per-task reservation. The window's address
        # space is sized for that reservation when the child VSpace is built, so
        # a spec declaring more describes a region no root will grant.
        spec["runtime"]["resource"]["privatePageQuota"] = PRIVATE_MEMORY_ROOT_REGION_PAGES + 1

    def undeclared_device(spec: dict) -> None:
        spec["runtime"]["devices"] = ["block"]

    def bad_version(spec: dict) -> None:
        spec["version"] = "1.0"

    def unsupported_format(spec: dict) -> None:
        spec["formatVersion"] = CONTRACT.FORMAT_VERSION + 1

    def undeclared_with_binary(spec: dict) -> None:
        spec["implementation"] = {
            "provider": CONTRACT.PROVIDER_UNDECLARED,
            "binary": "console",
            "contentHash": "",
        }

    def workspace_without_binary(spec: dict) -> None:
        spec["implementation"] = {
            "provider": CONTRACT.PROVIDER_WORKSPACE,
            "binary": "",
            "contentHash": "",
        }

    def workspace_with_content_hash(spec: dict) -> None:
        spec["implementation"]["contentHash"] = "0" * CONTRACT.MAX_CONTENT_HASH_BYTES

    def external_without_content_hash(spec: dict) -> None:
        spec["implementation"] = {
            "provider": CONTRACT.PROVIDER_EXTERNAL,
            "binary": "console-external",
            "contentHash": "",
        }

    def external_with_malformed_content_hash(spec: dict) -> None:
        spec["implementation"] = {
            "provider": CONTRACT.PROVIDER_EXTERNAL,
            "binary": "console-external",
            "contentHash": "A" * CONTRACT.MAX_CONTENT_HASH_BYTES,
        }

    SINGLE_SPEC_ARMS = (
        ("a spec missing an identity field", "console", drop_identity),
        ("an unresolvable interface reference", "fabric-publisher", unknown_interface),
        ("a lifecycle state outside the closed set", "console", unknown_lifecycle),
        ("a lifecycle out of canonical order", "console", unordered_lifecycle),
        ("a lifecycle missing a required state", "console", missing_required_lifecycle),
        ("an undeclared capability kind", "console", unknown_capability_kind),
        ("an unsorted capability list", "init", unsorted_capability_kinds),
        ("a semantic no referenced interface backs", "fabric-publisher", wrong_semantic),
        ("a stream interface tagged as a command", "fabric-publisher", mistagged_interface),
        ("a QoS policy naming no declared route", "fabric-publisher", dangling_qos),
        ("retained durability with no retained depth", "fabric-publisher", contradictory_durability),
        ("manual liveliness with no lease", "fabric-publisher", contradictory_liveliness),
        ("a self-dependency", "console", self_dependency),
        ("a parameter default outside its range", "console", default_outside_range),
        ("an unknown compatibility constraint", "console", unknown_platform_constraint),
        ("a platform disagreeing with the runtime", "console", platform_disagreement),
        ("an unknown interface contract directory", "console", unknown_interface_contract),
        ("a real directory that is no contract root", "console", non_contract_interface_path),
        ("a dependency mode disagreeing with the dependencies", "console", wrong_dependency_mode),
        ("a dependency mode disagreeing on a component that has some", "dango", wrong_dependency_mode),
        ("a resource mode that is not a ceiling", "console", wrong_resource_mode),
        ("a parameter default disagreeing with its resource field", "init", parameter_default_disagrees),
        ("a parameter naming no resource field", "console", parameter_names_no_resource),
        ("a criterion no gate literal contains", "console", unobserved_criterion),
        ("a test environment naming no Justfile target", "console", unknown_test_target),
        ("an empty pass/fail criterion", "console", empty_criterion),
        ("a stack that is not a whole number of pages", "console", unpaged_stack),
        ("a spawn budget above the platform ceiling", "console", overbudget_spawn),
        ("buffers declared with no page allowance", "console", buffers_without_pages),
        ("a private-memory quota above the root's reservation", "console", overlarge_private_quota),
        ("a device requirement in neither capability set", "console", undeclared_device),
        ("a malformed version", "console", bad_version),
        ("an unsupported format version", "console", unsupported_format),
        ("an undeclared provider naming a binary", "console", undeclared_with_binary),
        ("a workspace provider naming no binary", "console", workspace_without_binary),
        ("a workspace provider pinning external content", "console", workspace_with_content_hash),
        ("an external provider naming no content hash", "console", external_without_content_hash),
        (
            "an external provider naming a non-canonical content hash",
            "console",
            external_with_malformed_content_hash,
        ),
    )
    for label, name, apply in SINGLE_SPEC_ARMS:
        mutate(label, name, apply)
    REFUSALS = len(SINGLE_SPEC_ARMS)

    # A corpus-level rule needs a corpus-level case: one spec, so every
    # requirement it states is unmet unless the corpus provides it.
    lone = root / "lone"
    lone.mkdir()
    isolated = source_spec("storage-probe")
    isolated["dependencies"] = []
    write_spec(lone, isolated)
    try:
        admit_specs([lone / "storage-probe.zti"], catalogue=CATALOGUE)
    except ComponentSpecError:
        pass
    else:
        fail("a corpus requiring a capability kind nothing provides was accepted")
    REFUSALS += 1

    # And the same corpus rule must not fire on a corpus that does satisfy it,
    # or the arm above would prove nothing about the rule it names.
    paired = root / "paired"
    paired.mkdir()
    provider_spec = source_spec("init")
    provider_spec["dependencies"] = []
    write_spec(paired, provider_spec)
    write_spec(paired, isolated)
    admit_specs(sorted(paired.glob("*.zti")), catalogue=CATALOGUE)

    # Dependency resolution and cycle detection are corpus rules too: a name is
    # undeclared only relative to a corpus, and a cycle needs at least two
    # specs. Each arm pairs a refusal with an admitted baseline of the same
    # shape, so neither can pass by tripping the capability-coverage rule above.
    def corpus_rejected(label: str, directory: str, mutate_pair) -> None:
        scope = root / directory
        scope.mkdir()
        left_spec = source_spec("init")
        left_spec["dependencies"] = []
        right_spec = source_spec("storage-probe")
        right_spec["dependencies"] = []
        admit_specs(
            [write_spec(scope, left_spec), write_spec(scope, right_spec)], catalogue=CATALOGUE
        )
        mutate_pair(left_spec, right_spec)
        paths = [write_spec(scope, left_spec), write_spec(scope, right_spec)]
        try:
            admit_specs(paths, catalogue=CATALOGUE)
        except ComponentSpecError:
            return
        fail(f"{label} was accepted")

    def undeclared_dependency(left_spec: dict, right_spec: dict) -> None:
        right_spec["dependencies"] = ["no-such-component"]

    def dependency_cycle(left_spec: dict, right_spec: dict) -> None:
        left_spec["dependencies"] = [right_spec["name"]]
        right_spec["dependencies"] = [left_spec["name"]]

    def duplicate_binary(left_spec: dict, right_spec: dict) -> None:
        right_spec["implementation"] = copy.deepcopy(left_spec["implementation"])

    def cross_domain_collision(left_spec: dict, right_spec: dict) -> None:
        right_spec["implementation"] = {
            "provider": CONTRACT.PROVIDER_EXTERNAL,
            "binary": left_spec["name"],
            "contentHash": "0" * CONTRACT.MAX_CONTENT_HASH_BYTES,
        }

    corpus_rejected(
        "a dependency on an undeclared component", "undeclared-dep", undeclared_dependency
    )
    corpus_rejected("a dependency cycle", "cycle", dependency_cycle)
    corpus_rejected("two specs naming one implementation binary", "duplicate-binary", duplicate_binary)
    corpus_rejected(
        "a component name colliding with another implementation binary",
        "cross-domain-binary",
        cross_domain_collision,
    )
    REFUSALS += 4

# 7. Two independent runs over the same corpus agree, and the committed corpus
#    is byte-stable under its own normalizer.
again = admit_specs(catalogue=CATALOGUE)
if [entry.identity for entry in again] != [entry.identity for entry in CORPUS]:
    fail("two identical runs computed different component identities")
if len({entry.identity for entry in CORPUS}) != len(CORPUS):
    fail("two committed specs share one identity")

print(
    f"component spec model: {len(CORPUS)} records validated against "
    f"{len(CATALOGUE)} declared interfaces and the reference generation; "
    f"{REFUSALS} named mutations refused, identities stable; "
    f"declared-without-implementation: {', '.join(observed_undeclared)}"
)
