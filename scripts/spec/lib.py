"""Shared mechanism for the `scripts/spec` CLI.

Reads `specs/*.zt` through `zutai-cli json`, computes per-requirement
digests, resolves `just` recipes, and nothing else. No host or network
access lives here: workflows pass what they know on the command line.
"""

from __future__ import annotations

import ast
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SPECS = ROOT / "specs"
STDLIB = ROOT / "deps" / "zutai" / "stdlib"
SCHEMA_VERSION = "spec/v1"
OBSERVATION_SCHEMA = ROOT / "contracts" / "observation" / "v1" / "schema.zt"
DIGEST_PREFIX = "slime-spec-requirement-digest-v1\n"
UUID7 = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")
AREA_ID = re.compile(r"^[a-z][a-z0-9-]*$")
# `# spec: <requirement uuid>/<acceptance id>` in Python, just and shell;
# `// spec: ...` in Rust.
SPEC_MARK = re.compile(r"(?:#|//)\s*spec:\s*([0-9a-f-]{36})/([A-Za-z0-9_.:-]+)")

# Closure composition (decision E1). GRADER and the recipe definitions plus
# every script in the recipe's closure form the grader set, which
# `check-grader-independence` protects. TOOLCHAIN, declared inputs, image
# derivation ids and the product fallback are digest-only.
GRADER = ("scripts/spec/lib.py", "scripts/spec/record", "contracts/observation/v1/schema.zt")
TOOLCHAIN = ("rust-toolchain.toml", "sel4/pins.toml", "flake.lock", "Cargo.lock")
PRODUCT_ROOTS = ("components", "slime-root", "contracts", "boot-contracts")
CLOSURE_DIGEST_PREFIX = "slime-spec-closure-digest-v1\n"
SCRIPT_TOKEN = re.compile(r"(?<![\w/.-])scripts/[\w./-]+\.(?:py|sh)\b")
IMAGE_CLOSURE = re.compile(r'^CLOSURE\s*=\s*"([a-z][a-z0-9-]*)"', re.M)
LIB = ROOT / "scripts" / "lib"


class SpecError(SystemExit):
    pass


def zutai() -> str:
    binary = os.environ.get("ZUTAI") or shutil.which("zutai-cli")
    if not binary:
        raise SpecError("zutai-cli is not on PATH and ZUTAI is unset; enter the dev shell")
    return binary


def zutai_json(path: Path) -> object:
    """The final value of `path` as JSON, as `zutai-cli json` prints it."""
    environment = os.environ | {"ZUTAI_STDLIB_ROOT": os.environ.get("ZUTAI_STDLIB_ROOT", str(STDLIB))}
    process = subprocess.run(
        [zutai(), "json", str(path)],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        capture_output=True,
    )
    if process.returncode != 0:
        raise SpecError(f"{path.relative_to(ROOT)}: zutai-cli json failed (exit {process.returncode})\n{process.stderr}")
    try:
        return json.loads(process.stdout)
    except json.JSONDecodeError as error:
        raise SpecError(f"{path.relative_to(ROOT)}: zutai-cli json printed no JSON: {error}") from error


