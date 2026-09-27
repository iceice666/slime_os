"""Report typed observations from a checker to the `just-observations` gate.

A checker that runs under `just devloop gate … just-observations` is told the
recipe it is answering for through `DEVLOOP_JUST_TARGET`; outside the gate the
variable is unset and `record` is a no-op, so a checker reports the same way
whether a person or devloop invoked it. The gate adapter deletes the report
before the recipe starts and reads it after, so a report can only come from
the run whose exit code it accompanies.

Values are the three kinds devloop predicates read: `bool`, `int` (signed
64-bit) and `str`. Identifiers must be observations `.devloop/policy.json`
declares for the gate; the adapter refuses an undeclared or mistyped one as a
gate that could not run, never as evidence.
"""

from __future__ import annotations

import json
import os
from pathlib import Path

from harness import ROOT

ENVIRONMENT = "DEVLOOP_JUST_TARGET"
REPORTS = ROOT / "build" / "devloop-observations"
INT_BOUND = 1 << 63


def report_path(target: str) -> Path:
    if not target or "/" in target or target.startswith("."):
        raise ValueError(f"not a recipe name: {target!r}")
    return REPORTS / f"{target}.json"


def encode(values: dict[str, object]) -> dict[str, object]:
    """Validate a report's shape without knowing the policy declarations."""
    encoded: dict[str, object] = {}
    for identifier, value in values.items():
        if not isinstance(identifier, str) or not identifier.isidentifier():
            raise ValueError(f"observation id must be an identifier: {identifier!r}")
        # bool before int: bool is an int subclass and must stay a bool.
        if isinstance(value, bool) or isinstance(value, str):
            encoded[identifier] = value
        elif isinstance(value, int):
            if not -INT_BOUND <= value < INT_BOUND:
                raise ValueError(f"observation {identifier} is outside signed 64-bit range")
            encoded[identifier] = value
        else:
            raise ValueError(
                f"observation {identifier} must be bool, int, or str, not {type(value).__name__}"
            )
    return encoded


def record(**values: object) -> Path | None:
    """Write the observations for the recipe this process answers, if any.

    Returns the report path, or None when not running under the gate. Calling
    it twice in one run replaces the earlier report: a checker reports once,
    at the end, from what it actually observed.
    """
    target = os.environ.get(ENVIRONMENT)
    if not target:
        return None
    path = report_path(target)
    path.parent.mkdir(parents=True, exist_ok=True)
    partial = path.with_suffix(".json.partial")
    partial.write_text(json.dumps(encode(values), sort_keys=True) + "\n")
    partial.replace(path)
    return path
