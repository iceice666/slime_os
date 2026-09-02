#!/usr/bin/env python3

"""Build one seL4 image from a canonical CP11 image closure."""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
import copy
import hashlib
import importlib.util
import json
import os
import shutil
import sys
from pathlib import Path
from types import ModuleType

import system_image_closure_contract as CLOSURE_CONTRACT
from harness import ROOT
from system_image_closure import (
    compile_closure,
    compile_negative_case,
    make_build_result,
    resolve_closure,
)


def load_build_module(name: str, relative: str) -> ModuleType:
    path = ROOT / "scripts" / relative
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise SystemExit(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


GENERATION_BUILDER = load_build_module("system_image_generation_builder", "build/build-generation.py")
SEL4_BUILDER = load_build_module("system_image_sel4_builder", "build/build-sel4.py")


def fail(message: str) -> None:
    raise SystemExit(f"system image build: {message}")


def write_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def apply_parameters(manifest: dict, parameters: dict[str, str]) -> dict:
    """Apply the closure's declared build parameters to the derived manifest.

    These are the three deltas CP14 moved out of the environment. Each is
    validated against what the manifest already declares rather than trusted:
    a parameter naming an undeclared limit, route, participant, or field is a
    refusal, not a silently created graph field. The grammars are the ones
    `build-generation.py` parsed when these arrived as `SLIME_*` variables, so
    a closure and a legacy build express the same delta the same way.
    """
    for name, value in sorted(parameters.items()):
        if name == CLOSURE_CONTRACT.PARAMETER_GENERATION_NUMBER:
            if not value.isdigit() or int(value) <= 0:
                fail(f"{name}: must be a positive integer, not {value!r}")
            manifest["generation"] = int(value)
        elif name == CLOSURE_CONTRACT.PARAMETER_FABRIC_LIMIT_OVERRIDE:
            limit, separator, raw = value.partition("=")
            if not separator or not limit:
                fail(f"{name}: must be <limit>=<value>, not {value!r}")
            limits = manifest.get("fabricGraph", {}).get("limits")
            if not isinstance(limits, dict) or limit not in limits:
                fail(f"{name}: names undeclared limit {limit!r}")
            if not raw.isdigit() or int(raw) <= 0:
                fail(f"{name}: value must be a positive integer, not {raw!r}")
            limits[limit] = int(raw)
        elif name == CLOSURE_CONTRACT.PARAMETER_FABRIC_QOS_OVERRIDE:
            parts = value.split(":")
            if len(parts) != 4 or not all(parts):
                fail(f"{name}: must be <route>:<component>:<field>:<value>, not {value!r}")
            route_name, component, field, setting = parts
            routes = [
                route
                for route in manifest.get("fabricGraph", {}).get("routes", [])
                if route.get("name") == route_name
            ]
            if len(routes) != 1:
                fail(f"{name}: names undeclared route {route_name!r}")
            members = [
                member
                for member in routes[0]["participants"]
                if member.get("component") == component
            ]
            if len(members) != 1:
                fail(
                    f"{name}: names {component!r}, which is not a unique participant "
                    f"of {route_name!r}"
                )
            if field not in members[0]:
                fail(f"{name}: names undeclared field {field!r}")
            members[0][field] = setting
        else:
            fail(f"{name}: not an admitted build parameter")
    return manifest


def clean_environment(*, toolchain: str, prefix: Path, target_profile: str) -> dict[str, str]:
    denied = {
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_TARGET_DIR",
        "RUSTFLAGS",
        "RUSTUP_TOOLCHAIN",
        "SEL4_PREFIX",
        "SLIME_B40_MUTATION",
        "SLIME_COMPONENT_LINKER_DIR",
        "SLIME_GENERATION",
        "SLIME_SEL4_MANIFEST",
        "SLIME_TARGET_PROFILE",
    }
    environment = {
        name: value
        for name, value in os.environ.items()
        if not name.startswith("SLIME_") and name not in denied
    }
    environment["RUSTUP_TOOLCHAIN"] = toolchain
    environment["SEL4_PREFIX"] = str(prefix)
    environment["SLIME_TARGET_PROFILE"] = target_profile
    return environment


# Which compile-time knob each scenario build profile sets. The knobs are the
# `option_env!` names the components already read, so a closure-built scenario
# ELF and a legacy one are the same bytes; what changes is that the selection
# is now in the build key rather than in the caller's environment.
PROFILE_KNOBS: dict[str, tuple[str, str]] = {
    CLOSURE_CONTRACT.BUILD_PROFILE_PROXY_EARLY_EXIT: (
        "SLIME_FABRIC_PROXY_EARLY_EXIT",
        "1",
    ),
    CLOSURE_CONTRACT.BUILD_PROFILE_STREAM_EARLY_EXIT: (
        "SLIME_FABRIC_STREAM_EARLY_EXIT",
        "1",
    ),
    CLOSURE_CONTRACT.BUILD_PROFILE_GENERATION_CMD_BAD_CLOSURE: (
        "SLIME_GENERATION_CMD_SCENARIO",
        "bad-closure",
    ),
    CLOSURE_CONTRACT.BUILD_PROFILE_GENERATION_CMD_BAD_RELEASE: (
        "SLIME_GENERATION_CMD_SCENARIO",
        "bad-release",
    ),
    CLOSURE_CONTRACT.BUILD_PROFILE_BOOT_SELECTION_FAIL: (
        "SLIME_BOOT_SELECTION_FAIL",
        "1",
    ),
    CLOSURE_CONTRACT.BUILD_PROFILE_RECOVERY_IMAGE: ("SLIME_RECOVERY_IMAGE", "1"),
}


def profile_environment(profiles: dict[str, str]) -> dict[str, str]:
    """The compile-time knobs the closure's declared build profiles select.

    A knob is a Cargo-visible `option_env!`, so it applies to every component
    in the invocation rather than to one package. Several *distinct* knobs
    coexist — `sel4-fault` needs both the proxy and the stream death, which is
    why this returns a map rather than one selection — but two profiles that
    set the *same* knob to different values cannot both be honoured, so that is
    refused rather than silently resolved. A closure identity must be a claim
    about the bytes the build produced.
    """
    selected = sorted(
        {
            profile
            for profile in profiles.values()
            if profile != CLOSURE_CONTRACT.BUILD_PROFILE_DEFAULT
        }
    )
    environment: dict[str, str] = {}
    origin: dict[str, str] = {}
    for profile in selected:
        knob, value = PROFILE_KNOBS[profile]
        if knob in environment and environment[knob] != value:
            fail(
                f"build profiles {origin[knob]!r} and {profile!r} both set {knob} to "
                f"different values ({environment[knob]!r}, {value!r}); a compile-time knob "
                "has one value per build"
            )
        environment[knob] = value
        origin[knob] = profile
    return environment


def build(closure: Path, output: Path, *, mutation: str | None = None) -> Path:
    """Build one closure into `output`.

    `mutation` builds a *negative* case: the same closure with one child-CSpace
    perturbation compiled into the root. The output is not a product image and
    is never presented as one — `build_negative` below is the only caller, and
    it writes no image-identity record.
    """
    resolved = resolve_closure(closure)
    value = resolved.compiled.value
    output = output.resolve()
    if output.exists() and any(output.iterdir()):
        fail(f"output directory is not empty: {output}")
    output.mkdir(parents=True, exist_ok=True)
    prefix = resolved.artifacts["prefix"]
    platform_name = value["target"]["platform"]
    platform = SEL4_BUILDER.PLATFORMS.get(platform_name)
    if platform is None:
        fail(f"unsupported platform {platform_name!r}")
    platform = copy.copy(platform)
    object.__setattr__(platform, "prefix_dir", prefix)
    object.__setattr__(platform, "build_dir", output / "sel4-build")

    target_profile = GENERATION_BUILDER.resolve_target_profile(value["target"]["profile"])
    environment = clean_environment(
        toolchain=value["target"]["toolchain"],
        prefix=prefix,
        target_profile=target_profile.name,
    )
    component_target = output / "cargo" / "components"
    old_target = GENERATION_BUILDER.COMPONENTS_TARGET_DIR
    GENERATION_BUILDER.COMPONENTS_TARGET_DIR = component_target
    old_environment = os.environ.copy()
    try:
        os.environ.clear()
        os.environ.update(environment)
        os.environ["SLIME_SEL4_MANIFEST"] = resolved.compiled.identity.hex()
        # CP14: the scenario knobs the closure's build profiles select. Set
        # from resolved closure data rather than inherited, so a scenario ELF
        # is reachable only through the identity that declares it — and the
        # generation builder's `closure` profile strips any it did not get
        # from here.
        os.environ.update(profile_environment(resolved.build_profiles))
        generation_dir = output / "generation"
        generation_dir.mkdir()
        # CP14: the three deltas that used to arrive as ambient environment
        # variables are applied here, from the resolved closure, so the bytes
        # they change are keyed by the identity that selected them. Applied
        # before slot assignment because a narrowed fabric limit can change
        # what the graph declares.
        manifest = apply_parameters(copy.deepcopy(resolved.manifest), resolved.build_parameters)
        manifest = GENERATION_BUILDER.assign_declared_slots(manifest)
        GENERATION_BUILDER.build_sel4_generation(
            generation_dir,
            manifest,
            target_profile,
            resolved.external_components,
            toolchain=value["target"]["toolchain"],
            prefix=prefix,
            build_profile="closure",
        )
        generation = generation_dir / "generation.bin"

        pins = {
            "schema": 1,
            "rust_sel4": {
                "toolchain": value["target"]["toolchain"],
                platform.root_target_key: str(resolved.artifacts["release:root-target"]),
                platform.loader_target_key: "aarch64-unknown-none"
                if platform.architecture == "aarch64"
                else "riscv64imac-unknown-none-elf",
            },
            platform.pins_section: {},
        }
        old_cargo = SEL4_BUILDER.CARGO_BUILD
        old_artifacts = SEL4_BUILDER.ARTIFACTS
        SEL4_BUILDER.CARGO_BUILD = output / "cargo" / "image"
        SEL4_BUILDER.ARTIFACTS = output / "artifacts"
        try:
            child_elf, root_elf, embedded = SEL4_BUILDER.build_application(
                pins,
                platform=platform,
                resolved_generation=(
                    None
                    if resolved.root_role == CLOSURE_CONTRACT.ROOT_ROLE_BOOT_SELECTOR
                    else generation
                ),
                toolchain=value["target"]["toolchain"],
                root_target=resolved.artifacts["release:root-target"],
                child_target=resolved.artifacts["release:target-spec"],
                # CP14: the root's role and its declared parameters, from
                # closure data. No `variant` is passed, so the builder's
                # variant table cannot select anything here.
                closure_root_role=resolved.root_role,
                closure_root_parameters=resolved.root_parameters,
                closure_target_name=(
                    f"closure-{resolved.compiled.identity.hex()[:16]}"
                    if mutation is None
                    else f"negative-{mutation}-{resolved.compiled.identity.hex()[:12]}"
                ),
                closure_root_mutation=mutation,
            )
            # A boot-selector root carries no embedded generation, and an
            # embedded-generation root carries exactly the one this closure
            # resolved. The two claims are opposite and both are checked, which
            # is what keeps a selector image from silently shipping a
            # generation.
            if mutation is not None:
                # A mutated root still embeds its generation; what the case
                # asserts is that the audit refuses the CSpace at boot. So
                # nothing about the embedding is claimed here.
                pass
            elif resolved.root_role == CLOSURE_CONTRACT.ROOT_ROLE_BOOT_SELECTOR:
                if embedded is not None:
                    fail("boot-selector root embedded a generation")
            elif embedded != generation.resolve():
                fail("root build did not embed the resolved generation")
            loader, payload_tool = SEL4_BUILDER.build_loader(
                pins,
                platform,
                toolchain=value["target"]["toolchain"],
                loader_target=pins["rust_sel4"][platform.loader_target_key],
            )
            image = output / "image.elf"
            SEL4_BUILDER.package_image(payload_tool, loader, root_elf, image, platform)
            identity_manifest = output / "image.identity.json"
            image_identity = {
                "schema": 2,
                "kind": "slime-system-image-identity",
                "closureIdentity": resolved.compiled.identity.hex(),
                "systemIdentity": resolved.system.identity.hex(),
                "targetProfile": value["target"]["profile"],
                "platform": value["target"]["platform"],
                "rootRole": resolved.root_role,
                "rootParameters": list(resolved.root_parameters),
                "loaderRole": value["loader"]["role"],
                "generation": {
                    "bytes": generation.stat().st_size,
                    "sha256": hashlib.sha256(generation.read_bytes()).hexdigest(),
                },
                "root": {"path": "root.elf"},
                "loader": {"path": "loader.elf"},
                "image": {
                    "bytes": image.stat().st_size,
                    "sha256": hashlib.sha256(image.read_bytes()).hexdigest(),
                },
            }
        finally:
            SEL4_BUILDER.CARGO_BUILD = old_cargo
            SEL4_BUILDER.ARTIFACTS = old_artifacts
    finally:
        GENERATION_BUILDER.COMPONENTS_TARGET_DIR = old_target
        os.environ.clear()
        os.environ.update(old_environment)

    root_output = output / "root.elf"
    loader_output = output / "loader.elf"
    shutil.copyfile(root_elf, root_output)
    shutil.copyfile(loader, loader_output)
    image_identity["root"].update(
        bytes=root_output.stat().st_size,
        sha256=hashlib.sha256(root_output.read_bytes()).hexdigest(),
    )
    image_identity["loader"].update(
        bytes=loader_output.stat().st_size,
        sha256=hashlib.sha256(loader_output.read_bytes()).hexdigest(),
    )
    if mutation is not None:
        # A negative build is not a product image, so it gets no image-identity
        # record and no build result. Those two artifacts are what every
        # downstream consumer reads as "this is a verified image"; writing them
        # for a deliberately invalid root is exactly the confusion the negative
        # case type exists to prevent. The image itself stays, because its
        # owning gate boots it to observe the refusal.
        for stray in (identity_manifest, image, root_output, loader_output):
            if stray is identity_manifest and stray.exists():
                stray.unlink()
        (output / "negative-image.elf").write_bytes(image.read_bytes())
        image.unlink()
        print(f"system image build: wrote negative image to {output / 'negative-image.elf'}")
        return output
    write_json(identity_manifest, image_identity)
    result, normalized, identity = make_build_result(
        resolved,
        output_root=output,
        generation=generation,
        root=root_output,
        loader=loader_output,
        image=image,
        identity_manifest=identity_manifest,
    )
    write_json(output / "build-result.json", result)
    (output / "build-result.normalized.json").write_bytes(normalized)
    (output / "build-result.identity").write_text(identity + "\n", encoding="utf-8")
    print(f"system image build: wrote closure {resolved.compiled.identity.hex()} to {output}")
    return output


def build_negative(case: Path, output: Path) -> Path:
    """Build one negative case: a valid base closure plus one closed mutation.

    The base is found by identity rather than by path, so a case cannot name a
    closure that does not exist and cannot drift onto a different one silently.
    The result is a deliberately invalid image whose refusal its owning gate
    observes at boot; no image-identity record is written, so nothing here can
    be mistaken for a product image.
    """
    compiled = compile_negative_case(case)
    wanted = compiled.value["baseClosureIdentity"]
    for candidate in sorted(
        (ROOT / "contracts" / "system-image-closure" / "v1" / "closures").glob("*.zti")
    ):
        if compile_closure(candidate).identity.hex() == wanted:
            base = candidate
            break
    else:
        fail(f"{case.stem}: no closure has identity {wanted}")
    result = build(base, output, mutation=compiled.value["mutation"])
    (output / "negative-case.identity").write_text(
        compiled.identity.hex() + "\n", encoding="utf-8"
    )
    print(
        f"system image build: wrote negative case {compiled.identity.hex()} "
        f"({compiled.value['mutation']}) over closure {wanted} to {output}"
    )
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("closure", type=Path)
    parser.add_argument("output_dir", type=Path)
    parser.add_argument(
        "--negative",
        action="store_true",
        help="treat the record as a negative build case rather than a closure",
    )
    arguments = parser.parse_args()
    if arguments.closure.suffix != ".zti":
        fail("closure must be a .zti record")
    if arguments.negative:
        build_negative(arguments.closure, arguments.output_dir)
    else:
        build(arguments.closure, arguments.output_dir)


if __name__ == "__main__":
    main()
