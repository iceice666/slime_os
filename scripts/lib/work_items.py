"""The canonical MyQue work-item store, as repository checks see it.

Identity lives in two places since MyQue `work-item/v2`. Active work is
``.tasks/items/<uuid>.md``; a done or cancelled item may be *retired* to
``.tasks/terminal/<uuid>.json``, a minimal record MyQue keeps after the item's
exact final bytes are retained in Git. Both are canonical identity, so a check
that read only the active directory would report a retired UUID as absent and a
retired dependency as unknown. ``myque check`` still owns schema, graph, and
alias validation; this module owns only the question repository checks actually
ask — *is this UUID a real work item, and what is its state?*

Active items are read by filename, because the specification requires the
filename to equal the canonical id and makes any disagreement a finding. A
terminal record is read as JSON, because its state and title live inside it;
a record that does not decode is reported as corrupt rather than treated as
done, since "this item finished" is exactly the claim a broken file cannot
support.

Human keys such as ``C9.4`` are display aliases and deliberately do not
resolve here. A key can be renamed or dropped without touching a reference, and
that only holds while nothing durable depends on one. Callers match ``UUID``
and test membership in ``identities()`` directly: the two failures are
distinct — a malformed reference and an absent item — and a resolver that
returned one answer for both would hide which happened.
"""

from __future__ import annotations

import json
import re
from functools import lru_cache
from pathlib import Path

from harness import ROOT

TASKS = ROOT / ".tasks"
ITEMS = TASKS / "items"
TERMINAL = TASKS / "terminal"

UUID = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")

# Frontmatter fields the policy checks read. Everything else about an item is
# MyQue's business, and `myque check` is what validates it.
FIELD = {
    name: re.compile(rf"^{name}: (?P<value>.+)$", re.MULTILINE)
    for name in ("key", "kind", "state")
}
TAGS = re.compile(r"^tags:\n((?:  - .+\n)+)", re.MULTILINE)
# A consumer's frontmatter record starts at an unquoted top-level namespace.
CONSUMER = re.compile(r"^(?P<name>[a-z0-9_-]+):$", re.MULTILINE)

# The terminal schema devloop-independent consumers may rely on. MyQue owns it;
# this module reads only the envelope fields a policy check needs.
TERMINAL_SCHEMA = "terminal-item/v1"


def _record(text: str, *, identity: str, retired: bool, source: Path) -> dict[str, object]:
    tags = TAGS.search(text)
    return {
        "id": identity,
        "key": (match.group("value") if (match := FIELD["key"].search(text)) else None),
        "kind": (match.group("value") if (match := FIELD["kind"].search(text)) else None),
        "state": (match.group("value") if (match := FIELD["state"].search(text)) else None),
        "tags": [line.removeprefix("  - ") for line in tags.group(1).splitlines()] if tags else [],
        "retired": retired,
        "consumers": sorted(
            name
            for name in CONSUMER.findall(text.split("\n---", 2)[0])
            if name not in {"tags", "depends", "blocks", "related", "supersedes"}
        ),
        "path": source,
    }


@lru_cache(maxsize=1)
def _terminal() -> tuple[list[dict[str, object]], list[str]]:
    """Every retired item, plus one finding per record that does not decode."""
    found: list[dict[str, object]] = []
    corrupt: list[str] = []
    if not TERMINAL.is_dir():
        return found, corrupt
    for path in sorted(TERMINAL.glob("*.json")):
        relative = path.relative_to(ROOT) if path.is_relative_to(ROOT) else path
        try:
            record = json.loads(path.read_text())
        except (OSError, ValueError) as error:
            corrupt.append(f"{relative}: terminal record does not decode: {error}")
            continue
        if not isinstance(record, dict) or record.get("schema") != TERMINAL_SCHEMA:
            corrupt.append(f"{relative}: terminal record is not {TERMINAL_SCHEMA}")
            continue
        metadata = record.get("metadata")
        if not isinstance(metadata, str):
            corrupt.append(f"{relative}: terminal record carries no item metadata")
            continue
        item = _record(metadata, identity=path.stem, retired=True, source=path)
        # A retired record must still say which terminal state it reached; an
        # unreadable or non-terminal state is not "done".
        if item["state"] not in {"done", "cancelled"}:
            corrupt.append(
                f"{relative}: terminal record state {item['state']!r} is not done or cancelled"
            )
            continue
        found.append(item)
    return found, corrupt


@lru_cache(maxsize=1)
def identities() -> frozenset[str]:
    """Every canonical work-item id in the store, active or retired."""
    retired, _ = _terminal()
    return frozenset(path.stem for path in ITEMS.glob("*.md")) | frozenset(
        str(item["id"]) for item in retired
    )


@lru_cache(maxsize=1)
def items() -> list[dict[str, object]]:
    """Every item's id, key, kind, state, tags, consumer namespaces, and origin.

    Deliberately shallow: this exists for repository *policy* checks, such as
    "is an open backlog defect blocking milestone work", not to re-validate
    what ``myque check`` already validates. ``retired`` distinguishes a live
    file from a terminal record, because a retired item has no body to read.
    """
    found = [
        _record(path.read_text(), identity=path.stem, retired=False, source=path)
        for path in sorted(ITEMS.glob("*.md"))
    ]
    records, _ = _terminal()
    found.extend(records)
    return sorted(found, key=lambda item: str(item["id"]))


@lru_cache(maxsize=1)
def retired() -> frozenset[str]:
    """Ids whose body left the active tree for a terminal record."""
    records, _ = _terminal()
    return frozenset(str(item["id"]) for item in records)


def corrupt_terminal_records() -> list[str]:
    """Terminal records that cannot be read as a finished item."""
    _, corrupt = _terminal()
    return list(corrupt)


def done(identity: str) -> bool:
    """Whether this id names an item MyQue reports as done, retired or not.

    Cancelled, unknown, and corrupt identities are not done: the first was
    abandoned, and the other two carry no claim at all.
    """
    return any(item["id"] == identity and item["state"] == "done" for item in items())


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


def cache_clear() -> None:
    """Drop every memoized read of the store, for checks that mutate a fixture."""
    _terminal.cache_clear()
    identities.cache_clear()
    items.cache_clear()
    retired.cache_clear()
