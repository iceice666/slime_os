"""Parsed Just recipe metadata shared by repository checks."""

from __future__ import annotations

import json
import subprocess
from functools import lru_cache

from harness import ROOT


@lru_cache(maxsize=1)
def recipes() -> dict[str, dict]:
    """Return every recipe in the repository's fully imported Justfile."""
    process = subprocess.run(
        ["just", "--dump", "--dump-format", "json"],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    if process.returncode != 0:
        raise SystemExit(f"cannot parse Justfile metadata: {process.stderr.strip()}")
    try:
        metadata = json.loads(process.stdout)
    except json.JSONDecodeError as error:
        raise SystemExit(f"just emitted invalid JSON metadata: {error}") from error
    parsed = metadata.get("recipes")
    if not isinstance(parsed, dict):
        raise SystemExit("just metadata has no recipe table")
    return parsed


def targets() -> frozenset[str]:
    """Return every public and private top-level recipe name."""
    return frozenset(recipes())
