#!/usr/bin/env python3

"""Decide whether this repository's rules allow devloop to proceed.

devloop asks before it starts work and before it completes it; the answer is
Slime OS policy, so it lives here rather than in devloop. Approval is not
evidence: it says the repository's own preconditions hold, never that an
acceptance was observed.

Two standing rules apply, both from `AGENTS.md` and `CONTRIBUTING.md`:

* **Planning lands first.** The item must already exist on canonical `main`, so
  implementation runs against a landed canonical UUID rather than one invented
  in the branch. Read from `origin/main` in the local object store; no network.
* **Backlog first.** An open or active backlog defect blocks new track work
  while a milestone is active, exactly as `just tasks_check` enforces it.
* **The exam is landed.** At eligibility and completion, the execution inputs
  must be a tracked file under `.devloop/inputs/` that canonical `main`
  already carries byte-identically; the recipe it names and every recipe in
  its dependency chain must be defined as `origin/main` defines them; and
  the scripts those recipes invoke, with the `scripts/lib` modules they
  import, must match `origin/main` blob for blob. A gate proves only that a recipe passed; if the
  implementation branch may also rewrite the recipe, the checker, or the marker
  chain that decides its own acceptance, the evidence is self-graded. A checker
  that needs changing is a planning change: land it first.

Reads devloop's request on stdin and writes `{"approved": bool}`. Refusals are
explained on stderr, because a bare `false` is not actionable.
"""

from __future__ import annotations

import ast
import hashlib
import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from harness import ROOT  # noqa: E402
import work_items  # noqa: E402

CANONICAL = "origin/main"


def landed(identity: str) -> bool:
    """Whether canonical `main` already carries this identity."""
    for path in (f".tasks/items/{identity}.md", f".tasks/terminal/{identity}.json"):
        finished = subprocess.run(
            ["git", "cat-file", "-e", f"{CANONICAL}:{path}"],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )
        if finished.returncode == 0:
            return True
    return False


INPUTS = ROOT / ".devloop" / "inputs"
JUST_SOURCES = ("Justfile", "just")
LIB = ROOT / "scripts" / "lib"

# A token in a recipe body that names a script this repository runs. Just
# interpolations and flags are not paths; anything under `scripts/` that exists
# in the working tree is.
SCRIPT_TOKEN = re.compile(r"(?<![\w/.-])scripts/[\w./-]+\.py\b")


