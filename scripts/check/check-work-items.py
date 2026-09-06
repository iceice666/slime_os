#!/usr/bin/env python3

"""Validate the canonical work-item store and the repository's policy over it.

Two responsibilities, deliberately in one checker rather than two:

* **Store validity** is MyQue's own. This runs ``myque check``, which owns
  identity (UUID form, UUIDv7, duplicates, filename/id agreement), alias
  uniqueness, dangling references, parent and dependency cycles,
  state/``closed`` consistency, and schema conformance. Reimplementing any of
  that here would fork the schema, which the migration explicitly does not do.

* **Repository policy** is Slime OS's own and MyQue is generic, so it cannot
  live upstream: backlog-first ordering, and the invariants that make a
  migrated item auditable. These are consumers of the store, not extensions of
  ``work-item/v1``.

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

import shutil
import subprocess

from harness import ROOT
from work_items import ITEMS, LEGACY_MAP, identities, items, legacy, open_backlog

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

    ``AGENTS.md``: "Resolve open backlog items before starting a new roadmap
    track milestone. A green verification suite is a precondition for milestone
    work, not a milestone itself." ``deferred`` is the explicit deferral the
    rule allows, so it satisfies the rule rather than breaking it.
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


def check_migration_auditability() -> None:
    """Every legacy id maps to an item that exists, and collisions stay keyless.

    The legacy map is how 288 pre-migration devlog entries keep resolving. If
    it names an id that no longer exists, those references break silently, so
    the map is checked against the store rather than trusted.
    """
    if not LEGACY_MAP.is_file():
        fail(f"{LEGACY_MAP.relative_to(ROOT)} is missing: historical devlog references cannot resolve")
        return
    table = legacy()
    known = identities()
    for identifier, uuid_text in sorted(table["ids"].items()):
        if uuid_text not in known:
            fail(f"legacy id {identifier} maps to {uuid_text}, which is not in {ITEMS.relative_to(ROOT)}")

    keys = {item["key"] for item in items() if item["key"]}
    for identifier, record in sorted(table["ambiguous"].items()):
        if identifier in keys:
            fail(
                f"{identifier} was allocated twice, so it must not be carried as a key: "
                "the UUID distinguishes the two items"
            )
        for allocation in record["allocations"]:
            if allocation["uuid"] not in known:
                fail(f"{identifier}: allocation {allocation['uuid']} is not in the store")
        for reference in record["devlog_references"]:
            entry = ROOT / reference["entry"]
            if not entry.is_dir():
                fail(f"{identifier}: resolved reference names {reference['entry']}, which does not exist")
            if reference["uuid"] not in known:
                fail(f"{identifier}: resolved reference names {reference['uuid']}, which is not in the store")


def check_undated_closures_declare_themselves() -> None:
    """An *imported* closure with no dated evidence says so in its body.

    The repository's rule is that a milestone closes only when its exit
    condition was observed. 34 migrated items have no date anywhere in the
    repository, so their ``closed`` timestamp is the migration date. That is
    defensible only while each one admits it: an unlabelled synthetic date
    reads as an observation.

    Scoped to items the migration created, identified through the legacy map.
    Work genuinely closed on the migration date is an observed closure and
    needs no disclaimer — flagging it would be a false positive that pressures
    the next author into writing a disclaimer that is not true.
    """
    migrated = legacy().get("migrated")
    if migrated is None:
        return
    imported = set(legacy()["ids"].values())
    for record in legacy()["ambiguous"].values():
        imported.update(allocation["uuid"] for allocation in record["allocations"])
    stamp = f"closed: {migrated}T"
    for path in sorted(ITEMS.glob("*.md")):
        if path.stem not in imported:
            continue
        text = path.read_text()
        if stamp not in text:
            continue
        if "The original closure date is unrecorded" not in text:
            fail(
                f"{path.name}: closed on the migration date without saying the original "
                "date is unrecorded, which reads as an observed closure"
            )


def main() -> int:
    if not ITEMS.is_dir():
        raise SystemExit(f"{ITEMS.relative_to(ROOT)} does not exist; run scripts/migrate-roadmap-to-myque.py")
    run_myque_check()
    check_backlog_first()
    check_migration_auditability()
    check_undated_closures_declare_themselves()

    if failures:
        for failure in failures:
            print(f"work items: {failure}")
        raise SystemExit(f"work-item check failed with {len(failures)} problem(s)")

    total = len(identities())
    mapped = len(legacy()["ids"])
    print(f"work-item check passed: {total} items, {mapped} legacy ids mapped")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
