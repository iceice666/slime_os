#!/usr/bin/env python3

"""Validate the canonical work-item store and the repository's policy over it.

Two responsibilities, deliberately in one checker rather than two:

* **Store validity** is MyQue's own. This runs ``myque check``, which owns
  identity (UUID form, UUIDv7, duplicates, filename/id agreement), alias
  uniqueness, dangling references, parent and dependency cycles,
  state/``closed`` consistency, and schema conformance. Reimplementing any of
  that here would fork the schema, which the migration explicitly does not do.

* **Repository policy** is Slime OS's own and MyQue is generic, so it cannot
  live upstream: backlog-first ordering, and the choice that a spec-driven
  item's body is one fenced `zti` requirements block. The body rule is
  enforced by *devloop*, the project that owns that format — this checker only
  decides which items are subject to it and reports what devloop refused.

Adding this checker rather than extending an existing one is deliberate: the
work-item store is a new mechanism with its own execution boundary (an external
binary) and its own inputs, which is the case ``AGENTS.md`` reserves a
top-level checker for.
"""

from __future__ import annotations

import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import json
import re
import shutil
import subprocess
import tempfile
from unittest.mock import patch

from harness import ROOT
import devloop
import work_items
from work_items import ITEMS, identities, items, open_backlog
from work_item_retirement import RetirementError
from work_item_retirement_controls import check_retirement_controls
from work_item_retirement_publish_controls import (
    check_controls as check_retirement_publish_controls,
)


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


def spec_driven() -> list[dict[str, object]]:
    """Items that carry a devloop record, and are therefore spec-driven.

    The record is what admits an item to the convention. An item without one is
    ordinary work — legacy `work-item/v1` prose and plain-body v2 items both
    stay valid — so this deliberately does not guess from the body.
    """
    return [item for item in items() if "devloop" in item["consumers"]]


def check_devloop_bodies() -> list[str]:
    """Every spec-driven item's body and recorded semantics, per devloop.

    Slime OS chose `fenced-zti/v1`; devloop owns what that means and whether a
    payload, digest, profile, or helper identity is acceptable. A retired item
    has no body to validate — its requirements live in retained history — so
    only active records are subject.
    """
    subject = [item for item in spec_driven() if not item["retired"]]
    if not subject:
        return []
    findings = devloop.check_toolchain()
    if findings:
        return findings
    if not devloop.POLICY.is_file():
        return [
            f"{devloop.POLICY.relative_to(ROOT)} is missing: a spec-driven item cannot be "
            "validated without this repository's gate policy"
        ]
    for item in subject:
        identity = str(item["id"])
        try:
            devloop.validate(identity)
        except devloop.DevloopError as error:
            findings.extend(
                f"devloop {identity}: {line}" for line in str(error).splitlines() if line.strip()
            )
    return findings


def check_terminal_records() -> list[str]:
    """A retired item must still say, readably, which terminal state it reached."""
    return work_items.corrupt_terminal_records()


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
        # MyQue writes the envelope version it currently publishes, so the
        # refusal is exercised by replacing whatever version it wrote.
        dependency.write_bytes(
            re.sub(rb"schema: work-item/v\d+", b"schema: invalid/v1", original, count=1)
        )
        if not any(
            "unknown schema version: invalid/v1" in finding for finding in run_myque_check(root)
        ):
            fail("control: invalid store schema did not report the schema refusal")