def git(*arguments: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(["git", *arguments], cwd=ROOT, capture_output=True, text=True)


def blob_at_canonical(path: str) -> str | None:
    """The blob id `origin/main` records for `path`, or None when absent."""
    finished = git("rev-parse", "--verify", "--quiet", f"{CANONICAL}:{path}")
    return finished.stdout.strip() if finished.returncode == 0 else None


def blob_in_tree(path: str) -> str | None:
    """The blob id of the working-tree file, as Git would hash it."""
    if not (ROOT / path).is_file():
        return None
    finished = git("hash-object", "--", path)
    return finished.stdout.strip() if finished.returncode == 0 else None


def tracked_inputs() -> list[str]:
    finished = git("ls-files", "--", INPUTS.relative_to(ROOT).as_posix())
    return [line for line in finished.stdout.splitlines() if line.endswith(".json")]


def inputs_named(digest: str, candidates: list[str]) -> str | None:
    """The tracked inputs file whose bytes devloop bound as `digest`."""
    for path in candidates:
        try:
            if hashlib.sha256((ROOT / path).read_bytes()).hexdigest() == digest:
                return path
        except OSError:
            continue
    return None


def recipe_dump(justfile: Path = ROOT / "Justfile") -> dict:
    """Every recipe `justfile` declares, as `just` itself resolves them."""
    finished = subprocess.run(
        [
            "just",
            "--justfile",
            str(justfile),
            "--working-directory",
            str(justfile.parent),
            "--dump",
            "--dump-format",
            "json",
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if finished.returncode:
        raise RuntimeError(f"cannot dump just recipes: {finished.stderr.strip()}")
    return json.loads(finished.stdout)["recipes"]


def canonical_recipe_dump() -> dict:
    """The recipes canonical `main` declares, dumped from its own just sources."""
    with tempfile.TemporaryDirectory(prefix="canonical-just-") as temporary:
        archive = git("archive", CANONICAL, *JUST_SOURCES)
        if archive.returncode:
            raise RuntimeError(f"cannot read just sources at {CANONICAL}: {archive.stderr.strip()}")
        extract = subprocess.run(
            ["tar", "-x", "-C", temporary], input=archive.stdout.encode(), capture_output=True
        )
        if extract.returncode:
            raise RuntimeError("cannot extract canonical just sources")
        return recipe_dump(Path(temporary) / "Justfile")


def recipe_closure(target: str, recipes: dict) -> list[str]:
    """`target` and every recipe it depends on, transitively."""
    names: list[str] = []
    pending = [target]
    while pending:
        name = pending.pop()
        if name in names:
            continue
        recipe = recipes.get(name)
        if recipe is None:
            raise KeyError(name)
        names.append(name)
        pending.extend(dependency["recipe"] for dependency in recipe.get("dependencies", []))
    return sorted(names)


def definition(recipe: dict) -> str:
    """What a recipe does, independent of where in the sources it is written."""
    return json.dumps(
        {key: recipe.get(key) for key in ("body", "dependencies", "parameters", "shebang")},
        sort_keys=True,
    )


def drifted_recipes(target: str, tree: dict, canonical: dict) -> list[str]:
    findings = []
    for name in recipe_closure(target, tree):
        if name not in canonical:
            findings.append(f"recipe {name} is not on {CANONICAL}")
        elif definition(tree[name]) != definition(canonical[name]):
            findings.append(f"recipe {name} differs from {CANONICAL}")
    return findings


def recipe_scripts(target: str, recipes: dict) -> set[str]:
    """Every script token in `target`'s body and its dependency closure."""
    scripts: set[str] = set()
    for name in recipe_closure(target, recipes):
        for line in recipes[name].get("body", []):
            text = " ".join(part for part in line if isinstance(part, str))
            scripts.update(SCRIPT_TOKEN.findall(text))
    return scripts


def lib_imports(paths: set[str]) -> set[str]:
    """`scripts/lib` modules the given scripts import, transitively.

    Static, so a module the checker only names at runtime is not followed;
    that is the accepted bound, and `codePaths` still stales its evidence.
    """
    closure: set[str] = set()
    pending = list(paths)
    while pending:
        path = pending.pop()
        if path in closure:
            continue
        closure.add(path)
        try:
            tree = ast.parse((ROOT / path).read_text())
        except (OSError, SyntaxError):
            continue
        names: set[str] = set()
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                names.update(alias.name.split(".")[0] for alias in node.names)
            elif isinstance(node, ast.ImportFrom) and node.level == 0 and node.module:
                names.add(node.module.split(".")[0])
        for name in names:
            module = LIB / f"{name}.py"
            if module.is_file():
                pending.append(module.relative_to(ROOT).as_posix())
    return closure


def verification_closure(target: str, recipes: dict) -> set[str]:
    """The script files whose bytes decide what `just <target>` observes."""
    return lib_imports(recipe_scripts(target, recipes))


def drifted(files: set[str], at_canonical, in_tree) -> list[str]:
    """Files whose working-tree blob is not what canonical `main` carries."""
    findings = []
    for path in sorted(files):
        landed_blob, tree_blob = at_canonical(path), in_tree(path)
        if landed_blob is None:
            findings.append(f"{path} is not on {CANONICAL}")
        elif tree_blob != landed_blob:
            findings.append(f"{path} differs from {CANONICAL}")
    return findings


def exam_landed(execution: dict) -> list[str]:
    """Reasons the recipe deciding this completion is not a landed decision."""
    digest = execution.get("identity", {}).get("inputs", "")
    inputs = inputs_named(digest, tracked_inputs())
    if inputs is None:
        return [
            "the execution inputs are not a tracked file under .devloop/inputs/: "
            "commit the inputs there in the planning change so the recipe an "
            "acceptance binds is a landed decision, and gate with that path"
        ]
    if blob_at_canonical(inputs) != blob_in_tree(inputs):
        return [f"{inputs} differs from {CANONICAL}: the recipe it names is not a landed decision"]
    try:
        targets = named_recipes(json.loads((ROOT / inputs).read_text()))
        if not targets:
            return []
        recipes, canonical = recipe_dump(), canonical_recipe_dump()
    except (OSError, ValueError, RuntimeError) as error:
        return [f"cannot resolve the recipes {inputs} names: {error}"]
    reasons = []
    for target in targets:
        try:
            findings = drifted_recipes(target, recipes, canonical)
            files = verification_closure(target, recipes)
        except KeyError as error:
            reasons.append(f"{inputs} names recipe {error}, which `just` does not publish")
            continue
        findings += drifted(files, blob_at_canonical, blob_in_tree)
        if findings:
            reasons.append(
                f"`just {target}` is decided by definitions this branch changed; land them "
                "in a planning change first, or the evidence is self-graded: "
                + "; ".join(findings)
            )
    return reasons


def named_recipes(inputs: object) -> list[str]:
    """The recipes an inputs file binds: `justTarget` for the generic gates,
    `recipes` for a gate that runs a fixed set."""
    if not isinstance(inputs, dict):
        raise ValueError("execution inputs are not a JSON object")
    target = inputs.get("justTarget")
    if isinstance(target, str) and target:
        return [target]
    recipes = inputs.get("recipes", [])
    if not isinstance(recipes, list) or not all(isinstance(r, str) and r for r in recipes):
        raise ValueError("`recipes` must be a list of recipe names")
    return recipes


def main() -> int:
    request = json.load(sys.stdin)
    item = request.get("item", {})
    identity = item.get("id", "")
    reasons: list[str] = []

    if not identity:
        reasons.append("the request names no canonical UUID")
    elif not landed(identity):
        reasons.append(
            f"{identity} is not on {CANONICAL}: land the work-item-only planning change "
            "before implementing it"
        )

    blocking = work_items.open_backlog()
    active_milestones = [
        entry
        for entry in work_items.items()
        if entry["kind"] == "milestone" and entry["state"] == "active"
    ]
    if blocking and active_milestones:
        names = ", ".join(str(entry["key"] or entry["id"]) for entry in blocking)
        reasons.append(f"backlog-first: resolve, defer, or block {names} before track work")

    # devloop asks with an execution context only at eligibility and completion;
    # `start` carries none, and the exam need not be landed to begin work.
    execution = request.get("execution")
    if isinstance(execution, dict):
        reasons.extend(exam_landed(execution))

    for reason in reasons:
        sys.stderr.write(f"devloop approval refused: {reason}\n")
    json.dump({"approved": not reasons}, sys.stdout)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
