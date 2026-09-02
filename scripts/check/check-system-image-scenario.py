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
import importlib.util
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


def check_scenarios() -> int:
    """Every scenario resolves, differs from its base, and changes only what it names."""
    scenarios = GENERATOR_MODULE.SCENARIOS
    if not scenarios:
        fail("no scenario is declared, so this gate asserts nothing")
    identities: dict[str, str] = {}
    for name, (base_name, parameters) in sorted(scenarios.items()):
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
        if not parameters:
            fail(f"{name}: a scenario with no parameters is its base composition")
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


check_vocabulary()
check_unknown_parameter_refused()
scenario_count = check_scenarios()
check_malformed_refused()
closure_count = check_every_closure_applies()

print(
    f"system image scenario check: the closure contract admits exactly "
    f"{len(EXPECTED_PARAMETERS)} build parameters and refuses any other; "
    f"{scenario_count} scenario closure(s) resolve with identities distinct from their base "
    f"and from each other, changing exactly the manifest fields they name; "
    f"8 malformed parameters refused; all {closure_count} closures' parameters apply to "
    "their own manifests"
)
