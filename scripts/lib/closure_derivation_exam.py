"""The exam for closure sources that carry no in-tree digest (step 2 of the closure-derivation plan).

A committed v2 closure recorded the SHA-256 identity of each input tree, so every change to an input
rewrote the closures. After this step a committed closure is a source: composition choices and input
locators only. Each checkout resolves it into a v2 `SystemImageClosure`, whose identity is the
derivation id. This module owns what that must mean, and it imports nothing it judges:

* the sources: no committed closure source holds a digest, every pinned closure has a source, and no
  resolved v2 closure stays committed;
* the resolver: a resolved closure's identity, recomputed here from its JSON projection, equals the
  derivation id the resolver reports, and every in-tree and submodule artifact's identity equals the
  digest this module computes over the files git lists;
* the records: the resolver's report decodes through `contracts/closure-resolution/v1` and the
  builder's `build-result.zti` decodes through `contracts/system-image-closure/v2`'s
  `decodeSystemImageBuildResult`, each with exactly the fields this exam reads.

`scripts/check/check-closure-derivations.py` runs the resolver, the generator and the builder as
commands. Each judge here is trusted only after it refuses the corruptions in `controls()`.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
import tempfile
from collections.abc import Iterator
from pathlib import Path

SOURCE_GLOB = "contracts/system-image-closure/v3/closures/*.zti"
REPORT_SCHEMA = "contracts/closure-resolution/v1/schema.zt"
CLOSURE_SCHEMA = "contracts/system-image-closure/v2/schema.zt"
REPORT_FIELDS = ("derivationId", "dirty", "formatVersion", "name")
RESOLVED_V2_DIR = "contracts/system-image-closure/v2/closures"
IDENTITY_DOMAIN = b"slime-system-image-closure-v2:"
TREE_DOMAIN = b"slime-component-sdk-tree-v1\0"
DIGEST = re.compile(r"[0-9a-f]{64}")

# Every closure at the planning base, 00af4fde..80c960e5 (#893). A closure added later needs a source too,
# but only these must exist.
PINNED_CLOSURES = (
    "sel4", "sel4-boot", "sel4-boot-selector", "sel4-c-runtime", "sel4-call", "sel4-channel",
    "sel4-channel-fixture", "sel4-clock-authority", "sel4-crossing", "sel4-demo", "sel4-directory",
    "sel4-entropy", "sel4-fault", "sel4-filesystem", "sel4-generation", "sel4-http",
    "sel4-http-public", "sel4-input", "sel4-io-block", "sel4-io-driver-authority",
    "sel4-io-driver-reset", "sel4-io-driver-reset-in-flight", "sel4-io-lifetime", "sel4-io-link",
    "sel4-io-local", "sel4-io-network", "sel4-io-queue", "sel4-io-service-fault",
    "sel4-io-service-fault-in-flight", "sel4-io-tcp", "sel4-io-tcp-buffers",
    "sel4-io-tcp-impairment", "sel4-io-tcp-listener", "sel4-io-tcp-options", "sel4-large-image",
    "sel4-lifecycle-restart", "sel4-lifetime", "sel4-lifetime-exit", "sel4-loan", "sel4-matrix",
    "sel4-matrix-unsatisfiable", "sel4-mavlink", "sel4-net", "sel4-operation", "sel4-powerbox",
    "sel4-private-memory", "sel4-private-memory-1g", "sel4-private-memory-adaptive",
    "sel4-private-memory-adaptive-lifecycle", "sel4-private-memory-adaptive-overcommit",
    "sel4-private-memory-adaptive-rv64", "sel4-private-memory-bootstrap",
    "sel4-private-memory-bootstrap-rv64", "sel4-private-memory-conservation",
    "sel4-private-memory-conservation-rv64", "sel4-private-memory-cspace",
    "sel4-private-memory-cspace-rv64", "sel4-private-memory-cycles", "sel4-private-memory-elastic",
    "sel4-private-memory-elastic-rv64", "sel4-private-memory-fail-large-map",
    "sel4-private-memory-fail-second-allocation", "sel4-private-memory-fragmentation",
    "sel4-private-memory-fragmentation-rv64", "sel4-private-memory-heap-stress",
    "sel4-private-memory-heap-stress-rv64", "sel4-private-memory-isolation",
    "sel4-private-memory-matrix", "sel4-private-memory-matrix-rv64", "sel4-private-memory-metadata",
    "sel4-private-memory-metadata-rv64", "sel4-private-memory-rollback",
    "sel4-private-memory-rollback-rv64", "sel4-private-memory-stress",
    "sel4-private-memory-stress-injected", "sel4-private-memory-stress-rv64",
    "sel4-private-memory-stress-rv64-injected", "sel4-pwm", "sel4-qos", "sel4-qos-death",
    "sel4-reclamation", "sel4-reclamation-unwind", "sel4-recovery", "sel4-replay",
    "sel4-robot-runtime", "sel4-rollback", "sel4-sample", "sel4-saturation",
    "sel4-scheduling-class", "sel4-slisp", "sel4-spawn", "sel4-storage", "sel4-store",
    "sel4-stream", "sel4-stream-death", "sel4-stress", "sel4-supervision", "sel4-traffic",
    "sel4-transfer", "sel4-visibility", "sel4-wait-set", "sel4-zenoh", "sel4-zenoh-external",
)

# Closures the resolver is asked for: plain, with application inputs, over a computed system, a scenario,
# a root role, and a riscv64 computed system.
SAMPLE = (
    "sel4-channel", "sel4-zenoh", "sel4-call", "sel4-io-driver-reset-in-flight",
    "sel4-private-memory-stress-injected", "sel4-private-memory-adaptive-rv64",
)


def source_findings(sources: dict[str, str], resolved_v2: list[str]) -> list[str]:
    """Findings for the committed closure sources, keyed by closure name."""
    findings = []
    if resolved_v2:
        findings.append(f"{len(resolved_v2)} resolved v2 closure(s) remain under {RESOLVED_V2_DIR}, e.g. {resolved_v2[0]}")
    missing = [name for name in PINNED_CLOSURES if name not in sources]
    if missing:
        findings.append(f"{len(missing)} pinned closure(s) have no source, e.g. {missing[0]}")
    for name, text in sorted(sources.items()):
        match = DIGEST.search(text)
        if match is not None:
            findings.append(f"{name}: the source holds a digest, {match.group(0)[:16]}…")
        if re.search(r"(?<![\w-])\w*(?:identity|Identity)\s*=", text):
            findings.append(f"{name}: the source declares an identity field")
    return findings


def decode_record(schema: Path, decoder: str, type_name: str, record: Path) -> dict:
    """Decode `record` with `decoder` from a copy of `schema`. Raises ValueError unless it decodes.

    Zutai imports only inside the evaluated file's directory, so the schema is copied beside a
    generated entry point rather than imported from the checkout.
    """
    import zutai_cli

    with tempfile.TemporaryDirectory(prefix="slime-derivation-decode-") as directory:
        root = Path(directory)
        (root / "schema.zt").write_text(schema.read_text(encoding="utf-8"), encoding="utf-8")
        (root / "decode.zt").write_text(
            's ::= import "schema.zt";\n'
            f"main :: Load -> Validation DecodeIssue s.{type_name} ! {{ load.zti : Path -> Data; }}\n"
            f"  = load => s.{decoder} (loadZti {json.dumps(str(record.resolve()))});\n"
            "main\n",
            encoding="utf-8",
        )
        result = subprocess.run([str(zutai_cli.binary()), "json", str(root / "decode.zt")], cwd=root,
                                capture_output=True, text=True,
                                env={**os.environ, "ZUTAI_STDLIB_ROOT": str(zutai_cli.STDLIB)})
    if result.returncode != 0:
        raise ValueError(f"{record.name} could not be decoded: {(result.stderr or result.stdout).strip()[:300]}")
    decoded = json.loads(result.stdout)
    if not isinstance(decoded, dict) or decoded.get("tag") != "valid":
        raise ValueError(f"{record.name} does not decode through {decoder}: {json.dumps(decoded)[:300]}")
    return decoded["payload"]["value"]


def report_findings(name: str, report: dict, raw: dict) -> list[str]:
    """Findings for a decoded resolution report. `raw` is the record's plain projection, so a field the
    decoder ignores is still found."""
    findings = []
    if tuple(sorted(raw)) != REPORT_FIELDS:
        findings.append(f"{name}: the report's fields are {sorted(raw)}, not {list(REPORT_FIELDS)}")
    if report.get("formatVersion") != 1:
        findings.append(f"{name}: the report's formatVersion is {report.get('formatVersion')!r}, not 1")
    if report.get("name") != name:
        findings.append(f"{name}: the report names {report.get('name')!r}")
    if not isinstance(report.get("derivationId"), str) or DIGEST.fullmatch(report["derivationId"]) is None:
        findings.append(f"{name}: the report's derivationId is not a lowercase SHA-256")
    if not isinstance(report.get("dirty"), bool):
        findings.append(f"{name}: the report's dirty field is not a boolean")
    return findings


def build_result_findings(label: str, result: dict, derivation_id: str, dirty: bool) -> list[str]:
    """Findings for a decoded build result against the clean derivation id and the expected dirty flag."""
    findings = []
    if result.get("dirty") is not dirty:
        findings.append(f"{label}: the build result says dirty={result.get('dirty')!r}, expected {dirty}")
    if result.get("closureIdentity") != derivation_id:
        findings.append(f"{label}: the build result names closure {str(result.get('closureIdentity'))[:16]}, "
                        f"the resolver reports {derivation_id[:16]}")
    return findings


def closure_identity(value: object) -> str:
    """A v2 closure's identity from its JSON projection: SHA-256 over the domain and canonical JSON."""
    normalized = (json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True) + "\n").encode("utf-8")
    return hashlib.sha256(IDENTITY_DOMAIN + normalized).hexdigest()