def check_profile_controls() -> None:
    """Prove the body convention refuses its negative cases, offline.

    These run devloop's own refusals over the admitted item rather than
    asserting message text here: the profile belongs to devloop, so a control
    that re-encoded the rule would drift from it. None of them reach native
    validation, so the controls cost no compilation.

    A retired item has no body to mutate, so the controls need an active
    spec-driven item and report nothing when the store holds none. The summary
    line says how many were validated, so an empty run is visible rather than
    silently green.
    """
    active = [item for item in spec_driven() if not item["retired"]]
    if not active:
        return
    admitted = active[0]
    binary = shutil.which("myque")
    if binary is None:
        return
    finished = subprocess.run(
        [binary, "api", "get", str(admitted["id"])], cwd=ROOT, capture_output=True, text=True
    )
    if finished.returncode:
        fail(f"control: cannot read the admitted item: {finished.stderr.strip()}")
        return
    item = json.loads(finished.stdout)

    def refuses(name: str, mutate) -> None:
        payload = json.loads(finished.stdout)
        mutate(payload)
        try:
            devloop.render(payload)
        except devloop.DevloopError:
            return
        fail(f"control: devloop accepted {name}")

    if devloop.render(item).strip() == "":
        fail("control: devloop rendered the admitted item as nothing")
    refuses("a duplicated requirements block", lambda p: p.update(body=p["body"] + p["body"]))
    refuses("a missing requirements block", lambda p: p.update(body="\nOrdinary prose.\n"))
    refuses(
        "editable requirements prose beside the block",
        lambda p: p.update(body=p["body"] + "\nAlso required: something else.\n"),
    )
    refuses(
        "an unknown body profile",
        lambda p: p["consumers"].update(
            devloop=p["consumers"]["devloop"].replace("fenced-zti/v1", "fenced-zti/v9")
        ),
    )
    refuses(
        "a payload that no longer matches its digest",
        lambda p: p.update(body=p["body"].replace('problem = "', 'problem = "tampered ', 1)),
    )


def check_terminal_controls() -> None:
    """A corrupt or non-terminal record is reported, never counted as done."""
    with tempfile.TemporaryDirectory(prefix="terminal-controls-") as temporary:
        terminal = _Path(temporary) / "terminal"
        terminal.mkdir()
        identity = "00000000-0000-7000-8000-00000000000a"
        cases = {
            "not JSON": "{",
            "wrong schema": json.dumps({"schema": "terminal-item/v9", "metadata": ""}),
            "no metadata": json.dumps({"schema": "terminal-item/v1"}),
            "still open": json.dumps(
                {
                    "schema": "terminal-item/v1",
                    "metadata": f"---\nschema: work-item/v2\nid: {identity}\nkind: task\n"
                    "state: open\ncreated: 2026-09-15T10:00:00Z\n---\n\n# Control\n",
                }
            ),
        }
        for name, content in cases.items():
            (terminal / f"{identity}.json").write_text(content)
            with patch.object(work_items, "TERMINAL", terminal):
                work_items.cache_clear()
                try:
                    if not work_items.corrupt_terminal_records():
                        fail(f"control: terminal record that is {name} was accepted")
                    if work_items.done(identity):
                        fail(f"control: terminal record that is {name} was counted as done")
                    if identity in work_items.identities():
                        fail(f"control: terminal record that is {name} claimed an identity")
                finally:
                    work_items.cache_clear()

        # A well-formed cancelled record resolves as identity, but is not done.
        (terminal / f"{identity}.json").write_text(
            json.dumps(
                {
                    "schema": "terminal-item/v1",
                    "metadata": f"---\nschema: work-item/v2\nid: {identity}\nkind: task\n"
                    "state: cancelled\ncreated: 2026-09-15T10:00:00Z\n"
                    "closed: 2026-09-16T10:00:00Z\n---\n\n# Control\n",
                }
            )
        )
        with patch.object(work_items, "TERMINAL", terminal):
            work_items.cache_clear()
            try:
                if work_items.corrupt_terminal_records():
                    fail("control: a valid cancelled terminal record was reported as corrupt")
                if identity not in work_items.identities():
                    fail("control: a retired identity did not resolve")
                if work_items.done(identity):
                    fail("control: a cancelled item was counted as done")
            finally:
                work_items.cache_clear()


def main() -> int:
    if not ITEMS.is_dir():
        raise SystemExit(
            f"{ITEMS.relative_to(ROOT)} does not exist: it is the repository's only "
            "work-item authority and nothing reconstructs it"
        )
    check_controls()
    check_terminal_controls()
    failures.extend(check_retirement_controls())
    try:
        check_retirement_publish_controls()
    except RetirementError as error:
        fail(f"retirement publication control: {error}")
    failures.extend(run_myque_check())
    failures.extend(check_backlog_first())
    failures.extend(check_terminal_records())
    failures.extend(check_devloop_bodies())
    check_profile_controls()

    if failures:
        for failure in failures:
            print(f"work items: {failure}")
        raise SystemExit(f"work-item check failed with {len(failures)} problem(s)")

    total = len(identities())
    validated = len([item for item in spec_driven() if not item["retired"]])
    print(f"work-item check passed: {total} items, {validated} validated through devloop")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
