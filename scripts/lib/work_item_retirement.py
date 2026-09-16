"""Repository retirement policy; MyQue alone owns canonical storage and codecs.

Callers supply an isolated, clean checkout and run the full repository checks
before and after apply. Verification reads Git objects as data, never checks out
or imports a pending branch, and requires the caller's freshly fetched base.
"""

from __future__ import annotations

from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import tomllib

import devloop


class RetirementError(Exception):
    """An unsafe candidate, failed tool operation, or unverifiable batch."""


@dataclass(frozen=True)
class Candidate:
    identity: str
    title: str
    evidence: str
    retained: dict[str, str]
    original: bytes
    revision: str


_UUID = r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"
_OID = re.compile(r"(?:[0-9a-f]{40}|[0-9a-f]{64})\Z")
_ALL = " or ".join(
    f"state = {state}" for state in ("open", "active", "blocked", "deferred", "done", "cancelled")
)


def _run(root: Path, program: str, *arguments: str, data: bytes | None = None) -> bytes:
    executable = shutil.which(program)
    if executable is None:
        raise RetirementError(f"{program} is not on PATH; enter the pinned tool environment")
    environment = os.environ.copy()
    environment["GIT_NO_REPLACE_OBJECTS"] = "1"
    command = [executable, *arguments]
    if program == "git":
        command[1:1] = ["-c", "core.hooksPath=/dev/null", "-c", "core.fsmonitor=false"]
    try:
        result = subprocess.run(command, cwd=root, input=data, capture_output=True, env=environment)
    except OSError as error:
        raise RetirementError(f"cannot run {program}: {error}") from error
    if result.returncode:
        detail = (result.stderr or result.stdout).decode("utf-8", errors="replace").strip()
        raise RetirementError(f"{program} {arguments[0]} failed: {detail}")
    return result.stdout


def _json(raw: bytes) -> dict:
    try:
        result = json.loads(raw)
    except (ValueError, UnicodeError) as error:
        raise RetirementError(f"invalid machine response: {error}") from error
    if not isinstance(result, dict):
        raise RetirementError("expected a machine object")
    return result


def _get(root: Path, identity: str) -> dict:
    if re.fullmatch(_UUID, identity) is None:
        raise RetirementError(f"not a canonical UUID: {identity!r}")
    return _json(_run(root, "myque", "api", "get", identity))


def _query(root: Path) -> dict[str, dict]:
    records = [
        _json(line) for line in _run(root, "myque", "query", _ALL, "--format", "json").splitlines()
    ]
    return {record["id"]: record for record in records}


def _read(path: Path) -> bytes:
    try:
        return path.read_bytes()
    except OSError as error:
        raise RetirementError(f"cannot read {path}: {error}") from error


def _safe_store(root: Path) -> None:
    """No path indirection or replayable writes from untrusted branch data."""
    tasks = root / ".tasks"
    if not tasks.is_dir() or tasks.is_symlink():
        raise RetirementError("a regular canonical .tasks store is required")
    for path in tasks.rglob("*"):
        if path.is_symlink() or (not path.is_dir() and not path.is_file()):
            raise RetirementError(f"unsafe store entry: {path}")
    if (tasks / "transaction.json").exists():
        raise RetirementError("pending MyQue transaction: recover in a trusted checkout first")
    config = tasks / "config.toml"
    if config.exists():
        try:
            settings = tomllib.loads(_read(config).decode("utf-8"))
        except (ValueError, UnicodeError) as error:
            raise RetirementError(f"invalid MyQue configuration: {error}") from error
        storage = settings.get("storage", {})
        if not isinstance(storage, dict) or storage.get("items", ".tasks/items") != ".tasks/items":
            raise RetirementError(
                "retirement requires the repository's canonical .tasks/items layout"
            )


def _check(root: Path) -> None:
    _safe_store(root)
    _run(root, "myque", "check")


