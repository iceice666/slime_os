"""Typed replay of bounded post-execution observation capture refusals."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import time

import devloop_diagnostics
from harness import ROOT
import observation_capacity_contract as contract


class Refused(ValueError):
    """A capacity refusal with an already-executed recipe, retained for replay."""


def directory() -> Path:
    path = ROOT / "build/devloop-observation-capacity"
    if any(p.is_symlink() for p in (path, *path.parents)):
        raise ValueError("symlink capacity replay directory")
    for p in (ROOT / "build", path):
        p.mkdir(exist_ok=True)
        if p.is_symlink() or not p.is_dir():
            raise ValueError("unsafe capacity replay directory")
    return path


def decoded(path: Path) -> dict:
    if (
        path.is_symlink()
        or any(p.is_symlink() for p in path.parents)
        or path.stat().st_size > contract.MAX_RECEIPT_BYTES
    ):
        raise ValueError("unsafe capacity replay receipt")
    environment = dict(os.environ, SLIME_CAPACITY_RECEIPT=str(path))
    decoder = shutil.which("zutai-cli", path=environment.get("PATH"))
    if decoder is None or not environment.get("ZUTAI_STDLIB_ROOT"):
        raise ValueError("capacity decoder environment unavailable")
    try:
        result = subprocess.run(
            [decoder, "json", str(ROOT / "contracts/observation-capacity-refusal/v1/check.zt")],
            cwd=ROOT,
            env=environment,
            capture_output=True,
            text=True,
            timeout=120,
        )
    except subprocess.SubprocessError as error:
        raise ValueError("capacity decoder unavailable") from error
    if result.returncode:
        raise ValueError("capacity receipt decode failed")
    value = json.loads(result.stdout)
    if value.get("tag") != "valid":
        raise ValueError("invalid capacity receipt")
    return value["payload"]["value"]


def validate(record: dict, key: str, request: dict, target: str) -> None:
    if (
        record["formatVersion"] != contract.FORMAT_VERSION
        or record["kind"] != contract.KIND
        or record["gate"] != contract.GATE
        or record["outcome"] != contract.OUTCOME
    ):
        raise ValueError("capacity receipt version/classification mismatch")
    if (
        record["identity"] != request["execution"]["identity"]
        or record["runKey"] != key
        or record["justTarget"] != target
    ):
        raise ValueError("capacity receipt execution mismatch")
    if (
        record["stderrLimitBytes"] != contract.STDERR_LIMIT_BYTES
        or record["stderrObservedBytes"] <= contract.STDERR_LIMIT_BYTES
    ):
        raise ValueError("capacity receipt byte-count mismatch")
    if record["reuseUntil"] != record["finishedAt"] + contract.REUSE_SECONDS:
        raise ValueError("capacity receipt lifetime mismatch")
    diagnostic = ROOT / record["diagnosticReceiptPath"]
    relative = Path(record["diagnosticReceiptPath"])
    if (
        relative.is_absolute()
        or ".." in relative.parts
        or len(record["diagnosticReceiptPath"].encode()) > contract.MAX_PATH_BYTES
        or not diagnostic.is_file()
        or any(p.is_symlink() for p in (diagnostic, *diagnostic.parents))
        or diagnostic.stat().st_size > devloop_diagnostics.contract.MAX_RECEIPT_BYTES
    ):
        raise ValueError("unsafe capacity diagnostic binding")
    if hashlib.sha256(diagnostic.read_bytes()).hexdigest() != record["diagnosticReceiptSha256"]:
        raise ValueError("capacity diagnostic receipt changed")


def refuse(record: dict, path: Path, key: str, request: dict, target: str) -> None:
    validate(record, key, request, target)
    devloop_diagnostics.announce(record["diagnosticReceiptPath"], key, request, target)
    devloop_diagnostics.notify(
        f"SLIME_DEVLOOP_CAPACITY receipt={path.relative_to(ROOT).as_posix()} run={key}"
    )
    message = f"diagnostic capacity refusal for run {key}: raw stderr {record['stderrObservedBytes']} exceeds {contract.STDERR_LIMIT_BYTES}; recipe already executed"
    devloop_diagnostics.notify(message)
    raise Refused(message)


def replay(key: str, request: dict, target: str) -> bool:
    base = ROOT / "build/devloop-observation-capacity"
    if base.is_symlink() or any(p.is_symlink() for p in base.parents):
        raise ValueError("symlink capacity replay directory")
    if not base.exists():
        return False
    selected = []
    for path in sorted(base.glob(key[:32] + "*.zti")):
        record = decoded(path)
        expected_names = {
            record["runKey"] + ".zti",
            record["runKey"][:32]
            + hashlib.sha256(record["diagnosticReceiptPath"].encode()).hexdigest()[:32]
            + ".zti",
        }
        if path.name not in expected_names:
            raise ValueError("capacity generation filename mismatch")
        if record["runKey"] != key:
            if path.name == key + ".zti":
                raise ValueError("capacity receipt key changed")
            continue
        validate(record, key, request, target)
        now = int(time.time())
        if record["finishedAt"] > now:
            raise ValueError("capacity receipt is future dated")
        selected.append((record, path))
    if not selected:
        return False
    record, path = max(selected, key=lambda entry: (entry[0]["finishedAt"], entry[1].name))
    if int(time.time()) > record["reuseUntil"]:
        return False
    refuse(record, path, key, request, target)
    return True


def retain(key: str, request: dict, target: str, result, diagnostic_path: str) -> None:
    now = int(time.time())
    diagnostic = ROOT / diagnostic_path
    record = {
        "formatVersion": contract.FORMAT_VERSION,
        "kind": contract.KIND,
        "runKey": key,
        "gate": contract.GATE,
        "justTarget": target,
        "identity": request["execution"]["identity"],
        "outcome": contract.OUTCOME,
        "stderrLimitBytes": contract.STDERR_LIMIT_BYTES,
        "stderrObservedBytes": result.stderr_bytes,
        "recipeExitCode": result.exit_code,
        "diagnosticReceiptPath": diagnostic_path,
        "diagnosticReceiptSha256": hashlib.sha256(diagnostic.read_bytes()).hexdigest(),
        "finishedAt": now,
        "reuseUntil": now + contract.REUSE_SECONDS,
    }
    replay(key, request, target)
    base = directory()
    path = base / (key + ".zti")
    if path.exists():
        generation = key[:32] + hashlib.sha256(diagnostic_path.encode()).hexdigest()[:32]
        path = base / (generation + ".zti")
    with path.open("xb") as handle:
        handle.write(contract.encode(record))
    refuse(record, path, key, request, target)
