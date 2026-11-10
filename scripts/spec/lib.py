"""Shared mechanism for the `scripts/spec` CLI (devloop v1.5).

Reads `specs/*.zt` areas and `.tasks/**/*.zt` work items through
`zutai-cli json`, resolves `just` recipe closures and their digests, reads
and signs evidence records, and nothing else. No host or network access
lives here: workflows pass what they know on the command line, and
`forge.py` owns everything a code host answers.

Two roots. `CODE` is where this file lives, and with it the judge's own
configuration: the record and task contracts, `specs/allowed_signers`,
`HUMAN_OWNERS` and `AGENT_ACCOUNTS`. `ROOT` is the checkout being judged,
`$SPEC_ROOT` when set. `scripts/spec/check` runs the base commit's copy of
this code against the head checkout, so a pull request cannot change the
judge or the lists that judge it.
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
import tempfile
from dataclasses import dataclass, field
from fnmatch import fnmatchcase
from pathlib import Path

CODE = Path(__file__).resolve().parents[2]
ROOT = Path(os.environ.get("SPEC_ROOT") or CODE).resolve()
SPECS = ROOT / "specs"
ITEMS = ROOT / ".tasks" / "items"
TERMINAL = ROOT / ".tasks" / "terminal"
TASK_CONTRACT = CODE / "contracts" / "task" / "v2"
ITEM_SCHEMA_VERSION = "task/v2"
ITEM_SOURCES = ("schema.zt", "helpers.zt")
AREA_SOURCES = ("schema.zt", "helpers.zt", "legacy.zt")
ITEM_DIGEST_PREFIX = "slime-task-requirements-digest-v2\n"
SCHEMA_VERSION = "spec/v1"
OBSERVATION_SCHEMA = CODE / "contracts" / "observation" / "v1" / "schema.zt"
OBSERVATION_VERSION = "observation/v1"
OBSERVATIONS = ROOT / "build" / "observations"
EVIDENCE_REF = "refs/heads/evidence"
ALLOWED_SIGNERS = CODE / "specs" / "allowed_signers"
HUMAN_OWNERS = CODE / "HUMAN_OWNERS"
AGENT_ACCOUNTS = CODE / "AGENT_ACCOUNTS"
# What the judge reads from its own commit, never from the head it judges.
JUDGE_PATHS = ("scripts/spec", "contracts/observation", "contracts/task", "specs/allowed_signers", "HUMAN_OWNERS", "AGENT_ACCOUNTS")
SIGN_NAMESPACE = "slime-spec"
WITNESS_SIGNER = "ci"
DIGEST_PREFIX = "slime-spec-requirement-digest-v1\n"
UUID7 = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")
SLUG = re.compile(r"^[a-z][a-z0-9-]*$")

INTENTS = ("planned", "active", "deferred", "blocked", "cancelled")
# Legal `intent` transitions; `None` is "the item does not exist yet". A
# deferred item is planned again before it starts; a blocked item is
# unblocked before it is deferred. Done is no intent: closeout moves the file.
EDGES: dict[str | None, frozenset[str]] = {
    None: frozenset({"planned"}),
    "planned": frozenset({"active", "deferred", "cancelled"}),
    "active": frozenset({"blocked", "deferred", "cancelled"}),
    "blocked": frozenset({"active", "cancelled"}),
    "deferred": frozenset({"planned", "cancelled"}),
    "cancelled": frozenset(),
}
APPROVED_INTENTS = frozenset({"deferred", "cancelled"})

# The grader set every target shares: the record writer and its schema.
GRADER = ("scripts/spec/lib.py", "scripts/spec/record", "contracts/observation/v1/schema.zt")
TOOLCHAIN = ("rust-toolchain.toml", "sel4/pins.toml", "flake.lock", "Cargo.lock")
PRODUCT_ROOTS = ("components", "slime-root", "contracts", "boot-contracts")
CLOSURE_DIGEST_PREFIX = "slime-spec-closure-digest-v1\n"
SCRIPT_TOKEN = re.compile(r"(?<![\w/.-])scripts/[\w./-]+\.(?:py|sh)\b")
IMAGE_CLOSURE = re.compile(r'^CLOSURE\s*=\s*"([a-z][a-z0-9-]*)"', re.M)


class SpecError(SystemExit):
    pass


# --- Zutai -----------------------------------------------------------------


def zutai() -> str:
    binary = os.environ.get("ZUTAI") or shutil.which("zutai-cli")
    if not binary:
        for base in (CODE, ROOT):
            built = base / "deps" / "zutai" / "target" / "release" / "zutai-cli"
            if built.is_file():
                return str(built)
        raise SpecError("zutai-cli is not on PATH and ZUTAI is unset; enter the dev shell")
    return binary


def stdlib_root() -> str:
    """$ZUTAI_STDLIB_ROOT, else the submodule's stdlib, else the one packaged beside zutai-cli."""
    configured = os.environ.get("ZUTAI_STDLIB_ROOT")
    if configured:
        return configured
    for candidate in (CODE / "deps" / "zutai" / "stdlib", ROOT / "deps" / "zutai" / "stdlib"):
        if (candidate / "zutai.zti").is_file():
            return str(candidate)
    packaged = Path(zutai()).resolve().parent.parent / "share" / "zutai" / "stdlib"
    if (packaged / "zutai.zti").is_file():
        return str(packaged)
    raise SpecError("no Zutai stdlib: set ZUTAI_STDLIB_ROOT, check out deps/zutai, or enter the dev shell")