def _closure(body: str) -> str:
    """Extract one exact ATX level-two section, preserving all content bytes.

    Fenced examples are opaque; indentation, block quotes, trailing ATX hashes
    and alternate heading spellings do not constitute the policy's heading.
    """
    lines = body.splitlines(keepends=True)
    fence: tuple[str, int] | None = None
    starts: list[int] = []
    end: int | None = None
    for index, line in enumerate(lines):
        text = line.rstrip("\r\n")
        if fence:
            if re.fullmatch(
                r" {0,3}" + re.escape(fence[0]) + "{" + str(fence[1]) + r",}[ \t]*", text
            ):
                fence = None
            continue
        opening = re.match(r" {0,3}(`{3,}|~{3,})(.*)\Z", text)
        if opening and not (opening[1][0] == "`" and "`" in opening[2]):
            fence = (opening[1][0], len(opening[1]))
            continue
        if re.match(r" {0,3}<(?:!--|\?|!|/?[A-Za-z])", text):
            raise RetirementError(
                "HTML blocks make Closure evidence boundaries ambiguous; use plain Markdown"
            )
        heading = re.match(r" {0,3}(#{1,6})(?:[ \t]+|$)", text)
        # Setext level-one/two headings also end a Markdown section.
        setext = (
            index > 0
            and bool(lines[index - 1].strip())
            and not re.match(r"(?: {4}|\t| {0,3}(?:[-+*>#]|[0-9]+[.)])[ \t])", lines[index - 1])
            and re.fullmatch(r" {0,3}(?:=+|-+)[ \t]*", text)
        )
        if starts and end is None and ((heading and len(heading[1]) <= 2) or setext):
            end = index - 1 if setext else index
        if text == "## Closure evidence":
            starts.append(index + 1)
    if len(starts) != 1:
        raise RetirementError("requires exactly one unfenced '## Closure evidence' section")
    evidence = "".join(lines[starts[0] : end])
    if not evidence.strip():
        raise RetirementError("Closure evidence section is empty")
    return evidence


def _policy(root: Path, item: dict) -> tuple[str, dict[str, str]]:
    consumers = set(item["consumers"])
    if not consumers:
        return _closure(item["body"]), {}
    if consumers != {"devloop"}:
        raise RetirementError("unsupported consumer namespaces: " + ", ".join(sorted(consumers)))
    try:
        return devloop.retention(item, cwd=root)
    except devloop.DevloopError as error:
        raise RetirementError(f"devloop retention unavailable: {error}") from error


def _candidate(root: Path, item: dict) -> Candidate:
    identity = item["id"]
    if item["state"] != "done" or item["retired"]:
        raise RetirementError(f"{identity}: not a live done item")
    original = _read(root / ".tasks" / "items" / f"{identity}.md")
    if hashlib.sha256(original).hexdigest() != item["revision"]:
        raise RetirementError(f"{identity}: source changed while reading")
    evidence, retained = _policy(root, item)
    return Candidate(identity, item["title"], evidence, retained, original, item["revision"])


def select_candidates(root: Path) -> tuple[list[Candidate], list[str]]:
    """Validate the store and preview live done items, reporting policy skips."""
    _check(root)
    candidates: list[Candidate] = []
    skipped: list[str] = []
    for identity, summary in sorted(_query(root).items()):
        if summary["state"] != "done":
            continue
        item = _get(root, identity)
        if item["retired"]:
            continue
        try:
            candidates.append(_candidate(root, item))
        except RetirementError as error:
            skipped.append(f"{identity} ({item['title']}): {error}")
    return candidates, skipped


def _commit(root: Path, revision: str) -> str:
    if not isinstance(revision, str) or _OID.fullmatch(revision) is None:
        raise RetirementError("verification requires full lowercase commit IDs")
    resolved = _run(root, "git", "rev-parse", "--verify", revision + "^{commit}").decode().strip()
    if resolved != revision:
        raise RetirementError(f"not an exact commit: {revision}")
    return resolved


def _parents(root: Path, revision: str) -> list[str]:
    return _run(root, "git", "rev-list", "--parents", "-n", "1", revision).decode().split()[1:]


def _repository(root: Path) -> str:
    return " ".join(
        sorted(set(_run(root, "git", "rev-list", "--max-parents=0", "--all").decode().split()))
    )


