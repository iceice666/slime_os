"""The canonical MyQue work-item store, as repository checks see it.

Identity lives in ``.tasks/items/<uuid>.md``. ``myque check`` owns schema,
graph, and alias validation; this module owns only the question repository
checks actually ask — *is this UUID a real work item?* — so it reads filenames
rather than reimplementing MyQue's parser. The specification requires the
filename to equal the canonical id and makes any disagreement a finding, so the
filename set *is* the identity set.

Human keys such as ``C9.4`` are display aliases and deliberately do not
resolve here. A key can be renamed or dropped without touching a reference, and
that only holds while nothing durable depends on one. Callers match ``UUID``
and test membership in ``identities()`` directly: the two failures are
distinct — a malformed reference and an absent item — and a resolver that
returned one answer for both would hide which happened.
"""

from __future__ import annotations

import re
from functools import lru_cache

from harness import ROOT

TASKS = ROOT / ".tasks"
ITEMS = TASKS / "items"

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
    """Backlog items that are neither closed, deferred, nor externally blocked.

    The repository's standing rule (``AGENTS.md``) is that these are cleared or
    explicitly deferred before a new track milestone opens. ``deferred`` is that
    explicit deferral, so it satisfies the rule rather than violating it.

    ``blocked`` also satisfies it, for a different reason: it means the item
    waits on something outside this repository, which no amount of milestone
    sequencing resolves. Counting it as blocking would wedge every milestone
    behind work nobody here can start.
    """
    return [
        item
        for item in items()
        if "backlog" in item["tags"] and item["state"] in {"open", "active"}
    ]
