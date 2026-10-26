#!/usr/bin/env python3

"""CP11 canonical image-closure and test-run gate."""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import copy
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from types import ModuleType

import system_spec
from harness import ROOT
from system_image_closure import (
    SystemImageClosureError,
    artifact_identity,
    compile_closure,
    compile_test_run,
    resolve_closure,
    tree_identity,
)

CLOSURE = ROOT / "contracts" / "system-image-closure" / "v2" / "closures" / "sel4-channel.zti"
TEST_RUN = ROOT / "contracts" / "system-test-run" / "v1" / "runs" / "sel4-channel.zti"
BUILDER = ROOT / "scripts" / "build" / "build-system-image.py"
CLOSURE_GENERATOR = ROOT / "scripts" / "generate" / "generate-system-image-closures.py"
RUN_GENERATOR = ROOT / "scripts" / "generate" / "generate-system-test-runs.py"
JUSTFILE = ROOT / "Justfile"
RECIPE_TREE = ROOT / "just"
# The outputs two builds of one closure must agree on, byte for byte.
BUILD_OUTPUTS = (
    "generation/generation.bin",
    "root.elf",
    "loader.elf",
    "image.elf",
    "image.identity.json",
    "build-result.normalized.json",
    "build-result.identity",
)
# The recipe control's in-place change. Both are recognisable, so a run that
# finds them left by a killed run removes them before it starts.
RECIPE_CONTROL_LINE = (
    b"# system_image_closure_check recipe control: removed when the control ends\n"
)
RECIPE_CONTROL_FILE = RECIPE_TREE / "closure-recipe-control.marker"
RECIPE_CONTROL_RECIPE = "closure_recipe_control_marker"
# The `just` the recipe control puts first on the build's `PATH`. It serves the
# JSON recipe dump with one extra recipe and refuses every other invocation, so
# a builder that ran a recipe fails the build, and one that read the dump sees a
# changed recipe table.
RECIPE_CONTROL_SHIM = """
import json
import subprocess
import sys

arguments = sys.argv[1:]
dump = "--dump" in arguments and any(
    argument == "--dump-format=json"
    or (argument == "--dump-format" and arguments[index + 1 : index + 2] == ["json"])
    for index, argument in enumerate(arguments)
)
with open(LOG, "a", encoding="utf-8") as log:
    log.write(json.dumps({"dump": dump, "arguments": arguments}) + "\\n")
if not dump:
    sys.exit(f"recipe control: refused just {arguments}; an image build runs no recipe")
process = subprocess.run([JUST, *arguments], capture_output=True, text=True, check=False)
if process.returncode != 0:
    sys.stderr.write(process.stderr)
    sys.exit(process.returncode)
metadata = json.loads(process.stdout)
recipes = metadata["recipes"]
recipes[RECIPE] = {
    **recipes[min(recipes)],
    "name": RECIPE,
    "namepath": RECIPE,
    "body": [["false"]],
    "dependencies": [],
    "doc": "recipe control",
}
sys.stdout.write(json.dumps(metadata))
"""


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