def _materialize(root: Path, revision: str, destination: Path) -> None:
    """Extract data blobs, not a checkout: no hooks, filters, or branch code."""
    destination.mkdir()
    entries = _run(root, "git", "ls-tree", "-r", "-z", revision, "--", ".tasks").split(b"\0")
    for entry in filter(None, entries):
        header, encoded_path = entry.split(b"\t", 1)
        mode, kind, oid = header.split()
        relative = Path(os.fsdecode(encoded_path))
        if mode != b"100644" or kind != b"blob" or ".." in relative.parts or relative.is_absolute():
            raise RetirementError(f"non-regular store blob at {relative}")
        path = destination / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(_run(root, "git", "cat-file", "blob", oid.decode()))
    _check(destination)


def _expected_metadata(store: Path, candidates: list[Candidate]) -> dict[str, bytes]:
    """Ask MyQue's codec, rather than recreating its envelope serializer."""
    with tempfile.TemporaryDirectory(prefix="myque-retirement-metadata-") as temporary:
        scratch = Path(temporary)
        shutil.copytree(store / ".tasks", scratch / ".tasks")
        result = {}
        for candidate in candidates:
            _run(
                scratch,
                "myque",
                "api",
                "put",
                candidate.identity,
                "--expected",
                candidate.revision,
                data=json.dumps({"body": "", "consumers": candidate.retained}).encode(),
            )
            result[candidate.identity] = _read(
                scratch / ".tasks" / "items" / f"{candidate.identity}.md"
            )
        return result


def _verify_records(
    root: Path, store: Path, candidates: list[Candidate], base: str, metadata: dict[str, bytes]
) -> list[str]:
    repository = _repository(root)
    retained: list[str] = []
    for candidate in candidates:
        identity = candidate.identity
        original_path = f".tasks/items/{identity}.md"
        record = _json(_read(store / ".tasks" / "terminal" / f"{identity}.json"))
        history = record["history"]
        if record["originalPath"] != original_path or history["path"] != original_path:
            raise RetirementError(f"{identity}: original path changed")
        if record["evidence"] != candidate.evidence:
            raise RetirementError(f"{identity}: retirement evidence differs from source policy")
        if record["metadata"].encode() != metadata[identity]:
            raise RetirementError(f"{identity}: terminal envelope or retained consumers changed")
        if (
            history["repository"] != repository
            or history["digest"] != hashlib.sha256(candidate.original).hexdigest()
        ):
            raise RetirementError(f"{identity}: history repository or digest mismatch")
        commit = _commit(root, history["commit"])
        if _parents(root, commit) != [base]:
            raise RetirementError(
                f"{identity}: history snapshot does not have the fresh base as sole parent"
            )
        reference = (
            _run(root, "git", "rev-parse", "--verify", f"refs/myque/retained/{commit}")
            .decode()
            .strip()
        )
        if reference != commit:
            raise RetirementError(f"{identity}: retained ref does not identify its snapshot")
        snapshot = _run(root, "git", "ls-tree", "-r", "-z", commit).split(b"\0")
        entries = [entry.split(b"\t", 1) for entry in snapshot if entry]
        if (
            len(entries) != 1
            or entries[0][0].split()[:2] != [b"100644", b"blob"]
            or entries[0][1] != original_path.encode()
        ):
            raise RetirementError(f"{identity}: history is not an exact single-item snapshot")
        if _run(root, "git", "show", f"{commit}:{original_path}") != candidate.original:
            raise RetirementError(f"{identity}: retained snapshot bytes differ from source")
        observed = _get(store, identity)
        if (
            not observed["retired"]
            or observed["bodyAvailable"]
            or observed["body"] is not None
            or observed["consumers"] != candidate.retained
        ):
            raise RetirementError(f"{identity}: terminal machine view violates retention policy")
        retained.append(commit)
    return sorted(set(retained))


