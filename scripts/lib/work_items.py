"""Work identities after the living-spec devloop (proposal/devloop-redesign/).

The `.tasks/` store is retired. Two identity families remain and both are
UUIDv7: a legacy work item, indexed in `specs/legacy.zt` with the state it
froze in, and a requirement inside a `specs/<area>.zt` area, whose state is
`open` while `planned` and `done` otherwise. (`done` here is the narrow
sense the smoltcp matrix and the test-run inventory need — bound to an
exam — not an enforced record; `scripts/spec/status` computes that.)

Callers match ``UUID`` before asking ``identities()`` or ``state()``; the
state table is cached, and ``cache_clear()`` drops it after a test rewrote
the spec root (`spec_lib.SPECS`).
"""

from __future__ import annotations

import importlib.util
import re
from functools import lru_cache

from harness import ROOT

UUID = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
FINISHED = frozenset({"done", "cancelled", "retired"})
_SPEC_LIB = ROOT / "scripts" / "spec" / "lib.py"


def spec_lib():
    """`scripts/spec/lib`, loaded by path so its module name cannot collide."""
    import sys

    if "spec_lib" not in sys.modules:
        specification = importlib.util.spec_from_file_location("spec_lib", _SPEC_LIB)
        module = importlib.util.module_from_spec(specification)
        sys.modules["spec_lib"] = module
        specification.loader.exec_module(module)
    return sys.modules["spec_lib"]


@lru_cache(maxsize=1)
def _states() -> dict[str, str]:
    lib = spec_lib()
    states: dict[str, str] = {}
    legacy = lib.SPECS / "legacy.zt"
    if legacy.is_file():
        for row in lib.zutai_json(legacy):
            states[row["id"]] = row["state"]
    for area in lib.load_areas():
        for requirement in area.requirements:
            states[requirement["id"]] = "open" if requirement["planned"] else "done"
    return states


def identities() -> frozenset[str]:
    """Every identity a document may reference: legacy items and requirements."""
    return frozenset(_states())


def state(identity: str) -> str | None:
    return _states().get(identity)


def planned(identity: str) -> bool:
    """True for a requirement that is promised but not yet bound to an exam."""
    return _states().get(identity) == "open" and identity not in legacy_ids()


@lru_cache(maxsize=1)
def legacy_ids() -> frozenset[str]:
    lib = spec_lib()
    legacy = lib.SPECS / "legacy.zt"
    return frozenset(row["id"] for row in lib.zutai_json(legacy)) if legacy.is_file() else frozenset()


def items() -> list[dict[str, object]]:
    return [{"id": identity, "state": value} for identity, value in _states().items()]


def cache_clear() -> None:
    _states.cache_clear()
    legacy_ids.cache_clear()
