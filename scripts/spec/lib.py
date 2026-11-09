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
import tempfile
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SPECS = ROOT / "specs"
ITEMS = ROOT / ".tasks" / "items"
TERMINAL = ROOT / ".tasks" / "terminal"
ITEM_SCHEMA_VERSION = "task/v1"
ITEM_SOURCES = ("schema.zt", "helpers.zt")
ITEM_DIGEST_PREFIX = "slime-task-requirements-digest-v1\n"
STDLIB = ROOT / "deps" / "zutai" / "stdlib"
SCHEMA_VERSION = "spec/v1"
OBSERVATION_SCHEMA = ROOT / "contracts" / "observation" / "v1" / "schema.zt"
OBSERVATION_VERSION = "observation/v1"
OBSERVATIONS = ROOT / "build" / "observations"
EVIDENCE_REF = "refs/heads/evidence"
ALLOWED_SIGNERS = SPECS / "allowed_signers"
HUMAN_OWNERS = ROOT / "HUMAN_OWNERS"
SIGN_NAMESPACE = "slime-spec"
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


def stdlib_root() -> str:
    """$ZUTAI_STDLIB_ROOT, else the submodule's stdlib, else the one packaged beside zutai-cli."""
    configured = os.environ.get("ZUTAI_STDLIB_ROOT")
    if configured:
        return configured
    if (STDLIB / "zutai.zti").is_file():
        return str(STDLIB)
    packaged = Path(zutai()).resolve().parent.parent / "share" / "zutai" / "stdlib"
    if (packaged / "zutai.zti").is_file():
        return str(packaged)
    raise SpecError("no Zutai stdlib: set ZUTAI_STDLIB_ROOT, check out deps/zutai, or enter the dev shell")


def zutai_json(path: Path) -> object:
    """The final value of `path` as JSON, as `zutai-cli json` prints it."""
    environment = os.environ | {"ZUTAI_STDLIB_ROOT": stdlib_root()}
    process = subprocess.run(
        [zutai(), "json", str(path)],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        capture_output=True,
    )
    shown = path.relative_to(ROOT) if path.is_relative_to(ROOT) else path
    if process.returncode != 0:
        raise SpecError(f"{shown}: zutai-cli json failed (exit {process.returncode})\n{process.stderr}")
    try:
        return json.loads(process.stdout)
    except json.JSONDecodeError as error:
        raise SpecError(f"{shown}: zutai-cli json printed no JSON: {error}") from error


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
    return sorted(p for p in SPECS.glob("*.zt") if p.name not in {"schema.zt", "helpers.zt", "legacy.zt"})


def load_area(path: Path) -> Area:
    value = zutai_json(path)
    if not isinstance(value, dict) or value.get("schema") != SCHEMA_VERSION:
        raise SpecError(f"{path.relative_to(ROOT)}: final value is not a {SCHEMA_VERSION} Area")
    return Area(path, value)


def load_areas() -> list[Area]:
    return [load_area(p) for p in area_files()]


@dataclass
class Item:
    """A work item: `.tasks/items/<slug>.zt` (open) or `.tasks/terminal/<slug>.zt`."""

    path: Path
    value: dict

    @property
    def id(self) -> str:
        return self.value["id"]

    @property
    def terminal(self) -> bool:
        return self.path.parent.name == "terminal"

    @property
    def where(self) -> str:
        return self.path.relative_to(ROOT).as_posix() if self.path.is_relative_to(ROOT) else str(self.path)


def tag(value: object) -> str:
    """Union tag of a Zutai value as `zutai-cli json` prints it: `"#t"` or `{tag, payload}`."""
    if isinstance(value, str):
        return value.removeprefix("#")
    return value["tag"] if isinstance(value, dict) else ""


def payload(value: object) -> dict:
    return value.get("payload", {}) if isinstance(value, dict) else {}


def item_files(root: Path = ROOT) -> list[Path]:
    dirs = (root / ".tasks" / "items", root / ".tasks" / "terminal")
    return sorted(p for d in dirs if d.is_dir() for p in d.glob("*.zt") if p.name not in ITEM_SOURCES)


