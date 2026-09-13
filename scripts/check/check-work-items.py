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

# The backlog index parses as a sequence of heading-delimited sections, never
# as a set keyed by `B<N>`: `B29` and `B30` were each allocated twice, so two
# distinct entries share a display key. Keying anything by the heading text
# would let the first `B29` stand in for the second's evidence — a non-unique
# human key silently merging two records is precisely the failure this cutover
# removed. A section is identified by its position.
HEADING = re.compile(r"^### (B\d+) — (.*)$", re.MULTILINE)
ITEM_LINE = re.compile(r"^\*\*Evidence:\*\* .+? \*\*Item:\*\* `([0-9a-f-]{36})`$", re.MULTILINE)

# The `### B<N>` headings the cutover froze. Frozen means the set does not
# shrink: each is a live link target for merged devlog entries, and removing
# one breaks an inbound URL with no other symptom. It is not a ceiling — an
# older pre-cutover heading may still be recovered — so only a shortfall
# fails.
LANDED_BACKLOG_HEADINGS = 94

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
    """Every heading in the frozen backlog index names an item that exists.

    The backlog file is a frozen index, not a record: devlog entries link into
    it and several of those links are anchored at a specific heading, which is
    why the headings survive. A heading is checked for exactly one thing —
    that the UUID beside it is still an item — because that is what keeps the
    index a usable route into the store.

    Deliberately *not* checked: the item's state. Freezing the index must not
    freeze the store. If a defect indexed here is legitimately reopened under
    the same UUID, the index still routes to it correctly, and whether that
    reopening blocks milestone work is ``check_backlog_first()``'s question.
    Requiring a terminal state here would let a historical document veto a
    state transition in the canonical store, which inverts the authority this
    cutover established.
    """
    if not BACKLOG.is_file():
        fail(f"{BACKLOG.relative_to(ROOT)} is missing: devlog entries link into it")
        return
    text = BACKLOG.read_text()
    known = identities()
    matches = list(HEADING.finditer(text))
    for ordinal, match in enumerate(matches, start=1):
        key = match.group(1)
        # The section runs to the next heading, so each duplicate key is
        # checked against its own evidence. `where` names the ordinal as well
        # as the key: with two `B29` sections, the key alone cannot say which
        # one failed.
        end = matches[ordinal].start() if ordinal < len(matches) else len(text)
        section = text[match.end() : end]
        where = f"{key} (heading {ordinal} of {len(matches)})"
        found = ITEM_LINE.findall(section)
        if not found:
            fail(f"backlog index: {where} has no `**Evidence:** … **Item:** `<uuid>`` line")
            continue
        if len(found) > 1:
            fail(f"backlog index: {where} names {len(found)} items; a heading indexes exactly one")
            continue
        uuid = found[0]
        if uuid not in known:
            fail(f"backlog index: {where} names {uuid}, which is not in {ITEMS.relative_to(ROOT)}")

    # A per-section pass cannot notice wholesale deletion: no sections means no
    # failures. The landed set is pinned so bulk removal fails loudly; inbound
    # anchors are what make each heading load-bearing, and `check-devlog.py`
    # validates those fragments.
    if len(matches) < LANDED_BACKLOG_HEADINGS:
        fail(
            f"backlog index: {len(matches)} `### B<N>` heading(s), fewer than the "
            f"{LANDED_BACKLOG_HEADINGS} that landed; a heading is a frozen link target "
            "and devlog entries are anchored at them"
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
    indexed = len(HEADING.findall(BACKLOG.read_text())) if BACKLOG.is_file() else 0
    print(f"work-item check passed: {total} items, {indexed} frozen backlog headings indexed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
