"""Locate, build, and run the `zutai-cli` binary the checkers and generators share.

Evaluations are memoized on disk under `build/zutai-cache/`, keyed by the content of
everything that determines the output: the binary, the stdlib, every `.zt` and package
manifest under `contracts/`, this module, the checker's path, the command, and the input
bytes. The input's path is never part of the key, so
a temporary file rewritten with different contents is a different entry. Only a zero exit
is stored — a decode that prints `#invalid` exits zero and is a result like any other — so
a malformed input, which exits non-zero, is re-run and refused every time.

`SLIME_ZUTAI_CACHE=0` bypasses the cache, `SLIME_ZUTAI_JOBS` bounds `prefetch`'s parallel
width, and `rm -rf build/zutai-cache` resets the store. A store that cannot be read or
written is skipped, never fatal: the evaluation still runs and its result is returned.
"""

from __future__ import annotations

import hashlib
import os
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from functools import lru_cache
from pathlib import Path
from typing import Iterable

from harness import ROOT

ZUTAI_ROOT = ROOT / "deps" / "zutai"
ZUTAI_MANIFEST = ZUTAI_ROOT / "Cargo.toml"
STDLIB = ZUTAI_ROOT / "stdlib"
_BINARY = ZUTAI_ROOT / "target" / "release" / "zutai-cli"
CACHE_ROOT = ROOT / "build" / "zutai-cache"
# Bump to orphan every stored entry when the key recipe changes.
_CACHE_FORMAT = b"slime-zutai-cache/v1"
# One system-spec evaluation peaks near 1.7 GB of RSS; each parallel worker is budgeted this.
_WORKER_MEMORY_BYTES = 2 << 30

# (command, target, input_path, env_var): run `zutai-cli <command> <target>` with `env_var`
# naming `input_path` in the child's environment.
Job = tuple[str, Path, Path, str]


class ZutaiError(Exception):
    """`zutai-cli` exited non-zero; the message is its stderr, or stdout when that is empty."""


def _newest_source_mtime() -> float:
    newest = 0.0
    candidates = [ZUTAI_MANIFEST]
    lockfile = ZUTAI_ROOT / "Cargo.lock"
    if lockfile.exists():
        candidates.append(lockfile)
    candidates.extend(ZUTAI_ROOT.glob("crates/**/*.rs"))
    candidates.extend(ZUTAI_ROOT.glob("crates/**/Cargo.toml"))
    for path in candidates:
        mtime = path.stat().st_mtime
        if mtime > newest:
            newest = mtime
    return newest


