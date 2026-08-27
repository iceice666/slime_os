"""Lifecycle-owned paths for in-tree component crates."""

from __future__ import annotations

import tomllib
from functools import lru_cache
from pathlib import Path

from harness import ROOT

COMPONENT_ROOT = ROOT / "components"
COMPONENT_CATEGORIES = ("system", "services", "applications", "testkit")
COMPONENT_CRATE_ROOTS = tuple(COMPONENT_ROOT / category for category in COMPONENT_CATEGORIES)


@lru_cache(maxsize=1)
def crate_paths() -> dict[str, Path]:
    """Map each component binary name to its leaf crate directory."""
    found: dict[str, Path] = {}
    for root in COMPONENT_CRATE_ROOTS:
        for manifest in sorted(root.rglob("Cargo.toml")):
            data = tomllib.loads(manifest.read_text(encoding="utf-8"))
            bins = data.get("bin", [])
            if len(bins) != 1 or not isinstance(bins[0].get("name"), str):
                raise SystemExit(
                    f"{manifest.relative_to(ROOT)}: component crate must declare exactly one [[bin]]"
                )
            name = bins[0]["name"]
            previous = found.get(name)
            if previous is not None:
                raise SystemExit(
                    f"component binary {name!r} is declared by both "
                    f"{previous.relative_to(ROOT)} and {manifest.parent.relative_to(ROOT)}"
                )
            found[name] = manifest.parent
    return found


def crate_path(name: str) -> Path:
    """Return one component crate directory, failing closed on an unknown name."""
    path = crate_paths().get(name)
    if path is None:
        raise SystemExit(f"no in-tree component crate declares binary {name!r}")
    return path


def source_path(name: str) -> Path:
    """Return one component's Rust entry point."""
    return crate_path(name) / "src" / "main.rs"