def _git_files(repository: Path, sub: str) -> list[str]:
    """Files git lists under `sub` of `repository`, relative to `repository`, recursing into submodules."""
    staged = subprocess.run(["git", "-C", str(repository), "ls-files", "-z", "--stage", "--", sub or "."],
                            check=True, capture_output=True).stdout.split(b"\0")
    gitlinks = {entry.split(b"\t", 1)[1].decode() for entry in staged if entry.startswith(b"160000 ")}
    listed = subprocess.run(["git", "-C", str(repository), "ls-files", "-z", "--cached", "--others",
                             "--exclude-standard", "--", sub or "."], check=True, capture_output=True).stdout
    found = []
    for raw in sorted({item.decode() for item in listed.split(b"\0") if item}):
        if raw in gitlinks:
            found.extend(f"{raw}/{inner}" for inner in _git_files(repository / raw, ""))
        elif (repository / raw).is_file():
            found.append(raw)
    return found


def _absorb(digest, value: bytes) -> None:
    digest.update(len(value).to_bytes(8, "little"))
    digest.update(value)


def tree_digest(checkout: Path, relative: str) -> str:
    """The tree encoding v2 closures use, over the files git lists under `relative`."""
    files = sorted(path[len(relative) + 1:] for path in _git_files(checkout, relative) if path.startswith(relative + "/"))
    files = [path for path in files if path != ".git" and not path.startswith(".git/")]
    digest = hashlib.sha256()
    digest.update(TREE_DOMAIN)
    digest.update(len(files).to_bytes(8, "little"))
    for path in files:
        full = checkout / relative / path
        _absorb(digest, path.encode("utf-8"))
        digest.update(b"\1" if os.access(full, os.X_OK) else b"\0")
        _absorb(digest, hashlib.sha256(full.read_bytes()).digest())
    return digest.hexdigest()


