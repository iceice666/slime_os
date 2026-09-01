#!/usr/bin/env python3

"""CP11 canonical image-closure and test-run gate."""

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

from harness import ROOT
from system_image_closure import (
    SystemImageClosureError,
    compile_closure,
    compile_test_run,
    resolve_closure,
    tree_identity,
)

CLOSURE = ROOT / "contracts" / "system-image-closure" / "v1" / "closures" / "sel4-channel.zti"
TEST_RUN = ROOT / "contracts" / "system-test-run" / "v1" / "runs" / "sel4-channel.zti"
BUILDER = ROOT / "scripts" / "build" / "build-system-image.py"


def fail(message: str) -> None:
    raise SystemExit(f"system image closure check: {message}")


def expect_rejected(path: Path, needle: str) -> None:
    try:
        resolve_closure(path)
    except SystemImageClosureError as error:
        if needle not in str(error):
            fail(f"{path.name}: wrong refusal {error!s}; expected {needle!r}")
        return
    fail(f"{path.name}: malformed closure was accepted")


def render_zti(value: object, indent: int = 0) -> str:
    prefix = "  " * indent
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, str):
        return json.dumps(value)
    if isinstance(value, list):
        if not value:
            return "[]"
        rows = ["["]
        for entry in value:
            rows.append(f"{'  ' * (indent + 1)}{render_zti(entry, indent + 1)};")
        rows.append(f"{prefix}]")
        return "\n".join(rows)
    if isinstance(value, dict):
        rows = ["{"]
        for key, entry in value.items():
            rendered = render_zti(entry, indent + 1)
            rows.append(f"{'  ' * (indent + 1)}{key} = {rendered};")
        rows.append(f"{prefix}}}")
        return "\n".join(rows)
    raise TypeError(type(value))


def write_closure(path: Path, value: dict) -> None:
    path.write_text(render_zti(value) + "\n", encoding="utf-8")


def check_refusals(temporary: Path) -> None:
    compiled = compile_closure(CLOSURE)
    cases = (
        ("missing-prefix", lambda value: value["target"]["prefix"].update(path="missing-prefix"), "missing tree artifact"),
        ("changed-system", lambda value: value["systemSpec"].update(identity="0" * 64), "identity mismatch"),
        ("wrong-target", lambda value: value["target"].update(profile="riscv64-sel4-milkv-duo"), "target requirement"),
        ("wrong-profile", lambda value: value["target"].update(platform="milkv-duo"), "profile and platform"),
        ("unrecorded-component", lambda value: value["implementations"].pop(), "exactly cover"),
        ("missing-release", lambda value: value["releaseInputs"].pop(), "exactly cover"),
        ("ambient-parameter", lambda value: value["buildParameters"].append({"name": "ambient", "value": "1"}), "no admitted parameter"),
    )
    for name, mutate, refusal in cases:
        value = copy.deepcopy(compiled.value)
        mutate(value)
        case_root = temporary / name
        case_root.mkdir()
        path = case_root / "sel4-channel.zti"
        write_closure(path, value)
        expect_rejected(path, refusal)


def check_identity_boundaries(temporary: Path) -> None:
    base = compile_closure(CLOSURE)
    executable_fields = (
        lambda value: value["implementations"][0]["artifact"].update(identity="0" * 64),
        lambda value: value["target"].update(toolchain="nightly-2099-01-01"),
        lambda value: value["target"]["prefix"].update(identity="0" * 64),
        lambda value: value["systemSpec"].update(identity="0" * 64),
        lambda value: value["root"].update(role="boot-selector"),
        lambda value: value["releaseInputs"][0]["artifact"].update(identity="0" * 64),
    )
    for index, mutate in enumerate(executable_fields):
        value = copy.deepcopy(base.value)
        mutate(value)
        case_root = temporary / f"identity-{index}"
        case_root.mkdir()
        path = case_root / "sel4-channel.zti"
        write_closure(path, value)
        if compile_closure(path).identity == base.identity:
            fail(f"identity mutation {index} did not change closure identity")
    run = compile_test_run(TEST_RUN)
    changed_run = copy.deepcopy(run.value)
    changed_run["markerContractIdentity"] = "f" * 64
    case_root = temporary / "marker-oracle"
    case_root.mkdir()
    path = case_root / "sel4-channel.zti"
    write_closure(path, changed_run)
    if compile_test_run(path).identity == run.identity:
        fail("marker oracle did not change test-run identity")
    if compile_closure(CLOSURE).identity != base.identity:
        fail("test-run marker oracle changed image-closure identity")


def check_bounds(temporary: Path) -> None:
    closure = copy.deepcopy(compile_closure(CLOSURE).value)
    closure["name"] = "x" * 97
    closure_path = temporary / ("x" * 97 + ".zti")
    write_closure(closure_path, closure)
    try:
        compile_closure(closure_path)
    except SystemImageClosureError:
        pass
    else:
        fail("excessive closure name was accepted")
    run = copy.deepcopy(compile_test_run(TEST_RUN).value)
    run["timeoutSeconds"] = 3601
    run_path = temporary / "sel4-channel.zti"
    write_closure(run_path, run)
    try:
        compile_test_run(run_path)
    except SystemImageClosureError:
        pass
    else:
        fail("excessive test timeout was accepted")


def check_builds(temporary: Path) -> None:
    outputs = [temporary / "build-a", temporary / "build-b"]
    for output in outputs:
        subprocess.run(["python3", str(BUILDER), str(CLOSURE), str(output)], cwd=ROOT, check=True)
    relative_outputs = (
        "generation/generation.bin",
        "root.elf",
        "loader.elf",
        "image.elf",
        "image.identity.json",
        "build-result.normalized.json",
        "build-result.identity",
    )
    for relative in relative_outputs:
        if (outputs[0] / relative).read_bytes() != (outputs[1] / relative).read_bytes():
            fail(f"isolated builds differ at {relative}")

    baseline = temporary / "baseline"
    subprocess.run(
        ["python3", str(BUILDER), str(CLOSURE), str(baseline)],
        cwd=ROOT,
        check=True,
        env={
            **os.environ,
            "SLIME_FABRIC_PROXY_EARLY_EXIT": "1",
            "SLIME_B40_MUTATION": "missing",
            "SLIME_GENERATION_NUMBER": "999",
            "SLIME_TARGET_PROFILE": "wrong",
        },
    )
    for relative in relative_outputs:
        if (outputs[0] / relative).read_bytes() != (baseline / relative).read_bytes():
            fail(f"undeclared environment changed {relative}")


def main() -> None:
    resolved = resolve_closure(CLOSURE)
    run = compile_test_run(TEST_RUN)
    if run.value["imageClosureIdentity"] != resolved.compiled.identity.hex():
        fail("test run does not name the resolved image closure")
    if resolved.artifacts["prefix"].resolve().is_relative_to(ROOT / "build"):
        fail("closure resolved its prefix through ambient build output")
    if tree_identity(resolved.artifacts["prefix"]) != resolved.compiled.value["target"]["prefix"]["identity"]:
        fail("resolved prefix identity changed after resolution")
    with tempfile.TemporaryDirectory(prefix="slime-system-image-closure-check-") as directory:
        temporary = Path(directory)
        check_refusals(temporary)
        check_identity_boundaries(temporary)
        check_bounds(temporary)
        check_builds(temporary)
    print("system image closure check: contracts, resolution, identity, isolation, and bytes verified")


if __name__ == "__main__":
    main()
