"""Bounded immutable local diagnostic capture and operator notification."""

from __future__ import annotations

import hashlib
import io
import json
import os
import selectors
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import devloop_diagnostics_contract as contract
from harness import ROOT

RELAY_ENV = "SLIME_DEVLOOP_DIAGNOSTIC_RELAY"


def notify(message: str) -> None:
    sys.stderr.write(message + "\n")
    relay = os.environ.get(RELAY_ENV)
    if relay:
        try:
            with open(relay, "a", encoding="utf-8") as output:
                output.write(message + "\n")
        except OSError as error:
            raise ValueError(f"diagnostic relay unavailable: {error}") from error


class Capture(tuple):
    """Keep recipe's pair interface for existing gate controls."""

    def __new__(cls, exit_code: int, output: bytes, truncated: bool, transcript_digest: str):
        value = super().__new__(cls, (exit_code == 0, output.decode("utf-8", errors="replace")))
        value.exit_code = exit_code
        value.output = output
        value.truncated = truncated
        value.transcript_digest = transcript_digest
        return value


def capture(
    target: str, environment: dict[str, str] | None, *, complete_text: bool = False
) -> Capture:
    spools = {}
    try:
        for name in ("stdout", "stderr"):
            spools[name] = tempfile.TemporaryFile()
    except OSError:
        for spool in spools.values():
            spool.close()
        raise
    recipe_environment = dict(os.environ if environment is None else environment)
    recipe_environment.pop(RELAY_ENV, None)
    try:
        process = subprocess.Popen(
            ["just", target],
            cwd=ROOT,
            env=recipe_environment,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except OSError:
        for spool in spools.values():
            spool.close()
        raise
    chunks = {"stdout": bytearray(), "stderr": bytearray()}
    sizes = {"stdout": 0, "stderr": 0}
    # Retained logs are bounded. Temporary spools preserve the established
    # full stdout-then-stderr text digest without retaining that text in RAM.
    try:
        with selectors.DefaultSelector() as selector:
            for name in chunks:
                stream = getattr(process, name)
                os.set_blocking(stream.fileno(), False)
                selector.register(stream, selectors.EVENT_READ, name)
            while selector.get_map():
                for key, _ in selector.select():
                    data = os.read(key.fd, 8192)
                    if not data:
                        selector.unregister(key.fileobj)
                        key.fileobj.close()
                        continue
                    name = key.data
                    sizes[name] += len(data)
                    spools[name].write(data)
                    remaining = contract.MAX_OUTPUT_BYTES - len(chunks[name])
                    chunks[name].extend(data[:remaining])
        code = process.wait()
        digest = hashlib.sha256()
        complete = []
        for name in ("stdout", "stderr"):
            spools[name].seek(0)
            # Valid UTF-8 retains the former universal-newline digest. For
            # opaque invalid bytes, surrogateescape round-trips every byte.
            text = io.TextIOWrapper(
                spools[name], encoding="utf-8", errors="surrogateescape", newline=None
            )
            try:
                while part := text.read(8192):
                    digest.update(part.encode("utf-8", errors="surrogateescape"))
                    if complete_text:
                        complete.append(part)
            finally:
                text.detach()
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        for name in chunks:
            getattr(process, name).close()
            spools[name].close()
    output = bytes(chunks["stdout"] + chunks["stderr"])[: contract.MAX_OUTPUT_BYTES]
    result = Capture(
        code, output, sum(sizes.values()) > contract.MAX_OUTPUT_BYTES, digest.hexdigest()
    )
    result.complete_text = "".join(complete) if complete_text else None
    return result


def retain(key: str, request: dict, target: str, result: Capture) -> str:
    directory = ROOT / "build/devloop-diagnostics"
    directory.mkdir(parents=True, exist_ok=True)
    if any(path.is_symlink() for path in (directory, *directory.parents)):
        raise ValueError("symlink diagnostic directory")
    run = Path(tempfile.mkdtemp(prefix=key + "-", dir=directory))
    output = run / "output.log"
    receipt = run / "receipt.zti"
    relative_output = output.relative_to(ROOT).as_posix()
    relative_receipt = receipt.relative_to(ROOT).as_posix()
    if max(len(relative_output.encode()), len(relative_receipt.encode())) > contract.MAX_PATH_BYTES:
        raise ValueError("diagnostic path exceeds schema bound")
    record = {
        "formatVersion": contract.FORMAT_VERSION,
        "kind": contract.KIND,
        "runKey": key,
        "identity": request["execution"]["identity"],
        "justTarget": target,
        "recipeExitCode": result.exit_code,
        "outputPath": relative_output,
        "sha256": hashlib.sha256(result.output).hexdigest(),
        "capturedBytes": len(result.output),
        "limitBytes": contract.MAX_OUTPUT_BYTES,
        "truncated": result.truncated,
    }
    with output.open("xb") as handle:
        handle.write(result.output)
    with receipt.open("xb") as handle:
        handle.write(contract.encode(record))
    return relative_receipt


def announce(path: str, key: str, request: dict | None = None, target: str | None = None) -> None:
    relative = Path(path)
    if relative.is_absolute() or ".." in relative.parts:
        raise ValueError("unsafe cached receipt path")
    receipt = ROOT / relative
    if not receipt.is_file() or any(p.is_symlink() for p in (receipt, *receipt.parents)):
        raise ValueError("diagnostic receipt unavailable")
    if receipt.stat().st_size > contract.MAX_RECEIPT_BYTES:
        raise ValueError("diagnostic receipt exceeds schema bound")
    with tempfile.TemporaryDirectory(prefix="diagnostic-read-") as temporary:
        root = Path(temporary)
        shutil_source = ROOT / "contracts/devloop-diagnostics/v1/schema.zt"
        (root / "schema.zt").write_bytes(shutil_source.read_bytes())
        source = root / "decode.zt"
        source.write_text(
            's ::= import "schema.zt"; main :: Load -> Validation DecodeIssue s.DiagnosticReceipt ! { load.zti : Path -> Data; } = load => s.decodeReceipt (loadZti '
            + json.dumps(str(receipt))
            + "); main"
        )
        # The operator selected installed or submodule toolchain before the
        # adapter ran. Never build a second compiler here or override stdlib.
        environment = dict(os.environ)
        decoder = shutil.which("zutai-cli", path=environment.get("PATH"))
        if decoder is None or not environment.get("ZUTAI_STDLIB_ROOT"):
            raise ValueError("diagnostic receipt decoder environment unavailable")
        try:
            decoded = subprocess.run(
                [decoder, "json", str(source)],
                cwd=root,
                env=environment,
                capture_output=True,
                text=True,
                timeout=120,
            )
        except subprocess.SubprocessError as error:
            raise ValueError(f"diagnostic receipt decoder unavailable: {error}") from error
        if decoded.returncode:
            raise ValueError("diagnostic receipt cannot be decoded")
        value = json.loads(decoded.stdout)
        if value.get("tag") != "valid":
            raise ValueError("invalid diagnostic receipt")
        record = value["payload"]["value"]
    if (
        record["runKey"] != key
        or record["formatVersion"] != contract.FORMAT_VERSION
        or record["kind"] != contract.KIND
    ):
        raise ValueError("diagnostic receipt run/version mismatch")
    if request is not None and record["identity"] != request["execution"]["identity"]:
        raise ValueError("diagnostic receipt execution mismatch")
    if target is not None and record["justTarget"] != target:
        raise ValueError("diagnostic receipt recipe mismatch")
    output_relative = Path(record["outputPath"])
    if output_relative.is_absolute() or ".." in output_relative.parts:
        raise ValueError("unsafe diagnostic output path")
    output = ROOT / output_relative
    if not output.is_file() or any(p.is_symlink() for p in (output, *output.parents)):
        raise ValueError("diagnostic output unavailable")
    if (
        not 0 < record["limitBytes"] <= contract.MAX_OUTPUT_BYTES
        or not 0 <= output.stat().st_size <= record["limitBytes"]
    ):
        raise ValueError("diagnostic output exceeds bound")
    if max(len(path.encode()), len(record["outputPath"].encode())) > contract.MAX_PATH_BYTES:
        raise ValueError("diagnostic path exceeds bound")
    raw = output.read_bytes()
    if len(raw) != record["capturedBytes"] or hashlib.sha256(raw).hexdigest() != record["sha256"]:
        raise ValueError("diagnostic output length/digest mismatch")
    notify(f"SLIME_DEVLOOP_DIAGNOSTIC receipt={path} run={key}")
