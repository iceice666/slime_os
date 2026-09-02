#!/usr/bin/env python3

"""Emit one `contracts/system-image-closure/v1` closure per derived composition.

CP11 authored one closure by hand to prove the contract. CP13 needs one per
composition, and hand-authoring 40 records whose every field is a digest of
repository state would be a corpus nobody could keep current: each closure
names the system spec's identity, every component spec's identity, every
implementation tree's identity, and fifteen shared workspace build inputs.

So the closures are generated. What makes that sound rather than circular is
that generation and *resolution* are separate: this script reads repository
state and writes the identities down, while `scripts/lib/system_image_closure.py`
independently re-reads every path and refuses any digest that disagrees. A
closure this script emits from a dirty tree fails to resolve; one it emits from
a clean tree resolves and is the reproducible build key CP11 defined.

`--check` fails on drift, so a closure edited by hand instead of regenerated is
a gate failure, the same discipline `generate-generation-from-spec.py` applies
to the composition corpus.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
import json
import tempfile
from pathlib import Path

import system_image_closure_contract as CONTRACT
from component_sdk import tree_digest
from component_spec import admit_specs, interface_catalogue
from harness import ROOT
from system_image_closure import artifact_identity
from system_spec import (
    DERIVED_GENERATION_FIXTURES,
    SYSTEM_ROOT,
    compile_system,
)

CLOSURE_ROOT = ROOT / "contracts" / "system-image-closure" / "v1" / "closures"
INPUT_ROOT = ROOT / "contracts" / "system-image-closure" / "v1" / "inputs"
SDK_RELEASE = INPUT_ROOT / "sdk-release.json"
PREFIX = INPUT_ROOT / "sel4-prefix"

# The fifteen shared workspace inputs `resolve_closure` requires of every
# closure, as `(name, repository-relative path, kind)`. Declared here in one
# place because the resolver checks the set exactly; a build input added there
# must be added here or every closure stops resolving.
RELEASE_INPUTS: tuple[tuple[str, str, str], ...] = (
    ("boot-contracts", "boot-contracts", "tree"),
    ("cargo-lock", "Cargo.lock", "file"),
    ("component-build-support", "components/build-support", "tree"),
    ("component-cargo-config", "components/.cargo/config.toml", "file"),
    ("component-library", "components/lib", "tree"),
    ("component-proto", "components/proto", "tree"),
    ("component-runtime", "components/runtime", "tree"),
    ("component-spec-contract", "contracts/component-spec/v1", "tree"),
    ("interface-schema-contract", "contracts/interface-schema/v1", "tree"),
    ("just-recipes", "just", "tree"),
    ("justfile", "Justfile", "file"),
    ("root-child", "slime-root/child", "tree"),
    (
        "root-target",
        "deps/rust-sel4/support/targets/aarch64-sel4-roottask-minimal.json",
        "file",
    ),
    ("target-spec", "deps/rust-sel4/support/targets/aarch64-sel4-minimal.json", "file"),
    ("workspace-manifest", "Cargo.toml", "file"),
)

ROOT_IMPLEMENTATION = ("slime-root", "tree")
LOADER_IMPLEMENTATION = ("deps/rust-sel4", "tree")

# Which compositions get a closure. Every derived composition except the
# reference generation, which targets `x86_64-qemu-virtio` and has no seL4
# platform asset to name.
EXCLUDED = {"reference"}



# CP14: scenario closures. A scenario is one base composition plus declared
# build parameters that change generation bytes — the three deltas that used to
# reach the generation builder as ambient `SLIME_*` variables set from
# `build-sel4.py`'s `VARIANT_GENERATION_DELTAS`. Each becomes its own closure
# with its own identity, so "the saturation image" is a build key rather than
# an environment an operator remembered to set.
#
# The numbers are the ones that table declares, so each scenario's generation
# identity is unchanged by becoming closure data. `matrix-unsatisfiable` has no
# entry because its base composition (`sel4-matrix`) is not derived yet.
# Each entry is `(base composition, build parameters, {component: profile})`.
# The profiles are the executable-changing scenarios: `build-sel4.py` set the
# same knobs per variant from `FAULT_VARIANT`/`STREAM_DEATH_VARIANTS`, so the
# ELF bytes are unchanged by the selection becoming closure data.
SCENARIOS: dict[str, tuple[str, dict[str, str], dict[str, str]]] = {
    "sel4-saturation": (
        "sel4-traffic",
        {"generationNumber": "39", "fabricLimitOverride": "inFlightOperations=2"},
        {},
    ),
    # C8.14's degradation envelope: the interposition hop dies mid-route *and*
    # a publisher ends its stream early, which is why one closure carries two
    # distinct scenario profiles.
    "sel4-fault": (
        "sel4-traffic",
        {"generationNumber": "40"},
        {"fabric-proxy": "proxyEarlyExit", "fabric-publisher": "streamEarlyExit"},
    ),
    # C8.4's mid-stream publisher death, without the fault plane's proxy death:
    # the two must stay distinguishable, which they cannot be if one image
    # carries both.
    "sel4-stream-death": (
        "sel4-stream",
        {},
        {"fabric-publisher": "streamEarlyExit"},
    ),
}

# CP14 root roles. A root role is a distinct root *build* over the same
# composition: the selector carries no embedded generation and reads one from
# disk, the fixture root reports its capability layout, the unwind root forces
# B38's construction unwind. Each was a `build-sel4.py` variant branch; each is
# now its own closure with its own identity.
#
# `(base composition, root role, root parameters)`. The base is the graph the
# role's own gate boots, and the role changes only the root.
ROOT_ROLE_CLOSURES: dict[str, tuple[str, str, tuple[str, ...]]] = {
    "sel4-reclamation-unwind": ("sel4-reclamation", "reclamation-unwind", ()),
    "sel4-channel-fixture": ("sel4-channel", "root-fixture", ()),
}


def fail(message: str) -> None:
    raise SystemExit(f"system image closure generation: {message}")


def identity_of(relative: str, kind: str) -> str:
    path = ROOT / relative
    if kind == "tree":
        if not path.is_dir():
            fail(f"missing tree input: {relative}")
        return tree_digest(path)
    if not path.is_file():
        fail(f"missing file input: {relative}")
    return artifact_identity(path, "file")


def artifact(relative: str, kind: str) -> dict:
    return {"path": relative, "kind": kind, "identity": identity_of(relative, kind)}


def implementation_path(spec: dict) -> tuple[str, str]:
    """Where a component's implementation lives, and how it is identified.

    A `workspace` provider is identified by its crate tree, which is what a
    rebuild reads. An `external` provider is identified by the ELF the
    component spec already pins by content hash. An `undeclared` provider has
    no implementation to name, so a composition admitting one gets no closure.
    """
    from component_paths import crate_path

    provider = spec["implementation"]["provider"]
    if provider == CONTRACT.PROVIDER_WORKSPACE:
        crate = crate_path(spec["implementation"]["binary"])
        return str(crate.relative_to(ROOT)), "tree"
    raise LookupError(provider)


def render(value: object, indent: int = 0) -> str:
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
        rows = "".join(f"{padding}  {render(item, indent + 2)};\n" for item in value)
        return "[\n" + rows + padding + "]"
    if isinstance(value, dict):
        rows = "".join(
            f"{padding}  {key} = {render(item, indent + 2)};\n" for key, item in value.items()
        )
        return "{\n" + rows + padding + "}"
    raise TypeError(type(value))


def sdk_profile(profile_name: str) -> dict:
    record = json.loads(SDK_RELEASE.read_text(encoding="utf-8"))
    matches = [entry for entry in record.get("profiles", []) if entry.get("profile") == profile_name]
    if len(matches) != 1:
        fail(f"SDK release declares {len(matches)} profiles named {profile_name!r}")
    return {"record": record, "profile": matches[0]}


def closure_for(
    name: str,
    specs: dict,
    components: dict,
    *,
    base: str | None = None,
    parameters: dict[str, str] | None = None,
    profiles: dict[str, str] | None = None,
    root_role: str | None = None,
    root_parameters: tuple[str, ...] = (),
) -> dict | None:
    """One closure, or `None` when the composition cannot have a reproducible one.

    `base` and `parameters` build a *scenario* closure: the same composition
    with declared build parameters that change generation bytes. The closure's
    own `name` is the scenario's, so two scenarios over one composition are two
    build keys rather than one identity an environment variable disambiguated.
    """
    source = base or name
    system = compile_system(SYSTEM_ROOT / f"{source}.zti", components=components)
    profile_name = system.spec["targetRequirement"]
    if not profile_name.startswith("aarch64-sel4"):
        return None

    implementations = []
    for component in system.spec["components"]:
        spec = specs[component]
        try:
            relative, kind = implementation_path(spec.spec)
        except LookupError as error:
            # `undeclared` and `external` providers: the former has no
            # implementation to name at all, and the latter's ELF is not a
            # committed artifact in this repository, so neither can be a
            # closure input without inventing a path.
            print(
                f"  {name}: no closure — {component} has a {error.args[0]!r} implementation",
                flush=True,
            )
            return None
        implementations.append(
            {
                "component": component,
                "provider": spec.spec["implementation"]["provider"],
                "artifact": artifact(relative, kind),
                "identity": spec.identity.hex(),
                "buildProfile": (profiles or {}).get(component, "default"),
            }
        )
    implementations.sort(key=lambda entry: entry["component"])

    sdk = sdk_profile(profile_name)
    record, profile = sdk["record"], sdk["profile"]
    return {
        "formatVersion": CONTRACT.FORMAT_VERSION,
        "name": name,
        "systemSpec": artifact(
            str((SYSTEM_ROOT / f"{source}.zti").relative_to(ROOT)), "file"
        ),
        "systemIdentity": system.identity.hex(),
        "implementations": implementations,
        "target": {
            "profile": profile_name,
            "platform": profile["platform"],
            "sdkRelease": artifact(str(SDK_RELEASE.relative_to(ROOT)), "file"),
            "prefix": artifact(str(PREFIX.relative_to(ROOT)), "tree"),
            "toolchain": record["toolchain"],
            "rustSel4Commit": record["rustSel4"]["commit"],
        },
        "root": {
            "role": root_role or CONTRACT.ROOT_ROLE_EMBEDDED_GENERATION,
            "implementation": artifact(*ROOT_IMPLEMENTATION),
            "parameters": sorted(root_parameters),
        },
        "loader": {
            "role": CONTRACT.LOADER_ROLE_KERNEL_LOADER,
            "implementation": artifact(*LOADER_IMPLEMENTATION),
            "parameters": [],
        },
        "releaseInputs": [
            {"name": input_name, "artifact": artifact(relative, kind)}
            for input_name, relative, kind in RELEASE_INPUTS
        ],
        "buildParameters": [
            {"name": key, "value": parameters[key]} for key in sorted(parameters or {})
        ],
        "expectedOutputs": list(CONTRACT.OUTPUT_CLASSES),
    }


def outputs() -> dict[Path, str]:
    catalogue = interface_catalogue()
    compiled = admit_specs(catalogue=catalogue)
    specs = {entry.name: entry for entry in compiled}
    components = {entry.name: entry.spec for entry in compiled}
    emitted: dict[Path, str] = {}
    for name in sorted(DERIVED_GENERATION_FIXTURES):
        if name in EXCLUDED:
            continue
        closure = closure_for(name, specs, components)
        if closure is None:
            continue
        emitted[CLOSURE_ROOT / f"{name}.zti"] = render(closure) + "\n"
    for name, (base, parameters, profiles) in sorted(SCENARIOS.items()):
        if base not in DERIVED_GENERATION_FIXTURES:
            fail(f"scenario {name} names composition {base!r}, which is not derived")
        if not parameters and not profiles:
            fail(f"scenario {name} declares no parameters and no profiles, so it is its base")
        closure = closure_for(
            name, specs, components, base=base, parameters=parameters, profiles=profiles
        )
        if closure is None:
            fail(f"scenario {name} produced no closure, but its base composition has one")
        emitted[CLOSURE_ROOT / f"{name}.zti"] = render(closure) + "\n"
    for name, (base, role, parameters) in sorted(ROOT_ROLE_CLOSURES.items()):
        if base not in DERIVED_GENERATION_FIXTURES:
            fail(f"root-role closure {name} names composition {base!r}, which is not derived")
        if role == CONTRACT.ROOT_ROLE_EMBEDDED_GENERATION:
            fail(f"root-role closure {name} declares the ordinary role, so it is its base")
        closure = closure_for(
            name, specs, components, base=base, root_role=role, root_parameters=parameters
        )
        if closure is None:
            fail(f"root-role closure {name} produced none, but its base composition has one")
        emitted[CLOSURE_ROOT / f"{name}.zti"] = render(closure) + "\n"
    if not emitted:
        fail("no composition produced a closure")
    return emitted


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="fail if a closure is stale")
    arguments = parser.parse_args()
    emitted = outputs()
    if arguments.check:
        stale = [
            path
            for path, contents in emitted.items()
            if not path.is_file() or path.read_text(encoding="utf-8") != contents
        ]
        orphaned = sorted(
            path.name for path in CLOSURE_ROOT.glob("*.zti") if path not in emitted
        )
        if stale or orphaned:
            raise SystemExit(
                "stale closure(s): "
                + ", ".join(sorted(path.name for path in stale))
                + ("; orphaned: " + ", ".join(orphaned) if orphaned else "")
                + "; run python3 scripts/generate/generate-system-image-closures.py"
            )
        print(f"{len(emitted)} system-image closures are current")
        return
    CLOSURE_ROOT.mkdir(parents=True, exist_ok=True)
    for path in CLOSURE_ROOT.glob("*.zti"):
        if path not in emitted:
            path.unlink()
            print(f"Removed {path.relative_to(ROOT)}")
    for path, contents in emitted.items():
        handle = tempfile.NamedTemporaryFile(
            "w", encoding="utf-8", dir=path.parent, delete=False, suffix=".tmp"
        )
        temporary = Path(handle.name)
        try:
            with handle:
                handle.write(contents)
            temporary.replace(path)
        except BaseException:
            temporary.unlink(missing_ok=True)
            raise
        print(f"Generated {path.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
