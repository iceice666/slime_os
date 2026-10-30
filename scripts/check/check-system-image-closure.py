#!/usr/bin/env python3

"""CP11 canonical image-closure and test-run gate.

`--input-host-paths` runs only the committed-input scan: no file under
`contracts/system-image-closure/v2/inputs/` names an absolute host path, each
committed seL4 prefix names its libsel4 generator inputs under the logical
`/slime/sel4` prefix, and the riscv64 prefix headers hold the pinned bytes.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
import copy
import hashlib
import json
import os
import re
import shutil
import subprocess
import tempfile
from collections.abc import Callable
from pathlib import Path

import system_spec
from harness import ROOT
from system_image_closure import (
    SystemImageClosureError,
    compile_closure,
    compile_test_run,
    resolve_closure,
    tree_identity,
)

CLOSURE = ROOT / "contracts" / "system-image-closure" / "v2" / "closures" / "sel4-channel.zti"
TEST_RUN = ROOT / "contracts" / "system-test-run" / "v2" / "runs" / "sel4-channel.zti"
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
        (
            "missing-prefix",
            lambda value: value["target"]["prefix"].update(path="missing-prefix"),
            "missing tree artifact",
        ),
        (
            "changed-system",
            lambda value: value["systemSpec"].update(identity="0" * 64),
            "identity mismatch",
        ),
        (
            "wrong-target",
            lambda value: value["target"].update(profile="riscv64-sel4-milkv-duo"),
            "target requirement",
        ),
        (
            "wrong-profile",
            lambda value: value["target"].update(platform="milkv-duo"),
            "profile and platform",
        ),
        (
            "wrong-toolchain",
            lambda value: value["target"].update(toolchain="nightly-2099-01-01"),
            "toolchain",
        ),
        (
            "wrong-rust-sel4",
            lambda value: value["target"].update(rustSel4Commit="0" * 40),
            "rust-sel4",
        ),
        (
            "wrong-target-spec",
            lambda value: next(
                entry for entry in value["releaseInputs"] if entry["name"] == "target-spec"
            ).update(
                artifact=copy.deepcopy(
                    next(
                        entry for entry in value["releaseInputs"] if entry["name"] == "root-target"
                    )["artifact"]
                )
            ),
            "target-spec",
        ),
        ("unrecorded-component", lambda value: value["implementations"].pop(), "exactly cover"),
        ("missing-release", lambda value: value["releaseInputs"].pop(), "exactly cover"),
        (
            "missing-delivery-input",
            lambda value: value.update(
                releaseInputs=[
                    entry for entry in value["releaseInputs"] if entry["name"] != "elf-delivery"
                ]
            ),
            "exactly cover",
        ),
        (
            "changed-delivery-input",
            lambda value: next(
                entry for entry in value["releaseInputs"] if entry["name"] == "elf-delivery"
            )["artifact"].update(identity="0" * 64),
            "identity mismatch",
        ),
        (
            "ambient-parameter",
            lambda value: value["buildParameters"].append({"name": "ambient", "value": "1"}),
            "does not admit",
        ),
    )
    for name, mutate, refusal in cases:
        value = copy.deepcopy(compiled.value)
        mutate(value)
        case_root = temporary / name
        case_root.mkdir()
        path = case_root / "sel4-channel.zti"
        write_closure(path, value)
        expect_rejected(path, refusal)


def check_target_spec_path() -> None:
    base = resolve_closure(CLOSURE)
    with tempfile.TemporaryDirectory(prefix=".closure-target-control-", dir=ROOT) as directory:
        temporary = Path(directory)
        original = base.artifacts["release:target-spec"]
        copied = temporary / original.name
        shutil.copyfile(original, copied)
        value = copy.deepcopy(base.compiled.value)
        target_spec = next(
            entry for entry in value["releaseInputs"] if entry["name"] == "target-spec"
        )
        target_spec["artifact"]["path"] = str(copied.relative_to(ROOT))
        path = temporary / CLOSURE.name
        write_closure(path, value)
        expect_rejected(path, "target-spec")


def check_prefix_pins() -> None:
    base = resolve_closure(CLOSURE)
    files = (
        "bin/kernel.elf",
        "libsel4/include/kernel/gen_config.json",
        "libsel4/include/sel4/gen_config.json",
        "support/kernel.dtb",
        "support/platform_gen.yaml",
    )
    with tempfile.TemporaryDirectory(prefix=".closure-prefix-control-", dir=ROOT) as directory:
        temporary = Path(directory)
        prefix = temporary / "prefix"
        shutil.copytree(base.artifacts["prefix"], prefix)
        path = temporary / CLOSURE.name
        for relative in files:
            artifact = prefix / relative
            original = artifact.read_bytes()
            try:
                artifact.write_bytes(original + b"\n")
                value = copy.deepcopy(base.compiled.value)
                value["target"]["prefix"].update(
                    path=str(prefix.relative_to(ROOT)), identity=tree_identity(prefix)
                )
                write_closure(path, value)
                expect_rejected(path, relative)
            finally:
                artifact.write_bytes(original)


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


def check_computed_source_drift() -> None:
    """Default resolution refuses a committed computed system its source no longer renders.

    `closure_image.build` resolves before every build, and a plane gate that
    builds through it need not depend on `system_spec_check`; this refusal is
    what keeps such a build from booting a stale graph. The source's rendering
    is substituted rather than the file edited, so no repository byte changes.
    """
    name = "sel4-call"
    closure = ROOT / "contracts" / "system-image-closure" / "v2" / "closures" / f"{name}.zti"
    if name not in system_spec.COMPUTED_SYSTEM_SOURCES:
        fail(f"{name} is no longer a computed system; choose another control subject")
    committed = (system_spec.SYSTEM_ROOT / f"{name}.zti").read_text(encoding="utf-8")
    original = system_spec.render_computed_system

    def changed_source(system: str, *args, **kwargs) -> str:
        if system == name:
            return committed.replace("generation = ", "generation = 9", 1)
        return original(system, *args, **kwargs)

    system_spec.render_computed_system = changed_source
    try:
        resolve_closure(closure)
    except (SystemImageClosureError, system_spec.SystemSpecError) as error:
        if "drift" not in str(error):
            fail(f"stale computed system refused for the wrong reason: {error}")
    else:
        fail(f"{name}: a committed system its source no longer renders was resolved")
    finally:
        system_spec.render_computed_system = original


def check_repository_metadata(temporary: Path) -> None:
    """A tree input's identity is its content, not how the checkout was made."""
    primary = temporary / "metadata-primary"
    worktree = temporary / "metadata-worktree"
    edited = temporary / "metadata-edited"
    for root in (primary, worktree, edited):
        (root / "src").mkdir(parents=True)
        (root / "src" / "lib.rs").write_text("pub fn loader() {}\n", encoding="utf-8")
    (primary / ".git").write_text("gitdir: ../../.git/modules/deps/rust-sel4\n", encoding="utf-8")
    (worktree / ".git").write_text(
        "gitdir: ../../../slime_os/.git/worktrees/lane/modules/deps/rust-sel4\n", encoding="utf-8"
    )
    (edited / "src" / "lib.rs").write_text("pub fn loader() { unreachable!() }\n", encoding="utf-8")
    if tree_identity(primary) != tree_identity(worktree):
        fail("a tree input's identity depends on its .git gitlink")
    if tree_identity(primary) == tree_identity(edited):
        fail("a tree input's identity ignored a content change")


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


