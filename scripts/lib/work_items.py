"""The canonical MyQue work-item store, as repository checks see it.

Identity lives in ``.tasks/items/<uuid>.md``. ``myque check`` owns schema,
graph, and alias validation; this module owns only the question repository
checks actually ask — *does this reference name a real work item?* — so it
reads filenames rather than reimplementing MyQue's parser. The specification
requires the filename to equal the canonical id and makes any disagreement a
finding, so the filename set *is* the identity set.

Legacy roadmap ids resolve through ``.tasks/legacy-roadmap-ids.json``, which
the migration wrote once. That file is committed data, not a parse of
``roadmap/*.md``: it lets the 288 devlog entries that predate the migration
keep resolving without roadmap headings remaining authoritative. An id absent
from the map does not resolve — there is no fallback to scanning headings.
"""

from __future__ import annotations

import json
import re
from functools import lru_cache

from harness import ROOT

TASKS = ROOT / ".tasks"
ITEMS = TASKS / "items"
LEGACY_MAP = TASKS / "legacy-roadmap-ids.json"

UUID = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")

# Frontmatter fields the policy checks read. Everything else about an item is
# MyQue's business, and `myque check` is what validates it.
FIELD = {
    name: re.compile(rf"^{name}: (?P<value>.+)$", re.MULTILINE)
    for name in ("key", "kind", "state")
}
TAGS = re.compile(r"^tags:\n((?:  - .+\n)+)", re.MULTILINE)


@lru_cache(maxsize=1)
def identities() -> frozenset[str]:
    """Every canonical work-item id in the store."""
    return frozenset(path.stem for path in ITEMS.glob("*.md"))


@lru_cache(maxsize=1)
def legacy() -> dict:
    """The committed legacy-id map, or an empty map before the migration."""
    if not LEGACY_MAP.is_file():
        return {"ids": {}, "ambiguous": {}}
    return json.loads(LEGACY_MAP.read_text())


def resolve(reference: str, *, entry: str | None = None) -> str | None:
    """The canonical id a devlog reference names, or ``None``.

    A UUID resolves directly. A legacy roadmap id resolves through the map. An
    id that was allocated twice resolves only for an entry whose target the
    migration determined from evidence — never by picking an allocation.
    """
    if UUID.match(reference):
        return reference if reference in identities() else None
    table = legacy()
    ambiguous = table["ambiguous"].get(reference)
    if ambiguous is not None:
        for known in ambiguous["devlog_references"]:
            if entry is not None and known["entry"].endswith(entry):
                return known["uuid"]
        return None
    return table["ids"].get(reference)


def ambiguous_ids() -> frozenset[str]:
    """Legacy ids that name more than one historical item."""
    return frozenset(legacy()["ambiguous"])


@lru_cache(maxsize=1)
def items() -> list[dict[str, object]]:
    """Every item's id, key, kind, state, and tags.

    Deliberately shallow: this exists for repository *policy* checks, such as
    "is an open backlog defect blocking milestone work", not to re-validate
    what ``myque check`` already validates.
    """
    found: list[dict[str, object]] = []
    for path in sorted(ITEMS.glob("*.md")):
        text = path.read_text()
        tags = TAGS.search(text)
        found.append(
            {
                "id": path.stem,
                "key": (match.group("value") if (match := FIELD["key"].search(text)) else None),
                "kind": (match.group("value") if (match := FIELD["kind"].search(text)) else None),
                "state": (match.group("value") if (match := FIELD["state"].search(text)) else None),
                "tags": [line.removeprefix("  - ") for line in tags.group(1).splitlines()] if tags else [],
            }
        )
    return found


def open_backlog() -> list[dict[str, object]]:
    """Backlog items that are neither closed nor explicitly deferred.

    The repository's standing rule (``AGENTS.md``) is that these are cleared
    or explicitly deferred before a new track milestone opens. ``deferred`` is
    the explicit deferral, so it satisfies the rule rather than violating it.
    """
    return [
        item
        for item in items()
        if "backlog" in item["tags"] and item["state"] in {"open", "active", "blocked"}
    ]
