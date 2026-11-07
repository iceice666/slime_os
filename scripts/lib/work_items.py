"""Work identities after the living-spec devloop (proposal/devloop-redesign/).

The `.tasks/` store is retired. Two identity families remain and both are
UUIDv7: a legacy work item, indexed in `specs/legacy.zt` with the state it
froze in, and a requirement inside a `specs/<area>.zt` area, whose state is
`open` while `planned` and `done` otherwise. (`done` here is the narrow
sense the smoltcp matrix and the test-run inventory need — bound to an
exam — not an enforced record; `scripts/spec/status` computes that.)

This reader is deliberately shallow and binary-free: `docs_check` runs
without `zutai-cli`. It reads the literals `h.req "<id>"`, `h.planned
"<id>"` and the legacy rows' `id = "..."; state = "..."` as text.
`scripts/spec/check` is the deep reader that evaluates the files; it
refuses an id these regexes would misread (not a UUIDv7, repeated).

Callers match ``UUID`` before asking ``identities()`` or ``state()``; the
state table is cached, and ``cache_clear()`` drops it after a test rewrote
the spec root (``SPECS``).
"""

from __future__ import annotations

import re
from functools import lru_cache

from harness import ROOT

SPECS = ROOT / "specs"
UUID = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
FINISHED = frozenset({"done", "cancelled", "retired"})
_REQUIREMENT = re.compile(r"\bh\.(req|planned)\s+\"([0-9a-f-]{36})\"")
_LEGACY_ROW = re.compile(r"\{\s*id = \"([0-9a-f-]{36})\";\s*state = \"([a-z]+)\";")
_HELPER_FILES = frozenset({"schema.zt", "helpers.zt", "legacy.zt"})


def spec_root():
    """The directory the reader scans; tests patch this module's `SPECS`."""
    return SPECS


@lru_cache(maxsize=1)
def _states() -> dict[str, str]:
    root = spec_root()
    states: dict[str, str] = {}
    legacy = root / "legacy.zt"
    if legacy.is_file():
        for identity, state in _LEGACY_ROW.findall(legacy.read_text(encoding="utf-8")):
            states[identity] = state
    for path in sorted(root.glob("*.zt")):
        if path.name in _HELPER_FILES:
            continue
        for kind, identity in _REQUIREMENT.findall(path.read_text(encoding="utf-8")):
            states[identity] = "open" if kind == "planned" else "done"
    return states


@lru_cache(maxsize=1)
def legacy_ids() -> frozenset[str]:
    legacy = spec_root() / "legacy.zt"
    if not legacy.is_file():
        return frozenset()
    return frozenset(identity for identity, _ in _LEGACY_ROW.findall(legacy.read_text(encoding="utf-8")))


def identities() -> frozenset[str]:
    """Every identity a document may reference: legacy items and requirements."""
    return frozenset(_states())


def state(identity: str) -> str | None:
    return _states().get(identity)


def planned(identity: str) -> bool:
    """True for a requirement that is promised but not yet bound to an exam."""
    return _states().get(identity) == "open" and identity not in legacy_ids()


def items() -> list[dict[str, object]]:
    return [{"id": identity, "state": value} for identity, value in _states().items()]


def cache_clear() -> None:
    _states.cache_clear()
    legacy_ids.cache_clear()
