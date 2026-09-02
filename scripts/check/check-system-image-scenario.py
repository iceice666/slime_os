#!/usr/bin/env python3

"""CP14 scenario-identity gate.

A *scenario* is one composition built with declared parameters that change
generation bytes. Before CP14 the three such parameters reached the generation
builder as ambient environment variables set from `build-sel4.py`'s
`VARIANT_GENERATION_DELTAS`: `SLIME_GENERATION_NUMBER`,
`SLIME_FABRIC_LIMIT_OVERRIDE`, and `SLIME_FABRIC_QOS_OVERRIDE`. Bytes changed
by an input absent from the build key mean the key describes a different build
than the one that ran, and two scenarios over one composition computed one
identity.

This gate asserts they are now closure data:

1. the contract's admitted parameter vocabulary is exactly the three, so a
   fourth ambient knob cannot appear without a contract change;
2. the resolver refuses a closure naming a parameter outside that set;
3. every scenario closure resolves, and a scenario over a base composition
   computes a different identity than the base and than every other scenario;
4. each scenario's declared parameters change exactly the manifest fields they
   name and nothing else, measured field by field against the base;
5. a parameter naming an undeclared limit, route, participant, or field is
   refused rather than silently creating one, and a non-positive generation
   number is refused;
6. no closure carries a parameter its base composition cannot satisfy — the
   applied manifest is built, so a malformed value fails here rather than at
   image-build time.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import copy
import hashlib
import importlib.util
import json
import subprocess
import sys
import tempfile
from pathlib import Path

import system_image_closure_contract as CONTRACT
from harness import ROOT
from system_image_closure import (
    SystemImageClosureError,
    compile_closure,
    resolve_closure,
)

CLOSURE_ROOT = ROOT / "contracts" / "system-image-closure" / "v1" / "closures"
GENERATOR = ROOT / "scripts" / "generate" / "generate-system-image-closures.py"
BUILDER_SCRIPT = ROOT / "scripts" / "build" / "build-system-image.py"

# The scenario whose bytes the expensive arm proves. `sel4-stream-death` is the
# smallest closure carrying a build profile, so the arm is a real end-to-end
# comparison rather than a sampled one.
BYTE_ARM = "sel4-stream-death"

# The root-role closure whose bytes the expensive arm proves.
ROOT_ROLE_ARM = "sel4-channel-fixture"

EXPECTED_PARAMETERS = ("generationNumber", "fabricLimitOverride", "fabricQosOverride")


def fail(message: str) -> None:
    raise SystemExit(f"system image scenario check: {message}")


def load_builder():
    spec = importlib.util.spec_from_file_location(
        "scenario_image_builder", ROOT / "scripts" / "build" / "build-system-image.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


BUILDER = load_builder()


def load_generator():
    spec = importlib.util.spec_from_file_location("scenario_closure_generator", GENERATOR)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


GENERATOR_MODULE = load_generator()


def flatten(value: object, prefix: str = "") -> dict[str, object]:
    out: dict[str, object] = {}
    if isinstance(value, dict):
        for key, item in value.items():
            out.update(flatten(item, f"{prefix}.{key}"))
    elif isinstance(value, list):
        for index, item in enumerate(value):
            out.update(flatten(item, f"{prefix}[{index}]"))
    else:
        out[prefix] = value
    return out


def applied(resolved) -> dict:
    return BUILDER.apply_parameters(copy.deepcopy(resolved.manifest), resolved.build_parameters)


def check_vocabulary() -> None:
    """The admitted set is exactly the three parameters CP14 names."""
    if tuple(CONTRACT.BUILD_PARAMETERS) != EXPECTED_PARAMETERS:
        fail(
            "the closure contract admits build parameters "
            f"{tuple(CONTRACT.BUILD_PARAMETERS)}; expected {EXPECTED_PARAMETERS}"
        )


def check_unknown_parameter_refused() -> None:
    """A closure naming a parameter outside the admitted set is refused."""
    base = compile_closure(CLOSURE_ROOT / "sel4-traffic.zti")
    value = copy.deepcopy(base.value)
    value["buildParameters"] = [{"name": "slimeAmbientKnob", "value": "1"}]
    with tempfile.TemporaryDirectory(prefix="slime-scenario-") as scope:
        path = Path(scope) / "sel4-traffic.zti"
        path.write_text(GENERATOR_MODULE.render(value) + "\n", encoding="utf-8")
        try:
            compile_closure(path)
        except SystemImageClosureError as error:
            if "does not admit" not in str(error):
                fail(f"wrong refusal for an unadmitted parameter: {error}")
            return
    fail("a closure naming an unadmitted build parameter was accepted")


def check_profile_vocabulary() -> None:
    """The profile vocabulary is closed, and a same-knob conflict is refused."""
    expected = (
        "default",
        "proxyEarlyExit",
        "streamEarlyExit",
        "generationCmdBadClosure",
        "generationCmdBadRelease",
        "bootSelectionFail",
        "recoveryImage",
    )
    if tuple(CONTRACT.BUILD_PROFILES) != expected:
        fail(
            f"the closure contract admits build profiles {tuple(CONTRACT.BUILD_PROFILES)}; "
            f"expected {expected}"
        )
    # Every non-default profile maps to a knob the builder knows.
    unmapped = sorted(
        profile
        for profile in CONTRACT.BUILD_PROFILES
        if profile != CONTRACT.BUILD_PROFILE_DEFAULT and profile not in BUILDER.PROFILE_KNOBS
    )
    if unmapped:
        fail(f"build profile(s) {unmapped} map to no compile-time knob")
    # Distinct knobs coexist; the same knob at two values is refused.
    both = BUILDER.profile_environment(
        {"a": CONTRACT.BUILD_PROFILE_PROXY_EARLY_EXIT, "b": CONTRACT.BUILD_PROFILE_STREAM_EARLY_EXIT}
    )
    if len(both) != 2:
        fail(f"two distinct scenario knobs did not coexist: {both}")
    try:
        BUILDER.profile_environment(
            {
                "a": CONTRACT.BUILD_PROFILE_GENERATION_CMD_BAD_CLOSURE,
                "b": CONTRACT.BUILD_PROFILE_GENERATION_CMD_BAD_RELEASE,
            }
        )
    except SystemExit as error:
        if "one value per build" not in str(error):
            fail(f"wrong refusal for a same-knob profile conflict: {error}")
    else:
        fail("two profiles setting one knob to different values were accepted")


def check_unknown_profile_refused() -> None:
    """A closure naming a profile outside the admitted set is refused."""
    base = compile_closure(CLOSURE_ROOT / "sel4-traffic.zti")
    value = copy.deepcopy(base.value)
    value["implementations"][0]["buildProfile"] = "ambientScenario"
    with tempfile.TemporaryDirectory(prefix="slime-profile-") as scope:
        path = Path(scope) / "sel4-traffic.zti"
        path.write_text(GENERATOR_MODULE.render(value) + "\n", encoding="utf-8")
        try:
            compile_closure(path)
        except SystemImageClosureError as error:
            if "unknown profile" not in str(error):
                fail(f"wrong refusal for an unadmitted profile: {error}")
            return
    fail("a closure naming an unadmitted build profile was accepted")


def check_root_roles() -> int:
    """Root roles are closed, platform-qualified, and change the root ELF.

    A root role is a distinct root *build* over the same composition: the
    selector carries no embedded generation, the fixture root reports its
    capability layout, the unwind root forces B38's construction unwind. Each
    was a `build-sel4.py` variant branch, so each could change root bytes with
    nothing in any build key to say which. This asserts the vocabulary is
    closed, an unadmitted role or parameter is refused, a parameter on the
    wrong platform is refused before Cargo runs, and — the arm that matters —
    a role-only closure builds a different, reproducible root while leaving the
    generation alone.
    """
    expected = ("embedded-generation", "boot-selector", "root-fixture", "reclamation-unwind")
    if tuple(CONTRACT.ROOT_ROLES) != expected:
        fail(f"the contract admits root roles {tuple(CONTRACT.ROOT_ROLES)}; expected {expected}")
    if tuple(CONTRACT.ROOT_PARAMETERS) != ("qemuKeyboard", "duoTestTerminator"):
        fail(f"unexpected root parameters {tuple(CONTRACT.ROOT_PARAMETERS)}")

    base = compile_closure(CLOSURE_ROOT / "sel4-channel.zti")
    for mutate, needle in (
        (lambda v: v["root"].__setitem__("role", "ambientRole"), "unknown role"),
        (lambda v: v["root"].__setitem__("parameters", ["ambientKnob"]), "does not admit"),
        # Platform qualification: the Duo terminator on a QEMU target compiles
        # an address the platform does not have, so it is refused up front.
        (
            lambda v: v["root"].__setitem__("parameters", ["duoTestTerminator"]),
            "requires platform",
        ),
    ):
        value = copy.deepcopy(base.value)
        mutate(value)
        with tempfile.TemporaryDirectory(prefix="slime-root-role-") as scope:
            path = Path(scope) / "sel4-channel.zti"
            path.write_text(GENERATOR_MODULE.render(value) + "\n", encoding="utf-8")
            try:
                resolve_closure(path)
            except SystemImageClosureError as error:
                if needle not in str(error):
                    fail(f"wrong refusal for a root-role mutation: {error}; wanted {needle!r}")
            else:
                fail(f"a closure with an invalid root role/parameter was accepted ({needle})")

    roles = GENERATOR_MODULE.ROOT_ROLE_CLOSURES
    if not roles:
        fail("no root-role closure is declared, so this arm asserts nothing")
    for name, (base_name, role, parameters) in sorted(roles.items()):
        path = CLOSURE_ROOT / f"{name}.zti"
        if not path.is_file():
            fail(f"{name}: declared a root-role closure but has none")
        resolved = resolve_closure(path)
        if resolved.root_role != role:
            fail(f"{name}: resolved root role {resolved.root_role!r} differs from {role!r}")
        if resolved.root_parameters != tuple(sorted(parameters)):
            fail(f"{name}: resolved root parameters differ from the declared")
        if resolved.compiled.identity == compile_closure(
            CLOSURE_ROOT / f"{base_name}.zti"
        ).identity:
            fail(f"{name}: root-role closure and its base share an identity")
    return len(roles)


def check_root_role_bytes(name: str) -> tuple[str, str]:
    """A role-only closure builds a different root, reproducibly, same generation."""
    base_name, _role, _parameters = GENERATOR_MODULE.ROOT_ROLE_CLOSURES[name]

    def build(closure_name: str, output: Path) -> dict:
        process = subprocess.run(
            [
                sys.executable,
                str(BUILDER_SCRIPT),
                str(CLOSURE_ROOT / f"{closure_name}.zti"),
                str(output),
            ],
            cwd=ROOT,
            check=False,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
        )
        if process.returncode != 0:
            tail = "\n".join(process.stdout.strip().splitlines()[-12:])
            fail(f"{closure_name}: build failed:\n{tail}")
        return json.loads((output / "build-result.json").read_text(encoding="utf-8"))

    with tempfile.TemporaryDirectory(prefix=f"slime-root-bytes-{name}-") as scope:
        root = Path(scope)
        left = build(base_name, root / "base")
        right = build(name, root / "role")
        again = build(name, root / "again")
        if left["root"]["sha256"] == right["root"]["sha256"]:
            fail(f"{name}: the root role changed no root byte")
        if right["root"]["sha256"] != again["root"]["sha256"]:
            fail(f"{name}: the role's root ELF is not reproducible")
        # A root role changes the root, never the generation: the graph it
        # admits is its base's.
        if left["generation"]["sha256"] != right["generation"]["sha256"]:
            fail(f"{name}: a root role changed the generation, which it must not")
        if left["closureIdentity"] == right["closureIdentity"]:
            fail(f"{name}: base and role closure share a closure identity")
        return left["root"]["sha256"], right["root"]["sha256"]


def check_scenarios() -> int:
    """Every scenario resolves, differs from its base, and changes only what it names."""
    scenarios = GENERATOR_MODULE.SCENARIOS
    if not scenarios:
        fail("no scenario is declared, so this gate asserts nothing")
    identities: dict[str, str] = {}
    for name, (base_name, parameters, profiles) in sorted(scenarios.items()):
        scenario_path = CLOSURE_ROOT / f"{name}.zti"
        base_path = CLOSURE_ROOT / f"{base_name}.zti"
        if not scenario_path.is_file():
            fail(f"{name}: declared as a scenario but has no closure")
        if not base_path.is_file():
            fail(f"{name}: base composition {base_name} has no closure")
        scenario = resolve_closure(scenario_path)
        base = resolve_closure(base_path)
        if scenario.build_parameters != parameters:
            fail(
                f"{name}: resolved parameters {scenario.build_parameters} differ from the "
                f"declared {parameters}"
            )
        # The resolved build profiles are the declared ones, and every other
        # component stays `default`: a scenario names the components whose
        # bytes change, so a profile leaking onto an unnamed component would
        # make the closure identity a claim about the wrong ELFs.
        resolved_profiles = {
            component: profile
            for component, profile in scenario.build_profiles.items()
            if profile != CONTRACT.BUILD_PROFILE_DEFAULT
        }
        if resolved_profiles != profiles:
            fail(
                f"{name}: resolved build profiles {resolved_profiles} differ from the "
                f"declared {profiles}"
            )
        for component in sorted(profiles):
            if component not in base.build_profiles:
                fail(f"{name}: profile names {component!r}, which its base does not admit")
            if base.build_profiles[component] != CONTRACT.BUILD_PROFILE_DEFAULT:
                fail(f"{name}: base composition already carries a profile for {component!r}")
        # Two profiles setting one knob to different values cannot both be
        # honoured, so the builder refuses that rather than resolving it.
        BUILDER.profile_environment(scenario.build_profiles)
        if not parameters and not profiles:
            fail(f"{name}: a scenario with no parameters and no profiles is its base")
        scenario_identity = scenario.compiled.identity.hex()
        if scenario_identity == base.compiled.identity.hex():
            fail(f"{name}: scenario and base compute the same closure identity")
        if scenario_identity in identities.values():
            other = next(k for k, v in identities.items() if v == scenario_identity)
            fail(f"{name} and {other} compute the same closure identity")
        identities[name] = scenario_identity

        # The parameters change exactly the fields they name.
        left = flatten(applied(base))
        right = flatten(applied(scenario))
        changed = sorted(key for key in set(left) | set(right) if left.get(key) != right.get(key))
        expected = set()
        for parameter, value in parameters.items():
            if parameter == CONTRACT.PARAMETER_GENERATION_NUMBER:
                expected.add(".generation")
            elif parameter == CONTRACT.PARAMETER_FABRIC_LIMIT_OVERRIDE:
                expected.add(f".fabricGraph.limits.{value.partition('=')[0]}")
            elif parameter == CONTRACT.PARAMETER_FABRIC_QOS_OVERRIDE:
                route, component, field, _ = value.split(":")
                graph = applied(base)["fabricGraph"]
                index = next(
                    (
                        (r_index, p_index)
                        for r_index, r in enumerate(graph["routes"])
                        if r["name"] == route
                        for p_index, p in enumerate(r["participants"])
                        if p["component"] == component
                    ),
                    None,
                )
                if index is None:
                    fail(f"{name}: QoS override names no participant in the base graph")
                expected.add(
                    f".fabricGraph.routes[{index[0]}].participants[{index[1]}].{field}"
                )
        if set(changed) != expected:
            fail(
                f"{name}: declared parameters changed {changed}, expected exactly "
                f"{sorted(expected)}"
            )
    return len(scenarios)


def check_malformed_refused() -> None:
    """A parameter naming something undeclared is refused, not silently created."""
    base = resolve_closure(CLOSURE_ROOT / "sel4-traffic.zti")
    for parameters, needle in (
        ({CONTRACT.PARAMETER_FABRIC_LIMIT_OVERRIDE: "notALimit=2"}, "undeclared limit"),
        ({CONTRACT.PARAMETER_FABRIC_LIMIT_OVERRIDE: "inFlightOperations=0"}, "positive integer"),
        ({CONTRACT.PARAMETER_FABRIC_LIMIT_OVERRIDE: "inFlightOperations"}, "<limit>=<value>"),
        ({CONTRACT.PARAMETER_GENERATION_NUMBER: "0"}, "positive integer"),
        ({CONTRACT.PARAMETER_GENERATION_NUMBER: "-3"}, "positive integer"),
        ({CONTRACT.PARAMETER_FABRIC_QOS_OVERRIDE: "nope:x:y:z"}, "undeclared route"),
        ({CONTRACT.PARAMETER_FABRIC_QOS_OVERRIDE: "telemetry:nobody:reliability:x"}, "unique participant"),
        ({CONTRACT.PARAMETER_FABRIC_QOS_OVERRIDE: "a:b:c"}, "<route>:<component>:<field>:<value>"),
    ):
        try:
            BUILDER.apply_parameters(copy.deepcopy(base.manifest), parameters)
        except SystemExit as error:
            if needle not in str(error):
                fail(f"{parameters}: wrong refusal {error}; expected {needle!r}")
            continue
        fail(f"{parameters}: a malformed build parameter was accepted")


def check_every_closure_applies() -> int:
    """Every closure's parameters apply to its own manifest without refusal."""
    count = 0
    for path in sorted(CLOSURE_ROOT.glob("*.zti")):
        resolved = resolve_closure(path)
        try:
            applied(resolved)
        except SystemExit as error:
            fail(f"{path.stem}: declared parameters do not apply to its own manifest: {error}")
        count += 1
    return count