def artifacts(value: dict) -> Iterator[tuple[str, dict]]:
    """Every artifact reference in a resolved v2 closure, with a label."""
    yield "systemSpec", value["systemSpec"]
    for entry in value["implementations"]:
        yield f"implementation {entry['component']}", entry["artifact"]
    yield "target.prefix", value["target"]["prefix"]
    yield "root.implementation", value["root"]["implementation"]
    yield "loader.implementation", value["loader"]["implementation"]
    for entry in value["releaseInputs"]:
        yield f"release input {entry['name']}", entry["artifact"]


def resolved_findings(name: str, value: object, report: dict, checkout: Path) -> list[str]:
    """Findings for one resolved closure against its report and against the checkout's files."""
    findings = []
    if report["name"] != name or not isinstance(value, dict) or value.get("name") != name:
        return [f"{name}: the report or the resolved closure names another closure"]
    if closure_identity(value) != report["derivationId"]:
        findings.append(f"{name}: the resolved closure's identity is not the reported derivation id")
    for label, reference in artifacts(value):
        path = checkout / reference["path"]
        if reference["kind"] == "file":
            found = hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else "missing"
        elif reference["kind"] == "tree":
            found = tree_digest(checkout, reference["path"]) if path.is_dir() else "missing"
        else:
            findings.append(f"{name}: {label} has kind {reference['kind']!r}")
            continue
        if found != reference["identity"]:
            findings.append(f"{name}: {label} ({reference['path']}) is {found[:16]}, the closure says {reference['identity'][:16]}")
    return findings