def canonical(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def requirement_digest(requirement: dict) -> str:
    """Digest of what a record binds to: the statement and the acceptances.

    Rationale, planned, note, supersedes, depends and everything at area
    level are excluded, so rewording them does not stale evidence.
    """
    bound = {"statement": requirement["statement"], "acceptance": requirement["acceptance"]}
    return hashlib.sha256(DIGEST_PREFIX.encode() + canonical(bound)).hexdigest()


@dataclass
class Area:
    path: Path
    value: dict

    @property
    def id(self) -> str:
        return self.value["id"]

    @property
    def requirements(self) -> list[dict]:
        return self.value["requirements"]


def area_files() -> list[Path]:
    return sorted(p for p in SPECS.glob("*.zt") if p.name not in {"schema.zt", "helpers.zt"})


def load_area(path: Path) -> Area:
    value = zutai_json(path)
    if not isinstance(value, dict) or value.get("schema") != SCHEMA_VERSION:
        raise SpecError(f"{path.relative_to(ROOT)}: final value is not a {SCHEMA_VERSION} Area")
    return Area(path, value)


def load_areas() -> list[Area]:
    return [load_area(p) for p in area_files()]


def acceptances(requirement: dict) -> list[tuple[str, dict]]:
    """`(tag, payload)` for each acceptance of a requirement."""
    return [(a["tag"], a["payload"]) for a in requirement["acceptance"]]


@dataclass
class Recipes:
    """Every recipe the root justfile declares, as `just --dump` resolves them."""

    by_name: dict[str, dict] = field(default_factory=dict)

    @classmethod
    def load(cls) -> Recipes:
        process = subprocess.run(
            ["just", "--justfile", str(ROOT / "Justfile"), "--working-directory", str(ROOT), "--dump", "--dump-format", "json"],
            cwd=ROOT,
            check=False,
            text=True,
            capture_output=True,
        )
        if process.returncode != 0:
            raise SpecError(f"just --dump failed (exit {process.returncode})\n{process.stderr}")
        dump = json.loads(process.stdout)
        recipes = dict(dump.get("recipes", {}))
        for module in dump.get("modules", {}).values():
            recipes.update(module.get("recipes", {}))
        return cls(recipes)

    def __contains__(self, name: str) -> bool:
        return name in self.by_name

    def closure(self, target: str) -> list[str]:
        """`target` and every recipe it depends on, transitively, sorted."""
        names: list[str] = []
        pending = [target]
        while pending:
            name = pending.pop()
            if name in names:
                continue
            recipe = self.by_name.get(name)
            if recipe is None:
                raise SpecError(f"just target {target!r}: recipe {name!r} is not declared")
            names.append(name)
            pending.extend(d["recipe"] for d in recipe.get("dependencies", []))
        return sorted(names)

    @staticmethod
    def definition(recipe: dict) -> str:
        """What a recipe does, independent of where in the sources it is written."""
        return json.dumps({k: recipe.get(k) for k in ("body", "dependencies", "parameters", "shebang", "attributes")}, sort_keys=True)

    def scripts(self, names: list[str]) -> set[str]:
        found: set[str] = set()
        for name in names:
            for line in self.by_name[name].get("body", []):
                found.update(SCRIPT_TOKEN.findall(" ".join(p for p in line if isinstance(p, str))))
        return found

    def inputs(self, names: list[str]) -> list[str]:
        """Globs from `[metadata("inputs", ...)]` on any recipe in the closure."""
        globs: list[str] = []
        for name in names:
            for attribute in self.by_name[name].get("attributes", []):
                metadata = attribute.get("metadata") if isinstance(attribute, dict) else None
                if metadata and metadata[0] == "inputs":
                    globs.extend(metadata[1:])
        return sorted(set(globs))


def lib_imports(paths: set[str]) -> set[str]:
    """`scripts/lib` modules the given scripts import, transitively (static)."""
    closure: set[str] = set()
    pending = list(paths)
    while pending:
        path = pending.pop()
        if path in closure:
            continue
        closure.add(path)
        try:
            tree = ast.parse((ROOT / path).read_text())
        except (OSError, SyntaxError, UnicodeDecodeError):
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


def image_closures(scripts: set[str]) -> set[str]:
    """Closure names the checkers in `scripts` declare as `CLOSURE = "..."`."""
    names: set[str] = set()
    for path in scripts:
        try:
            names.update(IMAGE_CLOSURE.findall((ROOT / path).read_text()))
        except (OSError, UnicodeDecodeError):
            continue
    return names


def blob(path: str) -> str | None:
    """The working-tree file's blob id as git would hash it, or None."""
    if not (ROOT / path).is_file():
        return None
    done = subprocess.run(["git", "hash-object", "--", path], cwd=ROOT, check=False, text=True, capture_output=True)
    return done.stdout.strip() if done.returncode == 0 else None


def tree(path: str) -> str:
    """HEAD's tree id for `path`, suffixed `+dirty` when the working tree differs."""
    done = subprocess.run(["git", "rev-parse", f"HEAD:{path}"], cwd=ROOT, check=False, text=True, capture_output=True)
    identity = done.stdout.strip() if done.returncode == 0 else "missing"
    status = subprocess.run(["git", "status", "--porcelain", "--", path], cwd=ROOT, check=True, text=True, capture_output=True)
    return identity + ("+dirty" if status.stdout.strip() else "")


def expand(globs: list[str]) -> list[str]:
    """Tracked files matching the globs (`**` crosses directories)."""
    if not globs:
        return []
    done = subprocess.run(["git", "ls-files", "--", *globs], cwd=ROOT, check=True, text=True, capture_output=True)
    return sorted(set(done.stdout.splitlines()))


class ImageResolver:
    """Derivation ids of system-image closures, sharing one resolution cache."""

    def __init__(self) -> None:
        self._cache = None
        self._ids: dict[str, tuple[str, bool]] = {}

    def identity(self, name: str) -> tuple[str, bool]:
        if name not in self._ids:
            if str(LIB) not in sys.path:
                sys.path.insert(0, str(LIB))
            from system_image_closure import ResolutionCache, resolve_source  # noqa: PLC0415

            if self._cache is None:
                self._cache = ResolutionCache()
            materialized = resolve_source(name, cache=self._cache)
            self._ids[name] = (materialized.compiled.identity.hex(), materialized.dirty)
        return self._ids[name]


@dataclass
class Closure:
    """Everything that decides what `just <target>` observes."""

    target: str
    recipes: dict[str, str]  # name -> definition
    grader: dict[str, str | None]  # path -> blob; the protected set
    inputs: dict[str, str | None]  # declared inputs, digest only
    images: dict[str, str]  # closure name -> derivation id
    toolchain: dict[str, str | None]
    product: dict[str, str]  # coarse fallback: product root -> tree id
    dirty: bool

    @property
    def coarse(self) -> bool:
        return bool(self.product)

    def digest(self) -> str:
        lines = [f"recipe {n} {hashlib.sha256(d.encode()).hexdigest()}" for n, d in sorted(self.recipes.items())]
        for kind, table in (("grader", self.grader), ("input", self.inputs), ("toolchain", self.toolchain)):
            lines += [f"{kind} {p} {b or 'missing'}" for p, b in sorted(table.items())]
        lines += [f"image {n} {i}" for n, i in sorted(self.images.items())]
        lines += [f"product {p} {t}" for p, t in sorted(self.product.items())]
        return hashlib.sha256((CLOSURE_DIGEST_PREFIX + "\n".join(lines) + "\n").encode()).hexdigest()


def resolve(target: str, recipes: Recipes, images: ImageResolver | None = None) -> Closure:
    names = recipes.closure(target)
    scripts = lib_imports(recipes.scripts(names))
    grader = {p: blob(p) for p in scripts | set(GRADER)}
    inputs = {p: blob(p) for p in expand(recipes.inputs(names))}
    image_ids: dict[str, str] = {}
    dirty = False
    resolver = images or ImageResolver()
    for name in sorted(image_closures(scripts)):
        identity, is_dirty = resolver.identity(name)
        image_ids[name] = identity
        dirty = dirty or is_dirty
    product = {} if image_ids or inputs else {root: tree(root) for root in PRODUCT_ROOTS if (ROOT / root).is_dir()}
    dirty = dirty or any(t.endswith("+dirty") for t in product.values())
    return Closure(
        target=target,
        recipes={n: Recipes.definition(recipes.by_name[n]) for n in names},
        grader=grader,
        inputs=inputs,
        images=image_ids,
        toolchain={p: blob(p) for p in TOOLCHAIN},
        product=product,
        dirty=dirty,
    )


def spec_marks(paths: list[str]) -> list[tuple[str, int, str, str]]:
    """`(path, line, requirement id, acceptance id)` for every reverse mark."""
    found = []
    for path in paths:
        try:
            text = (ROOT / path).read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        for number, line in enumerate(text.splitlines(), 1):
            match = SPEC_MARK.search(line)
            if match:
                found.append((path, number, match.group(1), match.group(2)))
    return found


def tracked(pathspec: str = ".") -> list[str]:
    process = subprocess.run(["git", "ls-files", "--", pathspec], cwd=ROOT, check=True, text=True, capture_output=True)
    return process.stdout.splitlines()