def load_item(path: Path) -> Item:
    value = zutai_json(path)
    if not isinstance(value, dict) or value.get("schema") != ITEM_SCHEMA_VERSION:
        raise SpecError(f"{path}: final value is not a {ITEM_SCHEMA_VERSION} Item")
    return Item(path, value)


def load_items() -> list[Item]:
    return [load_item(p) for p in item_files()]


def load_items_at(rev: str) -> list[Item]:
    """Items as committed at `rev`, evaluated from a temporary extraction."""
    listing = git("ls-tree", "-r", "--name-only", rev, ".tasks/", check=False).splitlines()
    if not any(p.endswith(".zt") for p in listing):
        return []
    with tempfile.TemporaryDirectory(prefix="slime-task-base-") as temporary:
        root = Path(temporary)
        for name in listing:
            data = read_blob(rev, name)
            if data is not None:
                (root / name).parent.mkdir(parents=True, exist_ok=True)
                (root / name).write_bytes(data)
        items = []
        for path in item_files(root):
            value = zutai_json(path)
            if isinstance(value, dict) and value.get("schema") == ITEM_SCHEMA_VERSION:
                items.append(Item(ROOT / path.relative_to(root), value))
        return items


def item_digest(item: dict) -> str:
    """Digest a signed human record binds to: requirement statements and acceptances.

    Provenance, decisions, invariants and prose are excluded, so editing them
    does not ask a person again.
    """
    bound = {
        "requirements": [{k: r[k] for k in ("id", "statement", "acceptance")} for r in item["requirements"]],
        "acceptance": item["acceptance"],
    }
    return hashlib.sha256(ITEM_DIGEST_PREFIX.encode() + canonical(bound)).hexdigest()


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

    @classmethod
    def load_at(cls, rev: str) -> Recipes:
        """Recipes as committed at `rev`: the Justfile and just/ extracted to a temp dir."""
        with tempfile.TemporaryDirectory(prefix="slime-spec-just-") as temporary:
            archive = subprocess.run(["git", "archive", rev, "Justfile", "just"], cwd=ROOT, check=True, capture_output=True).stdout
            subprocess.run(["tar", "-x", "-C", temporary], input=archive, check=True)
            done = subprocess.run(
                ["just", "--justfile", f"{temporary}/Justfile", "--working-directory", temporary, "--dump", "--dump-format", "json"],
                check=False,
                text=True,
                capture_output=True,
            )
        if done.returncode != 0:
            raise SpecError(f"just --dump at {rev} failed\n{done.stderr}")
        dump = json.loads(done.stdout)
        recipes = dict(dump.get("recipes", {}))
        for module in dump.get("modules", {}).values():
            recipes.update(module.get("recipes", {}))
        return cls(recipes)

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


def git(*arguments: str, check: bool = True, env: dict | None = None, data: bytes | None = None) -> str:
    done = subprocess.run(["git", *arguments], cwd=ROOT, check=False, capture_output=True, env=env, input=data)
    if check and done.returncode != 0:
        raise SpecError(f"git {' '.join(arguments)} failed: {done.stderr.decode().strip()}")
    return done.stdout.decode()


def read_blob(ref: str, path: str) -> bytes | None:
    """`path` at `ref`, or None when either is absent."""
    done = subprocess.run(["git", "cat-file", "-p", f"{ref}:{path}"], cwd=ROOT, check=False, capture_output=True)
    return done.stdout if done.returncode == 0 else None


def evidence_ref() -> str | None:
    """The first evidence ref that exists: $SPEC_EVIDENCE_REF, origin/evidence, evidence."""
    for candidate in (os.environ.get("SPEC_EVIDENCE_REF"), "refs/remotes/origin/evidence", EVIDENCE_REF):
        if candidate and git("rev-parse", "-q", "--verify", candidate + "^{commit}", check=False).strip():
            return candidate
    return None


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


