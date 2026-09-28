"""The smoltcp support matrix, checked against the pinned release it describes.

`docs/plans/network-data-plane.md` owns a version-bound matrix of the pinned
smoltcp release. This module refuses the matrix when it stops describing what
is built: the lockfile pins a different version or crate checksum, the pinned
release advertises a Cargo feature the matrix has no row for, a row names a
feature the release does not provide, or the enabled feature set and the
`supported` rows disagree. A version bump or a feature change therefore fails
until the matrix is reviewed, instead of silently widening or narrowing the
claim.

The advertised feature set is read from the pinned crate archive in Cargo's
registry cache, and only after its SHA-256 matches the lockfile checksum, so a
different archive with the same version cannot stand in for the pinned one.
The archive is fetched by `cargo fetch --locked`; this module never touches the
network.

Matrix format, inside the `## smoltcp support matrix` section:

- one line `Pinned release: smoltcp `<version>`, crate checksum `<sha256>`.`;
- one or more tables whose header is exactly `COLUMNS`;
- a `feature` row names exactly one Cargo feature as `` `name` ``, once.

Row rules by status:

- `supported`: the feature is in the manifest's enabled closure; slice names a
  known work item; authority and bounds are stated; verification names an
  existing `just` recipe.
- `planned`: slice names at least one live (unfinished) work item, since
  unfinished work is not an exclusion.
- `excluded`: authority states the violated invariant (`Invariant:`) and the
  considered bounded alternative (`Alternative:`); verification names the
  `just` recipe that executes the denial or build-time exclusion.
- `not-applicable`: the release provides nothing applicable here; the
  authority cell says why.

A feature row that is not `supported` must not be enabled, so an excluded or
planned feature cannot enter the build without the matrix changing first.
"""

from __future__ import annotations

import hashlib
import io
import os
import re
import tarfile
import tempfile
import tomllib
from collections.abc import Callable, Iterable, Mapping
from dataclasses import dataclass
from pathlib import Path

from harness import ROOT

CRATE = "smoltcp"
DOCUMENT = ROOT / "docs" / "plans" / "network-data-plane.md"
SERVICE_MANIFEST = ROOT / "components" / "services" / "network-service" / "Cargo.toml"
LOCKFILE = ROOT / "Cargo.lock"
SECTION = "## smoltcp support matrix"
PIN = re.compile(
    r"^Pinned release: smoltcp `(?P<version>[^`]+)`, crate checksum `(?P<checksum>[0-9a-f]{64})`\.$",
    re.MULTILINE,
)
COLUMNS = (
    "Row",
    "Kind",
    "Upstream",
    "Status",
    "Backend",
    "Slice",
    "Authority",
    "Bounds",
    "Verification",
)
KINDS = frozenset({"feature", "protocol", "socket", "medium", "configuration", "resource"})
STATUSES = frozenset({"supported", "planned", "excluded", "not-applicable"})
NONE = "—"
FEATURE_CELL = re.compile(r"^`(?P<name>[A-Za-z0-9_][A-Za-z0-9_-]*)`$")
UUID_TEXT = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
JUST_LITERAL = re.compile(r"`just (?P<recipe>[A-Za-z_][A-Za-z0-9_-]*)`")
FINISHED = frozenset({"done", "cancelled"})
# Directories that never hold a first-party manifest.
SKIPPED = frozenset({".git", "deps", "target", "build", "node_modules", "result"})


class MatrixError(Exception):
    """An input the check depends on could not be read as required."""


@dataclass(frozen=True)
class Declaration:
    version: str
    default_features: bool
    features: tuple[str, ...]


@dataclass(frozen=True)
class Row:
    line: int
    cells: dict[str, str]


@dataclass(frozen=True)
class Matrix:
    version: str
    checksum: str
    rows: tuple[Row, ...]


