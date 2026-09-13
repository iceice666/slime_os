"""The declared `fabricGraph.limits` block of a generation fixture.

Shared by the traffic and saturation gates so both parse the one grammar
through one implementation. Two hand-maintained copies of a regex over a Zutai
file diverge silently the first time the fixture's formatting changes -- a
limits block at a different indent makes the closing-brace search miss, and
only the gate that happened to be edited notices.

This is a reader for gate assertions, not a Zutai parser. The authoritative
decode is `boot-contracts/src/fabric_graph.rs`, against the encoded generation;
what a gate needs is the number the fixture *declared*, so that tightening or
loosening the fixture moves the assertion with it instead of leaving a restated
constant behind.
"""

from __future__ import annotations

import re
from pathlib import Path

_LIMIT_FIELD = re.compile(r"^\s*(\w+) = (\d+);\s*$", re.MULTILINE)


def declared_limits(fixture: Path, overrides: dict[str, int] | None = None) -> dict[str, int]:
    """`fixture`'s own `fabricGraph.limits` block, as declared integers.

    Raises `ValueError` when the block is absent or unterminated, rather than
    returning an empty mapping: a gate that read no limits would silently skip
    every ceiling assertion built on them.

    `overrides` applies the same per-variant deltas `build-sel4.py` supplies
    through `SLIME_FABRIC_LIMIT_OVERRIDE` (B62). A variant that narrows one
    ceiling used to be a whole second fixture; now the delta is declared once in
    `VARIANT_GENERATION_DELTAS`, and a gate asserting against that variant must
    read the same narrowed value the image was built with. Overriding a limit the
    fixture does not declare is refused, so a typo cannot introduce a ceiling
    nothing bounds.
    """
    text = fixture.read_text(encoding="utf-8")
    try:
        start = text.index("limits = {")
        end = text.index("\n    };", start)
    except ValueError as error:
        raise ValueError(f"{fixture}: no terminated fabricGraph.limits block") from error
    limits = {name: int(value) for name, value in _LIMIT_FIELD.findall(text[start:end])}
    if not limits:
        raise ValueError(f"{fixture}: fabricGraph.limits block declares no integer fields")
    for name, value in (overrides or {}).items():
        if name not in limits:
            raise ValueError(f"{fixture}: cannot override undeclared limit {name!r}")
        limits[name] = value
    return limits
