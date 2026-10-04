"""Classify a pull request's changed paths into the three scope classes.

The classes decide which review flow a pull request needs, so they are
derived from lists the repository already maintains rather than from a third
table that could drift:

* **grader** — the closure that decides acceptances: `scripts/`, `just/`,
  `Justfile` and `.devloop/`. `devloop-approval.py` pins these to
  `origin/main` at completion, so a change here is an exam change.
* **product** — every other path devloop's code identity walks
  (`.devloop/policy.json` `codePaths`): the operating system, its contracts,
  build inputs, pins and toolchain configuration.
* **interface** — a *new* `contracts/<name>/v<N>/schema.zt`, or a contract's
  `README.md`. A Zutai schema is the boundary a grader binds to, so a
  planning pull request may fix it before any implementation exists; the
  binding generator (`gen_rust.zt`), generated bindings, system specs and a
  change to an existing schema version stay `product`.
* **derived** — generated closure records whose identities follow whatever
  else changed (`generate-system-image-closures.py`,
  `generate-system-test-runs.py`). They mirror a change; they are not one.
* **other** — everything outside the code identity: CI orchestration,
  contributor and agent guides, documentation, the work-item store.

A pull request that adds a work item may not change product paths, and a
pull request confined to `derived` and `other` needs no landed work item.
"""

from __future__ import annotations

from dataclasses import dataclass, field
import json
from pathlib import Path
import re

from harness import ROOT

POLICY = ROOT / ".devloop" / "policy.json"

# The grader closure, as `scripts/check/devloop-approval.py` pins it.
GRADER_PREFIXES = ("scripts/", "just/", ".devloop/")
GRADER_FILES = ("Justfile",)

# Outputs regenerated from the tree: a planning PR that adds a checker
# changes every closure that records the scripts tree, and refusing that
# would refuse the exam landing with its item.
DERIVED_PREFIXES = (
    "contracts/system-image-closure/v2/closures/",
    "contracts/system-image-closure/v2/negative/",
    "contracts/system-test-run/v1/runs/",
)

NEW_ITEM = re.compile(r"\.tasks/items/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\.md\Z")

# The schema file of one contract version, including nested contracts such as
# `contracts/bootstate/trace/v1/schema.zt`. Only an *added* match is an
# interface: a modified one changes a format that already shipped.
NEW_SCHEMA = re.compile(r"\Acontracts/(?:[A-Za-z0-9_-]+/)+v[0-9]+/schema\.zt\Z")
CONTRACT_README = re.compile(r"\Acontracts/(?:[A-Za-z0-9_-]+/)+README\.md\Z")

PRODUCT = "product"
GRADER = "grader"
INTERFACE = "interface"
DERIVED = "derived"
OTHER = "other"
CLASSES = (PRODUCT, GRADER, INTERFACE, DERIVED, OTHER)


def code_paths(policy: Path = POLICY) -> tuple[str, ...]:
    entries = json.loads(policy.read_text())["codePaths"]
    if not isinstance(entries, list) or not all(isinstance(entry, str) for entry in entries):
        raise ValueError("codePaths must be a list of paths")
    return tuple(entries)


def _under(path: str, entry: str) -> bool:
    entry = entry.rstrip("/")
    return path == entry or path.startswith(entry + "/")


def classify(path: str, code: tuple[str, ...], status: str = "M") -> str:
    """Class of one `git diff --name-status` row; `status` is its status letter(s)."""
    if path in GRADER_FILES or any(path.startswith(prefix) for prefix in GRADER_PREFIXES):
        return GRADER
    if any(path.startswith(prefix) for prefix in DERIVED_PREFIXES):
        return DERIVED
    if CONTRACT_README.search(path) or (status.startswith("A") and NEW_SCHEMA.search(path)):
        return INTERFACE
    if any(_under(path, entry) for entry in code):
        return PRODUCT
    return OTHER


@dataclass
class Verdict:
    classes: dict[str, list[str]] = field(default_factory=lambda: {name: [] for name in CLASSES})
    added_items: list[str] = field(default_factory=list)
    findings: list[str] = field(default_factory=list)

    @property
    def needs_landed_item(self) -> bool:
        """Whether the documented flow requires a work item already on main."""
        return bool(self.classes[PRODUCT] or self.classes[GRADER] or self.classes[INTERFACE])


def judge(changes: list[tuple[str, str]], code: tuple[str, ...]) -> Verdict:
    """Apply the scope rules to `(status, path)` rows from `git diff --name-status`.

    Rename rows carry the destination path; a rename out of the product tree
    is still a product change, and a renamed or copied schema is not a new one.
    """
    verdict = Verdict()
    for status, path in changes:
        verdict.classes[classify(path, code, status)].append(path)
        if status.startswith("A") and NEW_ITEM.search(path):
            verdict.added_items.append(path)
    if verdict.added_items and verdict.classes[PRODUCT]:
        verdict.findings.append(
            "a pull request that adds a work item may not change the product tree; "
            "land the item first, then implement it in a separate pull request. "
            f"Added items: {', '.join(sorted(verdict.added_items))}. "
            f"Product paths: {', '.join(sorted(verdict.classes[PRODUCT]))}"
        )
    return verdict


def parse_name_status(text: str) -> list[tuple[str, str]]:
    """Rows of `git diff --name-status`; renames and copies yield their destination."""
    rows = []
    for line in text.splitlines():
        if not line.strip():
            continue
        parts = line.split("\t")
        status = parts[0]
        path = parts[-1]
        rows.append((status, path))
    return rows