def check_profile_bytes(name: str) -> tuple[str, str]:
    """A scenario profile changes the ELF it names, reproducibly, and nothing else.

    The expensive arm, and the one that makes the rest more than bookkeeping:
    a profile that were merely recorded in the identity while changing no bytes
    would satisfy every assertion above. This builds the base and the scenario
    and compares the actual component ELFs.
    """
    base_name, _parameters, profiles = GENERATOR_MODULE.SCENARIOS[name]
    if not profiles:
        fail(f"{name}: selected for the byte arm but declares no build profile")
    component = sorted(profiles)[0]

    def build(closure_name: str, output: Path) -> Path:
        process = subprocess.run(
            [sys.executable, str(BUILDER_SCRIPT), str(CLOSURE_ROOT / f"{closure_name}.zti"), str(output)],
            cwd=ROOT,
            check=False,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
        )
        if process.returncode != 0:
            tail = "\n".join(process.stdout.strip().splitlines()[-12:])
            fail(f"{closure_name}: build failed:\n{tail}")
        return output

    def elf(output: Path, executable: str) -> str:
        hits = sorted(output.glob(f"cargo/components/**/release/{executable}.elf"))
        if not hits:
            fail(f"{output}: built no {executable}.elf")
        return hashlib.sha256(hits[0].read_bytes()).hexdigest()

    with tempfile.TemporaryDirectory(prefix=f"slime-profile-bytes-{name}-") as scope:
        root = Path(scope)
        base = build(base_name, root / "base")
        scenario = build(name, root / "scenario")
        again = build(name, root / "again")

        base_elf, scenario_elf, again_elf = (
            elf(base, component),
            elf(scenario, component),
            elf(again, component),
        )
        if base_elf == scenario_elf:
            fail(f"{name}: the {component} profile changed no ELF byte")
        if scenario_elf != again_elf:
            fail(f"{name}: the scenario {component} ELF is not reproducible")

        # A component the profile does not name is byte-identical, so the knob
        # did not leak across the graph.
        untouched = sorted(
            entry
            for entry, profile in resolve_closure(
                CLOSURE_ROOT / f"{name}.zti"
            ).build_profiles.items()
            if profile == CONTRACT.BUILD_PROFILE_DEFAULT
        )
        for entry in untouched[:3]:
            if elf(base, entry) != elf(scenario, entry):
                fail(f"{name}: {entry} changed although no profile names it")

        left = json.loads((base / "build-result.json").read_text(encoding="utf-8"))
        right = json.loads((scenario / "build-result.json").read_text(encoding="utf-8"))
        if left["image"]["sha256"] == right["image"]["sha256"]:
            fail(f"{name}: base and scenario produced the same image")
        if left["closureIdentity"] == right["closureIdentity"]:
            fail(f"{name}: base and scenario share a closure identity")
        return base_elf, scenario_elf


