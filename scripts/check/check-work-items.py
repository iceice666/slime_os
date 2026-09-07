#!/usr/bin/env python3

"""Validate the canonical work-item store and the repository's policy over it.

Two responsibilities, deliberately in one checker rather than two:

* **Store validity** is MyQue's own. This runs ``myque check``, which owns
  identity (UUID form, UUIDv7, duplicates, filename/id agreement), alias
  uniqueness, dangling references, parent and dependency cycles,
  state/``closed`` consistency, and schema conformance. Reimplementing any of
  that here would fork the schema, which the migration explicitly does not do.

* **Repository policy** is Slime OS's own and MyQue is generic, so it cannot
  live upstream: backlog-first ordering and the integrity of the frozen
  backlog index that 75 devlog entries link into. These are consumers of the
  store, not extensions of ``work-item/v1``.

Adding this checker rather than extending an existing one is deliberate: the
work-item store is a new mechanism with its own execution boundary (an external
binary) and its own inputs, which is the case ``AGENTS.md`` reserves a
top-level checker for. The devlog checker stays responsible for devlog
structure and for resolving the references an entry names.
"""

from __future__ import annotations

import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import re
import shutil
import subprocess

from harness import ROOT
from work_items import ITEMS, identities, items, open_backlog

BACKLOG = ROOT / "roadmap" / "00-backlog.md"

# A resolved backlog heading and the item id that carries its state. The
# heading is a stable anchor for devlog links; the id beside it is what makes
# the entry resolvable. The heading text resolves nothing and is never parsed
# for identity — `B29` and `B30` were each allocated twice, which is why an id
# is named per entry rather than derived from a key.
RESOLVED_ENTRY = re.compile(
    r"^### (?P<key>B\d+) — .+\n\n\*\*Evidence:\*\* .+? \*\*Item:\*\* `(?P<uuid>[0-9a-f-]{36})`$",
    re.MULTILINE,
)

failures: list[str] = []


def fail(message: str) -> None:
    failures.append(message)


def run_myque_check() -> None:
    """Delegate store validation to MyQue, which owns the schema."""
    binary = shutil.which("myque")
    if binary is None:
        raise SystemExit(
            "myque is not on PATH: enter the repository dev shell (`nix develop`), "
            "which provides it, or run `nix run github:mozufu/myque -- check`"
        )
    finished = subprocess.run([binary, "check"], cwd=ROOT, capture_output=True, text=True)
    if finished.returncode == 0:
        return
    # `myque check` distinguishes its failures by status: 1 is findings, 2 is a
    # missing tracker or a usage error. Reporting them identically would send a
    # reader looking for a broken item when the store is simply absent.
    label = "reported findings" if finished.returncode == 1 else "could not read the store"
    for line in (finished.stdout + finished.stderr).splitlines():
        if line.strip():
            fail(f"myque check {label}: {line.strip()}")


def check_backlog_first() -> None:
    """Backlog defects are cleared or explicitly deferred before track work.

    ``AGENTS.md``: "Resolve, defer, or block every open one before starting a
    new track milestone; a green verification suite is a precondition for
    milestone work, not a milestone itself." ``deferred`` and ``blocked`` are
    the explicit escapes the rule allows, so they satisfy it rather than
    breaking it.
    """
    blocking = open_backlog()
    active_milestones = [
        item
        for item in items()
        if item["kind"] == "milestone" and item["state"] == "active"
    ]
    if blocking and active_milestones:
        names = ", ".join(str(item["key"] or item["id"]) for item in blocking)
        opened = ", ".join(str(item["key"] or item["id"]) for item in active_milestones)
        fail(
            f"backlog-first: {opened} is active while backlog items are neither "
            f"resolved nor explicitly deferred: {names}"
        )


def check_backlog_index_resolves() -> None:
    """Every heading in the frozen backlog index names a closed item.

    The backlog file is a frozen index, not a record: 75 devlog entries link
    into it and 8 of those links are anchored at a specific heading, which is
    why the headings survive. Nothing new is required to appear there — a new
    backlog item is a store item and its devlog entry — but a landed heading's
    UUID must keep resolving, because a heading pointing at a deleted or
    reopened item is a dangling record with no visible symptom.
    """
    if not BACKLOG.is_file():
        fail(f"{BACKLOG.relative_to(ROOT)} is missing: 75 devlog entries link into it")
        return
    text = BACKLOG.read_text()
    known = identities()
    state = {item["id"]: item["state"] for item in items()}
    seen: set[str] = set()
    for match in RESOLVED_ENTRY.finditer(text):
        key, uuid = match.group("key"), match.group("uuid")
        if uuid not in known:
            fail(f"backlog index: {key} names {uuid}, which is not in {ITEMS.relative_to(ROOT)}")
            continue
        if state[uuid] not in {"done", "cancelled"}:
            fail(f"backlog index: {key} is under '## Resolved' but its item is {state[uuid]}")
        seen.add(uuid)

    headings = len(re.findall(r"^### B\d+ — ", text, re.MULTILINE))
    if headings != len(seen):
        fail(
            f"backlog index: {headings} resolved heading(s) but {len(seen)} resolve to an item; "
            "every heading needs its `**Item:** `<uuid>`` line"
        )


def main() -> int:
    if not ITEMS.is_dir():
        raise SystemExit(
            f"{ITEMS.relative_to(ROOT)} does not exist: it is the repository's only "
            "work-item authority and nothing reconstructs it"
        )
    run_myque_check()
    check_backlog_first()
    check_backlog_index_resolves()

    if failures:
        for failure in failures:
            print(f"work items: {failure}")
        raise SystemExit(f"work-item check failed with {len(failures)} problem(s)")

    total = len(identities())
    indexed = len(RESOLVED_ENTRY.findall(BACKLOG.read_text())) if BACKLOG.is_file() else 0
    print(f"work-item check passed: {total} items, {indexed} frozen backlog headings indexed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