INPUT_ROOT = ROOT / "contracts" / "system-image-closure" / "v2" / "inputs"
# The two libsel4 headers seL4's generators write with their `.bf` input in a leading comment. An
# installed prefix names that input by the checkout's absolute path until `canonicalize_prefix` in
# `scripts/lib/component_sdk.py` rewrites it to the logical source below.
LOGICAL_SEL4_SOURCE = b"/slime/sel4/"
GENERATED_HEADERS = (
    "libsel4/include/sel4/shared_types_gen.h",
    "libsel4/include/sel4/sel4_arch/types_gen.h",
)
# The riscv64 headers as canonicalization writes them: the committed bytes with only the checkout path
# in the first comment replaced by `/slime/sel4`.
PINNED_INPUTS = {
    "sel4-riscv64-prefix/libsel4/include/sel4/shared_types_gen.h":
        "e772d1c961a13b0be439ac7479eac04e07cfbabf704f141cc0c3d31f315ed842",
    "sel4-riscv64-prefix/libsel4/include/sel4/sel4_arch/types_gen.h":
        "8a4480f4b93a5992b825de7dbc214636ab59c5754985efae821c7bf32da81321",
}
HOST_PATH = re.compile(rb"/(?:Users|home)/[^/\s\"'*]+/")


def input_host_path_findings(root: Path, checkout: Path = ROOT) -> list[str]:
    """Findings for committed closure inputs that would not be byte-identical in another checkout."""
    findings = []
    own = str(checkout.resolve()).encode("utf-8")
    for path in sorted(item for item in root.rglob("*") if item.is_file()):
        relative = path.relative_to(root).as_posix()
        data = path.read_bytes()
        match = HOST_PATH.search(data)
        if match is not None:
            findings.append(f"{relative} names the host path {match.group(0).decode(errors='replace')}")
        elif own in data:
            findings.append(f"{relative} names this checkout's path")
    prefixes = sorted(item for item in root.glob("sel4*-prefix") if item.is_dir())
    if not prefixes:
        findings.append("no committed seL4 prefix to scan")
    for prefix in prefixes:
        for header in GENERATED_HEADERS:
            path = prefix / header
            first = path.read_bytes().split(b"\n", 1)[0] if path.is_file() else b""
            if not first.startswith(b"/* generated from " + LOGICAL_SEL4_SOURCE):
                findings.append(f"{prefix.name}/{header} does not name its input under /slime/sel4")
    for relative, digest in PINNED_INPUTS.items():
        path = root / relative
        found = hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else "missing"
        if found != digest:
            findings.append(f"{relative} is {found[:16]}, not the canonicalized bytes {digest[:16]}")
    return findings