def zutai_json(path: Path) -> object:
    """The final value of `path` as JSON, as `zutai-cli json` prints it."""
    environment = os.environ | {"ZUTAI_STDLIB_ROOT": stdlib_root()}
    process = subprocess.run([zutai(), "json", str(path)], cwd=path.parent, env=environment, check=False, text=True, capture_output=True)
    shown = path.relative_to(ROOT) if path.is_relative_to(ROOT) else path
    if process.returncode != 0:
        raise SpecError(f"{shown}: zutai-cli json failed (exit {process.returncode})\n{process.stderr}")
    try:
        return json.loads(process.stdout)
    except json.JSONDecodeError as error:
        raise SpecError(f"{shown}: zutai-cli json printed no JSON: {error}") from error


def canonical(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def tag(value: object) -> str:
    """Union tag of a Zutai value as `zutai-cli json` prints it: `"#t"` or `{tag, payload}`."""
    if isinstance(value, str):
        return value.removeprefix("#")
    return str(value.get("tag", "")).removeprefix("#") if isinstance(value, dict) else ""


def payload(value: object) -> dict:
    return value.get("payload", {}) if isinstance(value, dict) else {}


# --- Git -------------------------------------------------------------------


def git(*arguments: str, check: bool = True, env: dict | None = None, data: bytes | None = None, cwd: Path | None = None) -> str:
    done = subprocess.run(["git", *arguments], cwd=cwd or ROOT, check=False, capture_output=True, env=env, input=data)
    if check and done.returncode != 0:
        raise SpecError(f"git {' '.join(arguments)} failed: {done.stderr.decode().strip()}")
    return done.stdout.decode()


def read_blob(ref: str, path: str) -> bytes | None:
    """`path` at `ref`, or None when either is absent."""
    done = subprocess.run(["git", "cat-file", "-p", f"{ref}:{path}"], cwd=ROOT, check=False, capture_output=True)
    return done.stdout if done.returncode == 0 else None


def tracked(pathspec: str = ".") -> list[str]:
    return git("ls-files", "--", pathspec).splitlines()


def evidence_ref() -> str | None:
    """The first evidence ref that exists: $SPEC_EVIDENCE_REF, origin/evidence, evidence."""
    for candidate in (os.environ.get("SPEC_EVIDENCE_REF"), "refs/remotes/origin/evidence", EVIDENCE_REF):
        if candidate and git("rev-parse", "-q", "--verify", candidate + "^{commit}", check=False).strip():
            return candidate
    return None


def extract(rev: str, paths: list[str], into: Path) -> list[str]:
    """Write the files under `paths` as committed at `rev` into `into`; returns their names."""
    listing = git("ls-tree", "-r", "--name-only", rev, "--", *paths, check=False).splitlines()
    for name in listing:
        data = read_blob(rev, name)
        if data is not None:
            (into / name).parent.mkdir(parents=True, exist_ok=True)
            (into / name).write_bytes(data)
    return listing


# --- Areas (spec/v1, frozen) -----------------------------------------------


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


def area_files(root: Path | None = None) -> list[Path]:
    specs = (root or ROOT) / "specs"
    return sorted(p for p in specs.glob("*.zt") if p.name not in AREA_SOURCES)


def load_areas(root: Path | None = None) -> list[Area]:
    areas = []
    for path in area_files(root):
        value = zutai_json(path)
        if not isinstance(value, dict) or value.get("schema") != SCHEMA_VERSION:
            raise SpecError(f"{path}: final value is not a {SCHEMA_VERSION} Area")
        areas.append(Area(path, value))
    return areas


def load_areas_at(rev: str) -> list[Area]:
    with tempfile.TemporaryDirectory(prefix="slime-spec-base-") as temporary:
        extract(rev, ["specs"], Path(temporary))
        return [Area(ROOT / a.path.relative_to(temporary), a.value) for a in load_areas(Path(temporary))]


def requirement_digest(requirement: dict) -> str:
    """Digest an area record binds to: the statement and the acceptances."""
    bound = {"statement": requirement["statement"], "acceptance": requirement["acceptance"]}
    return hashlib.sha256(DIGEST_PREFIX.encode() + canonical(bound)).hexdigest()


def area_acceptances(requirement: dict) -> list[tuple[str, dict]]:
    """`(tag, payload)` for each acceptance of an area requirement."""
    return [(tag(a), payload(a)) for a in requirement["acceptance"]]


# --- Work items (task/v2) --------------------------------------------------


@dataclass
class Item:
    """A work item: `.tasks/items/<slug>.zt` (open) or `.tasks/terminal/<slug>.zt`."""

    path: Path
    value: dict
    root: Path = field(default_factory=lambda: ROOT)

    @property
    def id(self) -> str:
        return self.value["id"]

    @property
    def terminal(self) -> bool:
        return self.path.parent.name == "terminal"

    @property
    def intent(self) -> str:
        return tag(self.value["intent"])

    @property
    def kind(self) -> str:
        return tag(self.value["kind"])

    @property
    def where(self) -> str:
        return self.path.relative_to(self.root).as_posix() if self.path.is_relative_to(self.root) else str(self.path)

    def targets(self) -> list[str]:
        return [payload(a["check"])["justTarget"] for a in self.value["acceptance"] if tag(a["check"]) == "observed"]


def item_files(root: Path | None = None) -> list[Path]:
    base = root or ROOT
    dirs = (base / ".tasks" / "items", base / ".tasks" / "terminal")
    return sorted(p for d in dirs if d.is_dir() for p in d.glob("*.zt") if p.name not in ITEM_SOURCES)


def load_items(root: Path | None = None) -> list[Item]:
    base = root or ROOT
    items = []
    for path in item_files(base):
        value = zutai_json(path)
        if not isinstance(value, dict) or value.get("schema") != ITEM_SCHEMA_VERSION:
            raise SpecError(f"{path}: final value is not a {ITEM_SCHEMA_VERSION} Item")
        items.append(Item(path, value, base))
    return items


def load_items_at(rev: str) -> list[Item]:
    """Items as committed at `rev`, evaluated from a temporary extraction."""
    with tempfile.TemporaryDirectory(prefix="slime-task-base-") as temporary:
        root = Path(temporary)
        if not any(n.endswith(".zt") for n in extract(rev, [".tasks"], root)):
            return []
        return [Item(ROOT / i.path.relative_to(root), i.value) for i in load_items(root)]


def item_digest(item: dict) -> str:
    """What a done witness and a human record bind to: requirement statements and acceptances."""
    bound = {
        "requirements": [{k: r[k] for k in ("id", "statement", "acceptance")} for r in item["requirements"]],
        "acceptance": item["acceptance"],
    }
    return hashlib.sha256(ITEM_DIGEST_PREFIX.encode() + canonical(bound)).hexdigest()


def superseded(items: list[Item]) -> set[str]:
    """Everything some item's `supersedes` names: item ids, `<item>/<req>` and area requirement ids."""
    return {entry for item in items for entry in item.value["supersedes"]}


def bound_targets(items: list[Item], areas: list[Area], promises_only: bool = False) -> set[str]:
    """Targets an exam binds. Bound: any item in `items/` that is not cancelled,
    any terminal item that is not superseded, and any area requirement.
    Bound by a promise (`promises_only`): the last two only."""
    gone = superseded(items)
    targets: set[str] = set()
    for item in items:
        if item.terminal and item.id not in gone:
            targets.update(item.targets())
        elif not item.terminal and not promises_only and item.intent != "cancelled":
            targets.update(item.targets())
    for area in areas:
        for requirement in area.requirements:
            if requirement["id"] not in gone:
                targets.update(p["justTarget"] for t, p in area_acceptances(requirement) if t == "observed")
    return targets


# --- Recipes and closures --------------------------------------------------


def _dump(justfile: Path, workdir: Path) -> dict[str, dict]:
    done = subprocess.run(
        ["just", "--justfile", str(justfile), "--working-directory", str(workdir), "--dump", "--dump-format", "json"],
        check=False,
        text=True,
        capture_output=True,
    )
    if done.returncode != 0:
        raise SpecError(f"just --dump failed (exit {done.returncode})\n{done.stderr}")
    dump = json.loads(done.stdout)
    recipes = dict(dump.get("recipes", {}))
    for module in dump.get("modules", {}).values():
        recipes.update(module.get("recipes", {}))
    return recipes


@dataclass
class Recipes:
    """Every recipe the root Justfile declares, as `just --dump` resolves them."""

    by_name: dict[str, dict] = field(default_factory=dict)
    root: Path = field(default_factory=lambda: ROOT)

    @classmethod
    def load(cls, root: Path | None = None) -> Recipes:
        base = root or ROOT
        if not (base / "Justfile").is_file():
            return cls({}, base)
        return cls(_dump(base / "Justfile", base), base)

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

    def grader(self, target: str) -> set[str]:
        """The files a target's verdict depends on beyond the product: its
        scripts, the `scripts/lib` modules they import, and `GRADER`."""
        return lib_imports(self.scripts(self.closure(target)), self.root) | set(GRADER)


def lib_imports(paths: set[str], root: Path | None = None) -> set[str]:
    """`scripts/lib` modules the given scripts import, transitively (static)."""
    base = root or ROOT
    closure: set[str] = set()
    pending = list(paths)
    while pending:
        path = pending.pop()
        if path in closure:
            continue
        closure.add(path)
        try:
            tree = ast.parse((base / path).read_text())
        except (OSError, SyntaxError, UnicodeDecodeError):
            continue
        names: set[str] = set()
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                names.update(alias.name.split(".")[0] for alias in node.names)
            elif isinstance(node, ast.ImportFrom) and node.level == 0 and node.module:
                names.add(node.module.split(".")[0])
        for name in names:
            if (base / "scripts" / "lib" / f"{name}.py").is_file():
                pending.append(f"scripts/lib/{name}.py")
    return closure


def image_closures(scripts: set[str]) -> set[str]:
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
    if not globs:
        return []
    return sorted(set(git("ls-files", "--", *globs).splitlines()))


class ImageResolver:
    """Derivation ids of system-image closures, sharing one resolution cache."""

    def __init__(self) -> None:
        self._cache = None
        self._ids: dict[str, tuple[str, bool]] = {}

    def identity(self, name: str) -> tuple[str, bool]:
        if name not in self._ids:
            library = str(ROOT / "scripts" / "lib")
            if library not in sys.path:
                sys.path.insert(0, library)
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
    recipes: dict[str, str]
    grader: dict[str, str | None]
    inputs: dict[str, str | None]
    images: dict[str, str]
    toolchain: dict[str, str | None]
    product: dict[str, str]
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
    """digest = recipe definitions + grader set + declared inputs + image
    derivations + toolchain pins; a target with neither images nor inputs
    falls back to the whole product tree (coarse)."""
    names = recipes.closure(target)
    scripts = lib_imports(recipes.scripts(names))
    grader = {p: blob(p) for p in scripts | set(GRADER)}
    inputs = {p: blob(p) for p in expand(recipes.inputs(names))}
    image_ids: dict[str, str] = {}
    dirty = any(blob(p) != _head_blob(p) for p in grader if blob(p) is not None)
    resolver = images or ImageResolver()
    for name in sorted(image_closures(scripts)):
        identity, is_dirty = resolver.identity(name)
        image_ids[name] = identity
        dirty = dirty or is_dirty
    product = {} if image_ids or inputs else {root: tree(root) for root in PRODUCT_ROOTS if (ROOT / root).is_dir()}
    dirty = dirty or any(t.endswith("+dirty") for t in product.values())
    dirty = dirty or any(b != _head_blob(p) for p, b in inputs.items())
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


def _head_blob(path: str) -> str | None:
    done = subprocess.run(["git", "rev-parse", "-q", "--verify", f"HEAD:{path}"], cwd=ROOT, check=False, text=True, capture_output=True)
    return done.stdout.strip() or None


# --- Records and signatures ------------------------------------------------


def parse_record(data: bytes, kind: str = "Record") -> dict:
    """Type-check a record against contracts/observation/v1 and return its value."""
    text = data.decode("utf-8")
    if f"r :: o.{kind} =" not in text:
        raise SpecError(f"not an o.{kind} record")
    with tempfile.TemporaryDirectory(prefix="slime-spec-record-") as temporary:
        shutil.copyfile(OBSERVATION_SCHEMA, Path(temporary) / "schema.zt")
        entry = Path(temporary) / "record.zt"
        entry.write_text(text, encoding="utf-8")
        value = zutai_json(entry)
    if not isinstance(value, dict) or value.get("schema") != OBSERVATION_VERSION:
        raise SpecError(f"record schema is not {OBSERVATION_VERSION}")
    return value


def zt_text(value: str) -> str:
    return json.dumps(value, ensure_ascii=False)


def human_record_text(area: str, owner: str, acceptance: str, digest: str, value: bool, note: str, signer: str, observed_at: str) -> str:
    return (
        'o ::= import "./schema.zt";\n'
        "r :: o.HumanRecord = {\n"
        "  schema = o.schemaVersion;\n"
        f"  area = {zt_text(area)};\n"
        f"  requirement = {zt_text(owner)};\n"
        f"  acceptance = {zt_text(acceptance)};\n"
        f"  requirementsDigest = {zt_text(digest)};\n"
        f"  value = {'true' if value else 'false'};\n"
        f"  note = {zt_text(note)};\n"
        f"  signer = {zt_text(signer)};\n"
        f"  observedAt = {zt_text(observed_at)};\n"
        "};\nr\n"
    )


def witness_text(item: str, digest: str, main_sha: str, targets: list[tuple[str, str, int]], signer: str, observed_at: str) -> str:
    rows = "".join(f" {{ target = {zt_text(t)}; closureDigest = {zt_text(d)}; negativeControlsRefused = {n}; }};" for t, d, n in targets)
    return (
        'o ::= import "./schema.zt";\n'
        "r :: o.Witness = {\n"
        "  schema = o.schemaVersion;\n"
        f"  item = {zt_text(item)};\n"
        f"  itemDigest = {zt_text(digest)};\n"
        f"  mainSha = {zt_text(main_sha)};\n"
        "  targets = {" + (rows + " }" if rows else ";}") + ";\n"
        f"  signer = {zt_text(signer)};\n"
        f"  observedAt = {zt_text(observed_at)};\n"
        "};\nr\n"
    )


def human_path(owner: str, acceptance_id: str, digest: str) -> str:
    return f"human/{owner}/{acceptance_id}/{digest}.zt"


def witness_path(item_id: str, digest: str) -> str:
    return f"done/{item_id}/{digest}.zt"


def sign(path: Path, key: str) -> Path:
    """`ssh-keygen -Y sign` the file; returns the `.sig` path."""
    signature = path.with_name(path.name + ".sig")
    signature.unlink(missing_ok=True)
    done = subprocess.run(["ssh-keygen", "-q", "-Y", "sign", "-f", key, "-n", SIGN_NAMESPACE, str(path)], check=False, capture_output=True, text=True, stdin=subprocess.DEVNULL)
    if done.returncode != 0:
        raise SpecError(f"ssh-keygen -Y sign failed: {done.stderr.strip()}")
    return signature


def verify_signature(data: bytes, signature: bytes | None, signer: str) -> bool:
    """True when `signature` over `data` verifies for `signer` in the judge's allowed_signers."""
    if signature is None or not ALLOWED_SIGNERS.is_file():
        return False
    with tempfile.TemporaryDirectory(prefix="slime-spec-sig-") as temporary:
        sig = Path(temporary) / "record.sig"
        sig.write_bytes(signature)
        done = subprocess.run(
            ["ssh-keygen", "-Y", "verify", "-f", str(ALLOWED_SIGNERS), "-I", signer, "-n", SIGN_NAMESPACE, "-s", str(sig)],
            input=data,
            check=False,
            capture_output=True,
        )
    return done.returncode == 0


def names(path: Path) -> list[str]:
    """Logins listed one per line; `#` starts a comment."""
    if not path.is_file():
        return []
    rows = (line.split("#", 1)[0].strip() for line in path.read_text().splitlines())
    return [row for row in rows if row]


def human_owners() -> set[str]:
    return set(names(HUMAN_OWNERS))


def agent_accounts() -> set[str]:
    return set(names(AGENT_ACCOUNTS))


# --- Evidence --------------------------------------------------------------


class Evidence:
    """Answers: is there a passing record for this target at the current closure?

    Reads the evidence ref; with `local=True` also `build/observations/`.
    Closures are resolved from the working tree, which on CI is HEAD.
    """

    def __init__(self, ref: str | None = None, local: bool = False) -> None:
        self.ref = ref if ref is not None else evidence_ref()
        self.local = local
        self.recipes = Recipes.load()
        self.images = ImageResolver()
        self._closures: dict[str, Closure | None] = {}
        self._records: dict[str, tuple[str, str, dict | None]] = {}

    def closure(self, target: str) -> Closure | None:
        if target not in self._closures:
            try:
                self._closures[target] = resolve(target, self.recipes, self.images)
            except SpecError:
                self._closures[target] = None
        return self._closures[target]

    def blob(self, path: str) -> bytes | None:
        if self.local and (OBSERVATIONS / path).is_file():
            return (OBSERVATIONS / path).read_bytes()
        return read_blob(self.ref, path) if self.ref else None

    def record(self, target: str) -> tuple[str, str, dict | None]:
        """(state, detail, record) for a machine target: enforced, failing or claimed."""
        if target in self._records:
            return self._records[target]
        closure = self.closure(target)
        if closure is None:
            out: tuple[str, str, dict | None] = ("claimed", f"{target} is not a just target", None)
        else:
            digest = closure.digest()
            data = read_blob(self.ref, f"{target}/{digest}.zt") if self.ref else None
            local = OBSERVATIONS / f"{target}.zt"
            if self.local and local.is_file():
                try:
                    candidate = local.read_bytes()
                    if parse_record(candidate)["closureDigest"] == digest:
                        data = candidate
                except SpecError:
                    pass
            if data is None:
                out = ("claimed", f"no record at {digest[:12]}", None)
            else:
                try:
                    record = parse_record(data)
                    ok = record["passed"] and record["target"] == target and record["closureDigest"] == digest
                    out = ("enforced" if ok else "failing", f"record at {digest[:12]}" if ok else f"exit {record['exitCode']}", record)
                except SpecError as error:
                    out = ("failing", f"bad record: {str(error).splitlines()[0]}", None)
        self._records[target] = out
        return out


def _predicate(predicate: dict, counts: dict[str, int], flags: dict[str, bool]) -> str | None:
    kind, body = tag(predicate), payload(predicate)
    key = body["key"]
    if kind == "flag":
        return None if flags.get(key) is True else f"flag {key} is {flags.get(key)}"
    if key not in counts:
        return f"count {key} missing"
    value = counts[key]
    if kind == "count" and value != body["equals"]:
        return f"{key} = {value}, want {body['equals']}"
    if kind == "atLeast" and value < body["min"]:
        return f"{key} = {value}, want >= {body['min']}"
    if kind == "atMost" and value > body["max"]:
        return f"{key} = {value}, want <= {body['max']}"
    return None


def observed_state(target: str, predicates: list[dict], evidence: Evidence) -> tuple[str, str]:
    state, detail, record = evidence.record(target)
    if state == "enforced" and record is not None:
        counts = {c["key"]: c["value"] for c in record["counts"]}
        flags = {f["key"]: f["value"] for f in record["flags"]}
        failed = [m for m in (_predicate(p, counts, flags) for p in predicates) if m]
        if failed:
            return "failing", "; ".join(failed)
    return state, detail


def human_state(path: str, evidence: Evidence) -> tuple[str, str]:
    data = evidence.blob(path)
    if data is None:
        return "advisory", "awaiting a signed human record"
    try:
        record = parse_record(data, "HumanRecord")
    except SpecError as error:
        return "advisory", f"bad human record: {str(error).splitlines()[0]}"
    if not verify_signature(data, evidence.blob(path + ".sig"), record["signer"]):
        return "advisory", f"signature by {record['signer']} does not verify against specs/allowed_signers"
    return ("enforced", f"signed yes by {record['signer']}") if record["value"] else ("failing", f"signed no by {record['signer']}")


def area_acceptance_state(kind: str, body: dict, requirement: dict, evidence: Evidence) -> tuple[str, str]:
    """(enforced | failing | claimed | advisory, detail) for an area acceptance."""
    if kind == "observed":
        return observed_state(body["justTarget"], body["predicates"], evidence)
    return human_state(human_path(requirement["id"], body["id"], requirement_digest(requirement)), evidence)


def item_acceptance_state(item: Item, acceptance: dict, evidence: Evidence) -> tuple[str, str]:
    """State of one acceptance of a work item. A `#planned` item records nothing."""
    check = acceptance["check"]
    if not acceptance["mandatory"]:
        return "advisory", "not mandatory"
    if item.intent == "planned":
        return "advisory", "planned: its exam is not recorded until the item is active"
    kind, body = tag(check), payload(check)
    if kind == "observed":
        return observed_state(body["justTarget"], body["predicates"], evidence)
    if kind == "human":
        return human_state(human_path(item.id, acceptance["id"], item_digest(item.value)), evidence)
    return "claimed", "clause oracles are not read yet"


def item_done(item: Item, evidence: Evidence) -> bool:
    """Every mandatory acceptance enforced, on an `#active` item."""
    if item.intent != "active":
        return False
    mandatory = [a for a in item.value["acceptance"] if a["mandatory"]]
    return bool(mandatory) and all(item_acceptance_state(item, a, evidence)[0] == "enforced" for a in mandatory)


def witness_state(item_id: str, digest: str, blob_at) -> tuple[bool, str, dict | None]:
    """(valid, detail, witness) for the done witness of `item_id` at `digest`,
    read through `blob_at(path)` from an evidence commit."""
    path = witness_path(item_id, digest)
    data = blob_at(path)
    if data is None:
        return False, f"no done witness at {path}", None
    try:
        witness = parse_record(data, "Witness")
    except SpecError as error:
        return False, f"bad witness: {str(error).splitlines()[0]}", None
    if witness["item"] != item_id or witness["itemDigest"] != digest:
        return False, "witness names another item or digest", witness
    if witness["signer"] != WITNESS_SIGNER or not verify_signature(data, blob_at(path + ".sig"), witness["signer"]):
        return False, f"witness signature by {witness['signer']!r} does not verify against specs/allowed_signers", witness
    return True, f"witnessed at {witness['mainSha'][:12]}", witness


def owns_match(path: str, globs: list[str]) -> bool:
    for glob in globs:
        if glob.endswith("/**") and (path.startswith(glob[:-2]) or path == glob[:-3]):
            return True
        if fnmatchcase(path, glob):
            return True
    return False