# Controls: each corrupts honest evidence and requires a judge to find it.

def _source_controls() -> Iterator[tuple[str, bool]]:
    honest = {name: f'{{\n  name = "{name}";\n  systemSpec = {{ path = "x.zti"; kind = "file"; }};\n}}\n' for name in PINNED_CLOSURES}
    yield "sources-honest", not source_findings(honest, [])
    cases = {
        "sources-digest": ({**honest, "sel4": honest["sel4"].replace('"x.zti"', '"' + "a" * 64 + '"')}, []),
        "sources-identity-field": ({**honest, "sel4": honest["sel4"].replace("kind", "identity")}, []),
        "sources-system-identity": ({**honest, "sel4": honest["sel4"] + "  systemIdentity = \"\";\n"}, []),
        "sources-missing": ({k: v for k, v in honest.items() if k != "sel4-zenoh"}, []),
        "sources-resolved-remain": (honest, ["sel4.zti"]),
    }
    for label, (sources, resolved) in cases.items():
        yield label, bool(source_findings(sources, resolved))


def _report_controls(root: Path) -> Iterator[tuple[str, bool]]:
    """The report and build-result checks, with real decoding through the committed schemas."""
    good = {"formatVersion": 1, "name": "sel4-channel", "derivationId": "a" * 64, "dirty": False}
    yield "report-honest", not report_findings("sel4-channel", good, good)
    for label, value in {
        "report-extra-field": {**good, "closureIdentity": "a" * 64},
        "report-version-2": {**good, "formatVersion": 2},
        "report-other-name": {**good, "name": "sel4-zenoh"},
        "report-short-id": {**good, "derivationId": "a" * 63},
        "report-upper-id": {**good, "derivationId": "A" * 64},
        "report-dirty-text": {**good, "dirty": "false"},
    }.items():
        yield label, bool(report_findings("sel4-channel", value, value))
    schema = root / REPORT_SCHEMA
    with tempfile.TemporaryDirectory(prefix="slime-derivation-report-") as directory:
        record = Path(directory) / "report.zti"
        record.write_text('{\n  formatVersion = 1;\n  name = "sel4-channel";\n  derivationId = "' + "a" * 64
                          + '";\n  dirty = false;\n}\n', encoding="utf-8")
        try:
            decoded = decode_record(schema, "decodeResolutionReport", "ResolutionReport", record)
            yield "report-decodes", decoded == good
        except ValueError:
            yield "report-decodes", False
        record.write_text('{\n  formatVersion = 1;\n  name = "sel4-channel";\n  dirty = "no";\n}\n', encoding="utf-8")
        try:
            decode_record(schema, "decodeResolutionReport", "ResolutionReport", record)
            yield "report-malformed-refused", False
        except ValueError:
            yield "report-malformed-refused", True
    built = {"closureIdentity": "a" * 64, "dirty": False}
    yield "build-result-honest", not build_result_findings("clean", built, "a" * 64, False)
    yield "build-result-dirty-wrong", bool(build_result_findings("clean", built, "a" * 64, True))
    yield "build-result-dirty-missing", bool(build_result_findings("clean", {"closureIdentity": "a" * 64}, "a" * 64, False))
    yield "build-result-other-closure", bool(build_result_findings("clean", built, "b" * 64, False))