def input_host_path_controls() -> int:
    """Each corruption of an honest input tree must produce its own diagnostic; the honest tree must pass.

    The path corruptions go into an unpinned prefix fixture, so the pinned-header digest check cannot
    stand in for the host-path scan or the logical-prefix check. Only the digest control edits a pinned
    header.
    """
    with tempfile.TemporaryDirectory(prefix="slime-closure-input-paths-") as directory:
        root = Path(directory) / "inputs"
        for relative in PINNED_INPUTS:
            target = root / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            canonical = re.sub(rb"/(?:Users|home)/[^ ]*?/deps/sel4/", LOGICAL_SEL4_SOURCE,
                               (INPUT_ROOT / relative).read_bytes(), count=1)
            target.write_bytes(canonical)
        fixture = root / "sel4-fixture-prefix"
        for header in GENERATED_HEADERS:
            path = fixture / header
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"/* generated from /slime/sel4/libsel4/fixture.bf */\n#define FIXTURE 1\n")
        notes = fixture / "notes.txt"
        notes.write_bytes(b"fixture\n")
        checkout = Path(directory) / "checkout"
        honest = input_host_path_findings(root, checkout)
        if honest:
            fail(f"input host-path control: the canonicalized tree was refused: {honest}")
        fixture_header = fixture / GENERATED_HEADERS[0]
        pinned_header = root / next(iter(PINNED_INPUTS))

        def edit(path: Path, data: bytes | None) -> Callable[[], None]:
            def apply() -> None:
                if data is None:
                    path.unlink()
                else:
                    path.write_bytes(data)
            return apply

        cases = (
            ("a macOS home path in an unpinned file", edit(notes, b"/* /Users/someone/src */\n"),
             "sel4-fixture-prefix/notes.txt names the host path /Users/someone/"),
            ("a Linux home path in an unpinned file", edit(notes, b"/* /home/runner/work/x */\n"),
             "sel4-fixture-prefix/notes.txt names the host path /home/runner/"),
            ("the checkout's own path in an unpinned file", edit(notes, b"/* " + str(checkout.resolve()).encode() + b" */\n"),
             "sel4-fixture-prefix/notes.txt names this checkout's path"),
            ("the logical prefix missing from an unpinned header",
             edit(fixture_header, fixture_header.read_bytes().replace(LOGICAL_SEL4_SOURCE, b"/elsewhere/sel4/")),
             f"sel4-fixture-prefix/{GENERATED_HEADERS[0]} does not name its input under /slime/sel4"),
            ("an unpinned generated header missing", edit(fixture / GENERATED_HEADERS[1], None),
             f"sel4-fixture-prefix/{GENERATED_HEADERS[1]} does not name its input under /slime/sel4"),
            ("one changed byte in a pinned header", edit(pinned_header, pinned_header.read_bytes().replace(b"\n", b"\n ", 1)),
             f"{next(iter(PINNED_INPUTS))} is "),
        )
        refused = 0
        for label, apply, expected in cases:
            saved = {path: path.read_bytes() for path in root.rglob("*") if path.is_file()}
            apply()
            findings = input_host_path_findings(root, checkout)
            for path, data in saved.items():
                path.write_bytes(data)
            if not any(expected in finding for finding in findings):
                fail(f"input host-path control: {label} did not produce {expected!r}; it produced {findings}")
            refused += 1
        if input_host_path_findings(root, checkout):
            fail("input host-path control: restoring the honest tree did not clear every finding")
    return refused


def check_input_host_paths() -> None:
    controls = input_host_path_controls()
    findings = input_host_path_findings(INPUT_ROOT)
    if findings:
        fail("committed closure inputs name host paths: " + "; ".join(findings))
    print(
        f"system image closure check: {sum(1 for item in INPUT_ROOT.rglob('*') if item.is_file())} committed "
        f"input file(s) name no host path, {len(PINNED_INPUTS)} pinned header(s) hold canonicalized bytes, "
        f"and {controls} control(s) refused"
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--input-host-paths", action="store_true", help="scan committed closure inputs only")
    arguments = parser.parse_args()
    if arguments.input_host_paths:
        check_input_host_paths()
        return
    resolved = resolve_closure(CLOSURE)
    run = compile_test_run(TEST_RUN)
    if run.value["imageClosure"] != CLOSURE.stem:
        fail("test run does not name the resolved image closure")
    if resolved.artifacts["prefix"].resolve().is_relative_to(ROOT / "build"):
        fail("closure resolved its prefix through ambient build output")
    if (
        tree_identity(resolved.artifacts["prefix"])
        != resolved.compiled.value["target"]["prefix"]["identity"]
    ):
        fail("resolved prefix identity changed after resolution")
    with tempfile.TemporaryDirectory(prefix="slime-system-image-closure-check-") as directory:
        temporary = Path(directory)
        check_refusals(temporary)
        check_prefix_pins()
        check_computed_source_drift()
        check_target_spec_path()
        check_identity_boundaries(temporary)
        check_repository_metadata(temporary)
        check_bounds(temporary)
        check_builds(temporary)
    print(
        "system image closure check: contracts, resolution, identity, isolation, and bytes verified"
    )


if __name__ == "__main__":
    main()
