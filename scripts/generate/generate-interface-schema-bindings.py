#!/usr/bin/env python3

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
import fcntl
import hashlib
import importlib.util
from contextlib import contextmanager
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from types import ModuleType

from harness import ROOT
from interface_schema import admit_interfaces, load_manifest_interface_paths, render_rust
from zutai_cli import STDLIB, binary

CONTRACT_GENERATOR = ROOT / "contracts" / "interface-schema" / "v1" / "gen_python.zt"
PYTHON_OUTPUT = ROOT / "scripts" / "lib" / "interface_schema_contract.py"
RUST_OUTPUT = ROOT / "components" / "proto" / "src" / "interface_schema.rs"
LOCK_PATH = Path(tempfile.gettempdir()) / (
    "slime-interface-schema-bindings-"
    + hashlib.sha256(str(ROOT).encode("utf-8")).hexdigest()[:16]
    + ".lock"
)


def _run_contract_generator(staging: Path) -> Path:
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    environment["SLIME_INTERFACE_BINDINGS_ROOT"] = str(staging)
    process = subprocess.run(
        [str(binary()), "run", str(CONTRACT_GENERATOR)],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode != 0:
        sys.stderr.write(process.stdout)
        sys.stderr.write(process.stderr)
        raise SystemExit(process.returncode)
    output = staging / "interface_schema_contract.py"
    if not output.exists():
        raise SystemExit("interface-schema contract generator did not write Python bindings")
    return output


def _load_contract(path: Path) -> ModuleType:
    specification = importlib.util.spec_from_file_location("staged_interface_schema_contract", path)
    if specification is None or specification.loader is None:
        raise SystemExit("cannot load generated interface-schema constants")
    module = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(module)
    return module


def _format_rust(source: str) -> str:
    process = subprocess.run(
        ["rustfmt", "--edition", "2024", "--emit", "stdout"],
        cwd=ROOT,
        input=source,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode != 0:
        sys.stderr.write(process.stderr)
        raise SystemExit(process.returncode)
    return process.stdout


def render(paths: tuple[Path, ...] | None = None) -> tuple[str, str]:
    with tempfile.TemporaryDirectory(prefix="slime-interface-schema-bindings-") as temporary:
        staged_contract = _run_contract_generator(Path(temporary))
        contract_source = staged_contract.read_text(encoding="utf-8")
        contract = _load_contract(staged_contract)
        catalog = list(paths) if paths is not None else load_manifest_interface_paths(contract=contract)
        interfaces = admit_interfaces(catalog, contract=contract)
        rust_source = _format_rust(render_rust(interfaces, contract=contract))
        if len(rust_source.encode("utf-8")) > contract.MAX_GENERATED_BYTES:
            raise SystemExit("formatted interface-schema bindings exceed generated-output bound")
        return contract_source, rust_source


@contextmanager
def _output_lock():
    with LOCK_PATH.open("w", encoding="utf-8") as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        yield


def _write_atomic(path: Path, contents: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(
            "w",
            encoding="utf-8",
            dir=path.parent,
            prefix=f".{path.name}.",
            delete=False,
        ) as handle:
            temporary = Path(handle.name)
            handle.write(contents)
            handle.flush()
            os.fsync(handle.fileno())
        temporary.replace(path)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    arguments = parser.parse_args()
    python_source, rust_source = render()
    outputs = ((PYTHON_OUTPUT, python_source), (RUST_OUTPUT, rust_source))
    with _output_lock():
        if arguments.check:
            for path, contents in outputs:
                if not path.exists() or path.read_text(encoding="utf-8") != contents:
                    raise SystemExit(
                        f"generated {path.relative_to(ROOT)} is stale; run `just interface_schema_gen`"
                    )
            print("Interface-schema bindings are current")
            return
        for path, contents in outputs:
            _write_atomic(path, contents)
            print(f"Generated {path.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