check_vocabulary()
check_profile_vocabulary()
check_unknown_parameter_refused()
check_unknown_profile_refused()
scenario_count = check_scenarios()
check_malformed_refused()
closure_count = check_every_closure_applies()
base_elf, scenario_elf = check_profile_bytes(BYTE_ARM)
role_count = check_root_roles()
base_root, role_root = check_root_role_bytes(ROOT_ROLE_ARM)

print(
    f"system image scenario check: the closure contract admits exactly "
    f"{len(EXPECTED_PARAMETERS)} build parameters and refuses any other; "
    f"{scenario_count} scenario closure(s) resolve with identities distinct from their base "
    f"and from each other, changing exactly the manifest fields they name; "
    f"8 malformed parameters refused; the {len(CONTRACT.BUILD_PROFILES)}-name build-profile "
    "vocabulary is closed with a same-knob conflict and an unadmitted profile both refused; "
    f"all {closure_count} closures' parameters apply to their own manifests; and "
    f"{BYTE_ARM}'s profile moved its component ELF from {base_elf[:12]} to "
    f"{scenario_elf[:12]} reproducibly, leaving unnamed components and the base image "
    "byte-identical; the 4-name root-role vocabulary is closed with an unadmitted role, "
    f"an unadmitted parameter, and a wrong-platform parameter all refused, {role_count} "
    f"root-role closure(s) resolving distinctly from their bases, and {ROOT_ROLE_ARM} moving "
    f"root.elf from {base_root[:12]} to {role_root[:12]} reproducibly with its generation "
    "unchanged"
)