def pinned(lock: str) -> tuple[str, str]:
    """The single locked smoltcp version and its registry checksum."""
    try:
        packages = tomllib.loads(lock).get("package", [])
    except tomllib.TOMLDecodeError as error:
        raise MatrixError(f"Cargo.lock does not parse: {error}") from error
    found = [package for package in packages if package.get("name") == CRATE]
    if len(found) != 1:
        raise MatrixError(f"Cargo.lock pins {len(found)} {CRATE} packages; exactly one is required")
    package = found[0]
    checksum = package.get("checksum")
    if not isinstance(checksum, str) or not re.fullmatch(r"[0-9a-f]{64}", checksum):
        raise MatrixError(f"Cargo.lock records no registry checksum for {CRATE}")
    return str(package["version"]), checksum


def declaration(manifest: str, source: str) -> Declaration | None:
    """A manifest's smoltcp dependency, or None when it declares none."""
    try:
        parsed = tomllib.loads(manifest)
    except tomllib.TOMLDecodeError as error:
        raise MatrixError(f"{source} does not parse: {error}") from error
    tables = [parsed.get("dependencies", {})]
    tables.extend(
        target.get("dependencies", {}) for target in parsed.get("target", {}).values()
    )
    entries = [table[CRATE] for table in tables if CRATE in table]
    if not entries:
        return None
    if len(entries) != 1 or not isinstance(entries[0], dict):
        raise MatrixError(f"{source} must declare {CRATE} once as a table with explicit features")
    entry = entries[0]
    version = entry.get("version", "")
    if not isinstance(version, str) or not version.startswith("="):
        raise MatrixError(f"{source} must pin {CRATE} with an exact `=` version")
    return Declaration(
        version=version[1:],
        default_features=bool(entry.get("default-features", True)),
        features=tuple(entry.get("features", ())),
    )