def _changes(root: Path, base: str, head: str | None = None) -> dict[str, str]:
    arguments = [
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
        "--name-status",
        "-z",
        base,
    ]
    if head is not None:
        arguments.append(head)
    fields = _run(root, "git", *arguments, "--").split(b"\0")
    if fields[-1] == b"":
        fields.pop()
    return {
        os.fsdecode(fields[index + 1]): fields[index].decode() for index in range(0, len(fields), 2)
    }


def _expected_changes(candidates: list[Candidate]) -> dict[str, str]:
    return {
        path: state
        for candidate in candidates
        for path, state in (
            (f".tasks/items/{candidate.identity}.md", "D"),
            (f".tasks/terminal/{candidate.identity}.json", "A"),
        )
    }


def apply_candidates(root: Path, candidates: list[Candidate]) -> None:
    """Retire a preselected batch only in a clean, exclusively owned checkout."""
    _check(root)
    if not candidates:
        return
    if len({candidate.identity for candidate in candidates}) != len(candidates):
        raise RetirementError("duplicate candidates")
    if _run(root, "git", "status", "--porcelain=v1", "--untracked-files=all"):
        raise RetirementError("apply requires a clean isolated checkout")
    base = _run(root, "git", "rev-parse", "HEAD").decode().strip()
    before = _query(root)
    for candidate in candidates:
        if _candidate(root, _get(root, candidate.identity)) != candidate:
            raise RetirementError(f"{candidate.identity}: source or policy changed since preview")
        if (
            _run(root, "git", "show", f"{base}:.tasks/items/{candidate.identity}.md")
            != candidate.original
        ):
            raise RetirementError(f"{candidate.identity}: source is not the committed base")
    metadata = _expected_metadata(root, candidates)
    for candidate in candidates:
        # MyQue has no guarded retire CLI. Exclusive ownership plus this immediate
        # byte/revision check bounds the call; MyQue owns its lock and transaction.
        if _candidate(root, _get(root, candidate.identity)) != candidate:
            raise RetirementError(
                f"{candidate.identity}: source changed immediately before retirement"
            )
        _run(
            root,
            "myque",
            "retire",
            candidate.identity,
            "--evidence",
            candidate.evidence,
            data=json.dumps(candidate.retained).encode(),
        )
    _check(root)
    if _query(root) != before:
        raise RetirementError("retirement changed the semantic envelope or dependency graph")
    _verify_records(root, root, candidates, base, metadata)
    # New terminal files are deliberately not staged by this API.
    changes = _changes(root, base)
    for path in _run(root, "git", "ls-files", "--others", "--exclude-standard", "-z").split(b"\0"):
        if path:
            changes[os.fsdecode(path)] = "A"
    if changes != _expected_changes(candidates):
        raise RetirementError("retirement changed paths outside the selected storage transitions")


def verify_batch(root: Path, base: str, head: str) -> list[str]:
    """Verify an untrusted single-commit branch against freshly fetched main.

    Return only full, verified retained commit IDs. Advancing main invalidates
    the entire old batch; the publisher must obtain a new base, never rebase it.
    """
    base, head = _commit(root, base), _commit(root, head)
    if _parents(root, head) != [base]:
        raise RetirementError("retirement branch is not one commit on the current base")
    changes = _changes(root, base, head)
    identities = sorted(
        path.removeprefix(".tasks/items/").removesuffix(".md")
        for path, status in changes.items()
        if status == "D" and re.fullmatch(r"\.tasks/items/" + _UUID + r"\.md", path)
    )
    if not identities:
        raise RetirementError("retirement batch contains no item transitions")
    with tempfile.TemporaryDirectory(prefix="myque-retirement-verify-") as temporary:
        source, pending = Path(temporary) / "base", Path(temporary) / "head"
        _materialize(root, base, source)
        candidates = [_candidate(source, _get(source, identity)) for identity in identities]
        if changes != _expected_changes(candidates):
            raise RetirementError(
                "batch diff is not exclusively same-UUID Markdown deletions and terminal additions"
            )
        metadata = _expected_metadata(source, candidates)
        _materialize(root, head, pending)
        if _query(source) != _query(pending):
            raise RetirementError("batch changes the semantic envelope or dependency graph")
        return _verify_records(root, pending, candidates, base, metadata)
