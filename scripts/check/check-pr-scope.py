#!/usr/bin/env python3

"""Refuse a pull request whose diff mixes a new work item with product changes.

The input is a Git revision range, which is why this is its own checker
rather than a case in `check-work-items.py`: that gate validates the store
offline and must not depend on what a branch is being compared against.
The classes themselves live in `scripts/lib/pr_scope.py`; this file only
reads the diff, reports the classification, and runs the negative controls
that prove the rule can refuse.
"""

from __future__ import annotations

import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
import subprocess

from harness import ROOT
import pr_scope


def diff(base: str, head: str) -> list[tuple[str, str]]:
    finished = subprocess.run(
        ["git", "diff", "--name-status", base, head],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if finished.returncode:
        raise SystemExit(f"git diff failed: {finished.stderr.strip()}")
    return pr_scope.parse_name_status(finished.stdout)


def report(verdict: pr_scope.Verdict) -> None:
    for name in pr_scope.CLASSES:
        paths = verdict.classes[name]
        print(f"{name}: {len(paths)} path(s)")
        for path in sorted(paths):
            print(f"  {path}")
    if verdict.added_items:
        print("planning: lands work item(s) " + ", ".join(sorted(verdict.added_items)))
    elif verdict.needs_landed_item:
        print("implementation: a work item already on main must cover this change")
    else:
        print("process: no landed work item required")


def controls() -> None:
    """Synthetic diffs the rule must refuse or accept, without Git."""
    code = ("slime-root", "components", "contracts", "scripts/check/check-docs.py", "Justfile")
    item = ".tasks/items/01a0ec61-d599-73ec-9e42-1ee6966420fd.md"
    cases = [
        ([("A", item), ("M", "slime-root/src/main.rs")], True, "item plus product"),
        ([("A", item), ("R100", "components/a.rs")], True, "item plus renamed product"),
        ([("A", item), ("M", "scripts/check/check-docs.py")], False, "item plus grader"),
        ([("A", item), ("A", ".devloop/inputs/x.json"), ("M", "Justfile")], False, "item plus exam"),
        ([("A", item), ("M", "docs/plans/x.md")], False, "item plus docs"),
        ([("M", item), ("M", "slime-root/src/main.rs")], False, "modified item plus product"),
        ([("A", ".tasks/terminal/01a0ec61-d599-73ec-9e42-1ee6966420fd.json")], False, "retirement"),
        ([("M", ".github/workflows/ci.yml"), ("M", "CONTRIBUTING.md")], False, "process only"),
        (
            [("A", item), ("M", "scripts/check/x.py"), ("M", "contracts/system-image-closure/v2/closures/sel4.zti")],
            False,
            "item plus grader plus regenerated closures",
        ),
        ([("A", item), ("M", "contracts/system-image-closure/v2/check.zt")], True, "item plus closure contract source"),
    ]
    for changes, refused, description in cases:
        verdict = pr_scope.judge(changes, code)
        if bool(verdict.findings) != refused:
            raise SystemExit(f"pr-scope control failed: {description}")
    hatch = pr_scope.judge([("M", ".github/workflows/ci.yml"), ("M", "docs/x.md")], code)
    if hatch.needs_landed_item:
        raise SystemExit("pr-scope control failed: process-only diff demanded an item")
    for path in ("slime-root/src/main.rs", "Justfile", "scripts/lib/pr_scope.py"):
        if not pr_scope.judge([("M", path)], code).needs_landed_item:
            raise SystemExit(f"pr-scope control failed: {path} escaped the landed-item rule")
    if pr_scope.classify("Justfile", code) != pr_scope.GRADER:
        raise SystemExit("pr-scope control failed: Justfile is grader, not product")
    if pr_scope.classify("scripts/x", ()) != pr_scope.GRADER:
        raise SystemExit("pr-scope control failed: scripts/ is grader regardless of codePaths")
    if pr_scope.judge([("M", "contracts/system-image-closure/v2/closures/sel4.zti")], code).needs_landed_item:
        raise SystemExit("pr-scope control failed: regenerated closures alone demanded an item")
    print("pr-scope controls passed: 10 diffs judged, 4 refused")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", help="base revision of the pull request")
    parser.add_argument("--head", help="head revision of the pull request")
    parser.add_argument("--controls", action="store_true", help="run the negative controls only")
    args = parser.parse_args()
    controls()
    if args.controls:
        return 0
    if not args.base or not args.head:
        parser.error("--base and --head are required unless --controls")
    verdict = pr_scope.judge(diff(args.base, args.head), pr_scope.code_paths())
    report(verdict)
    for finding in verdict.findings:
        print(f"pr-scope: {finding}")
    return 1 if verdict.findings else 0


if __name__ == "__main__":
    raise SystemExit(main())