def advertised(archive: bytes, version: str) -> dict[str, tuple[str, ...]]:
    """Every Cargo feature the crate archive advertises, with what each enables.

    An optional dependency that no feature names through `dep:` is itself an
    implicit feature, so it is advertised too.
    """
    member = f"{CRATE}-{version}/Cargo.toml"
    try:
        with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as crate:
            handle = crate.extractfile(member)
            if handle is None:
                raise MatrixError(f"crate archive has no {member}")
            manifest = tomllib.loads(handle.read().decode())
    except (tarfile.TarError, KeyError, UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        raise MatrixError(f"crate archive does not yield {member}: {error}") from error
    features = {name: tuple(enables) for name, enables in manifest.get("features", {}).items()}
    explicit = {
        enable[len("dep:") :]
        for enables in features.values()
        for enable in enables
        if enable.startswith("dep:")
    }
    for name, dependency in manifest.get("dependencies", {}).items():
        if isinstance(dependency, dict) and dependency.get("optional") and name not in explicit:
            features.setdefault(name, ())
    return features


def cached_archive(version: str, checksum: str) -> bytes:
    """The pinned crate archive from Cargo's registry cache, checksum-verified."""
    home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    candidates = sorted((home / "registry" / "cache").glob(f"*/{CRATE}-{version}.crate"))
    for candidate in candidates:
        data = candidate.read_bytes()
        if hashlib.sha256(data).hexdigest() == checksum:
            return data
    if candidates:
        raise MatrixError(
            f"no cached {CRATE}-{version}.crate matches the Cargo.lock checksum {checksum}"
        )
    raise MatrixError(
        f"{CRATE}-{version}.crate is not in {home / 'registry' / 'cache'}; run `cargo fetch --locked`"
    )


def enabled(declared: Declaration, features: Mapping[str, tuple[str, ...]]) -> set[str]:
    """The transitive feature closure a declaration turns on in the pinned crate."""
    pending = list(declared.features) + (["default"] if declared.default_features else [])
    closure: set[str] = set()
    while pending:
        name = pending.pop()
        if name in closure or name not in features:
            continue
        closure.add(name)
        for enable in features[name]:
            # `dep:x`, `x/feature` and `x?/feature` name dependencies, not
            # features of this crate.
            if not enable.startswith("dep:") and "/" not in enable:
                pending.append(enable)
    return closure


def parse(document: str) -> Matrix:
    """The matrix section of the owning plan page."""
    lines = document.splitlines()
    try:
        start = lines.index(SECTION)
    except ValueError as error:
        raise MatrixError(f"{DOCUMENT.name} has no `{SECTION}` section") from error
    end = next(
        (
            index
            for index in range(start + 1, len(lines))
            if lines[index].startswith("## ") or lines[index].startswith("# ")
        ),
        len(lines),
    )
    section = "\n".join(lines[start:end])
    pins = PIN.findall(section)
    if len(pins) != 1:
        raise MatrixError(f"the matrix section must carry exactly one pin line, found {len(pins)}")
    version, checksum = pins[0]
    rows: list[Row] = []
    index = start + 1
    tables = 0
    while index < end:
        if not lines[index].startswith("|"):
            index += 1
            continue
        header = cells(lines[index])
        if tuple(header) != COLUMNS:
            raise MatrixError(
                f"line {index + 1}: matrix table header must be {' | '.join(COLUMNS)}"
            )
        if index + 1 >= end or not re.fullmatch(r"\|(\s*:?-+:?\s*\|)+", lines[index + 1].strip()):
            raise MatrixError(f"line {index + 1}: matrix table has no delimiter row")
        tables += 1
        index += 2
        while index < end and lines[index].startswith("|"):
            values = cells(lines[index])
            if len(values) != len(COLUMNS):
                raise MatrixError(
                    f"line {index + 1}: row has {len(values)} cells, expected {len(COLUMNS)}"
                )
            rows.append(Row(line=index + 1, cells=dict(zip(COLUMNS, values, strict=True))))
            index += 1
    if not tables:
        raise MatrixError("the matrix section holds no table")
    return Matrix(version=version, checksum=checksum, rows=tuple(rows))


def cells(line: str) -> list[str]:
    return [cell.strip() for cell in line.strip().strip("|").split("|")]


def row_failures(
    row: Row,
    *,
    enabled_features: set[str],
    states: Mapping[str, str],
    recipes: frozenset[str],
) -> list[str]:
    """What one row fails to state for its status."""
    where = f"line {row.line}"
    value = row.cells
    failures = [f"{where}: {column} is empty" for column in COLUMNS if not value[column]]
    if failures:
        return failures
    kind, status = value["Kind"], value["Status"]
    if kind not in KINDS:
        failures.append(f"{where}: kind {kind!r} is not one of {sorted(KINDS)}")
    if status not in STATUSES:
        return [*failures, f"{where}: status {status!r} is not one of {sorted(STATUSES)}"]
    if value["Upstream"] == NONE:
        failures.append(f"{where}: every row names its upstream feature or source reference")
    slices = UUID_TEXT.findall(value["Slice"])
    unknown = [identity for identity in slices if identity not in states]
    failures.extend(f"{where}: slice {identity} is not a work item" for identity in unknown)
    if value["Slice"] != NONE and not slices:
        failures.append(f"{where}: slice must name canonical work-item UUIDs or {NONE}")
    literals = JUST_LITERAL.findall(value["Verification"])
    failures.extend(
        f"{where}: verification names absent recipe `just {recipe}`"
        for recipe in literals
        if recipe not in recipes
    )
    if status == "supported":
        if not slices:
            failures.append(f"{where}: a supported row names the work item that delivered it")
        for column in ("Backend", "Authority", "Bounds"):
            if value[column] == NONE:
                failures.append(f"{where}: a supported row states its {column.lower()}")
        if not literals:
            failures.append(f"{where}: a supported row names the `just` recipe that observes it")
    elif status == "planned":
        live = [identity for identity in slices if states.get(identity) not in FINISHED | {None}]
        if not live:
            failures.append(f"{where}: a planned row names a live owning work item")
    elif status == "excluded":
        authority = value["Authority"]
        if "Invariant:" not in authority or "Alternative:" not in authority:
            failures.append(
                f"{where}: an excluded row states `Invariant:` and the considered `Alternative:`"
            )
        if not literals:
            failures.append(f"{where}: an excluded row names the `just` recipe enforcing it")
    elif value["Authority"] == NONE:
        failures.append(f"{where}: a not-applicable row says why in its authority cell")
    if kind == "feature":
        match = FEATURE_CELL.match(value["Row"])
        if match is None:
            failures.append(f"{where}: a feature row names one feature as `name`")
        elif (match["name"] in enabled_features) != (status == "supported"):
            failures.append(
                f"{where}: feature {match['name']} is "
                + ("enabled but not supported" if status != "supported" else "supported but not enabled")
            )
    return failures


def evaluate(
    *,
    document: str,
    lock: str,
    service: str,
    manifests: Mapping[str, str],
    archive: Callable[[str, str], bytes],
    states: Mapping[str, str],
    recipes: frozenset[str],
) -> list[str]:
    """Every way the matrix and the pinned, enabled release disagree.

    `service` names the manifest in `manifests` that owns the enabled set; any
    other manifest that declares smoltcp must declare exactly the same thing,
    because it compiles the same engine.
    """
    try:
        version, checksum = pinned(lock)
        declarations = {
            source: found
            for source, text in manifests.items()
            if (found := declaration(text, source)) is not None
        }
        if service not in declarations:
            raise MatrixError(f"{service} declares no {CRATE} dependency")
        matrix = parse(document)
        features = advertised(archive(version, checksum), version)
    except MatrixError as error:
        return [str(error)]
    failures: list[str] = []
    owner = declarations[service]
    if owner.version != version:
        failures.append(f"{service} pins {CRATE} {owner.version}, Cargo.lock pins {version}")
    for source, other in sorted(declarations.items()):
        if source != service and other != owner:
            failures.append(f"{source} declares {CRATE} differently from {service}")
    if matrix.version != version:
        failures.append(f"matrix describes {CRATE} {matrix.version}, Cargo.lock pins {version}")
    if matrix.checksum != checksum:
        failures.append(f"matrix records crate checksum {matrix.checksum}, Cargo.lock has {checksum}")
    for name in owner.features:
        if name not in features:
            failures.append(f"{service} enables {name}, which {CRATE} {version} does not provide")
    closure = enabled(owner, features)
    seen: dict[str, int] = {}
    for row in matrix.rows:
        failures.extend(
            row_failures(row, enabled_features=closure, states=states, recipes=recipes)
        )
        if row.cells["Kind"] != "feature":
            continue
        match = FEATURE_CELL.match(row.cells["Row"])
        if match is None:
            continue
        name = match["name"]
        if name in seen:
            failures.append(f"line {row.line}: feature {name} already has a row at line {seen[name]}")
        seen.setdefault(name, row.line)
        if name not in features:
            failures.append(f"line {row.line}: {CRATE} {version} does not provide feature {name}")
    for name in sorted(set(features) - set(seen)):
        failures.append(
            f"{CRATE} {version} advertises feature {name}"
            + (", which is enabled," if name in closure else "")
            + " but the matrix has no row for it"
        )
    return failures


def manifests(root: Path = ROOT) -> dict[str, str]:
    """Every first-party Cargo manifest, keyed by its repository-relative path."""
    found: dict[str, str] = {}
    for directory, children, files in os.walk(root):
        children[:] = sorted(child for child in children if child not in SKIPPED)
        if "Cargo.toml" in files:
            path = Path(directory) / "Cargo.toml"
            found[str(path.relative_to(root))] = path.read_text()
    return found


def check(
    *,
    states: Mapping[str, str],
    recipes: frozenset[str],
    archive: Callable[[str, str], bytes] = cached_archive,
) -> list[str]:
    """The repository's matrix against its lockfile, manifests and pinned crate."""
    return evaluate(
        document=DOCUMENT.read_text(),
        lock=LOCKFILE.read_text(),
        service=str(SERVICE_MANIFEST.relative_to(ROOT)),
        manifests=manifests(),
        archive=archive,
        states=states,
        recipes=recipes,
    )


def _crate(version: str, manifest: str) -> bytes:
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz") as crate:
        data = manifest.encode()
        info = tarfile.TarInfo(f"{CRATE}-{version}/Cargo.toml")
        info.size = len(data)
        crate.addfile(info, io.BytesIO(data))
    return buffer.getvalue()


def controls() -> list[str]:
    """Prove the check passes a consistent fixture and refuses each drift."""
    live = "00000000-0000-7000-8000-000000000001"
    finished = "00000000-0000-7000-8000-000000000002"
    states = {live: "open", finished: "done"}
    recipes = frozenset({"fixture_check"})
    upstream = (
        '[package]\nname = "smoltcp"\nversion = "9.9.9"\n'
        "[features]\n"
        'default = ["socket-tcp"]\nsocket = []\nsocket-tcp = ["socket"]\n'
        'proto-extra = []\nlegacy = ["dep:helper"]\n'
        "[dependencies]\n"
        'helper = { version = "1", optional = true }\n'
        'logger = { version = "1", optional = true }\n'
    )
    archive = _crate("9.9.9", upstream)
    checksum = hashlib.sha256(archive).hexdigest()
    lock = f'[[package]]\nname = "smoltcp"\nversion = "9.9.9"\nchecksum = "{checksum}"\n'
    service = "service/Cargo.toml"
    manifest = (
        '[dependencies]\nsmoltcp = { version = "=9.9.9", default-features = false, '
        'features = ["socket-tcp"] }\n'
    )
    header = "| " + " | ".join(COLUMNS) + " |\n|" + "---|" * len(COLUMNS) + "\n"
    supported = f"supported | QEMU | {live} | Exact grants | 4 sockets | `just fixture_check` |"
    rows = {
        "socket": f"| `socket` | feature | features | {supported}",
        "socket-tcp": f"| `socket-tcp` | feature | features | {supported}",
        "default": "| `default` | feature | features | not-applicable | — | — | Implies std | — | — |",
        "proto-extra": f"| `proto-extra` | feature | features | planned | QEMU | {live} | Pending | Pending | — |",
        "legacy": (
            "| `legacy` | feature | features | excluded | — | — | Invariant: no ambient "
            "authority. Alternative: none bounded. | — | `just fixture_check` |"
        ),
        "logger": "| `logger` | feature | dependencies | not-applicable | — | — | Host logging | — | — |",
        "tcp": f"| TCP stream | socket | src/socket/tcp.rs | {supported}",
    }

    def document(version: str = "9.9.9", sum_: str = checksum, **replace: str | None) -> str:
        body = [line for name, line in {**rows, **replace}.items() if line is not None]
        return (
            "# Plan\n\n"
            f"{SECTION}\n\nPinned release: smoltcp `{version}`, crate checksum `{sum_}`.\n\n"
            + header
            + "\n".join(body)
            + "\n\n## Next\n\n| Unrelated | table |\n"
        )

    def run(text: str, *, lock_text: str = lock, service_text: str = manifest,
            extra: Mapping[str, str] | None = None, data: bytes = archive) -> list[str]:
        def fetch(version: str, expected: str) -> bytes:
            if hashlib.sha256(data).hexdigest() != expected:
                raise MatrixError(f"no cached archive matches the Cargo.lock checksum {expected}")
            return data

        return evaluate(
            document=text,
            lock=lock_text,
            service=service,
            manifests={service: service_text, **(extra or {})},
            archive=fetch,
            states=states,
            recipes=recipes,
        )

    enable_extra = manifest.replace('["socket-tcp"]', '["socket-tcp", "proto-extra"]')
    cases: Iterable[tuple[str, list[str], str | None]] = (
        ("unmodified tree", run(document()), None),
        ("removed row", run(document(**{"proto-extra": None})), "no row for it"),
        ("edited matrix version", run(document(version="9.9.8")), "matrix describes smoltcp 9.9.8"),
        (
            "edited lock version",
            run(document(), lock_text=lock.replace('"9.9.9"', '"9.9.8"')),
            "does not yield",
        ),
        ("edited checksum", run(document(sum_="0" * 64)), "matrix records crate checksum"),
        (
            "enabled feature without a row",
            run(document(**{"proto-extra": None}), service_text=enable_extra),
            "which is enabled, but the matrix has no row",
        ),
        ("enabled but planned", run(document(), service_text=enable_extra), "enabled but not supported"),
        (
            "supported but not enabled",
            run(document(**{"proto-extra": f"| `proto-extra` | feature | features | {supported}"})),
            "supported but not enabled",
        ),
        (
            "feature the release does not provide",
            run(document(ghost="| `ghost` | feature | features | not-applicable | — | — | None | — | — |")),
            "does not provide feature ghost",
        ),
        (
            "enabling a feature the release does not provide",
            run(document(), service_text=manifest.replace('["socket-tcp"]', '["socket-tcp", "ghost"]')),
            "enables ghost",
        ),
        (
            "duplicate row",
            run(document(again=rows["socket"])),
            "already has a row",
        ),
        (
            "planned slice finished",
            run(document(**{"proto-extra": rows["proto-extra"].replace(live, finished)})),
            "names a live owning work item",
        ),
        (
            "unknown slice",
            run(document(**{"proto-extra": rows["proto-extra"].replace(live, "0" * 8 + live[8:-1] + "9")})),
            "is not a work item",
        ),
        (
            "exclusion without invariant",
            run(document(legacy=rows["legacy"].replace("Invariant:", "Because"))),
            "states `Invariant:`",
        ),
        (
            "supported without recipe",
            run(document(tcp=rows["tcp"].replace("`just fixture_check`", "manual review"))),
            "names the `just` recipe that observes it",
        ),
        (
            "absent recipe",
            run(document(tcp=rows["tcp"].replace("fixture_check", "gone_check"))),
            "absent recipe `just gone_check`",
        ),
        ("bad status", run(document(tcp=rows["tcp"].replace("supported", "partial"))), "status 'partial'"),
        ("empty cell", run(document(tcp=rows["tcp"].replace("QEMU", ""))), "Backend is empty"),
        ("missing section", run(document().replace(SECTION, "## Other")), "has no `## smoltcp"),
        (
            "wrong header",
            run(document().replace("| Bounds |", "| Limits |")),
            "matrix table header must be",
        ),
        (
            "archive differs from the lockfile checksum",
            run(document(), data=_crate("9.9.9", upstream + "\n")),
            "matches the Cargo.lock checksum",
        ),
        (
            "second manifest diverges",
            run(document(), extra={"tests/Cargo.toml": enable_extra}),
            "declares smoltcp differently",
        ),
        (
            "unpinned service version",
            run(document(), service_text=manifest.replace("=9.9.9", "9.9.9")),
            "exact `=` version",
        ),
    )
    failures: list[str] = []
    for name, found, signal in cases:
        if signal is None and found:
            failures.append(f"smoltcp matrix control {name}: expected a pass, got {found}")
        elif signal is not None and not any(signal in failure for failure in found):
            failures.append(f"smoltcp matrix control {name}: expected {signal!r}, got {found}")
    with tempfile.TemporaryDirectory() as temporary:
        home = Path(temporary)
        cache = home / "registry" / "cache" / "index"
        cache.mkdir(parents=True)
        previous = os.environ.get("CARGO_HOME")
        os.environ["CARGO_HOME"] = str(home)
        try:
            for name, contents, signal in (
                ("absent archive", None, "cargo fetch --locked"),
                ("substituted archive", _crate("9.9.9", upstream + "\n"), "matches the Cargo.lock"),
                ("pinned archive", archive, None),
            ):
                if contents is not None:
                    (cache / f"{CRATE}-9.9.9.crate").write_bytes(contents)
                try:
                    cached_archive("9.9.9", checksum)
                    outcome = None
                except MatrixError as error:
                    outcome = str(error)
                if (signal is None) != (outcome is None) or (
                    signal is not None and outcome is not None and signal not in outcome
                ):
                    failures.append(f"smoltcp archive control {name}: expected {signal!r}, got {outcome!r}")
        finally:
            if previous is None:
                os.environ.pop("CARGO_HOME", None)
            else:
                os.environ["CARGO_HOME"] = previous
    return failures