def _resolved_controls() -> Iterator[tuple[str, bool]]:
    with tempfile.TemporaryDirectory(prefix="slime-derivation-control-") as directory:
        checkout = Path(directory)
        subprocess.run(["git", "init", "-q", str(checkout)], check=True)
        (checkout / "tree").mkdir()
        (checkout / "tree" / "a.txt").write_text("a\n")
        (checkout / "spec.zti").write_text("{}\n")
        subprocess.run(["git", "-C", str(checkout), "add", "."], check=True)

        def reference(path: str, kind: str) -> dict:
            full = checkout / path
            identity = hashlib.sha256(full.read_bytes()).hexdigest() if kind == "file" else tree_digest(checkout, path)
            return {"path": path, "kind": kind, "identity": identity}

        def value() -> dict:
            tree = reference("tree", "tree")
            return {
                "name": "sel4-x", "systemSpec": reference("spec.zti", "file"),
                "implementations": [{"component": "c", "artifact": tree}],
                "target": {"prefix": tree}, "root": {"implementation": tree}, "loader": {"implementation": tree},
                "releaseInputs": [{"name": "r", "artifact": tree}],
            }

        honest = value()
        report = {"name": "sel4-x", "derivationId": closure_identity(honest), "dirty": False}
        yield "resolved-honest", not resolved_findings("sel4-x", honest, report, checkout)
        tampered = value()
        tampered["releaseInputs"][0]["artifact"] = {**tampered["releaseInputs"][0]["artifact"], "identity": "b" * 64}
        yield "resolved-artifact-digest", bool(resolved_findings("sel4-x", tampered, {**report, "derivationId": closure_identity(tampered)}, checkout))
        yield "resolved-derivation-id", bool(resolved_findings("sel4-x", honest, {**report, "derivationId": "c" * 64}, checkout))
        yield "resolved-other-name", bool(resolved_findings("sel4-x", honest, {**report, "name": "sel4-y"}, checkout))
        (checkout / "tree" / "b.txt").write_text("untracked\n")
        yield "resolved-untracked-file", bool(resolved_findings("sel4-x", honest, report, checkout))
        (checkout / "tree" / "b.txt").unlink()
        (checkout / ".gitignore").write_text("ignored.bin\n")
        (checkout / "tree" / "ignored.bin").write_text("ignored\n")
        yield "resolved-ignored-file-excluded", not resolved_findings("sel4-x", honest, report, checkout)


def controls(root: Path) -> list[tuple[str, bool]]:
    """(name, passed): the honest form accepted, or the corruption found. `root` supplies the schemas."""
    return [*_source_controls(), *_report_controls(root), *_resolved_controls()]