def load_script(name: str, path: Path) -> ModuleType:
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        fail(f"cannot load {path.relative_to(ROOT)}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def with_release_input(value: dict, name: str, relative: str, kind: str) -> None:
    """Make `value` name exactly one `name` release input: `relative` at its real identity."""
    entries = [entry for entry in value["releaseInputs"] if entry["name"] != name]
    entries.append(
        {
            "name": name,
            "artifact": {
                "path": relative,
                "kind": kind,
                "identity": artifact_identity(ROOT / relative, kind),
            },
        }
    )
    value["releaseInputs"] = sorted(entries, key=lambda entry: entry["name"])


def reads_recipes(relative: str) -> bool:
    """Whether a closure input at `relative` is, lies under, or contains `just/` or `Justfile`."""
    path = (ROOT / relative).resolve()
    return any(
        path.is_relative_to(recipes) or recipes.is_relative_to(path)
        for recipes in (RECIPE_TREE.resolve(), JUSTFILE.resolve())
    )


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
        # The recipes are no release input: a closure naming either, even at
        # its true identity, carries an input the resolver does not admit.
        (
            "recipe-tree-input",
            lambda value: with_release_input(value, "just-recipes", "just", "tree"),
            "extra=['just-recipes']",
        ),
        (
            "justfile-input",
            lambda value: with_release_input(value, "justfile", "Justfile", "file"),
            "extra=['justfile']",
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
    for relative in BUILD_OUTPUTS:
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
    for relative in BUILD_OUTPUTS:
        if (outputs[0] / relative).read_bytes() != (baseline / relative).read_bytes():
            fail(f"undeclared environment changed {relative}")


def check_recipe_inputs_absent() -> None:
    """No release input reads the recipes; devloop's `codePaths` pins them instead."""
    generator = load_script("closure_recipe_generator", CLOSURE_GENERATOR)
    named = sorted(
        name for name, relative, _kind in generator.RELEASE_INPUTS if reads_recipes(relative)
    )
    if named:
        fail(f"release inputs {named} read the recipes, so a recipe change moves every closure")


def check_recipe_independence(temporary: Path) -> None:
    """A recipe-only change leaves every committed closure and test-run record current.

    The generator renders the corpus with every identity it takes of `just/` or
    `Justfile` read from changed copies, and every emitted file must equal the
    committed one, in both directions as `--check` compares them. The test-run
    records are then rendered against those closures. The identities are
    substituted rather than the files edited, so no repository byte changes.
    """
    changed_tree = temporary / "recipe-exam" / "just"
    shutil.copytree(RECIPE_TREE, changed_tree)
    for path in changed_tree.rglob("*"):
        if path.is_file():
            path.write_bytes(path.read_bytes() + b"\n# recipe exam\n")
    (changed_tree / "recipe-exam.just").write_text("# recipe exam\n", encoding="utf-8")
    changed_justfile = temporary / "recipe-exam" / "Justfile"
    changed_justfile.write_bytes(JUSTFILE.read_bytes() + b"\n# recipe exam\n")

    generator = load_script("closure_recipe_generator", CLOSURE_GENERATOR)
    original = generator.identity_of

    def changed_identity(relative: str, kind: str) -> str:
        path = (ROOT / relative).resolve()
        if path == JUSTFILE.resolve():
            return artifact_identity(changed_justfile, kind)
        if path.is_relative_to(RECIPE_TREE.resolve()):
            return artifact_identity(changed_tree / path.relative_to(RECIPE_TREE.resolve()), kind)
        if reads_recipes(relative):
            fail(f"closure input {relative} contains the recipes, so this arm cannot change them")
        return original(relative, kind)

    generator.identity_of = changed_identity
    try:
        emitted = generator.outputs()
    finally:
        generator.identity_of = original
    moved = sorted(
        path.name
        for path, contents in emitted.items()
        if not path.is_file() or path.read_text(encoding="utf-8") != contents
    ) + sorted(
        path.name
        for directory in (generator.CLOSURE_ROOT, generator.NEGATIVE_ROOT)
        for path in directory.glob("*.zti")
        if path not in emitted
    )

    closures = temporary / "recipe-exam" / "closures"
    closures.mkdir()
    identities: dict[str, str] = {}

    def emitted_identity(name: str) -> str:
        path = generator.CLOSURE_ROOT / f"{name}.zti"
        if path not in emitted:
            return ""
        if name not in identities:
            copied = closures / path.name
            copied.write_text(emitted[path], encoding="utf-8")
            identities[name] = compile_closure(copied).identity.hex()
        return identities[name]

    runs = load_script("closure_recipe_runs", RUN_GENERATOR)
    runs.closure_identity_for = emitted_identity
    moved_runs = sorted(
        path.name
        for path, contents in runs.outputs().items()
        if not path.is_file() or path.read_text(encoding="utf-8") != contents
    )
    if moved or moved_runs:
        fail(
            f"a recipe-only change moves {len(moved)} closure file(s) "
            f"(first {moved[0] if moved else 'none'}) and {len(moved_runs)} test-run "
            f"record(s) (first {moved_runs[0] if moved_runs else 'none'})"
        )


def remove_recipe_control() -> None:
    """Undo the recipe control's in-place change, including one a killed run left."""
    text = JUSTFILE.read_bytes()
    if RECIPE_CONTROL_LINE in text:
        JUSTFILE.write_bytes(text.replace(RECIPE_CONTROL_LINE, b""))
    RECIPE_CONTROL_FILE.unlink(missing_ok=True)


def build_with_changed_recipes(temporary: Path, output: Path) -> int:
    """Build `CLOSURE` into `output` with the recipes changed, and return the dumps served.

    `Justfile` gains a comment line and `just/` an unimported file, at the root
    the builder reads from, and the build's `PATH` starts with the
    `RECIPE_CONTROL_SHIM` `just`. Both files are restored before this returns.
    """
    just = shutil.which("just")
    if just is None:
        fail("recipe control: no just on PATH to serve the recipe dump")
    shim_directory = temporary / "recipe-control-bin"
    shim_directory.mkdir()
    log = temporary / "recipe-control-just.log"
    shim = shim_directory / "just"
    shim.write_text(
        f"#!{sys.executable}\n"
        f"JUST = {just!r}\nLOG = {str(log)!r}\nRECIPE = {RECIPE_CONTROL_RECIPE!r}\n"
        + RECIPE_CONTROL_SHIM,
        encoding="utf-8",
    )
    shim.chmod(0o755)
    environment = {
        **os.environ,
        "PATH": os.pathsep.join([str(shim_directory), os.environ.get("PATH", "")]),
    }
    remove_recipe_control()
    original = JUSTFILE.read_bytes()
    try:
        JUSTFILE.write_bytes(original + RECIPE_CONTROL_LINE)
        RECIPE_CONTROL_FILE.write_bytes(RECIPE_CONTROL_LINE)
        build = subprocess.run(
            ["python3", str(BUILDER), str(CLOSURE), str(output)],
            cwd=ROOT,
            check=False,
            env=environment,
        )
    finally:
        JUSTFILE.write_bytes(original)
        RECIPE_CONTROL_FILE.unlink(missing_ok=True)
    calls = (
        [json.loads(line) for line in log.read_text(encoding="utf-8").splitlines()]
        if log.is_file()
        else []
    )
    refused = [call["arguments"] for call in calls if not call["dump"]]
    if refused:
        fail(f"recipe control: the build ran just {refused[0]}")
    if build.returncode != 0:
        fail(f"recipe control: the build failed with the recipes changed (exit {build.returncode})")
    return len(calls)


def check_recipe_control(temporary: Path) -> int:
    """The recipes reach no image byte: a build with them changed equals the clean build."""
    output = temporary / "build-recipes"
    dumps = build_with_changed_recipes(temporary, output)
    for relative in BUILD_OUTPUTS:
        if (temporary / "build-a" / relative).read_bytes() != (output / relative).read_bytes():
            fail(f"changed recipes changed {relative}")
    return dumps


def main() -> None:
    resolved = resolve_closure(CLOSURE)
    run = compile_test_run(TEST_RUN)
    if run.value["imageClosureIdentity"] != resolved.compiled.identity.hex():
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
        check_recipe_inputs_absent()
        check_recipe_independence(temporary)
        check_builds(temporary)
        dumps = check_recipe_control(temporary)
    print(
        "system image closure check: contracts, resolution, identity, isolation, and bytes "
        f"verified; recipes reach no closure and no image byte ({dumps} recipe dump(s) served)"
    )


if __name__ == "__main__":
    main()
