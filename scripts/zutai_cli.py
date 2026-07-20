from __future__ import annotations

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ZUTAI_ROOT = ROOT / "deps" / "zutai"
ZUTAI_MANIFEST = ZUTAI_ROOT / "Cargo.toml"
STDLIB = ZUTAI_ROOT / "stdlib"
_BINARY = ZUTAI_ROOT / "target" / "release" / "zutai-cli"


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


def binary() -> Path:
    """Return the release `zutai-cli` binary, rebuilding only when the Zutai
    submodule's sources are newer than the last build."""
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