@lru_cache(maxsize=1)
def binary() -> Path:
    """Return the release `zutai-cli` binary, rebuilding only when the Zutai
    submodule's sources are newer than the last build; checked once per process."""
    if _BINARY.exists() and _BINARY.stat().st_mtime >= _newest_source_mtime():
        return _BINARY
    process = subprocess.run(
        [
            "cargo",
            "build",
            "--release",
            "--manifest-path",
            str(ZUTAI_MANIFEST),
            "-q",
            "-p",
            "zutai-cli",
        ],
        cwd=ROOT,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode != 0:
        sys.stderr.write(process.stdout)
        sys.stderr.write(process.stderr)
        raise SystemExit(process.returncode)
    if not _BINARY.exists():
        raise SystemExit(f"cargo build did not produce {_BINARY}")
    return _BINARY


def cache_enabled() -> bool:
    return os.environ.get("SLIME_ZUTAI_CACHE") != "0"


def _feed(digest: hashlib.blake2b, field: bytes) -> None:
    digest.update(len(field).to_bytes(8, "big"))
    digest.update(field)


@lru_cache(maxsize=None)
def _file_digest(path: Path, size: int, mtime_ns: int) -> bytes:
    with path.open("rb") as handle:
        return hashlib.file_digest(handle, "blake2b").digest()


def _content_digest(path: Path) -> bytes:
    """The file's content digest, memoized per process on its size and mtime so a
    rebuilt binary is noticed but an 11 MB binary is hashed once."""
    stat = path.stat()
    return _file_digest(path, stat.st_size, stat.st_mtime_ns)


def _tree_digest(files: Iterable[Path], root: Path) -> bytes:
    digest = hashlib.blake2b()
    for path in files:
        _feed(digest, path.relative_to(root).as_posix().encode("utf-8"))
        _feed(digest, _content_digest(path))
    return digest.digest()


@lru_cache(maxsize=1)
def _stdlib_digest() -> bytes:
    return _tree_digest(sorted(path for path in STDLIB.rglob("*") if path.is_file()), STDLIB)


@lru_cache(maxsize=1)
def _contracts_digest() -> bytes:
    # `zutai-cli` discovers the package graph from every `zutai.zti` under `contracts/` and
    # a checker may import any `.zt` those packages expose, so the whole set is the closure.
    root = ROOT / "contracts"
    files = [path for path in root.rglob("*.zt")] + [path for path in root.rglob("zutai.zti")]
    return _tree_digest(sorted(files), root)


def _key(command: str, target: Path, input_bytes: bytes, env_var: str) -> str:
    digest = hashlib.blake2b()
    fields = (
        _CACHE_FORMAT,
        _content_digest(Path(__file__)),
        _content_digest(binary()),
        _stdlib_digest(),
        _contracts_digest(),
        command.encode("utf-8"),
    )
    for field in fields:
        _feed(digest, field)
    if command == "run":
        checker = target.resolve()
        _feed(digest, checker.relative_to(ROOT.resolve()).as_posix().encode("utf-8"))
    _feed(digest, env_var.encode("utf-8"))
    _feed(digest, input_bytes)
    return digest.hexdigest()


def _run(command: str, target: Path, input_path: Path, env_var: str) -> str:
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    environment[env_var] = str(input_path)
    process = subprocess.run(
        [str(binary()), command, str(target)],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode != 0:
        raise ZutaiError((process.stderr or process.stdout).strip())
    return process.stdout


def _store(entry: Path, text: str) -> None:
    # Written beside the entry and renamed into place, so a concurrent reader sees a whole
    # file or none, and concurrent writers of one key race harmlessly on identical bytes.
    # A store that cannot be written is skipped: the result is already correct.
    try:
        CACHE_ROOT.mkdir(parents=True, exist_ok=True)
        handle = tempfile.NamedTemporaryFile(
            "wb", dir=CACHE_ROOT, prefix=f"{entry.name}.", suffix=".tmp", delete=False
        )
    except OSError:
        return
    temporary = Path(handle.name)
    try:
        with handle:
            handle.write(text.encode("utf-8"))
        temporary.replace(entry)
    except OSError:
        temporary.unlink(missing_ok=True)


def _lookup(entry: Path) -> str | None:
    try:
        return entry.read_bytes().decode("utf-8")
    except FileNotFoundError:
        return None
    except OSError:
        return None


def _fill(key: str, job: Job) -> str:
    command, target, input_path, env_var = job
    text = _run(command, target, input_path, env_var)
    _store(CACHE_ROOT / key, text)
    return text


def evaluate(command: str, target: Path, *, input_path: Path, env_var: str) -> str:
    """`zutai-cli <command> <target>` with `env_var` naming `input_path`, memoized by content.

    Raises `ZutaiError` when the binary exits non-zero; that result is never stored.
    """
    if not cache_enabled():
        return _run(command, target, input_path, env_var)
    key = _key(command, target, input_path.read_bytes(), env_var)
    cached = _lookup(CACHE_ROOT / key)
    if cached is not None:
        return cached
    return _fill(key, (command, target, input_path, env_var))


def workers() -> int:
    configured = os.environ.get("SLIME_ZUTAI_JOBS")
    if configured:
        if not configured.isdigit() or int(configured) < 1:
            raise SystemExit(f"SLIME_ZUTAI_JOBS must be a positive integer, not {configured!r}")
        return int(configured)
    cpus = os.cpu_count() or 1
    try:
        with open("/proc/meminfo", encoding="utf-8") as handle:
            for line in handle:
                if line.startswith("MemAvailable:"):
                    available = int(line.split()[1]) * 1024
                    return max(1, min(cpus, available // _WORKER_MEMORY_BYTES))
    except (OSError, ValueError):
        pass
    # Without a memory reading, stay small: every core times 1.7 GB is not a safe default.
    return min(cpus, 4)


def prefetch(jobs: Iterable[Job]) -> None:
    """Fill the cache for `jobs` in parallel so the caller's serial pass hits every entry.

    A job that fails is left for that serial pass to re-run and report, so refusals keep
    their order and text. No job's input may be written while this runs.
    """
    if not cache_enabled():
        return
    pending: dict[str, Job] = {}
    for job in jobs:
        command, target, input_path, env_var = job
        # Keys are computed here, on the calling thread, so the binary rebuild and every
        # memoized digest happen once before any worker starts.
        key = _key(command, target, input_path.read_bytes(), env_var)
        if key not in pending and _lookup(CACHE_ROOT / key) is None:
            pending[key] = job
    if not pending:
        return
    with ThreadPoolExecutor(max_workers=min(workers(), len(pending))) as pool:
        futures = [pool.submit(_fill, key, job) for key, job in pending.items()]
        for future in futures:
            try:
                future.result()
            except ZutaiError:
                pass
