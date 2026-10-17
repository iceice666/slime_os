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


def code_paths_data(text: str) -> tuple[str, ...]:
    policy = json.loads(text)
    if not isinstance(policy, dict) or "codePaths" not in policy:
        raise ValueError("policy must contain codePaths")
    entries = policy["codePaths"]
    if not isinstance(entries, list) or not all(
        isinstance(entry, str)
        and bool(entry)
        and not entry.startswith("/")
        and all(part not in ("", ".", "..") for part in entry.rstrip("/").split("/"))
        and "\x00" not in entry
        for entry in entries
    ):
        raise ValueError("codePaths must be a list of nonempty relative paths")
    return tuple(entries)


def code_paths(policy: Path = POLICY) -> tuple[str, ...]:
    return code_paths_data(policy.read_text())


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

    The parser emits source rows as well as rename/copy destination rows.
    Destinations introduce identities even when Git reports R or C rather than A.
    """
    verdict = Verdict()
    for status, path in changes:
        verdict.classes[classify(path, code, status)].append(path)
        if status.startswith(("A", "R", "C")) and NEW_ITEM.search(path):
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
    """Decode `git diff --name-status -z` without quoting or line splitting.

    Source rows never qualify for the new-schema or new-item exceptions.
    Copies preserve source protection even though their source remains present.
    """
    if not text:
        return []
    if not text.endswith("\x00"):
        raise ValueError("name-status stream must be NUL terminated")
    fields = text[:-1].split("\x00")
    rows = []
    cursor = 0
    while cursor < len(fields):
        status = fields[cursor]
        cursor += 1
        if not re.fullmatch(r"(?:[ADMT]|M\d{1,3}|[RC]\d{1,3})", status):
            raise ValueError(f"unsupported name-status code: {status!r}")
        count = 2 if status.startswith(("R", "C")) else 1
        paths = fields[cursor:cursor + count]
        if len(paths) != count or any(not path for path in paths):
            raise ValueError("incomplete name-status record")
        cursor += count
        if count == 2:
            rows.append(("D" if status.startswith("R") else "M", paths[0]))
        rows.append((status, paths[-1]))
    return rows