def human_record_text(area: str, requirement: str, acceptance: str, digest: str, value: bool, note: str, signer: str, observed_at: str) -> str:
    return (
        'o ::= import "./schema.zt";\n'
        "r :: o.HumanRecord = {\n"
        "  schema = o.schemaVersion;\n"
        f"  area = {zt_text(area)};\n"
        f"  requirement = {zt_text(requirement)};\n"
        f"  acceptance = {zt_text(acceptance)};\n"
        f"  requirementsDigest = {zt_text(digest)};\n"
        f"  value = {'true' if value else 'false'};\n"
        f"  note = {zt_text(note)};\n"
        f"  signer = {zt_text(signer)};\n"
        f"  observedAt = {zt_text(observed_at)};\n"
        "};\nr\n"
    )


def human_path(requirement: dict, acceptance_id: str) -> str:
    return f"human/{requirement['id']}/{acceptance_id}/{requirement_digest(requirement)}.zt"


def sign(path: Path, key: str) -> Path:
    """`ssh-keygen -Y sign` the file; returns the `.sig` path."""
    signature = path.with_name(path.name + ".sig")
    signature.unlink(missing_ok=True)
    done = subprocess.run(["ssh-keygen", "-q", "-Y", "sign", "-f", key, "-n", SIGN_NAMESPACE, str(path)], check=False, capture_output=True, text=True)
    if done.returncode != 0:
        raise SpecError(f"ssh-keygen -Y sign failed: {done.stderr.strip()}")
    return signature


def verify_signature(data: bytes, signature: bytes | None, signer: str) -> bool:
    """True when `signature` over `data` verifies for `signer` in specs/allowed_signers."""
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


def human_owners() -> set[str]:
    if not HUMAN_OWNERS.is_file():
        return set()
    return {line.strip() for line in HUMAN_OWNERS.read_text().splitlines() if line.strip() and not line.startswith("#")}


def load_areas_at(rev: str) -> list[Area]:
    """Areas as committed at `rev`, evaluated from a temporary extraction."""
    listing = git("ls-tree", "--name-only", rev, "specs/", check=False)
    names = [line.split("/", 1)[1] for line in listing.splitlines() if line.endswith(".zt")]
    if not names:
        return []
    with tempfile.TemporaryDirectory(prefix="slime-spec-base-") as temporary:
        root = Path(temporary)
        for name in names:
            blob = read_blob(rev, f"specs/{name}")
            if blob is not None:
                (root / name).write_bytes(blob)
        areas = []
        for name in sorted(names):
            if name in {"schema.zt", "helpers.zt", "legacy.zt"}:
                continue
            value = zutai_json(root / name)
            if isinstance(value, dict) and value.get("schema") == SCHEMA_VERSION:
                areas.append(Area(SPECS / name, value))
        return areas


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
                    out = ("enforced" if ok else "failing", f"exit {record['exitCode']}" if not ok else f"record at {digest[:12]}", record)
                except SpecError as error:
                    out = ("failing", f"bad record: {str(error).splitlines()[0]}", None)
        self._records[target] = out
        return out


def _predicate(predicate: dict, counts: dict[str, int], flags: dict[str, bool]) -> str | None:
    tag, payload = predicate["tag"], predicate["payload"]
    key = payload["key"]
    if tag == "flag":
        return None if flags.get(key) is True else f"flag {key} is {flags.get(key)}"
    if key not in counts:
        return f"count {key} missing"
    value = counts[key]
    if tag == "count" and value != payload["equals"]:
        return f"{key} = {value}, want {payload['equals']}"
    if tag == "atLeast" and value < payload["min"]:
        return f"{key} = {value}, want >= {payload['min']}"
    if tag == "atMost" and value > payload["max"]:
        return f"{key} = {value}, want <= {payload['max']}"
    return None


def acceptance_state(tag: str, payload: dict, requirement: dict, evidence: Evidence) -> tuple[str, str]:
    """(enforced | failing | claimed | advisory, detail)."""
    if tag == "observed":
        state, detail, record = evidence.record(payload["justTarget"])
        if state == "enforced" and record is not None:
            counts = {c["key"]: c["value"] for c in record["counts"]}
            flags = {f["key"]: f["value"] for f in record["flags"]}
            failed = [m for m in (_predicate(p, counts, flags) for p in payload["predicates"]) if m]
            if failed:
                return "failing", "; ".join(failed)
        return state, detail
    path = human_path(requirement, payload["id"])
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
