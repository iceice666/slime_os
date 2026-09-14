#!/usr/bin/env python3

"""Validate the canonical work-item store and the repository's policy over it.

Two responsibilities, deliberately in one checker rather than two:

* **Store validity** is MyQue's own. This runs ``myque check``, which owns
  identity (UUID form, UUIDv7, duplicates, filename/id agreement), alias
  uniqueness, dangling references, parent and dependency cycles,
  state/``closed`` consistency, and schema conformance. Reimplementing any of
  that here would fork the schema, which the migration explicitly does not do.

* **Repository policy** is Slime OS's own and MyQue is generic, so it cannot
  live upstream: backlog-first ordering. This consumes the store rather than
  extending ``work-item/v1``.

Adding this checker rather than extending an existing one is deliberate: the
work-item store is a new mechanism with its own execution boundary (an external
binary) and its own inputs, which is the case ``AGENTS.md`` reserves a
top-level checker for.
"""

from __future__ import annotations

import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import shutil
import subprocess
import tempfile
from unittest.mock import patch

from harness import ROOT
import work_items
from work_items import ITEMS, identities, items, open_backlog


failures: list[str] = []


def fail(message: str) -> None:
    failures.append(message)


def run_myque_check(root: _Path = ROOT) -> list[str]:
    """Delegate store validation to MyQue, which owns the schema."""
    binary = shutil.which("myque")
    if binary is None:
        raise SystemExit(
            "myque is not on PATH: enter the repository dev shell (`nix develop`), "
            "which provides it, or run `nix run github:mozufu/myque -- check`"
        )
    finished = subprocess.run([binary, "check"], cwd=root, capture_output=True, text=True)
    if finished.returncode == 0:
        return []
    # `myque check` distinguishes its failures by status: 1 is findings, 2 is a
    # missing tracker or a usage error. Reporting them identically would send a
    # reader looking for a broken item when the store is simply absent.
    label = "reported findings" if finished.returncode == 1 else "could not read the store"
    details = [
        line.strip() for line in (finished.stdout + finished.stderr).splitlines() if line.strip()
    ]
    return [f"myque check {label}: {line}" for line in details or [f"exit {finished.returncode}"]]


def check_backlog_first() -> list[str]:
    """Backlog defects are cleared or explicitly deferred before track work.

    ``AGENTS.md``: "Resolve, defer, or block every open one before starting a
    new track milestone; a green verification suite is a precondition for
    milestone work, not a milestone itself." ``deferred`` and ``blocked`` are
    the explicit escapes the rule allows, so they satisfy it rather than
    breaking it.
    """
    blocking = open_backlog()
    active_milestones = [
        item for item in items() if item["kind"] == "milestone" and item["state"] == "active"
    ]
    if blocking and active_milestones:
        names = ", ".join(str(item["key"] or item["id"]) for item in blocking)
        opened = ", ".join(str(item["key"] or item["id"]) for item in active_milestones)
        return [
            f"backlog-first: {opened} is active while backlog items are neither "
            f"resolved nor explicitly deferred: {names}"
        ]
    return []


def check_controls() -> None:
    """Exercise the real store and shared policy reader without historical files."""
    binary = shutil.which("myque")
    if binary is None:
        run_myque_check()  # Report the same prerequisite as the real-store path.
        return

    with tempfile.TemporaryDirectory(prefix="work-item-controls-") as temporary:
        root = _Path(temporary)

        def command(*arguments: str) -> None:
            result = subprocess.run([binary, *arguments], cwd=root, capture_output=True, text=True)
            if result.returncode:
                raise SystemExit(
                    f"work-item control setup failed ({' '.join(arguments)}): "
                    f"{result.stdout}{result.stderr}"
                )

        with patch.object(shutil, "which", return_value=None):
            try:
                run_myque_check(root)
            except SystemExit:
                pass
            else:
                fail("control: missing MyQue executable was accepted")

        if not any("no tracker found" in finding for finding in run_myque_check(root)):
            fail("control: missing store did not report the tracker prerequisite")
        command("init")
        command("new", "Backlog control", "--kind", "bug", "--key", "BACKLOG", "--tag", "backlog")
        command("new", "Milestone control", "--kind", "milestone", "--key", "MILESTONE")
        command("start", "MILESTONE")

        with patch.object(work_items, "ITEMS", root / ".tasks" / "items"):
            try:
                for transition, rejected in (
                    (None, True),
                    ("start", True),
                    ("defer", False),
                    ("block", False),
                    ("close", False),
                ):
                    if transition is not None:
                        command(transition, "BACKLOG")
                    findings = run_myque_check(root)
                    if findings:
                        fail(f"control: valid {transition} store rejected: {findings}")
                    items.cache_clear()
                    if bool(check_backlog_first()) != rejected:
                        fail(
                            f"control: backlog-first misclassified {transition} backlog with active milestone"
                        )
                command("reopen", "BACKLOG")
                command("reopen", "MILESTONE")
                items.cache_clear()
                if check_backlog_first():
                    fail("control: open backlog without active milestone was rejected")
            finally:
                items.cache_clear()

        command("new", "Dependency control", "--key", "DEPENDENCY")
        command("depend", "BACKLOG", "DEPENDENCY")
        if run_myque_check(root):
            fail("control: valid dependency graph was rejected")
        fixture_items = root / ".tasks" / "items"
        dependency = next(
            path for path in fixture_items.glob("*.md") if "key: DEPENDENCY\n" in path.read_text()
        )
        original = dependency.read_bytes()
        dependency.unlink()
        if not any(
            f"dangling depends reference to {dependency.stem}" in finding
            for finding in run_myque_check(root)
        ):
            fail("control: dangling dependency did not identify the missing target")
        dependency.write_bytes(original.replace(b"schema: work-item/v1", b"schema: invalid/v1"))
        if not any(
            "unknown schema version: invalid/v1" in finding for finding in run_myque_check(root)
        ):
            fail("control: invalid store schema did not report the schema refusal")


def main() -> int:
    if not ITEMS.is_dir():
        raise SystemExit(
            f"{ITEMS.relative_to(ROOT)} does not exist: it is the repository's only "
            "work-item authority and nothing reconstructs it"
        )
    check_controls()
    failures.extend(run_myque_check())
    failures.extend(check_backlog_first())

    if failures:
        for failure in failures:
            print(f"work items: {failure}")
        raise SystemExit(f"work-item check failed with {len(failures)} problem(s)")

    total = len(identities())
    print(f"work-item check passed: {total} items")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
